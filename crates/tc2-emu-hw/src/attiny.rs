//! ATtiny1616 power controller at I2C address 0x25.
//!
//! Register map and protocol from tc2-hat-controller (internal/tc2-hat-attiny):
//! requests are `[register, (value), crc_hi, crc_lo]` and reads return
//! `[value, crc_hi, crc_lo]`, using CRC-16/AUG-CCITT. tc2-agent uses register
//! 0x07 as a mailbox with the RP2040.

use crate::crc::crc16;
use serde::{Deserialize, Serialize};

pub const ADDRESS: u8 = 0x25;

/// Firmware version tc2-hat-controller 0.34.3 expects; any other version makes
/// tc2-hat-attiny try to reflash the ATtiny over UPDI.
pub const FIRMWARE_VERSION: (u8, u8, u8) = (1, 0, 10);

const TYPE_VALUE: u8 = 0xCA;

pub mod reg {
    pub const TYPE: u8 = 0x00;
    pub const MAJOR_VERSION: u8 = 0x01;
    pub const CAMERA_STATE: u8 = 0x02;
    pub const CAMERA_CONNECTION: u8 = 0x03;
    pub const PI_COMMANDS: u8 = 0x04;
    pub const RP2040_PI_POWER_CTRL: u8 = 0x05;
    pub const TC2_AGENT_STATE: u8 = 0x07;
    pub const MINOR_VERSION: u8 = 0x08;
    pub const PATCH_VERSION: u8 = 0x0B;
    pub const BOOT_DURATION_1: u8 = 0x0C;
    pub const BOOT_DURATION_2: u8 = 0x0D;
    pub const TIME_SINCE_INTERACTION: u8 = 0x0F;
    pub const LV_BATTERY_1: u8 = 0x13;
    pub const HV_BATTERY_1: u8 = 0x15;
    pub const RTC_BATTERY_1: u8 = 0x17;
    /// The ATtiny1616's RSTCTRL.RSTFR, why it last reset.
    pub const RESET_FLAGS: u8 = 0x30;
}

const REGISTER_COUNT: usize = 0x31;
/// Power-on reset, the only reset the emulated ATtiny has had.
const POWER_ON_RESET: u8 = 0x01;
const READ_ONLY: [u8; 4] = [reg::TYPE, reg::MAJOR_VERSION, reg::MINOR_VERSION, reg::PATCH_VERSION];
/// Writing this bit to the first register of a battery pair starts an analog
/// reading; the ATtiny clears it when the reading is ready.
const ANALOG_START: u8 = 1 << 7;
const BOOT_DURATION_SECONDS: u16 = 30;

/// Voltage divider in front of an ATtiny analog input.
#[derive(Clone, Copy, Debug)]
pub struct Divider {
    pub vref: f32,
    pub r1: f32,
    pub r2: f32,
}

impl Divider {
    /// 10-bit ADC reading for `volts` at the divider input, as computed in
    /// reverse by tc2-hat-attiny's calculateBatteryVoltage.
    fn raw_reading(&self, volts: f32) -> u16 {
        let v_out = volts * self.r2 / (self.r1 + self.r2);
        (v_out * 1023.0 / self.vref).round().clamp(0.0, 1023.0) as u16
    }
}

/// Dividers for power PCB v0.7.0 (lvResistorVals, hvResistorVals and
/// rtcResistorVals in tc2-hat-attiny).
pub const LV_DIVIDER: Divider = Divider { vref: 3.3, r1: 2000.0, r2: 470.0 };
pub const HV_DIVIDER: Divider = Divider { vref: 3.3, r1: 2000.0, r2: 168.0 };
pub const RTC_DIVIDER: Divider = Divider { vref: 3.3, r1: 0.0, r2: 1.0 };

/// Inputs to the ATtiny that tests can change.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Inputs {
    pub hv_battery_volts: f32,
    pub lv_battery_volts: f32,
    pub rtc_battery_volts: f32,
    pub minutes_since_interaction: u8,
    /// Bit 0 of register 0x05. When clear, tc2-hat-attiny powers the Pi off once
    /// nothing else needs it on, so it defaults to on.
    pub rp2040_wants_pi_on: bool,
}

impl Default for Inputs {
    fn default() -> Self {
        Inputs {
            hv_battery_volts: 12.0,
            lv_battery_volts: 0.0,
            rtc_battery_volts: 3.0,
            minutes_since_interaction: 0,
            rp2040_wants_pi_on: true,
        }
    }
}

