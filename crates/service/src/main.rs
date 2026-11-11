//! The Earshot background service: owns the recorder and exposes it, and the
//! recordings folder, on the session bus.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use earshot_client::{BUS_NAME, OBJECT_PATH, RecordingInfo, StartOptions, Status};
use earshot_core::{Config, Error, Event, Library, Recorder, State};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, oneshot};
use zbus::object_server::SignalEmitter;
use zbus::{Variant, fdo, interface};

struct Service {
    recorder: Arc<Recorder>,
}

#[interface(name = "io.github.nyxalium.Earshot1")]
impl Service {
    async fn start(&self, options: StartOptions) -> fdo::Result<String> {
        let recorder = self.recorder.clone();
        blocking(move || {
            let config = Config::load()?;
            recorder
                .start(earshot_core::StartOptions {
                    source: options.source.map_or(Ok(config.source), |s| s.parse())?,
                    format: options.format.map_or(Ok(config.format), |s| s.parse())?,
                    dir: config.recordings_dir,
                })
                .map(|path| path_string(&path))
        })
        .await
    }

    async fn pause(&self) -> fdo::Result<()> {
        let recorder = self.recorder.clone();
        blocking(move || recorder.pause()).await
    }

    async fn resume(&self) -> fdo::Result<()> {
        let recorder = self.recorder.clone();
        blocking(move || recorder.resume()).await
    }

    async fn stop(&self) -> fdo::Result<String> {
        let recorder = self.recorder.clone();
        blocking(move || recorder.stop().map(|path| path_string(&path))).await
    }

    async fn cancel(&self) -> fdo::Result<()> {
        let recorder = self.recorder.clone();
        blocking(move || recorder.cancel()).await
    }

    async fn status(&self) -> fdo::Result<Status> {
        let recorder = self.recorder.clone();
        blocking(move || Ok(status(&recorder))).await
    }

    async fn list(&self) -> fdo::Result<Vec<RecordingInfo>> {
        let recorder = self.recorder.clone();
        blocking(move || {
            // The file still being written is not a saved recording yet.
            let recording_now = recorder.status().path;
            let recordings = library()?
                .list()?
                .into_iter()
                .filter(|r| Some(&r.path) != recording_now.as_ref())
                .map(|r| RecordingInfo {
                    path: path_string(&r.path),
                    name: r.name,
                    created: r.created,
                    duration_ms: r.duration_ms,
                    size_bytes: r.size_bytes,
                    source: r.source.map(|s| s.to_string()),
                })
                .collect();
            Ok(recordings)
        })
        .await
    }

    async fn rename(
        &self,
        recording: String,
        new_name: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        let recorder = self.recorder.clone();
        let renamed = blocking(move || {
            let library = library()?;
            let path = saved_recording(&library, &recorder, &recording)?;
            library
                .rename(&path, &new_name)
                .map(|path| path_string(&path))
        })
        .await?;
        report(Self::recordings_changed(&emitter).await);
        Ok(renamed)
    }

    async fn delete(
        &self,
        recording: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let recorder = self.recorder.clone();
        blocking(move || {
            let library = library()?;
            let path = saved_recording(&library, &recorder, &recording)?;
            library.delete(&path)
        })
        .await?;
        report(Self::recordings_changed(&emitter).await);
        Ok(())
    }

    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, status: Status) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn level(emitter: &SignalEmitter<'_>, peak: f64) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn recording_failed(
        emitter: &SignalEmitter<'_>,
        message: &str,
        path: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn recordings_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Runs recorder and file work, which can block, off the D-Bus executor.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> earshot_core::Result<T> + Send + 'static,
) -> fdo::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| fdo::Error::Failed(e.to_string()))?
        .map_err(|e| fdo::Error::Failed(e.to_string()))
}

fn library() -> earshot_core::Result<Library> {
    Ok(Library::new(Config::load()?.recordings_dir))
}

