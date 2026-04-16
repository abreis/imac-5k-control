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
use alloc::{boxed::Box, format};
use core::{future::Future, pin::Pin};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer, with_timeout};

const BOARD_OFF_DWELL_BEFORE_POWER_BUTTON: Duration = Duration::from_secs(5);
const POWER_ON_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const POWER_OFF_WAIT_BOARD_OFF_TIMEOUT: Duration = Duration::from_secs(2);
const POWER_OFF_RELAY_CUT_DELAY: Duration = Duration::from_secs(5);

const DISPLAY_POWER_TIMEOUT_PATTERN: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 100 },
];

#[derive(Debug, Copy, Clone, PartialEq)]
enum SequenceResult {
    Finished,
    TimedOut(&'static str),
    UnexpectedState(DisplayState),
}

#[embassy_executor::task]
pub async fn display_control(
    mut casebutton_receiver: CaseButtonDynReceiver,
    mut displayboard_receiver: DisplayStateDynReceiver,
    mut pincontrol_publisher: PinControlPublisher,
    mut powerrelay_sender: PowerRelayDynSender,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) {
    loop {
        // Wait for the case button to be pressed.
        let button_press = casebutton_receiver.changed().await;

        // A long press always forces the relay open.
        if button_press == CaseButton::LongPress {
            powerrelay_sender.send(PowerRelayCommand::Open).await;
        }

        // For a short press, find our current state, and dispatch a
        // corresponding power-on or power-off sequence.
        if button_press == CaseButton::ShortPress {
            let display_state = displayboard_receiver.get().await;

            use DisplayState::*;
            let mut power_seq_fut: Pin<Box<dyn Future<Output = SequenceResult>>>;
            power_seq_fut = match display_state {
                DcPowerOff => {
                    let fut = power_on_from_dc_power_off(
                        &mut displayboard_receiver,
                        &mut pincontrol_publisher,
                        &mut powerrelay_sender,
                    );
                    Box::pin(fut)
                }

                BoardOff => {
                    let fut = power_on_from_board_off(
                        &mut displayboard_receiver,
                        &mut pincontrol_publisher,
                    );
                    Box::pin(fut)
                }

                Active | Standby => {
                    let fut = power_off_from_operational(
                        &mut displayboard_receiver,
                        &mut pincontrol_publisher,
                        &mut powerrelay_sender,
                    );
                    Box::pin(fut)
                }

                // Can't transition out of these states.
                Unknown | RelayLatchedFault => continue,
            };

            // Now that we have a future that will perform the sequence of
            // commands, await it while watching for a LongPress. If we get a
            // long press, the sequence fut is dropped and we open the relay.
            let long_press_fut =
                casebutton_receiver.changed_and(|&press| press == CaseButton::LongPress);

            match select(long_press_fut, &mut power_seq_fut).await {
                // Long press arrived interrupting a sequence.
                Either::First(_longpress) => {
                    drop(power_seq_fut); // terminates the sequence (async cancellation)
                    powerrelay_sender.send(PowerRelayCommand::Open).await;
                    memlog.warn("dspl_ctl: long press during power sequence, forced relay off");
                }

                // Sequence completed.
                Either::Second(result) => match result {
                    SequenceResult::Finished => memlog.info("dspl_ctl: power sequence complete"),

                    SequenceResult::TimedOut(reason) => {
                        buzzer_channel.send(DISPLAY_POWER_TIMEOUT_PATTERN).await;
                        memlog.warn(format!("dspl_ctl: power sequence timed out: {reason}"));
                    }

                    SequenceResult::UnexpectedState(state) => {
                        memlog.warn("dspl_ctl: moved to unexpected state: {state:?}")
                    }
                },
            }
        }
    }
}

async fn power_on_from_dc_power_off(
    displayboard_receiver: &mut DisplayStateDynReceiver,
    pincontrol_publisher: &PinControlPublisher,
    powerrelay_sender: &PowerRelayDynSender,
) -> SequenceResult {
    use DisplayState::*;

    // Close the relay, providing DC power.
    powerrelay_sender.send(PowerRelayCommand::Close).await;

    // Wait for a move to BoardOff, or Active/Standby.
    // We might be there already, so don't wait on a change.
    // Note: `get_and()` waits for the predicate to match, and also resolves
    // immediately if it's already matching.
    let timeout = POWER_ON_WAIT_TIMEOUT;
    let boardoff_fut = displayboard_receiver
        .get_and(|&state| state == BoardOff || state == Active || state == Standby);
    match with_timeout(timeout, boardoff_fut).await {
        Err(_timeout) => return SequenceResult::TimedOut("no move from power off"),
        Ok(Active) | Ok(Standby) => return SequenceResult::Finished,
        Ok(BoardOff) => (),
        _ => unreachable!(),
    }

    // Now give the board time to physically power on.
    Timer::after(BOARD_OFF_DWELL_BEFORE_POWER_BUTTON).await;

    // At this stage we might be in BoardOff or in Active/Standby.
    // If the former, press the power button. If the latter, we're done.
    match displayboard_receiver.get().await {
        BoardOff => power_on_from_board_off(displayboard_receiver, pincontrol_publisher).await,

        Active | Standby => SequenceResult::Finished,

        unexpected => SequenceResult::UnexpectedState(unexpected),
    }
}

async fn power_on_from_board_off(
    displayboard_receiver: &mut DisplayStateDynReceiver,
    pincontrol_publisher: &PinControlPublisher,
) -> SequenceResult {
    use DisplayState::*;

    // Push the board's power button.
    pincontrol_publisher
        .publish(PinControlMessage::ButtonPower)
        .await;

    // Expect the board to flash either red or green, switching us to an
    // operational state (Active or Standby).
    let timeout = POWER_ON_WAIT_TIMEOUT;
    let operational_fut =
        displayboard_receiver.get_and(|&state| state == Active || state == Standby);

    if let Err(_timeout) = with_timeout(timeout, operational_fut).await {
        SequenceResult::TimedOut("no move to operational")
    } else {
        SequenceResult::Finished
    }
}

async fn power_off_from_operational(
    displayboard_receiver: &mut DisplayStateDynReceiver,
    pincontrol_publisher: &PinControlPublisher,
    powerrelay_sender: &PowerRelayDynSender,
) -> SequenceResult {
    use DisplayState::*;

    // Push the board's power button.
    pincontrol_publisher
        .publish(PinControlMessage::ButtonPower)
        .await;

    // Expect the state to transition to BoardOff.
    let timeout = POWER_OFF_WAIT_BOARD_OFF_TIMEOUT;
    let boardoff_fut = displayboard_receiver.get_and(|&state| state == BoardOff);
    if let Err(_timeout) = with_timeout(timeout, boardoff_fut).await {
        return SequenceResult::TimedOut("no move to board off");
    }

    // Pause, then open the relay.
    Timer::after(POWER_OFF_RELAY_CUT_DELAY).await;
    powerrelay_sender.send(PowerRelayCommand::Open).await;

    SequenceResult::Finished
}
