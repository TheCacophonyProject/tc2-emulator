//! Replaces tc2-agent in the guest until tc2-agent can talk to an emulated
//! RP2040. Serves the `org.cacophony.TC2Agent` methods managementd calls, with
//! the same replies and status codes as tc2-agent 0.7.3. A requested test
//! recording goes through tc2-agent's status sequence and then saves a fixture
//! CPTV, rewritten to look like it came from this device, where tc2-agent would
//! save an offloaded recording.

mod cptv;

use anyhow::Context;
use chrono::{Local, Utc};
use clap::Parser;
use log::{info, warn};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DBUS_NAME: &str = "org.cacophony.TC2Agent";
const DBUS_PATH: &str = "/org/cacophony/TC2Agent";
/// How long the RP2040 takes to pick up a recording request from the ATtiny.
const REQUEST_DELAY: Duration = Duration::from_secs(2);
const SHORT_RECORDING: Duration = Duration::from_secs(10);
const LONG_RECORDING: Duration = Duration::from_secs(60);

#[derive(Parser)]
struct Args {
    /// Recording saved for each requested test recording.
    #[arg(long, default_value = "/opt/tc2-emu/guest/fixtures/cat-trigger.cptv")]
    fixture: PathBuf,
    #[arg(long, default_value = "/etc/cacophony/config.toml")]
    config: PathBuf,
    /// tc2-agent's default output directory; thermal-uploader uploads from here.
    #[arg(long, default_value = "/var/spool/cptv")]
    output_dir: PathBuf,
    /// Location written into recordings, replacing the fixture's real one. The
    /// default is the test location cacophony-web's e2e tests use.
    #[arg(long, default_value_t = -42.0, allow_negative_numbers = true)]
    latitude: f32,
    #[arg(long, default_value_t = 172.0)]
    longitude: f32,
}

/// tc2-agent's TestRecordingStatus.
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
enum Status {
    Ready = 1,
    WaitingToTakeTestRecording = 2,
    TakingTestRecording = 3,
    TakingLongRecording = 5,
    WaitingToTakeLongRecording = 6,
}

#[derive(Deserialize)]
struct Config {
    device: Option<DeviceConfig>,
}

#[derive(Deserialize)]
struct DeviceConfig {
    name: String,
    id: u32,
}

struct Recorder {
    args: Args,
    status: Mutex<Status>,
}

impl Recorder {
    /// Steps through the statuses tc2-agent reports while the RP2040 records,
    /// then saves the recording. The status is already `Waiting...`.
    fn record(&self, taking: Status, duration: Duration) {
        std::thread::sleep(REQUEST_DELAY);
        *self.status.lock().unwrap() = taking;
        let started = Utc::now();
        std::thread::sleep(duration);
        match self.save(started) {
            Ok(path) => info!("Saved test recording {}", path.display()),
            Err(e) => warn!("Failed to save test recording: {e:#}"),
        }
        *self.status.lock().unwrap() = Status::Ready;
    }

    fn save(&self, started: chrono::DateTime<Utc>) -> anyhow::Result<PathBuf> {
        let config = std::fs::read_to_string(&self.args.config)
            .with_context(|| format!("reading {}", self.args.config.display()))?;
        let config: Config = toml::from_str(&config)?;
        let device = config.device.context("device is not registered")?;
        let fixture = std::fs::read(&self.args.fixture)
            .with_context(|| format!("reading {}", self.args.fixture.display()))?;
        let cptv = cptv::rewrite_header(
            &fixture,
            &cptv::HeaderUpdate {
                timestamp_micros: started.timestamp_micros() as u64,
                device_name: &device.name,
                device_id: device.id,
                motion_config: cptv::TEST_RECORDING_MOTION_CONFIG,
                latitude: self.args.latitude,
                longitude: self.args.longitude,
            },
        )?;

        // Named like tc2-agent's save_cptv_file_to_disk, and written to a temporary
        // name first so thermal-uploader never sees a partial file.
        let name = started.with_timezone(&Local).format("%Y-%m-%d--%H-%M-%S");
        let path = self.args.output_dir.join(format!("{name}.cptv"));
        let partial = self.args.output_dir.join(format!(".{name}.cptv.partial"));
        std::fs::write(&partial, cptv)?;
        std::fs::rename(&partial, &path)?;
        Ok(path)
    }
}

