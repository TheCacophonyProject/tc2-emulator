/// CRC-16/AUG-CCITT, used by the ATtiny's I2C protocol and the EEPROM data.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0x1D0F;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// CRC-8 (poly 0x31, init 0xFF), used by the AHT20.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_matches_i2c_service_example() {
        // From tc2-hat-i2c's example of reading ATtiny register 0x00: CRC 0xcc,0x9c.
        assert_eq!(crc16(&[0x00]), 0xCC9C);
    }

    #[test]
    fn crc8_matches_aht20_datasheet_example() {
        // Sensirion-style CRC-8 check value for 0xBEEF.
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
    }
}
