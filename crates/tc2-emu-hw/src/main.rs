//! Runs in the emulator container next to QEMU. Serves the guest stubs' requests
//! over QEMU's virtio-serial socket, the emulated RP2040's (tc2-firmware-emu) over
//! a unix socket, as both are on the HAT's I2C bus, and a control API for tests.

mod aht20;
mod attiny;
mod control;
mod crc;
mod eeprom;
mod hat;
mod pcf8563;
mod qmp;

use anyhow::Context;
use clap::Parser;
use control::SharedHat;
use hat::Hat;
use log::{debug, info, warn};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tc2_emu_proto::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::io::AsyncReadExt;
use tokio::net::{UnixListener, UnixStream};

#[derive(Parser)]
struct Args {
    /// QEMU chardev socket backing the guest's virtio-serial port.
    #[arg(long, env = "TC2_EMU_HW_SOCKET", default_value = "/state/hw.sock")]
    socket: PathBuf,
    /// Socket to listen on for the emulated RP2040's I2C transactions.
    #[arg(long, env = "TC2_EMU_HW_RP2040_SOCKET", default_value = "/state/hw-rp2040.sock")]
    rp2040_socket: PathBuf,
    /// QEMU's QMP socket, to power cycle the guest.
    #[arg(long, env = "TC2_EMU_HW_QMP_SOCKET", default_value = "/state/qmp.sock")]
    qmp_socket: PathBuf,
    /// The RTC's time of day (UTC, as HH:MM) when started, instead of the container's. It
    /// is the next such time from now, so it is never behind the real time (the API files
    /// recordings by their time) and then runs at the usual rate. Tests use it so
    /// their results don't depend on the time of day they run at.
    #[arg(long, env = "TC2_START_TIME")]
    start_time: Option<String>,
    /// Start with time warp on (see `PUT /time-warp`): 1 or true. Empty, 0 and false
    /// are off, so that an unset variable passed through by docker compose is.
    #[arg(long, env = "TC2_TIME_WARP", default_value = "")]
    time_warp: String,
    /// Address for the control API.
    #[arg(long, env = "TC2_EMU_HW_LISTEN", default_value = "0.0.0.0:9000")]
    listen: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let hat: SharedHat = Arc::new(Mutex::new(Hat::new()));
    hat.lock().unwrap().time_warp = matches!(args.time_warp.as_str(), "1" | "true");
    if let Some(start_time) = args.start_time.as_deref().filter(|t| !t.is_empty()) {
        let start = next_time_of_day(start_time, chrono::Utc::now())
            .with_context(|| format!("--start-time {start_time:?} isn't HH:MM"))?;
        info!("RTC starts at {start}");
        hat.lock().unwrap().rtc.set_time(start);
    }

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("binding control API to {}", args.listen))?;
    info!("Control API on {}", args.listen);
    let router = control::router(hat.clone());
    tokio::spawn(async move { axum::serve(listener, router).await });

    let _ = std::fs::remove_file(&args.rp2040_socket);
    let rp2040 = UnixListener::bind(&args.rp2040_socket)
        .with_context(|| format!("binding {}", args.rp2040_socket.display()))?;
    info!("RP2040 I2C socket at {}", args.rp2040_socket.display());
    let rp2040_hat = hat.clone();
    tokio::spawn(async move {
        loop {
            match rp2040.accept().await {
                Ok((stream, _)) => {
                    let hat = rp2040_hat.clone();
                    tokio::spawn(async move {
                        info!("RP2040 connected");
                        if let Err(e) = serve_rp2040(stream, &hat).await {
                            debug!("RP2040 connection: {e:#}");
                        }
                        info!("RP2040 disconnected");
                    });
                }
                Err(e) => warn!("Accepting RP2040 connection: {e}"),
            }
        }
    });

    let qmp_hat = hat.clone();
    let qmp_socket = args.qmp_socket.clone();
    tokio::spawn(async move { qmp::run(&qmp_socket, &qmp_hat).await });

    loop {
        match serve_guest(&args.socket, &hat).await {
            Ok(()) => info!("Guest connection closed"),
            Err(e) => debug!("Guest connection: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The next time of day `hh_mm` (UTC) at or after `now`, so within a day ahead. The
/// API files a recording under the device as it was at the recording's time, so a
/// device clock behind the real one would put a new recording under the device's
/// earlier registration.
fn next_time_of_day(
    hh_mm: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    use chrono::{NaiveTime, TimeDelta};
    let time = NaiveTime::parse_from_str(hh_mm, "%H:%M")?;
    let today = now.date_naive().and_time(time).and_utc();
    Ok(if today < now { today + TimeDelta::days(1) } else { today })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn start_time_is_the_next_time_of_day() {
        let now = Utc.with_ymd_and_hms(2026, 11, 24, 3, 0, 0).unwrap();
        let at = |d, h| Utc.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap();
        assert_eq!(next_time_of_day("03:00", now).unwrap(), at(24, 3));
        assert_eq!(next_time_of_day("12:00", now).unwrap(), at(24, 12));
        assert_eq!(next_time_of_day("00:00", now).unwrap(), at(25, 0));
        assert!(next_time_of_day("noon", now).is_err());
    }
}

/// QEMU owns the socket and may not have started yet, so this is retried.
async fn serve_guest(socket: &Path, hat: &SharedHat) -> anyhow::Result<()> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    info!("Connected to guest port at {}", socket.display());
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(e) => {
                warn!("Ignoring malformed request {line:?}: {e}");
                continue;
            }
        };
        let Request { id, address, write, read_len } = request;
        let result = hat.lock().unwrap().i2c_tx(address, &write, read_len);
        debug!("I2C {address:#04x} write {write:02x?} read {read_len} -> {result:02x?}");
        let mut response = serde_json::to_vec(&Response { id, result })?;
        response.push(b'\n');
        writer.write_all(&response).await?;
    }
    Ok(())
}

/// The RP2040's transactions, framed as tc2-firmware's emulator sends them (its
/// src/emulator/hw.rs): a request is `[address][read length: u16 LE][write length:
/// u16 LE][write bytes]`, answered with `[status][length: u16 LE][bytes]`, where
/// status 0 means the bytes are the data read, and 1 that they're an error message.
async fn serve_rp2040(mut stream: UnixStream, hat: &SharedHat) -> anyhow::Result<()> {
    loop {
        let mut header = [0u8; 5];
        stream.read_exact(&mut header).await?;
        let address = header[0];
        let read_len = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut write = vec![0u8; u16::from_le_bytes([header[3], header[4]]) as usize];
        stream.read_exact(&mut write).await?;
        // Without power, the RP2040 can't use the bus; it starts again at the alarm.
        while hat.lock().unwrap().rp2040_is_asleep() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let result = hat.lock().unwrap().i2c_tx(address, &write, read_len);
        debug!("RP2040 I2C {address:#04x} write {write:02x?} read {read_len} -> {result:02x?}");
        let (status, data) = match result {
            Ok(data) => (0, data),
            Err(e) => (1, e.into_bytes()),
        };
        let mut response = vec![status];
        response.extend((data.len() as u16).to_le_bytes());
        response.extend(data);
        stream.write_all(&response).await?;
    }
}
