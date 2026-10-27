//! Rewrites the header of a CPTV v2 file, so a fixture recording looks like it
//! was just made by this device.
//!
//! The decompressed file starts `CPTV 0x02 'H' <field count>`, followed by
//! fields of `<length> <code> <value>` with little-endian numbers (see
//! tc2-firmware's cptv_encoder). Frames follow the header unchanged.

use anyhow::{Context, ensure};
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use std::io::{Read, Write};

const MAGIC: &[u8] = b"CPTV\x02H";
const TIMESTAMP: u8 = b'T';
const DEVICE_NAME: u8 = b'D';
const DEVICE_ID: u8 = b'I';
const MOTION_CONFIG: u8 = b'M';

/// What tc2-firmware writes to the motion config field of a user-requested test
/// recording (startup and shutdown recordings get `status: startup|shutdown`).
pub const TEST_RECORDING_MOTION_CONFIG: &str = "test: true";

pub struct HeaderUpdate<'a> {
    pub timestamp_micros: u64,
    pub device_name: &'a str,
    pub device_id: u32,
    pub motion_config: &'a str,
}

pub fn rewrite_header(cptv: &[u8], update: &HeaderUpdate) -> anyhow::Result<Vec<u8>> {
    let mut raw = Vec::new();
    MultiGzDecoder::new(cptv)
        .read_to_end(&mut raw)
        .context("decompressing CPTV")?;
    ensure!(raw.len() > MAGIC.len() && raw.starts_with(MAGIC), "not a CPTV v2 file");

    let mut fields = Vec::new();
    let mut pos = MAGIC.len() + 1;
    for _ in 0..raw[MAGIC.len()] {
        let len = *raw.get(pos).context("truncated CPTV header")? as usize;
        let value = raw.get(pos + 2..pos + 2 + len).context("truncated CPTV header")?;
        fields.push((raw[pos + 1], value.to_vec()));
        pos += 2 + len;
    }
    let frames = &raw[pos..];

    // Device names longer than this don't fit the one-byte field length.
    let name = &update.device_name.as_bytes()[..update.device_name.len().min(255)];
    set_field(&mut fields, TIMESTAMP, update.timestamp_micros.to_le_bytes().to_vec());
    set_field(&mut fields, DEVICE_NAME, name.to_vec());
    set_field(&mut fields, DEVICE_ID, update.device_id.to_le_bytes().to_vec());
    set_field(&mut fields, MOTION_CONFIG, update.motion_config.as_bytes().to_vec());

    let mut out = MAGIC.to_vec();
    out.push(u8::try_from(fields.len()).context("too many CPTV header fields")?);
    for (code, value) in &fields {
        out.push(value.len() as u8);
        out.push(*code);
        out.extend(value);
    }
    out.extend(frames);

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&out)?;
    Ok(encoder.finish()?)
}

fn set_field(fields: &mut Vec<(u8, Vec<u8>)>, code: u8, value: Vec<u8>) {
    match fields.iter_mut().find(|(c, _)| *c == code) {
        Some((_, existing)) => *existing = value,
        None => fields.push((code, value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_fields(cptv: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut raw = Vec::new();
        MultiGzDecoder::new(cptv).read_to_end(&mut raw).unwrap();
        let mut fields = Vec::new();
        let mut pos = MAGIC.len() + 1;
        for _ in 0..raw[MAGIC.len()] {
            let len = raw[pos] as usize;
            fields.push((raw[pos + 1], raw[pos + 2..pos + 2 + len].to_vec()));
            pos += 2 + len;
        }
        fields
    }

    #[test]
    fn rewrites_fixture_header_and_keeps_frames() {
        let fixture = include_bytes!("../../../fixtures/cat-trigger.cptv");
        let update = HeaderUpdate {
            timestamp_micros: 1_800_000_000_000_000,
            device_name: "genuinely-harmless-gannet",
            device_id: 184,
            motion_config: TEST_RECORDING_MOTION_CONFIG,
        };
        let rewritten = rewrite_header(fixture, &update).unwrap();
        let fields = header_fields(&rewritten);
        let field = |code| fields.iter().find(|(c, _)| *c == code).unwrap().1.clone();

        assert_eq!(field(TIMESTAMP), 1_800_000_000_000_000u64.to_le_bytes());
        assert_eq!(field(DEVICE_NAME), b"genuinely-harmless-gannet");
        assert_eq!(field(DEVICE_ID), 184u32.to_le_bytes());
        assert_eq!(field(MOTION_CONFIG), b"test: true");
        assert_eq!(field(b'J'), 76u16.to_le_bytes(), "frame count untouched");

        let decompressed_len = |cptv: &[u8]| {
            let mut raw = Vec::new();
            MultiGzDecoder::new(cptv).read_to_end(&mut raw).unwrap();
            raw.len()
        };
        // The fixture has no motion config field, so it gains one.
        let growth = "genuinely-harmless-gannet".len() - "CI-fake-test-device".len()
            + 2
            + "test: true".len();
        assert_eq!(decompressed_len(&rewritten), decompressed_len(fixture) + growth);
    }
}
