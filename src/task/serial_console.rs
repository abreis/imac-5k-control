use crate::{
    memlog,
    task::{fan_duty::FANDUTY_SIGNAL, pin_control::PINCONTROL_CHANNEL},
    types::{ControlMessage, OnOff},
};
use alloc::{format, string::String};
use embassy_time::{Duration, Timer};
use esp_hal::{Async, gpio, uart};

// Number of bytes to allocate to keep a history of commands.
const COMMAND_HISTORY_BUFFER_SIZE: usize = 1000; // in bytes
const MOTD: &'static str = const_format::formatcp!(
    "\r\n{} {}\r\n",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_VERSION")
);

// Uart::write_async doesn't guarantee it will send everything.
trait UartWriteAllAsync {
    async fn write_all_async(&mut self, data: &[u8]) -> Result<(), uart::TxError>;
}
impl UartWriteAllAsync for uart::Uart<'_, Async> {
    async fn write_all_async(&mut self, mut data: &[u8]) -> Result<(), uart::TxError> {
        while !data.is_empty() {
            let bytes_written = self.write_async(data).await?;
            data = &data[bytes_written..];
        }
        Ok(())
    }
}

/// Triggers actions controlled by output pins.
#[embassy_executor::task]
pub async fn serial_console(
    peripheral_uart: uart::AnyUart,
    pin_uart_rx: gpio::AnyPin,
    pin_uart_tx: gpio::AnyPin,
) {
    // UART setup. When in loopback mode, ensure TX is configured first (#2914).
    let mut uart = uart::Uart::new(peripheral_uart, uart::Config::default())
        .unwrap()
        .with_tx(pin_uart_tx)
        .with_rx(pin_uart_rx)
        .into_async();

    // Line editor setup.
    let mut input_buffer = [0u8; 100]; // Commands are short, could be smaller
    let mut history_buffer = [0u8; COMMAND_HISTORY_BUFFER_SIZE];
    // let mut editor = noline::builder::EditorBuilder::new_unbounded()
    let mut editor = noline::builder::EditorBuilder::from_slice(&mut input_buffer)
        .with_slice_history(&mut history_buffer)
        .build_async(&mut uart)
        .await
        .unwrap(); // always returns Ok()

    loop {
        // Try block to catch UART errors.
        let catch = async || -> Result<(), uart::TxError> {
            // Write the MOTD out.
            uart.write_async(MOTD.as_bytes()).await?;

            let prompt = "> ";
            // Note: Ctrl-C and Ctrl-D break the readline while loop.
            while let Ok(line) = editor.readline(prompt, &mut uart).await {
                cli_parser(line, &mut uart).await?;
            }

            Ok(())
        }()
        .await;

        if let Err(tx_error) = catch {
            // Push the UART error to the memlog.
            memlog::warn(format!("uart error: {}", tx_error)).await;
        }

        // Pause before trying the UART again after an error.
        Timer::after(Duration::from_secs(1)).await;
    } // loop
}

