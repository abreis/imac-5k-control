use alloc::boxed::Box;
use embassy_net::{self as net};
use esp_hal::rng::Rng;
use esp_radio::wifi;

/// Maximum number of sockets to allocate memory for.
const NET_SOCKETS: usize = 3;
use crate::config::NET_CONFIG;

pub async fn init(
    driver: wifi::Interface<'static>,
    rng: Rng,
) -> (
    net::Stack<'static>,
    net::Runner<'static, wifi::Interface<'static>>,
) {
    // Memory resources for the network stack.
    let net_resources = Box::leak::<'static>(Box::new(net::StackResources::<NET_SOCKETS>::new()));

    let seed_64b = (rng.random() as u64) << 32 | rng.random() as u64;
    let (net_stack, net_runner) = net::new(driver, NET_CONFIG.clone(), net_resources, seed_64b);

    (net_stack, net_runner)
}

/// Drives the network stack.
#[embassy_executor::task]
pub async fn stack_runner(mut runner: net::Runner<'static, wifi::Interface<'static>>) {
    runner.run().await
}
