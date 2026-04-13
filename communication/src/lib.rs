#![no_std]

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, MaxSize, Debug)]
pub enum ServerData {
    Controller(ControllerState),
    SetID(u8),
}

#[derive(Serialize, Deserialize, MaxSize, Debug, Clone, Copy, Default)]
pub struct ControllerState {
    pub stick: (i8, i8),
    pub shot: u8,
}

#[derive(Serialize, Deserialize, MaxSize, Debug)]
pub struct RobotRespond {
    pub robot_id: u8,
    pub shot_id: u8,
}
