//! Medium power mode: with the Pi powered off, the RP2040 sees motion, starts recording to
//! its flash and wakes the Pi, then streams the recording to tc2-agent as it records it.
//! tc2-agent hands it to thermal-recorder-py (`mediumpower`), which classifies it while
//! it's still being received, so the device can act on what it saw as soon as it can.
//!
//! This plays the cat scene to an emulated device whose Pi has powered itself off, and checks
//! that the Pi comes back, the recording reaches it as a medium power stream (not the offload
//! a low power device does after the fact) and the classifier comes to a conclusion about it.
//! (It doesn't check what: the model calls this scene a dog, and whether it should is the
//! model's business.)
//!
//! Medium power is `use-low-power-mode` with `instant-classify` (see power_mode.rs), and it
//! only classifies when comms is on, which sets `run-classifier`. Like power_cycle, the Pi is
//! told to go off with tc2-hat-attiny's `--skip-wait`, as the 5 minutes' grace it otherwise
//! keeps the Pi on for would be most of this test.
//!
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use anyhow::bail;
use serde_json::json;
use std::time::Duration;
use tc2_emu_e2e::{Guest, Hardware, Scenes, TestDevice, wait_for};

/// `wait_for` while the Pi is on: its power going off ends the wait at once, since nothing
/// the test is waiting for can happen after that.
fn wait_while_pi_is_on<T>(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> anyhow::Result<Option<T>>,
) -> anyhow::Result<T> {
    let outcome = wait_for(what, timeout, || {
        if Hardware::pi_is_off().unwrap_or(false) {
            return Ok(Some(None));
        }
        Ok(check()?.map(Some))
    })?;
    outcome.ok_or_else(|| anyhow::anyhow!("the Pi powered off while waiting for {what}"))
}

const DROP_IN: &str = "/etc/systemd/system/tc2-hat-attiny.service.d/zz-tc2-e2e-skip-wait.conf";

