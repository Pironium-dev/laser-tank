use dioxus::prelude::*;
use gilrs;
use std::sync::{Arc, Mutex};
use tokio;

const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[derive(Debug)]
struct Controller {
    left_stick: f32,
    right_stick: f32,
    shot: bool,
}

impl Controller {
    fn new() -> Self {
        Controller {
            left_stick: 0.0,
            right_stick: 0.0,
            shot: false,
        }
    }
}

#[tokio::main]
async fn main() {
    let controller_1p = Arc::new(Mutex::new(Controller::new()));
    let controller_2p = Arc::new(Mutex::new(Controller::new()));

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
        loop {
            println!("{:?}", &controller_1p);
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
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
