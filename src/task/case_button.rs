use crate::{
    memlog::SharedLogger,
    task::buzzer::{BuzzerAction, BuzzerChannel, BuzzerPattern},
};
use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};
use embassy_time::Duration;
use esp_hal::gpio;

const SHORT_PRESS_MIN_DURATION: Duration = Duration::from_millis(1000);
const LONG_PRESS_MIN_DURATION: Duration = Duration::from_millis(4000);

// TODO: move this to our state machine task
const CASE_BUTTON_SHORT_PRESS_PATTERN: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 50 },
    BuzzerAction::Beep { ms: 100 },
];
const CASE_BUTTON_LONG_PRESS_PATTERN: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseButton {
    ShortPress,
    LongPress,
}

pub type CaseButtonWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, CaseButton, W>;
pub type CaseButtonDynSender = watch::DynSender<'static, CaseButton>;
pub type CaseButtonDynReceiver = watch::DynReceiver<'static, CaseButton>;

pub fn init<const WATCHERS: usize>() -> CaseButtonWatch<WATCHERS> {
    Box::leak(Box::new(watch::Watch::new()))
}

#[embassy_executor::task]
pub async fn case_button(
    pin: gpio::AnyPin<'static>,
    casebutton_sender: CaseButtonDynSender,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) {
    // Initialize the pin with a pull-up. The button is wired to GND.
    let mut case_pin =
        gpio::Input::new(pin, gpio::InputConfig::default().with_pull(gpio::Pull::Up));

    loop {
        case_pin.wait_for_falling_edge().await;

        // Button was pressed.

        if embassy_time::with_timeout(SHORT_PRESS_MIN_DURATION, case_pin.wait_for_high())
            .await
            .is_ok()
        {
            // Button was released before a short press.
            continue;
        }

        // Button is held for a short press.
        buzzer_channel.send(CASE_BUTTON_SHORT_PRESS_PATTERN).await;

        let long_press_remaining = LONG_PRESS_MIN_DURATION - SHORT_PRESS_MIN_DURATION;
        if embassy_time::with_timeout(long_press_remaining, case_pin.wait_for_high())
            .await
            .is_ok()
        {
            // Button is released in time for a short press.
            casebutton_sender.send(CaseButton::ShortPress);
            memlog.info("case: short button press");
            continue;
        }

        // Button was held for a long press.
        buzzer_channel.send(CASE_BUTTON_LONG_PRESS_PATTERN).await;
        casebutton_sender.send(CaseButton::LongPress);
        memlog.info("case: long button press");
    }
}
