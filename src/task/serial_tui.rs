use super::{
    display_board::{DisplayBoardDynReceiver, DisplayBoardState},
    fan_control::{FanDutyDynReceiver, FanDutyDynSender, FanTachyDynReceiver},
    net_monitor::{NetStatusDynReceiver, NetworkStatus},
    pin_control::{DisplayLedDynReceiver, LedState, PinControlMessage, PinControlPublisher},
    power_relay::{PowerRelay, PowerRelayCommand, PowerRelayDynSender, PowerRelayStateDynReceiver},
    temp_sensor::{TempSensorDynReceiver, TemperatureReading},
};
use crate::memlog::{Record, SharedLogger};
use alloc::{boxed::Box, format, vec::Vec};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel, signal};
use embassy_time::{Duration, Timer};
use esp_hal::{gpio, uart};

// const UART_BAUD_RATE: u32 = 115_200;
const UART_BAUD_RATE: u32 = 921_600;

const SESSION_TIMEOUT: Duration = Duration::from_secs(5 * 60);

const PANEL_HEIGHT: u16 = 10;
const BUTTON_PANEL_WIDTH: u16 = 24;
const MAX_FAN_INPUT_LEN: usize = 3;
const TEMP_HISTORY_LEN: usize = 24;
const LOG_LINE_COUNT: usize = 12;
const EVENT_CHANNEL_CAPACITY: usize = 16;

struct ButtonSpec {
    label: &'static str,
    message: PinControlMessage,
}
const BUTTONS: [ButtonSpec; 5] = [
    ButtonSpec {
        label: "power",
        message: PinControlMessage::ButtonPower,
    },
    ButtonSpec {
        label: "menu",
        message: PinControlMessage::ButtonMenu,
    },
    ButtonSpec {
        label: "up",
        message: PinControlMessage::ButtonUp,
    },
    ButtonSpec {
        label: "down",
        message: PinControlMessage::ButtonDown,
    },
    ButtonSpec {
        label: "back",
        message: PinControlMessage::ButtonBack,
    },
];

pub enum Event {
    Led(LedState),
    FanDuty(u8),
    FanTachy(u16),
    Net(NetworkStatus),
    Relay(PowerRelay),
    Temperature(TemperatureReading),
    DisplayBoard(DisplayBoardState),
    LogsSnapshot(Vec<Record>),
    TimedOut,
}
type EventChannel = channel::Channel<NoopRawMutex, Event, EVENT_CHANNEL_CAPACITY>;

#[derive(Clone, Copy, PartialEq)]
pub enum SessionCommand {
    Start,
    Stop,
}
type SessionControlSignal = signal::Signal<NoopRawMutex, SessionCommand>;

fn collect_logs(memlog: SharedLogger, limit: usize) -> Vec<Record> {
    let records = memlog.records();
    let mut logs = Vec::with_capacity(limit.min(records.len()));

    for record in records.iter().take(limit).rev() {
        logs.push(record.clone());
    }

    logs
}

