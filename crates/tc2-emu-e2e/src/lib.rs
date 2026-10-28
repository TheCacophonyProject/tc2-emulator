//! Helpers for end-to-end tests: a client for the Cacophony API acting as a user
//! (mirroring cacophony-web's e2e-tests helpers), and one for the emulated
//! device's managementd, acting as Sidekick or the web UI.

use anyhow::{Context, bail};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub fn api_url() -> String {
    std::env::var("TC2_E2E_API").unwrap_or_else(|_| "http://localhost:1080".into())
}

pub fn managementd_url() -> String {
    std::env::var("TC2_E2E_MANAGEMENTD").unwrap_or_else(|_| "http://localhost:8080".into())
}

/// A unique name like cacophony-web's getTestName, e.g. `tc2-emu_user-k3j9x2`.
pub fn test_name(kind: &str) -> String {
    format!("tc2-emu_{kind}-{:x}", rand::random::<u32>())
}

/// Polls `check` until it returns `Some`, or fails after `timeout`.
pub fn wait_for<T>(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> anyhow::Result<Option<T>>,
) -> anyhow::Result<T> {
    let deadline = Instant::now() + timeout;
    let mut last_error = None;
    loop {
        match check() {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(e) => last_error = Some(e),
        }
        if Instant::now() > deadline {
            match last_error {
                Some(e) => bail!("timed out after {timeout:?} waiting for {what}: {e:#}"),
                None => bail!("timed out after {timeout:?} waiting for {what}"),
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn client() -> Client {
    Client::builder().timeout(Duration::from_secs(30)).build().unwrap()
}

fn send_json<T: DeserializeOwned>(request: RequestBuilder) -> anyhow::Result<T> {
    let response = request.send()?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("{status}: {body}");
    }
    serde_json::from_str(&body).with_context(|| format!("parsing response {body}"))
}

/// A user on the Cacophony API.
pub struct User {
    pub name: String,
    pub id: u64,
    /// JWT as returned by the API, sent as-is in the Authorization header.
    pub token: String,
    client: Client,
}

#[derive(Deserialize)]
struct LoggedInUser {
    token: String,
    #[serde(rename = "userData")]
    user_data: UserData,
}

#[derive(Deserialize)]
struct UserData {
    id: u64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiDevice {
    pub id: u64,
    pub device_name: String,
    pub group_name: String,
}

impl User {
    /// Registers a new user with a unique name, as createUser does in cacophony-web.
    pub fn create() -> anyhow::Result<Self> {
        let name = test_name("user");
        let client = client();
        let user: LoggedInUser =
            send_json(client.post(format!("{}/api/v1/users", api_url())).json(&json!({
                "userName": name,
                "password": "password",
                "email": format!("{name}@email.com"),
                "endUserAgreement": 3,
            })))
            .context("registering user")?;
        Ok(User { name, id: user.user_data.id, token: user.token, client })
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.client.get(format!("{}{path}", api_url())).header("Authorization", &self.token)
    }

    /// Creates a group (a "project" in browse-next) and returns its id.
    pub fn create_group(&self, name: &str) -> anyhow::Result<u64> {
        let response: Value = send_json(
            self.client
                .post(format!("{}/api/v1/groups", api_url()))
                .header("Authorization", &self.token)
                .json(&json!({ "groupName": name })),
        )
        .context("creating group")?;
        response["groupId"].as_u64().context("no groupId in response")
    }

    /// Creates group `name` if no one has, returning whether it did. Groups can
    /// only be looked up by their members, so this tries to create it.
    pub fn ensure_group(&self, name: &str) -> anyhow::Result<bool> {
        let response = self
            .client
            .post(format!("{}/api/v1/groups", api_url()))
            .header("Authorization", &self.token)
            .json(&json!({ "groupName": name }))
            .send()?;
        let status = response.status();
        let body = response.text()?;
        let in_use = serde_json::from_str::<Value>(&body).unwrap_or_default()["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|m| m == "Group name in use"));
        match status.as_u16() {
            200..=299 => Ok(true),
            422 if in_use => Ok(false),
            _ => bail!("creating group {name}: {status}: {body}"),
        }
    }

    pub fn group_devices(&self, group: &str) -> anyhow::Result<Vec<ApiDevice>> {
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<ApiDevice>,
        }
        let devices: Devices = send_json(self.get(&format!("/api/v1/groups/{group}/devices")))?;
        Ok(devices.devices)
    }

    /// The device's settings on the API, as the web portal shows them.
    pub fn device_settings(&self, device_id: u64) -> anyhow::Result<Value> {
        let mut response: Value =
            send_json(self.get(&format!("/api/v1/devices/{device_id}/settings")))?;
        Ok(response["settings"].take())
    }

    /// Updates device settings as the web portal does. Each section needs its own
    /// `updated` time, which the device sync uses to pick the newest.
    pub fn update_device_settings(&self, device_id: u64, settings: Value) -> anyhow::Result<()> {
        let _: Value = send_json(
            self.client
                .post(format!("{}/api/v1/devices/{device_id}/settings", api_url()))
                .header("Authorization", &self.token)
                .json(&json!({ "settings": settings })),
        )
        .context("updating device settings")?;
        Ok(())
    }

    /// Thermal recordings of any length from `device_id` in a group, newest first.
    pub fn thermal_recordings(&self, group_id: u64, device_id: u64) -> anyhow::Result<Vec<Value>> {
        let response: Value =
            send_json(self.get(&format!("/api/v1/recordings/for-project/{group_id}")).query(&[
                ("types", "thermal"),
                ("devices", &device_id.to_string()),
                ("include-false-positives", "true"),
                // Without this the API applies a minimum duration. Not
                // `status-recordings`, which means only those under 2.5s.
                ("duration", "0"),
            ]))?;
        Ok(response["recordings"].as_array().cloned().unwrap_or_default())
    }
}

/// The emulated device's managementd HTTP API, as used by Sidekick and the web UI.
pub struct Managementd {
    client: Client,
}

/// managementd's /api/device-info.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub devicename: String,
    pub groupname: String,
    #[serde(rename = "deviceID")]
    pub device_id: u64,
}

/// managementd's /api/thermal/thermal-status; `status` is tc2-agent's
/// TestRecordingStatus.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct ThermalStatus {
    pub mode: i32,
    pub status: i32,
}

pub const TEST_RECORDING_READY: i32 = 1;

impl Managementd {
    pub fn new() -> Self {
        Managementd { client: client() }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}/api{path}", managementd_url()))
            // Fixed credentials in managementd.
            .basic_auth("admin", Some("feathers"))
    }

    fn send(&self, request: RequestBuilder) -> anyhow::Result<Response> {
        let response = request.send()?;
        if !response.status().is_success() {
            let status = response.status();
            bail!("{status}: {}", response.text()?);
        }
        Ok(response)
    }

    pub fn device_info(&self) -> anyhow::Result<DeviceInfo> {
        send_json(self.request(reqwest::Method::GET, "/device-info"))
    }

    /// Moves and/or renames the device, authorized by `user`, as Sidekick does.
    pub fn reregister_authorized(
        &self,
        user: &User,
        group: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        self.send(self.request(reqwest::Method::POST, "/reregister-authorized").json(&json!({
            "newGroup": group,
            "newName": name,
            "authorizedUser": user.token,
        })))?;
        Ok(())
    }

    /// The device's config sections, as `/api/config` returns them: keyed by
    /// camelCase section name, with Go field names inside.
    pub fn config(&self) -> anyhow::Result<Value> {
        let mut config: Value = send_json(self.request(reqwest::Method::GET, "/config"))?;
        Ok(config["values"].take())
    }

    /// Writes keys of one config section (e.g. `thermal-recorder`, with mapstructure
    /// keys like `use-low-power-mode`), as Sidekick does. The device stamps `updated`.
    pub fn set_config(&self, section: &str, config: Value) -> anyhow::Result<()> {
        self.send(
            self.request(reqwest::Method::POST, "/config")
                .form(&[("section", section), ("config", &config.to_string())]),
        )?;
        Ok(())
    }

    /// The audio recording form of the web UI.
    pub fn set_audio_recording(&self, mode: &str) -> anyhow::Result<()> {
        self.send(
            self.request(reqwest::Method::POST, "/audiorecording")
                .form(&[("audio-mode", mode), ("audio-seed", "0")]),
        )?;
        Ok(())
    }

    /// Rewrites the device's location as it is, so the config file changes and
    /// cacophony-config-sync runs (after 10s), as it would after Sidekick edits.
    /// Nothing else prompts a sync of portal changes: it otherwise runs at start,
    /// when the modem connects, and daily.
    pub fn trigger_config_sync(&self) -> anyhow::Result<()> {
        let location = self.config()?["location"].clone();
        let coordinate = |key: &str| location[key].as_f64().unwrap_or_default().to_string();
        self.send(self.request(reqwest::Method::POST, "/location").form(&[
            ("latitude", coordinate("Latitude")),
            ("longitude", coordinate("Longitude")),
            ("altitude", "0".into()),
            ("accuracy", "0".into()),
            ("timestamp", "0".into()),
        ]))?;
        Ok(())
    }

    pub fn take_short_test_recording(&self) -> anyhow::Result<String> {
        send_json(self.request(reqwest::Method::PUT, "/thermal/short-test-recording"))
    }

    pub fn thermal_status(&self) -> anyhow::Result<ThermalStatus> {
        send_json(self.request(reqwest::Method::GET, "/thermal/thermal-status"))
    }
}

/// A registered device moved into a new user's group, as after `new_device_setup`.
pub struct TestDevice {
    pub managementd: Managementd,
    pub user: User,
    pub group: String,
    pub group_id: u64,
    pub device_id: u64,
}

impl TestDevice {
    /// Waits for the emulated device to register, then has a new user move it into
    /// a new group. Reregistering gives the device a new id each time.
    pub fn set_up() -> anyhow::Result<Self> {
        let managementd = Managementd::new();
        let user = User::create()?;
        user.ensure_group("new")?;
        wait_for("device to register", Duration::from_secs(300), || {
            Ok(managementd.device_info().ok().filter(|info| info.device_id != 0))
        })?;
        let group = test_name("group");
        let group_id = user.create_group(&group)?;
        managementd.reregister_authorized(&user, &group, &test_name("device"))?;
        let device_id = managementd.device_info()?.device_id;
        Ok(TestDevice { managementd, user, group, group_id, device_id })
    }

    pub fn settings(&self) -> anyhow::Result<Value> {
        self.user.device_settings(self.device_id)
    }
}

/// An `updated` time for a settings section as the web portal makes it.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
