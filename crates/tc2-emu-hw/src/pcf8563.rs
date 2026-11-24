//! PCF8563 real-time clock at I2C address 0x51.
//!
//! The clock runs as an offset from the container's clock, so tests can set
//! the device time. Times are UTC with a two-digit year from 2000, as
//! tc2-hat-rtc reads and writes them.

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Timelike, Utc};
use serde::Serialize;

pub const ADDRESS: u8 = 0x51;

const REGISTER_COUNT: usize = 16;
const CONTROL_STATUS_2: usize = 0x01;
const SECONDS: usize = 0x02;
const YEARS: usize = 0x08;
const ALARM_MINUTE: usize = 0x09;
const ALARM_DAY: usize = 0x0B;
/// Alarm fired flag in control/status 2, which stays set until it's cleared.
const AF: u8 = 1 << 3;
/// Set in an alarm register to leave that field out of the match.
const AE: u8 = 1 << 7;
/// Voltage-low flag in the seconds register; set means the time can't be trusted.
const VL: u8 = 1 << 7;

#[derive(Serialize, Debug)]
pub struct State {
    pub time: DateTime<Utc>,
}

pub struct Pcf8563 {
    regs: [u8; REGISTER_COUNT],
    offset: TimeDelta,
    /// The next time the alarm registers match, until it fires.
    alarm_at: Option<DateTime<Utc>>,
}

fn to_bcd(n: u32) -> u8 {
    ((n / 10) << 4 | n % 10) as u8
}

fn from_bcd(b: u8) -> u32 {
    (b >> 4) as u32 * 10 + (b & 0x0F) as u32
}

impl Pcf8563 {
    pub fn new() -> Self {
        let mut regs = [0; REGISTER_COUNT];
        // No alarm, as after a power on with the alarm registers cleared.
        regs[ALARM_MINUTE..=ALARM_MINUTE + 3].fill(AE);
        Pcf8563 { regs, offset: TimeDelta::zero(), alarm_at: None }
    }

    /// When the alarm will go off, if it's set and hasn't yet.
    pub fn alarm_at(&mut self) -> Option<DateTime<Utc>> {
        self.update_alarm();
        self.alarm_at
    }

    pub fn alarm_fired(&mut self) -> bool {
        self.update_alarm();
        self.regs[CONTROL_STATUS_2] & AF != 0
    }

    fn update_alarm(&mut self) {
        if self.alarm_at.is_some_and(|at| self.time() >= at) {
            self.regs[CONTROL_STATUS_2] |= AF;
            self.alarm_at = None;
        }
    }

    /// The first minute after `after` whose minute, hour and day of the month match
    /// the alarm registers' enabled fields, as the chip matches on the minute
    /// rolling over. The weekday isn't matched: tc2-firmware keeps its alarm mode there.
    fn next_alarm(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let field = |register: usize| {
            let value = self.regs[register];
            (value & AE == 0).then(|| from_bcd(value & 0x7F))
        };
        let (minute, hour, day) = (field(ALARM_MINUTE), field(ALARM_MINUTE + 1), field(ALARM_DAY));
        if minute.is_none() && hour.is_none() && day.is_none() {
            return None;
        }
        let mut t = after.with_second(0)?.with_nanosecond(0)? + TimeDelta::minutes(1);
        // Far enough for any day of the month to come round.
        for _ in 0..(32 * 24 * 60) {
            if minute.is_none_or(|m| m == t.minute())
                && hour.is_none_or(|h| h == t.hour())
                && day.is_none_or(|d| d == t.day())
            {
                return Some(t);
            }
            t += TimeDelta::minutes(1);
        }
        None
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
        self.update_alarm();
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
        let end = start + data.len();
        let wrote = |register: usize| (start..end).contains(&register);
        if wrote(CONTROL_STATUS_2) || (ALARM_MINUTE..=ALARM_MINUTE + 3).any(wrote) {
            // Writing the flag as clear acknowledges the alarm.
            self.alarm_at = self.next_alarm(self.time());
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
            self.alarm_at = self.alarm_at.and(self.next_alarm(self.time()));
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

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2031, 7, day).unwrap().and_hms_opt(hour, minute, 0).unwrap().and_utc()
    }

    /// The alarm as tc2-firmware sets it: minutes, hours, day, and the weekday
    /// disabled, then the control register with the interrupt enabled.
    fn set_alarm(rtc: &mut Pcf8563, day: u8, hour: u8, minute: u8) {
        rtc.tx(&[0x09, to_bcd(minute as u32), to_bcd(hour as u32), to_bcd(day as u32), 0x80], 0)
            .unwrap();
        rtc.tx(&[0x01, 0b10], 0).unwrap();
    }

    #[test]
    fn no_alarm_until_one_is_set() {
        let mut rtc = Pcf8563::new();
        assert_eq!(rtc.alarm_at(), None);
        assert!(!rtc.alarm_fired());
    }

    #[test]
    fn alarm_is_the_next_match_and_fires_when_the_time_reaches_it() {
        let mut rtc = Pcf8563::new();
        rtc.set_time(at(4, 13, 45));
        set_alarm(&mut rtc, 5, 9, 30);
        assert_eq!(rtc.alarm_at(), Some(at(5, 9, 30)));
        assert!(!rtc.alarm_fired());
        assert_eq!(rtc.tx(&[0x01], 1).unwrap()[0] & AF, 0);

        rtc.set_time(at(5, 9, 30));
        assert!(rtc.alarm_fired());
        assert_eq!(rtc.tx(&[0x01], 1).unwrap()[0] & AF, AF);
        assert_eq!(rtc.alarm_at(), None);
    }

    #[test]
    fn writing_the_control_register_acknowledges_the_alarm() {
        let mut rtc = Pcf8563::new();
        rtc.set_time(at(4, 13, 45));
        set_alarm(&mut rtc, 4, 13, 50);
        rtc.set_time(at(4, 13, 51));
        assert!(rtc.alarm_fired());
        rtc.tx(&[0x01, 0], 0).unwrap();
        assert!(!rtc.alarm_fired());
        // The same time comes round next month, not at once.
        assert_eq!(rtc.alarm_at().map(|t| t.day()), Some(4));
        assert!(rtc.alarm_at().unwrap() > at(4, 13, 51));
    }

    #[test]
    fn a_disabled_alarm_never_fires() {
        let mut rtc = Pcf8563::new();
        set_alarm(&mut rtc, 5, 9, 30);
        rtc.tx(&[0x09, 0x80, 0x80, 0x80, 0x80], 0).unwrap();
        assert_eq!(rtc.alarm_at(), None);
    }
}
