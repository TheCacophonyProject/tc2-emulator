//! EEPROM at I2C address 0x50 holding the hardware description, in the V2
//! layout read by tc2-hat-controller's eeprom package:
//! `CA 02 main[3] power[3] touch[3] mic[3] audio_only id[8] time[4] crc[2]`,
//! big-endian, with a CRC-16/AUG-CCITT over everything before it.

use crate::crc::crc16;

pub const ADDRESS: u8 = 0x50;

const SIZE: usize = 256;
const MAGIC: u8 = 0xCA;
const DATA_VERSION: u8 = 2;

/// Hardware revision being emulated. The power PCB version selects the battery
/// voltage dividers, which must match those in the ATtiny model.
pub const MAIN_PCB: [u8; 3] = [0, 7, 0];
pub const POWER_PCB: [u8; 3] = [0, 7, 0];
pub const TOUCH_PCB: [u8; 3] = [0, 1, 0];
pub const MICROPHONE_PCB: [u8; 3] = [0, 1, 0];
const ID: u64 = 0x7C2E_0000_0000_0001;
/// 2026-01-01T00:00:00Z, when this "board" was programmed.
const PROGRAMMED_AT: u32 = 1_767_225_600;

pub struct Eeprom {
    memory: [u8; SIZE],
}

impl Eeprom {
    pub fn new() -> Self {
        let mut data = vec![MAGIC, DATA_VERSION];
        data.extend(MAIN_PCB);
        data.extend(POWER_PCB);
        data.extend(TOUCH_PCB);
        data.extend(MICROPHONE_PCB);
        data.push(0); // Not audio-only.
        data.extend(ID.to_be_bytes());
        data.extend(PROGRAMMED_AT.to_be_bytes());
        data.extend(crc16(&data).to_be_bytes());

        let mut memory = [0xFF; SIZE];
        memory[..data.len()].copy_from_slice(&data);
        Eeprom { memory }
    }

    pub fn tx(&mut self, write: &[u8], read_len: usize) -> Result<Vec<u8>, String> {
        let Some((&address, data)) = write.split_first() else {
            return Err("EEPROM request has no address".into());
        };
        let start = address as usize;
        for (i, &byte) in data.iter().enumerate() {
            self.memory[(start + i) % SIZE] = byte;
        }
        Ok((0..read_len).map(|i| self.memory[(start + i) % SIZE]).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_data_reads_back_in_pages_with_valid_crc() {
        let mut eeprom = Eeprom::new();
        assert_eq!(eeprom.tx(&[0x00], 2).unwrap(), vec![MAGIC, DATA_VERSION]);

        // readEEPROMV2FromChip reads 30 bytes in 16-byte pages.
        let len = 1 + 1 + 3 * 4 + 1 + 8 + 4 + 2;
        let mut data = eeprom.tx(&[0], 16).unwrap();
        data.extend(eeprom.tx(&[16], len - 16).unwrap());
        let (payload, crc) = data.split_last_chunk::<2>().unwrap();
        assert_eq!(crc16(payload), u16::from_be_bytes(*crc));
        assert_eq!(&payload[5..8], &POWER_PCB);
    }
}
