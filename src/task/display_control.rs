use crate::{
    memlog::SharedLogger,
    task::{
        buzzer::{BuzzerAction, BuzzerChannel, BuzzerPattern},
        case_button::{CaseButton, CaseButtonDynReceiver},
        display_state::{DisplayState, DisplayStateDynReceiver},
        pin_control::{PinControlMessage, PinControlPublisher},
        power_relay::{PowerRelayCommand, PowerRelayDynSender},
    },
};
use alloc::format;
use core::future::pending;
use embassy_futures::select::{Either3, select3};
use embassy_time::{Duration, Instant, Timer};

const BOARD_OFF_DWELL_BEFORE_POWER_BUTTON: Duration = Duration::from_secs(5);
const POWER_ON_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const POWER_OFF_WAIT_BOARD_OFF_TIMEOUT: Duration = Duration::from_secs(10);
const POWER_OFF_RELAY_CUT_DELAY: Duration = Duration::from_secs(3);

const DISPLAY_POWER_TIMEOUT_PATTERN: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 100 },
];

#[derive(Clone, Copy, Debug)]
enum PendingAction {
    PowerOnWaitAfterRelayClose {
        timeout_at: Instant,
        board_off_since: Option<Instant>,
    },
    PowerOnWaitAfterButton {
        timeout_at: Instant,
    },
    PowerOffWaitForBoardOff {
        timeout_at: Instant,
    },
    PowerOffWaitRelayCut {
        relay_cut_at: Instant,
    },
}

impl PendingAction {
    fn timer_deadline(&self) -> Instant {
        match self {
            PendingAction::PowerOnWaitAfterRelayClose {
                timeout_at,
                board_off_since: Some(board_off_since),
            } => {
                let board_off_dwell_at = *board_off_since + BOARD_OFF_DWELL_BEFORE_POWER_BUTTON;
                if board_off_dwell_at < *timeout_at {
                    board_off_dwell_at
                } else {
                    *timeout_at
                }
            }

            PendingAction::PowerOnWaitAfterRelayClose {
                timeout_at,
                board_off_since: None,
            }
            | PendingAction::PowerOnWaitAfterButton { timeout_at }
            | PendingAction::PowerOffWaitForBoardOff { timeout_at } => *timeout_at,

            PendingAction::PowerOffWaitRelayCut { relay_cut_at } => *relay_cut_at,
        }
    }
}

async fn pending_timer(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => Timer::at(deadline).await,
        None => pending::<()>().await,
    }
}

async fn sequence_timed_out(
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
    reason: &'static str,
) {
    memlog.warn(format!("displayctl: {reason}"));
    buzzer_channel.send(DISPLAY_POWER_TIMEOUT_PATTERN).await;
}

async fn advance_pending_action(
    pending_action: PendingAction,
    display_state: DisplayState,
    pincontrol_publisher: &PinControlPublisher,
    powerrelay_sender: &PowerRelayDynSender,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) -> Option<PendingAction> {
    let now = Instant::now();

    match pending_action {
        PendingAction::PowerOnWaitAfterRelayClose {
            timeout_at,
            board_off_since,
        } => {
            if matches!(display_state, DisplayState::Active | DisplayState::Standby) {
                memlog.info(format!(
                    "displayctl: relay restore auto-resumed to {display_state:?}"
                ));
                return None;
            }

            if display_state == DisplayState::BoardOff {
                let board_off_since = board_off_since.unwrap_or(now);
                let board_off_dwell_at = board_off_since + BOARD_OFF_DWELL_BEFORE_POWER_BUTTON;

                if now >= board_off_dwell_at {
                    pincontrol_publisher
                        .publish(PinControlMessage::ButtonPower)
                        .await;
                    memlog.info("displayctl: board off dwell met, pressed display power button");

                    return Some(PendingAction::PowerOnWaitAfterButton {
                        timeout_at: now + POWER_ON_WAIT_TIMEOUT,
                    });
                }

                if now >= timeout_at {
                    sequence_timed_out(
                        buzzer_channel,
                        memlog,
                        "power-on timed out while waiting for BoardOff dwell",
                    )
                    .await;
                    return None;
                }

                return Some(PendingAction::PowerOnWaitAfterRelayClose {
                    timeout_at,
                    board_off_since: Some(board_off_since),
                });
            }

            if now >= timeout_at {
                sequence_timed_out(
                    buzzer_channel,
                    memlog,
                    "power-on timed out waiting for BoardOff or auto-resume",
                )
                .await;
                None
            } else {
                Some(PendingAction::PowerOnWaitAfterRelayClose {
                    timeout_at,
                    board_off_since: None,
                })
            }
        }

        PendingAction::PowerOnWaitAfterButton { timeout_at } => {
            if matches!(display_state, DisplayState::Active | DisplayState::Standby) {
                memlog.info(format!(
                    "displayctl: power-on completed to {display_state:?}"
                ));
                return None;
            }

            if now >= timeout_at {
                sequence_timed_out(
                    buzzer_channel,
                    memlog,
                    "power-on timed out waiting for Active or Standby",
                )
                .await;
                None
            } else {
                Some(PendingAction::PowerOnWaitAfterButton { timeout_at })
            }
        }

        PendingAction::PowerOffWaitForBoardOff { timeout_at } => {
            if matches!(
                display_state,
                DisplayState::DcPowerOff | DisplayState::RelayLatchedFault
            ) {
                memlog.info(format!(
                    "displayctl: power-off finished early in observed state {display_state:?}"
                ));
                return None;
            }

            if display_state == DisplayState::BoardOff {
                memlog.info("displayctl: board reached BoardOff, waiting before opening relay");
                return Some(PendingAction::PowerOffWaitRelayCut {
                    relay_cut_at: now + POWER_OFF_RELAY_CUT_DELAY,
                });
            }

            if now >= timeout_at {
                sequence_timed_out(
                    buzzer_channel,
                    memlog,
                    "power-off timed out waiting for BoardOff",
                )
                .await;
                None
            } else {
                Some(PendingAction::PowerOffWaitForBoardOff { timeout_at })
            }
        }

        PendingAction::PowerOffWaitRelayCut { relay_cut_at } => {
            if matches!(
                display_state,
                DisplayState::DcPowerOff | DisplayState::RelayLatchedFault
            ) {
                memlog.info(format!(
                    "displayctl: relay already open in observed state {display_state:?}"
                ));
                return None;
            }

            if now >= relay_cut_at {
                powerrelay_sender.send(PowerRelayCommand::Open).await;
                memlog.info("displayctl: opened relay after board shutdown");
                None
            } else {
                Some(PendingAction::PowerOffWaitRelayCut { relay_cut_at })
            }
        }
    }
}

