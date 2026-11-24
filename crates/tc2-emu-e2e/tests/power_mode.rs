//! Pins down how the medium power mode behaves today. It isn't a setting of its own:
//! it's `use-low-power-mode` with `instant-classify`, both in the device's
//! `[thermal-recorder]` config.
//!
//!   low:    use-low-power-mode, no instant-classify
//!   medium: use-low-power-mode and instant-classify
//!   high:   no use-low-power-mode (instant-classify is meaningless)
//!
//! managementd (the web UI and Sidekick) keeps the two keys consistent when it writes
//! them, and shows Sidekick medium as high. cacophony-config-sync syncs both with the
//! API, as `useLowPowerMode` and `instantClassify`, but only managementd's checks keep
//! them consistent: a portal that only knows `useLowPowerMode` can leave a device with
//! low power off and instant classify on. Tests here describe behaviour that is arguably
//! a gap, and say so; if they fail after a deliberate change, update them.
//!
//! The sync tests need the API to accept `instantClassify`
//! (upstream-patches/cacophony-web/0002) and go-config with
//! guest/patches/go-config/0009 (built into the emulator by `make stubs`).
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use serde_json::{Value, json};
use std::time::Duration;
use tc2_emu_e2e::{LoggedInEntity, Managementd, TestDevice, now_iso, wait_for};

const SYNC_TIMEOUT: Duration = Duration::from_secs(20);

/// `(use-low-power-mode, instant-classify)`.
type Power = (bool, bool);
const HIGH: Power = (false, false);
const LOW: Power = (true, false);
const MEDIUM: Power = (true, true);

/// The device's `(use-low-power-mode, instant-classify)`, from a config as managementd
/// returns it.
fn power_in(config: &Value) -> Power {
    (
        config["thermalRecorder"]["UseLowPowerMode"].as_bool().unwrap_or_default(),
        config["thermalRecorder"]["InstantClassify"].as_bool().unwrap_or_default(),
    )
}

fn device_power(managementd: &Managementd) -> anyhow::Result<Power> {
    Ok(power_in(&managementd.config()?))
}

fn set_up() -> anyhow::Result<TestDevice> {
    let device = TestDevice::set_up()?;
    println!("Device {} in project {}", device.device_id, device.project);
    // Settings are inherited from the last test's device, so start from known ones:
    // no comms, which changes how Sidekick's power mode is handled.
    device.managementd.as_sidekick().set_config("comms", json!({ "enable": false }))?;
    Ok(device)
}

/// What the web UI's config page sends for a power mode: both keys.
fn power_mode(power: Power) -> Value {
    json!({ "use-low-power-mode": power.0, "instant-classify": power.1 })
}

/// Waits for the device to have `want`.
fn wait_for_power(device: &TestDevice, want: Power) -> anyhow::Result<()> {
    wait_for("the device's power mode", SYNC_TIMEOUT, || {
        Ok((device_power(&device.managementd)? == want).then_some(()))
    })
}

/// The API's thermal recording settings.
fn api_thermal(device: &TestDevice) -> anyhow::Result<Value> {
    Ok(device.settings()?["thermalRecording"].clone())
}

/// Waits for the API's power mode.
fn wait_for_api_power(device: &TestDevice, want: Power) -> anyhow::Result<()> {
    wait_for("the API's power mode", SYNC_TIMEOUT, || {
        let thermal = api_thermal(device)?;
        // A device that has never had medium power may not have sent instantClassify.
        let got = (
            thermal["useLowPowerMode"].as_bool().unwrap_or_default(),
            thermal["instantClassify"].as_bool().unwrap_or_default(),
        );
        Ok((got == want).then_some(()))
    })
    .map_err(|e| {
        let api = api_thermal(device).map(|t| t.to_string()).unwrap_or_default();
        anyhow::anyhow!("{e}: wanted (low power, instant classify) {want:?}, the API has {api}")
    })
}

/// The portal changing the power mode, as one that knows about medium power does.
fn portal_sets_power(device: &TestDevice, power: Power) -> anyhow::Result<()> {
    device.user.update_device_settings(
        device.device_id,
        json!({ "thermalRecording": {
            "useLowPowerMode": power.0,
            "instantClassify": power.1,
            "updated": now_iso(),
        } }),
    )?;
    device.managementd.trigger_config_sync()
}

