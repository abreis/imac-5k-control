use super::temp_sensor::TempSensorDynReceiver;
use crate::task::fan_control::fan_pid::FanPidController;
use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::{
    gpio,
    ledc::{self, LowSpeed, channel::ChannelIFace, timer::TimerIFace},
    peripherals::LEDC,
    time,
};

const INITIAL_FAN_DUTY: u8 = 100;
pub type FanDutyWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, u8, W>;
pub type FanDutyDynSender = watch::DynSender<'static, u8>;
pub type FanDutyDynReceiver = watch::DynReceiver<'static, u8>;

/// How often to measure the fan's tachometer.
const FAN_TACHY_MEASURE_INTERVAL: Duration = Duration::from_secs(20);

pub type FanTachyWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, u16, W>;
pub type FanTachyDynSender = watch::DynSender<'static, u16>;
pub type FanTachyDynReceiver = watch::DynReceiver<'static, u16>;

/// Initializes the fan PWM controller to be passed to the fan_duty task.
#[must_use]
pub fn init<const WATCHERS: usize>(
    ledc_peripheral: LEDC<'static>,
    pin_fan_pwm: gpio::Output<'static>,
) -> (
    ledc::channel::Channel<'static, LowSpeed>,
    FanDutyWatch<WATCHERS>,
    FanTachyWatch<WATCHERS>,
) {
    // LED Controller (LEDC) PWM setup.
    let mut ledc = ledc::Ledc::new(ledc_peripheral);
    ledc.set_global_slow_clock(ledc::LSGlobalClkSource::APBClk);

    // The timer needs to be 'static for the LEDC channel to also be 'static.
    let mut lstimer0 = ledc.timer::<ledc::LowSpeed>(ledc::timer::Number::Timer0);
    lstimer0
        .configure(ledc::timer::config::Config {
            duty: ledc::timer::config::Duty::Duty5Bit,
            clock_source: ledc::timer::LSClockSource::APBClk,
            frequency: time::Rate::from_khz(25),
        })
        .unwrap();
    let lstimer0 = Box::leak(Box::new(lstimer0));

    let mut ledc_channel0 = ledc.channel(ledc::channel::Number::Channel0, pin_fan_pwm);
    ledc_channel0
        .configure(ledc::channel::config::Config {
            timer: lstimer0,
            duty_pct: INITIAL_FAN_DUTY,
            drive_mode: esp_hal::gpio::DriveMode::PushPull,
        })
        .unwrap();

    let fanduty_watch = Box::leak(Box::new(watch::Watch::new()));
    let fanrpm_watch = Box::leak(Box::new(watch::Watch::new()));

    (ledc_channel0, fanduty_watch, fanrpm_watch)
}

#[embassy_executor::task]
pub async fn fan_duty(
    pwm_channel: ledc::channel::Channel<'static, LowSpeed>,
    mut fanduty_receiver: FanDutyDynReceiver,
) {
    loop {
        // Wait for a new duty cycle to be signalled.
        let new_fan_duty = fanduty_receiver.changed().await;
        pwm_channel.set_duty(new_fan_duty).unwrap(); // Does not fail if timer and channel are configured, and duty ∈ [0,100]
    }
}

