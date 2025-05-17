use crate::vendor::{
    ds18b20::{self, Ds18b20},
    one_wire_bus::{self, OneWire, OneWireError},
};
use alloc::{boxed::Box, format, string::String};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, pubsub, signal, watch};
use embassy_time::{Duration, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use esp_hal::gpio;

const TEMPSENSOR_MAX_RECEIVERS: usize = 2;

pub type TempSensorWatch =
    &'static watch::Watch<NoopRawMutex, TempSensorReading, TEMPSENSOR_MAX_RECEIVERS>;
pub type TempSensorDynSender = watch::DynSender<'static, TempSensorReading>;
pub type TempSensorDynReceiver = watch::DynReceiver<'static, TempSensorReading>;

pub type TempSensorReading = Result<ds18b20::SensorData, OneWireError<core::convert::Infallible>>;

pub fn init() -> TempSensorWatch {
    Box::leak(Box::new(watch::Watch::new()))
}

#[embassy_executor::task]
pub async fn temp_sensor(
    onewire_pin: gpio::AnyPin,
    sensor_address: u64,
    tempsensor_sender: TempSensorDynSender,
    loop_secs: u64,
) {
    let mut one_wire_pin = gpio::Flex::new(onewire_pin);
    one_wire_pin.enable_input(true);
    one_wire_pin.set_drive_strength(gpio::DriveStrength::_40mA);
    one_wire_pin.set_as_open_drain(gpio::Pull::Up);
    let mut onewire_bus = OneWire::new(one_wire_pin).unwrap();

    let sensor_addr = one_wire_bus::Address(sensor_address);
    let sensor = Ds18b20::new::<core::convert::Infallible>(sensor_addr).unwrap();

    let mut blocking_delay = embassy_time::Delay;

    loop {
        Timer::after(Duration::from_secs(loop_secs)).await;

        // Attempt to catch errors from
        let sensor_reading =
            async || -> Result<ds18b20::SensorData, OneWireError<core::convert::Infallible>> {
                // Begin a measurement and wait for it to complete.
                sensor.start_temp_measurement(&mut onewire_bus, &mut blocking_delay)?;
                Timer::after(Duration::from_millis(750)).await; // 750ms for 12bit resolution (default)

                let data = sensor.read_data(&mut onewire_bus, &mut blocking_delay)?;

                Ok(data)
            }()
            .await;

        // Translate the result into
        // let sensor_reading = match catch {
        //     Ok(data) => TempSensorReading::Temperature(data.temperature),
        //     Err(error) => TempSensorReading::Error(error),
        // };

        tempsensor_sender.send(sensor_reading);
    }
}
