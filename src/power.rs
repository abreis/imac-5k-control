use crate::{
    memlog::SharedLogger,
    state::{SharedState, State},
    task::pin_control::{OnOff, PinControlChannel, PinControlMessage},
};
use anyhow::bail;
use embassy_time::{Duration, Timer};

// TODO: how long to wait after 24v power is on?
const POWER_ON_PAUSE: Duration = Duration::from_millis(2500);
// TODO: how long to wait after clicking power button?
const BUTTON_OFF_PAUSE: Duration = Duration::from_millis(2500);

pub async fn power_on(
    state: SharedState,
    pincontrol_channel: PinControlChannel,
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
    Timer::after(POWER_ON_PAUSE).await;

    state.set_display_on()?;
    pincontrol_channel
        .send(PinControlMessage::ButtonPower)
        .await;

    Ok(())
}
pub async fn power_off(
    state: SharedState,
    pincontrol_channel: PinControlChannel,
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
    Timer::after(BUTTON_OFF_PAUSE).await;

    state.set_standby()?;
    pincontrol_channel
        .send(PinControlMessage::DisplayPower(OnOff::Off))
        .await;

    Ok(())
}
