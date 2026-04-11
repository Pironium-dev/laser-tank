#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use communication::{RobotMethod, RobotRespond, ServerData};
use embassy_executor::Spawner;
use embassy_futures::{
    select::{Either, select},
    yield_now,
};
use embassy_net::{
    self, Runner, Stack,
    udp::{PacketMetadata, UdpMetadata, UdpSocket},
};
use embassy_sync::once_lock::OnceLock;
use embassy_time::{Duration, Ticker, Timer, with_timeout};
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
use esp_radio::wifi::{ClientConfig, ModeConfig, PowerSaveMode, WifiController};
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
const RECEIVE_PORT: &str = env!("RECEIVE_PORT");
const SEND_PORT: &str = env!("SEND_PORT");
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
    wifi_controller
        .set_power_saving(PowerSaveMode::None)
        .unwrap();

    let rng = rng::Rng::new();

    let random_seed = rng.random() as u64 | (rng.random() as u64) << 32;

    static RESOURCES: StaticCell<embassy_net::StackResources<8>> = StaticCell::new();

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
        .timer_clock_with_frequency(99, PwmWorkingMode::Increase, Rate::from_khz(5))
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

    static ID: OnceLock<u8> = OnceLock::new();

    /*
    task1 socketのコントロール、udp送信
    task2 udp送信、rmt受信、つぎに送信する内容の一時記録
    task3 udp受信、機体操作
    */

    spawner
        .spawn(drive(stack, motor_right, motor_left, interval, &ID))
        .unwrap();

    spawner
        .spawn(monitor(stack, wifi_controller, interval, &ID))
        .unwrap();

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
    stack: Stack<'static>,
    mut motor_right: Motor<'static, MCPWM0<'static>, 0>,
    mut motor_left: Motor<'static, MCPWM0<'static>, 1>,
    interval: u64,
    id: &'static OnceLock<u8>,
) {
    let interval = Duration::from_millis(interval * 10);

    let mut rx_buffer = [0 as u8; 1024 * 2];
    let mut tx_buffer = [];

    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_meta = [];

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    socket.bind(RECEIVE_PORT.parse::<u16>().unwrap()).unwrap();

    let mut buf = [0; ServerData::POSTCARD_MAX_SIZE];

    let mut left_velocity = 0.0f32;
    let mut right_velocity = 0.0f32;

    loop {
        yield_now().await;
        match with_timeout(interval, socket.recv_from(&mut buf)).await {
            Ok(result) => match result {
                Ok((x, _)) => match from_bytes(&buf[..x]).unwrap() {
                    ServerData::Controller(c) => {
                        motor_left.set_velocity(ease_motor(&mut left_velocity, c.left_stick));
                        motor_right.set_velocity(ease_motor(&mut right_velocity, c.right_stick));
                    }
                    ServerData::SetID(new_id) => {
                        println!("ID: {new_id}");
                        let _ = id.init(new_id);
                    }
                },
                Err(_x) => {}
            },

            Err(_) => {
                motor_right.set_velocity(0.0);
                motor_left.set_velocity(0.0);
            }
        }
    }
}

#[embassy_executor::task]
async fn monitor(
    stack: Stack<'static>,
    mut wifi_controller: WifiController<'static>,
    interval: u64,
    id: &'static OnceLock<u8>,
) {
    let mut ip_address = [0; 4];
    for (i, s) in SERVER_IP.split(".").enumerate() {
        ip_address[i] = s.parse().unwrap();
    }

    let ip_address =
        embassy_net::IpAddress::v4(ip_address[0], ip_address[1], ip_address[2], ip_address[3]);

    let endpoint = (ip_address, RECEIVE_PORT.parse::<u16>().unwrap());

    loop {
        println!("MAKE");
        let mut tx_buffer = [0 as u8; 20];
        let mut rx_buffer = [];

        let mut tx_meta = [PacketMetadata::EMPTY; 3];
        let mut rx_meta = [];

        let mut socket = UdpSocket::new(
            stack,
            &mut rx_meta,
            &mut rx_buffer,
            &mut tx_meta,
            &mut tx_buffer,
        );

        socket.bind(SEND_PORT.parse::<u16>().unwrap()).unwrap();

        let mut buf = [0; RobotRespond::POSTCARD_MAX_SIZE];

        let mut heartbeat = Ticker::every(Duration::from_millis(interval));
        loop {
            yield_now().await;
            let respond = {
                RobotRespond {
                    id: { if id.is_set() { *(id.get().await) } else { 0 } },
                    method: RobotMethod::HeartBeat,
                }
            };

            if socket.may_send() {
                socket
                    .send_to(to_slice(&respond, &mut buf).unwrap(), endpoint)
                    .await
                    .unwrap();
            } else {
                println!("UDP may_send() == false; transmit buffer full or socket not ready");
                dbg!(stack.is_link_up());
                dbg!(stack.is_config_up());
                wifi_controller.stop_async().await.unwrap();
                println!("WiFi controller stopped");
                wifi_controller.start_async().await.unwrap();
                println!("WiFi controller started");
                wifi_controller.connect_async().await.unwrap();
                break;
            }
            heartbeat.next().await;
        }
    }
}

fn ease_motor(past: &mut f32, current: f32) -> f32 {
    let delta = (current - *past) * 0.8;
    *past += delta;
    *past
}
