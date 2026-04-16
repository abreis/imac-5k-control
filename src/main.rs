#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]
#![feature(impl_trait_in_assoc_type)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

mod config;
mod driver;
mod ioexpander;
mod memlog;
mod task;

use crate::ioexpander::IoExpander;
use embassy_executor::{SpawnError, Spawner};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::i2c::master::I2c;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{gpio, i2c};

// Default app-descriptor required by the esp-idf bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let esp_config = esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz);
    let peripherals = esp_hal::init(esp_config);

    // Heap allocation. ESP32C6 has 512KB SRAM for its high-speed core.
    // - 64 KiB reclaimed only: safest if heap demand is low
    // - 64 KiB reclaimed + 96 KiB regular: very safe
    // - 64 KiB reclaimed + 128 KiB regular: still comfortable
    // - 64 KiB reclaimed + 160 KiB regular: probably still fine, but we would measure first
    // To measure, print `esp_alloc::HEAP.stats()` after boot and again after Wi‑Fi + MQTT are connected.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    // esp_alloc::heap_allocator!(size: 160 * 1024);
    esp_alloc::heap_allocator!(size: 224 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);
    let rng = esp_hal::rng::Rng::new();

    // Initialize an in-memory logger with space for 480 characters.
    let memlog = memlog::init(480);
    memlog.enable_print();
    memlog.info("init: imac5k display controller");
    memlog.info("init: hardware initialized");

    //
    // XIAO ESP32C6 pinout
    //

    // A default output config with a 5mA drive strength.
    let output_5ma = gpio::OutputConfig::default().with_drive_strength(gpio::DriveStrength::_5mA);

    // G0 sends a PWM signal to the fan. A high signal corresponds to 100% duty cycle.
    let pin_fan_pwm = gpio::Output::new(peripherals.GPIO0, gpio::Level::High, output_5ma);
    // G1 reads the fan tachometer. The external pull-up and RC filter are on the board.
    let pin_fan_tachy = gpio::Input::new(
        peripherals.GPIO1,
        gpio::InputConfig::default().with_pull(gpio::Pull::None),
    );
    // G2 is the 1Wire bus commanding the DS18B20 temperature sensors, which are phantom-powered.
    let pin_sensor_display_temp = peripherals.GPIO2;
    // G3+G14 drive the RF switch. Hold G3 low to enable switch control and G14 low to select
    // the onboard antenna (high would select the external U.FL antenna).
    let _pin_rf_switch_ctrl = gpio::Output::new(peripherals.GPIO3, gpio::Level::Low, output_5ma);
    let _pin4_unused = peripherals.GPIO4;
    let _pin5_unused = peripherals.GPIO5;
    let _pin6_unused = peripherals.GPIO6;
    let _pin7_unused = peripherals.GPIO7;
    let _pin8_unused = peripherals.GPIO8;
    let _pin9_unused = peripherals.GPIO9;
    let _pin10_unused = peripherals.GPIO10;
    let _pin11_unused = peripherals.GPIO11;
    let _pin12_unused = peripherals.GPIO12;
    let _pin13_unused = peripherals.GPIO13;
    // Antenna selection (see G3).
    let _pin_antenna_sel = gpio::Output::new(peripherals.GPIO14, gpio::Level::Low, output_5ma);
    let _pin15_unused = peripherals.GPIO15;
    // UART pins.
    let pin_uart_tx = peripherals.GPIO16;
    let pin_uart_rx = peripherals.GPIO17;
    // G18 drives the low-side MOSFET for the 24V relay coil feeding the display controller board.
    let pin_power_display_relay =
        gpio::Output::new(peripherals.GPIO18, gpio::Level::Low, output_5ma);
    // G19 controls the buzzer.
    let pin_buzzer = gpio::Output::new(
        peripherals.GPIO19,
        gpio::Level::Low,
        gpio::OutputConfig::default().with_drive_strength(gpio::DriveStrength::_5mA),
    );
    // G20 reads the case button, which pulls the line to GND when pressed.
    let pin_button_case = peripherals.GPIO20;
    // G21 is the MCP23009 interrupt pin. We will poll initially, but reserve it now.
    let _pin_io_expander_int = gpio::Input::new(peripherals.GPIO21, gpio::InputConfig::default());
    // G22/G23 carry the MCP23009 I2C bus.
    let pin_i2c_sda = peripherals.GPIO22;
    let pin_i2c_scl = peripherals.GPIO23;
    // Display-board buttons and LEDs live behind the MCP23009.
    // GP0 green LED, GP1 red LED, GP2 power, GP3 up, GP4 down, GP5 enter, GP6 menu.

    memlog.info("init: board pinout configured");

    //
    // Task initialization.
    //

    // Initialize the I2C bus.
    let i2c_config = i2c::master::Config::default().with_frequency(Rate::from_khz(400));
    let i2c_master = I2c::new(peripherals.I2C0, i2c_config)
        .unwrap()
        .with_sda(pin_i2c_sda)
        .with_scl(pin_i2c_scl);

    // Initialize the IO expander.
    let mcp23009 = driver::mcp23009::Mcp23009::new(i2c_master);
    let ioexpander = IoExpander::init(mcp23009).unwrap();

    // Set up the WiFi.
    let (wifi_controller, wifi_interfaces) = task::wifi::init(peripherals.WIFI).await.unwrap();

    // Set up the network stack.
    let (net_stack, net_runner) = task::net::init(wifi_interfaces.station, rng).await;

    // Get a shareable channel to send buzzer control messages.
    let buzzer_channel = task::buzzer::init();

    // Get a watch to know when the case button has been pressed.
    let casebutton_watch = task::case_button::init::<2>();

    // Get a shareable channel to send messages to the pincontrol task.
    let (pincontrol_pubsub, displayled_watch) = task::pin_control::init::<4, 3, 3>();

    // Init the fan duty PWM controller.
    let (pwm_channel, fanduty_watch, fantachy_watch) =
        task::fan_control::init::<4>(peripherals.LEDC, pin_fan_pwm);

    // Get a watcher to await changes in temperature sensor readings.
    let tempsensor_watch = task::temp_sensor::init::<5>();

    // Get a watcher to monitor the network interface.
    let netstatus_watch = task::net_monitor::init::<3>();

    // Get a command channel and state watcher for the display-controller power relay.
    let (powerrelay_channel, powerrelay_watch) = task::power_relay::init::<4, 3>();

    // Get a watcher for the consolidated display-board state.
    let displayboard_watch = task::display_state::init::<4>();

    // WRITEME
    let (control_signal, event_channel) = task::serial_tui::init();

    // // Set up the internal temperature sensor.
    // let _onboard_sensor =
    //     tsens::TemperatureSensor::new(peripherals.TSENS, tsens::Config::default()).unwrap();

    memlog.info("init: tasks initialized");

    //
    // Spawn tasks.
    || -> Result<(), SpawnError> {
        // Run the buzzer controller.
        spawner.spawn(task::buzzer_control(pin_buzzer, buzzer_channel))?;

        // Control the display-board buttons behind the MCP23009 and watch the board LEDs.
        spawner.spawn(task::pin_control(
            ioexpander,
            pincontrol_pubsub.dyn_subscriber().unwrap(),
            displayled_watch.dyn_sender(),
            buzzer_channel,
            memlog,
        ))?;

        // Keep the wifi connected.
        spawner.spawn(task::wifi::wifi_permanent_connection(
            wifi_controller,
            memlog,
        ))?;

        // Run the network stack.
        spawner.spawn(task::net::stack_runner(net_runner))?;

        // Monitor the network stack for changes.
        spawner.spawn(task::net_monitor(net_stack, netstatus_watch.dyn_sender()))?;

        // Operate the display-controller power relay.
        spawner.spawn(task::power_relay(
            pin_power_display_relay,
            powerrelay_channel.dyn_receiver(),
            powerrelay_watch.dyn_sender(),
        ))?;

        // Recognize display-board state from LEDs and relay.
        spawner.spawn(task::display_board(
            displayled_watch.dyn_receiver().unwrap(),
            powerrelay_watch.dyn_receiver().unwrap(),
            displayboard_watch.dyn_sender(),
            memlog,
        ))?;

        // Handle power-on and power-off sequences on command.
        spawner.spawn(task::display_control(
            casebutton_watch.dyn_receiver().unwrap(),
            displayboard_watch.dyn_receiver().unwrap(),
            pincontrol_pubsub.dyn_publisher().unwrap(),
            powerrelay_channel.dyn_sender(),
            buzzer_channel,
            memlog,
        ))?;

        // Watch the case button for presses.
        spawner.spawn(task::case_button(
            pin_button_case.into(),
            casebutton_watch.dyn_sender(),
            buzzer_channel,
            memlog,
        ))?;

        // Control the case fan duty cycle.
        spawner.spawn(task::fan_duty(
            pwm_channel,
            fanduty_watch.dyn_receiver().unwrap(),
        ))?;

        // Read the fan tachometer periodically.
        spawner.spawn(task::fan_tachy(pin_fan_tachy, fantachy_watch.dyn_sender()))?;

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

        // Hardware safety watchdog.
        spawner.spawn(task::watchdog(
            tempsensor_watch.dyn_receiver().unwrap(),
            fantachy_watch.dyn_receiver().unwrap(),
            fanduty_watch.dyn_sender(),
            powerrelay_channel.dyn_sender(),
            buzzer_channel,
            memlog,
        ))?;

        // Spawn the MQTT control task.
        spawner.spawn(task::mqtt::run(
            net_stack,
            fanduty_watch.dyn_receiver().unwrap(),
            fantachy_watch.dyn_receiver().unwrap(),
            pincontrol_pubsub.dyn_publisher().unwrap(),
            pincontrol_pubsub.dyn_subscriber().unwrap(),
            netstatus_watch.dyn_receiver().unwrap(),
            tempsensor_watch.dyn_receiver().unwrap(),
            displayboard_watch.dyn_receiver().unwrap(),
            memlog,
        ))?;

        // Launch the UART interface event stream.
        spawner.spawn(task::serial_tui::tui_event_stream(
            displayled_watch.dyn_receiver().unwrap(),
            fanduty_watch.dyn_receiver().unwrap(),
            fantachy_watch.dyn_receiver().unwrap(),
            netstatus_watch.dyn_receiver().unwrap(),
            powerrelay_watch.dyn_receiver().unwrap(),
            tempsensor_watch.dyn_receiver().unwrap(),
            displayboard_watch.dyn_receiver().unwrap(),
            memlog,
            control_signal,
            event_channel,
        ))?;

        // Launch the UART control interface.
        spawner.spawn(task::serial_tui::run(
            peripherals.UART0.into(),
            pin_uart_rx.into(),
            pin_uart_tx.into(),
            pincontrol_pubsub.dyn_publisher().unwrap(),
            fanduty_watch.dyn_sender(),
            powerrelay_channel.dyn_sender(),
            memlog,
            control_signal,
            event_channel,
        ))?;

        Ok(())
    }()
    .unwrap();

    memlog.info("init: tasks spawned");
}