#[embassy_executor::task]
pub async fn tui_event_stream(
    mut displayled_receiver: DisplayLedDynReceiver,
    mut fanduty_receiver: FanDutyDynReceiver,
    mut fantachy_receiver: FanTachyDynReceiver,
    mut netstatus_receiver: NetStatusDynReceiver,
    mut powerrelay_receiver: PowerRelayStateDynReceiver,
    mut tempsensor_receiver: TempSensorDynReceiver,
    mut displayboard_receiver: DisplayBoardDynReceiver,
    memlog: SharedLogger,
    control_signal: &'static SessionControlSignal,
    event_channel: &'static EventChannel,
) {
    memlog.enable_watch();
    let mut logwatch_receiver = memlog.watch().unwrap();

    loop {
        loop {
            if control_signal.wait().await == SessionCommand::Start {
                break;
            }
        }

        event_channel.clear();

        //
        // Feed the interface with initial events.
        if let Some(value) = displayled_receiver.try_get() {
            event_channel.send(Event::Led(value)).await;
        }
        if let Some(value) = fanduty_receiver.try_get() {
            event_channel.send(Event::FanDuty(value)).await;
        }
        if let Some(value) = fantachy_receiver.try_get() {
            event_channel.send(Event::FanTachy(value)).await;
        }
        if let Some(value) = netstatus_receiver.try_get() {
            event_channel.send(Event::Net(value)).await;
        }
        if let Some(value) = powerrelay_receiver.try_get() {
            event_channel.send(Event::Relay(value)).await;
        }
        if let Some(value) = tempsensor_receiver.try_get() {
            event_channel.send(Event::Temperature(value)).await;
        }
        if let Some(value) = displayboard_receiver.try_get() {
            event_channel.send(Event::DisplayBoard(value)).await;
        }
        let value = collect_logs(memlog, LOG_LINE_COUNT);
        event_channel.send(Event::LogsSnapshot(value)).await;

        //
        // Start listening for updates.
        let mut timeout_fut = Timer::after(SESSION_TIMEOUT);
        'session: loop {
            let led_fut = displayled_receiver.changed();
            let duty_fut = fanduty_receiver.changed();
            let tachy_fut = fantachy_receiver.changed();
            let net_fut = netstatus_receiver.changed();
            let relay_fut = powerrelay_receiver.changed();
            let temp_fut = tempsensor_receiver.changed();
            let log_fut = logwatch_receiver.changed();
            let control_fut = control_signal.wait();

            embassy_infinite_futures::generate_select!(9);
            let event = match select9(
                led_fut,
                duty_fut,
                tachy_fut,
                net_fut,
                relay_fut,
                temp_fut,
                log_fut,
                &mut timeout_fut,
                control_fut,
            )
            .await
            {
                Either9::Future1(led_state) => Event::Led(led_state),
                Either9::Future2(fan_duty) => Event::FanDuty(fan_duty),
                Either9::Future3(fan_tachy) => Event::FanTachy(fan_tachy),
                Either9::Future4(net_status) => Event::Net(net_status),
                Either9::Future5(relay_state) => Event::Relay(relay_state),
                Either9::Future6(temperature) => Event::Temperature(temperature),
                Either9::Future7(_record) => {
                    let logs = collect_logs(memlog, LOG_LINE_COUNT);
                    Event::LogsSnapshot(logs)
                }

                Either9::Future8(_timeout) => {
                    // This event must arrive.
                    event_channel.send(Event::TimedOut).await;
                    break 'session;
                }

                Either9::Future9(command) => match command {
                    SessionCommand::Stop => break 'session,
                    SessionCommand::Start => unreachable!(),
                },
            };

            event_channel.try_send(event).ok();
        } // 'session loop
    }
}

pub fn init() -> (&'static SessionControlSignal, &'static EventChannel) {
    let control_signal = Box::leak(Box::new(SessionControlSignal::new()));
    let event_channel = Box::leak(Box::new(EventChannel::new()));
    (control_signal, event_channel)
}

