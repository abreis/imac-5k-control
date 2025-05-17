#![no_std]
#![no_main]
// #![allow(unused_imports)]
// #![allow(dead_code)]
// #![allow(unused_variables)]
// #![allow(unreachable_code)]
// #![allow(unused_must_use)]
// #![allow(unused_mut)]

extern crate alloc;

mod memlog;
mod power;
mod state;
mod task;

use core::result::Result;
use embassy_executor::{SpawnError, Spawner};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio;
use esp_hal::timer::systimer::SystemTimer;
use esp_println::println;

// NOTES
// - esp_println sends prints to 'jtag-serial' via the USB port (set in Cargo.toml)
//
// TODO
// - we can probably run at a lower clock speed (runs less hot)

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz);
    let peripherals = esp_hal::init(esp_config);

    //
    // M5Stamp-C3U pinout
    //
    // A default output config with a 5mA drive strength.
    let output_5ma = gpio::OutputConfig::default().with_drive_strength(gpio::DriveStrength::_5mA);
    // Unused pins, taken here so they aren't used accidentally.
    let _pin18_unused = peripherals.GPIO18;
    let _pin19_unused = peripherals.GPIO19;
    // Onboard LED on the M5Stamp C3U.
    let _pin_led_onboard = peripherals.GPIO2;
    // UART pins.
    let pin_uart_rx = peripherals.GPIO20;
    let pin_uart_tx = peripherals.GPIO21;
    // G5 triggers the controller power button on level:high (via nMOS).
    let pin_button_power = gpio::Output::new(peripherals.GPIO5, gpio::Level::Low, output_5ma);
    // G6, G7, G8, G10 trigger controller buttons on level:low (via SPST switches).
    let pin_button_menu = gpio::Output::new(peripherals.GPIO6, gpio::Level::High, output_5ma);
    let pin_button_enter = gpio::Output::new(peripherals.GPIO7, gpio::Level::High, output_5ma);
    let pin_button_down = gpio::Output::new(peripherals.GPIO8, gpio::Level::High, output_5ma);
    let pin_button_up = gpio::Output::new(peripherals.GPIO10, gpio::Level::High, output_5ma);
    // G4 reads the case button, which pulls the line to GND when pressed.
    let pin_button_case = peripherals.GPIO4;
    // G0 commands the DS18B20 temperature sensor, which is phantom-powered.
    let _pin_sensor_temp = gpio::Output::new(
        peripherals.GPIO0,
        gpio::Level::High,
        gpio::OutputConfig::default()
            .with_pull(gpio::Pull::Up)
            .with_drive_strength(gpio::DriveStrength::_40mA),
    );
    // G3 goes to the Power MOSFET gate that switches 24VDC power on to the display controller.
    // IRLZ44N I_gate = 48nC * 1Hz = 48nA (current depends on switching frequency)
    let pin_power_display = gpio::Output::new(peripherals.GPIO3, gpio::Level::Low, output_5ma);
    // G1 goes to the nMOS gate that switches 12VDC power on to the case fan.
    let pin_power_fan = gpio::Output::new(peripherals.GPIO1, gpio::Level::Low, output_5ma);
    // TODO: fan control is on USB pins? requires a pin09 held low + reset to be able to flash again
    // // G18 reads the tachometer in the case fan.
    // let _pin_fan_tachy = gpio::Input::new(peripherals.GPIO18, gpio::InputConfig::default());
    // G19 sends a PWM signal to the fans. A high signal corresponds to 100% duty cycle.
    // let pin_fan_pwm = gpio::Output::new(peripherals.GPIO19, gpio::Level::High, output_5ma);
    // Note: this is a strapping pin. Should be safe to use as an output.
    let pin_fan_pwm = gpio::Output::new(peripherals.GPIO9, gpio::Level::High, output_5ma);

    //
    // Resume initialization.
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);
    println!("imac5k display controller initialized");

    // Initialize an in-memory logger with space for 480 characters.
    let memlog = memlog::init(480);
    memlog.info("imac5k display controller initialized");

    // A shared state for the display.
    let state = state::SharedState::new_standby();

    // Init the fan duty PWM controller.
    let (pwm_channel, fanduty_signal) = task::fan_duty::init(peripherals.LEDC, pin_fan_pwm);

    // Get a shareable channel to send messages to the pincontrol task.
    let pincontrol_channel = task::pin_control::init();

    //
    // Spawn tasks.
    || -> Result<(), SpawnError> {
        spawner.spawn(task::pin_control(
            pin_button_power,
            pin_button_menu,
            pin_button_enter,
            pin_button_down,
            pin_button_up,
            pin_power_display,
            pin_power_fan,
            pincontrol_channel,
        ))?;
        spawner.spawn(task::serial_console(
            peripherals.UART0.into(),
            pin_uart_rx.into(),
            pin_uart_tx.into(),
            pincontrol_channel,
            fanduty_signal,
            state,
            memlog,
        ))?;
        spawner.spawn(task::case_button(
            state,
            pin_button_case.into(),
            pincontrol_channel,
            memlog,
        ))?;

        spawner.spawn(task::fan_duty(pwm_channel, fanduty_signal))?;

        Ok(())
    }()
    .unwrap();
}
