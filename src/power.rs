use crate::{
    memlog,
    state::{self, State},
    task::pin_control::{OnOff, PINCONTROL_CHANNEL, PinControlMessage},
};
use anyhow::bail;
use embassy_time::{Duration, Timer};

// TODO: how long to wait after 24v power is on?
const POWER_ON_PAUSE: Duration = Duration::from_millis(2500);
// TODO: how long to wait after clicking power button?
const BUTTON_OFF_PAUSE: Duration = Duration::from_millis(2500);

pub async fn power_on() -> anyhow::Result<()> {
    memlog::info("powering display on").await;

    {
        let current_state = state::get().await;
        if current_state != State::Standby {
            bail!("asked to power on while in state {current_state:?}")
        }
    }

    state::to_powering_on().await?;
    PINCONTROL_CHANNEL
        .send(PinControlMessage::DisplayPower(OnOff::On))
        .await;
    Timer::after(POWER_ON_PAUSE).await;

    state::to_display_on().await?;
    PINCONTROL_CHANNEL
        .send(PinControlMessage::ButtonPower)
        .await;

    Ok(())
}
pub async fn power_off() -> anyhow::Result<()> {
    memlog::info("powering display off").await;

    {
        let current_state = state::get().await;
        if current_state != State::DisplayOn {
            bail!("asked to power off while in state {current_state:?}")
        }
    }

    state::to_powering_off().await?;
    PINCONTROL_CHANNEL
        .send(PinControlMessage::ButtonPower)
        .await;
    Timer::after(BUTTON_OFF_PAUSE).await;

    state::to_standby().await?;
    PINCONTROL_CHANNEL
        .send(PinControlMessage::DisplayPower(OnOff::Off))
        .await;

    Ok(())
}
