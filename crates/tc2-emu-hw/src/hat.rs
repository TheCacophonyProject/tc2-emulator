//! The devices on the TC2 HAT's I2C bus, as seen from the Pi.

use crate::{aht20, attiny, eeprom, pcf8563};
use serde::Serialize;

pub struct Hat {
    pub attiny: attiny::Attiny,
    pub rtc: pcf8563::Pcf8563,
    pub aht20: aht20::Aht20,
    pub eeprom: eeprom::Eeprom,
    /// Whether the Pi is powered off, as QEMU reports.
    pub pi_off: bool,
    /// The RP2040 has had its power cut by the ATtiny, until the RTC's alarm.
    pub rp2040_asleep: bool,
    /// While the Pi and the RP2040 are both off, jump the RTC to its alarm instead of
    /// waiting for it.
    pub time_warp: bool,
}

#[derive(Serialize)]
pub struct State {
    pub attiny: attiny::State,
    pub rtc: pcf8563::State,
    pub aht20: aht20::Inputs,
    pub pi_off: bool,
    pub rp2040_asleep: bool,
    pub time_warp: bool,
}

impl Hat {
    pub fn new() -> Self {
        Hat {
            attiny: attiny::Attiny::new(),
            rtc: pcf8563::Pcf8563::new(),
            aht20: aht20::Aht20::new(),
            eeprom: eeprom::Eeprom::new(),
            pi_off: false,
            rp2040_asleep: false,
            time_warp: false,
        }
    }

    /// The Pi's power was cut (QEMU shut down) or restored.
    pub fn set_pi_off(&mut self, off: bool) {
        self.pi_off = off;
        self.attiny.set_pi_powered(!off);
    }

    /// Whether the RP2040 is still without power, which is until the RTC's alarm. With
    /// time warp, and the Pi off too so nothing is running in real time, the alarm
    /// comes at once.
    pub fn rp2040_is_asleep(&mut self) -> bool {
        if !self.rp2040_asleep {
            return false;
        }
        if self.time_warp && self.pi_off {
            if let Some(at) = self.rtc.alarm_at() {
                self.rtc.set_time(at);
            }
        }
        if self.rtc.alarm_fired() {
            self.rp2040_asleep = false;
        }
        self.rp2040_asleep
    }

    pub fn state(&self) -> State {
        State {
            pi_off: self.pi_off,
            rp2040_asleep: self.rp2040_asleep,
            time_warp: self.time_warp,
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
        let result = self.i2c_tx_device(address, write, read_len);
        if self.attiny.take_sleep_request() {
            self.rp2040_asleep = true;
        }
        result
    }

    fn i2c_tx_device(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc::crc16;
    use chrono::{TimeZone, Utc};

    fn attiny_write(hat: &mut Hat, register: u8, value: u8) {
        let payload = [register, value];
        let mut request = payload.to_vec();
        request.extend(crc16(&payload).to_be_bytes());
        hat.i2c_tx(attiny::ADDRESS, &request, 0).unwrap();
    }

    fn sleep_until_alarm(hat: &mut Hat) -> chrono::DateTime<Utc> {
        let alarm = Utc::now() + chrono::TimeDelta::hours(5);
        hat.rtc.set_time(Utc::now());
        let (minute, hour, day) = (alarm.minute(), alarm.hour(), alarm.day());
        let bcd = |n: u32| ((n / 10) << 4 | n % 10) as u8;
        hat.i2c_tx(pcf8563::ADDRESS, &[0x09, bcd(minute), bcd(hour), bcd(day), 0x80], 0).unwrap();
        hat.i2c_tx(pcf8563::ADDRESS, &[0x01, 0b10], 0).unwrap();
        attiny_write(hat, 0x05, 0x02);
        Utc.with_ymd_and_hms(alarm.year(), alarm.month(), alarm.day(), hour, minute, 0).unwrap()
    }

    use chrono::{Datelike, Timelike};

    #[test]
    fn the_rp2040_sleeps_until_the_alarm() {
        let mut hat = Hat::new();
        assert!(!hat.rp2040_is_asleep());
        sleep_until_alarm(&mut hat);
        assert!(hat.rp2040_is_asleep());
        assert!(hat.rp2040_is_asleep(), "still, as the alarm is hours away");
    }

    #[test]
    fn time_warp_skips_to_the_alarm_only_when_the_pi_is_off_too() {
        let mut hat = Hat::new();
        hat.time_warp = true;
        let alarm = sleep_until_alarm(&mut hat);
        assert!(hat.rp2040_is_asleep(), "the Pi is on, so time passes as usual");
        hat.set_pi_off(true);
        assert!(!hat.rp2040_is_asleep());
        assert_eq!(hat.rtc.time().timestamp(), alarm.timestamp());
        assert!(hat.rtc.alarm_fired());
    }

    #[test]
    fn without_time_warp_the_alarm_is_waited_for() {
        let mut hat = Hat::new();
        hat.set_pi_off(true);
        sleep_until_alarm(&mut hat);
        assert!(hat.rp2040_is_asleep());
    }

    #[test]
    fn the_camera_state_follows_the_pi_s_power() {
        let mut hat = Hat::new();
        hat.set_pi_off(true);
        assert_eq!(hat.attiny.state().camera_state, 3);
        hat.set_pi_off(false);
        assert_eq!(hat.attiny.state().camera_state, 0);
    }
}
