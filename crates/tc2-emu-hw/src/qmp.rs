//! Power cycling the emulated Pi. On a TC2 the ATtiny cuts the Pi's power once
//! tc2-hat-attiny has shut it down, and restores it when the RP2040 asks for the Pi
//! again (bit 0 of its register 0x05). QEMU runs with `-no-shutdown`, so a guest
//! shutdown leaves it paused rather than exiting, and this resets it when that bit
//! is set again.

use crate::control::SharedHat;
use anyhow::Context;
use log::{debug, info, warn};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// How often the RP2040's request for the Pi is checked while the Pi is off.
const POLL: Duration = Duration::from_millis(200);

/// Whether the Pi is powered, from QEMU's events and the RP2040's requests.
#[derive(Default)]
struct PiPower {
    off: bool,
    /// The request seen at the last check, so that a wake needs the request to
    /// rise after the shutdown, not just be set (the ATtiny starts with it set).
    wanted: bool,
}

impl PiPower {
    fn on_event(&mut self, event: &str, wants_pi_on: bool) {
        if event == "SHUTDOWN" {
            self.off = true;
            self.wanted = wants_pi_on;
        }
    }

    /// True when the Pi should be powered on now.
    fn poll(&mut self, wants_pi_on: bool) -> bool {
        let rising = self.off && wants_pi_on && !self.wanted;
        self.wanted = wants_pi_on;
        if rising {
            self.off = false;
        }
        rising
    }
}

/// QEMU owns the socket and may not have started yet, so this is retried.
pub async fn run(socket: &Path, hat: &SharedHat) -> ! {
    loop {
        match serve(socket, hat).await {
            Ok(()) => info!("QMP connection closed"),
            Err(e) => debug!("QMP connection: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn serve(socket: &Path, hat: &SharedHat) -> anyhow::Result<()> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    info!("Connected to QEMU at {}", socket.display());
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"{\"execute\":\"qmp_capabilities\"}\n").await?;
    let mut power = PiPower::default();
    let mut tick = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                let message: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(message) => message,
                    Err(e) => {
                        warn!("Ignoring malformed QMP message {line:?}: {e}");
                        continue;
                    }
                };
                if let Some(error) = message.get("error") {
                    warn!("QMP error: {error}");
                }
                if let Some(event) = message.get("event").and_then(|e| e.as_str()) {
                    debug!("QMP event {event}");
                    let wants = hat.lock().unwrap().attiny.rp2040_wants_pi_on();
                    power.on_event(event, wants);
                    if event == "SHUTDOWN" {
                        hat.lock().unwrap().set_pi_off(true);
                        info!("Pi powered off");
                    }
                }
            }
            _ = tick.tick() => {
                let wants = hat.lock().unwrap().attiny.rp2040_wants_pi_on();
                if power.poll(wants) {
                    hat.lock().unwrap().set_pi_off(false);
                                    info!("RP2040 wants the Pi on, powering it on");
                    // After a shutdown QEMU is stopped; the reset is the power-up and
                    // `cont` makes sure it then runs.
                    writer.write_all(b"{\"execute\":\"system_reset\"}\n{\"execute\":\"cont\"}\n").await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_on_while_running() {
        let mut power = PiPower::default();
        assert!(!power.poll(true));
        assert!(!power.poll(false));
        assert!(!power.poll(true));
    }

    #[test]
    fn wakes_when_the_request_rises_after_shutdown() {
        let mut power = PiPower::default();
        power.on_event("SHUTDOWN", false);
        assert!(!power.poll(false));
        assert!(power.poll(true));
        assert!(!power.poll(true), "woken once");
    }

    #[test]
    fn a_request_already_set_at_shutdown_does_not_wake() {
        let mut power = PiPower::default();
        power.on_event("SHUTDOWN", true);
        assert!(!power.poll(true));
        assert!(!power.poll(false));
        assert!(power.poll(true));
    }

    #[test]
    fn other_events_are_ignored() {
        let mut power = PiPower::default();
        power.on_event("RESET", false);
        assert!(!power.poll(true));
    }
}
