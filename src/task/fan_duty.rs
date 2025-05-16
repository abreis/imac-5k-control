use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal};
use esp_hal::{
    gpio,
    ledc::{self, LowSpeed, channel::ChannelIFace, timer::TimerIFace},
    peripheral::Peripheral,
    peripherals::LEDC,
    time,
};

const INITIAL_FAN_DUTY: u8 = 100;
pub type FanDutySignal = &'static signal::Signal<NoopRawMutex, u8>;

/// Initializes the fan PWM controller to be passed to the fan_duty task.
#[must_use]
pub fn init(
    peripheral: impl Peripheral<P = LEDC> + 'static,
    pin_fan_pwm: gpio::Output<'static>,
) -> (ledc::channel::Channel<'static, LowSpeed>, FanDutySignal) {
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

    let fanduty_signal = Box::leak(Box::new(signal::Signal::new()));
    (ledc_channel0, fanduty_signal)
}

#[embassy_executor::task]
pub async fn fan_duty(
    pwm_channel: ledc::channel::Channel<'static, LowSpeed>,
    signal: FanDutySignal,
) {
    loop {
        // Wait for a new duty cycle to be signalled.
        let new_fan_duty = signal.wait().await;
        pwm_channel.set_duty(new_fan_duty).unwrap(); // Does not fail if timer and channel are configured, and duty ∈ [0,100]
    }
}
