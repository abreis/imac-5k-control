use alloc::boxed::Box;
use anyhow::{Result, anyhow};
use core::cell::Cell;

// Embassy tasks are statically allocated. This is a version of the state that can be
// shared between tasks without the need for critical_section.
#[derive(Clone, Copy)]
pub struct SharedState {
    inner: &'static Cell<State>,
}

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

impl SharedState {
    pub fn new_standby() -> Self {
        Self {
            inner: Box::leak(Box::new(Cell::new(State::Standby))),
        }
    }

    fn try_transition(&self, valid_from: State, transition_to: State) -> Result<()> {
        if self.inner.get() == valid_from {
            self.inner.replace(transition_to);
            Ok(())
        } else {
            Err(anyhow!(
                "Invalid state transition: can't transition to {transition_to:?}"
            ))
        }
    }
    pub fn set_standby(&self) -> Result<()> {
        self.try_transition(State::PoweringOff, State::Standby)
    }

    pub fn set_powering_on(&self) -> Result<()> {
        self.try_transition(State::Standby, State::PoweringOn)
    }

    pub fn set_display_on(&self) -> Result<()> {
        self.try_transition(State::PoweringOn, State::DisplayOn)
    }

    pub fn set_powering_off(&self) -> Result<()> {
        self.try_transition(State::DisplayOn, State::PoweringOff)
    }

    pub fn get(&self) -> State {
        self.inner.get()
    }
}
