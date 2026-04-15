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
pub enum DisplayBoardState {
    Unknown,
    DcPowerOff,
    BoardOff,
    Standby,
    Active,
    RelayLatchedFault,
}

pub type DisplayBoardWatch<const W: usize> =
    &'static watch::Watch<NoopRawMutex, DisplayBoardState, W>;
pub type DisplayBoardDynSender = watch::DynSender<'static, DisplayBoardState>;
pub type DisplayBoardDynReceiver = watch::DynReceiver<'static, DisplayBoardState>;

pub fn init<const WATCHERS: usize>() -> DisplayBoardWatch<WATCHERS> {
    Box::leak(Box::new(watch::Watch::new()))
}

fn derive_state(relay_state: PowerRelay, led_state: LedState) -> DisplayBoardState {
    match relay_state {
        PowerRelay::ForcedOpen => DisplayBoardState::RelayLatchedFault,
        PowerRelay::Open => DisplayBoardState::DcPowerOff,
        PowerRelay::Closed => match led_state {
            LedState {
                red: false,
                green: false,
            } => DisplayBoardState::BoardOff,
            LedState {
                red: true,
                green: false,
            } => DisplayBoardState::Standby,
            LedState {
                red: false,
                green: true,
            } => DisplayBoardState::Active,
            LedState {
                red: true,
                green: true,
            } => DisplayBoardState::Unknown,
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
    let _ = &pincontrol_publisher;
    let _ = &powerrelay_sender;

    let mut relay_state = powerrelay_receiver.get().await;
    let mut led_state = displayled_receiver.get().await;
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
            Either3::First(case_button) => match (case_button, display_state) {
                (CaseButton::ShortPress, DisplayBoardState::Unknown) => {
                    memlog.info("display: placeholder short press in Unknown")
                }
                (CaseButton::ShortPress, DisplayBoardState::DcPowerOff) => {
                    memlog.info("display: placeholder short press in DcPowerOff")
                }
                (CaseButton::ShortPress, DisplayBoardState::BoardOff) => {
                    memlog.info("display: placeholder short press in BoardOff")
                }
                (CaseButton::ShortPress, DisplayBoardState::Standby) => {
                    memlog.info("display: placeholder short press in Standby")
                }
                (CaseButton::ShortPress, DisplayBoardState::Active) => {
                    memlog.info("display: placeholder short press in Active")
                }
                (CaseButton::ShortPress, DisplayBoardState::RelayLatchedFault) => {
                    memlog.info("display: placeholder short press in RelayLatchedFault")
                }
                (CaseButton::LongPress, DisplayBoardState::Unknown) => {
                    memlog.info("display: placeholder long press in Unknown")
                }
                (CaseButton::LongPress, DisplayBoardState::DcPowerOff) => {
                    memlog.info("display: placeholder long press in DcPowerOff")
                }
                (CaseButton::LongPress, DisplayBoardState::BoardOff) => {
                    memlog.info("display: placeholder long press in BoardOff")
                }
                (CaseButton::LongPress, DisplayBoardState::Standby) => {
                    memlog.info("display: placeholder long press in Standby")
                }
                (CaseButton::LongPress, DisplayBoardState::Active) => {
                    memlog.info("display: placeholder long press in Active")
                }
                (CaseButton::LongPress, DisplayBoardState::RelayLatchedFault) => {
                    memlog.info("display: placeholder long press in RelayLatchedFault")
                }
            },

            Either3::Second(new_led_state) => {
                led_state = new_led_state;
                let new_display_state = derive_state(relay_state, led_state);
                if display_state != new_display_state {
                    display_state = new_display_state;
                    displayboard_sender.send(display_state);
                    memlog.info(format!("display: state -> {display_state:?}"));
                }
            }

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
