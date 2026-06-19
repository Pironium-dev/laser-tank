use rodio;
use std::fs::File;
use std::io::BufReader;
use std::sync::LazyLock;

use dioxus::prelude::*;
use tokio::time::{Duration, sleep};
mod logic;

const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

const TIMER_MAX: f32 = 180.0;
const RELOAD_INC: f32 = 5.0;

fn main() {
    dioxus::launch(App);
}

#[derive(PartialEq, Props, Clone)]
struct PlayerProps {
    player_num: u8,
    is_left: bool,
    connected: bool,
    lives: i32,
    reload_percentage: f32,
}

#[component]
fn PlayerArea(props: PlayerProps) -> Element {
    let border_color = if props.connected {
        if props.is_left {
            "border-blue-500 shadow-[0_0_15px_rgba(59,130,246,0.3)]"
        } else {
            "border-red-500 shadow-[0_0_15px_rgba(239,68,68,0.3)]"
        }
    } else {
        "border-gray-600"
    };

    let text_color = if props.connected {
        if props.is_left {
            "text-blue-400"
        } else {
            "text-red-400"
        }
    } else {
        "text-gray-500"
    };

    let theme_color = if props.is_left { "#3b82f6" } else { "#ef4444" };

    rsx! {
        div {
            class: "flex-1 bg-gray-800 rounded-xl p-6 border-2 flex flex-col items-center justify-evenly shadow-lg transition-all duration-500 {border_color}",
            h2 { class: "text-[clamp(1.5rem,3vw,2.5rem)] font-bold {text_color}", "PLAYER {props.player_num}" }

            div {
                class: "text-[clamp(1rem,2vw,1.5rem)] font-semibold",
                if props.connected {
                    span { class: "text-green-400", "● CONNECTED" }
                } else {
                    span { class: "text-gray-500", "○ DISCONNECTED" }
                }
            }

            // 残機 (Lives)
            div {
                class: "flex gap-[2%] justify-center w-full",
                for _ in 0..props.lives {
                    span { class: "text-[clamp(2.5rem,5vw,5.5rem)] leading-none text-pink-500 drop-shadow-[0_0_8px_rgba(236,72,153,0.8)] transition-all", "♥" }
                }
                for _ in props.lives..3 {
                    span { class: "text-[clamp(2.5rem,5vw,5.5rem)] leading-none text-gray-700", "♥" }
                }
            }

            // リロード (Reload Pie Chart)
            div {
                class: "w-full flex flex-col items-center",
                span { class: "mb-[5%] text-[clamp(0.75rem,2vw,1.5rem)] text-gray-400 font-bold tracking-widest", "RELOAD" }
                div {
                    class: "w-[50%] max-w-[220px] min-w-[80px] aspect-square rounded-full bg-gray-700 flex items-center justify-center relative shadow-inner text-[clamp(1.2rem,3.5vw,3rem)]",
                    style: "background: conic-gradient({theme_color} {props.reload_percentage}%, #374151 {props.reload_percentage}%); transform: scaleX(-1);",
                    div {
                        class: "absolute inset-[10%] bg-gray-800 rounded-full flex items-center justify-center shadow-md",
                        style: "transform: scaleX(-1);",
                        span {
                            class: "font-bold text-gray-300 font-mono",
                            "{props.reload_percentage as i32}%"
                        }
                    }
                }
            }

        }
    }
}

