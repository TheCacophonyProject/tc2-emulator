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
    /// Address for the control API.
    #[arg(long, env = "TC2_EMU_HW_LISTEN", default_value = "0.0.0.0:9000")]
    listen: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let hat: SharedHat = Arc::new(Mutex::new(Hat::new()));

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

    loop {
        match serve_guest(&args.socket, &hat).await {
            Ok(()) => info!("Guest connection closed"),
            Err(e) => debug!("Guest connection: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
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
