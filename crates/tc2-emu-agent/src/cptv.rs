//! Rewrites the header of a CPTV v2 file, so a fixture recording looks like it
//! was just made by this device. The fixture's location is replaced too, since
//! recordings from real cameras carry where they were made.
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
const LATITUDE: u8 = b'L';
const LONGITUDE: u8 = b'O';
/// Location details that only make sense with the original location.
const DROPPED_LOCATION_FIELDS: [u8; 3] = [b'A', b'U', b'S']; // Altitude, accuracy, timestamp.

/// What tc2-firmware writes to the motion config field of a user-requested test
/// recording (startup and shutdown recordings get `status: startup|shutdown`).
pub const TEST_RECORDING_MOTION_CONFIG: &str = "test: true";

pub struct HeaderUpdate<'a> {
    pub timestamp_micros: u64,
    pub device_name: &'a str,
    pub device_id: u32,
    pub motion_config: &'a str,
    pub latitude: f32,
    pub longitude: f32,
}

pub fn rewrite_header(cptv: &[u8], update: &HeaderUpdate) -> anyhow::Result<Vec<u8>> {
    let mut raw = Vec::new();
    MultiGzDecoder::new(cptv).read_to_end(&mut raw).context("decompressing CPTV")?;
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
    set_field(&mut fields, LATITUDE, update.latitude.to_le_bytes().to_vec());
    set_field(&mut fields, LONGITUDE, update.longitude.to_le_bytes().to_vec());
    fields.retain(|(code, _)| !DROPPED_LOCATION_FIELDS.contains(code));

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

    /// A CPTV with a header like the fixture's, located somewhere real.
    fn cptv_with_location() -> Vec<u8> {
        let fields: [(u8, Vec<u8>); 8] = [
            (TIMESTAMP, 1_758_623_183_625_000u64.to_le_bytes().to_vec()),
            (DEVICE_NAME, b"CI-fake-test-device".to_vec()),
            (DEVICE_ID, 123u32.to_le_bytes().to_vec()),
            (b'J', 76u16.to_le_bytes().to_vec()),
            (LATITUDE, (-43.5f32).to_le_bytes().to_vec()),
            (LONGITUDE, 172.64f32.to_le_bytes().to_vec()),
            (b'A', 12.0f32.to_le_bytes().to_vec()),
            (b'U', 5.0f32.to_le_bytes().to_vec()),
        ];
        let mut raw = MAGIC.to_vec();
        raw.push(fields.len() as u8);
        for (code, value) in &fields {
            raw.push(value.len() as u8);
            raw.push(*code);
            raw.extend(value);
        }
        raw.extend(b"frames");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn rewrites_header_and_keeps_frames() {
        let update = HeaderUpdate {
            timestamp_micros: 1_800_000_000_000_000,
            device_name: "genuinely-harmless-gannet",
            device_id: 184,
            motion_config: TEST_RECORDING_MOTION_CONFIG,
            latitude: -42.0,
            longitude: 172.0,
        };
        let rewritten = rewrite_header(&cptv_with_location(), &update).unwrap();
        let fields = header_fields(&rewritten);
        let field = |code| fields.iter().find(|(c, _)| *c == code).map(|(_, v)| v.clone());

        assert_eq!(field(TIMESTAMP).unwrap(), 1_800_000_000_000_000u64.to_le_bytes());
        assert_eq!(field(DEVICE_NAME).unwrap(), b"genuinely-harmless-gannet");
        assert_eq!(field(DEVICE_ID).unwrap(), 184u32.to_le_bytes());
        assert_eq!(field(MOTION_CONFIG).unwrap(), b"test: true");
        assert_eq!(field(LATITUDE).unwrap(), (-42.0f32).to_le_bytes());
        assert_eq!(field(LONGITUDE).unwrap(), 172.0f32.to_le_bytes());
        assert_eq!(field(b'A'), None, "altitude dropped");
        assert_eq!(field(b'U'), None, "accuracy dropped");
        assert_eq!(field(b'J').unwrap(), 76u16.to_le_bytes(), "frame count untouched");

        let mut raw = Vec::new();
        MultiGzDecoder::new(&rewritten[..]).read_to_end(&mut raw).unwrap();
        assert!(raw.ends_with(b"frames"));
    }
}
