use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, watch};
use embassy_time::{Duration, Instant, Timer};
use esp_ds18b20::{Ds18b20, Ds18b20Error, Resolution, SensorData};
use esp_hal::gpio;
use esp_onewire::{OneWireBus, OneWireBusError};

pub type TempSensorWatch<const W: usize> =
    &'static watch::Watch<NoopRawMutex, TemperatureReading, W>;
pub type TempSensorDynSender = watch::DynSender<'static, TemperatureReading>;
pub type TempSensorDynReceiver = watch::DynReceiver<'static, TemperatureReading>;

#[derive(Copy, Clone, Debug)]
pub struct TemperatureReading {
    pub timestamp: Instant,
    pub temperature: Result<f32, Ds18b20Error>,
    pub retries: u8,
}

pub fn init<const WATCHERS: usize>() -> TempSensorWatch<WATCHERS> {
    Box::leak(Box::new(watch::Watch::new()))
}

const DSPL_TEMP_SENSOR_ADDRESS: u64 = 0xF682AA490B646128;
// const PSU_TEMP_SENSOR_ADDRESS: u64 = 0xF682AA490B646128;
// How long to wait between temperature readings.
const TEMP_MEASUREMENT_INTERVAL: Duration = Duration::from_secs(5);
// How many attempts to retry reading after a checksum error.
const CHECKSUM_RETRIES: u8 = 3;

#[embassy_executor::task]
pub async fn temp_sensor(
    onewire_pin: gpio::AnyPin<'static>,
    tempsensor_sender: TempSensorDynSender,
) {
    let onewire_bus = OneWireBus::new(onewire_pin);
    let mut sensor = Ds18b20::new(DSPL_TEMP_SENSOR_ADDRESS, onewire_bus).unwrap();

    loop {
        Timer::after(TEMP_MEASUREMENT_INTERVAL).await;

        let mut retries = 0;

        let sensor_reading = 'checksum_retries: loop {
            // Attempt to catch errors from 1Wire.
            let reading: Result<SensorData, Ds18b20Error> = async {
                // Begin a measurement and wait for it to complete.
                sensor.start_temp_measurement()?;

                // 12bit resolution is the default, expects a 750ms wait time.
                let wait_time_ms = Resolution::Bits12.measurement_time_ms();
                let wait_time = Duration::from_millis(wait_time_ms as u64);
                Timer::after(wait_time).await;

                let data = sensor.read_sensor_data()?;

                Ok(data)
            }
            .await;

            // Retry on checksum errors.
            match reading {
                Err(Ds18b20Error::OneWireError(OneWireBusError::ChecksumFailed))
                    if retries < CHECKSUM_RETRIES =>
                {
                    retries += 1;
                    continue 'checksum_retries;
                }
                _ => {
                    break 'checksum_retries reading;
                }
            }
        };

        // Pull out the temperature and add a timestamp to our reading.
        let reading = TemperatureReading {
            timestamp: Instant::now(),
            temperature: sensor_reading.map(|data| data.temperature),
            retries,
        };

        tempsensor_sender.send(reading);
    }
}