#[embassy_executor::task]
pub async fn display_control(
    mut casebutton_receiver: CaseButtonDynReceiver,
    mut displayboard_receiver: DisplayStateDynReceiver,
    pincontrol_publisher: PinControlPublisher,
    powerrelay_sender: PowerRelayDynSender,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) {
    let mut display_state = displayboard_receiver.get().await;
    let mut pending_action: Option<PendingAction> = None;

    loop {
        match select3(
            casebutton_receiver.changed(),
            displayboard_receiver.changed(),
            pending_timer(pending_action.as_ref().map(PendingAction::timer_deadline)),
        )
        .await
        {
            Either3::First(CaseButton::LongPress) => {
                if pending_action.take().is_some() {
                    memlog.info("displayctl: long press interrupted active short-press sequence");
                }

                powerrelay_sender.send(PowerRelayCommand::Open).await;
                memlog.info("displayctl: long press -> relay open");
            }

            Either3::First(CaseButton::ShortPress) => {
                if pending_action.is_some() {
                    memlog.info("displayctl: ignoring short press while transition is active");
                    continue;
                }

                match display_state {
                    DisplayState::DcPowerOff => {
                        powerrelay_sender.send(PowerRelayCommand::Close).await;
                        pending_action = Some(PendingAction::PowerOnWaitAfterRelayClose {
                            timeout_at: Instant::now() + POWER_ON_WAIT_TIMEOUT,
                            board_off_since: None,
                        });
                        memlog.info("displayctl: short press from DcPowerOff -> relay close");
                    }

                    DisplayState::BoardOff => {
                        pincontrol_publisher
                            .publish(PinControlMessage::ButtonPower)
                            .await;
                        pending_action = Some(PendingAction::PowerOnWaitAfterButton {
                            timeout_at: Instant::now() + POWER_ON_WAIT_TIMEOUT,
                        });
                        memlog.info("displayctl: short press from BoardOff -> power button");
                    }

                    DisplayState::Active | DisplayState::Standby => {
                        pincontrol_publisher
                            .publish(PinControlMessage::ButtonPower)
                            .await;
                        pending_action = Some(PendingAction::PowerOffWaitForBoardOff {
                            timeout_at: Instant::now() + POWER_OFF_WAIT_BOARD_OFF_TIMEOUT,
                        });
                        memlog.info(format!(
                            "displayctl: short press from {display_state:?} -> power button"
                        ));
                    }

                    DisplayState::Unknown => {
                        memlog.warn("displayctl: ignoring short press in Unknown state");
                    }

                    DisplayState::RelayLatchedFault => {
                        memlog.warn("displayctl: ignoring short press while relay is latched open");
                    }
                }
            }

            Either3::Second(new_display_state) => {
                display_state = new_display_state;

                if let Some(action) = pending_action.take() {
                    pending_action = advance_pending_action(
                        action,
                        display_state,
                        &pincontrol_publisher,
                        &powerrelay_sender,
                        buzzer_channel,
                        memlog,
                    )
                    .await;
                }
            }

            Either3::Third(()) => {
                if let Some(action) = pending_action.take() {
                    pending_action = advance_pending_action(
                        action,
                        display_state,
                        &pincontrol_publisher,
                        &powerrelay_sender,
                        buzzer_channel,
                        memlog,
                    )
                    .await;
                }
            }
        }
    }
}
