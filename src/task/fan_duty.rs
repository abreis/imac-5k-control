use super::temp_sensor::TempSensorDynReceiver;
use crate::task::fan_duty::fan_pid::FanPidController;
use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};
use esp_hal::{
    gpio,
    ledc::{self, LowSpeed, channel::ChannelIFace, timer::TimerIFace},
    peripherals::LEDC,
    time,
};

const INITIAL_FAN_DUTY: u8 = 100;
pub type FanDutySignal<const W: usize> = &'static watch::Watch<NoopRawMutex, u8, W>;
pub type FanDutyDynSender = watch::DynSender<'static, u8>;
pub type FanDutyDynReceiver = watch::DynReceiver<'static, u8>;

/// Initializes the fan PWM controller to be passed to the fan_duty task.
#[must_use]
pub fn init<const WATCHERS: usize>(
    peripheral: LEDC<'static>,
    pin_fan_pwm: gpio::Output<'static>,
) -> (
    ledc::channel::Channel<'static, LowSpeed>,
    FanDutySignal<WATCHERS>,
) {
    // LED Controller (LEDC) PWM setup.
    let mut ledc = ledc::Ledc::new(peripheral);
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
    (ledc_channel0, fanduty_watch)
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
            let new_duty_cycle = libm::roundf(new_duty_cycle) as u8;
            fanduty_sender.send(new_duty_cycle);
        }
    }
}

mod fan_pid {
    // Default target temperature.
    const SETPOINT_TEMP_C: f32 = 70.0;

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
