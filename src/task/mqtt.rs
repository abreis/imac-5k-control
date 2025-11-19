use crate::{
    memlog::SharedLogger,
    task::{
        fan_control::{FanDutyDynReceiver, FanTachyDynReceiver},
        net_monitor::NetStatusDynReceiver,
        pin_control::{PinControlMessage, PinControlPublisher, PinControlSubscriber},
        temp_sensor::TempSensorDynReceiver,
    },
};
use alloc::{format, string::ToString};
use const_format::concatcp;
use core::fmt::Display;
use embassy_net::{IpEndpoint, dns::DnsQueryType, tcp::TcpSocket};
use embassy_sync::pubsub::WaitResult;
use embassy_time::{Duration, Timer};
use mountain_mqtt::{
    client::{
        Client, ClientError, ClientNoQueue, ClientReceivedEvent, ConnectionSettings, EventHandler,
        EventHandlerError,
    },
    data::{
        property::{Property, PublishProperty},
        quality_of_service::QualityOfService,
        string_pair::StringPair,
    },
    embedded_io_async::ConnectionEmbedded,
    packets::connect::Will,
};

const MQTT_PING_INTERVAL: Duration = Duration::from_secs(20);
const MQTT_SERVER_ADDR: &str = "broker.abu";
const MQTT_PORT: u16 = 1883;
const MQTT_TIMEOUT_MS: u32 = 5000;
const MQTT_PROPERTIES: usize = 16;
const MQTT_DISPLAY_TOPIC_ROOT: &str = "devices/display";
use crate::config::MQTT_CLIENT_ID;
use crate::config::MQTT_TOPIC_DEVICE_NAME;

macro_rules! topic_display {
    ($TAIL:expr) => {
        concatcp!(
            MQTT_DISPLAY_TOPIC_ROOT,
            '/',
            MQTT_TOPIC_DEVICE_NAME,
            '/',
            $TAIL
        )
    };
}

//
// Inter-task communication.
//

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum MqttStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}
impl Display for MqttStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MqttStatus::Disconnected => write!(f, "disconnected"),
            MqttStatus::Connecting => write!(f, "connecting"),
            MqttStatus::Connected => write!(f, "connected"),
        }
    }
}

//
// Broker connection.
//

struct MqttDelay;
impl mountain_mqtt::client::Delay for MqttDelay {
    async fn delay_us(&mut self, us: u32) {
        Timer::after_micros(us as u64).await
    }
}

type MqttClient<'a> = ClientNoQueue<
    'a,
    ConnectionEmbedded<TcpSocket<'a>>,
    MqttDelay,
    MqttHandler<'a>,
    MQTT_PROPERTIES,
>;

async fn connect_to_broker<'a>(
    socket: TcpSocket<'a>,
    mqtt_buffer: &'a mut [u8],
    delay: MqttDelay,
    event_handler: MqttHandler<'a>,
) -> Result<MqttClient<'a>, ClientError> {
    // Create an MQTT client.
    let mqtt_conn = ConnectionEmbedded::new(socket);

    let mut mqtt_client = ClientNoQueue::new(
        mqtt_conn,
        mqtt_buffer,
        delay,
        MQTT_TIMEOUT_MS,
        event_handler,
    );

    // // PayloadFormatIndicator '0' -> unspecified byte stream
    // // PayloadFormatIndicator '1' -> UTF-8 encoded payload
    // let mut will_properties: heapless::Vec<_, 1> = heapless::Vec::new();
    // will_properties
    //     .push(WillProperty::PayloadFormatIndicator(
    //         PayloadFormatIndicator::new(1),
    //     ))
    //     .unwrap();

    // Set up a LWT marking the client as offline if it is disconnected.
    let will = Will::new(
        QualityOfService::Qos1,
        true,
        topic_display!("status"),
        "offline".as_bytes(),
        heapless::Vec::<_, 0>::new(),
    );

    // Open the MQTT connection.
    mqtt_client
        .connect_with_will(
            &ConnectionSettings::unauthenticated(MQTT_CLIENT_ID),
            Some(will),
        )
        .await?;

    Ok(mqtt_client)
}

