//! Replaces tc2-hat-i2c in the guest. Serves `org.cacophony.i2c.Tx` on the system
//! bus, the only way TC2 software reaches the HAT's I2C devices, and forwards each
//! transaction to the device models in tc2-emu-hw over a virtio-serial port.

use anyhow::Context;
use log::{debug, info, warn};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use tc2_emu_proto::{PORT_NAME, Request, Response};

const DBUS_NAME: &str = "org.cacophony.i2c";
const DBUS_PATH: &str = "/org/cacophony/i2c";
/// Lower bound on the caller's timeout, to allow for the round trip to the host.
const MIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Request/response channel to tc2-emu-hw. Transactions are made one at a time,
/// as on a real bus, so a single lock covers the whole round trip.
struct Bridge {
    inner: Mutex<BridgeInner>,
}

struct BridgeInner {
    port: File,
    responses: Receiver<Response>,
    next_id: u64,
}

impl Bridge {
    fn open() -> anyhow::Result<Self> {
        let path = format!("/dev/virtio-ports/{PORT_NAME}");
        let port = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {path}"))?;
        let (tx, responses) = mpsc::channel();
        let reader = port.try_clone()?;
        std::thread::spawn(move || read_responses(reader, tx));
        Ok(Bridge { inner: Mutex::new(BridgeInner { port, responses, next_id: 1 }) })
    }

    fn i2c_tx(
        &self,
        address: u8,
        write: Vec<u8>,
        read_len: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        let mut line = serde_json::to_vec(&Request { id, address, write, read_len })
            .map_err(|e| e.to_string())?;
        line.push(b'\n');
        inner
            .port
            .write_all(&line)
            .map_err(|e| format!("writing to {PORT_NAME}: {e}"))?;

        // Skip responses to earlier requests that timed out.
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match inner.responses.recv_timeout(remaining) {
                Ok(response) if response.id == id => return response.result,
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("no response from tc2-emu-hw within {timeout:?}"));
                }
                Err(RecvTimeoutError::Disconnected) => return Err("response reader stopped".into()),
            }
        }
    }
}

fn read_responses(port: File, responses: Sender<Response>) {
    let mut reader = BufReader::new(port);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            // The port reads as EOF while nothing is connected on the host side.
            Ok(0) => std::thread::sleep(Duration::from_millis(200)),
            Ok(_) => match serde_json::from_str::<Response>(&line) {
                Ok(response) => {
                    if responses.send(response).is_err() {
                        return;
                    }
                }
                Err(e) => warn!("Ignoring malformed response {line:?}: {e}"),
            },
            Err(e) => {
                warn!("Reading {PORT_NAME}: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cacophony.i2c")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// Same error name tc2-hat-i2c uses for a failed transaction.
    ErrorUsingI2CBus(String),
}

struct I2c {
    bridge: Bridge,
}

#[zbus::interface(name = "org.cacophony.i2c")]
impl I2c {
    #[zbus(name = "Tx")]
    fn tx(
        &self,
        address: u8,
        write: Vec<u8>,
        read_len: i32,
        timeout: i32,
    ) -> Result<Vec<u8>, Error> {
        let read_len = usize::try_from(read_len)
            .map_err(|_| Error::ErrorUsingI2CBus(format!("invalid read length {read_len}")))?;
        let timeout = Duration::from_millis(timeout.max(0) as u64).max(MIN_TIMEOUT);
        let result = self.bridge.i2c_tx(address, write.clone(), read_len, timeout);
        debug!("Tx {address:#04x} write {write:02x?} read {read_len} -> {result:02x?}");
        result.map_err(Error::ErrorUsingI2CBus)
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let bridge = Bridge::open()?;
    let _connection = zbus::blocking::connection::Builder::system()?
        .name(DBUS_NAME)?
        .serve_at(DBUS_PATH, I2c { bridge })?
        .build()?;
    info!("Serving {DBUS_NAME} via tc2-emu-hw");
    loop {
        std::thread::park();
    }
}
