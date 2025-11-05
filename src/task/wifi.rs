use crate::memlog::SharedLogger;
use alloc::{boxed::Box, format};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals;
use esp_radio::wifi::{self, PowerSaveMode, WifiStaState};

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

    let radio_init = Box::leak::<'static>(Box::new(esp_radio::init().unwrap()));
    let wifi_config = wifi::Config::default().with_country_code(wifi::CountryInfo::from(*b"NZ"));
    let (mut wifi_controller, wifi_interfaces) =
        esp_radio::wifi::new(radio_init, wifi, wifi_config).unwrap();

    // Set the wifi client configuration.
    let wifi_client_config = wifi::ClientConfig::default()
        .with_ssid(WIFI_SSID.into())
        .with_password(WIFI_PASS.into());
    wifi_controller.set_config(&wifi::ModeConfig::Client(wifi_client_config))?;

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
        if wifi::sta_state() == WifiStaState::Connected {
            controller
                .wait_for_event(wifi::WifiEvent::StaDisconnected)
                .await;
        }

        // Pause before attempting to reconnect.
        Timer::after(WIFI_RECONNECT_PAUSE).await;

        // Start the WiFi controller if necessary.
        if !matches!(controller.is_started(), Ok(true)) {
            memlog.info("wifi: starting controller");
            controller.start_async().await.unwrap();
        }

        match controller.connect_async().await {
            Ok(()) => memlog.info("wifi: connected"),
            Err(error) => memlog.debug(format!("wifi: connect error: {:?}", error)),
        }
    }
}
