#![no_std]

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, MaxSize, Debug)]
pub enum FromServerData {
    Controller(ControllerState),
    SetID(u8),
}

#[derive(Serialize, Deserialize, MaxSize, Debug, Clone, Copy, Default)]
pub struct ControllerState {
    pub left_stick: f32,
    pub right_stick: f32,
    pub shot: u8,
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
