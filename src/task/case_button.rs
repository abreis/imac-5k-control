use super::pin_control::PinControlMessage;
use crate::{
    memlog::SharedLogger,
    task::{
        buzzer::{BuzzerAction, BuzzerChannel, BuzzerPattern},
        pin_control::PinControlPublisher,
    },
};
use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};
use embassy_time::Duration;
use esp_hal::{gpio, time::Instant};

const BUTTON_HELD_DURATION_MIN: Duration = Duration::from_millis(500);
const BUTTON_HELD_DURATION_MAX: Duration = Duration::from_millis(1500);

const CASE_BUTTON_TONE: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 50 },
    BuzzerAction::Beep { ms: 100 },
];

pub type CaseButtonWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, Instant, W>;
pub type CaseButtonDynSender = watch::DynSender<'static, Instant>;
pub type CaseButtonDynReceiver = watch::DynReceiver<'static, Instant>;

pub fn init<const WATCHERS: usize>() -> CaseButtonWatch<WATCHERS> {
    Box::leak(Box::new(watch::Watch::new()))
}

#[embassy_executor::task]
pub async fn case_button(
    pin: gpio::AnyPin<'static>,
    pincontrol_publisher: PinControlPublisher,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) {
    // Initialize the pin with a pull-up. The button is wired to GND.
    let mut case_pin =
        gpio::Input::new(pin, gpio::InputConfig::default().with_pull(gpio::Pull::Up));

    // Wait for the pin to go low for a given amount of time.
    // Ignore the click if its duration is too short. Shortcircuit if the button
    // is held for a long time. The idea here is that the user can hold the
    // button 'until something happens', not knowing how long that takes.
    loop {
        case_pin.wait_for_falling_edge().await;
        let fall_time = embassy_time::Instant::now();

        let wait_for_high = case_pin.wait_for_high();
        let _ = embassy_time::with_timeout(BUTTON_HELD_DURATION_MAX, wait_for_high).await;

        let held_duration = fall_time.elapsed();
        if held_duration > BUTTON_HELD_DURATION_MIN {
            memlog.info("case: button triggered");

            casebutton_sender.send(Instant::now());

            buzzer_channel.send(CASE_BUTTON_TONE).await;
            pincontrol_publisher
                .publish(PinControlMessage::ButtonPower)
                .await;
        }
    }
}
