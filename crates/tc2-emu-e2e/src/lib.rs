//! Helpers for end-to-end tests: a client for the Cacophony API acting as a user
//! (mirroring cacophony-web's e2e-tests helpers), and one for the emulated
//! device's managementd, acting as Sidekick or the web UI.
//!
//! Projects are called groups by the API and managementd; that name is kept only
//! in their paths and JSON fields.

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

/// A logged-in session on the Cacophony API, as a user or a device.
pub struct Session {
    /// JWT as returned by the API, sent as-is in the Authorization header.
    pub token: String,
    client: Client,
}

impl Session {
    pub fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", api_url()))
            .header("Authorization", &self.token)
    }

    pub fn get(&self, path: &str) -> RequestBuilder {
        self.request(reqwest::Method::GET, path)
    }

    pub fn post(&self, path: &str) -> RequestBuilder {
        self.request(reqwest::Method::POST, path)
    }
}

/// Anything that makes API requests with its own token. Requests only some kinds
/// of entity may make are inherent methods on that type; ones open to any entity
/// belong here as provided methods.
pub trait LoggedInEntity {
    fn session(&self) -> &Session;

    /// The device's settings on the API, as the web portal shows them. Users must
    /// be in the device's project; a device can only get its own.
    fn device_settings(&self, device_id: u64) -> anyhow::Result<Value> {
        let mut response: Value =
            send_json(self.session().get(&format!("/api/v1/devices/{device_id}/settings")))?;
        Ok(response["settings"].take())
    }

    /// Updates device settings as the web portal does. Each section needs its own
    /// `updated` time, which the device sync uses to pick the newest.
    fn update_device_settings(&self, device_id: u64, settings: Value) -> anyhow::Result<()> {
        let _: Value = send_json(
            self.session()
                .post(&format!("/api/v1/devices/{device_id}/settings"))
                .json(&json!({ "settings": settings })),
        )
        .context("updating device settings")?;
        Ok(())
    }
}

/// A user on the Cacophony API.
pub struct User {
    pub name: String,
    pub id: u64,
    pub session: Session,
}

impl LoggedInEntity for User {
    fn session(&self) -> &Session {
        &self.session
    }
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
    #[serde(rename = "groupName")]
    pub project_name: String,
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
        Ok(User { name, id: user.user_data.id, session: Session { token: user.token, client } })
    }

    /// Creates a project, of which this user becomes admin.
    pub fn create_project(&self, name: &str) -> anyhow::Result<ProjectAdmin<'_>> {
        let response: Value =
            send_json(self.session.post("/api/v1/groups").json(&json!({ "groupName": name })))
                .context("creating project")?;
        let project_id = response["groupId"].as_u64().context("no groupId in response")?;
        Ok(ProjectAdmin::assume(self, project_id))
    }

    /// Checks that this user is an admin of `project_id`, so that a setup mistake
    /// fails here rather than as a 403 later.
    pub fn as_admin_of(&self, project_id: u64) -> anyhow::Result<ProjectAdmin<'_>> {
        let response: Value = send_json(self.session.get(&format!("/api/v1/groups/{project_id}")))
            .with_context(|| format!("getting project {project_id}"))?;
        if response["group"]["admin"] != true {
            bail!("{} is not an admin of project {project_id}", self.name);
        }
        Ok(ProjectAdmin::assume(self, project_id))
    }

    /// Creates project `name` if it doesn't exist, returning creation success.
    /// Projects can only be looked up by their members.
    pub fn ensure_project_exists(&self, name: &str) -> anyhow::Result<bool> {
        let response =
            self.session.post("/api/v1/groups").json(&json!({ "groupName": name })).send()?;
        let status = response.status();
        let body = response.text()?;
        let in_use = serde_json::from_str::<Value>(&body).unwrap_or_default()["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|m| m == "Group name in use"));
        match status.as_u16() {
            200..=299 => Ok(true),
            422 if in_use => Ok(false),
            _ => bail!("creating project {name}: {status}: {body}"),
        }
    }

    pub fn project_devices(&self, project: &str) -> anyhow::Result<Vec<ApiDevice>> {
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<ApiDevice>,
        }
        let devices: Devices =
            send_json(self.session.get(&format!("/api/v1/groups/{project}/devices")))?;
        Ok(devices.devices)
    }

    /// Thermal recordings of any length from `device_id` in a project, newest first.
    pub fn thermal_recordings(
        &self,
        project_id: u64,
        device_id: u64,
    ) -> anyhow::Result<Vec<Value>> {
        let response: Value = send_json(
            self.session.get(&format!("/api/v1/recordings/for-project/{project_id}")).query(&[
                ("types", "thermal"),
                ("devices", &device_id.to_string()),
                ("include-false-positives", "true"),
                // Without this the API applies a minimum duration. Not
                // `status-recordings`, which means only those under 2.5s.
                ("duration", "0"),
            ]),
        )?;
        Ok(response["recordings"].as_array().cloned().unwrap_or_default())
    }
}