async fn cli_parser(
    line: &str,
    uart: &mut uart::Uart<'static, Async>,
) -> Result<(), uart::TxError> {
    use OnOff::*;

    // Get the command from the first argument.
    let mut chunks = line.split_whitespace();
    let response = match chunks.next() {
        Some("help") => {
            "button\r\n\
             · power\r\n\
             · menu\r\n\
             · enter\r\n\
             · down\r\n\
             · up\r\n\
             power\r\n\
             · display {on|off}\r\n\
             · fan {on|off}\r\n\
             fan\r\n\
             · pwm <duty>\r\n\
             · tachy\r\n\
             log\r\n\
             · read\r\n\
             · clear"
        }

        // Trigger display controller buttons.
        Some("button") => match chunks.next() {
            Some("power") => {
                PINCONTROL_CHANNEL.send(ControlMessage::ButtonPower).await;
                "Triggered button 'power'"
            }
            Some("menu") => {
                PINCONTROL_CHANNEL.send(ControlMessage::ButtonMenu).await;
                "Triggered button 'menu'"
            }
            Some("enter") => {
                PINCONTROL_CHANNEL.send(ControlMessage::ButtonEnter).await;
                "Triggered button 'enter'"
            }
            Some("down") => {
                PINCONTROL_CHANNEL.send(ControlMessage::ButtonDown).await;
                "Triggered button 'down'"
            }
            Some("up") => {
                PINCONTROL_CHANNEL.send(ControlMessage::ButtonUp).await;
                "Triggered button 'up'"
            }
            None => "Subcommand required for 'button'",
            _ => "Invalid subcommand for 'button'",
        },

        // Control 24V power to display.
        Some("power") => match chunks.next() {
            Some("display") => match chunks.next() {
                Some("on") => {
                    PINCONTROL_CHANNEL
                        .send(ControlMessage::DisplayPower(On))
                        .await;
                    "Display power turned on"
                }
                Some("off") => {
                    PINCONTROL_CHANNEL
                        .send(ControlMessage::DisplayPower(Off))
                        .await;
                    "Display power turned off"
                }
                None => "Subcommand required for 'power display'",
                _ => "Invalid subcommand for 'power display'",
            },
            Some("fan") => match chunks.next() {
                Some("on") => {
                    PINCONTROL_CHANNEL.send(ControlMessage::FanPower(On)).await;
                    "Fan power turned on"
                }
                Some("off") => {
                    PINCONTROL_CHANNEL.send(ControlMessage::FanPower(Off)).await;
                    "Fan power turned off"
                }
                None => "Subcommand required for 'power fan'",
                _ => "Invalid subcommand for 'power fan'",
            },
            None => "Subcommand required for 'power'",
            _ => "Invalid subcommand for 'power'",
        },

        // Fan controls, duty cycle and tachometer.
        Some("fan") => match chunks.next() {
            Some("pwm") => match chunks.next() {
                Some(pwm_value) => match pwm_value.parse::<u8>() {
                    Ok(value) => {
                        if (0..=100).contains(&value) {
                            FANDUTY_SIGNAL.signal(value);
                            "Fan duty set"
                        } else {
                            "Fan duty value must be between 0 and 100"
                        }
                    }
                    Err(_parse_error) => "Failed to parse fan duty value",
                },
                None => "Fan duty value required for 'fan pwm'",
            },
            Some("tachy") => "TODO: Fan tachometer readout",
            None => "Subcommand required for 'fan'",
            _ => "Invalid subcommand for 'fan'",
        },

        // Log control.
        Some("log") => match chunks.next() {
            Some("read") => {
                // Note: this locks the entire memlog while it is being printed.
                for record in memlog::records().await.iter().rev() {
                    let timestamp = format_milliseconds_to_hms(record.instant.as_millis());
                    let formatted =
                        format!("[{}] {}: {}\r\n", timestamp, record.level, record.text);
                    uart.write_all_async(formatted.as_bytes()).await?;
                }
                ""
            }
            Some("clear") => {
                memlog::clear().await;
                "Logs cleared"
            }
            None => "Subcommand required for 'log'",
            _ => "Invalid subcommand for 'log'",
        },

        None => "Please enter a command",
        _ => "Unrecognized command",
    };

    if !response.is_empty() {
        uart.write_all_async(response.as_bytes()).await?;
        uart.write_all_async(b"\r\n").await?;
    }

    Ok(())
}

/// Formats a u64 millisecond value into "HHHHH:MM:SS.xxx" string.
#[inline]
fn format_milliseconds_to_hms(total_ms: u64) -> String {
    let millis_part = total_ms % 1000;
    let total_seconds = total_ms / 1000;

    let seconds_part = total_seconds % 60;
    let total_minutes = total_seconds / 60;

    let minutes_part = total_minutes % 60;
    let hours_part = total_minutes / 60;

    format!(
        "{:05}:{:02}:{:02}.{:03}",
        hours_part, minutes_part, seconds_part, millis_part
    )
}
