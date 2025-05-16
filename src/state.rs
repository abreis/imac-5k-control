use core::{cell::Cell, ops::Deref};

use anyhow::anyhow;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};

static DISPLAY_STATE: Mutex<CriticalSectionRawMutex, Cell<State>> =
    Mutex::new(Cell::new(State::Standby));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Off state. No 24V power to the display controller.
    Standby,
    /// Display controller powered with 24V power, waiting to click power button to on.
    PoweringOn,
    /// On state. Display powered and power button clicked to on.
    DisplayOn,
    /// Power button clicked to off, waiting to unpower 24V from the display controller.
    PoweringOff,
}

async fn try_transition(valid_from: State, transition_to: State) -> anyhow::Result<()> {
    let state = DISPLAY_STATE.lock().await;
    if state.get() == valid_from {
        state.replace(transition_to);
        Ok(())
    } else {
        Err(anyhow!(
            "Invalid state transition: can't transition to {transition_to:?}"
        ))
    }
}

pub async fn to_standby() -> anyhow::Result<()> {
    try_transition(State::PoweringOff, State::Standby).await
}

pub async fn to_powering_on() -> anyhow::Result<()> {
    try_transition(State::Standby, State::PoweringOn).await
}

pub async fn to_display_on() -> anyhow::Result<()> {
    try_transition(State::PoweringOn, State::DisplayOn).await
}

pub async fn to_powering_off() -> anyhow::Result<()> {
    try_transition(State::DisplayOn, State::PoweringOff).await
}

pub async fn get() -> State {
    DISPLAY_STATE.lock().await.get()
}
