use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel};
use embassy_time::{Duration, Timer};
use esp_hal::gpio;

// How long to toggle button control pins for.
const BUTTON_DELAY_MS: Duration = Duration::from_millis(100);

pub static PINCONTROL_CHANNEL: channel::Channel<CriticalSectionRawMutex, PinControlMessage, 5> =
    channel::Channel::new();

#[derive(Copy, Clone)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Copy, Clone)]
pub enum PinControlMessage {
    ButtonPower,
    ButtonMenu,
    ButtonEnter,
    ButtonDown,
    ButtonUp,
    DisplayPower(OnOff),
    FanPower(OnOff),
}

/// Triggers actions controlled by output pins.
#[embassy_executor::task]
pub async fn pin_control(
    mut pin_button_power: gpio::Output<'static>,
    mut pin_button_menu: gpio::Output<'static>,
    mut pin_button_enter: gpio::Output<'static>,
    mut pin_button_down: gpio::Output<'static>,
    mut pin_button_up: gpio::Output<'static>,
    mut pin_power_display: gpio::Output<'static>,
    mut pin_power_fan: gpio::Output<'static>,
) {
    loop {
        use OnOff::*;
        use PinControlMessage::*;
        match PINCONTROL_CHANNEL.receive().await {
            // Power button is active high.
            ButtonPower => {
                pin_button_power.set_high();
                Timer::after(BUTTON_DELAY_MS).await;
                pin_button_power.set_low();
            }
            // Menu, Enter, Up, Down buttons are active low.
            ButtonMenu => {
                pin_button_menu.set_low();
                Timer::after(BUTTON_DELAY_MS).await;
                pin_button_menu.set_high();
            }
            ButtonEnter => {
                pin_button_enter.set_low();
                Timer::after(BUTTON_DELAY_MS).await;
                pin_button_enter.set_high();
            }
            ButtonDown => {
                pin_button_down.set_low();
                Timer::after(BUTTON_DELAY_MS).await;
                pin_button_down.set_high();
            }
            ButtonUp => {
                pin_button_up.set_low();
                Timer::after(BUTTON_DELAY_MS).await;
                pin_button_up.set_high();
            }
            // Power control MOSFETs get turned on or off.
            DisplayPower(value) => match value {
                On => pin_power_display.set_high(),
                Off => pin_power_display.set_low(),
            },
            FanPower(value) => match value {
                On => pin_power_fan.set_high(),
                Off => pin_power_fan.set_low(),
            },
        }
    }
}
