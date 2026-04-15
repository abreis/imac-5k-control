use crate::{
    memlog::SharedLogger,
    task::{
        pin_control::{DisplayLedDynReceiver, LedState},
        power_relay::{PowerRelay, PowerRelayStateDynReceiver},
    },
};
use alloc::{boxed::Box, format};
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayState {
    Unknown,
    DcPowerOff,
    BoardOff,
    Standby,
    Active,
    RelayLatchedFault,
}

pub type DisplayStateWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, DisplayState, W>;
pub type DisplayStateDynSender = watch::DynSender<'static, DisplayState>;
pub type DisplayStateDynReceiver = watch::DynReceiver<'static, DisplayState>;

pub fn init<const WATCHERS: usize>() -> DisplayStateWatch<WATCHERS> {
    Box::leak(Box::new(watch::Watch::new()))
}

fn derive_state(relay_state: PowerRelay, led_state: LedState) -> DisplayState {
    match relay_state {
        PowerRelay::ForcedOpen => DisplayState::RelayLatchedFault,

        PowerRelay::Open => DisplayState::DcPowerOff,

        PowerRelay::Closed => match led_state {
            LedState {
                red: false,
                green: false,
            } => DisplayState::BoardOff,

            LedState {
                red: true,
                green: false,
            } => DisplayState::Standby,

            LedState {
                red: false,
                green: true,
            } => DisplayState::Active,

            LedState {
                red: true,
                green: true,
            } => DisplayState::Unknown,
        },
    }
}

#[embassy_executor::task]
pub async fn display_board(
    mut displayled_receiver: DisplayLedDynReceiver,
    mut powerrelay_receiver: PowerRelayStateDynReceiver,
    displayboard_sender: DisplayStateDynSender,
    memlog: SharedLogger,
) {
    // Wait for initial values of the relay and the board LEDs to arrive.
    let mut relay_state = powerrelay_receiver.get().await;
    let mut led_state = displayled_receiver.get().await;

    // Initial display state.
    let mut display_state = derive_state(relay_state, led_state);
    displayboard_sender.send(display_state);
    memlog.info(format!("display: state -> {display_state:?}"));

    loop {
        let dspl_fut = displayled_receiver.changed();
        let relay_fut = powerrelay_receiver.changed();

        match select(dspl_fut, relay_fut).await {
            Either::First(new_led_state) => {
                led_state = new_led_state;
                let new_display_state = derive_state(relay_state, led_state);

                if display_state != new_display_state {
                    display_state = new_display_state;
                    displayboard_sender.send(display_state);
                    memlog.info(format!("display: state -> {display_state:?}"));
                }
            }

            Either::Second(new_relay_state) => {
                relay_state = new_relay_state;
                let new_display_state = derive_state(relay_state, led_state);

                if display_state != new_display_state {
                    display_state = new_display_state;
                    displayboard_sender.send(display_state);
                    memlog.info(format!("display: state -> {display_state:?}"));
                }
            }
        }
    }
}
