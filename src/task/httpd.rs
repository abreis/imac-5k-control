use super::{net_monitor::NetStatusDynReceiver, temp_sensor::TempSensorDynReceiver};
use crate::{
    memlog::{self, SharedLogger},
    state::SharedState,
    task::{
        fan_duty::FanDutySignal,
        pin_control::{OnOff, PinControlChannel, PinControlMessage},
    },
};
use alloc::{
    boxed::Box,
    format,
    rc::Rc,
    string::{String, ToString},
};
use core::cell::RefCell;
use embassy_executor::{SpawnError, Spawner};
use embassy_time::Duration;
use picoserve::{
    AppBuilder, AppRouter, Config, Router, Timeouts,
    routing::{PathRouter, get, parse_path_segment, post},
};

const HTTPD_MOTD: &str =
    const_format::formatcp!("{} {}\n", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

/// Number of workers to spawn.
pub const HTTPD_WORKERS: usize = 2;

/// Server port.
pub const HTTPD_PORT: u16 = 80;

/// Server timeouts, chosen for operation over embedded WiFi.
pub const HTTPD_TIMEOUTS: Timeouts<Duration> = Timeouts {
    // Timeout for the initial request on a new connection, accommodating potential WiFi latency.
    start_read_request: Some(Duration::from_secs(5)),
    // Shorter timeout for subsequent requests on an existing persistent (keep-alive) connection.
    persistent_start_read_request: Some(Duration::from_secs(2)),
    // Timeout if the server has started reading a request but stalls (e.g., client sends partial data).
    read_request: Some(Duration::from_secs(3)),
    // Timeout if the server is writing a response but the client is not reading it promptly.
    write: Some(Duration::from_secs(3)),
};

pub const HTTPD_CONFIG: Config<Duration> =
    Config::new(HTTPD_TIMEOUTS).close_connection_after_response(); // .keep_connection_alive();

pub fn launch_workers(
    spawner: Spawner,
    stack: embassy_net::Stack<'static>,
    pincontrol_channel: PinControlChannel,
    fanduty_signal: FanDutySignal,
    netstatus_receiver: NetStatusDynReceiver,
    tempsensor_receiver: TempSensorDynReceiver,
    state: SharedState,
    memlog: SharedLogger,
) -> Result<(), SpawnError> {
    let app = AppProps {
        netstatus_receiver,
        tempsensor_receiver,
        pincontrol_channel,
        fanduty_signal,
        state,
        memlog,
    }
    .build_app();
    let app: &'static AppRouter<AppProps> = Box::leak(Box::new(app));

    for worker_id in 0..HTTPD_WORKERS {
        spawner.spawn(worker(worker_id, stack, app))?;
    }

    Ok(())
}

#[embassy_executor::task(pool_size = HTTPD_WORKERS)]
pub async fn worker(
    worker_id: usize,
    stack: embassy_net::Stack<'static>,
    app: &'static AppRouter<AppProps>,
) {
    let mut tcp_rx_buffer = [0; 1024];
    let mut tcp_tx_buffer = [0; 1024];
    let mut http_buffer = [0; 2048];

    picoserve::listen_and_serve(
        worker_id,
        app,
        &HTTPD_CONFIG,
        stack,
        HTTPD_PORT,
        &mut tcp_rx_buffer,
        &mut tcp_tx_buffer,
        &mut http_buffer,
    )
    .await
}

//
// HTTP routing.

struct AppProps {
    netstatus_receiver: NetStatusDynReceiver,
    tempsensor_receiver: TempSensorDynReceiver,
    pincontrol_channel: PinControlChannel,
    fanduty_signal: FanDutySignal,
    state: SharedState,
    memlog: SharedLogger,
}
impl AppBuilder for AppProps {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        let app: &'static RefCell<AppProps> = Box::leak(Box::new(RefCell::new(self)));

        picoserve::Router::new()
            .route("/", get(|| async { HTTPD_MOTD }))
            .route(
                "/help",
                get(|| async {
                    "GET /button/power\n\
                     GET /button/menu\n\
                     GET /button/enter\n\
                     GET /button/down\n\
                     GET /button/up\n\
                     GET /power/display/{on,off}\n\
                     GET /power/fan/{on,off}\n\
                     GET /fan/pwm/<duty>\n\
                     GET /state\n\
                     GET /temp\n\
                     GET /net\n\
                     GET /log\n\
                     GET /log/clear\n\
                     GET /help\n"
                }),
            )
            // Button routes
            .route(
                "/button/power",
                get(|| async {
                    app.borrow()
                        .pincontrol_channel
                        .send(PinControlMessage::ButtonPower)
                        .await;
                    "Triggered button 'power'\n"
                }),
            )
            .route(
                "/button/menu",
                get(|| async {
                    app.borrow()
                        .pincontrol_channel
                        .send(PinControlMessage::ButtonMenu)
                        .await;
                    "Triggered button 'menu'\n"
                }),
            )
            .route(
                "/button/enter",
                get(|| async {
                    app.borrow()
                        .pincontrol_channel
                        .send(PinControlMessage::ButtonEnter)
                        .await;
                    "Triggered button 'enter'\n"
                }),
            )
            .route(
                "/button/down",
                get(|| async {
                    app.borrow()
                        .pincontrol_channel
                        .send(PinControlMessage::ButtonDown)
                        .await;
                    "Triggered button 'down'\n"
                }),
            )
            .route(
                "/button/up",
                get(|| async {
                    app.borrow()
                        .pincontrol_channel
                        .send(PinControlMessage::ButtonUp)
                        .await;
                    "Triggered button 'up'\n"
                }),
            )
            // Power routes
            .route(
                ("/power/display", parse_path_segment()),
                get(move |action: String| async move {
                    match action.as_str() {
                        "on" => {
                            app.borrow()
                                .pincontrol_channel
                                .send(PinControlMessage::DisplayPower(OnOff::On))
                                .await;
                            "Display power turned on\n"
                        }
                        "off" => {
                            app.borrow()
                                .pincontrol_channel
                                .send(PinControlMessage::DisplayPower(OnOff::Off))
                                .await;
                            "Display power turned off\n"
                        }
                        _ => "Invalid action\n",
                    }
                }),
            )
            .route(
                ("/power/fan", parse_path_segment()),
                get(move |action: String| async move {
                    match action.as_str() {
                        "on" => {
                            app.borrow()
                                .pincontrol_channel
                                .send(PinControlMessage::FanPower(OnOff::On))
                                .await;
                            "Fan power turned on\n"
                        }
                        "off" => {
                            app.borrow()
                                .pincontrol_channel
                                .send(PinControlMessage::FanPower(OnOff::Off))
                                .await;
                            "Fan power turned off\n"
                        }
                        _ => "Invalid action\n",
                    }
                }),
            )
            // Fan PWM route
            .route(
                ("/fan/pwm", parse_path_segment()),
                get(move |duty: u8| async move {
                    if (0u8..=100).contains(&duty) {
                        app.borrow_mut().fanduty_signal.signal(duty);
                        format!("Fan duty set to {duty}\n")
                    } else {
                        "Fan duty value must be between 0 and 100\n".to_string()
                    }
                }),
            )
            // State route
            .route(
                "/state",
                get(|| async { format!("{:#?}\n", app.borrow().state.get()) }),
            )
            .route(
                "/temp",
                get(|| async {
                    let value = app.borrow_mut().tempsensor_receiver.try_get();
                    format!("{:#?}\n", value)
                }),
            )
            .route(
                "/net",
                get(|| async {
                    let value = app.borrow_mut().netstatus_receiver.try_get();
                    format!("{:#?}\n", value)
                }),
            )
            .route(
                "/log",
                get(|| async {
                    app.borrow()
                        .memlog
                        .records()
                        .iter()
                        .rev()
                        .map(|record| {
                            let timestamp =
                                memlog::format_milliseconds_to_hms(record.instant.as_millis());
                            format!("[{}] {}: {}\n", timestamp, record.level, record.text)
                        })
                        .collect::<String>()
                }),
            )
            .route(
                "/log/clear",
                get(|| async {
                    app.borrow().memlog.clear();
                    "Logs cleared\n"
                }),
            )
    }
}