struct Agent {
    recorder: Arc<Recorder>,
}

impl Agent {
    fn status(&self) -> Status {
        *self.recorder.status.lock().unwrap()
    }

    /// Callers poll the status straight after requesting a recording, so it
    /// changes before the reply.
    fn start(&self, waiting: Status, taking: Status, duration: Duration) {
        *self.recorder.status.lock().unwrap() = waiting;
        let recorder = self.recorder.clone();
        std::thread::spawn(move || recorder.record(taking, duration));
    }
}

fn is_taking_test_recording(status: Status) -> bool {
    matches!(status, Status::WaitingToTakeTestRecording | Status::TakingTestRecording)
}

fn is_taking_long_recording(status: Status) -> bool {
    matches!(status, Status::WaitingToTakeLongRecording | Status::TakingLongRecording)
}

#[zbus::interface(name = "org.cacophony.TC2Agent")]
impl Agent {
    #[zbus(name = "shorttestthermalrecording")]
    fn short_test_thermal_recording(&self) -> &'static str {
        let status = self.status();
        if is_taking_test_recording(status) {
            "Already making a test recording"
        } else if is_taking_long_recording(status) {
            "Already making a 1 minute recording"
        } else {
            self.start(
                Status::WaitingToTakeTestRecording,
                Status::TakingTestRecording,
                SHORT_RECORDING,
            );
            "Asked for a test recording"
        }
    }

    /// managementd passes a duration, which tc2-agent ignores.
    #[zbus(name = "longtestthermalrecording")]
    fn long_test_thermal_recording(&self, _seconds: u64) -> &'static str {
        let status = self.status();
        if is_taking_long_recording(status) {
            "Already making a 1 minute recording"
        } else if is_taking_test_recording(status) {
            "Already making a test recording"
        } else {
            self.start(
                Status::WaitingToTakeLongRecording,
                Status::TakingLongRecording,
                LONG_RECORDING,
            );
            "Asked for a 1 minute recording"
        }
    }

    /// Returns (in thermal mode, status).
    #[zbus(name = "testthermalstatus")]
    fn test_thermal_status(&self) -> (i32, u8) {
        (1, self.status() as u8)
    }

    #[zbus(name = "testaudio")]
    fn test_audio(&self) -> &'static str {
        warn!("Audio recordings aren't emulated");
        "Asked for a test recording"
    }

    #[zbus(name = "longaudiorecording")]
    fn long_audio_recording(&self, _seconds: u64) -> &'static str {
        warn!("Audio recordings aren't emulated");
        "Asked for a 5 minute recording"
    }

    /// Returns (in audio mode, status).
    #[zbus(name = "audiostatus")]
    fn audio_status(&self) -> (i32, u8) {
        (0, Status::Ready as u8)
    }

    /// Returns (offloading, percent complete, seconds remaining, total files,
    /// remaining files, total events, remaining events).
    #[zbus(name = "offloadstatus")]
    fn offload_status(&self) -> (i32, u32, u32, u16, u16, u16, u32) {
        (0, 0, 0, 0, 0, 0, 0)
    }

    #[zbus(name = "canceloffload")]
    fn cancel_offload(&self) -> &'static str {
        "No offload in progress"
    }

    #[zbus(name = "forcerp2040offload")]
    fn force_rp2040_offload(&self) -> &'static str {
        "Requested offload of files from rp2040"
    }

    #[zbus(name = "prioritiseframeserve")]
    fn prioritise_frame_serve(&self) -> &'static str {
        "Requested priority frame serving from rp2040"
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let recorder = Arc::new(Recorder { args, status: Mutex::new(Status::Ready) });
    let _connection = zbus::blocking::connection::Builder::system()?
        .name(DBUS_NAME)?
        .serve_at(DBUS_PATH, Agent { recorder })?
        .build()?;
    info!("Serving {DBUS_NAME}");
    loop {
        std::thread::park();
    }
}
