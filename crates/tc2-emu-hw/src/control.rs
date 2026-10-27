//! HTTP API for tests to inspect and change the emulated hardware.
//!
//! - `GET /state`
//! - `PATCH /attiny` with any of the fields of [`attiny::Inputs`]
//! - `PATCH /aht20` with any of the fields of [`aht20::Inputs`]
//! - `PUT /rtc/time` with `{"time": "<RFC 3339>"}`

use crate::hat::{Hat, State};
use crate::{aht20, attiny};
use axum::extract::State as Shared;
use axum::routing::{get, patch, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

pub type SharedHat = Arc<Mutex<Hat>>;

pub fn router(hat: SharedHat) -> Router {
    Router::new()
        .route("/state", get(state))
        .route("/attiny", patch(patch_attiny))
        .route("/aht20", patch(patch_aht20))
        .route("/rtc/time", put(put_rtc_time))
        .with_state(hat)
}

async fn state(Shared(hat): Shared<SharedHat>) -> Json<State> {
    Json(hat.lock().unwrap().state())
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