/// Triggers actions controlled by output pins.
#[embassy_executor::task]
pub async fn run(
    peripheral_uart: uart::AnyUart<'static>,
    pin_uart_rx: gpio::AnyPin<'static>,
    pin_uart_tx: gpio::AnyPin<'static>,
    pincontrol_publisher: PinControlPublisher,
    fanduty_sender: FanDutyDynSender,
    powerrelay_sender: PowerRelayDynSender,
    memlog: SharedLogger,
    control_signal: &'static SessionControlSignal,
    event_channel: &'static EventChannel,
) {
    let uart = uart::Uart::new(
        peripheral_uart,
        uart::Config::default().with_baudrate(UART_BAUD_RATE),
    )
    .unwrap()
    .with_tx(pin_uart_tx)
    .with_rx(pin_uart_rx);

    let (uart_rx, uart_tx) = uart.split();
    let mut uart_rx = uart_rx.into_async();
    let mut uart_tx = uart_tx;

    //
    //

    let tui_config = ratatui_serial::Config {
        terminal_size: (80, 24).into(),
        rx_buffer_len: 64,
        eof_behavior: ratatui_serial::EofBehavior::Retry,
    };

    loop {
        //
        // Present user with a launch button app.
        {
            let mut launch_runner = ratatui_serial::Runner::with_config(
                app::LaunchApp,
                &mut uart_rx,
                &mut uart_tx,
                tui_config,
            )
            .unwrap();
            if let Err(error) = launch_runner.run().await {
                memlog.warn(format!("uart: launch runner: {error}"));
                Timer::after(Duration::from_secs(1)).await;
                continue;
            }
        }

        //
        // User confirms launch.
        // Switch to control panel app.
        // Start listening for events.

        event_channel.clear();
        control_signal.signal(SessionCommand::Start);

        let panel_app = app::ControlPanelApp::new(
            &pincontrol_publisher,
            fanduty_sender.clone(),
            powerrelay_sender,
        );
        let panel_events = event_channel.receiver();
        {
            // A runner owns the terminal buffers, so it must be dropped before
            // we construct the next screen or the heap footprint briefly doubles.
            let mut panel_runner = ratatui_serial::Runner::with_config(
                panel_app,
                &mut uart_rx,
                &mut uart_tx,
                tui_config,
            )
            .unwrap()
            .with_event_source(panel_events);

            if let Err(error) = panel_runner.run().await {
                memlog.warn(format!("uart: panel runner: {error}"));
                Timer::after(Duration::from_secs(1)).await;
            }
        }

        //
        // Control panel exited.
        // Stop listening for events.
        // Return to the launch button app.

        control_signal.signal(SessionCommand::Stop);
        event_channel.clear();
    }
}

//
// Ratatui application models.
//

mod app {
    use super::*;
    use alloc::{
        collections::vec_deque::VecDeque,
        string::{String, ToString},
    };
    use ratatui_core::{
        layout::{Constraint, Direction, Layout, Rect},
        terminal::Frame,
        text::Line,
    };
    use ratatui_serial::{Action, InputEvent, TerminalApp};
    use ratatui_widgets::{block::Block, list::List, paragraph::Paragraph, sparkline::Sparkline};

    //
    // Launch screen application.
    //
    pub struct LaunchApp;

    impl TerminalApp for LaunchApp {
        fn render(&mut self, frame: &mut Frame<'_>) {
            let launch_area = frame
                .area()
                .centered(Constraint::Length(18), Constraint::Length(5));
            let block = Block::bordered();
            let inner = block.inner(launch_area);
            frame.render_widget(block, launch_area);
            frame.render_widget(Paragraph::new(Line::from("[ launch ]").centered()), inner);
        }

        fn on_input(&mut self, event: InputEvent) -> Action {
            match event {
                InputEvent::CtrlL => Action::RedrawFull,
                InputEvent::Enter => Action::Exit,
                _ => Action::RedrawNone,
            }
        }
    }

    //
    // Control panel application.
    //

    pub struct ControlPanelApp<'a> {
        focus: Focus,
        selected_button: usize,
        led_state: Option<LedState>,
        fan_duty: Option<u8>,
        fan_tachy: Option<u16>,
        fan_input: String,
        fan_input_dirty: bool,
        net_status: Option<NetworkStatus>,
        relay_state: Option<PowerRelay>,
        temperature: Option<TemperatureReading>,
        temp_history: VecDeque<u64>,
        logs: Vec<Record>,
        status: String,
        pincontrol_publisher: &'a PinControlPublisher,
        fanduty_sender: FanDutyDynSender,
        powerrelay_sender: PowerRelayDynSender,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Focus {
        Buttons,
        RelayToggle,
        FanInput,
        FanSend,
    }

    impl Focus {
        fn next(self) -> Self {
            match self {
                Self::Buttons => Self::RelayToggle,
                Self::RelayToggle => Self::FanInput,
                Self::FanInput => Self::FanSend,
                Self::FanSend => Self::Buttons,
            }
        }
    }

    impl<'a> ControlPanelApp<'a> {
        pub fn new(
            pincontrol_publisher: &'a PinControlPublisher,
            fanduty_sender: FanDutyDynSender,
            powerrelay_sender: PowerRelayDynSender,
        ) -> Self {
            Self {
                focus: Focus::Buttons,
                selected_button: 0,
                led_state: None,
                fan_duty: None,
                fan_tachy: None,
                fan_input: String::new(),
                fan_input_dirty: false,
                net_status: None,
                relay_state: None,
                temperature: None,
                temp_history: VecDeque::with_capacity(TEMP_HISTORY_LEN),
                logs: Vec::new(),
                status: String::new(),
                pincontrol_publisher,
                fanduty_sender,
                powerrelay_sender,
            }
        }

