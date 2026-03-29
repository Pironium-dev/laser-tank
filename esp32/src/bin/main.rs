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
use embassy_futures::select::{Either3, select3};
use embassy_net::{
    self, Runner,
    udp::{PacketMetadata, UdpMetadata, UdpSocket},
};
use embassy_time::{Duration, Ticker};
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    mcpwm::{McPwm, PeripheralClockConfig, operator::PwmPinConfig, timer::PwmWorkingMode},
    rng,
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_println::{dbg, println};
use esp_radio::wifi::{ClientConfig, ModeConfig};
use esp32::motor::Motor;
use postcard::{experimental::max_size::MaxSize, from_bytes, to_slice};
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

    let _ = spawner.spawn(start_wifi(runner));

    wifi_controller.start().unwrap();
    wifi_controller.connect().unwrap();

    println!("Preparing WIFI");

    stack.wait_config_up().await;

    println!("WIFI is UP");

    println!("{:?}", stack.config_v4());

    let mut ip_address = [0; 4];
    for (i, s) in SERVER_IP.split(".").enumerate() {
        ip_address[i] = s.parse().unwrap();
    }

    let ip_address =
        embassy_net::IpAddress::v4(ip_address[0], ip_address[1], ip_address[2], ip_address[3]);
    let port = SERVER_PORT.parse().unwrap();

    let mut rx_buffer = [0 as u8; 1024];
    let mut tx_buffer = [0 as u8; 1024];

    let mut rx_meta = [PacketMetadata::EMPTY];
    let mut tx_meta = [PacketMetadata::EMPTY];

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    let server_endpoint = UdpMetadata::from((ip_address, port));

    let my_ipaddress = stack.config_v4().unwrap().address.address();

    socket.bind((my_ipaddress, 0)).unwrap();

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

    let mut heartbeat = Ticker::every(Duration::from_millis(interval));
    let mut timeout = Ticker::every(Duration::from_millis(interval * 10));

    // その他色々

    let mut robot_id: u8 = 0;

    let mut buf = [0 as u8; RobotRespond::POSTCARD_MAX_SIZE];

    loop {
        match select3(
            socket.recv_from_with(|s, _| from_bytes::<FromServerData>(s)),
            heartbeat.next(),
            timeout.next(),
        )
        .await
        {
            Either3::First(d) => {
                let data = d.unwrap();
                timeout.reset();
                match data {
                    FromServerData::SetID(id) => {
                        robot_id = id;
                    }
                    FromServerData::Controller(c) => {
                        motor_right.set_velocity(c.right_stick);
                        motor_left.set_velocity(c.left_stick);
                    }
                }
            }
            Either3::Second(_) => {
                let data = to_slice(
                    &RobotRespond {
                        id: robot_id,
                        method: RobotMethod::HeartBeat,
                    },
                    &mut buf,
                )
                .unwrap();
                socket.send_to(data, server_endpoint).await.unwrap();
                println!("SEND");
            }
            Either3::Third(_) => {
                motor_right.set_velocity(0.0);
                motor_left.set_velocity(0.0);
            }
        }
    }


}

#[embassy_executor::task]
async fn start_wifi(mut runner: Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}
