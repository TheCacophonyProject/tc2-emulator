//! HTTP API for tests to inspect and change the emulated hardware.
//!
//! - `GET /`, a page listing these
//! - `GET /state`
//! - `GET /time`, the emulated RTC's time as epoch seconds with milliseconds, which
//!   the guest sets its clock from (the RTC itself only has whole seconds)
//! - `PATCH /attiny` with any of the fields of [`attiny::Inputs`]
//! - `PATCH /aht20` with any of the fields of [`aht20::Inputs`]
//! - `PUT /rtc/time` with `{"time": "<RFC 3339>"}`

use crate::hat::{Hat, State};
use crate::{aht20, attiny};
use axum::extract::State as Shared;
use axum::routing::{get, patch, put};
use axum::response::Html;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

pub type SharedHat = Arc<Mutex<Hat>>;

pub fn router(hat: SharedHat) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/state", get(state))
        .route("/time", get(time))
        .route("/attiny", patch(patch_attiny))
        .route("/aht20", patch(patch_aht20))
        .route("/rtc/time", put(put_rtc_time))
        .with_state(hat)
}

const INDEX: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>TC2 emulator hardware control API</title>
<body style="font-family: sans-serif; max-width: 42em; margin: 2em auto">
<h1>TC2 emulator hardware control API</h1>
<p>This controls the emulated hardware (ATtiny, RTC, AHT20, EEPROM). The
device's management interface is on <a href="http://localhost:8080">port 8080</a>.</p>
<ul>
<li><a href="/state"><code>GET /state</code></a>: the hardware's current state</li>
<li><code>PATCH /attiny</code>: JSON with any of the ATtiny inputs, e.g. <code>{"hv_battery_volts": 11.2}</code></li>
<li><code>PATCH /aht20</code>: JSON with any of the AHT20 inputs, e.g. <code>{"temperature_celsius": 35}</code></li>
<li><code>PUT /rtc/time</code>: <code>{"time": "2027-01-01T09:00:00Z"}</code></li>
</ul>
</body>
"#;

async fn index() -> Html<&'static str> {
    Html(INDEX)
}

async fn state(Shared(hat): Shared<SharedHat>) -> Json<State> {
    Json(hat.lock().unwrap().state())
}

async fn time(Shared(hat): Shared<SharedHat>) -> String {
    let time = hat.lock().unwrap().rtc.time();
    format!("{}.{:03}", time.timestamp(), time.timestamp_subsec_millis())
}

async fn patch_attiny(
    Shared(hat): Shared<SharedHat>,
    Json(patch): Json<attiny::InputsPatch>,
) -> Json<State> {
    let mut hat = hat.lock().unwrap();
    hat.attiny.update_inputs(patch);
    Json(hat.state())
}

async fn patch_aht20(
    Shared(hat): Shared<SharedHat>,
    Json(patch): Json<aht20::InputsPatch>,
) -> Json<State> {
    let mut hat = hat.lock().unwrap();
    hat.aht20.update_inputs(patch);
    Json(hat.state())
}

#[derive(Deserialize)]
struct RtcTime {
    time: DateTime<Utc>,
}

async fn put_rtc_time(Shared(hat): Shared<SharedHat>, Json(body): Json<RtcTime>) -> Json<State> {
    let mut hat = hat.lock().unwrap();
    hat.rtc.set_time(body.time);
    Json(hat.state())
}
