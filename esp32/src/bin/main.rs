#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use communication::communication::{ControllerState, FromServerData, RobotRespond};
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_net::{self, Runner, tcp};
use embassy_time::{Duration, Ticker, with_timeout};
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
use futures::{FutureExt, Stream, task::Poll};
use postcard::{experimental::max_size::MaxSize, from_bytes_cobs, to_slice_cobs};
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

    static RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();

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

    let mut rx_buffer = [0 as u8; 1024];
    let mut tx_buffer = [0 as u8; 1024];

    let mut socket = tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);

    let mut ip_address = [0; 4];
    for (i, s) in SERVER_IP.split(".").enumerate() {
        ip_address[i] = s.parse().unwrap();
    }

    let ip_address =
        embassy_net::IpAddress::v4(ip_address[0], ip_address[1], ip_address[2], ip_address[3]);
    let port = SERVER_PORT.parse().unwrap();

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
            .with_pin_a(peripherals.GPIO27, PwmPinConfig::UP_ACTIVE_HIGH),
        Output::new(peripherals.GPIO26, Level::Low, OutputConfig::default()),
    );

    let mut motor_left = Motor::new(
        mcpwm
            .operator1
            .with_pin_a(peripherals.GPIO33, PwmPinConfig::UP_ACTIVE_HIGH),
        Output::new(peripherals.GPIO32, Level::Low, OutputConfig::default()),
    );

    let interval = INTERVAL.parse().unwrap();

    let mut timeout_ticker = Ticker::every(Duration::from_millis(interval * 3));
    let mut heartbeat_ticker = Ticker::every(Duration::from_millis(interval));

    let mut robot_id = 0;

    const DATA_MAX_SIZE: usize = FromServerData::POSTCARD_MAX_SIZE + 2;

    let mut tx_buf = [0 as u8; RobotRespond::POSTCARD_MAX_SIZE + 2];

    loop {
        // バラバラになったパケットを再結合したい
        let mut data_head = 0;
        let mut data = [0 as u8; DATA_MAX_SIZE];

        socket
            .connect(embassy_net::IpEndpoint::new(ip_address, port))
            .await
            .unwrap();

        let (rx, mut tx) = socket.split();

        tx.write(to_slice_cobs(&RobotRespond::SendID(robot_id), &mut tx_buf).unwrap())
            .await;

        heartbeat_ticker.reset();

        loop {
            let mut buf = [0 as u8; DATA_MAX_SIZE];

            match select3(
                socket.read(&mut buf),
                heartbeat_ticker.next(),
                timeout_ticker.next(),
            )
            .await
            {
                Either3::First(socket_data) => {
                    // サーバーからの通信
                    match socket_data {
                        Ok(bites) => {
                            if bites == 0 {
                                println!("No Connection");
                                continue;
                            }
                            for i in buf {
                                if i == 0 && data_head == 0 {
                                    continue;
                                }
                                data[data_head] = i;
                                data_head += 1;
                                if i == 0 {
                                    println!("{:?}", data);
                                    data_head = 0;
                                    timeout_ticker.reset();
                                    let data: FromServerData = from_bytes_cobs(&mut data).unwrap();
                                    match data {
                                        FromServerData::Controller(state) => {
                                            motor_right.set_velocity(state.right_stick);
                                            motor_left.set_velocity(state.left_stick);
                                        }
                                        FromServerData::SetID(id) => {
                                            robot_id = id;
                                            println!("{}", id);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            println!("{:?}", e);
                        }
                    }
                }
                Either3::Second(_) => {
                    println!("HeartBeat");
                    socket
                        .write(to_slice_cobs(&RobotRespond::HeartBeat, &mut tx_buf).unwrap())
                        .await;
                }
                Either3::Third(_) => {
                    println!("TIMEOUT");
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn start_wifi(mut runner: Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}
