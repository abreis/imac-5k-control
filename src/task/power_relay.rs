#![allow(dead_code)]
use alloc::boxed::Box;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel, watch};
use esp_hal::gpio;

pub type PowerRelayChannel<const N: usize> =
    &'static channel::Channel<NoopRawMutex, RelayCommand, N>;
pub type PowerRelayDynSender = channel::DynamicSender<'static, RelayCommand>;
pub type PowerRelayDynReceiver = channel::DynamicReceiver<'static, RelayCommand>;

pub type PowerRelayWatch<const W: usize> = &'static watch::Watch<NoopRawMutex, RelayStatus, W>;
pub type PowerRelayStateDynSender = watch::DynSender<'static, RelayStatus>;
pub type PowerRelayStateDynReceiver = watch::DynReceiver<'static, RelayStatus>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayCommand {
    Open,
    Close,
    ForceOpenLatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayStatus {
    Open,
    Closed,
    ForcedOpen,
}

#[must_use]
pub fn init<const BACKLOG: usize, const WATCHERS: usize>()
-> (PowerRelayChannel<BACKLOG>, PowerRelayWatch<WATCHERS>) {
    let relay_channel = Box::leak(Box::new(channel::Channel::new()));
    let relay_watch = Box::leak(Box::new(watch::Watch::new()));

    (relay_channel, relay_watch)
}

#[embassy_executor::task]
pub async fn power_relay(
    mut pin_power_display_relay: gpio::Output<'static>,
    relay_receiver: PowerRelayDynReceiver,
    relay_state_sender: PowerRelayStateDynSender,
) {
    let mut state = RelayStatus::Open;
    pin_power_display_relay.set_low();
    relay_state_sender.send(state);

    loop {
        let command = relay_receiver.receive().await;

        if state != RelayStatus::ForcedOpen {
            match command {
                RelayCommand::Close => {
                    state = RelayStatus::Closed;
                    pin_power_display_relay.set_high();
                }

                RelayCommand::Open => {
                    state = RelayStatus::Open;
                    pin_power_display_relay.set_low();
                }

                RelayCommand::ForceOpenLatch => {
                    state = RelayStatus::ForcedOpen;
                    pin_power_display_relay.set_low();
                }
            }

            relay_state_sender.send(state);
        }
    }
}
