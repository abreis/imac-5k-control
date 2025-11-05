use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, pubsub};
use embassy_time::{Duration, Timer};
use esp_hal::gpio;
use serde::{Deserialize, Serialize};

// How long to toggle button control pins for.
const BUTTON_DELAY_MS: Duration = Duration::from_millis(250);

const PUBSUB_CAPACITY: usize = 5;
pub type PinControlPubSub<const P: usize, const S: usize> =
    &'static pubsub::PubSubChannel<NoopRawMutex, PinControlMessage, PUBSUB_CAPACITY, S, P>;
pub type PinControlPublisher = pubsub::DynPublisher<'static, PinControlMessage>;
pub type PinControlSubscriber = pubsub::DynSubscriber<'static, PinControlMessage>;

#[allow(clippy::enum_variant_names)]
#[derive(Copy, Clone, Serialize, Deserialize)]
pub enum PinControlMessage {
    ButtonPower,
    // Note: doubles as 'Enter'
    ButtonMenu,
    // Note: mislabeled as 'Enter' in controller
    ButtonBack,
    ButtonDown,
    ButtonUp,
}

pub fn init<const PUBS: usize, const SUBS: usize>() -> PinControlPubSub<PUBS, SUBS> {
    Box::leak(Box::new(pubsub::PubSubChannel::new()))
}

/// Triggers actions controlled by output pins.
#[embassy_executor::task]
pub async fn pin_control(
    mut pin_button_power: gpio::Output<'static>,
    mut pin_button_menu: gpio::Output<'static>,
    mut pin_button_back: gpio::Output<'static>,
    mut pin_button_down: gpio::Output<'static>,
    mut pin_button_up: gpio::Output<'static>,
    mut pincontrol_subscriber: PinControlSubscriber,
) {
    loop {
        use PinControlMessage::*;

        if let pubsub::WaitResult::Message(message) = pincontrol_subscriber.next_message().await {
            match message {
                // Power button is active high.
                ButtonPower => {
                    pin_button_power.set_high();
                    Timer::after(BUTTON_DELAY_MS).await;
                    pin_button_power.set_low();
                }
                // Menu, Back, Up, Down buttons are active low.
                ButtonMenu => {
                    pin_button_menu.set_low();
                    Timer::after(BUTTON_DELAY_MS).await;
                    pin_button_menu.set_high();
                }
                ButtonBack => {
                    pin_button_back.set_low();
                    Timer::after(BUTTON_DELAY_MS).await;
                    pin_button_back.set_high();
                }
                ButtonDown => {
                    pin_button_down.set_low();
                    Timer::after(BUTTON_DELAY_MS).await;
                    pin_button_down.set_high();
                }
                ButtonUp => {
                    pin_button_up.set_low();
                    Timer::after(BUTTON_DELAY_MS).await;
                    pin_button_up.set_high();
                }
            }
        }
    }
}
