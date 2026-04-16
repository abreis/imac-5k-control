use crate::{
    memlog::SharedLogger,
    task::{
        buzzer::{BuzzerAction, BuzzerChannel, BuzzerPattern},
        fan_control::{FAN_TACHY_MEASURE_INTERVAL, FanDutyDynSender, FanTachyDynReceiver},
        power_relay::{PowerRelayDynSender, RelayCommand},
        temp_sensor::TempSensorDynReceiver,
    },
};
use alloc::format;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, with_timeout};

// Trip the relay if temperature exceeds this.
const MAX_SAFE_TEMP_C: f32 = 85.0;
// Trip the relay if temp sensor fails and fan tachy is below this.
const MIN_SAFE_FAN_RPM: u16 = 2000;

const SAFETY_ALARM_PATTERN: BuzzerPattern = &[
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 320 },
    BuzzerAction::Pause { ms: 100 },
    BuzzerAction::Beep { ms: 320 },
];

#[embassy_executor::task]
pub async fn watchdog(
    mut tempsensor_receiver: TempSensorDynReceiver,
    mut fantachy_receiver: FanTachyDynReceiver,
    fanduty_sender: FanDutyDynSender,
    powerrelay_sender: PowerRelayDynSender,
    buzzer_channel: BuzzerChannel,
    memlog: SharedLogger,
) {
    let missing_temp_window = {
        use crate::task::temp_sensor::*;
        4 * (TEMP_READING_INTERVAL + SENSOR_MEASUREMENT_TIME * (CHECKSUM_RETRIES as u32))
    };
    let mut missing_temp_deadline = Instant::now() + missing_temp_window;
    let mut fan_park_sent_since_last_good_temp = false;
    let mut last_tachy_at: Option<Instant> = None;
    let mut last_tachy_rpm = 0;

    loop {
        let timeout = {
            let now = Instant::now();
            if missing_temp_deadline <= now {
                Duration::from_millis(0)
            } else {
                missing_temp_deadline - now
            }
        };

        let sensor_or_tach = select(tempsensor_receiver.changed(), fantachy_receiver.changed());
        match with_timeout(timeout, sensor_or_tach).await {
            Ok(Either::First(reading)) => {
                if let Ok(temp_c) = reading.temperature {
                    missing_temp_deadline = reading.timestamp + missing_temp_window;
                    fan_park_sent_since_last_good_temp = false;

                    if temp_c > MAX_SAFE_TEMP_C {
                        powerrelay_sender.send(RelayCommand::ForceOpenLatch).await;
                        buzzer_channel.send(SAFETY_ALARM_PATTERN).await;
                        memlog.warn(format!("safety: overtemp {temp_c:.1}c"));
                    }
                }
            }

            Ok(Either::Second(rpm)) => {
                last_tachy_at = Some(Instant::now());
                last_tachy_rpm = rpm;
            }

            Err(_timeout) => {
                missing_temp_deadline = Instant::now() + missing_temp_window;

                if !fan_park_sent_since_last_good_temp {
                    fanduty_sender.send(100);
                    fan_park_sent_since_last_good_temp = true;
                    memlog.warn("watchdog: no valid temperature updates, fan -> 100%");
                }

                let tachy_fresh = last_tachy_at
                    .map(|timestamp| timestamp.elapsed() <= FAN_TACHY_MEASURE_INTERVAL * 2)
                    .unwrap_or(false);

                if !(tachy_fresh && last_tachy_rpm > MIN_SAFE_FAN_RPM) {
                    powerrelay_sender.send(RelayCommand::ForceOpenLatch).await;
                    buzzer_channel.send(SAFETY_ALARM_PATTERN).await;
                    memlog.warn(format!(
                        "watchdog: no temp, unsafe fan tachy ({last_tachy_rpm}rpm)"
                    ));
                }
            }
        }
    }
}
