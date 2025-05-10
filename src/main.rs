#![no_std]
#![no_main]
// #![allow(unused_imports)]
#![allow(unused_variables)]
// #![allow(unreachable_code)]
// #![allow(unused_must_use)]
// #![allow(unused_mut)]
#![allow(dead_code)]

extern crate alloc;

mod task;
mod types;

use core::result::Result;
use embassy_executor::{SpawnError, Spawner};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio;
use esp_hal::timer::systimer::SystemTimer;
use esp_println::println;
use static_cell::StaticCell;
use task::pin_control::PinControlChannel;

// NOTES
// - esp_println sends prints to 'jtag-serial' via the USB port (set in Cargo.toml)

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz);
    let peripherals = esp_hal::init(esp_config);

    //
    // M5Stamp-C3U pinout
    //
    // A default output config with a 5mA drive strength.
    let output_5ma = gpio::OutputConfig::default().with_drive_strength(gpio::DriveStrength::_5mA);
    // Unused pins, taken here so they aren't used accidentally.
    let _pin9_unused = peripherals.GPIO9;
    // Onboard LED on the M5Stamp C3U.
    let _pin_led_onboard = peripherals.GPIO2;
    // UART pins.
    let _pin_uart_tx = peripherals.GPIO20;
    let _pin_uart_rx = peripherals.GPIO21;
    // G5 triggers the controller power button on level:high (via nMOS).
    let pin_button_power = gpio::Output::new(peripherals.GPIO5, gpio::Level::Low, output_5ma);
    // G6, G7, G8, G10 trigger controller buttons on level:low (via SPST switches).
    let pin_button_menu = gpio::Output::new(peripherals.GPIO6, gpio::Level::High, output_5ma);
    let pin_button_enter = gpio::Output::new(peripherals.GPIO7, gpio::Level::High, output_5ma);
    let pin_button_down = gpio::Output::new(peripherals.GPIO8, gpio::Level::High, output_5ma);
    let pin_button_up = gpio::Output::new(peripherals.GPIO10, gpio::Level::High, output_5ma);
    // G4 reads the case button, which pulls the line to GND when pressed.
    let pin_button_case = gpio::Input::new(
        peripherals.GPIO4,
        gpio::InputConfig::default().with_pull(gpio::Pull::Up),
    );
    // G0 commands the DS18B20 temperature sensor, which is phantom-powered.
    let pin_sensor_temp = gpio::Output::new(
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
    // G18 reads the tachometer in the case fan.
    // TODO: fan tachy on USB pins?
    let pin_fan_tachy = gpio::Input::new(peripherals.GPIO18, gpio::InputConfig::default());
    // G19 sends a PWM signal to the fans. A high signal corresponds to 100% duty cycle.
    let pin_fan_pwm = gpio::Output::new(peripherals.GPIO19, gpio::Level::High, output_5ma);

    //
    // Resume initialization.
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);
    // let mut rng = rng::Rng::new(peripherals.RNG);
    println!("imac5k display controller initialized");

    //
    // Channel setup.
    //
    // A channel to send pin control messages.
    static PINCONTROL_CHANNEL: StaticCell<PinControlChannel> = StaticCell::new();
    let pincontrol_channel = PINCONTROL_CHANNEL.init_with(|| PinControlChannel::new());

    //
    // Spawn tasks.
    || -> Result<(), SpawnError> {
        spawner.spawn(task::pin_control(
            pincontrol_channel.receiver(),
            pin_button_power,
            pin_button_menu,
            pin_button_enter,
            pin_button_down,
            pin_button_up,
            pin_power_display,
            pin_power_fan,
        ))?;

        Ok(())
    }()
    .unwrap();

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
