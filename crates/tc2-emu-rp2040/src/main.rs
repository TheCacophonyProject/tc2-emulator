//! Scripted stand-in for the RP2040 in high-power thermal mode, talking to
//! tc2-agent over its emulated link (`--rp2040-link`, see tc2-agent's
//! rp2040_link.rs). It makes the startup handshake, then streams flat frames, so
//! the real tc2-agent can run before tc2-firmware's simulation sits behind the link.
//!
//! Runs in the container, connected to QEMU's socket for the guest's
//! `org.cacophony.tc2-emu.rp2040` virtio-serial port.

use anyhow::Context;
use clap::Parser;
use crc::{CRC_16_XMODEM, Crc};
use log::{debug, info, warn};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread::sleep;
use std::time::{Duration, Instant};

// Link message kinds.
const PING: u8 = 1;
const ACK: u8 = 2;
const READ: u8 = 3;
const DATA: u8 = 4;
const WRITE: u8 = 5;
const RESTART: u8 = 6;

// Transfer types, from tc2-agent's camera_transfer_state.rs.
const CAMERA_CONNECT_INFO: u8 = 0x1;
const CAMERA_RAW_FRAME_TRANSFER: u8 = 0x2;
const CAMERA_STARTUP_HANDSHAKE: u8 = 0x9;

/// EXPECTED_RP2040_FIRMWARE_VERSION in the pinned tc2-agent release (see
/// guest/versions/packages.txt); otherwise tc2-agent tries to reprogram the RP2040.
const FIRMWARE_VERSION: u32 = 40;
const LEPTON_SERIAL: u32 = 123_456;
/// tc2-agent reads this many bytes for every transfer, then the rest of a frame.
const FIRST_READ: usize = 2066;
/// 160x122 16-bit words: two rows of Lepton telemetry, then the image.
const FRAME_LENGTH: usize = 160 * 122 * 2;
const TELEMETRY_LENGTH: usize = 160 * 2 * 2;
const FRAME_INTERVAL: Duration = Duration::from_millis(115);
/// Raw Lepton value for the flat frames, roughly room temperature.
const BACKGROUND: u16 = 29_000;
/// How long to wait on tc2-agent mid-transfer before giving up on it.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);

#[derive(Parser)]
struct Args {
    /// QEMU chardev socket backing the guest's rp2040 virtio-serial port.
    #[arg(long, env = "TC2_EMU_RP2040_SOCKET", default_value = "/state/rp2040.sock")]
    socket: PathBuf,
}

enum Outcome {
    /// The transfer completed; holds what tc2-agent wrote back, if anything.
    Done(Option<Vec<u8>>),
    /// tc2-agent toggled the run pin.
    Restart,
    /// tc2-agent stopped responding, e.g. because it restarted.
    TimedOut,
}

struct Link {
    stream: UnixStream,
    messages: Receiver<(u8, Vec<u8>)>,
}

impl Link {
    /// QEMU owns the socket and may not have started yet, so this retries.
    fn connect(socket: &PathBuf) -> Link {
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(stream) => break stream,
                Err(e) => debug!("Connecting to {}: {e}", socket.display()),
            }
            sleep(Duration::from_secs(1));
        };
        info!("Connected to guest port at {}", socket.display());
        let mut reader = stream.try_clone().expect("cloning socket");
        let (tx, messages) = channel();
        std::thread::spawn(move || {
            while let Ok(message) = read_message(&mut reader) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        });
        Link { stream, messages }
    }

    fn send(&mut self, kind: u8, payload: &[u8]) -> anyhow::Result<()> {
        let mut message = vec![kind];
        message.extend((payload.len() as u32).to_le_bytes());
        message.extend(payload);
        self.stream.write_all(&message).context("writing to guest port")
    }

    /// Pings tc2-agent and serves its SPI reads of `transfer_type` with `payload`.
    /// `header_crc` replaces the payload CRC in the header, as raw frames use it
    /// as the "is recording" flag.
    fn transfer(
        &mut self,
        transfer_type: u8,
        payload: &[u8],
        header_crc: Option<u16>,
    ) -> anyhow::Result<Outcome> {
        let crc = header_crc.unwrap_or_else(|| CRC.checksum(payload));
        let len = payload.len() as u32;
        let mut bytes = vec![transfer_type, transfer_type];
        bytes.extend(len.to_le_bytes());
        bytes.extend(len.to_le_bytes());
        for value in [crc, crc, !crc, !crc] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(payload);
        let total = bytes.len();
        bytes.resize(total.max(FIRST_READ), 0);

        self.send(PING, &[])?;
        let mut cursor = 0;
        loop {
            let (kind, message) = match self.messages.recv_timeout(RESPONSE_TIMEOUT) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => return Ok(Outcome::TimedOut),
                Err(RecvTimeoutError::Disconnected) => anyhow::bail!("guest port closed"),
            };
            match kind {
                ACK => {}
                READ => {
                    let n = u32::from_le_bytes(message[..4].try_into()?) as usize;
                    let end = (cursor + n).min(bytes.len());
                    let mut data = bytes[cursor..end].to_vec();
                    data.resize(n, 0);
                    cursor += n;
                    self.send(DATA, &data)?;
                    // Raw frames aren't acknowledged with a write.
                    if transfer_type == CAMERA_RAW_FRAME_TRANSFER && cursor >= total {
                        return Ok(Outcome::Done(None));
                    }
                }
                WRITE => return Ok(Outcome::Done(Some(message))),
                RESTART => return Ok(Outcome::Restart),
                _ => warn!("Unexpected message kind {kind} from tc2-agent"),
            }
        }
    }
}