/// Partial update of [`Inputs`] from the control API.
#[derive(Deserialize, Debug, Default)]
pub struct InputsPatch {
    pub hv_battery_volts: Option<f32>,
    pub lv_battery_volts: Option<f32>,
    pub rtc_battery_volts: Option<f32>,
    pub minutes_since_interaction: Option<u8>,
    pub rp2040_wants_pi_on: Option<bool>,
}

/// Register values the Pi writes, as reported by the control API.
#[derive(Serialize, Debug)]
pub struct State {
    pub inputs: Inputs,
    pub camera_state: u8,
    pub camera_connection: u8,
    pub pi_commands: u8,
    pub tc2_agent_state: u8,
}

pub struct Attiny {
    regs: [u8; REGISTER_COUNT],
    inputs: Inputs,
}

impl Attiny {
    pub fn new() -> Self {
        let mut regs = [0; REGISTER_COUNT];
        regs[reg::TYPE as usize] = TYPE_VALUE;
        regs[reg::MAJOR_VERSION as usize] = FIRMWARE_VERSION.0;
        regs[reg::MINOR_VERSION as usize] = FIRMWARE_VERSION.1;
        regs[reg::PATCH_VERSION as usize] = FIRMWARE_VERSION.2;
        let [hi, lo] = BOOT_DURATION_SECONDS.to_be_bytes();
        regs[reg::BOOT_DURATION_1 as usize] = hi;
        regs[reg::BOOT_DURATION_2 as usize] = lo;
        regs[reg::RESET_FLAGS as usize] = POWER_ON_RESET;
        let mut attiny = Attiny { regs, inputs: Inputs::default() };
        attiny.apply_inputs();
        attiny
    }

    pub fn state(&self) -> State {
        State {
            inputs: self.inputs.clone(),
            camera_state: self.regs[reg::CAMERA_STATE as usize],
            camera_connection: self.regs[reg::CAMERA_CONNECTION as usize],
            pi_commands: self.regs[reg::PI_COMMANDS as usize],
            tc2_agent_state: self.regs[reg::TC2_AGENT_STATE as usize],
        }
    }

    pub fn update_inputs(&mut self, patch: InputsPatch) {
        let inputs = &mut self.inputs;
        if let Some(v) = patch.hv_battery_volts {
            inputs.hv_battery_volts = v;
        }
        if let Some(v) = patch.lv_battery_volts {
            inputs.lv_battery_volts = v;
        }
        if let Some(v) = patch.rtc_battery_volts {
            inputs.rtc_battery_volts = v;
        }
        if let Some(v) = patch.minutes_since_interaction {
            inputs.minutes_since_interaction = v;
        }
        if let Some(v) = patch.rp2040_wants_pi_on {
            inputs.rp2040_wants_pi_on = v;
        }
        self.apply_inputs();
    }

    fn apply_inputs(&mut self) {
        self.regs[reg::TIME_SINCE_INTERACTION as usize] = self.inputs.minutes_since_interaction;
        let power = &mut self.regs[reg::RP2040_PI_POWER_CTRL as usize];
        if self.inputs.rp2040_wants_pi_on {
            *power |= 0x01;
        } else {
            *power &= !0x01;
        }
    }

    pub fn tx(&mut self, write: &[u8], read_len: usize) -> Result<Vec<u8>, String> {
        // i2crequest.CheckAddress probes with a bare register byte and no CRC.
        if let [register] = write {
            return Ok(self.read(*register, read_len));
        }
        let Some((payload, crc)) = write.split_last_chunk::<2>() else {
            return Err("ATtiny request too short".into());
        };
        if crc16(payload) != u16::from_be_bytes(*crc) {
            return Err("ATtiny request CRC mismatch".into());
        }
        match *payload {
            [register] => {
                let len = read_len.checked_sub(2).ok_or("ATtiny read must include 2 CRC bytes")?;
                let mut response = self.read(register, len);
                let crc = crc16(&response);
                response.extend(crc.to_be_bytes());
                Ok(response)
            }
            [register, value] => {
                self.write(register, value);
                Ok(vec![0; read_len])
            }
            _ => Err(format!("unexpected ATtiny request {payload:02x?}")),
        }
    }