/// The portal changing only useLowPowerMode, as the web portal does today.
fn portal_sets_low_power(device: &TestDevice, low_power: bool) -> anyhow::Result<()> {
    device.user.update_device_settings(
        device.device_id,
        json!({ "thermalRecording": { "useLowPowerMode": low_power, "updated": now_iso() } }),
    )?;
    device.managementd.trigger_config_sync()
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn web_ui_power_modes() -> anyhow::Result<()> {
    let device = set_up()?;
    let ui = &device.managementd;

    // The three modes, as the config page sets them.
    for mode in [HIGH, MEDIUM, LOW, MEDIUM, HIGH] {
        ui.set_config("thermal-recorder", power_mode(mode))?;
        assert_eq!(device_power(ui)?, mode, "setting {mode:?}");
    }

    // Instant classify needs low power mode, so turning it on turns that on too.
    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    ui.set_config("thermal-recorder", json!({ "instant-classify": true }))?;
    assert_eq!(device_power(ui)?, MEDIUM);

    // But the other way round isn't enforced: turning low power mode off alone leaves
    // instant classify on, which the agent treats as high power. (The config page
    // always sends both keys, so it doesn't do this.)
    ui.set_config("thermal-recorder", json!({ "use-low-power-mode": false }))?;
    assert_eq!(device_power(ui)?, (false, true));

    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn web_ui_comms_needs_medium_or_high_power() -> anyhow::Result<()> {
    let device = set_up()?;
    let ui = &device.managementd;

    // Trap comms can't work in low power mode (unless it's at-esl, below), so
    // enabling it from the web UI makes low power medium power.
    ui.set_config("thermal-recorder", power_mode(LOW))?;
    ui.set_config("comms", json!({ "enable": true, "comms-out": "uart" }))?;
    assert_eq!(device_power(ui)?, MEDIUM);

    // Already medium or high power is left alone.
    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    ui.set_config("comms", json!({ "enable": true, "comms-out": "uart" }))?;
    assert_eq!(device_power(ui)?, HIGH);

    // at-esl is the low power comms: it works in low power mode, so isn't changed.
    ui.set_config("comms", json!({ "enable": false }))?;
    ui.set_config("thermal-recorder", power_mode(LOW))?;
    ui.set_config("comms", json!({ "enable": true, "comms-out": "at-esl" }))?;
    assert_eq!(device_power(ui)?, LOW);

    // Nothing here checks that the device can reach a network, which is what
    // instant classify is for. Comms is only a proxy for that.
    ui.set_config("comms", json!({ "enable": false }))?;
    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn sidekick_power_modes() -> anyhow::Result<()> {
    let device = set_up()?;
    let ui = &device.managementd;
    let sidekick = ui.as_sidekick();

    // Sidekick only has low and high power. Without comms, its high power is the
    // device's medium power, and low power stays low.
    sidekick.set_config("thermal-recorder", json!({ "use-low-power-mode": false }))?;
    assert_eq!(device_power(ui)?, MEDIUM);
    sidekick.set_config("thermal-recorder", json!({ "use-low-power-mode": true }))?;
    assert_eq!(device_power(ui)?, LOW);
    sidekick.set_config("thermal-recorder", json!({ "use-low-power-mode": false }))?;
    assert_eq!(device_power(ui)?, MEDIUM);

    // And what it reads back is high power for medium, as it set it, though the
    // device's own config (and the web UI) say medium. Only use-low-power-mode is
    // changed in what it's shown: it still sees instant-classify on.
    assert_eq!(power_in(&sidekick.config()?), (false, true));
    assert_eq!(power_in(&ui.config()?), MEDIUM);

    // Low power reads back as low power.
    sidekick.set_config("thermal-recorder", json!({ "use-low-power-mode": true }))?;
    assert_eq!(power_in(&sidekick.config()?), LOW);

    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn device_power_modes_sync_to_portal() -> anyhow::Result<()> {
    let device = set_up()?;
    let ui = &device.managementd;

    // Start somewhere different from the first mode below.
    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    wait_for_api_power(&device, HIGH)?;

    for mode in [MEDIUM, LOW, MEDIUM, HIGH] {
        ui.set_config("thermal-recorder", power_mode(mode))?;
        wait_for_api_power(&device, mode)?;
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn portal_power_modes_sync_to_device() -> anyhow::Result<()> {
    let device = set_up()?;
    let ui = &device.managementd;
    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    wait_for_api_power(&device, HIGH)?;

    // A portal that knows about medium power sends both.
    for mode in [MEDIUM, LOW, MEDIUM, HIGH] {
        portal_sets_power(&device, mode)?;
        wait_for_power(&device, mode)?;
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn portal_without_instant_classify_leaves_it_alone() -> anyhow::Result<()> {
    let device = set_up()?;
    let ui = &device.managementd;

    // A portal that only knows useLowPowerMode turning it on doesn't make it medium,
    // or turn instant classify off.
    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    wait_for_api_power(&device, HIGH)?;
    portal_sets_low_power(&device, true)?;
    wait_for_power(&device, LOW)?;

    // Medium on the device, then such a portal says "high power". The device's
    // instant classify stays on, which the agent treats as high power, but the
    // config no longer says one mode: use-low-power-mode off with instant-classify
    // on. The synced write doesn't go through managementd's checks.
    ui.set_config("thermal-recorder", power_mode(MEDIUM))?;
    wait_for_api_power(&device, MEDIUM)?;
    portal_sets_low_power(&device, false)?;
    wait_for_power(&device, (false, true))?;

    // Then "low power" from the portal gives medium back.
    portal_sets_low_power(&device, true)?;
    wait_for_power(&device, MEDIUM)?;

    ui.set_config("thermal-recorder", power_mode(HIGH))?;
    Ok(())
}
