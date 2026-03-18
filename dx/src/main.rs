use communication::communication::{ControllerState, FromServerData};
use dioxus::prelude::*;
use gilrs;
use postcard::{experimental::max_size::MaxSize, to_slice_cobs};
use std::sync::{Arc, Mutex};
use tokio::{self, io::AsyncWriteExt, net, time};

const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[tokio::main]
async fn main() {
    let controller_1p = Arc::new(Mutex::new(ControllerState::new()));
    let controller_2p = Arc::new(Mutex::new(ControllerState::new()));

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
        let mut stream_1p = None;
        let mut stream_2p = None;

        let mut interval = time::interval(time::Duration::from_millis(
            env!("INTERVAL").parse().unwrap(),
        ));

        const BUFFER_SIZE: usize = FromServerData::POSTCARD_MAX_SIZE + 2;

        let mut buf_1p = [0; BUFFER_SIZE];
        let mut buf_2p = [0; BUFFER_SIZE];

        loop {
            tokio::select! {
                Ok(mut i) = listener.accept() => {
                    println!("{:?}", &i);
                    if stream_1p.is_none(){
                        i.0.write_all(dbg!(&mut to_slice_cobs(&FromServerData::SetID(1), &mut buf_1p).unwrap())).await.unwrap();
                        stream_1p = Some(i.0);
                    } else if stream_2p.is_none() {
                        i.0.write_all(&mut to_slice_cobs(&FromServerData::SetID(2), &mut buf_2p).unwrap()).await.unwrap();
                        stream_2p = Some(i.0);
                    } else {
                        println!("?違う人が入ってきたようだ、、、?")
                    }
                },
                _ = interval.tick() => {

                    let data_1p;
                    let data_2p;

                    {
                        let mut controller_1p = controller_1p.lock().unwrap();
                        let mut controller_2p = controller_2p.lock().unwrap();

                        data_1p = to_slice_cobs(&FromServerData::Controller(*controller_1p), &mut buf_1p).unwrap();
                        data_2p = to_slice_cobs(&FromServerData::Controller(*controller_1p), &mut buf_2p).unwrap();

                        controller_1p.shot = false;
                        controller_2p.shot = false;
                    }

                    if let Some(ref mut stream) = stream_1p {
                        match stream.write_all(&data_1p).await {
                            Ok(_) => {
                                println!("SEND_1p: {:?}", &data_1p);
                            },
                            Err(e) => {
                                println!("{e}");
                                stream_1p = None;
                            }
                        }
                    }

                    if let Some(ref mut stream) = stream_2p {
                        match stream.write_all(&data_2p).await {
                            Ok(_) => {
                                println!("SEND_2p: {:?}", &data_2p);
                            },
                            Err(e) => {
                                println!("{e}");
                                stream_2p = None;
                            }
                        }
                    }
                }
            }
        }
    });

    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
    }
}
