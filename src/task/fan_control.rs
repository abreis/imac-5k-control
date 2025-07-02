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

// How often to measure the fan's tachometer.
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
            pin_config: ledc::channel::config::PinConfig::PushPull,
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
        const PULSES_TO_MEASURE: u32 = 10;
        // 1/2 pulse width at 2200 rpms (2 pulses per revolution) is 6.2ms.
        // Any less than that and the pulse is likely a glitch.
        // 1/2 pulse width at 500 rpms (2 pulses per revolution) is 33ms.
        // Any more than that and the fan is probably stopped.
        const PULSE_MIN: Duration = Duration::from_millis(25);
        const PULSE_MAX: Duration = Duration::from_millis(132);

        enum PulseError {
            TooLong,
            TooShort,
        }

        let pulse_widths: Result<[Duration; 10], PulseError> = async {
            let mut pulse_widths: [Duration; 10] = [Duration::MIN; 10];

            // Measure 10 pulses.
            for pulse_slot in pulse_widths.iter_mut() {
                // Wait for a falling edge.
                with_timeout(PULSE_MAX, pin_fan_tachy.wait_for_falling_edge())
                    .await
                    .map_err(|_| PulseError::TooLong)?;
                let start_time = Instant::now();

                // Wait for the rising edge.
                with_timeout(PULSE_MAX, pin_fan_tachy.wait_for_rising_edge())
                    .await
                    .map_err(|_| PulseError::TooLong)?;

                let elapsed = start_time.elapsed();
                if elapsed < PULSE_MIN {
                    return Err(PulseError::TooShort);
                } else {
                    *pulse_slot = elapsed;
                }
            }

            Ok(pulse_widths)
        }
        .await;

        let rpm = match pulse_widths {
            // A pulse measurement timed out, report a zero duty.
            Err(PulseError::TooLong) => 0,
            // If pulse < PULSE_MIN, a zero duty might be wrong, should be no duty.
            Err(PulseError::TooShort) => continue 'tachy,

            Ok(widths) => {
                // Average the durations.
                let duration_total_us = widths.iter().map(Duration::as_micros).sum::<u64>();
                let duration_avg_us = duration_total_us / 10;

                // From a falling to a rising edge is 1/4 of the revolution length.
                60_000_000 / (duration_avg_us * 4)
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
