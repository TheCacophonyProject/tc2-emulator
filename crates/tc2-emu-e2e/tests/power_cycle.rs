//! A device outside its recording window powers itself off and wakes at the next one:
//! the firmware advises the Pi to shut down, tc2-hat-attiny does, the ATtiny cuts its
//! power, the firmware sleeps until its RTC alarm, and at the alarm wakes the Pi.
//!
//! With time warp the RTC skips the hours the Pi and RP2040 are both off, so the test
//! doesn't wait for them. What it does wait for is tc2-hat-attiny, which keeps the Pi
//! on for 5 minutes after it starts, and for 5 minutes after each request managementd
//! sees. The test restarts it with `--skip-wait`, which drops both: they're in its
//! memory, not the device's state.
//!
//! Run it last: the device's clock is hours ahead after the jump.
//! Needs a freshly started emulator and the Cacophony API; run with `make e2e`.

use anyhow::bail;
use serde_json::json;
use std::time::Duration;
use tc2_emu_e2e::{Guest, Hardware, TestDevice, wait_for};

const DROP_IN: &str = "/etc/systemd/system/tc2-hat-attiny.service.d/zz-tc2-e2e-skip-wait.conf";

#[test]
#[ignore = "needs the emulator and the Cacophony API; run with `make e2e`"]
fn device_outside_its_window_powers_off_and_wakes_at_the_next_one() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let lap = |what: &str| println!("[{:5.1}s] {what}", started.elapsed().as_secs_f32());
    let device = TestDevice::set_up()?;
    lap("device set up");
    Hardware::set_time_warp(true)?;

    // A window that starts in six hours, by the device's own clock and time zone.
    let since = Guest::now()?;
    // An earlier test may have left the device audio only, which the firmware shuts down
    // for even with the Pi on, and which wakes the Pi for audio recordings.
    device.managementd.set_audio_recording("Disabled", 0)?;
    let hour: u32 = Guest::run("date +%H")?.trim().parse()?;
    let (start, stop) = ((hour + 6) % 24, (hour + 7) % 24);
    device.managementd.set_config(
        "windows",
        json!({ "start-recording": format!("{start:02}:00"), "stop-recording": format!("{stop:02}:00") }),
    )?;
    lap("window set");
    // The RP2040 may be asleep from an earlier test, out of the agent's reach until its
    // alarm. Someone could restore its power; the agent then gives it the config.
    if Hardware::state()?["rp2040_asleep"] == true {
        Hardware::wake_rp2040()?;
        lap("RP2040 woken");
    }
    // The RP2040 must have the window before the Pi powers off. Otherwise it goes by
    // what an earlier test left it (audio alarms, say) and wakes the Pi for those.
    wait_for("the RP2040 to get the new config", Duration::from_secs(120), || {
        Ok(Some(()).filter(|_| {
            Guest::journal_since("tc2-agent", since)
                .map(|log| log.contains("Rp2040GotNewConfig"))
                .unwrap_or(false)
        }))
    })?;
    lap("RP2040 has the config");

    // The firmware won't have the Pi power off while it holds recordings to offload, which
    // earlier tests leave, and it only offloads them when the agent asks.
    let asked = Guest::now()?;
    Guest::run(
        "sudo busctl --system call org.cacophony.TC2Agent /org/cacophony/TC2Agent \
         org.cacophony.TC2Agent forcerp2040offload",
    )?;
    wait_for("the RP2040's recordings to be offloaded", Duration::from_secs(120), || {
        let log = Guest::journal_since("tc2-agent", asked)?;
        // Each restart reports what's left. Done when it reports events but no files.
        let last = log.lines().rev().find(|l| l.contains("to offload"));
        Ok(last.filter(|l| !l.contains("file(s)")).map(|_| ()))
    })?;
    lap("RP2040's recordings offloaded");

    Guest::run(&format!(
        "sudo mkdir -p $(dirname {DROP_IN}) && printf '%s\\n' '[Service]' 'ExecStart=' \
         'ExecStart=/opt/tc2-emu/guest/tc2-hat-controller attiny --skip-wait' | sudo tee {DROP_IN} \
         && sudo systemctl daemon-reload && sudo systemctl restart tc2-hat-attiny"
    ))?;
    lap("tc2-hat-attiny restarted without waiting");
    let before = Hardware::rtc_time()?;

    wait_for("the Pi to power off", Duration::from_secs(240), || {
        Ok(Some(()).filter(|_| Hardware::pi_is_off().unwrap_or(false)))
    })?;
    lap("Pi powered off");
    let state = Hardware::state()?;
    assert_eq!(state["attiny"]["camera_state"], 3, "the ATtiny reports the Pi powered off");

    wait_for("the firmware to wake the Pi", Duration::from_secs(240), || {
        Ok(Some(()).filter(|_| !Hardware::pi_is_off().unwrap_or(true)))
    })?;
    lap("Pi powered on");
    let skipped = Hardware::rtc_time()? - before;
    // The window starts six hours ahead; the alarm is a little before it.
    if !(4 * 3600..8 * 3600).contains(&skipped) {
        bail!("the RTC moved {skipped}s, not the hours to the window");
    }

    // It booted: the guest answers, with the jumped clock, and managementd is up.
    let guest_time = wait_for("the guest to boot", Duration::from_secs(300), || {
        Ok(Guest::now().ok())
    })?;
    assert!(
        (guest_time - Hardware::rtc_time()?).abs() < 120,
        "the guest's clock {guest_time} isn't the RTC's after the wake"
    );
    wait_for("managementd to answer", Duration::from_secs(120), || {
        Ok(device.managementd.device_info().ok().filter(|info| info.device_id != 0))
    })?;
    lap("guest back up");
    Hardware::set_time_warp(false)?;
    Ok(())
}