#[embassy_executor::task]
pub async fn run(
    stack: embassy_net::Stack<'static>,
    mut fanduty_receiver: FanDutyDynReceiver,
    mut fantachy_receiver: FanTachyDynReceiver,
    pincontrol_publisher: PinControlPublisher,
    mut pincontrol_subscriber: PinControlSubscriber,
    mut netstatus_receiver: NetStatusDynReceiver,
    mut tempsensor_receiver: TempSensorDynReceiver,
    memlog: SharedLogger,
) {
    let broker_addr = 'dns: loop {
        match stack.dns_query(MQTT_SERVER_ADDR, DnsQueryType::A).await {
            Ok(mut dns_result) => match dns_result.pop() {
                Some(addr) => break 'dns addr,
                None => memlog.warn("mqtt: empty dns response"),
            },
            Err(_) => memlog.warn("mqtt: failed to resolve broker address"),
        };

        // Retry DNS request every 10 seconds.
        Timer::after_secs(10).await;
    };

    // Enable log watching and get a receiver.
    memlog.enable_watch();
    let mut logwatch_receiver = memlog.watch().unwrap();

    //
    // Connect loop.
    //

    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 1024];
    let mut mqtt_buffer = [0u8; 2048];

    // We continue this loop if the mqtt client is disconnected or failed to connect.
    'connect: loop {
        // Open a TCP connection to the broker.
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        if let Err(error) = socket
            .connect(IpEndpoint::new(broker_addr, MQTT_PORT))
            .await
        {
            memlog.warn(format!("tcp socket failed to connect to broker: {error:?}"));
        }

        let catch: Result<_, ClientError> = async {
            let delay = MqttDelay;
            let event_handler = MqttHandler {
                pincontrol_publisher: &pincontrol_publisher,
                memlog,
            };
            let mut mqtt_client =
                connect_to_broker(socket, &mut mqtt_buffer, delay, event_handler).await?;

            // Publish an 'online' status.
            mqtt_client
                .publish(
                    topic_display!("status"),
                    "online".as_bytes(),
                    QualityOfService::Qos1,
                    true,
                )
                .await?;

            Ok(mqtt_client)
        }
        .await;

        let mut mqtt_client = match catch {
            Ok(client) => client,
            Err(error) => {
                // Something went wrong, pause and retry the connection steps.
                memlog.warn(format!("failed to initialize mqtt: {error}"));
                Timer::after_secs(10).await;
                continue 'connect;
            }
        };

        // Connected.
        memlog.info("mqtt: connected");

        //
        // Main loop.
        //

        // We continue this loop if the mqtt client throws an error but did not disconnect.
        'main: loop {
            let catch: Result<(), ClientError> = async {
                let mut ping_fut = Timer::after(MQTT_PING_INTERVAL);
                // Poor API design of mountain-mqtt forces us to poll periodically.
                let mut poll_fut = Timer::after_secs(1);

                '_select: loop {
                    let temp_fut = tempsensor_receiver.changed();
                    let fanduty_fut = fanduty_receiver.changed();
                    let fantachy_fut = fantachy_receiver.changed();
                    let pincontrol_fut = pincontrol_subscriber.next_message();
                    let net_fut = netstatus_receiver.changed();
                    let log_fut = logwatch_receiver.changed();

                    embassy_infinite_futures::generate_select!(8);
                    match select8(
                        temp_fut,
                        fanduty_fut,
                        fantachy_fut,
                        pincontrol_fut,
                        net_fut,
                        log_fut,
                        &mut ping_fut,
                        &mut poll_fut,
                    )
                    .await
                    {
                        // Publish temperature sensor readings.
                        Either8::Future1(sensor_data) => {
                            if let Ok(temp) = sensor_data.temperature {
                                mqtt_client
                                    .publish(
                                        topic_display!("temp"),
                                        temp.to_string().as_bytes(),
                                        QualityOfService::Qos0,
                                        false,
                                    )
                                    .await?;
                            }
                        }

                        // Publish fan duty values.
                        Either8::Future2(duty) => {
                            mqtt_client
                                .publish(
                                    topic_display!("fan/duty"),
                                    duty.to_string().as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish fan tachy readings.
                        Either8::Future3(rpms) => {
                            mqtt_client
                                .publish(
                                    topic_display!("fan/tachy"),
                                    rpms.to_string().as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish pincontrol commands.
                        Either8::Future4(pincontrol) => {
                            if let WaitResult::Message(command) = pincontrol {
                                let command =
                                    serde_json_core::to_string::<_, 128>(&command).unwrap();
                                mqtt_client
                                    .publish(
                                        topic_display!("control"),
                                        command.as_bytes(),
                                        QualityOfService::Qos0,
                                        false,
                                    )
                                    .await?;
                            }
                        }

                        // Publish network status updates.
                        Either8::Future5(net) => {
                            mqtt_client
                                .publish(
                                    topic_display!("net"),
                                    format!("{net:?}").as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish logs.
                        Either8::Future6(log) => {
                            mqtt_client
                                .publish(
                                    topic_display!("log"),
                                    format!("{log}").as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Periodically send a ping to the server.
                        Either8::Future7(_ping) => {
                            mqtt_client.send_ping().await?;
                            ping_fut = Timer::after_secs(10);
                        }

                        // Periodic poll for MQTT messages.
                        Either8::Future8(_trigger) => {
                            mqtt_client.poll(false).await?;
                            poll_fut = Timer::after_secs(1);
                        }
                    }
                } // 'select loop
            }
            .await; // async catch

            match catch {
                Err(ClientError::Disconnected(reason)) => {
                    memlog.info(format!("mqtt client disconnected: {reason}"));
                    continue 'connect;
                }
                Err(error) => {
                    memlog.info(format!("mqtt client error: {error}"));
                    continue 'main;
                }
                Ok(()) => (),
            }
        } // 'main loop
    } // 'connect loop
}

struct MqttHandler<'h> {
    pincontrol_publisher: &'h PinControlPublisher,
    memlog: SharedLogger,
}

impl<'h, const P: usize> EventHandler<P> for MqttHandler<'h> {
    async fn handle_event(
        &mut self,
        event: ClientReceivedEvent<'_, P>,
    ) -> Result<(), EventHandlerError> {
        let ClientReceivedEvent::ApplicationMessage(message) = event else {
            return Ok(());
        };

        // Receive pincontrol commands on devices/display/<id>/control/set
        if message.topic_name.eq(topic_display!("control/set")) {
            match serde_json_core::from_slice::<PinControlMessage>(message.payload) {
                Ok((command, _remainder)) => self.pincontrol_publisher.publish(command).await,
                Err(error) => self
                    .memlog
                    .warn(format!("failed to deserialize pin command: {error}")),
            }
        }

        // Unrecognized topics.
        self.memlog
            .warn(format!("unexpected topic: {}", message.topic_name));

        // Note: we deliberately do not error on an unexpected topic.

        Ok(())
    }
}

fn find_user_property<'p, const N: usize>(
    properties: &heapless::Vec<PublishProperty<'p>, N>,
    name: &str,
    value: Option<&str>,
) -> Option<StringPair<'p>> {
    properties.iter().find_map(|property| {
        let PublishProperty::UserProperty(user_property) = property else {
            return None;
        };

        if user_property.value().name() != name {
            return None;
        }

        if let Some(value) = value
            && value != user_property.value().value()
        {
            return None;
        }

        Some(user_property.value())
    })
}
