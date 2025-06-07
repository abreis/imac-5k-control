use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel};
use embassy_time::Timer;
use esp_hal::gpio;

const CHANNEL_BACKLOG: usize = 5;

pub type BuzzerChannel = &'static channel::Channel<NoopRawMutex, BuzzerPattern, CHANNEL_BACKLOG>;
pub type BuzzerPattern = &'static [BuzzerAction];

#[derive(Clone, Copy, Debug)]
pub enum BuzzerAction {
    Beep { ms: u32 },
    Pause { ms: u32 },
}

pub fn init() -> BuzzerChannel {
    Box::leak(Box::new(channel::Channel::new()))
}

/// Plays patterns on the buzzer pin.
#[embassy_executor::task]
pub async fn buzzer_control(mut pin_buzzer: gpio::Output<'static>, buzzer_channel: BuzzerChannel) {
    // Queue a pattern on buzzer init.
    buzzer_channel
        .send([BuzzerAction::Beep { ms: 100 }].as_ref().into())
        .await;

    pin_buzzer.set_low();

    loop {
        let pattern = buzzer_channel.receive().await;
        for step in pattern.iter() {
            match step {
                BuzzerAction::Beep { ms } => {
                    pin_buzzer.set_high();
                    Timer::after_millis(*ms as u64).await;
                    pin_buzzer.set_low();
                }
                BuzzerAction::Pause { ms } => Timer::after_millis(*ms as u64).await,
            }
        }
        pin_buzzer.set_low();

        // Min 1 second pause between any consecutive beep sequences.
        Timer::after_secs(1).await;
    }
}
