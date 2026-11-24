//! A test recording made by the RP2040 itself, as the web UI's low power thermal
//! recording page takes one: tc2-agent asks the RP2040 for it, the RP2040 records
//! it to its flash and offloads the file to the Pi, and thermal-uploader uploads it.
//! That happens whatever the power mode, so this also covers it in high power mode,
//! where test recordings from Sidekick are made by thermal-recorder instead (see
//! `new_device_setup`). With the real tc2-agent and tc2-firmware (the default, not
//! `TC2_AGENT=stand-in`) this checks that the RP2040 made it.
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use chrono::{DateTime, Utc};
use std::time::Duration;
use tc2_emu_e2e::{Guest, TEST_RECORDING_READY, TestDevice, wait_for};

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn rp2040_test_recording() -> anyhow::Result<()> {
    let device = TestDevice::set_up()?;
    let managementd = &device.managementd;
    // So that the Pi stays up with the real tc2-firmware; see `set_always_on`.
    managementd.set_always_on()?;
    // The API files a recording under the device registered at the guest's time,
    // which can be up to a second behind the API's (see `new_device_setup`).
    std::thread::sleep(Duration::from_millis(1500));

    let real_agent = Guest::runs_real_agent()?;
    let since = Guest::now()?;
    let requested_at = Utc::now();
    let reply = wait_for("tc2-agent to take a test recording", Duration::from_secs(60), || {
        managementd.take_short_test_recording().map(Some)
    })?;
    assert_eq!(reply, "Asked for a test recording");
    wait_for("test recording to finish", Duration::from_secs(60), || {
        let status = managementd.thermal_status()?;
        Ok((status.status == TEST_RECORDING_READY).then_some(()))
    })?;

    let recording = wait_for("recording to reach the API", Duration::from_secs(120), || {
        Ok(device.user.thermal_recordings(device.project_id, device.device_id)?.into_iter().next())
    })?;
    println!("Recording on the API: {recording:#}");
    let recorded_at: DateTime<Utc> =
        recording["recordingDateTime"].as_str().expect("recordingDateTime").parse()?;
    assert!(
        recorded_at >= requested_at - chrono::TimeDelta::seconds(5),
        "recording from {recorded_at} predates the request at {requested_at}"
    );

    if real_agent {
        // The RP2040 made it, rather than thermal-recorder.
        let agent = Guest::journal_since("tc2-agent", since)?;
        assert!(
            agent.contains("Telling rp2040 to take test recording"),
            "tc2-agent didn't ask the RP2040 for the recording:\n{agent}"
        );
        // Not thermal-recorder, which also takes a snapshot whenever it restarts (as
        // the config changes make it) and so can't be told apart by its log. Its
        // recordings have a preview; the RP2040's don't.
        assert!(
            recording["additionalMetadata"].get("previewSecs").is_none(),
            "thermal-recorder made the recording, not the RP2040: {recording:#}"
        );
    }
    Ok(())
}