    fn read(&self, register: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| self.regs.get(register as usize + i).copied().unwrap_or(0)).collect()
    }

    fn write(&mut self, register: u8, value: u8) {
        if READ_ONLY.contains(&register) || register as usize >= REGISTER_COUNT {
            return;
        }
        if value & ANALOG_START != 0 {
            let reading = match register {
                reg::LV_BATTERY_1 => Some(LV_DIVIDER.raw_reading(self.inputs.lv_battery_volts)),
                reg::HV_BATTERY_1 => Some(HV_DIVIDER.raw_reading(self.inputs.hv_battery_volts)),
                reg::RTC_BATTERY_1 => Some(RTC_DIVIDER.raw_reading(self.inputs.rtc_battery_volts)),
                _ => None,
            };
            if let Some(raw) = reading {
                let [hi, lo] = raw.to_be_bytes();
                self.regs[register as usize] = hi;
                self.regs[register as usize + 1] = lo;
                return;
            }
        }
        self.regs[register as usize] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc_request(payload: &[u8]) -> Vec<u8> {
        let mut request = payload.to_vec();
        request.extend(crc16(payload).to_be_bytes());
        request
    }

    fn read_register(attiny: &mut Attiny, register: u8) -> u8 {
        let response = attiny.tx(&crc_request(&[register]), 3).unwrap();
        assert_eq!(crc16(&response[..1]).to_be_bytes(), response[1..]);
        response[0]
    }

    #[test]
    fn identifies_with_expected_type_and_version() {
        let mut attiny = Attiny::new();
        assert_eq!(attiny.tx(&[reg::TYPE], 1).unwrap(), vec![TYPE_VALUE]);
        assert_eq!(read_register(&mut attiny, reg::TYPE), TYPE_VALUE);
        assert_eq!(read_register(&mut attiny, reg::MAJOR_VERSION), 1);
        assert_eq!(read_register(&mut attiny, reg::MINOR_VERSION), 0);
        assert_eq!(read_register(&mut attiny, reg::PATCH_VERSION), 10);
        assert_eq!(read_register(&mut attiny, reg::RESET_FLAGS), POWER_ON_RESET);
    }

    #[test]
    fn writes_are_read_back_and_versions_are_read_only() {
        let mut attiny = Attiny::new();
        attiny.tx(&crc_request(&[reg::TC2_AGENT_STATE, 0x42]), 0).unwrap();
        assert_eq!(read_register(&mut attiny, reg::TC2_AGENT_STATE), 0x42);
        attiny.tx(&crc_request(&[reg::MAJOR_VERSION, 7]), 0).unwrap();
        assert_eq!(read_register(&mut attiny, reg::MAJOR_VERSION), 1);
    }

    #[test]
    fn rejects_bad_crc() {
        let mut attiny = Attiny::new();
        assert!(attiny.tx(&[reg::TYPE, 0x00, 0x00], 3).is_err());
    }

    #[test]
    fn battery_reading_round_trips_through_divider() {
        let mut attiny = Attiny::new();
        attiny.update_inputs(InputsPatch { hv_battery_volts: Some(12.0), ..Default::default() });
        attiny.tx(&crc_request(&[reg::HV_BATTERY_1, ANALOG_START]), 0).unwrap();
        let hi = read_register(&mut attiny, reg::HV_BATTERY_1);
        let lo = read_register(&mut attiny, reg::HV_BATTERY_1 + 1);
        assert_eq!(hi & ANALOG_START, 0, "reading should be marked complete");
        let raw = u16::from_be_bytes([hi, lo]) as f32;
        // tc2-hat-attiny's calculateBatteryVoltage.
        let d = HV_DIVIDER;
        let volts = raw * d.vref / 1023.0 * (d.r1 + d.r2) / d.r2;
        assert!((volts - 12.0).abs() < 0.05, "got {volts}");
    }

    #[test]
    fn keeps_pi_on_by_default() {
        let mut attiny = Attiny::new();
        assert_eq!(read_register(&mut attiny, reg::RP2040_PI_POWER_CTRL) & 1, 1);
        attiny.update_inputs(InputsPatch { rp2040_wants_pi_on: Some(false), ..Default::default() });
        assert_eq!(read_register(&mut attiny, reg::RP2040_PI_POWER_CTRL) & 1, 0);
    }
}
