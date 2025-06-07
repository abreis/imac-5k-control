#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

mod memlog;
mod power;
mod state;
mod task;

use embassy_executor::{SpawnError, Spawner};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;

// NOTES
// - esp_println sends prints to 'jtag-serial' via the USB port

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz);
    let peripherals = esp_hal::init(esp_config);
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);
    let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    let timer1 = TimerGroup::new(peripherals.TIMG0);

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
    let pin_button_back = gpio::Output::new(peripherals.GPIO4, gpio::Level::High, output_5ma);
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
    // G14 controls the buzzer.
    let pin_buzzer = gpio::Output::new(
        peripherals.GPIO14,
        gpio::Level::Low,
        gpio::OutputConfig::default().with_drive_strength(gpio::DriveStrength::_5mA),
    );

    // Initialize an in-memory logger with space for 480 characters.
    let memlog = memlog::init(480);
    memlog.info("imac5k display controller initialized");

    // Set up the WiFi.
    let (wifi_controller, wifi_interfaces) =
        task::wifi::init(timer1.timer0, peripherals.RADIO_CLK, peripherals.WIFI, rng)
            .await
            .unwrap();

    // Set up the network stack.
    let (net_stack, net_runner) = task::net::init(wifi_interfaces.sta, rng).await;

    // A shared state for the display.
    let state = state::SharedState::new_standby();

    // Get a shareable channel to send messages to the pincontrol task.
    let pincontrol_channel = task::pin_control::init();

    // Get a shareable channel to send buzzer control messages.
    let buzzer_channel = task::buzzer::init();

    //
    // Watcher count: 1 for serial console, 1 for httpd.

    // Init the fan duty PWM controller.
    let (pwm_channel, fanduty_watch) = task::fan_duty::init::<3>(peripherals.LEDC, pin_fan_pwm);

    // Get a watcher to await changes in temperature sensor readings.
    let tempsensor_watch = task::temp_sensor::init::<3>();

    // Get a watcher to monitor the network interface.
    let netstatus_watch = task::net_monitor::init::<2>();

    // // Set up the internal temperature sensor.
    // let _onboard_sensor =
    //     tsens::TemperatureSensor::new(peripherals.TSENS, tsens::Config::default()).unwrap();

    //
    // Spawn tasks.
    || -> Result<(), SpawnError> {
        // Run the buzzer controller.
        spawner.spawn(task::buzzer_control(pin_buzzer, buzzer_channel))?;

        // Keep the wifi connected.
        spawner.spawn(task::wifi::permanent_connection(wifi_controller, memlog))?;

        // Run the network stack.
        spawner.spawn(task::net::stack_runner(net_runner))?;

        // Monitor the network stack for changes.
        spawner.spawn(task::net_monitor(net_stack, netstatus_watch.dyn_sender()))?;

        // Control the buttons on the display board.
        spawner.spawn(task::pin_control(
            pin_button_power,
            pin_button_menu,
            pin_button_back,
            pin_button_down,
            pin_button_up,
            pin_power_display,
            pin_power_fan,
            pincontrol_channel,
        ))?;

        // Launch a control interface on UART0.
        spawner.spawn(task::serial_console(
            peripherals.UART0.into(),
            pin_uart_rx.into(),
            pin_uart_tx.into(),
            pincontrol_channel,
            fanduty_watch.dyn_sender(),
            fanduty_watch.dyn_receiver().unwrap(),
            netstatus_watch.dyn_receiver().unwrap(),
            tempsensor_watch.dyn_receiver().unwrap(),
            state,
            memlog,
        ))?;

        // Watch the case button for presses.
        spawner.spawn(task::case_button(
            state,
            pin_button_case.into(),
            pincontrol_channel,
            buzzer_channel,
            memlog,
        ))?;

        // Control the case fan duty cycle.
        spawner.spawn(task::fan_duty(
            pwm_channel,
            fanduty_watch.dyn_receiver().unwrap(),
        ))?;

        // Take a temperature measurement periodically.
        spawner.spawn(task::temp_sensor(
            pin_sensor_display_temp.into(),
            tempsensor_watch.dyn_sender(),
        ))?;

        // Keep adjusting the fan duty based on the temperature measurements.
        spawner.spawn(task::fan_temp_control(
            fanduty_watch.dyn_sender(),
            tempsensor_watch.dyn_receiver().unwrap(),
        ))?;

        // Launch httpd workers.
        task::httpd::launch_workers(
            spawner,
            net_stack,
            pincontrol_channel,
            fanduty_watch.dyn_sender(),
            fanduty_watch.dyn_receiver().unwrap(),
            netstatus_watch.dyn_receiver().unwrap(),
            tempsensor_watch.dyn_receiver().unwrap(),
            state,
            memlog,
        )?;

        Ok(())
    }()
    .unwrap();
}