#[component]
fn App() -> Element {
    let mut is_playing = use_signal(|| false);
    let mut timer = use_signal(|| TIMER_MAX);

    let p1_connected = use_signal(|| false);
    let mut p1_lives = use_signal(|| 3);
    let mut p1_reload = use_signal(|| 100.0);

    let p2_connected = use_signal(|| false);
    let mut p2_lives = use_signal(|| 3);
    let mut p2_reload = use_signal(|| 100.0);

    let mut countdown_text = use_signal(|| "".to_string());
    let mut is_counting_down = use_signal(|| false);
    let mut is_fullscreen = use_signal(|| false);

    use_future(move || async move {
        loop {
            sleep(Duration::from_millis(100)).await;
            if is_playing() {
                *timer.write() -= 0.1;
                if *timer.read() <= 0.0 {
                    is_playing.set(false);
                    *timer.write() = TIMER_MAX;
                    spawn(async move {
                        let start_wav = BufReader::new(File::open("assets/finish.wav").unwrap());
                        let _player = rodio::play(SINK_HANDLE.mixer(), start_wav);
                        sleep(Duration::from_millis(1500)).await;
                    });
                }
            }

            if *p1_reload.read() < 100.0 {
                *p1_reload.write() += RELOAD_INC;
                if *p1_reload.read() > 100.0 {
                    *p1_reload.write() = 100.0;
                }
            }
            if *p2_reload.read() < 100.0 {
                *p2_reload.write() += RELOAD_INC;
                if *p2_reload.read() > 100.0 {
                    *p2_reload.write() = 100.0;
                }
            }
        }
    });

    // 音関連

    static SINK_HANDLE: LazyLock<rodio::stream::MixerDeviceSink> =
        LazyLock::new(|| rodio::DeviceSinkBuilder::open_default_sink().unwrap());

    let logic_coroutine;

    {
        let mut p1_connected = p1_connected.clone();
        let mut p2_connected = p2_connected.clone();
        let mut p1_lives = p1_lives.clone();
        let mut p2_lives = p2_lives.clone();

        logic_coroutine = use_coroutine(
            move |mut rx: UnboundedReceiver<logic::ToRobot>| async move {
                let (to_robot_tx, mut to_server_rx) = logic::init();
                loop {
                    tokio::select! {
                        x = rx.recv() => {
                            match x.unwrap() {
                                logic::ToRobot::Stop => {
                                    to_robot_tx.send((0, logic::ToRobot::Stop)).await.unwrap();
                                }
                                logic::ToRobot::Start => {
                                    to_robot_tx.send((0, logic::ToRobot::Start)).await.unwrap();
                                }
                                _ => {}
                            }
                        }
                        x = to_server_rx.recv() => {
                            match x {
                                Some(msg) => {
                                    match msg.1 {
                                        logic::ToServer::Connect => {
                                            if msg.0 == 1 {
                                                p1_connected.set(true);
                                            } else if msg.0 == 2 {
                                                p2_connected.set(true);
                                            }
                                        },
                                        logic::ToServer::Disconnect => {
                                            if msg.0 == 1 {
                                                p1_connected.set(false);
                                            } else if msg.0 == 2 {
                                                p2_connected.set(false);
                                            }
                                        },
                                        logic::ToServer::Hit => {
                                            if msg.0 == 1 && *p1_lives.read() > 0 && *is_playing.read() {
                                                *p1_lives.write() -= 1;
                                                if *p1_lives.read() == 0 {
                                                    timer.set(0.0);
                                                }
                                            } else if msg.0 == 2 && *p2_lives.read() > 0 && *is_playing.read() {
                                                *p2_lives.write() -= 1;
                                                if *p2_lives.read() == 0 {
                                                    timer.set(0.0);
                                                }
                                            }
                                        },
                                        logic::ToServer::AskShot => {
                                            if msg.0 == 1 && *p1_reload.read() >= 100.0 {
                                                *p1_reload.write() = 0.0;
                                                to_robot_tx.send((1, logic::ToRobot::AllowShot)).await.unwrap();
                                            } else if msg.0 == 2 && *p2_reload.read() >= 100.0 {
                                                *p2_reload.write() = 0.0;
                                                to_robot_tx.send((2, logic::ToRobot::AllowShot)).await.unwrap();
                                            }
                                        },
                                    }
                                },
                                None => {
                                    break;
                                }
                            }
                        }
                    }
                }
            },
        );
    }

    let timer_percentage = (timer() / TIMER_MAX) * 100.0;
    let current_secs = timer() as u32;
    let mins = current_secs / 60;
    let secs = current_secs % 60;

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        div {
            class: "min-h-screen bg-gray-900 text-white font-sans flex flex-col relative overflow-hidden",

            if !countdown_text().is_empty() {
                div {
                    class: "absolute inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-md",
                    h1 {
                        class: "text-[20vw] font-black text-white drop-shadow-[0_0_40px_rgba(255,255,255,0.8)] tracking-widest",
                        "{countdown_text()}"
                    }
                }
            }

            header {
                class: "text-center py-4 bg-gray-800 shadow-lg z-10 border-b border-gray-700 relative",
                h1 { class: "text-[clamp(1.5rem,3vw,2.5rem)] font-bold text-blue-400 tracking-wider drop-shadow-[0_0_10px_rgba(96,165,250,0.5)]", "LASER TANK BATTLE" }

                button {
                    class: "absolute right-6 top-1/2 -translate-y-1/2 p-3 bg-gray-700 hover:bg-gray-600 rounded-lg text-white font-bold transition-all shadow-[0_0_10px_rgba(0,0,0,0.3)]",
                    onclick: move |_| {
                        let current = *is_fullscreen.read();
                        *is_fullscreen.write() = !current;
                        #[cfg(feature = "desktop")]
                        {
                            let window = dioxus::desktop::window();
                            window.set_fullscreen(!current);
                        }
                    },
                    if is_fullscreen() { "🗗" } else { "⛶" }
                }
            }

            main {
                class: "flex-1 flex flex-row items-stretch p-6 gap-8",

                PlayerArea {
                    player_num: 1,
                    is_left: true,
                    connected: p1_connected(),
                    lives: p1_lives(),
                    reload_percentage: p1_reload(),
                }

                div {
                    class: "w-1/3 flex flex-col items-center justify-center bg-gray-800/40 rounded-xl p-8 shadow-inner border border-gray-700/50",

                    // ゲーム時間 (Timer Pie Chart)
                    div {
                        class: "w-[clamp(150px,35vmin,450px)] aspect-square mb-16 rounded-full relative shadow-[0_0_25px_rgba(234,179,8,0.2)] flex-shrink-0",
                        style: "background: conic-gradient(#eab308 {timer_percentage}%, #374151 {timer_percentage}%); transform: scaleX(-1); transition: background 0.1s linear;",
                        div {
                            class: "absolute inset-[6%] bg-gray-900 rounded-full flex items-center justify-center shadow-inner",
                            style: "transform: scaleX(-1);",
                            div {
                                class: "flex flex-col items-center justify-center w-full h-full",
                                span { class: "text-[clamp(0.8rem,2.5vmin,2rem)] text-yellow-600/80 font-bold mb-[2%] tracking-widest", "TIME" }
                                span {
                                    class: "text-[clamp(3rem,9vmin,9rem)] leading-none font-mono font-bold text-yellow-400 drop-shadow-[0_0_10px_rgba(250,204,21,0.6)]",
                                    "{mins:02}:{secs:02}"
                                }
                            }
                        }
                    }

                    button {
                        class: "px-10 py-5 bg-red-600 hover:bg-red-500 text-white font-bold rounded-xl text-2xl transition-all duration-200 shadow-[0_0_15px_rgba(220,38,38,0.5)] hover:shadow-[0_0_25px_rgba(239,68,68,0.8)] active:scale-95",
                        onclick: move |_| {
                            if !is_playing() && !is_counting_down() {
                                is_counting_down.set(true);
                                *p1_lives.write() = 3;
                                *p2_lives.write() = 3;
                                timer.set(TIMER_MAX);

                                spawn(async move {
                                    logic_coroutine.send(logic::ToRobot::Stop);
                                    let start_wav = BufReader::new(File::open("assets/321 start.wav").unwrap());
                                    let _player = rodio::play(SINK_HANDLE.mixer(), start_wav);
                                    sleep(Duration::from_millis(1250)).await;
                                    for i in ["3", "2", "1", "START!"]{
                                        countdown_text.set(i.to_string());
                                        sleep(Duration::from_secs(1)).await;
                                    }
                                    logic_coroutine.send(logic::ToRobot::Start);

                                    countdown_text.set("".to_string());
                                    is_counting_down.set(false);
                                    is_playing.set(true);
                                });
                            } else {
                                is_playing.set(false);
                                is_counting_down.set(false);
                                countdown_text.set("".to_string());
                                timer.set(TIMER_MAX);
                            }
                        },
                        if is_playing() { "PLAYING..." } else if is_counting_down() { "WAITING..." } else { "START GAME" }
                    }
                }

                PlayerArea {
                    player_num: 2,
                    is_left: false,
                    connected: p2_connected(),
                    lives: p2_lives(),
                    reload_percentage: p2_reload(),
                }
            }
        }
    }
}
