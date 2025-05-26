/*
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
let pin_sensor_temp = peripherals.GPIO0;
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
*/

/*
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
let _pin21_unused = peripherals.GPIO21;
let _pin22_unused = peripherals.GPIO22;
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
// G6 commands the DS18B20 temperature sensor, which is phantom-powered.
let pin_sensor_temp = peripherals.GPIO6;
// G18 goes to the Power MOSFET gate that switches 24VDC power on to the display controller.
// IRLZ44N I_gate = 48nC * 1Hz = 48nA (current depends on switching frequency)
let pin_power_display = gpio::Output::new(peripherals.GPIO18, gpio::Level::Low, output_5ma);
// G7 goes to the nMOS gate that switches 12VDC power on to the case fan.
let pin_power_fan = gpio::Output::new(peripherals.GPIO7, gpio::Level::Low, output_5ma);
// G19 reads the tachometer in the case fan.
let _pin_fan_tachy = gpio::Input::new(peripherals.GPIO19, gpio::InputConfig::default());
// G20 sends a PWM signal to the fans. A high signal corresponds to 100% duty cycle.
let pin_fan_pwm = gpio::Output::new(peripherals.GPIO20, gpio::Level::High, output_5ma);
*/