#[test]
#[ignore = "needs the emulator and the Cacophony API; run with `make e2e`"]
fn a_scene_seen_with_the_pi_off_is_streamed_to_it_and_classified() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let lap = |what: &str| println!("[{:5.1}s] {what}", started.elapsed().as_secs_f32());
    let device = TestDevice::set_up()?;
    // What the Pi logs from here on, across its reboot, is this test's.
    let since = Guest::now()?;
    lap("device set up");

    let result = (|| -> anyhow::Result<()> {
        // Medium power, recording all day, classifying.
        device.managementd.set_config(
            "thermal-recorder",
            json!({ "use-low-power-mode": true, "instant-classify": true }),
        )?;
        device.managementd.set_config("comms", json!({ "enable": true, "comms-out": "uart" }))?;
        device.managementd.set_always_on()?;
        lap("medium power set");
        // tc2-agent resets the RP2040 to give it the new config, and it's started up with it
        // once the agent has its thermal startup info (its own record of getting the config
        // only reaches the agent at the restart after).
        wait_for("the RP2040 to restart with the config", Duration::from_secs(180), || {
            let log = Guest::journal_since("tc2-agent", since)?;
            let reset = log.rfind("Requesting reset of rp2040 at");
            let started = log.rfind("Got thermal startup info");
            Ok(Some(()).filter(|_| matches!((reset, started), (Some(reset), Some(started)) if started > reset)))
        })?;
        lap("RP2040 restarted with the config");

        // The Pi is told it may shut down once a minute, and tc2-hat-attiny would keep it up
        // for its 5 minutes' grace first. The drop-in that skips the grace removes itself as
        // it's started: it's on the guest's disk, so would otherwise skip it again when the
        // RP2040 wakes the Pi, and the Pi would power off while it was still classifying, as
        // nothing would be keeping it on yet (thermal-recorder's request to stay on can fail
        // early in the boot, before tc2-hat-attiny has its D-Bus name).
        Guest::run(&format!(
            "sudo mkdir -p $(dirname {DROP_IN}) && printf '%s\\n' '[Service]' 'ExecStart=' \
             'ExecStart=/opt/tc2-emu/guest/tc2-hat-controller attiny --skip-wait' \
             'ExecStartPre=-/usr/bin/rm -f {DROP_IN}' | sudo tee {DROP_IN} \
             && sudo systemctl daemon-reload && sudo systemctl restart tc2-hat-attiny"
        ))?;
        wait_for("the Pi to power off", Duration::from_secs(300), || {
            Ok(Some(()).filter(|_| Hardware::pi_is_off().unwrap_or(false)))
        })?;
        lap("Pi powered off");
        let state = Hardware::state()?;
        if state["rp2040_asleep"] == true {
            bail!("the RP2040 went to sleep as well as the Pi: {state}");
        }

        // The cat walks in. The RP2040 notices, starts recording and wakes the Pi.
        Scenes::play("cat-trigger")?;
        lap("playing the cat");
        wait_for("the RP2040 to wake the Pi", Duration::from_secs(120), || {
            Ok(Some(()).filter(|_| !Hardware::pi_is_off().unwrap_or(true)))
        })?;
        lap("Pi powered on");
        let boot = std::time::Instant::now();
        wait_for("the Pi to boot", Duration::from_secs(300), || {
            Ok(Guest::run("cat /proc/sys/kernel/random/boot_id").ok())
        })?;
        lap(&format!("Pi is up after {:.0}s", boot.elapsed().as_secs_f32()));

        wait_while_pi_is_on("the recording to be streamed to the Pi", Duration::from_secs(300), || {
            let log = Guest::journal_since("tc2-agent", since)?;
            Ok(Some(log).filter(|log| log.contains("Client finished streamed recording")))
        })
        .inspect_err(|_| dump(since))?;
        lap("streamed recording handed over");

        // The classifier works on it as it arrives, predicting again as more frames of the track
        // come in, which takes about a minute for the first on the emulated Pi (21s each). What
        // it makes of the cat scene is the model's business (it has called it a dog, 95% sure,
        // on the first prediction), so this checks only that it comes to a conclusion, and
        // reports every prediction it makes.
        let predictions = || -> anyhow::Result<Vec<String>> {
            let log = Guest::journal_since("thermal-recorder-py", since)?;
            Ok(log
                .lines()
                .filter_map(|line| line.split_once(" is predicted as ").map(|(_, rest)| rest.trim().to_string()))
                .collect())
        };
        wait_while_pi_is_on("the track to be classified", Duration::from_secs(300), || {
            Ok(Some(()).filter(|_| predictions().is_ok_and(|predictions| !predictions.is_empty())))
        })
        .inspect_err(|_| dump(since))?;
        lap("classified");
        // The classifier predicts again as more frames of the track come in. Waiting for it to
        // finish adds a minute or more to the test, so by default this takes the first;
        // TC2_E2E_ALL_PREDICTIONS=1 waits until it's had nothing more to say for a minute and
        // reports them all, for looking into what it makes of a scene.
        let mut seen = predictions()?;
        if std::env::var("TC2_E2E_ALL_PREDICTIONS").is_ok_and(|value| value == "1") {
            let mut last_new = std::time::Instant::now();
            while last_new.elapsed() < Duration::from_secs(60) && !Hardware::pi_is_off().unwrap_or(true) {
                std::thread::sleep(Duration::from_secs(5));
                if let Ok(now) = predictions() {
                    if now.len() != seen.len() {
                        seen = now;
                        last_new = std::time::Instant::now();
                    }
                }
            }
        }
        println!("The classifier made {} prediction(s):", seen.len());
        for prediction in &seen {
            println!("  {prediction}");
        }
        lap("predictions collected");

        // Whatever else is uploaded (thermal-recorder takes snapshots), the recording of the
        // cat is the one that's more than a couple of seconds.
        let recording = wait_while_pi_is_on("the recording to reach the API", Duration::from_secs(300), || {
            let recordings = device.user.thermal_recordings(device.project_id, device.device_id)?;
            Ok(recordings.into_iter().find(|r| r["duration"].as_f64().unwrap_or_default() > 4.0))
        })
        .inspect_err(|_| dump(since))?;
        lap("recording on the API");
        println!("Recording on the API: {recording:#}");
        Ok(())
    })();

    // Leave the device as the other tests expect to find it.
    let _ = device.managementd.set_config(
        "thermal-recorder",
        json!({ "use-low-power-mode": false, "instant-classify": false }),
    );
    let _ = device.managementd.set_config("comms", json!({ "enable": false, "comms-out": "uart" }));
    let _ = Guest::run(&format!("sudo rm -f {DROP_IN}; sudo systemctl daemon-reload; sudo systemctl restart tc2-hat-attiny"));
    result
}

/// What the Pi's services logged since `since`, for working out what a failure was.
fn dump(since: i64) {
    for unit in ["tc2-agent", "thermal-recorder-py"] {
        if let Ok(log) = Guest::journal_since(unit, since) {
            let tail: Vec<&str> = log.lines().rev().take(40).collect();
            println!("--- {unit} (last {} lines)\n{}", tail.len(), tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
    }
}