        fn push_temperature_sample(&mut self, reading: TemperatureReading) {
            self.temperature = Some(reading);

            if let Ok(temp_c) = reading.temperature {
                if self.temp_history.len() == TEMP_HISTORY_LEN {
                    let _ = self.temp_history.pop_front();
                }
                self.temp_history
                    .push_back((temp_c.clamp(0.0, 100.0) + 0.5) as u64);
            }
        }

        fn set_live_fan_duty(&mut self, fan_duty: Option<u8>) {
            self.fan_duty = fan_duty;

            if !self.fan_input_dirty {
                self.fan_input = fan_duty.map(|value| value.to_string()).unwrap_or_default();
            }
        }

        fn move_button_selection(&mut self, dx: i8, dy: i8) {
            let row = self.selected_button / 2;
            let col = self.selected_button % 2;
            let new_row = (row as i8 + dy).clamp(0, 2) as usize;
            let new_col = (col as i8 + dx).clamp(0, 1) as usize;
            let mut next = new_row * 2 + new_col;

            if next >= BUTTONS.len() {
                next = new_row * 2;
            }
            if next < BUTTONS.len() {
                self.selected_button = next;
            }
        }

        fn activate_selected_button(&mut self) {
            let button = &BUTTONS[self.selected_button];
            if self
                .pincontrol_publisher
                .try_publish(button.message)
                .is_ok()
            {
                self.status = format!("btn {}", button.label);
            } else {
                self.status = String::from("btn queue full");
            }
        }

        fn send_fan_input(&mut self) {
            match parse_fan_input(&self.fan_input) {
                Some(value) => {
                    self.fanduty_sender.send(value);
                    self.fan_input = value.to_string();
                    self.fan_input_dirty = false;
                    self.fan_duty = Some(value);
                    self.status = format!("fan {}%", value);
                }
                None => self.status = String::from("bad duty"),
            }
        }

        fn toggle_relay(&mut self) {
            let command = match self.relay_state {
                Some(PowerRelay::Open) => PowerRelayCommand::Close,
                Some(PowerRelay::Closed) => PowerRelayCommand::Open,
                Some(PowerRelay::ForcedOpen) => {
                    self.status = String::from("relay latched");
                    return;
                }
                None => {
                    self.status = String::from("relay unknown");
                    return;
                }
            };

            match self.powerrelay_sender.try_send(command) {
                Ok(()) => {
                    self.status = match command {
                        PowerRelayCommand::Close => String::from("relay on"),
                        PowerRelayCommand::Open => String::from("relay off"),
                        PowerRelayCommand::ForceOpenLatch => unreachable!(),
                    };
                }
                Err(_) => self.status = String::from("relay queue full"),
            }
        }

        fn handle_event(&mut self, event: Event) -> Action {
            match event {
                Event::Led(led_state) => self.led_state = Some(led_state),
                Event::FanDuty(fan_duty) => self.set_live_fan_duty(Some(fan_duty)),
                Event::FanTachy(fan_tachy) => self.fan_tachy = Some(fan_tachy),
                Event::Net(net_status) => self.net_status = Some(net_status),
                Event::Relay(relay_state) => self.relay_state = Some(relay_state),
                Event::Temperature(temperature) => self.push_temperature_sample(temperature),
                Event::DisplayBoard(board_state) => (), // TODO
                Event::LogsSnapshot(logs) => self.logs = logs,
                Event::TimedOut => return Action::Exit,
            }

            Action::RedrawChanged
        }

