//! cacophony-config-sync keeps the power mode, recording window, audio recording,
//! battery and trap settings the same on the device and the API, where the web
//! portal edits them. Whichever side changed a section last wins.
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.
//! The trap tests need the API with species confidences in its trap settings
//! (upstream-patches/cacophony-web).
//!
//! The emulator runs cacophony-config-sync with fixes from guest/patches/go-config,
//! and managementd built against them. Without them: after Sidekick reregisters
//! the device it syncs as the old device until it restarts, so these tests time
//! out; battery settings from the portal have no effect, and "Auto-detect" never
//! reaches the device; invalid settings from the portal are written to the
//! device's config, or stop the sync; and trap settings aren't synced at all.

use serde_json::{Value, json};
use std::thread::sleep;
use std::time::Duration;
use tc2_emu_e2e::{LoggedInEntity, TestDevice, now_iso, wait_for};

/// A sync takes under 2s on the emulated device, and a change can take two. The
/// sync waits after the config file changes first, only briefly under `make e2e`
/// (TC2_CONFIG_SYNC_DELAY), but 10s otherwise.
const SYNC_TIMEOUT: Duration = Duration::from_secs(20);

struct Case {
    low_power: bool,
    /// `AudioRecordingMode`: AudioOrThermal, AudioAndThermal, AudioOnly or Disabled.
    audio_mode: &'static str,
    audio_seed: u32,
    /// A time like "22:00", or relative to sunset/sunrise like "-30m".
    start: &'static str,
    stop: &'static str,
}

const CASES: [Case; 4] = [
    // Custom recording window, low power.
    Case {
        low_power: true,
        audio_mode: "AudioOrThermal",
        audio_seed: 7,
        start: "22:00",
        stop: "06:00",
    },
    // The portal's "always": the same start and stop.
    Case {
        low_power: false,
        audio_mode: "AudioAndThermal",
        audio_seed: 0,
        start: "12:00",
        stop: "12:00",
    },
    // The portal's default window, relative to sunset and sunrise.
    Case {
        low_power: true,
        audio_mode: "AudioOnly",
        audio_seed: u32::MAX,
        start: "-30m",
        stop: "+30m",
    },
    Case { low_power: false, audio_mode: "Disabled", audio_seed: 0, start: "09:00", stop: "17:00" },
];

type CaseSettings = (bool, String, u32, String, String);

/// The device's version of the settings in `Case`, from managementd.
fn device_case(device: &TestDevice) -> anyhow::Result<CaseSettings> {
    let config = device.managementd.config()?;
    Ok((
        config["thermalRecorder"]["UseLowPowerMode"].as_bool().unwrap_or_default(),
        config["audioRecording"]["AudioMode"].as_str().unwrap_or_default().into(),
        config["audioRecording"]["AudioSeed"].as_u64().unwrap_or_default() as u32,
        config["windows"]["StartRecording"].as_str().unwrap_or_default().into(),
        config["windows"]["StopRecording"].as_str().unwrap_or_default().into(),
    ))
}

/// The API's version of the settings in `Case`. A seed that was never set is 0,
/// as the portal shows it.
fn api_case(settings: &Value) -> CaseSettings {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    (
        settings["thermalRecording"]["useLowPowerMode"].as_bool().unwrap_or_default(),
        text(&settings["audioRecording"]["audioMode"]),
        settings["audioRecording"]["audioSeed"].as_u64().unwrap_or_default() as u32,
        text(&settings["windows"]["startRecording"]),
        text(&settings["windows"]["stopRecording"]),
    )
}

fn expected(case: &Case) -> CaseSettings {
    (case.low_power, case.audio_mode.into(), case.audio_seed, case.start.into(), case.stop.into())
}

/// Waits for the device's settings to be `want`.
fn wait_for_device<T: PartialEq + std::fmt::Debug>(
    what: &str,
    want: &T,
    get: impl Fn() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    wait_for(what, SYNC_TIMEOUT, || {
        let got = get()?;
        Ok((&got == want).then_some(got))
    })
}

