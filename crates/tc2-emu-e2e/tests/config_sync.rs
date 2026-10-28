//! cacophony-config-sync keeps the power mode, recording window and audio recording
//! mode the same on the device and the API, where the web portal edits them.
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.
//!
//! Released cacophony-config-sync keeps the device id and token it started with,
//! so after Sidekick reregisters the device it syncs as the old device until it
//! restarts, and these tests time out. The emulator runs it with a fix from
//! guest/patches/go-config.

use serde_json::{Value, json};
use std::time::Duration;
use tc2_emu_e2e::{TestDevice, now_iso, wait_for};

/// The sync waits 10s after the config file changes, and the emulated device is slow.
const SYNC_TIMEOUT: Duration = Duration::from_secs(180);

struct Case {
    low_power: bool,
    /// `AudioRecordingMode`: AudioOrThermal, AudioAndThermal, AudioOnly or Disabled.
    audio_mode: &'static str,
    /// A time like "22:00", or relative to sunset/sunrise like "-30m".
    start: &'static str,
    stop: &'static str,
}

const CASES: [Case; 4] = [
    // Custom recording window, low power.
    Case { low_power: true, audio_mode: "AudioOrThermal", start: "22:00", stop: "06:00" },
    // The portal's "always": the same start and stop.
    Case { low_power: false, audio_mode: "AudioAndThermal", start: "12:00", stop: "12:00" },
    // The portal's default window, relative to sunset and sunrise.
    Case { low_power: true, audio_mode: "AudioOnly", start: "-30m", stop: "+30m" },
    Case { low_power: false, audio_mode: "Disabled", start: "09:00", stop: "17:00" },
];

/// The device's version of the settings in `Case`, from managementd.
fn device_case(device: &TestDevice) -> anyhow::Result<(bool, String, String, String)> {
    let config = device.managementd.config()?;
    Ok((
        config["thermalRecorder"]["UseLowPowerMode"].as_bool().unwrap_or_default(),
        config["audioRecording"]["AudioMode"].as_str().unwrap_or_default().into(),
        config["windows"]["StartRecording"].as_str().unwrap_or_default().into(),
        config["windows"]["StopRecording"].as_str().unwrap_or_default().into(),
    ))
}

fn expected(case: &Case) -> (bool, String, String, String) {
    (case.low_power, case.audio_mode.into(), case.start.into(), case.stop.into())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn portal_settings_sync_to_device() -> anyhow::Result<()> {
    let device = TestDevice::set_up()?;
    println!("Device {} in group {}", device.device_id, device.group);

    for case in &CASES {
        // The web portal sends every section it edited, each stamped as updated now.
        let updated = now_iso();
        device.user.update_device_settings(
            device.device_id,
            json!({
                "thermalRecording": { "useLowPowerMode": case.low_power, "updated": updated },
                "audioRecording": { "audioMode": case.audio_mode, "updated": updated },
                "windows": {
                    "startRecording": case.start,
                    "stopRecording": case.stop,
                    "updated": updated,
                },
            }),
        )?;
        device.managementd.trigger_config_sync()?;

        let want = expected(case);
        let got = wait_for("portal settings to reach the device", SYNC_TIMEOUT, || {
            let got = device_case(&device)?;
            Ok((got == want).then_some(got))
        })?;
        println!("Device now has {got:?}");
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn device_settings_sync_to_portal() -> anyhow::Result<()> {
    let device = TestDevice::set_up()?;
    println!("Device {} in group {}", device.device_id, device.group);

    for case in &CASES {
        // Set the way Sidekick (config sections) and the web UI (audio form) do.
        device
            .managementd
            .set_config("thermal-recorder", json!({ "use-low-power-mode": case.low_power }))?;
        device.managementd.set_config(
            "windows",
            json!({ "start-recording": case.start, "stop-recording": case.stop }),
        )?;
        device.managementd.set_audio_recording(case.audio_mode)?;

        let got = wait_for("device settings to reach the API", SYNC_TIMEOUT, || {
            let settings = device.settings()?;
            let got = api_case(&settings);
            Ok((got == expected(case)).then_some(got))
        })?;
        println!("API now has {got:?}");
    }
    Ok(())
}

fn api_case(settings: &Value) -> (bool, String, String, String) {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    (
        settings["thermalRecording"]["useLowPowerMode"].as_bool().unwrap_or_default(),
        text(&settings["audioRecording"]["audioMode"]),
        text(&settings["windows"]["startRecording"]),
        text(&settings["windows"]["stopRecording"]),
    )
}
