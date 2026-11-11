//! The window's connection to the Earshot service.
//!
//! D-Bus work runs on a small Tokio runtime of its own; GTK's main loop only
//! awaits the results.

use std::future::Future;
use std::sync::Arc;

use earshot_client::{EarshotProxy, Status};
use futures_util::StreamExt;
use tokio::sync::mpsc;

/// Something the service reported.
pub enum Event {
    State(Status),
    Level {
        microphone: f32,
        system: f32,
        elapsed_ms: u64,
    },
    Failed(String),
    RecordingsChanged,
    /// The service is no longer on the session bus.
    ServiceStopped,
}

#[derive(Clone)]
pub struct Backend {
    runtime: Arc<tokio::runtime::Runtime>,
    proxy: EarshotProxy<'static>,
}

impl Backend {
    /// Connects to the service, starting it if needed. Its events arrive on
    /// the returned channel.
    pub fn connect() -> Result<(Self, mpsc::UnboundedReceiver<Event>), String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let proxy = runtime
            .block_on(earshot_client::connect())
            .map_err(|e| earshot_client::error_message(&e))?;

        let (events_tx, events) = mpsc::unbounded_channel();
        runtime.spawn({
            let proxy = proxy.clone();
            async move {
                if let Err(e) = forward(proxy, events_tx).await {
                    eprintln!("earshot: lost the service's events: {e}");
                }
            }
        });
        Ok((
            Self {
                runtime: Arc::new(runtime),
                proxy,
            },
            events,
        ))
    }

    /// Makes a call to the service and returns its result, with errors as
    /// the message to show.
    pub async fn call<T, F, Fut>(&self, call: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(EarshotProxy<'static>) -> Fut + Send + 'static,
        Fut: Future<Output = zbus::Result<T>> + Send,
    {
        let proxy = self.proxy.clone();
        self.runtime
            .spawn(async move {
                // The service may have stopped since the last call.
                earshot_client::ensure_running(proxy.inner().connection()).await?;
                call(proxy).await
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| earshot_client::error_message(&e))
    }
}

async fn forward(
    proxy: EarshotProxy<'static>,
    events: mpsc::UnboundedSender<Event>,
) -> zbus::Result<()> {
    let mut state = proxy.receive_state_changed().await?;
    let mut level = proxy.receive_level().await?;
    let mut failed = proxy.receive_recording_failed().await?;
    let mut changed = proxy.receive_recordings_changed().await?;
    let mut owners = proxy.inner().receive_owner_changed().await?;
    loop {
        let event = tokio::select! {
            // In order, so what the service said last is seen before its going.
            biased;
            Some(signal) = state.next() => Event::State(signal.args()?.status),
            Some(signal) = level.next() => {
                let args = signal.args()?;
                Event::Level {
                    microphone: args.microphone as f32,
                    system: args.system as f32,
                    elapsed_ms: args.elapsed_ms,
                }
            }
            Some(signal) = failed.next() => Event::Failed(signal.args()?.message.to_owned()),
            Some(_) = changed.next() => Event::RecordingsChanged,
            Some(owner) = owners.next() => match owner {
                None => Event::ServiceStopped,
                // Running again; its signals say what it is doing.
                Some(_) => continue,
            },
            else => return Ok(()),
        };
        if events.send(event).is_err() {
            // The window has closed.
            return Ok(());
        }
    }
}