fn set_up() -> anyhow::Result<TestDevice> {
    let device = TestDevice::set_up()?;
    println!("Device {} in project {}", device.device_id, device.project);
    Ok(device)
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn portal_settings_sync_to_device() -> anyhow::Result<()> {
    let device = set_up()?;

    for case in &CASES {
        // The web portal sends every section it edited, each stamped as updated now.
        let updated = now_iso();
        device.user.update_device_settings(
            device.device_id,
            json!({
                "thermalRecording": { "useLowPowerMode": case.low_power, "updated": updated },
                "audioRecording": {
                    "audioMode": case.audio_mode,
                    "audioSeed": case.audio_seed,
                    "updated": updated,
                },
                "windows": {
                    "startRecording": case.start,
                    "stopRecording": case.stop,
                    "updated": updated,
                },
            }),
        )?;
        device.managementd.trigger_config_sync()?;

        let got = wait_for_device("portal settings to reach the device", &expected(case), || {
            device_case(&device)
        })?;
        println!("Device now has {got:?}");
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn device_settings_sync_to_portal() -> anyhow::Result<()> {
    let device = set_up()?;

    for case in &CASES {
        // Set the way Sidekick (config sections) and the web UI (audio form) do.
        device
            .managementd
            .set_config("thermal-recorder", json!({ "use-low-power-mode": case.low_power }))?;
        device.managementd.set_config(
            "windows",
            json!({ "start-recording": case.start, "stop-recording": case.stop }),
        )?;
        device.managementd.set_audio_recording(case.audio_mode, case.audio_seed)?;

        let got = wait_for_device("device settings to reach the API", &expected(case), || {
            Ok(api_case(&device.settings()?))
        })?;
        println!("API now has {got:?}");
    }
    Ok(())
}

/// Battery settings in the device's config.
#[derive(Debug, Clone, PartialEq)]
struct BatteryConfig {
    chemistry: String,
    cell_count: u32,
    manually_configured: bool,
}

fn battery(chemistry: &str, cell_count: u32) -> BatteryConfig {
    BatteryConfig {
        chemistry: chemistry.into(),
        cell_count,
        manually_configured: !chemistry.is_empty() || cell_count != 0,
    }
}

/// The device's battery settings, from its config. managementd's battery page
/// doesn't reread the config, so shows them as they were when it last did.
fn device_battery(device: &TestDevice) -> anyhow::Result<BatteryConfig> {
    let config = device.managementd.config()?;
    let battery = &config["battery"];
    Ok(BatteryConfig {
        chemistry: battery["Chemistry"].as_str().unwrap_or_default().into(),
        cell_count: battery["ManualCellCount"].as_u64().unwrap_or_default() as u32,
        manually_configured: battery["ManuallyConfigured"].as_bool().unwrap_or_default(),
    })
}

/// The portal's battery section, leaving out what it sets to "Auto-detect".
fn portal_battery(chemistry: &str, cell_count: u32) -> Value {
    let mut section = json!({ "updated": now_iso() });
    if !chemistry.is_empty() {
        section["chemistry"] = chemistry.into();
    }
    if cell_count != 0 {
        section["manualCellCount"] = cell_count.into();
    }
    section
}

/// The API's battery chemistry and cell count, empty and 0 when auto-detected.
fn api_battery(settings: &Value) -> (String, u32) {
    let battery = &settings["battery"];
    (
        battery["chemistry"].as_str().unwrap_or_default().into(),
        battery["manualCellCount"].as_u64().unwrap_or_default() as u32,
    )
}

/// Chemistry and cell count, each set or auto-detected, ending auto-detected.
const BATTERY_CASES: [(&str, u32); 4] = [("lipo", 3), ("", 0), ("li-ion", 0), ("", 0)];

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn portal_battery_syncs_to_device() -> anyhow::Result<()> {
    let device = set_up()?;

    for (chemistry, cell_count) in BATTERY_CASES {
        device.user.update_device_settings(
            device.device_id,
            json!({ "battery": portal_battery(chemistry, cell_count) }),
        )?;
        device.managementd.trigger_config_sync()?;

        let got = wait_for_device(
            "portal battery settings to reach the device",
            &battery(chemistry, cell_count),
            || device_battery(&device),
        )?;
        println!("Device now has {got:?}");
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn device_battery_syncs_to_portal() -> anyhow::Result<()> {
    let device = set_up()?;

    for (chemistry, cell_count) in BATTERY_CASES {
        if chemistry.is_empty() && cell_count == 0 {
            device.managementd.clear_battery_config()?;
        } else {
            device.managementd.set_battery_config(chemistry, cell_count)?;
        }

        let want = (chemistry.to_string(), cell_count);
        let got = wait_for_device("device battery settings to reach the API", &want, || {
            Ok(api_battery(&device.settings()?))
        })?;
        println!("API now has {got:?}");
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn invalid_portal_settings_are_ignored() -> anyhow::Result<()> {
    let device = set_up()?;
    device.managementd.set_config("thermal-recorder", json!({ "use-low-power-mode": false }))?;
    device
        .managementd
        .set_config("windows", json!({ "start-recording": "09:00", "stop-recording": "17:00" }))?;
    device.managementd.set_audio_recording("AudioOnly", 7)?;
    device.managementd.set_battery_config("lipo", 3)?;
    let low_power = |device: &TestDevice| -> anyhow::Result<bool> {
        Ok(device.managementd.config()?["thermalRecorder"]["UseLowPowerMode"].as_bool()
            == Some(true))
    };
    let before = (device_case(&device)?, device_battery(&device)?);

    // The API accepts any window text, seed number and chemistry text. The
    // power mode is valid, so is still applied.
    let updated = now_iso();
    device.user.update_device_settings(
        device.device_id,
        json!({
            "thermalRecording": { "useLowPowerMode": true, "updated": updated },
            "windows": { "startRecording": "soon", "stopRecording": "17:00", "updated": updated },
            "audioRecording": { "audioMode": "Disabled", "audioSeed": -1, "updated": updated },
            "battery": { "chemistry": "unobtainium", "updated": updated },
        }),
    )?;
    device.managementd.trigger_config_sync()?;
    wait_for_device("the valid setting to reach the device", &true, || low_power(&device))?;

    let mut want = before.clone();
    want.0.0 = true;
    assert_eq!((device_case(&device)?, device_battery(&device)?), want);

    // Valid changes to the ignored sections still get through. This also leaves
    // valid settings on the API, which the device takes with it when it's next
    // reregistered.
    let updated = now_iso();
    device.user.update_device_settings(
        device.device_id,
        json!({
            "windows": { "startRecording": "08:00", "stopRecording": "18:00", "updated": updated },
            "audioRecording": { "audioMode": "AudioAndThermal", "audioSeed": 3, "updated": updated },
            "battery": portal_battery("lifepo4", 4),
        }),
    )?;
    device.managementd.trigger_config_sync()?;
    let want = (
        (true, "AudioAndThermal".into(), 3, "08:00".into(), "18:00".into()),
        battery("lifepo4", 4),
    );
    wait_for_device("valid settings to reach the device", &want, || {
        Ok((device_case(&device)?, device_battery(&device)?))
    })?;
    Ok(())
}

fn api_window(settings: &Value) -> (String, String) {
    let windows = &settings["windows"];
    (
        windows["startRecording"].as_str().unwrap_or_default().into(),
        windows["stopRecording"].as_str().unwrap_or_default().into(),
    )
}

fn device_window(device: &TestDevice) -> anyhow::Result<(String, String)> {
    let case = device_case(device)?;
    Ok((case.3, case.4))
}

fn window(start: &str, stop: &str) -> (String, String) {
    (start.into(), stop.into())
}

/// Leaves time between a change on one side and the other, so that which is newer
/// doesn't depend on how closely the device's clock and the test's agree.
fn next_second() {
    sleep(Duration::from_secs(2));
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn newer_portal_change_wins() -> anyhow::Result<()> {
    let device = set_up()?;

    // Changed on the device, then on the portal before they sync.
    device
        .managementd
        .set_config("windows", json!({ "start-recording": "10:00", "stop-recording": "11:00" }))?;
    next_second();
    device.user.update_device_settings(
        device.device_id,
        json!({ "windows": { "startRecording": "20:00", "stopRecording": "21:00", "updated": now_iso() } }),
    )?;
    device.managementd.trigger_config_sync()?;

    let want = window("20:00", "21:00");
    wait_for_device("the portal's window on the device", &want, || device_window(&device))?;
    wait_for_device("the portal's window to stay on the API", &want, || {
        Ok(api_window(&device.settings()?))
    })?;
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn newer_device_change_wins() -> anyhow::Result<()> {
    let device = set_up()?;

    // Changed on the portal, then on the device before they sync.
    device.user.update_device_settings(
        device.device_id,
        json!({ "windows": { "startRecording": "20:00", "stopRecording": "21:00", "updated": now_iso() } }),
    )?;
    next_second();
    device
        .managementd
        .set_config("windows", json!({ "start-recording": "10:00", "stop-recording": "11:00" }))?;

    let want = window("10:00", "11:00");
    wait_for_device("the device's window on the API", &want, || {
        Ok(api_window(&device.settings()?))
    })?;
    // Another sync, which mustn't bring the portal's older window back. A change to
    // another section on the portal shows when it has run: the sync takes it after
    // deciding about the window.
    let seed = rand::random::<u32>();
    device.user.update_device_settings(
        device.device_id,
        json!({ "audioRecording": { "audioMode": "AudioOnly", "audioSeed": seed, "updated": now_iso() } }),
    )?;
    device.managementd.trigger_config_sync()?;
    wait_for_device("the next sync to run", &seed, || Ok(device_case(&device)?.2))?;
    assert_eq!(device_window(&device)?, want);
    assert_eq!(api_window(&device.settings()?), want);
    Ok(())
}

/// Trap settings: enabled, the default state, then the species to trap and to
/// protect, each a map of species to the confidence needed. "armed" triggers on
/// any motion unless a species to protect was seen recently; "safe" triggers only
/// while a species to trap was seen recently, and none to protect.
type Trap = (bool, String, Value, Value);

fn trap(enabled: bool, default_state: &str, target: Value, protect: Value) -> Trap {
    (enabled, default_state.into(), target, protect)
}

/// Species with their confidences, with none as an empty map.
fn species(value: &Value) -> Value {
    if value.is_null() { json!({}) } else { value.clone() }
}

/// The device's trap settings, from its config.
fn device_trap(device: &TestDevice) -> anyhow::Result<Trap> {
    let config = device.managementd.config()?;
    let trap = &config["trap"];
    Ok((
        trap["Enabled"].as_bool().unwrap_or_default(),
        trap["DefaultState"].as_str().unwrap_or_default().into(),
        species(&trap["TargetSpecies"]),
        species(&trap["ProtectSpecies"]),
    ))
}

/// The settings the device sends the trap, which aren't synced.
fn device_trap_config(device: &TestDevice) -> anyhow::Result<Value> {
    Ok(device.managementd.config()?["trap"]["Config"].clone())
}

fn api_trap(settings: &Value) -> Trap {
    let trap = &settings["trap"];
    (
        trap["enabled"].as_bool().unwrap_or_default(),
        trap["defaultState"].as_str().unwrap_or_default().into(),
        species(&trap["target"]),
        species(&trap["protect"]),
    )
}

/// The portal's trap section. It sends the whole section, with each species'
/// tuned confidence.
fn portal_trap((enabled, default_state, target, protect): &Trap) -> Value {
    json!({
        "enabled": enabled,
        "defaultState": default_state,
        "target": target,
        "protect": protect,
        "updated": now_iso(),
    })
}

/// Enabled with species to trap and protect, armed then safe, with a species
/// removed, then disabled.
fn trap_cases() -> [Trap; 4] {
    [
        trap(true, "armed", json!({ "possum": 80, "cat": 80 }), json!({ "kiwi": 80 })),
        trap(true, "safe", json!({ "possum": 80, "cat": 80 }), json!({ "kiwi": 80 })),
        trap(true, "safe", json!({ "possum": 80 }), json!({})),
        trap(false, "armed", json!({}), json!({})),
    ]
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn portal_trap_syncs_to_device() -> anyhow::Result<()> {
    let device = set_up()?;
    // The trap's own settings, which the device sends it as they are.
    let trap_config = json!({ "cage_doors": 2 });
    device.managementd.set_config("trap", json!({ "config": trap_config }))?;
    next_second();

    for case in trap_cases() {
        device
            .user
            .update_device_settings(device.device_id, json!({ "trap": portal_trap(&case) }))?;
        device.managementd.trigger_config_sync()?;

        let got = wait_for_device("portal trap settings to reach the device", &case, || {
            device_trap(&device)
        })?;
        println!("Device now has {got:?}");
        assert_eq!(device_trap_config(&device)?, trap_config);
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn device_trap_syncs_to_portal() -> anyhow::Result<()> {
    let device = set_up()?;

    for case in trap_cases() {
        // Set the way Sidekick sets config sections.
        let (enabled, default_state, target, protect) = &case;
        device.managementd.set_config(
            "trap",
            json!({
                "enabled": enabled,
                "default-state": default_state,
                "target-species": target,
                "protect-species": protect,
            }),
        )?;

        let got = wait_for_device("device trap settings to reach the API", &case, || {
            Ok(api_trap(&device.settings()?))
        })?;
        println!("API now has {got:?}");
    }
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn invalid_portal_trap_settings_are_ignored() -> anyhow::Result<()> {
    let device = set_up()?;
    device.managementd.set_config("thermal-recorder", json!({ "use-low-power-mode": false }))?;
    let before = trap(true, "safe", json!({ "possum": 80 }), json!({ "kiwi": 80 }));
    device
        .user
        .update_device_settings(device.device_id, json!({ "trap": portal_trap(&before) }))?;
    device.managementd.trigger_config_sync()?;
    wait_for_device("the trap settings to reach the device", &before, || device_trap(&device))?;

    // The API takes any whole number as a confidence. The power mode is valid, so
    // is still applied.
    let updated = now_iso();
    let mut invalid = portal_trap(&trap(false, "armed", json!({ "possum": 101 }), json!({})));
    invalid["updated"] = updated.clone().into();
    device.user.update_device_settings(
        device.device_id,
        json!({
            "thermalRecording": { "useLowPowerMode": true, "updated": updated },
            "trap": invalid,
        }),
    )?;
    device.managementd.trigger_config_sync()?;
    wait_for_device("the valid setting to reach the device", &true, || {
        Ok(device.managementd.config()?["thermalRecorder"]["UseLowPowerMode"].as_bool()
            == Some(true))
    })?;
    assert_eq!(device_trap(&device)?, before);
    Ok(())
}

#[test]
#[ignore = "needs a running emulator and API; run with `make e2e`"]
fn legacy_sidekick_trap_settings() -> anyhow::Result<()> {
    let device = set_up()?;
    // Whatever earlier tests left it as, on the same device.
    let (enabled, ..) = device_trap(&device)?;

    // Sidekick's AI control page, which still sets the trap settings in comms. With
    // comms disabled, as the emulator has no trap for tc2-hat-comms to talk to.
    device.managementd.as_sidekick().set_config(
        "comms",
        json!({
            "enable": false,
            "comms-out": "uart",
            "trap-enabled-by-default": false,
            "trap-species": { "possum": 80 },
            "trap-duration": "2m0s",
            "protect-species": null,
            "protect-duration": "5m0s",
        }),
    )?;

    // They're moved to the trap section, which is enabled or not as before, as that's
    // left to the portal.
    let want = trap(enabled, "safe", json!({ "possum": 80 }), json!({}));
    assert_eq!(device_trap(&device)?, want);
    let config = device.managementd.as_sidekick().config()?;
    assert_eq!(config["trap"]["TrapDuration"], json!(120_000_000_000u64));
    assert_eq!(config["trap"]["ProtectDuration"], json!(300_000_000_000u64));

    // Sidekick reads them back from comms, with durations in nanoseconds.
    let comms = &config["comms"];
    assert_eq!(comms["Enable"], json!(false));
    assert_eq!(comms["CommsOut"], json!("uart"));
    assert_eq!(comms["TrapEnabledByDefault"], json!(false));
    assert_eq!(comms["TrapSpecies"], json!({ "possum": 80 }));
    assert_eq!(comms["TrapDuration"], json!(120_000_000_000u64));
    assert_eq!(species(&comms["ProtectSpecies"]), json!({}));
    assert_eq!(comms["ProtectDuration"], json!(300_000_000_000u64));

    // And they reach the portal like any change on the device.
    wait_for_device("Sidekick's trap settings to reach the API", &want, || {
        Ok(api_trap(&device.settings()?))
    })?;
    Ok(())
}
