//! Classifying recordings on the device, which is what comms (a trap) needs, rather
//! than in the cloud after upload.
//!
//! The device's classifier (classifier-pipeline's `pi_classify`, run by
//! thermal-recorder-py) only loads its models and serves them on 127.0.0.1:8123 while
//! `[thermal-motion] run-classifier` is true. managementd sets that from comms: on
//! with comms enabled, off without it. Config sync doesn't, as comms can't be
//! enabled remotely.
//!
//! These tests set comms from the web UI, check the model server comes and goes with
//! it, then classify the test recordings salt puts in /var/spool/cptv/test-recordings
//! with `pi_classify --file`, as it would a recording off the camera. They don't use the
//! API, and assume the emulated device is up, so they run after the others, which leave
//! the device as they found it.
//!
//! Classifying is slow on the emulated Pi, about a minute for each recording.
//!
//! Needs a freshly started emulator; run with `make e2e`.

use serde_json::{Value, json};
use std::time::Duration;
use tc2_emu_e2e::{Guest, Managementd, wait_for};

/// The classifier restarts when the config changes, once the recorder is idle, then
/// loads two models. About 30s on the emulated device when it isn't busy.
const RESTART_TIMEOUT: Duration = Duration::from_secs(180);

/// managementd, once the freshly started device is up to answer it.
fn managementd() -> anyhow::Result<Managementd> {
    let managementd = Managementd::new();
    wait_for("managementd to answer", Duration::from_secs(300), || {
        Ok(managementd.config().ok().map(|_| ()))
    })?;
    Ok(managementd)
}

fn run_classifier(managementd: &Managementd) -> anyhow::Result<bool> {
    Ok(managementd.config()?["thermalMotion"]["RunClassifier"].as_bool().unwrap_or(false))
}

/// Whether the model server is up and has loaded its model, from the guest's side.
fn model_server_ready() -> anyhow::Result<bool> {
    let (_, body) = Guest::run_any("curl -s -m 5 http://127.0.0.1:8123/ready")?;
    Ok(serde_json::from_str::<Value>(&body).is_ok_and(|ready| ready["ready"] == true))
}

fn wait_for_model_server(up: bool) -> anyhow::Result<()> {
    let what = if up { "model server to be ready" } else { "model server to stop" };
    wait_for(what, RESTART_TIMEOUT, || Ok((model_server_ready()? == up).then_some(()))).inspect_err(|_| {
        // The model server has twice failed to come up in a full run, without logging
        // anything, and not when this test is run alone: say what the guest was doing.
        if let Ok((_, state)) = Guest::run_any(
            "free -m; ps -eo pid,ppid,etime,stat,rss,pcpu,wchan:20,args --sort=-rss | head -15; \
             journalctl -b -q -o short-monotonic --no-pager | grep -E 'servemodel|serve_model|thermal-recorder|swap' | tail -15",
        ) {
            println!("Guest state when waiting for the model server to {}:\n{state}", if up { "be ready" } else { "stop" });
        }
    })
}

/// High power mode, so recordings come through the classifier's normal path whatever the
/// time of day (medium power only runs inside the recording window), then comms on or
/// off. managementd sets run-classifier from comms.
fn set_comms(managementd: &Managementd, enable: bool) -> anyhow::Result<()> {
    managementd.set_config(
        "thermal-recorder",
        json!({ "use-low-power-mode": false, "instant-classify": false }),
    )?;
    managementd.set_config("comms", json!({ "enable": enable, "comms-out": "uart" }))
}

/// What `pi_classify` logs for a track: `Clip 1 1 cat 93% threshold: 50%`.
#[derive(Debug, PartialEq)]
struct Classification {
    label: String,
    confidence: u32,
    threshold: u32,
}

/// The tracks classified by the log of `pi_classify --file`. The false positive model
/// also logs its verdict, as "animal" or "false-positive".
fn classifications(log: &str) -> Vec<Classification> {
    log.lines()
        .filter_map(|line| {
            let mut words = line.split_once(" Clip ")?.1.split_whitespace();
            let (_clip, _track, label) = (words.next()?, words.next()?, words.next()?);
            let confidence = words.next()?.strip_suffix('%')?.parse().ok()?;
            let threshold = words.nth(1)?.strip_suffix('%')?.parse().ok()?;
            Some(Classification { label: label.into(), confidence, threshold })
        })
        .filter(|c| c.label != "animal" && c.label != "false-positive")
        .collect()
}

#[test]
fn parses_classifications_from_the_log() {
    let log = "2284 1234:   INFO Clip 1 1 animal 97% threshold: 80% \n\
               2284 1234:   INFO Clip 1 1 cat 93% threshold: 50% \n\
               2284 1234:   INFO Pred for model FP Model\n";
    assert_eq!(
        classifications(log),
        [Classification { label: "cat".into(), confidence: 93, threshold: 50 }]
    );
}

#[test]
#[ignore = "needs a running emulator; run with `make e2e`"]
fn classifier_runs_only_with_comms() -> anyhow::Result<()> {
    let managementd = managementd()?;

    set_comms(&managementd, false)?;
    assert!(!run_classifier(&managementd)?, "run-classifier without comms");
    wait_for_model_server(false)?;

    set_comms(&managementd, true)?;
    assert!(run_classifier(&managementd)?, "run-classifier with comms");
    wait_for_model_server(true)?;

    set_comms(&managementd, false)?;
    assert!(!run_classifier(&managementd)?, "run-classifier after comms is turned off");
    wait_for_model_server(false)?;
    Ok(())
}

#[test]
#[ignore = "needs a running emulator; run with `make e2e`"]
fn classifies_test_recordings() -> anyhow::Result<()> {
    let managementd = managementd()?;
    set_comms(&managementd, true)?;
    wait_for_model_server(true)?;

    // Recordings the models are sure of. The hedgehog one isn't: the models call it a
    // possum, below the possum threshold. The label is what the model's config calls it.
    let expected = [("cat", "cat"), ("human", "human"), ("stoat", "mustelid")];
    let mut failures = vec![];
    for (recording, label) in expected {
        // From /, where the label mappings file is, as for the service.
        let log = Guest::run(&format!(
            "cd / && sudo /home/pi/.venv/classifier/bin/pi_classify \
             --file /var/spool/cptv/test-recordings/{recording}.cptv --seed 1"
        ))?;
        let found = classifications(&log);
        println!("{recording}: {found:?}");
        if !found.iter().any(|c| c.label == label && c.confidence >= c.threshold) {
            failures.push(format!("{recording} wasn't classified as {label}: {found:?}"));
        }
    }

    set_comms(&managementd, false)?;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
