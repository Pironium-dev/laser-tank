use dioxus::prelude::*;
use gilrs;
use postcard::{experimental::max_size::MaxSize, from_bytes, to_slice};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex}
};
use tokio::{
    self,
    net::UdpSocket,
    time::{self, Duration, Instant},
};

const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const SERVER_IP: &str = env!("SERVER_IP");
const RECEIVE_PORT: &str = env!("RECEIVE_PORT");
const INTERVAL: &str = env!("INTERVAL");

#[tokio::main]
async fn main() {
    let controllers = [
        Arc::new(Mutex::new(communication::ControllerState::default())),
        Arc::new(Mutex::new(communication::ControllerState::default())),
    ];

    // コントローラーの入力を受け取る
    {
        let controllers = controllers.clone();

        tokio::spawn(async move {
            let mut g = gilrs::Gilrs::new().unwrap();
            while let Some(e) = g.next_event_blocking(None) {
                let id: usize = e.id.into();
                if id >= 2 {
                    continue;
                }
                let mut controller = controllers[id].lock().unwrap();

                match e.event {
                    gilrs::EventType::ButtonPressed(b, _) => {
                        if b == gilrs::ev::Button::East {
                            controller.shot = controller.shot.wrapping_add(1);
                            println!("{}", controller.shot);
                        }
                    }
                    gilrs::EventType::AxisChanged(axis, x, _) => match axis {
                        gilrs::Axis::LeftStickY => controller.left_stick = x,
                        gilrs::Axis::RightStickY => controller.right_stick = x,
                        _ => {}
                    },
                    _ => {}
                }
            }
        });
    }

    tokio::spawn(async move {
        let addr = format!("{}:{}", SERVER_IP, RECEIVE_PORT);

        let socket = Arc::new(UdpSocket::bind(addr).await.unwrap());
        dbg!(&socket);

        const BUFFER_SIZE: usize = communication::ServerData::POSTCARD_MAX_SIZE;

        let mut rx_buf = [0; BUFFER_SIZE];

        let mut handlers: [Option<RobotHandler>; 2] = [None, None];

        let mut ticker = time::interval(Duration::from_millis(
            INTERVAL.parse::<u64>().unwrap(),
        ));

        loop {
            tokio::select! {
                Ok((_, addr)) = socket.recv_from(&mut rx_buf) => {
                    let mut message: communication::RobotRespond = from_bytes(&mut rx_buf).unwrap();
                    println!("{:?}", &message);
                    /*
                    id = 0 => 振り分け
                    id = 0 でも addrを知っている => addrを再通知
                    id = 1 or 2 続行
                    */
                    if message.id == 0 {
                        if let Some(idx) = handlers.iter().position(|h| h.as_ref().is_some_and(|h| h.recv_addr == addr)) {
                            println!("OK (Reconnecting)");
                            let h = handlers[idx].as_ref().unwrap();
                            h.notify_id().await;
                            message.id = (idx + 1) as u8;
                            continue;
                        } else if let Some(idx) = handlers.iter().position(|h| h.is_none()) {
                            println!("OK (New Connection)");
                            let h = RobotHandler::new((idx + 1) as u8, addr, socket.clone(), controllers[idx].clone());
                            h.notify_id().await;
                            handlers[idx] = Some(h);
                            message.id = (idx + 1) as u8;
                            let mut controller = controllers[idx].lock().unwrap();
                            controller.shot = 0;
                            continue;
                        } else {
                            println!("id: 0だけど何もできませんでした");
                            continue;
                        }
                    }
                    
                    if message.id >= 1 && message.id <= 2 {
                        let idx = (message.id - 1) as usize;
                        if let Some(ref mut h) = handlers[idx] {
                            h.recv_heatbeat();
                            continue
                        }
                    }

                    println!("何もしてません: {:?}", message);
                }
                _ = ticker.tick() => {
                    let now = Instant::now();
                    for (_i, h_opt) in handlers.iter_mut().enumerate() {
                        if let Some(h) = h_opt {
                            if now >= h.heartbeat_deadline {
                                //println!("CLOSE {}", i + 1);
                                //*h_opt = None;
                            } else {
                                println!("SEND");
                                h.send_controller_data().await;
                            }
                        }
                    }
                }
            }
        }
    });

    dioxus::launch(App);
}

struct RobotHandler {
    id: u8,
    recv_addr: SocketAddr,
    send_addr: SocketAddr,
    socket: Arc<UdpSocket>,
    controller: Arc<Mutex<communication::ControllerState>>,
    heartbeat_timeout: Duration,
    heartbeat_deadline: Instant,
}

impl RobotHandler {
    fn new(
        id: u8,
        addr: SocketAddr,
        socket: Arc<UdpSocket>,
        controller: Arc<Mutex<communication::ControllerState>>,
    ) -> Self {
        let heartbeat_timeout = Duration::from_millis(INTERVAL.parse::<u64>().unwrap() * 10);

        let send_addr = SocketAddr::new(addr.ip(), RECEIVE_PORT.parse().unwrap());
        
        Self {
            id,
            recv_addr: addr,
            send_addr,
            socket: socket,
            controller,
            heartbeat_deadline: Instant::now() + heartbeat_timeout,
            heartbeat_timeout,
        }
    }

    async fn notify_id(&self) {
        let mut buf = [0; communication::ServerData::POSTCARD_MAX_SIZE];

        self.socket
            .send_to(
                to_slice(&communication::ServerData::SetID(self.id), &mut buf).unwrap(),
                self.send_addr,
            )
            .await
            .unwrap();
    }

    fn recv_heatbeat(&mut self) {
        self.heartbeat_deadline = Instant::now() + self.heartbeat_timeout;
    }

    async fn send_controller_data(&self) {
        let mut buf = [0; communication::ServerData::POSTCARD_MAX_SIZE];
        let data = {
            let controller = self.controller.lock().unwrap();
            to_slice(&communication::ServerData::Controller(*controller), &mut buf).unwrap()
        };
        self.socket.send_to(data, self.send_addr).await.unwrap();
    }
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
    }
}
