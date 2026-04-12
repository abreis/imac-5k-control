//! Blocking MCP23009 driver.
//!
//! This module wraps the ESP HAL blocking I2C driver and exposes a compact API
//! for the MCP23009:
//! * switching pins between input and output
//! * reading inputs
//! * driving open-drain outputs
//! * enabling internal pull-ups
//! * enabling per-pin interrupts
//!
//! It is intentionally narrow: it owns the I2C peripheral, targets the 8-bit
//! MCP23009 only, and leaves broader interrupt handling, IOCON tuning, and
//! multi-device abstractions out of scope.
//!
//! Call [`Mcp23009::init`] before normal use to restore a known baseline
//! configuration.

use esp_hal::{
    Blocking,
    i2c::master::{Error as I2cError, I2c},
};
use thiserror::Error;

#[derive(Debug)]
pub struct Mcp23009<'d> {
    i2c: I2c<'d, Blocking>,
    address: u8,
}

/// MCP23009 pin selector.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum Pin {
    Gp0 = 0,
    Gp1 = 1,
    Gp2 = 2,
    Gp3 = 3,
    Gp4 = 4,
    Gp5 = 5,
    Gp6 = 6,
    Gp7 = 7,
}

/// Pin direction.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Direction {
    /// Configure as input.
    Input,
    /// Configure as output.
    Output,
}

/// Pin logic level.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Level {
    /// Logic low.
    Low,
    /// Logic high.
    High,
}
impl Into<bool> for Level {
    fn into(self) -> bool {
        self == Self::High
    }
}

/// Open-drain output state.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OutputState {
    /// Drive low.
    Low,
    /// Release line.
    Released,
}

/// Internal pull-up state.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PullUp {
    /// Pull-up off.
    Disabled,
    /// Pull-up on.
    Enabled,
}

/// Per-pin interrupt enable.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InterruptEnable {
    /// Interrupt off.
    Disabled,
    /// Interrupt on.
    Enabled,
}

/// Driver error.
#[derive(Debug, Error)]
pub enum Error {
    /// I2C address invalid.
    #[error("invalid MCP23009 I2C address 0x{0:02X}; expected 0x20..=0x27")]
    InvalidAddress(u8),

    /// I2C transaction failed.
    #[error(transparent)]
    I2c(#[from] I2cError),
}

/// MCP23009 register address.
#[derive(Debug, Copy, Clone)]
#[repr(u8)]
enum Register {
    /// Direction register.
    Iodir = 0x00,
    /// Input polarity register.
    Ipol = 0x01,
    /// Interrupt enable register.
    Gpinten = 0x02,
    /// Default compare register.
    Defval = 0x03,
    /// Interrupt control register.
    Intcon = 0x04,
    /// Configuration register.
    Iocon = 0x05,
    /// Pull-up register.
    Gppu = 0x06,
    /// GPIO port register.
    Gpio = 0x09,
    /// Output latch register.
    Olat = 0x0A,
}

// Public interface.
impl<'d> Mcp23009<'d> {
    pub const DEFAULT_ADDRESS: u8 = 0x20;

    /// Creates a driver using the default I2C address.
    pub fn new(i2c: I2c<'d, Blocking>) -> Self {
        Self {
            i2c,
            address: Self::DEFAULT_ADDRESS,
        }
    }

    /// Sets a validated I2C address on the driver.
    pub fn with_address(mut self, address: u8) -> Result<Self, Error> {
        Self::validate_address(address)?;
        self.address = address;

        Ok(self)
    }

    /// Returns the configured I2C address.
    pub fn address(&self) -> u8 {
        self.address
    }

    /// Returns the owned I2C peripheral.
    pub fn free(self) -> I2c<'d, Blocking> {
        self.i2c
    }

    /// Resets the expander configuration to a known baseline.
    pub fn init(&mut self) -> Result<(), Error> {
        // Make every pin an input before resetting the latch state.
        self.write_register(Register::Iodir, 0xFF)?;
        self.write_register(Register::Ipol, 0x00)?;
        self.write_register(Register::Gpinten, 0x00)?;
        self.write_register(Register::Defval, 0x00)?;
        self.write_register(Register::Intcon, 0x00)?;
        self.write_register(Register::Iocon, 0x00)?;
        self.write_register(Register::Gppu, 0x00)?;
        self.write_register(Register::Olat, 0x00)?;

        Ok(())
    }

    /// Reads the current logic level on a pin.
    pub fn read_level(&mut self, pin: Pin) -> Result<Level, Error> {
        if self.read_bit(Register::Gpio, pin)? {
            Ok(Level::High)
        } else {
            Ok(Level::Low)
        }
    }

