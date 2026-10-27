//! Initial setup of a new device by its user: the device registers itself into
//! the `new` group, the user moves and renames it into their own group through
//! Sidekick, then takes a test recording and finds it on the server.
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use chrono::{DateTime, Utc};
use std::time::Duration;
use tc2_emu_e2e::{Managementd, TEST_RECORDING_READY, User, test_name, wait_for};

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn new_device_setup() -> anyhow::Result<()> {
    let device = Managementd::new();
    let user = User::create()?;

    // A fresh API database may not have the `new` group yet. device-register
    // keeps retrying until it can register, so create it now if absent.
    if user.ensure_group("new")? {
        println!("Created the new group");
    }

    // The emulated device registers itself into `new` on first boot, then reboots.
    let registered = wait_for("device to register", Duration::from_secs(300), || {
        Ok(device.device_info().ok().filter(|info| info.device_id != 0))
    })?;
    assert_eq!(registered.groupname, "new");
    println!("Device registered as {registered:?}");

    let group = test_name("group");
    let group_id = user.create_group(&group)?;
    println!("Created user {} and group {group} ({group_id})", user.name);

    // Sidekick moves the device into the user's group and renames it.
    let name = test_name("device");
    device.reregister_authorized(&user, &group, &name)?;
    let moved = device.device_info()?;
    assert_eq!(
        (moved.groupname.as_str(), moved.devicename.as_str()),
        (group.as_str(), name.as_str())
    );
    assert_ne!(moved.device_id, registered.device_id, "reregistering makes a new device");
    let api_devices = user.group_devices(&group)?;
    assert!(
        api_devices.iter().any(|d| d.id == moved.device_id && d.device_name == name),
        "device {name} not in {group}: {api_devices:?}"
    );
    println!("Moved device to {group} as {name} ({})", moved.device_id);

    // The user takes a test recording from the web UI or Sidekick.
    // tc2-agent may still be starting after the device's post-registration reboot.
    let requested_at = Utc::now();
    let reply = wait_for("tc2-agent to take a test recording", Duration::from_secs(60), || {
        device.take_short_test_recording().map(Some)
    })?;
    assert_eq!(reply, "Asked for a test recording");
    wait_for("test recording to finish", Duration::from_secs(60), || {
        let status = device.thermal_status()?;
        Ok((status.status == TEST_RECORDING_READY).then_some(()))
    })?;

    let recording = wait_for("recording to reach the API", Duration::from_secs(120), || {
        Ok(user.thermal_recordings(group_id, moved.device_id)?.into_iter().next())
    })?;
    println!("Recording on the API: {recording:#}");
    let recorded_at: DateTime<Utc> =
        recording["recordingDateTime"].as_str().expect("recordingDateTime").parse()?;
    assert!(
        recorded_at >= requested_at - chrono::TimeDelta::seconds(5),
        "recording from {recorded_at} predates the request at {requested_at}"
    );
    Ok(())
}
