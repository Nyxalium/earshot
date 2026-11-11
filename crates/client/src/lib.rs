//! The D-Bus interface of the Earshot service.
//!
//! The service does all recording. The terminal command, the window and the
//! MCP server control it through [`EarshotProxy`], so they always agree on
//! what is being recorded.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use zbus::names::WellKnownName;
use zbus::zvariant::{DeserializeDict, SerializeDict, Type};

pub const BUS_NAME: &str = "io.github.nyxalium.Earshot";
pub const OBJECT_PATH: &str = "/io/github/nyxalium/Earshot";
/// File name of the service binary, looked for next to the calling program
/// when the service is not installed for D-Bus activation.
pub const SERVICE_BINARY: &str = "earshotd";

/// Options for a new recording. Anything left out uses the user's config.
#[derive(Clone, Debug, Default, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
pub struct StartOptions {
    /// `microphone`, `system` or `both`.
    pub source: Option<String>,
    /// `opus`, `flac` or `wav`.
    pub format: Option<String>,
}

#[derive(Clone, Debug, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
pub struct Status {
    /// `idle`, `recording` or `paused`.
    pub state: String,
    /// Length of the audio recorded so far; time spent paused is not counted.
    pub elapsed_ms: u64,
    pub path: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
pub struct RecordingInfo {
    pub path: String,
    /// File name without its extension.
    pub name: String,
    /// Start time, in seconds since the Unix epoch.
    pub created: u64,
    /// Missing for audio files Earshot did not record.
    pub duration_ms: Option<u64>,
    pub size_bytes: u64,
    pub source: Option<String>,
}

#[zbus::proxy(
    interface = "io.github.nyxalium.Earshot1",
    default_service = "io.github.nyxalium.Earshot",
    default_path = "/io/github/nyxalium/Earshot"
)]
pub trait Earshot {
    /// Starts a recording and returns the file it is being written to.
    fn start(&self, options: StartOptions) -> zbus::Result<String>;
    fn pause(&self) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    /// Finishes the recording and returns the saved file.
    fn stop(&self) -> zbus::Result<String>;
    /// Discards the recording in progress.
    fn cancel(&self) -> zbus::Result<()>;
    fn status(&self) -> zbus::Result<Status>;

    /// Lists saved recordings, newest first.
    fn list(&self) -> zbus::Result<Vec<RecordingInfo>>;
    /// Renames a recording, given its path or file name. Returns the new path.
    fn rename(&self, recording: &str, new_name: &str) -> zbus::Result<String>;
    /// Moves a recording, given its path or file name, to the trash.
    fn delete(&self, recording: &str) -> zbus::Result<()>;

    /// A recording started, paused, resumed or ended.
    #[zbus(signal)]
    fn state_changed(&self, status: Status) -> zbus::Result<()>;
    /// Input loudness from 0 to 1, sent about 20 times a second while recording.
    #[zbus(signal)]
    fn level(&self, peak: f64) -> zbus::Result<()>;
    /// A recording ended because of an error. `path` is empty if no audio
    /// could be saved.
    #[zbus(signal)]
    fn recording_failed(&self, message: &str, path: &str) -> zbus::Result<()>;
    /// The set of saved recordings changed.
    #[zbus(signal)]
    fn recordings_changed(&self) -> zbus::Result<()>;
}

/// Connects to the Earshot service, starting it if it is not running.
pub async fn connect() -> zbus::Result<EarshotProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    ensure_running(&connection).await?;
    EarshotProxy::new(&connection).await
}

async fn ensure_running(connection: &zbus::Connection) -> zbus::Result<()> {
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    let name = WellKnownName::from_static_str(BUS_NAME)?;
    if bus.name_has_owner(name.clone().into()).await? {
        return Ok(());
    }

    // An installed service is started by the session bus from its service file.
    match bus.start_service_by_name(name.clone(), 0).await {
        Ok(_) => return Ok(()),
        Err(zbus::fdo::Error::ServiceUnknown(_)) => {}
        Err(e) => return Err(e.into()),
    }

    // Otherwise run the service binary that sits next to this program.
    let binary = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(SERVICE_BINARY)))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            zbus::Error::Failure(format!(
                "the Earshot service is not running and could not be started: \
                 it is not installed as a D-Bus service, and no {SERVICE_BINARY} \
                 binary sits next to this program"
            ))
        })?;
    Command::new(&binary)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, so closing this terminal does not end a recording.
        .process_group(0)
        .spawn()
        .map_err(|e| zbus::Error::Failure(format!("could not run {}: {e}", binary.display())))?;

    for _ in 0..50 {
        if bus.name_has_owner(name.clone().into()).await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(zbus::Error::Failure(format!(
        "{} was started but did not appear on the session bus",
        binary.display()
    )))
}