    /// Reads the current logic levels for all pins.
    ///
    /// Index 0 maps to GP0 and index 7 maps to GP7.
    pub fn read_levels(&mut self) -> Result<[Level; 8], Error> {
        let gpio = self.read_register(Register::Gpio)?;
        let levels = core::array::from_fn(|index| {
            if gpio & (1 << index) != 0 {
                Level::High
            } else {
                Level::Low
            }
        });

        Ok(levels)
    }

    /// Configures a pin direction.
    pub fn set_direction(&mut self, pin: Pin, direction: Direction) -> Result<(), Error> {
        self.write_bit(Register::Iodir, pin, direction == Direction::Input)
    }

    /// Configures all pin directions in one register write.
    ///
    /// Index 0 maps to GP0 and index 7 maps to GP7.
    pub fn set_directions(&mut self, directions: [Direction; 8]) -> Result<(), Error> {
        self.write_register(
            Register::Iodir,
            Self::pack_bits(directions, |dir| dir == Direction::Input),
        )
    }

    /// Updates the output latch for a pin.
    pub fn set_output(&mut self, pin: Pin, state: OutputState) -> Result<(), Error> {
        self.write_bit(Register::Olat, pin, state == OutputState::Released)
    }

    /// Updates the output latch for all pins in one register write.
    ///
    /// Index 0 maps to GP0 and index 7 maps to GP7.
    pub fn set_outputs(&mut self, states: [OutputState; 8]) -> Result<(), Error> {
        self.write_register(
            Register::Olat,
            Self::pack_bits(states, |state| state == OutputState::Released),
        )
    }

    /// Enables or disables the internal pull-up on a pin.
    pub fn set_pull_up(&mut self, pin: Pin, pull_up: PullUp) -> Result<(), Error> {
        self.write_bit(Register::Gppu, pin, pull_up == PullUp::Enabled)
    }

    /// Enables or disables the internal pull-up on all pins in one register write.
    ///
    /// Index 0 maps to GP0 and index 7 maps to GP7.
    pub fn set_pull_ups(&mut self, pull_ups: [PullUp; 8]) -> Result<(), Error> {
        self.write_register(
            Register::Gppu,
            Self::pack_bits(pull_ups, |pull_up| pull_up == PullUp::Enabled),
        )
    }

    /// Enables or disables interrupt generation for a pin.
    pub fn set_interrupt(&mut self, pin: Pin, enabled: InterruptEnable) -> Result<(), Error> {
        self.write_bit(Register::Gpinten, pin, enabled == InterruptEnable::Enabled)
    }

    /// Enables or disables interrupts on all pins in one register write.
    ///
    /// Index 0 maps to GP0 and index 7 maps to GP7.
    pub fn set_interrupts(&mut self, enabled: [InterruptEnable; 8]) -> Result<(), Error> {
        self.write_register(
            Register::Gpinten,
            Self::pack_bits(enabled, |state| state == InterruptEnable::Enabled),
        )
    }
}

// Internals.
impl<'d> Mcp23009<'d> {
    const MIN_ADDRESS: u8 = 0x20;
    const MAX_ADDRESS: u8 = 0x27;

    fn validate_address(address: u8) -> Result<(), Error> {
        if (Self::MIN_ADDRESS..=Self::MAX_ADDRESS).contains(&address) {
            Ok(())
        } else {
            Err(Error::InvalidAddress(address))
        }
    }

    fn read_register(&mut self, register: Register) -> Result<u8, Error> {
        let mut value = [0u8; 1];
        self.i2c
            .write_read(self.address, &[register as u8], &mut value)?;

        Ok(value[0])
    }

    fn write_register(&mut self, register: Register, value: u8) -> Result<(), Error> {
        self.i2c.write(self.address, &[register as u8, value])?;

        Ok(())
    }

    fn read_bit(&mut self, register: Register, pin: Pin) -> Result<bool, Error> {
        let reg = self.read_register(register)?;
        let mask = 1 << (pin as u8);

        Ok(reg & mask != 0)
    }

    fn write_bit(&mut self, register: Register, pin: Pin, enabled: bool) -> Result<(), Error> {
        let mask = 1 << (pin as u8);
        let mut value = self.read_register(register)?;
        if enabled {
            value |= mask;
        } else {
            value &= !mask;
        }
        self.write_register(register, value)
    }

    fn pack_bits<T>(values: [T; 8], is_set: impl Fn(T) -> bool) -> u8 {
        values
            .into_iter()
            .enumerate()
            .fold(0, |mask, (index, value)| match is_set(value) {
                true => mask | (1 << index),
                false => mask,
            })
    }
}
