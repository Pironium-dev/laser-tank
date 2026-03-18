#![no_std]
pub mod communication {
    use postcard::experimental::max_size::MaxSize;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, MaxSize, Debug)]
    pub enum FromServerData {
        Controller(ControllerState),
        SetID(usize),
    }

    #[derive(Serialize, Deserialize, MaxSize, Debug, Clone, Copy)]
    pub struct ControllerState {
        pub left_stick: f32,
        pub right_stick: f32,
        pub shot: bool,
    }

    impl ControllerState {
        pub fn new() -> Self {
            ControllerState {
                left_stick: 0.0,
                right_stick: 0.0,
                shot: false,
            }
        }
    }
}

// esp32からの通信は&[1]のみ