#[embassy_executor::task]
pub async fn fan_tachy(
    mut pin_fan_tachy: gpio::Input<'static>,
    fantachy_sender: FanTachyDynSender,
) {
    'tachy: loop {
        Timer::after(FAN_TACHY_MEASURE_INTERVAL).await;

        // Measure 10 pulses to get a good average.
        const PULSES_TO_MEASURE: usize = 10;
        // We measure full pulse periods (falling edge to falling edge), where:
        // - 300 RPM -> 100ms period (2 pulses/rev)
        // - 2400 RPM -> 12.5ms period
        //
        // Keep a bit of room around this expected operating range.
        const PULSE_PERIOD_MIN: Duration = Duration::from_millis(10);
        const PULSE_PERIOD_MAX: Duration = Duration::from_millis(120);

        enum PulseError {
            TooLong,
            TooShort,
        }

        let pulse_periods: Result<[Duration; PULSES_TO_MEASURE], PulseError> = async {
            let mut pulse_periods: [Duration; PULSES_TO_MEASURE] =
                [Duration::MIN; PULSES_TO_MEASURE];

            // Measure 10 pulses.
            for pulse_slot in pulse_periods.iter_mut() {
                // Wait for a falling edge.
                with_timeout(PULSE_PERIOD_MAX, pin_fan_tachy.wait_for_falling_edge())
                    .await
                    .map_err(|_| PulseError::TooLong)?;
                let start_time = Instant::now();

                // Wait for the next falling edge to measure one full tach pulse period.
                with_timeout(PULSE_PERIOD_MAX, pin_fan_tachy.wait_for_falling_edge())
                    .await
                    .map_err(|_| PulseError::TooLong)?;

                let elapsed = start_time.elapsed();
                if elapsed < PULSE_PERIOD_MIN {
                    return Err(PulseError::TooShort);
                } else {
                    *pulse_slot = elapsed;
                }
            }

            Ok(pulse_periods)
        }
        .await;

        let rpm = match pulse_periods {
            // A pulse measurement timed out, report a zero duty.
            Err(PulseError::TooLong) => 0,
            // Too-short periods are likely glitches or invalid edges.
            Err(PulseError::TooShort) => continue 'tachy,

            Ok(periods) => {
                // Average the durations.
                let duration_total_us = periods.iter().map(Duration::as_micros).sum::<u64>();
                let duration_avg_us = duration_total_us / PULSES_TO_MEASURE as u64;

                // 2 pulses/revolution: RPM = 60s / (period * 2).
                60_000_000 / (duration_avg_us * 2)
            }
        };

        fantachy_sender.send(rpm as u16);
    }
}

/// Sets the fan duty based on the sensed temperature.
#[embassy_executor::task]
pub async fn fan_temp_control(
    fanduty_sender: FanDutyDynSender,
    mut tempsensor_receiver: TempSensorDynReceiver,
) {
    let mut pid_controller = FanPidController::new();

    loop {
        if let Ok(sensor_temp) = tempsensor_receiver.changed().await.temperature {
            let new_duty_cycle = pid_controller.update(sensor_temp);
            fanduty_sender.send(new_duty_cycle as u8);
        }
    }
}

mod fan_pid {
    // Default target temperature.
    const SETPOINT_TEMP_C: f32 = 65.0;

    // PID output is mapped to [-PID_SYMMETRIC_LIMIT, +PID_SYMMETRIC_LIMIT].
    // Actual fan duty cycle will be pid_output + FAN_DUTY_OFFSET.
    const PID_SYMMETRIC_LIMIT: f32 = 50.0;
    const FAN_DUTY_OFFSET: f32 = 50.0;

    // Controller gains.
    //
    // Goal: ensure fan reaches 100% duty at 85ºC.
    //    temp:  85º
    //   error: -15º
    //  p_gain:  15*2 = 30
    //    duty:  30+50 = 80%
    // Integral component takes the fan to the remaining 20%.
    const KP_GAIN: f32 = -2.0;
    const KI_GAIN: f32 = -0.2;

    // Limits for individual term contributions to the PID output.
    const P_TERM_CONTRIBUTION_LIMIT: f32 = 40.0;
    const I_TERM_CONTRIBUTION_LIMIT: f32 = 40.0;

    pub struct FanPidController(pid::Pid<f32>);

    impl FanPidController {
        /// Initializes the fan PID controller with pre-defined gains and limits.
        pub fn new() -> Self {
            let mut pid_controller = pid::Pid::new(SETPOINT_TEMP_C, PID_SYMMETRIC_LIMIT);

            pid_controller
                .p(KP_GAIN, P_TERM_CONTRIBUTION_LIMIT)
                .i(KI_GAIN, I_TERM_CONTRIBUTION_LIMIT);
            //  .d(KD_PARAM, D_TERM_CONTRIBUTION_LIMIT);

            Self(pid_controller)
        }

        /// Takes the current temperature measurement and returns the new fan duty cycle.
        pub fn update(&mut self, current_temp_c: f32) -> f32 {
            let control_signal = self.0.next_control_output(current_temp_c);

            // Apply offset to map to [0.0, 100.0].
            // We trust that `output_limit` will have it clamped.
            control_signal.output + FAN_DUTY_OFFSET
        }
    }
}
