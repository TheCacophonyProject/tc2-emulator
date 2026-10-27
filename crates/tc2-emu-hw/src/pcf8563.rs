//! PCF8563 real-time clock at I2C address 0x51.
//!
//! The clock runs as an offset from the container's clock, so tests can set
//! the device time. Times are UTC with a two-digit year from 2000, as
//! tc2-hat-rtc reads and writes them.

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Timelike, Utc};
use serde::Serialize;

pub const ADDRESS: u8 = 0x51;

const REGISTER_COUNT: usize = 16;
const SECONDS: usize = 0x02;
const YEARS: usize = 0x08;
/// Voltage-low flag in the seconds register; set means the time can't be trusted.
const VL: u8 = 1 << 7;

#[derive(Serialize, Debug)]
pub struct State {
    pub time: DateTime<Utc>,
}

pub struct Pcf8563 {
    regs: [u8; REGISTER_COUNT],
    offset: TimeDelta,
}

fn to_bcd(n: u32) -> u8 {
    ((n / 10) << 4 | n % 10) as u8
}

fn from_bcd(b: u8) -> u32 {
    (b >> 4) as u32 * 10 + (b & 0x0F) as u32
}

impl Pcf8563 {
    pub fn new() -> Self {
        Pcf8563 { regs: [0; REGISTER_COUNT], offset: TimeDelta::zero() }
    }

    pub fn time(&self) -> DateTime<Utc> {
        Utc::now() + self.offset
    }

    pub fn set_time(&mut self, time: DateTime<Utc>) {
        self.offset = time - Utc::now();
    }

    pub fn state(&self) -> State {
        State { time: self.time() }
    }

    fn time_registers(&self) -> [u8; 7] {
        let t = self.time();
        [
            to_bcd(t.second()),
            to_bcd(t.minute()),
            to_bcd(t.hour()),
            to_bcd(t.day()),
            to_bcd(t.weekday().num_days_from_sunday()),
            to_bcd(t.month()),
            to_bcd(t.year().rem_euclid(100) as u32),
        ]
    }

    pub fn tx(&mut self, write: &[u8], read_len: usize) -> Result<Vec<u8>, String> {
        let Some((&start, data)) = write.split_first() else {
            return Err("RTC request has no register".into());
        };
        if !data.is_empty() {
            self.write(start as usize, data)?;
        }
        let mut regs = self.regs;
        regs[SECONDS..=YEARS].copy_from_slice(&self.time_registers());
        Ok((0..read_len).map(|i| regs[(start as usize + i) % REGISTER_COUNT]).collect())
    }

    fn write(&mut self, start: usize, data: &[u8]) -> Result<(), String> {
        let mut time = self.time_registers();
        let mut time_written = false;
        for (i, &value) in data.iter().enumerate() {
            let register = (start + i) % REGISTER_COUNT;
            if (SECONDS..=YEARS).contains(&register) {
                time[register - SECONDS] = value;
                time_written = true;
            } else {
                self.regs[register] = value;
            }
        }
        if time_written {
            let [sec, min, hour, day, _weekday, month, year] = time;
            let written = NaiveDate::from_ymd_opt(
                2000 + from_bcd(year) as i32,
                from_bcd(month & 0x1F),
                from_bcd(day & 0x3F),
            )
            .and_then(|d| {
                d.and_hms_opt(from_bcd(hour & 0x3F), from_bcd(min & 0x7F), from_bcd(sec & !VL))
            })
            .ok_or_else(|| format!("invalid RTC time registers {time:02x?}"))?;
            self.set_time(written.and_utc());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_time_with_integrity() {
        let mut rtc = Pcf8563::new();
        let data = rtc.tx(&[0x00], 9).unwrap();
        assert_eq!(data[0], 0x00, "control register 1");
        assert_eq!(data[2] & VL, 0, "integrity flag");
        let now = Utc::now();
        assert_eq!(2000 + from_bcd(data[8]) as i32, now.year());
        assert_eq!(from_bcd(data[7] & 0x1F), now.month());
    }

    #[test]
    fn time_written_like_tc2_hat_rtc_is_read_back() {
        let mut rtc = Pcf8563::new();
        // 2031-07-04 13:45:30, a Friday; SetTime writes from register 0x02.
        let write = [0x02, 0x30, 0x45, 0x13, 0x04, 0x05, 0x07, 0x31];
        rtc.tx(&write, 0).unwrap();
        let data = rtc.tx(&[0x02], 7).unwrap();
        assert_eq!(&data[1..], &write[2..]);
        assert!(from_bcd(data[0]) - 30 <= 1);
    }

    #[test]
    fn other_registers_are_stored() {
        let mut rtc = Pcf8563::new();
        rtc.tx(&[0x09, 0x15, 0x06, 0x80, 0x80], 0).unwrap();
        assert_eq!(rtc.tx(&[0x09], 4).unwrap(), vec![0x15, 0x06, 0x80, 0x80]);
    }
}
