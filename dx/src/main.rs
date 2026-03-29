use communication::communication;
use dioxus::prelude::*;
use gilrs;
use postcard::{experimental::max_size::MaxSize, from_bytes, to_slice};
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex}
};
use tokio::{
    self,
    net::UdpSocket,
    time::{self, Duration, Instant, Sleep},
};

const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[tokio::main]
async fn main() {
    let controller_1p = Arc::new(Mutex::new(communication::ControllerState::new()));
    let controller_2p = Arc::new(Mutex::new(communication::ControllerState::new()));

    // コントローラーの入力を受け取る
    {
        let controller_1p = controller_1p.clone();
        let controller_2p = controller_2p.clone();

        tokio::spawn(async move {
            let mut g = gilrs::Gilrs::new().unwrap();
            while let Some(e) = g.next_event_blocking(None) {
                let id: usize = e.id.into();
                let mut controller = {
                    if id == 0 {
                        controller_1p.lock().unwrap()
                    } else if id == 1 {
                        controller_2p.lock().unwrap()
                    } else {
                        continue;
                    }
                };

                match e.event {
                    gilrs::EventType::ButtonPressed(b, _) => {
                        if b == gilrs::ev::Button::East {
                            controller.shot = true;
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
        let addr = format!("{}:{}", env!("SERVER_IP"), env!("SERVER_PORT"));

        let socket = Arc::new(UdpSocket::bind(addr).await.unwrap());
        dbg!(&socket);

        const BUFFER_SIZE: usize = communication::FromServerData::POSTCARD_MAX_SIZE;

        let mut rx_buf = [0; BUFFER_SIZE];

        let mut handler_1p: Option<RobotHandler> = None;
        let mut handler_2p: Option<RobotHandler> = None;

        let mut ticker = time::interval(Duration::from_millis(
            env!("INTERVAL").parse::<u64>().unwrap(),
        ));

        loop {
            tokio::select! {
                Ok((_, addr)) = socket.recv_from(&mut rx_buf) => {
                    let mut message: communication::RobotRespond = from_bytes(&mut rx_buf).unwrap();
                    /*
                    id = 0 => 振り分け
                    id = 0 でも addrを知っている => addrを再通知
                    id = 1 or 2 続行
                    */
                    if message.id == 0 {
                        if let Some(ref h) = handler_1p && h.addr == addr {
                            println!("OK");
                            h.notify_id().await;
                            message.id = 1;
                        } else if let Some(ref h) = handler_2p && h.addr == addr {
                            h.notify_id().await;
                            message.id = 2;
                        } else if handler_1p.is_none() {
                            println!("OK");
                            let h = RobotHandler::new(1, addr, socket.clone(), controller_1p.clone());
                            h.notify_id().await;
                            handler_1p = Some(RobotHandler::new(1, addr, socket.clone(), controller_1p.clone()));
                            message.id = 1;
                        } else if handler_2p.is_none() {
                            let h = RobotHandler::new(2, addr, socket.clone(), controller_1p.clone());
                            h.notify_id().await;
                            handler_2p = Some(RobotHandler::new(2, addr, socket.clone(), controller_1p.clone()));
                            message.id = 2;
                        } else {
                            println!("id: 0だけど何もできませんでした");
                            continue;
                        }
                    }
                    if let Some(ref mut h) = handler_1p && message.id == 1 {
                        h.recv_heatbeat();
                    }
                    if let Some(ref mut h) = handler_2p && message.id == 2 {
                        h.recv_heatbeat();
                    }
                }
                _ = ticker.tick() => {
                    if let Some(ref h) = handler_1p {
                        h.send_controller_data().await;
                    }
                    if let Some(ref h) = handler_2p {
                        h.send_controller_data().await;
                    }
                }
                _ = async {
                    if let Some(h) = handler_1p.as_mut() {
                        h.heartbeat_timer.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    println!("CLOSE");
                    handler_1p = None;
                }
                _ = async {
                    if let Some(h) = handler_2p.as_mut() {
                        h.heartbeat_timer.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    handler_2p = None;
                }
            }
        }
    });

    dioxus::launch(App);
}

struct RobotHandler {
    id: u8,
    addr: SocketAddr,
    socket: Arc<UdpSocket>,
    controller: Arc<Mutex<communication::ControllerState>>,
    heartbeat_timeout: Duration,
    heartbeat_timer: Pin<Box<Sleep>>,
}

impl RobotHandler {
    fn new(
        id: u8,
        addr: SocketAddr,
        socket: Arc<UdpSocket>,
        controller: Arc<Mutex<communication::ControllerState>>,
    ) -> Self {
        let heartbeat_timeout = Duration::from_millis(env!("INTERVAL").parse::<u64>().unwrap() * 10);
        let timer = Box::pin(time::sleep_until(Instant::now() + heartbeat_timeout));
        Self {
            id,
            addr,
            socket,
            controller,
            heartbeat_timer: timer,
            heartbeat_timeout,
        }
    }

    async fn notify_id(&self) {
        let mut buf = [0; communication::FromServerData::POSTCARD_MAX_SIZE];

        self.socket
            .send_to(
                to_slice(&communication::FromServerData::SetID(self.id), &mut buf).unwrap(),
                self.addr,
            )
            .await
            .unwrap();
    }

    fn recv_heatbeat(&mut self) {
        self.heartbeat_timer = Box::pin(time::sleep_until(Instant::now() + self.heartbeat_timeout));
    }

    async fn send_controller_data(&self) {
        let mut buf = [0; communication::FromServerData::POSTCARD_MAX_SIZE];
        let data = {
            let controller = self.controller.lock().unwrap();
            to_slice(&communication::FromServerData::Controller(*controller), &mut buf).unwrap()
        };
        self.socket.send_to(data, self.addr).await.unwrap();
    }
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
    }
}
