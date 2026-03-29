#![no_std]
pub mod communication {
    use postcard::experimental::max_size::MaxSize;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, MaxSize, Debug)]
    pub enum FromServerData {
        Controller(ControllerState),
        SetID(u8),
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

    #[derive(Serialize, Deserialize, MaxSize, Debug)]
    pub struct RobotRespond {
        pub id: u8,
        pub method: RobotMethod,
    }

    #[derive(Serialize, Deserialize, MaxSize, Debug)]
    pub enum RobotMethod {
        HeartBeat,
        Hit(u8),
    }

}

// esp32からの通信は&[1]のみ
