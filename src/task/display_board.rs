use crate::{
    memlog::SharedLogger,
    task::{
        case_button::{CaseButton, CaseButtonDynReceiver},
        pin_control::{DisplayLedDynReceiver, LedState, PinControlPublisher},
        power_relay::{PowerRelay, PowerRelayDynSender, PowerRelayStateDynReceiver},
    },
};
use alloc::{boxed::Box, format};
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayBoard {
    Unknown,
    DcPowerOff,
    BoardOff,
    Standby,
    Active,
    RelayLatchedFault,
}

pub type DisplayBoardWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, DisplayBoard, W>;
pub type DisplayBoardDynSender = watch::DynSender<'static, DisplayBoard>;
pub type DisplayBoardDynReceiver = watch::DynReceiver<'static, DisplayBoard>;

pub fn init<const WATCHERS: usize>() -> DisplayBoardWatch<WATCHERS> {
    Box::leak(Box::new(watch::Watch::new()))
}

fn derive_state(relay_state: PowerRelay, led_state: LedState) -> DisplayBoard {
    match relay_state {
        PowerRelay::ForcedOpen => DisplayBoard::RelayLatchedFault,

        PowerRelay::Open => DisplayBoard::DcPowerOff,

        PowerRelay::Closed => match led_state {
            LedState {
                red: false,
                green: false,
            } => DisplayBoard::BoardOff,

            LedState {
                red: true,
                green: false,
            } => DisplayBoard::Standby,

            LedState {
                red: false,
                green: true,
            } => DisplayBoard::Active,

            LedState {
                red: true,
                green: true,
            } => DisplayBoard::Unknown,
        },
    }
}

#[embassy_executor::task]
pub async fn display_board(
    mut casebutton_receiver: CaseButtonDynReceiver,
    mut displayled_receiver: DisplayLedDynReceiver,
    pincontrol_publisher: PinControlPublisher,
    powerrelay_sender: PowerRelayDynSender,
    mut powerrelay_receiver: PowerRelayStateDynReceiver,
    displayboard_sender: DisplayBoardDynSender,
    memlog: SharedLogger,
) {
    let _ = &pincontrol_publisher; // TODO
    let _ = &powerrelay_sender; // TODO

    // Wait for initial values of the relay and the board LEDs to arrive.
    let mut relay_state = powerrelay_receiver.get().await;
    let mut led_state = displayled_receiver.get().await;

    // Initial display state.
    let mut display_state = derive_state(relay_state, led_state);
    displayboard_sender.send(display_state);
    memlog.info(format!("display: state -> {display_state:?}"));

    loop {
        match select3(
            casebutton_receiver.changed(),
            displayled_receiver.changed(),
            powerrelay_receiver.changed(),
        )
        .await
        {
            Either3::First(case_button) => {
                // TODO match (case_button, display_state)
            }

            //
            // Update state based on board LEDs.
            Either3::Second(new_led_state) => {
                led_state = new_led_state;
                let new_display_state = derive_state(relay_state, led_state);

                if display_state != new_display_state {
                    display_state = new_display_state;
                    displayboard_sender.send(display_state);
                    memlog.info(format!("display: state -> {display_state:?}"));
                }
            }

            //
            // Update state based on our power relay.
            Either3::Third(new_relay_state) => {
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
