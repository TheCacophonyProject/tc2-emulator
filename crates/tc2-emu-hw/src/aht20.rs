//! AHT20 temperature and humidity sensor at I2C address 0x38.
//!
//! tc2-hat-temp triggers a measurement with `AC 33 00`, then reads 7 bytes
//! after the status register 0x71: status, 20 bits of humidity, 20 bits of
//! temperature and a CRC-8. Measurements are ready immediately.

use crate::crc::crc8;
use serde::{Deserialize, Serialize};

pub const ADDRESS: u8 = 0x38;

const STATUS_REG: u8 = 0x71;
const STATUS_CALIBRATED: u8 = 1 << 3;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Inputs {
    pub temperature_celsius: f64,
    pub humidity_percent: f64,
}

impl Default for Inputs {
    fn default() -> Self {
        Inputs { temperature_celsius: 18.0, humidity_percent: 60.0 }
    }
}

#[derive(Deserialize, Debug, Default)]
pub struct InputsPatch {
    pub temperature_celsius: Option<f64>,
    pub humidity_percent: Option<f64>,
}

pub struct Aht20 {
    pub inputs: Inputs,
}

impl Aht20 {
    pub fn new() -> Self {
        Aht20 { inputs: Inputs::default() }
    }

    pub fn update_inputs(&mut self, patch: InputsPatch) {
        if let Some(v) = patch.temperature_celsius {
            self.inputs.temperature_celsius = v;
        }
        if let Some(v) = patch.humidity_percent {
            self.inputs.humidity_percent = v;
        }
    }

    pub fn tx(&mut self, write: &[u8], read_len: usize) -> Result<Vec<u8>, String> {
        match write.first() {
            // Status/measurement read; the presence check reads 1 byte of it.
            Some(&STATUS_REG) | Some(0x00) => {
                Ok(self.measurement().into_iter().take(read_len).collect())
            }
            // Trigger measurement (AC 33 00) or calibrate (BE 08 00).
            Some(0xAC) | Some(0xBE) => Ok(vec![0; read_len]),
            _ => Err(format!("unexpected AHT20 request {write:02x?}")),
        }
    }

    fn measurement(&self) -> [u8; 7] {
        let full_scale = (1u32 << 20) as f64;
        let humidity = (self.inputs.humidity_percent.clamp(0.0, 100.0) / 100.0 * full_scale)
            .round()
            .min(full_scale - 1.0) as u32;
        let temperature = ((self.inputs.temperature_celsius + 50.0) / 200.0 * full_scale)
            .round()
            .clamp(0.0, full_scale - 1.0) as u32;
        let mut data = [
            STATUS_CALIBRATED,
            (humidity >> 12) as u8,
            (humidity >> 4) as u8,
            ((humidity & 0x0F) << 4) as u8 | (temperature >> 16) as u8,
            (temperature >> 8) as u8,
            temperature as u8,
            0,
        ];
        data[6] = crc8(&data[..6]);
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurement_decodes_like_tc2_hat_temp() {
        let mut aht20 = Aht20::new();
        aht20.update_inputs(InputsPatch {
            temperature_celsius: Some(21.5),
            humidity_percent: Some(45.0),
        });
        aht20.tx(&[0xAC, 0x33, 0x00], 0).unwrap();
        let raw = aht20.tx(&[STATUS_REG], 7).unwrap();

        assert_eq!(raw[0] & 0x80, 0, "not busy");
        assert_ne!(raw[0] & STATUS_CALIBRATED, 0, "calibrated");
        let humidity_raw = (raw[1] as u32) << 12 | (raw[2] as u32) << 4 | (raw[3] as u32) >> 4;
        let humidity = humidity_raw as f64 / (1 << 20) as f64 * 100.0;
        let temperature_raw = ((raw[3] & 0x0F) as u32) << 16 | (raw[4] as u32) << 8 | raw[5] as u32;
        let temperature = temperature_raw as f64 / (1 << 20) as f64 * 200.0 - 50.0;
        assert!((humidity - 45.0).abs() < 0.01, "humidity {humidity}");
        assert!((temperature - 21.5).abs() < 0.01, "temperature {temperature}");
        assert_eq!(raw[6], crc8(&raw[..6]));
    }
}
