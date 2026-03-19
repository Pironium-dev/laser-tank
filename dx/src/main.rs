use async_stream::stream;
use communication::communication;
use dioxus::prelude::*;
use futures_core::Stream;
use futures_util::StreamExt;
use gilrs;
use postcard::{from_bytes_cobs, to_slice_cobs};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::{self, io::AsyncWriteExt, net, time};

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
        let listener =
            net::TcpListener::bind(format!("{}:{}", env!("SERVER_IP"), env!("SERVER_PORT")))
                .await
                .unwrap();

        const BUFFER_SIZE: usize =
            communication::calc_buffer_lengh::<communication::FromServerData>();

        let mut buf_1p = [0; BUFFER_SIZE];
        let mut buf_2p = [0; BUFFER_SIZE];

        let mut robot_1p: Option<tokio::task::JoinHandle<()>> = None;
        let mut robot_2p: Option<tokio::task::JoinHandle<()>> = None;

        loop {
            tokio::select! {
                // 接続を待ち、robot_*pに分ける
                Ok(i) = listener.accept() => {
                    println!("{:?}", &i);
                    let (rx, mut tx) = i.0.into_split();

                    let mut fut = Box::<_>::pin(cobs_reader(rx).await);

                    match fut.next().await {
                        None => {continue;}
                        Some(x) => {
                            match x {
                                communication::RobotRespond::SendID(id) =>{
                                    let mut flag_1p = false;
                                    let mut flag_2p = false;
                                    if id == 0 {
                                        if robot_1p.is_none(){
                                            flag_1p = true;
                                        } else if robot_2p.is_none() {
                                            flag_2p = true;
                                        }
                                    } else if id == 1{
                                        flag_1p = true;
                                    } else if id == 2{
                                        flag_2p = true;
                                    }
                                    if flag_1p{
                                        tx.write_all(dbg!(&mut to_slice_cobs(&communication::FromServerData::SetID(1), &mut buf_1p).unwrap())).await.unwrap();
                                        robot_1p = Some(tokio::spawn(robot_handler(tx, fut, controller_1p.clone())));
                                    } else if flag_2p {
                                        tx.write_all(&mut to_slice_cobs(&communication::FromServerData::SetID(2), &mut buf_2p).unwrap()).await.unwrap();
                                        robot_2p = Some(tokio::spawn(robot_handler(tx, fut, controller_2p.clone())));
                                    } else {
                                        println!("?違う人が入ってきたようだ、、、?")
                                    }
                                },
                                _ => {continue;}
                            }
                        }
                    }

                },
            }
        }
    });

    dioxus::launch(App);
}

async fn robot_handler(
    mut write_stream: net::tcp::OwnedWriteHalf,
    mut fut: impl Stream<Item = communication::RobotRespond> + std::marker::Unpin,
    controller: Arc<Mutex<communication::ControllerState>>,
) {
    let mut interval = time::interval(time::Duration::from_millis(
        env!("INTERVAL").parse().unwrap(),
    ));

    let mut buf = [0 as u8; communication::calc_buffer_lengh::<communication::FromServerData>()];

    loop {
        tokio::select! {
            response = fut.next() => {
                println!("{:?}", response);
            },
            _ = interval.tick() => {
                let data;
                {
                    let controller_state = controller.lock().unwrap();
                    data = to_slice_cobs(&communication::FromServerData::Controller(*controller_state), &mut buf).unwrap();
                }
                write_stream.write_all(data).await.unwrap();
            }
        }
    }
}

async fn cobs_reader(
    mut read_stream: net::tcp::OwnedReadHalf,
) -> impl Stream<Item = communication::RobotRespond> {
    const BUFFER_LENGTH: usize = communication::calc_buffer_lengh::<communication::RobotRespond>();
    stream! {
        let mut buf = [0 as u8; BUFFER_LENGTH];
        let mut data_head = 0;
        let mut data = [0 as u8; BUFFER_LENGTH];

        loop {
            // cobsのデコードをする
            match read_stream.read(&mut buf).await {
                Ok(l) => {
                    for i in 0..l{
                        if buf[i] == 0 && data_head == 0 {
                            continue;
                        }
                        data[data_head] = buf[i];
                        data_head += 1;
                        if buf[i] == 0 {
                            yield from_bytes_cobs(&mut data).unwrap();
                            data.fill(0);
                            data_head = 0;
                        }
                    }
                },
                Err(_) => break,
            }
        }
    }
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
    }
}
