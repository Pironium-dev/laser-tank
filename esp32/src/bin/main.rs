#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use communication::{FromServerData, RobotMethod, RobotRespond};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{
    self, Runner, Stack,
    udp::{PacketMetadata, UdpMetadata, UdpSocket},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embassy_time::{Duration, Ticker, with_timeout, Timer};
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    mcpwm::{McPwm, PeripheralClockConfig, operator::PwmPinConfig, timer::PwmWorkingMode},
    peripherals::MCPWM0,
    rmt::{PulseCode, Rmt, RxChannelConfig, RxChannelCreator, TxChannelConfig, TxChannelCreator},
    rng,
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_println::{dbg, println};
use esp_radio::wifi::{ClientConfig, ModeConfig};
use esp32::motor::Motor;
use postcard::{experimental::max_size::MaxSize, from_bytes, to_slice};
use smoltcp::socket::icmp::Socket;
use static_cell::StaticCell;

#[panic_handler]
fn panic(p: &core::panic::PanicInfo) -> ! {
    dbg!(p);
    loop {}
}

extern crate alloc;
use crate::alloc::string::ToString;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");
const SERVER_IP: &str = env!("SERVER_IP");
const SERVER_PORT: &str = env!("SERVER_PORT");
const INTERVAL: &str = env!("INTERVAL");

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.2.0

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    static RADIO_INIT: StaticCell<esp_radio::Controller> = StaticCell::new();

    let (mut wifi_controller, interfaces) = esp_radio::wifi::new(
        RADIO_INIT.init(esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller")),
        peripherals.WIFI,
        Default::default(),
    )
    .expect("Failed to initialize Wi-Fi controller");

    // ここから開始

    // socket通信の設定

    let client_config = ClientConfig::default()
        .with_ssid(SSID.to_string())
        .with_password(PASSWORD.to_string());

    let config = ModeConfig::Client(client_config);

    wifi_controller.set_config(&config).unwrap();

    let rng = rng::Rng::new();

    let random_seed = rng.random() as u64 | (rng.random() as u64) << 32;

    static RESOURCES: StaticCell<embassy_net::StackResources<4>> = StaticCell::new();

    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        embassy_net::Config::dhcpv4(embassy_net::DhcpConfig::default()),
        RESOURCES.init(embassy_net::StackResources::new()),
        random_seed,
    );

    wifi_controller.start().unwrap();

    let _ = spawner.spawn(start_wifi(runner));

    wifi_controller.connect().unwrap();

    println!("Preparing WIFI");

    stack.wait_config_up().await;

    println!("WIFI is UP");

    println!("{:?}", stack.config_v4());

    // モーターの設定

    let clock_cfg = PeripheralClockConfig::with_frequency(Rate::from_mhz(20)).unwrap();

    let mut mcpwm = McPwm::new(peripherals.MCPWM0, clock_cfg);
    let timer_clock_cfg = clock_cfg
        .timer_clock_with_frequency(99, PwmWorkingMode::Increase, Rate::from_khz(10))
        .unwrap();
    mcpwm.timer0.start(timer_clock_cfg);

    let mut motor_right = Motor::new(
        mcpwm
            .operator0
            .with_pin_a(peripherals.GPIO33, PwmPinConfig::UP_ACTIVE_HIGH),
        Output::new(peripherals.GPIO32, Level::Low, OutputConfig::default()),
    );

    let mut motor_left = Motor::new(
        mcpwm
            .operator1
            .with_pin_a(peripherals.GPIO26, PwmPinConfig::UP_ACTIVE_HIGH),
        Output::new(peripherals.GPIO25, Level::Low, OutputConfig::default()),
    );

    // timer等の設定

    let interval = INTERVAL.parse().unwrap();

    let mut timeout = Ticker::every(Duration::from_millis(interval * 10));

    // rmtの設定

    let freq = Rate::from_mhz(80);
    let rmt = Rmt::new(peripherals.RMT, freq).unwrap().into_async();

    let tx_config = TxChannelConfig::default()
        .with_clk_divider(80)
        .with_carrier_modulation(true)
        .with_carrier_high(13)
        .with_carrier_low(13)
        .with_carrier_level(Level::High)
        .with_idle_output(true)
        .with_idle_output_level(Level::Low);

    let rx_config = RxChannelConfig::default()
        .with_clk_divider(80)
        .with_idle_threshold(10000)
        .with_filter_threshold(10);

    let mut tx_data = [PulseCode::new(Level::High, 50, Level::Low, 50); 3];
    tx_data[2] = PulseCode::end_marker();

    let mut rx_data = [PulseCode::end_marker(); 5];

    let mut tx_channel = rmt
        .channel0
        .configure_tx(peripherals.GPIO13, tx_config)
        .unwrap();
    let mut rx_channel = rmt
        .channel1
        .configure_rx(peripherals.GPIO23, rx_config)
        .unwrap();

    // その他色々

    let mut shot_id: u8 = 0;

    let mut buf = [0 as u8; RobotRespond::POSTCARD_MAX_SIZE];

    /*
    task1 socketのコントロール、udp送信
    task2 udp送信、rmt受信、つぎに送信する内容の一時記録
    task3 udp受信、機体操作
    */

    static UDP_RECV: Channel<CriticalSectionRawMutex, FromServerData, 3> = Channel::new();
    static UDP_SEND: Channel<CriticalSectionRawMutex, RobotMethod, 3> = Channel::new();

    spawner
        .spawn(udp_handler(stack, UDP_SEND.receiver(), UDP_RECV.sender()))
        .unwrap();

    spawner
        .spawn(drive(
            motor_right,
            motor_left,
            interval,
            UDP_RECV.receiver(),
        ))
        .unwrap();

    spawner.spawn(monitor(UDP_SEND.sender(), interval)).unwrap();

    // loop {
    //     match select4(
    //         socket.recv_from_with(|s, _| from_bytes::<FromServerData>(s)),
    //         heartbeat.next(),
    //         timeout.next(),
    //         rx_channel.receive(&mut rx_data),
    //     )
    //     .await
    //     {
    //         Either4::First(d) => {
    //             let data = d.unwrap();
    //             println!("OK 1");
    //             timeout.reset();
    //             match data {
    //                 FromServerData::SetID(id) => {
    //                     robot_id = id;
    //                 }
    //                 FromServerData::Controller(c) => {
    //                     motor_right.set_velocity(c.right_stick);
    //                     motor_left.set_velocity(c.left_stick);
    //                     if shot_id.wrapping_add(1) == c.shot {
    //                         shot_id = c.shot;
    //                         println!("SHOT: {}", shot_id);
    //                         tx_channel.transmit(&tx_data).await.unwrap();
    //                         println!("FIN");
    //                     }
    //                 }
    //             }
    //         }
    //         Either4::Second(_) => {
    //             println!("OK 2");
    //             let data = to_slice(
    //                 &RobotRespond {
    //                     id: robot_id,
    //                     method: RobotMethod::HeartBeat,
    //                 },
    //                 &mut buf,
    //             )
    //             .unwrap();
    //             socket.send_to(data, server_endpoint).await.unwrap();
    //         }
    //         Either4::Third(_) => {
    //             println!("OK 3");
    //             timeout.reset();
    //             motor_right.set_velocity(0.0);
    //             motor_left.set_velocity(0.0);
    //         }
    //         Either4::Fourth(result) => {
    //             result.unwrap();
    //             dbg!(rx_data); // printの代わり
    //         }
    //     }
    // }

    loop {
        Timer::after_secs(3600).await;
    }
}

