use esp_hal::{
    gpio::Output,
    mcpwm::{PwmPeripheral, operator::PwmPin},
};

pub struct Motor<'a, T: PwmPeripheral, const X: u8> {
    pin: PwmPin<'a, T, X, true>,
    phase: Output<'a>,
}

impl<'a, T: PwmPeripheral, const X: u8> Motor<'_, T, X> {
    pub fn new(pin: PwmPin<'a, T, { X }, true>, phase: Output<'a>) -> Motor<'a, T, { X }> {
        Motor { pin, phase }
    }
    pub fn set_velocity(&mut self, v: f32) {
        if v > 0.0 {
            self.phase.set_low();
        } else {
            self.phase.set_high();
        }
        self.pin.set_timestamp((v.abs() * 100.0) as u16);
    }
}
