use crate::{
    memlog::SharedLogger,
    task::{
        display_state::DisplayStateDynReceiver,
        fan_control::{FanDutyDynReceiver, FanTachyDynReceiver},
        net_monitor::NetStatusDynReceiver,
        pin_control::{PinControlMessage, PinControlPublisher, PinControlSubscriber},
        temp_sensor::TempSensorDynReceiver,
    },
};
use alloc::{format, string::ToString};
use const_format::concatcp;
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
const MQTT_TOPIC_ROOT: &str = "devices/display";
use crate::config::MQTT_CLIENT_ID;
use crate::config::MQTT_TOPIC_DEVICE_NAME;

macro_rules! mqtt_topic {
    ($TAIL:expr) => {
        concatcp!(MQTT_TOPIC_ROOT, '/', MQTT_TOPIC_DEVICE_NAME, '/', $TAIL)
    };
}

/// Topics to subscribe to when connected.
const SUBSCRIBE_TOPICS: &[&str] = &[mqtt_topic!("control/set")];

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
        mqtt_topic!("status"),
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
    mut displayboard_receiver: DisplayStateDynReceiver,
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
            memlog.warn(format!("mqtt: failed to connect to broker: {error:?}"));
            continue 'connect;
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
                    mqtt_topic!("status"),
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
                memlog.warn(format!("mqtt: failed to initialize: {error}"));
                Timer::after_secs(10).await;
                continue 'connect;
            }
        };

        // Subscribe to topics.
        memlog.info("mqtt: subscribing to topics");
        for sub_topic in SUBSCRIBE_TOPICS {
            if mqtt_client
                .subscribe(sub_topic, QualityOfService::Qos1)
                .await
                .is_err()
            {
                // Something went wrong, retry the connection.
                Timer::after_secs(10).await;
                continue 'connect;
            }
        }

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
                    let dspl_fut = displayboard_receiver.changed();

                    embassy_infinite_futures::generate_select!(9);
                    match select9(
                        temp_fut,
                        fanduty_fut,
                        fantachy_fut,
                        pincontrol_fut,
                        net_fut,
                        log_fut,
                        dspl_fut,
                        &mut ping_fut,
                        &mut poll_fut,
                    )
                    .await
                    {
                        // Publish temperature sensor readings.
                        Either9::Future1(sensor_data) => {
                            if let Ok(temp) = sensor_data.temperature {
                                mqtt_client
                                    .publish(
                                        mqtt_topic!("temp"),
                                        temp.to_string().as_bytes(),
                                        QualityOfService::Qos0,
                                        false,
                                    )
                                    .await?;
                            }
                        }

                        // Publish fan duty values.
                        Either9::Future2(duty) => {
                            mqtt_client
                                .publish(
                                    mqtt_topic!("fan/duty"),
                                    duty.to_string().as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish fan tachy readings.
                        Either9::Future3(rpms) => {
                            mqtt_client
                                .publish(
                                    mqtt_topic!("fan/tachy"),
                                    rpms.to_string().as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish pincontrol commands.
                        Either9::Future4(pincontrol) => {
                            if let WaitResult::Message(command) = pincontrol {
                                let command =
                                    serde_json_core::to_string::<_, 128>(&command).unwrap();
                                mqtt_client
                                    .publish(
                                        mqtt_topic!("control"),
                                        command.as_bytes(),
                                        QualityOfService::Qos0,
                                        false,
                                    )
                                    .await?;
                            }
                        }

                        // Publish network status updates.
                        Either9::Future5(net) => {
                            mqtt_client
                                .publish(
                                    mqtt_topic!("net"),
                                    format!("{net:?}").as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish logs.
                        Either9::Future6(log) => {
                            mqtt_client
                                .publish(
                                    mqtt_topic!("log"),
                                    format!("{log}").as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Publish changes to the display board state.
                        Either9::Future7(state) => {
                            mqtt_client
                                .publish(
                                    mqtt_topic!("state"),
                                    format!("{state:?}").as_bytes(),
                                    QualityOfService::Qos0,
                                    false,
                                )
                                .await?;
                        }

                        // Periodically send a ping to the server.
                        Either9::Future8(_ping) => {
                            mqtt_client.send_ping().await?;
                            ping_fut = Timer::after(MQTT_PING_INTERVAL);
                        }

                        // Periodic poll for MQTT messages.
                        Either9::Future9(_trigger) => {
                            mqtt_client.poll(false).await?;
                            poll_fut = Timer::after_secs(1);
                        }
                    }
                } // 'select loop
            }
            .await; // async catch

            match catch {
                Err(ClientError::Disconnected(reason)) => {
                    memlog.info(format!("mqtt: client disconnected: {reason}"));
                    continue 'connect;
                }
                Err(error) => {
                    memlog.info(format!("mqtt: client error: {error}"));
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
        if message.topic_name.eq(mqtt_topic!("control/set")) {
            match serde_json_core::from_slice::<PinControlMessage>(message.payload) {
                Ok((command, _remainder)) => self.pincontrol_publisher.publish(command).await,
                Err(error) => self
                    .memlog
                    .warn(format!("failed to deserialize pin command: {error}")),
            }

            Ok(())
        } else {
            // Unrecognized topics.
            // Note: we deliberately do not error on an unexpected topic.
            self.memlog
                .warn(format!("unexpected topic: {}", message.topic_name));

            Ok(())
        }
    }
}

#[allow(dead_code)]
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