        fn handle_input(&mut self, event: InputEvent) -> Action {
            match event {
                InputEvent::CtrlL => Action::RedrawFull,

                InputEvent::Tab => {
                    self.focus = self.focus.next();
                    Action::RedrawChanged
                }

                InputEvent::Left if self.focus == Focus::Buttons => {
                    self.move_button_selection(-1, 0);
                    Action::RedrawChanged
                }
                InputEvent::Right if self.focus == Focus::Buttons => {
                    self.move_button_selection(1, 0);
                    Action::RedrawChanged
                }
                InputEvent::Up if self.focus == Focus::Buttons => {
                    self.move_button_selection(0, -1);
                    Action::RedrawChanged
                }
                InputEvent::Down if self.focus == Focus::Buttons => {
                    self.move_button_selection(0, 1);
                    Action::RedrawChanged
                }

                InputEvent::Enter | InputEvent::Char(b' ') if self.focus == Focus::Buttons => {
                    self.activate_selected_button();
                    Action::RedrawChanged
                }

                InputEvent::Enter | InputEvent::Char(b' ') if self.focus == Focus::RelayToggle => {
                    self.toggle_relay();
                    Action::RedrawChanged
                }

                InputEvent::Backspace if self.focus == Focus::FanInput => {
                    let _ = self.fan_input.pop();
                    self.fan_input_dirty = true;
                    Action::RedrawChanged
                }

                InputEvent::Char(byte)
                    if self.focus == Focus::FanInput && byte.is_ascii_digit() =>
                {
                    if self.fan_input.len() < MAX_FAN_INPUT_LEN {
                        self.fan_input.push(byte as char);
                        self.fan_input_dirty = true;
                        Action::RedrawChanged
                    } else {
                        Action::RedrawNone
                    }
                }

                InputEvent::Enter | InputEvent::Char(b' ') if self.focus == Focus::FanSend => {
                    self.send_fan_input();
                    Action::RedrawChanged
                }

                _ => Action::RedrawNone,
            }
        }

        fn render_frame(&self, frame: &mut Frame<'_>) {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(PANEL_HEIGHT), Constraint::Min(0)])
                .split(frame.area());

            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(BUTTON_PANEL_WIDTH), Constraint::Min(0)])
                .split(layout[0]);