#[embassy_executor::task]
async fn start_wifi(mut runner: Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn drive(
    mut motor_right: Motor<'static, MCPWM0<'static>, 0>,
    mut motor_left: Motor<'static, MCPWM0<'static>, 1>,
    interval: u64,
    recv: Receiver<'static, CriticalSectionRawMutex, FromServerData, 3>,
) {
    let interval = Duration::from_millis(interval * 10);
    loop {
        match with_timeout(interval, recv.receive()).await {
            Ok(result) => match result {
                FromServerData::Controller(c) => {
                    motor_left.set_velocity(c.left_stick);
                    motor_right.set_velocity(c.right_stick);
                }
                FromServerData::SetID(id) => {
                    println!("ID: {id}");
                }
            },
            Err(_) => {
                motor_right.set_velocity(0.0);
                motor_left.set_velocity(0.0);
            }
        }
    }
}

#[embassy_executor::task]
async fn udp_handler(
    stack: Stack<'static>,
    send_rx: Receiver<'static, CriticalSectionRawMutex, RobotMethod, 3>,
    recv_tx: Sender<'static, CriticalSectionRawMutex, FromServerData, 3>,
) {
    let mut ip_address = [0; 4];
    for (i, s) in SERVER_IP.split(".").enumerate() {
        ip_address[i] = s.parse().unwrap();
    }

    let ip_address =
        embassy_net::IpAddress::v4(ip_address[0], ip_address[1], ip_address[2], ip_address[3]);
    let port = SERVER_PORT.parse().unwrap();

    let mut rx_buffer = [0 as u8; 1024];
    let mut tx_buffer = [0 as u8; 1024];

    let mut rx_meta = [PacketMetadata::EMPTY; 3];
    let mut tx_meta = [PacketMetadata::EMPTY; 3];

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    let server_endpoint = UdpMetadata::from((ip_address, port));

    let my_ipaddress = stack.config_v4().unwrap().address.address();

    socket.bind((my_ipaddress, port)).unwrap();

    let mut rx_buffer = [0; FromServerData::POSTCARD_MAX_SIZE];
    let mut tx_buffer = [0; RobotRespond::POSTCARD_MAX_SIZE];

    let mut robot_id: u8 = 0;

    loop {
        match select(socket.recv_from(&mut rx_buffer), send_rx.receive()).await {
            Either::First(x) => match x {
                Ok(x) => {
                    let data = from_bytes::<FromServerData>(&rx_buffer[..x.0]).unwrap();
                    if let FromServerData::SetID(id) = data {
                        robot_id = id;
                    }
                    println!("OK");
                    recv_tx.send(data).await;
                }
                Err(_) => {}
            },
            Either::Second(x) => {
                let data = to_slice(
                    &RobotRespond {
                        id: robot_id,
                        method: x,
                    },
                    &mut tx_buffer,
                )
                .unwrap();
                socket.send_to(data, server_endpoint).await.unwrap();
            }
        }
    }
}

#[embassy_executor::task]
async fn monitor(send: Sender<'static, CriticalSectionRawMutex, RobotMethod, 3>, interval: u64) {
    let mut heartbeat = Ticker::every(Duration::from_millis(interval));
    loop {
        send.send(RobotMethod::HeartBeat).await;
        heartbeat.next().await;
    }
}