fn read_message(reader: &mut UnixStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok((header[0], payload))
}

fn handshake_payload(config_crc: u16) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(0u16.to_le_bytes()); // Events to offload.
    payload.extend(FIRMWARE_VERSION.to_le_bytes());
    payload.extend(0u16.to_le_bytes()); // Files to offload.
    payload.extend(0u16.to_le_bytes()); // Flash blocks to offload.
    payload.extend(config_crc.to_le_bytes());
    payload
}

fn connect_info_payload() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(2u32.to_le_bytes()); // Radiometry enabled.
    payload.extend(0u32.to_le_bytes());
    payload.extend(LEPTON_SERIAL.to_le_bytes());
    payload.extend(0u32.to_le_bytes()); // Thermal, not audio, mode.
    payload
}

/// A flat frame. tc2-agent reads the frame number, time on, and time of the last
/// flat-field correction from the telemetry rows.
fn frame(frame_num: u32, msec_on: u32) -> Vec<u8> {
    let mut frame = vec![0u8; TELEMETRY_LENGTH];
    frame[2..6].copy_from_slice(&msec_on.to_le_bytes());
    frame[40..44].copy_from_slice(&frame_num.to_le_bytes());
    frame.extend(BACKGROUND.to_le_bytes().repeat((FRAME_LENGTH - TELEMETRY_LENGTH) / 2));
    frame
}

/// The CRC of the device config tc2-agent sent in reply to a startup handshake.
fn sent_config_crc(reply: &[u8]) -> Option<u16> {
    let crc = u16::from_le_bytes(reply.get(8..10)?.try_into().ok()?);
    let len = *reply.get(12)? as usize;
    let config = reply.get(14..14 + len)?;
    (CRC.checksum(config) == crc).then_some(crc)
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let mut link = Link::connect(&args.socket);

    // The config tc2-agent last sent, reported back at the next startup. tc2-agent
    // restarts the RP2040 if it's out of date.
    let mut stored_config_crc = 0;
    'boot: loop {
        info!("Starting up");
        let booted = Instant::now();
        match link.transfer(
            CAMERA_STARTUP_HANDSHAKE,
            &handshake_payload(stored_config_crc),
            None,
        )? {
            Outcome::Done(Some(reply)) => match sent_config_crc(&reply) {
                Some(crc) => stored_config_crc = crc,
                None => warn!("No valid device config in the handshake reply"),
            },
            Outcome::Done(None) | Outcome::TimedOut => continue 'boot,
            Outcome::Restart => continue 'boot,
        }
        info!("Handshake done, device config CRC {stored_config_crc}");

        match link.transfer(CAMERA_CONNECT_INFO, &connect_info_payload(), None)? {
            Outcome::Done(_) => {}
            Outcome::TimedOut | Outcome::Restart => continue 'boot,
        }

        let mut frame_num = 0;
        let mut next_frame = Instant::now();
        loop {
            next_frame += FRAME_INTERVAL;
            sleep(next_frame.saturating_duration_since(Instant::now()));
            let msec_on = booted.elapsed().as_millis() as u32;
            match link.transfer(CAMERA_RAW_FRAME_TRANSFER, &frame(frame_num, msec_on), Some(0))? {
                Outcome::Done(_) => {}
                // tc2-agent restarts the RP2040 when it starts up again.
                Outcome::TimedOut => next_frame = Instant::now(),
                Outcome::Restart => continue 'boot,
            }
            frame_num += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_has_expected_length_and_telemetry() {
        let frame = frame(7, 1234);
        assert_eq!(frame.len(), FRAME_LENGTH);
        assert_eq!(u32::from_le_bytes(frame[40..44].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(frame[2..6].try_into().unwrap()), 1234);
        assert_eq!(&frame[TELEMETRY_LENGTH..TELEMETRY_LENGTH + 2], &BACKGROUND.to_le_bytes());
    }

    #[test]
    fn reads_config_crc_from_handshake_reply() {
        let config = b"some config";
        let crc = CRC.checksum(config);
        let mut reply = vec![0u8; 136];
        reply[8..10].copy_from_slice(&crc.to_le_bytes());
        reply[12] = config.len() as u8;
        reply[14..14 + config.len()].copy_from_slice(config);
        assert_eq!(sent_config_crc(&reply), Some(crc));
        reply[14] ^= 0xFF;
        assert_eq!(sent_config_crc(&reply), None);
    }
}
