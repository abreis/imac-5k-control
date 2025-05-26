#![no_std]
#![no_main]
// #![allow(unused_imports)]
// #![allow(dead_code)]
// #![allow(unused_variables)]
// #![allow(unreachable_code)]
// #![allow(unused_must_use)]
// #![allow(unused_mut)]

extern crate alloc;

mod ds18b20;
mod memlog;
mod onewire;
mod power;
mod state;
mod task;

use core::result::Result;
use embassy_executor::{SpawnError, Spawner};
use embassy_time::Duration;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::{gpio, tsens};
use esp_println::println;

// NOTES
// - esp_println sends prints to 'jtag-serial' via the USB port (set in Cargo.toml)
//
// TODO
// - we can probably run at a lower clock speed (runs less hot)

const DSPL_TEMP_SENSOR_ADDRESS: u64 = 0xF682AA490B646128;
// const PSU_TEMP_SENSOR_ADDRESS: u64 = 0xF682AA490B646128;

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz);
    let peripherals = esp_hal::init(esp_config);

    //
    // C6-SuperMini pinout
    //
    // A default output config with a 5mA drive strength.
    let output_5ma = gpio::OutputConfig::default().with_drive_strength(gpio::DriveStrength::_5mA);
    // Unused pins, taken here so they aren't used accidentally.
    let _pin8_unused = peripherals.GPIO8;
    let _pin9_unused = peripherals.GPIO9;
    let _pin12_unused = peripherals.GPIO12;
    let _pin13_unused = peripherals.GPIO13;
    let _pin14_unused = peripherals.GPIO14;
    let _pin15_unused = peripherals.GPIO15;
    // let _pin21_unused = peripherals.GPIO21;
    // let _pin22_unused = peripherals.GPIO22;
    let _pin23_unused = peripherals.GPIO23;
    // UART pins.
    let pin_uart_tx = peripherals.GPIO16;
    let pin_uart_rx = peripherals.GPIO17;
    // G1 triggers the controller power button on level:high (via nMOS).
    let pin_button_power = gpio::Output::new(peripherals.GPIO1, gpio::Level::Low, output_5ma);
    // G5, G4, G3, G2 trigger controller buttons on level:low (via SPST switches).
    let pin_button_menu = gpio::Output::new(peripherals.GPIO5, gpio::Level::High, output_5ma);
    let pin_button_enter = gpio::Output::new(peripherals.GPIO4, gpio::Level::High, output_5ma);
    let pin_button_down = gpio::Output::new(peripherals.GPIO3, gpio::Level::High, output_5ma);
    let pin_button_up = gpio::Output::new(peripherals.GPIO2, gpio::Level::High, output_5ma);
    // G0 reads the case button, which pulls the line to GND when pressed.
    let pin_button_case = peripherals.GPIO0;
    // G6 is the 1Wire bus commanding the DS18B20 temperature sensors, which are phantom-powered.
    let pin_sensor_display_temp = peripherals.GPIO6;
    // G18 goes to the Power MOSFET gate that switches 24VDC power on to the display controller.
    // IRLZ44N I_gate = 48nC * 1Hz = 48nA (current depends on switching frequency)
    let pin_power_display = gpio::Output::new(peripherals.GPIO18, gpio::Level::Low, output_5ma);
    // G7 goes to the nMOS gate that switches 12VDC power on to the case fan.
    let pin_power_fan = gpio::Output::new(peripherals.GPIO7, gpio::Level::Low, output_5ma);
    // G19 reads the tachometer in the case fan.
    let _pin_fan_tachy = gpio::Input::new(peripherals.GPIO19, gpio::InputConfig::default());
    // G20 sends a PWM signal to the fans. A high signal corresponds to 100% duty cycle.
    let pin_fan_pwm = gpio::Output::new(peripherals.GPIO20, gpio::Level::High, output_5ma);
    // G21 and G22 track the status LEDs on the display board.
    let _pin_display_led_red = gpio::Input::new(peripherals.GPIO21, gpio::InputConfig::default());
    let _pin_display_led_green = gpio::Input::new(peripherals.GPIO22, gpio::InputConfig::default());

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

    // Get a watcher to await changes in temperature sensor readings.
    let tempsensor_watch = task::temp_sensor::init();

    // Set up the internal temperature sensor.
    let _onboard_sensor =
        tsens::TemperatureSensor::new(peripherals.TSENS, tsens::Config::default()).unwrap();

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
            tempsensor_watch.dyn_receiver().unwrap(),
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

        // Take a temperature measurement every 10 seconds.
        const TEMP_MEASUREMENT_INTERVAL: Duration = Duration::from_secs(10);
        spawner.spawn(task::temp_sensor(
            pin_sensor_display_temp.into(),
            DSPL_TEMP_SENSOR_ADDRESS,
            tempsensor_watch.dyn_sender(),
            TEMP_MEASUREMENT_INTERVAL,
        ))?;

        // Keep adjusting the fan duty based on the temperature measurements.
        spawner.spawn(task::fan_temp_control(
            fanduty_signal,
            tempsensor_watch.dyn_receiver().unwrap(),
        ))?;

        Ok(())
    }()
    .unwrap();
}
