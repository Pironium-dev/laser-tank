use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, MaxSize)]
pub enum Controll {
    HeartBeat,
    Fire,
    Drive(f32, f32),
}

// esp32からの通信は&[1]のみ