/// Resolves a path or file name to a saved recording, refusing the file that
/// is currently being recorded.
fn saved_recording(
    library: &Library,
    recorder: &Recorder,
    recording: &str,
) -> earshot_core::Result<PathBuf> {
    let path = library.resolve(Path::new(recording))?;
    if recorder.status().path.as_ref() == Some(&path) {
        return Err(Error::Invalid(format!(
            "{} is still being recorded",
            path.display()
        )));
    }
    Ok(path)
}

fn status(recorder: &Recorder) -> Status {
    let status = recorder.status();
    Status {
        state: status.state.to_string(),
        elapsed_ms: status.elapsed_ms,
        path: status.path.as_deref().map(path_string),
        source: status.source.map(|s| s.to_string()),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn report(result: zbus::Result<()>) {
    if let Err(e) = result {
        eprintln!("earshotd: {e}");
    }
}

/// Turns one recorder event into D-Bus signals and desktop notifications.
async fn forward(
    connection: &zbus::Connection,
    emitter: &SignalEmitter<'static>,
    recorder: &Arc<Recorder>,
    event: Event,
) {
    if let Event::Level(peak) = event {
        report(Service::level(emitter, peak.into()).await);
        return;
    }

    let recorder = recorder.clone();
    match tokio::task::spawn_blocking(move || status(&recorder)).await {
        Ok(status) => report(Service::state_changed(emitter, status).await),
        Err(e) => eprintln!("earshotd: {e}"),
    }

    match event {
        Event::Stopped { path } => {
            report(Service::recordings_changed(emitter).await);
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            report(notify(connection, "Recording saved", &name).await);
        }
        Event::Failed { message, path } => {
            let path = path.as_deref().map(path_string).unwrap_or_default();
            report(Service::recording_failed(emitter, &message, &path).await);
            if !path.is_empty() {
                report(Service::recordings_changed(emitter).await);
            }
            report(notify(connection, "Recording failed", &message).await);
        }
        _ => {}
    }
}

/// Shows a desktop notification.
async fn notify(connection: &zbus::Connection, summary: &str, body: &str) -> zbus::Result<()> {
    connection
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "Earshot",
                0u32,
                "audio-input-microphone",
                summary,
                body,
                Vec::<&str>::new(),
                HashMap::<&str, Variant<'_>>::new(),
                -1i32,
            ),
        )
        .await
        .map(drop)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let recorder = Arc::new(Recorder::new(move |event| {
        let _ = events_tx.send(event);
    })?);

    let connection = zbus::connection::Builder::session()
        .name(BUS_NAME)
        .serve_at(
            OBJECT_PATH,
            Service {
                recorder: recorder.clone(),
            },
        )
        .build()
        .await
        .map_err(|e| format!("could not claim {BUS_NAME} on the session bus: {e}"))?;

    let emitter = connection
        .object_server()
        .interface::<_, Service>(OBJECT_PATH)
        .await?
        .signal_emitter()
        .clone();
    let (quit_tx, mut quit) = oneshot::channel::<()>();
    let forwarder = tokio::spawn({
        let (connection, recorder) = (connection.clone(), recorder.clone());
        async move {
            loop {
                // Biased, so events already queued are sent before quitting.
                tokio::select! {
                    biased;
                    Some(event) = events.recv() => {
                        forward(&connection, &emitter, &recorder, event).await;
                    }
                    _ = &mut quit => break,
                }
            }
        }
    });

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }

    // Save a recording in progress instead of losing it on logout or shutdown.
    let stopped = tokio::task::spawn_blocking(move || {
        if recorder.status().state == State::Idle {
            return Ok(());
        }
        recorder.stop().map(drop)
    })
    .await?;
    if let Err(e) = stopped {
        eprintln!("earshotd: {e}");
    }
    let _ = quit_tx.send(());
    forwarder.await?;
    Ok(())
}
