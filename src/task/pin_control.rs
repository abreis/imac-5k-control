use crate::{
    driver::mcp23009::{OutputState, Pin},
    ioexpander::{self, IoExpander},
    memlog::SharedLogger,
    task::buzzer::{BuzzerAction, BuzzerChannel, BuzzerPattern},
};
use alloc::{boxed::Box, format};
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, pubsub, watch};
use embassy_time::{Duration, Ticker};
use serde::{Deserialize, Serialize};

// How long to toggle button control pins for.
const BUTTON_DELAY_MS: Duration = Duration::from_millis(200);
// Rate at which to poll the display LEDs.
// 4 Hz keeps latency low while remaining negligible on CPU budget.
const LED_POLL_INTERVAL: Duration = Duration::from_hz(4);

// Pin connections.
const PIN_LED_GREEN: Pin = Pin::Gp0;
const PIN_LED_RED: Pin = Pin::Gp1;
const PIN_BTN_POWER: Pin = Pin::Gp2;
const PIN_BTN_UP: Pin = Pin::Gp3;
const PIN_BTN_DOWN: Pin = Pin::Gp4;
const PIN_BTN_MENU: Pin = Pin::Gp5;
const PIN_BTN_BACK: Pin = Pin::Gp6;

const PUBSUB_CAPACITY: usize = 5;
pub type PinControlPubSub<const P: usize, const S: usize> =
    &'static pubsub::PubSubChannel<NoopRawMutex, PinControlMessage, PUBSUB_CAPACITY, S, P>;
pub type PinControlPublisher = pubsub::DynPublisher<'static, PinControlMessage>;
pub type PinControlSubscriber = pubsub::DynSubscriber<'static, PinControlMessage>;

pub type DisplayLedWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, LedState, W>;
pub type DisplayLedDynSender = watch::DynSender<'static, LedState>;
pub type DisplayLedDynReceiver = watch::DynReceiver<'static, LedState>;

const ERROR_PATTERN: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 120 },
    BuzzerAction::Pause { ms: 80 },
    BuzzerAction::Beep { ms: 350 },
];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedState {
    pub red: bool,
    pub green: bool,
}

pub fn init<'d, const PUBS: usize, const SUBS: usize, const WATCHERS: usize>()
-> (PinControlPubSub<PUBS, SUBS>, DisplayLedWatch<WATCHERS>) {
    let pincontrol_pubsub = Box::leak(Box::new(pubsub::PubSubChannel::new()));
    let display_led_watch = Box::leak(Box::new(watch::Watch::new()));

    (pincontrol_pubsub, display_led_watch)
}

impl IoExpander {
    pub fn read_leds(&mut self) -> Result<LedState, ioexpander::Error> {
        let levels = self.driver.read_levels()?;
        let green = levels[PIN_LED_GREEN as usize].into();
        let red = levels[PIN_LED_RED as usize].into();

        Ok(LedState { red, green })
    }

    pub async fn press_button(
        &mut self,
        message: PinControlMessage,
    ) -> Result<(), ioexpander::Error> {
        let pin = match message {
            PinControlMessage::ButtonPower => PIN_BTN_POWER,
            PinControlMessage::ButtonUp => PIN_BTN_UP,
            PinControlMessage::ButtonDown => PIN_BTN_DOWN,
            PinControlMessage::ButtonMenu => PIN_BTN_MENU,
            PinControlMessage::ButtonBack => PIN_BTN_BACK,
        };

        self.driver.set_output(pin, OutputState::Low)?;
        embassy_time::Timer::after(BUTTON_DELAY_MS).await;
        self.driver.set_output(pin, OutputState::Released)?;

        Ok(())
    }
}

/// Triggers display-board button actions behind the IO expander and publishes LED changes.
#[embassy_executor::task]
pub async fn pin_control(
    mut ioexpander: IoExpander,
    mut pincontrol_subscriber: PinControlSubscriber,
    display_led_sender: DisplayLedDynSender,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) {
    let mut led_state = LedState {
        green: false,
        red: false,
    };
    let mut fault_active = false;
    let mut led_poll_ticker = Ticker::every(LED_POLL_INTERVAL);

    loop {
        let catch = (async || -> Result<(), ioexpander::Error> {
            let ticker_fut = led_poll_ticker.next();
            let pincontrol_fut = pincontrol_subscriber.next_message();

            match select(ticker_fut, pincontrol_fut).await {
                // LED poller ticked, read LED pins and update.
                Either::First(_tick) => {
                    let new_led_state = ioexpander.read_leds()?;
                    if led_state != new_led_state {
                        display_led_sender.send(new_led_state);
                        led_state = new_led_state;
                    }
                }

                // Control message received, press a button pin.
                Either::Second(result) => {
                    if let pubsub::WaitResult::Message(message) = result {
                        ioexpander.press_button(message).await?;
                    }
                }
            }

            Ok(())
        })()
        .await;

        match (catch, fault_active) {
            // New fault.
            (Err(error), false) => {
                fault_active = true;
                buzzer_channel.send(ERROR_PATTERN).await;
                memlog.warn(format!("mcp23009 fault: {error}"));

                let _ = ioexpander.configure();
            }

            // Fault cleared.
            (Ok(()), true) => fault_active = false,

            // Error, but already in fault.
            // Keep resetting the device.
            (Err(_), true) => {
                let _ = ioexpander.configure();
            }

            // No error, no fault.
            (Ok(_), false) => (),
        }
    }
}
