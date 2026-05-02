use gilrs;
use postcard::{experimental::max_size::MaxSize, from_bytes, to_slice};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    self,
    net::UdpSocket,
    sync::mpsc::{Receiver, Sender, channel},
    time::{self, Duration, Instant},
};

const SERVER_IP: &str = env!("SERVER_IP");
const RECEIVE_PORT: &str = env!("RECEIVE_PORT");
const INTERVAL: &str = env!("INTERVAL");

#[derive(Debug)]
pub enum ToServer {
    Connect,
    Disconnect,
    Hit,
    AskShot,
}

#[derive(Debug)]
pub enum ToRobot {
    AllowShot,
    Stop,
    Start,
}

pub fn init() -> (Sender<(u8, ToRobot)>, Receiver<(u8, ToServer)>) {
    let (to_robot_tx, to_robot_rx) = channel::<(u8, ToRobot)>(8);
    let (to_server_tx, to_server_rx) = channel::<(u8, ToServer)>(8);

    let controllers = [
        Arc::new(Mutex::new(communication::ControllerState::default())),
        Arc::new(Mutex::new(communication::ControllerState::default())),
    ];

    // コントローラーの入力を受け取る
    {
        let controllers = controllers.clone();
        let tx = to_server_tx.clone();
        tokio::spawn(async move {
            controller_handler(controllers, tx).await;
        });
    }

    tokio::spawn(async move {
        robot_handler(controllers, to_server_tx, to_robot_rx).await;
    });

    (to_robot_tx, to_server_rx)
}

async fn controller_handler(
    controllers: [Arc<Mutex<communication::ControllerState>>; 2],
    to_robot_tx: Sender<(u8, ToServer)>,
) {
    let mut g = gilrs::Gilrs::new().unwrap();
    while let Some(e) = g.next_event_blocking(None) {
        let id: usize = e.id.into();
        if id >= 2 {
            continue;
        }

        match e.event {
            gilrs::EventType::ButtonPressed(b, _) => {
                if b == gilrs::ev::Button::East {
                    to_robot_tx
                        .send((id as u8 + 1, ToServer::AskShot))
                        .await
                        .unwrap();
                }
            }
            gilrs::EventType::AxisChanged(axis, x, _) => {
                let mut controller = controllers[id].lock().unwrap();
                match axis {
                    gilrs::Axis::LeftStickX => controller.stick.0 = x as i8,
                    gilrs::Axis::LeftStickY => controller.stick.1 = x as i8,
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

async fn robot_handler(
    controllers: [Arc<Mutex<communication::ControllerState>>; 2],
    to_server_tx: Sender<(u8, ToServer)>,
    mut to_robot_rx: Receiver<(u8, ToRobot)>,
) {
    let addr = format!("{}:{}", SERVER_IP, RECEIVE_PORT);

    let socket = Arc::new(UdpSocket::bind(addr).await.unwrap());

    const BUFFER_SIZE: usize = communication::ServerData::POSTCARD_MAX_SIZE;

    let mut rx_buf = [0; BUFFER_SIZE];

    let mut stop_flag = false;

    let mut handlers: [Option<RobotHandler>; 2] = [None, None];

    let mut ticker = time::interval(Duration::from_millis(INTERVAL.parse::<u64>().unwrap()));

    loop {
        tokio::select! {
            Ok((_, addr)) = socket.recv_from(&mut rx_buf) => {
                let mut message: communication::RobotRespond = from_bytes(&mut rx_buf).unwrap();
                /*
                id = 0 => 振り分け
                id = 0 でも addrを知っている => addrを再通知
                id = 1 or 2 続行
                */
                if message.robot_id == 0 {
                    if let Some(idx) = handlers.iter().position(|h| h.as_ref().is_some_and(|h| h.recv_addr == addr)) {
                        println!("OK (Reconnecting)");
                        let h = handlers[idx].as_ref().unwrap();
                        h.notify_id().await;
                        message.robot_id = (idx + 1) as u8;
                        to_server_tx.send((message.robot_id, ToServer::Connect)).await.unwrap();
                        continue;
                    } else if let Some(idx) = handlers.iter().position(|h| h.is_none()) {
                        println!("OK (New Connection)");
                        let h = RobotHandler::new((idx + 1) as u8, addr, socket.clone(), controllers[idx].clone());
                        h.notify_id().await;
                        handlers[idx] = Some(h);
                        message.robot_id = (idx + 1) as u8;
                        {
                            let mut controller = controllers[idx].lock().unwrap();
                            controller.shot_id = 0;
                        }
                        to_server_tx.send((message.robot_id, ToServer::Connect)).await.unwrap();
                        continue;
                    } else {
                        println!("id: 0だけど何もできませんでした");
                        continue;
                    }
                }

                if message.robot_id >= 1 && message.robot_id <= 2 {
                    let idx = (message.robot_id - 1) as usize;
                    if let Some(ref mut h) = handlers[idx] {
                        if h.recv_heartbeat(&message) {
                            println!("HIT! robot_id: {}", message.robot_id);
                            to_server_tx.send((message.robot_id, ToServer::Hit)).await.unwrap();
                        }
                        continue;
                    } else {
                        let h = RobotHandler::new((idx + 1) as u8, addr, socket.clone(), controllers[idx].clone());
                        h.notify_id().await;
                        handlers[idx] = Some(h);
                        {
                            let mut controller = controllers[idx].lock().unwrap();
                            controller.shot_id = 0;
                        }
                        to_server_tx.send((message.robot_id, ToServer::Connect)).await.unwrap();
                        continue;
                    }
                }

                println!("何もしてません: {:?}", message);
            }
            _ = ticker.tick() => {
                let now = Instant::now();
                for (i, h_opt) in handlers.iter_mut().enumerate() {
                    if let Some(h) = h_opt {
                        if now >= h.heartbeat_deadline {
                            println!("CLOSE {}", i + 1);
                            to_server_tx.send((h.robot_id, ToServer::Disconnect)).await.unwrap();
                            *h_opt = None;
                        } else {
                            h.send_controller_data(stop_flag).await;
                        }
                    }
                }
            }
            Some(x) = to_robot_rx.recv() => {
                println!("{:?}", x);
                match x.1 {
                    ToRobot::AllowShot => {
                        let mut controller = controllers[x.0 as usize - 1].lock().unwrap();
                        controller.shot_id = controller.shot_id.wrapping_add(1);
                        println!("SEND");
                    }
                    ToRobot::Stop => {
                        stop_flag = true;
                    }
                    ToRobot::Start => {
                        stop_flag = false;
                    }
                }
            }
        }
    }
}

struct RobotHandler {
    robot_id: u8,
    hit_id: u8,
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
            robot_id: id,
            hit_id: 0,
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
                to_slice(&communication::ServerData::SetID(self.robot_id), &mut buf).unwrap(),
                self.send_addr,
            )
            .await
            .unwrap();
    }

    fn recv_heartbeat(&mut self, data: &communication::RobotRespond) -> bool {
        self.heartbeat_deadline = Instant::now() + self.heartbeat_timeout;
        communication::detect_id_change(&mut self.hit_id, data.hit_id)
    }

    async fn send_controller_data(&self, stop_flag: bool) {
        let mut buf = [0; communication::ServerData::POSTCARD_MAX_SIZE];
        let data = {
            let mut controller = self.controller.lock().unwrap().clone();
            if stop_flag {
                controller.stick = (0, 0);
            }
            to_slice(
                &communication::ServerData::Controller(controller),
                &mut buf,
            )
            .unwrap()
        };
        self.socket.send_to(data, self.send_addr).await.unwrap();
    }
}
