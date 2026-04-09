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

/// Pin logic level.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Level {
    /// Logic low.
    Low,
    /// Logic high.
    High,
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

impl<'d> Mcp23009<'d> {
    pub const DEFAULT_ADDRESS: u8 = 0x20;
    const MIN_ADDRESS: u8 = 0x20;
    const MAX_ADDRESS: u8 = 0x27;

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

    /// Configures a pin as an input.
    pub fn set_input(&mut self, pin: Pin) -> Result<(), Error> {
        self.write_bit(Register::Iodir, pin, true)
    }

    /// Configures a pin as an output with the given initial open-drain state.
    pub fn set_output(&mut self, pin: Pin, initial: OutputState) -> Result<(), Error> {
        self.write_bit(Register::Olat, pin, initial == OutputState::Released)?;
        self.write_bit(Register::Iodir, pin, false)
    }

    /// Reads the current logic level on a pin.
    pub fn read_pin(&mut self, pin: Pin) -> Result<Level, Error> {
        let level = if self.read_bit(Register::Gpio, pin)? {
            Level::High
        } else {
            Level::Low
        };

        Ok(level)
    }

    /// Updates the output latch for a pin.
    pub fn write_output(&mut self, pin: Pin, state: OutputState) -> Result<(), Error> {
        self.write_bit(Register::Olat, pin, state == OutputState::Released)
    }

    /// Enables or disables the internal pull-up on a pin.
    pub fn set_pull_up(&mut self, pin: Pin, pull_up: PullUp) -> Result<(), Error> {
        self.write_bit(Register::Gppu, pin, pull_up == PullUp::Enabled)
    }

    /// Enables or disables interrupt generation for a pin.
    pub fn set_interrupt(&mut self, pin: Pin, enabled: InterruptEnable) -> Result<(), Error> {
        self.write_bit(Register::Gpinten, pin, enabled == InterruptEnable::Enabled)
    }

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
}
