//! Initial setup of a new device by its user: the device registers itself into
//! the `new` project, the user moves and renames it into their own project through
//! Sidekick, then takes a test recording as Sidekick does and finds it on the
//! server. The device is in high power mode, so with the real tc2-agent and
//! tc2-firmware (the default, not `TC2_AGENT=stand-in`) thermal-recorder makes the
//! recording from the frames the RP2040 streams, which this checks.
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};
use tc2_emu_e2e::{Guest, Managementd, User, test_name, wait_for};

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn new_device_setup() -> anyhow::Result<()> {
    let start = Instant::now();
    let lap = |what: &str| println!("[{:6.1}s] {what}", start.elapsed().as_secs_f32());
    let device = Managementd::new();
    Guest::connect_in_background();
    let user = User::create()?;
    lap("user created");

    // A fresh API database may not have the `new` project yet. device-register
    // keeps retrying until it can register, so create it now if absent.
    if user.ensure_project_exists("new")? {
        println!("Created the new project");
    }

    // The emulated device registers itself into `new` on first boot.
    let registered = wait_for("device to register", Duration::from_secs(300), || {
        Ok(device.device_info().ok().filter(|info| info.device_id != 0))
    })?;
    assert_eq!(registered.projectname, "new");
    lap("device registered");
    println!("Device registered as {registered:?}");

    let project = test_name("project");
    let project_id = user.create_project(&project)?.project_id;
    lap("project created");
    println!("Created user {} and project {project} ({project_id})", user.name);

    // Sidekick moves the device into the user's project and renames it.
    let name = test_name("device");
    device.reregister_authorized(&user, &project, &name)?;
    let moved = device.device_info()?;
    assert_eq!(
        (moved.projectname.as_str(), moved.devicename.as_str()),
        (project.as_str(), name.as_str())
    );
    // A new name in a new project, so there's no existing device to take over.
    assert_ne!(moved.device_id, registered.device_id, "reregistering makes a new device");
    let api_devices = user.project_devices(&project)?;
    assert!(
        api_devices.iter().any(|d| d.id == moved.device_id && d.device_name == name),
        "device {name} not in {project}: {api_devices:?}"
    );
    lap("device moved");
    println!("Moved device to {project} as {name} ({})", moved.device_id);

    // The user has the camera record all day, so that it's inside its recording
    // window whenever the test runs: outside it, the real tc2-firmware would have
    // the Pi power off.
    device.set_always_on()?;

    // The user takes a test recording from Sidekick. Not straight away, as the
    // emulated device saves it at once: the guest's clock, set from the RTC at boot,
    // can be up to a second behind the API's, and the API files a recording under
    // the device registered at its time, so one made straight after reregistering
    // would go to the old device.
    lap("set always on");
    std::thread::sleep(Duration::from_millis(1500));
    lap("slept");
    let real_agent = Guest::runs_real_agent()?;
    let since = Guest::now()?;
    lap("guest checked");
    let requested_at = Utc::now();
    // managementd may still be starting after the device registers.
    wait_for("managementd to take a test recording", Duration::from_secs(60), || {
        device.take_snapshot_recording().map(Some)
    })?;

    lap("test recording requested");
    let recording = wait_for("recording to reach the API", Duration::from_secs(120), || {
        Ok(user.thermal_recordings(project_id, moved.device_id)?.into_iter().next())
    })?;
    lap("recording on the API");
    println!("Recording on the API: {recording:#}");
    let recorded_at: DateTime<Utc> =
        recording["recordingDateTime"].as_str().expect("recordingDateTime").parse()?;
    assert!(
        recorded_at >= requested_at - chrono::TimeDelta::seconds(5),
        "recording from {recorded_at} predates the request at {requested_at}"
    );

    if real_agent {
        // thermal-recorder made it, from streamed frames, rather than the RP2040.
        let recorder = Guest::journal_since("thermal-recorder-py", since)?;
        assert!(
            recorder.contains("Taking new snapshot recorder"),
            "thermal-recorder didn't take the recording:\n{recorder}"
        );
        let agent = Guest::journal_since("tc2-agent", since)?;
        assert!(
            !agent.contains("Request short test thermal recording"),
            "tc2-agent asked the RP2040 for the recording:\n{agent}"
        );
    }
    lap("checks done");
    Ok(())
}