/// A user acting as an admin of one project. Admin rights are per project, so
/// requests that need them are methods here, implicitly on `project_id`.
pub struct ProjectAdmin<'a> {
    pub user: &'a User,
    pub project_id: u64,
}

impl<'a> ProjectAdmin<'a> {
    /// Without checking `user` is an admin: prefer `User::create_project` or
    /// `User::as_admin_of`. For tests that a non-admin is refused.
    pub fn assume(user: &'a User, project_id: u64) -> Self {
        ProjectAdmin { user, project_id }
    }

    /// Adds `user` to the project, or changes their admin status if already in it.
    pub fn add_user(&self, user: &User, admin: bool) -> anyhow::Result<()> {
        let _: Value = send_json(self.session().post("/api/v1/groups/users").json(&json!({
            "groupId": self.project_id,
            "userId": user.id,
            "admin": admin,
        })))
        .with_context(|| format!("adding {} to project {}", user.name, self.project_id))?;
        Ok(())
    }
}

impl LoggedInEntity for ProjectAdmin<'_> {
    fn session(&self) -> &Session {
        self.user.session()
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
    #[serde(rename = "groupname")]
    pub projectname: String,
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
    /// `user` only needs to be in `project`: managementd always sends a project,
    /// filling in the current one, so the API's admin check for a rename alone
    /// never applies.
    pub fn reregister_authorized(
        &self,
        user: &User,
        project: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        self.send(self.request(reqwest::Method::POST, "/reregister-authorized").json(&json!({
            "newGroup": project,
            "newName": name,
            "authorizedUser": user.session.token,
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
    pub fn set_audio_recording(&self, mode: &str, seed: u32) -> anyhow::Result<()> {
        self.send(
            self.request(reqwest::Method::POST, "/audiorecording")
                .form(&[("audio-mode", mode), ("audio-seed", &seed.to_string())]),
        )?;
        Ok(())
    }

    /// Sets the battery chemistry and/or cell count, as the battery page does. An
    /// empty chemistry or zero cell count is left for the device to detect.
    pub fn set_battery_config(&self, chemistry: &str, cell_count: u32) -> anyhow::Result<()> {
        self.send(
            self.request(reqwest::Method::POST, "/battery/config")
                .json(&json!({ "chemistry": chemistry, "cellCount": cell_count })),
        )?;
        Ok(())
    }

    /// Goes back to detecting the battery, as the battery page does.
    pub fn clear_battery_config(&self) -> anyhow::Result<()> {
        self.send(self.request(reqwest::Method::DELETE, "/battery/config"))?;
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

/// A registered device moved into a new user's project, as after `new_device_setup`.
pub struct TestDevice {
    pub managementd: Managementd,
    pub user: User,
    pub project: String,
    pub project_id: u64,
    pub device_id: u64,
}

impl TestDevice {
    /// Waits for the emulated device to register, then has a new user move it into
    /// a new project. With no device of the same name there to take over, that
    /// makes a new device, so it gets a new id each time.
    pub fn set_up() -> anyhow::Result<Self> {
        let managementd = Managementd::new();
        let user = User::create()?;
        user.ensure_project_exists("new")?;
        wait_for("device to register", Duration::from_secs(300), || {
            Ok(managementd.device_info().ok().filter(|info| info.device_id != 0))
        })?;
        let project = test_name("project");
        let project_id = user.create_project(&project)?.project_id;
        managementd.reregister_authorized(&user, &project, &test_name("device"))?;
        let device_id = managementd.device_info()?.device_id;
        Ok(TestDevice { managementd, user, project, project_id, device_id })
    }

    /// `user`, as the admin of the project it created.
    pub fn admin(&self) -> ProjectAdmin<'_> {
        ProjectAdmin::assume(&self.user, self.project_id)
    }

    pub fn settings(&self) -> anyhow::Result<Value> {
        self.user.device_settings(self.device_id)
    }
}

/// An `updated` time for a settings section as the web portal makes it.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
