//! Runs in the emulator container next to QEMU. Serves the guest stubs' requests
//! over QEMU's virtio-serial socket, and a control API for tests.

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
use tokio::net::UnixStream;

#[derive(Parser)]
struct Args {
    /// QEMU chardev socket backing the guest's virtio-serial port.
    #[arg(long, env = "TC2_EMU_HW_SOCKET", default_value = "/state/hw.sock")]
    socket: PathBuf,
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
