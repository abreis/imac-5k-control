use super::pin_control::PINCONTROL_CHANNEL;
use crate::{
    memlog, power,
    state::{self, State},
};
use embassy_time::Duration;
use esp_hal::gpio;

const BUTTON_HELD_DURATION_MIN: Duration = Duration::from_millis(500);
const BUTTON_HELD_DURATION_MAX: Duration = Duration::from_millis(2000);

#[embassy_executor::task]
pub async fn case_button(pin: gpio::AnyPin) {
    // Initialize the pin with a pull-up. The button is wired to GND.
    let mut case_pin =
        gpio::Input::new(pin, gpio::InputConfig::default().with_pull(gpio::Pull::Up));

    // Wait for the pin to go low for a given amount of time.
    // Ignore the click if its duration is too short. Shortcircuit if the button is held for a long time.
    // The idea here is that the user can hold the button 'until something happens', not knowing how long that takes.
    loop {
        case_pin.wait_for_falling_edge().await;
        let fall_time = embassy_time::Instant::now();

        let wait_for_high = case_pin.wait_for_high();
        let _ = embassy_time::with_timeout(BUTTON_HELD_DURATION_MAX, wait_for_high).await;

        let held_duration = fall_time.elapsed();
        if held_duration > BUTTON_HELD_DURATION_MIN {
            memlog::debug("case button triggered").await;

            match state::get().await {
                State::Standby => power::power_on().await.unwrap(),
                State::DisplayOn => power::power_off().await.unwrap(),
                _invalid_state => {
                    memlog::warn("case button pressed while in invalid state, ignored").await
                }
            }
        }
    }
}
