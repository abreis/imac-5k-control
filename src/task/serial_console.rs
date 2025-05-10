use crate::{
    task::{fan_duty::FANDUTY_SIGNAL, pin_control::PINCONTROL_CHANNEL},
    types::{ControlMessage, OnOff},
};
use embassy_time::{Duration, Timer};
use esp_hal::{gpio, uart};

// Number of bytes to allocate to keep a history of commands.
const COMMAND_HISTORY_BUFFER_SIZE: usize = 1000; // in bytes
const MOTD: &'static str = const_format::formatcp!(
    "\r\n{} {}\r\n",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_VERSION")
);

/// Triggers actions controlled by output pins.
#[embassy_executor::task]
pub async fn serial_console(
    peripheral_uart: uart::AnyUart,
    pin_uart_tx: gpio::AnyPin,
    pin_uart_rx: gpio::AnyPin,
) {
    // UART setup.
    let mut uart = uart::Uart::new(peripheral_uart, uart::Config::default())
        .unwrap()
        .with_rx(pin_uart_rx)
        .with_tx(pin_uart_tx)
        .into_async();

    // Line editor setup.
    let mut input_buffer = [0u8; 100]; // Commands are short, could be smaller
    let mut history_buffer = [0u8; COMMAND_HISTORY_BUFFER_SIZE];
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
            // A reusable fixed-capacity string to store replies to user commands.
            // let mut response_buffer = ArrayString::<100>::new();

            // Note: Ctrl-C and Ctrl-D break the readline while loop.
            while let Ok(line) = editor.readline(prompt, &mut uart).await {
                let response = cli_parser(line).await;
                uart.write_async(response.as_bytes()).await?;
            }

            Ok(())
        }()
        .await;

        let Err(tx_error) = catch else { unreachable!() };
        // TODO: error handling, push UART errors to central log.
        let _ = tx_error;

        // Pause before trying the UART again after an error.
        Timer::after(Duration::from_secs(5)).await;
    } // loop
}

async fn cli_parser(line: &str) -> &'static str {
    use OnOff::*;

    // Get the command from the first argument.
    let mut chunks = line.split_whitespace();
    match chunks.next() {
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

        None => "Please enter a command",
        _ => "Unrecognized command",
    }
}
