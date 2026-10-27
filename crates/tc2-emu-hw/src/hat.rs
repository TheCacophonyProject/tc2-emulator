//! The devices on the TC2 HAT's I2C bus, as seen from the Pi.

use crate::{aht20, attiny, eeprom, pcf8563};
use serde::Serialize;

pub struct Hat {
    pub attiny: attiny::Attiny,
    pub rtc: pcf8563::Pcf8563,
    pub aht20: aht20::Aht20,
    pub eeprom: eeprom::Eeprom,
}

#[derive(Serialize)]
pub struct State {
    pub attiny: attiny::State,
    pub rtc: pcf8563::State,
    pub aht20: aht20::Inputs,
}

impl Hat {
    pub fn new() -> Self {
        Hat {
            attiny: attiny::Attiny::new(),
            rtc: pcf8563::Pcf8563::new(),
            aht20: aht20::Aht20::new(),
            eeprom: eeprom::Eeprom::new(),
        }
    }

    pub fn state(&self) -> State {
        State {
            attiny: self.attiny.state(),
            rtc: self.rtc.state(),
            aht20: self.aht20.inputs.clone(),
        }
    }

    /// Errors are reported to the guest like a failed transaction on a real bus.
    pub fn i2c_tx(
        &mut self,
        address: u8,
        write: &[u8],
        read_len: usize,
    ) -> Result<Vec<u8>, String> {
        match address {
            attiny::ADDRESS => self.attiny.tx(write, read_len),
            pcf8563::ADDRESS => self.rtc.tx(write, read_len),
            aht20::ADDRESS => self.aht20.tx(write, read_len),
            eeprom::ADDRESS => self.eeprom.tx(write, read_len),
            _ => Err(format!("no device at I2C address {address:#04x}")),
        }
    }
}
