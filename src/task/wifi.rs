use crate::memlog::SharedLogger;
use alloc::format;
use alloc::string::ToString;
use embassy_time::{Duration, Timer};
use esp_hal::peripherals;
use esp_radio::wifi::{self, Config, ControllerConfig, PowerSaveMode, sta::StationConfig};

use crate::config::WIFI_PASS;
use crate::config::WIFI_SSID;

// How long to wait before attempting to reconnect to WiFi.
const WIFI_RECONNECT_PAUSE: Duration = Duration::from_secs(5);

/// Initializes the WiFi in client mode.
///
/// Returns a WiFi controller and WiFi interfaces.
///
/// Sets a hardcoded SSID and passphrase, and disables power save for performance.
pub async fn init(
    wifi: peripherals::WIFI<'static>,
) -> Result<(wifi::WifiController<'static>, wifi::Interfaces<'static>), wifi::WifiError> {
    // Allow some time before initializing the (power-hungry) WiFi.
    Timer::after(Duration::from_millis(250)).await;

    let wifi_config =
        ControllerConfig::default().with_country_info(wifi::CountryInfo::from(*b"NZ"));
    let (mut wifi_controller, wifi_interfaces) = esp_radio::wifi::new(wifi, wifi_config).unwrap();

    // Set the wifi client configuration.
    let wifi_client_config = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASS.to_string());
    wifi_controller.set_config(&Config::Station(wifi_client_config))?;

    // Disable power saving, can cause random packet delay and loss (#3014).
    wifi_controller.set_power_saving(PowerSaveMode::None)?;

    Ok((wifi_controller, wifi_interfaces))
}

#[embassy_executor::task]
pub async fn wifi_permanent_connection(
    mut controller: wifi::WifiController<'static>,
    memlog: SharedLogger,
) {
    loop {
        // If we're still connected, wait until we disconnect.
        if controller.is_connected() {
            if let Ok(info) = controller.wait_for_disconnect_async().await {
                memlog.info(format!("wifi: disconnected: {:?}", info.reason));
            }
        }

        // Pause before attempting to reconnect.
        Timer::after(WIFI_RECONNECT_PAUSE).await;

        match controller.connect_async().await {
            Ok(_info) => memlog.info("wifi: connected"),
            Err(error) => memlog.debug(format!("wifi: connect error: {:?}", error)),
        }
    }
}
