//! What the emulated Lepton sees decides what the device records: a held frame is no
//! motion, and a scene with a cat in it is. The device is in high power mode with an
//! always-on window, so thermal-recorder on the Pi finds the motion in the frames the
//! firmware streams and makes the recording, as on a camera in the field.
//!
//! The held frame is the first of the emulator's fixture, and scenes are the CPTV files
//! in the fixtures directory. Moving between them cross-fades, so switching is not itself
//! motion (see tc2-firmware's src/emulator/lepton.rs).
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use std::time::Duration;
use tc2_emu_e2e::{Guest, Scenes, TestDevice, wait_for};

/// The recordings thermal-uploader has uploaded since `since` (from `Guest::now`), other
/// than snapshots: thermal-recorder takes those itself around starting up, and they are
/// not motion.
fn motion_uploads(since: i64) -> anyhow::Result<Vec<String>> {
    Ok(Guest::journal_since("thermal-uploader", since)?
        .lines()
        .filter(|line| line.contains("uploading: ") && !line.contains("-snap.cptv"))
        .map(str::to_string)
        .collect())
}

#[test]
#[ignore = "needs the emulator and the Cacophony API; run with `make e2e`"]
fn a_scene_with_a_cat_is_recorded_and_a_held_frame_is_not() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let lap = |what: &str| println!("[{:5.1}s] {what}", started.elapsed().as_secs_f32());
    let device = TestDevice::set_up()?;
    device.managementd.set_always_on()?;
    lap("device set up, always on");
    assert_eq!(Scenes::status()?, "hold");
    let since = Guest::now()?;

    // Long enough for thermal-recorder to restart for the config change and run with
    // frames, in which a held frame must not look like anything.
    std::thread::sleep(Duration::from_secs(45));
    let uploads = motion_uploads(since)?;
    assert!(uploads.is_empty(), "recorded from a held frame: {uploads:#?}");
    lap("nothing recorded from the held frame");
    let snapshots = device.user.thermal_recordings(device.project_id, device.device_id)?.len();

    Scenes::play("cat-trigger")?;
    lap("playing the cat");
    let recording = wait_for("the cat to be recorded", Duration::from_secs(240), || {
        let uploads = motion_uploads(since)?;
        let recordings = device.user.thermal_recordings(device.project_id, device.device_id)?;
        Ok(Some(recordings).filter(|recordings| !uploads.is_empty() && recordings.len() > snapshots))
    })?;
    lap("recorded");
    println!("Recordings on the API: {recording:#?}");

    // The scene ends by itself and the Lepton goes back to the held frame.
    wait_for("the scene to end", Duration::from_secs(120), || {
        Ok(Some(()).filter(|_| Scenes::status().is_ok_and(|status| status == "hold")))
    })?;
    lap("back to the held frame");
    Ok(())
}
