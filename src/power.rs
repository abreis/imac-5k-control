use crate::{
    memlog::SharedLogger,
    state::{SharedState, State},
    task::{
        buzzer::{BuzzerAction, BuzzerChannel, BuzzerPattern},
        pin_control::{OnOff, PinControlChannel, PinControlMessage},
    },
};
use anyhow::bail;
use embassy_time::{Duration, Timer};

// Measured ~3.5s from the time power is applied until controller settles.
// By settling, we mean its power draw stabilizing (at 0.04 watts).
const POWER_ON_PAUSE: Duration = Duration::from_millis(3500 + 500);
// Measured ~3s from the time the power button is pressed (to OFF) until the controller stops drawing power.
const BUTTON_OFF_PAUSE: Duration = Duration::from_millis(3000 + 500);

const POWER_TONE: BuzzerPattern = &[BuzzerAction::Beep { ms: 200 }];
const DISPLAY_TONE: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 100 },
    BuzzerAction::Pause { ms: 50 },
    BuzzerAction::Beep { ms: 100 },
];

pub async fn power_on(
    state: SharedState,
    pincontrol_channel: PinControlChannel,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) -> anyhow::Result<()> {
    memlog.info("powering display on");

    {
        let current_state = state.get();
        if current_state != State::Standby {
            bail!("asked to power on while in state {current_state:?}")
        }
    }

    state.set_powering_on()?;
    pincontrol_channel
        .send(PinControlMessage::DisplayPower(OnOff::On))
        .await;
    buzzer_channel.send(POWER_TONE).await;

    Timer::after(POWER_ON_PAUSE).await;

    state.set_display_on()?;
    pincontrol_channel
        .send(PinControlMessage::ButtonPower)
        .await;
    buzzer_channel.send(DISPLAY_TONE).await;

    Ok(())
}

pub async fn power_off(
    state: SharedState,
    pincontrol_channel: PinControlChannel,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) -> anyhow::Result<()> {
    memlog.info("powering display off");

    {
        let current_state = state.get();
        if current_state != State::DisplayOn {
            bail!("asked to power off while in state {current_state:?}")
        }
    }

    state.set_powering_off()?;
    pincontrol_channel
        .send(PinControlMessage::ButtonPower)
        .await;
    buzzer_channel.send(DISPLAY_TONE).await;

    Timer::after(BUTTON_OFF_PAUSE).await;

    state.set_standby()?;
    pincontrol_channel
        .send(PinControlMessage::DisplayPower(OnOff::Off))
        .await;
    buzzer_channel.send(POWER_TONE).await;

    Ok(())
}