            self.render_buttons(frame, top[0]);
            self.render_status(frame, top[1]);
            self.render_logs(frame, layout[1]);
        }

        fn render_buttons(&self, frame: &mut Frame<'_>, area: Rect) {
            let block = Block::bordered();
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .split(inner);

            for row in 0..3 {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(rows[row]);

                for col in 0..2 {
                    let index = row * 2 + col;
                    let label = if index < BUTTONS.len() {
                        format_button_label(
                            BUTTONS[index].label,
                            index == self.selected_button,
                            self.focus == Focus::Buttons,
                        )
                    } else {
                        String::new()
                    };

                    frame.render_widget(Paragraph::new(Line::from(label).centered()), cols[col]);
                }
            }
        }

        fn render_status(&self, frame: &mut Frame<'_>, area: Rect) {
            let block = Block::bordered();
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .split(inner);

            frame.render_widget(Paragraph::new(self.led_text()), rows[0]);
            frame.render_widget(Paragraph::new(self.network_text()), rows[1]);
            frame.render_widget(Paragraph::new(self.relay_text()), rows[2]);
            frame.render_widget(Paragraph::new(self.temperature_text()), rows[3]);

            if self.temp_history.is_empty() {
                frame.render_widget(Paragraph::new(""), rows[4]);
            } else {
                let spark_data = self.temp_history.iter().copied().collect::<Vec<_>>();
                frame.render_widget(Sparkline::default().data(spark_data).max(100), rows[4]);
            }

            frame.render_widget(Paragraph::new(self.fan_live_text()), rows[5]);
            frame.render_widget(Paragraph::new(self.fan_editor_text()), rows[6]);
            frame.render_widget(Paragraph::new(self.status.as_str()), rows[7]);
        }

        fn render_logs(&self, frame: &mut Frame<'_>, area: Rect) {
            let block = Block::bordered();
            let inner = block.inner(area);
            let max_width = inner.width as usize;
            let lines = self
                .logs
                .iter()
                .map(|record| truncate(&format!("{record}"), max_width))
                .collect::<Vec<_>>();

            frame.render_widget(List::new(lines).block(block), area);
        }

        fn led_text(&self) -> String {
            match self.led_state {
                Some(led_state) => format!(
                    "led g{} r{}",
                    bool_mark(led_state.green),
                    bool_mark(led_state.red)
                ),
                None => String::from("led g? r?"),
            }
        }

        fn network_text(&self) -> String {
            let net_text = match self.net_status.as_ref() {
                Some(net_status) if !net_status.link_up => String::from("down"),
                Some(net_status) => match net_status
                    .ip_config
                    .as_ref()
                    .map(|config| config.address.address())
                {
                    Some(address) => format!("up {address}"),
                    None => String::from("up no-ip"),
                },
                None => String::from("--"),
            };

            format!("net {net_text}")
        }

        fn relay_text(&self) -> String {
            let state_text = match self.relay_state {
                Some(PowerRelay::Open) => "off",
                Some(PowerRelay::Closed) => "on",
                Some(PowerRelay::ForcedOpen) => "latched",
                None => "--",
            };

            let button_text = match self.relay_state {
                Some(PowerRelay::Open) => {
                    format_focus_button("on", self.focus == Focus::RelayToggle)
                }
                Some(PowerRelay::Closed) => {
                    format_focus_button("off", self.focus == Focus::RelayToggle)
                }
                Some(PowerRelay::ForcedOpen) => String::from("[locked]"),
                None => String::from("[wait]"),
            };

            format!("relay {state_text} {button_text}")
        }

        fn temperature_text(&self) -> String {
            match self.temperature {
                Some(reading) => match reading.temperature {
                    Ok(temp_c) if reading.retries > 0 => {
                        format!("temp {:>4.1}c r{}", temp_c, reading.retries)
                    }
                    Ok(temp_c) => format!("temp {:>4.1}c", temp_c),
                    Err(_) => format!("temp err r{}", reading.retries),
                },
                None => String::from("temp --.-c"),
            }
        }

        fn fan_live_text(&self) -> String {
            let duty_text = self
                .fan_duty
                .map(|fan_duty| format!("{fan_duty:03}%"))
                .unwrap_or_else(|| String::from("---%"));
            let tachy_text = self
                .fan_tachy
                .map(|fan_tachy| format!("{fan_tachy:4}rpm"))
                .unwrap_or_else(|| String::from("----rpm"));

            format!("fan {duty_text} {tachy_text}")
        }

        fn fan_editor_text(&self) -> String {
            let input = if self.fan_input.is_empty() {
                String::from("---")
            } else {
                self.fan_input.clone()
            };

            let input = if self.focus == Focus::FanInput {
                format!("<{input}>")
            } else {
                format!("[{input}]")
            };
            let send = if self.focus == Focus::FanSend {
                "<send>"
            } else {
                "[send]"
            };

            format!("set {input} {send}")
        }
    }

    impl TerminalApp<Event> for ControlPanelApp<'_> {
        fn render(&mut self, frame: &mut Frame<'_>) {
            self.render_frame(frame);
        }

        fn on_input(&mut self, event: InputEvent) -> Action {
            self.handle_input(event)
        }

        fn on_event(&mut self, event: Event) -> Action {
            self.handle_event(event)
        }
    }

    impl TerminalApp for ControlPanelApp<'_> {
        fn render(&mut self, frame: &mut Frame<'_>) {
            self.render_frame(frame);
        }

        fn on_input(&mut self, event: InputEvent) -> Action {
            self.handle_input(event)
        }
    }

    fn parse_fan_input(input: &str) -> Option<u8> {
        let value = input.parse::<u8>().ok()?;
        (value <= 100).then_some(value)
    }

    fn bool_mark(value: bool) -> char {
        if value { '+' } else { '-' }
    }

    fn format_button_label(label: &str, selected: bool, focused: bool) -> String {
        match (selected, focused) {
            (true, true) => format!("[>{label}<]"),
            (true, false) => format!("[ {label} ]"),
            (false, _) => format!("  {label}  "),
        }
    }

    fn format_focus_button(label: &str, focused: bool) -> String {
        if focused {
            format!("<{label}>")
        } else {
            format!("[{label}]")
        }
    }

    fn truncate(text: &str, max_width: usize) -> String {
        text.chars().take(max_width).collect()
    }
}
