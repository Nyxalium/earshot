//! Records audio to a file with GStreamer.
//!
//! The recorder is only ever in the `Recording` or `Paused` state while a
//! pipeline is confirmed to be running. A recording that cannot start returns
//! an error from [`Recorder::start`], and one that breaks later reports
//! [`Event::Failed`] and returns to `Idle`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gst::glib;
use gst::prelude::*;
use gstreamer as gst;

use crate::library::{Library, Meta, downsample_peaks};
use crate::{Error, Result, named_enum};

/// How often the loudness of the input is measured.
const LEVEL_INTERVAL: Duration = Duration::from_millis(50);
/// Longest wait for the pipeline to start, or to finish writing the file.
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
/// Number of peaks kept per recording for drawing its waveform.
const STORED_PEAKS: usize = 2000;

const CONVERT: &str = "audioconvert ! audioresample";

/// GStreamer descriptions of the two audio inputs.
struct Inputs<'a> {
    microphone: &'a str,
    system: &'a str,
}

const DEVICES: Inputs<'static> = Inputs {
    microphone: "pulsesrc client-name=Earshot",
    system: "pulsesrc client-name=Earshot device=@DEFAULT_MONITOR@",
};

// Recording both sources keeps them as two tracks inside one ordinary file: a
// four-channel stream with the microphone on the front pair and system audio
// on the back pair. Any player plays that as a mix, and the two stay separable.
const FRONT_PAIR: &str =
    "<<(float)1,(float)0>,<(float)0,(float)1>,<(float)0,(float)0>,<(float)0,(float)0>>";
const BACK_PAIR: &str =
    "<<(float)0,(float)0>,<(float)0,(float)0>,<(float)1,(float)0>,<(float)0,(float)1>>";

/// The four-channel layout both pairs are mixed into. It is stated again
/// after the mixer, which would otherwise mix everything down to mono.
const FOUR_CHANNELS: &str = "audio/x-raw,rate=48000,channels=4,channel-mask=(bitmask)0x33";

/// Converts a source to stereo and places it on one pair of four channels.
fn on_pair(matrix: &str) -> String {
    format!(
        "audioconvert ! audio/x-raw,channels=2 ! audioresample ! \
         audioconvert mix-matrix=\"{matrix}\" ! {FOUR_CHANNELS}"
    )
}

named_enum! {
    /// Where the audio comes from.
    Source {
        /// The default input device.
        Microphone => "microphone",
        /// Whatever the default output device is playing.
        System => "system",
        /// Microphone and system audio mixed together.
        Both => "both",
    }
}

named_enum! {
    /// File format of a recording. The name is also the file extension.
    Format {
        Opus => "opus",
        Flac => "flac",
        Wav => "wav",
    }
}

impl Format {
    pub fn extension(self) -> &'static str {
        self.as_str()
    }

    pub(crate) fn encoder(self) -> &'static str {
        match self {
            Format::Opus => "opusenc ! oggmux",
            Format::Flac => "flacenc",
            Format::Wav => "wavenc",
        }
    }
}

named_enum! {
    State {
        Idle => "idle",
        Recording => "recording",
        Paused => "paused",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub state: State,
    /// Length of the audio recorded so far; time spent paused is not counted.
    pub elapsed_ms: u64,
    pub path: Option<PathBuf>,
    pub source: Option<Source>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Started {
        path: PathBuf,
    },
    Paused,
    Resumed,
    /// The recording was saved.
    Stopped {
        path: PathBuf,
    },
    /// The recording was discarded on request.
    Cancelled,
    /// The recording ended because of an error. `path` is set if some audio
    /// was saved before it happened.
    Failed {
        message: String,
        path: Option<PathBuf>,
    },
    /// Loudness of each source from 0 to 1 and the length recorded so far,
    /// sent about 20 times a second while recording. A source that is not
    /// being recorded stays at 0.
    Level {
        microphone: f32,
        system: f32,
        elapsed_ms: u64,
    },
}

#[derive(Clone, Debug)]
pub struct StartOptions {
    pub source: Source,
    pub format: Format,
    /// Folder the recording is saved in; created if it does not exist.
    pub dir: PathBuf,
}

pub struct Recorder {
    shared: Arc<Shared>,
}

/// Loudness over time of each source being recorded.
#[derive(Default)]
struct Peaks {
    microphone: Vec<f32>,
    system: Vec<f32>,
}

struct Shared {
    active: Mutex<Option<Active>>,
    on_event: Box<dyn Fn(Event) + Send + Sync>,
}

struct Active {
    pipeline: gst::Pipeline,
    path: PathBuf,
    dir: PathBuf,
    source: Source,
    created: u64,
    paused: bool,
    peaks: Arc<Mutex<Peaks>>,
    /// Tells the watcher thread to exit.
    done: Arc<AtomicBool>,
    /// Receives how the pipeline ended: cleanly, or with an error message.
    ended: mpsc::Receiver<std::result::Result<(), String>>,
}

impl Recorder {
    /// Creates a recorder. `on_event` is called from background threads and
    /// must not call back into the recorder.
    pub fn new(on_event: impl Fn(Event) + Send + Sync + 'static) -> Result<Self> {
        gst::init()
            .map_err(|e| Error::Start(format!("GStreamer could not be initialised: {e}")))?;
        Ok(Self {
            shared: Arc::new(Shared {
                active: Mutex::new(None),
                on_event: Box::new(on_event),
            }),
        })
    }

    pub fn status(&self) -> Status {
        match &*self.shared.lock() {
            None => Status {
                state: State::Idle,
                elapsed_ms: 0,
                path: None,
                source: None,
            },
            Some(active) => Status {
                state: if active.paused {
                    State::Paused
                } else {
                    State::Recording
                },
                elapsed_ms: active
                    .pipeline
                    .query_position::<gst::ClockTime>()
                    .map_or(0, |t| t.mseconds()),
                path: Some(active.path.clone()),
                source: Some(active.source),
            },
        }
    }

    /// Starts a recording and returns the file it is being written to.
    pub fn start(&self, options: StartOptions) -> Result<PathBuf> {
        self.start_from(&DEVICES, options)
    }

    /// Starts a recording from the given GStreamer source description.
    fn start_from(&self, inputs: &Inputs, options: StartOptions) -> Result<PathBuf> {
        let mut guard = self.shared.lock();
        if guard.is_some() {
            return Err(Error::AlreadyRecording);
        }

        fs::create_dir_all(&options.dir).map_err(|e| {
            let dir = options.dir.display();
            Error::Start(match fs::read_link(&options.dir) {
                Ok(target) => format!(
                    "the recordings folder {dir} is a link to {}, which does not exist",
                    target.display()
                ),
                Err(_) => format!("the recordings folder {dir} could not be created: {e}"),
            })
        })?;
        let path = new_recording_path(&options.dir, options.format)?;

        let source = options.source;
        let Inputs { microphone, system } = inputs;
        let sources = match source {
            Source::Microphone => microphone.to_string(),
            Source::System => system.to_string(),
            Source::Both => format!(
                "audiomixer name=mix  {microphone} ! {} ! mix.  {system} ! {} ! mix.  \
                 mix. ! {FOUR_CHANNELS}",
                on_pair(FRONT_PAIR),
                on_pair(BACK_PAIR),
            ),
        };
        let description = format!(
            "{sources} ! {CONVERT} ! level name=level interval={} ! {} ! filesink name=sink",
            LEVEL_INTERVAL.as_nanos(),
            options.format.encoder(),
        );
        let pipeline = gst::parse::launch(&description)
            .map_err(|e| Error::Start(format!("the audio pipeline could not be built: {e}")))?
            .downcast::<gst::Pipeline>()
            .expect("a pipeline description parses to a pipeline");
        pipeline
            .by_name("sink")
            .expect("the description names the file sink")
            .set_property("location", &path);

        let bus = pipeline.bus().expect("a pipeline has a bus");
        let started = pipeline.set_state(gst::State::Playing).is_ok()
            && pipeline
                .state(gst::ClockTime::from_mseconds(
                    STATE_TIMEOUT.as_millis() as u64
                ))
                .0
                .is_ok();
        if !started {
            let message = bus
                .pop_filtered(&[gst::MessageType::Error])
                .and_then(|m| error_text(&m))
                .unwrap_or_else(|| "the audio pipeline did not start".to_owned());
            let _ = pipeline.set_state(gst::State::Null);
            let _ = fs::remove_file(&path);
            return Err(Error::Start(message));
        }

        let peaks = Arc::new(Mutex::new(Peaks::default()));
        let done = Arc::new(AtomicBool::new(false));
        let (ended_tx, ended) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("earshot-recorder".into())
            .spawn({
                let (shared, pipeline) = (self.shared.clone(), pipeline.clone());
                let (peaks, done) = (peaks.clone(), done.clone());
                move || watch(&shared, &pipeline, &bus, source, &peaks, &done, &ended_tx)
            })
            .map_err(|e| Error::Start(format!("could not start the recorder thread: {e}")))?;

        *guard = Some(Active {
            pipeline,
            path: path.clone(),
            dir: options.dir,
            source,
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            paused: false,
            peaks,
            done,
            ended,
        });
        drop(guard);
        self.shared.emit(Event::Started { path: path.clone() });
        Ok(path)
    }

    pub fn pause(&self) -> Result<()> {
        self.set_paused(true)
    }

    pub fn resume(&self) -> Result<()> {
        self.set_paused(false)
    }

    fn set_paused(&self, paused: bool) -> Result<()> {
        let mut guard = self.shared.lock();
        let active = guard.as_mut().ok_or(Error::NotRecording)?;
        if active.paused == paused {
            return Ok(());
        }
        let (state, verb) = if paused {
            (gst::State::Paused, "pause")
        } else {
            (gst::State::Playing, "resume")
        };
        active
            .pipeline
            .set_state(state)
            .map_err(|_| Error::Pipeline(format!("could not {verb} the recording")))?;
        active.paused = paused;
        drop(guard);
        self.shared.emit(if paused {
            Event::Paused
        } else {
            Event::Resumed
        });
        Ok(())
    }

    /// Finishes the recording and returns the saved file.
    pub fn stop(&self) -> Result<PathBuf> {
        let active = self.shared.lock().take().ok_or(Error::NotRecording)?;

        // The end-of-stream marker only travels through a running pipeline,
        // and the encoder needs it to finish the file properly.
        if active.paused {
            let _ = active.pipeline.set_state(gst::State::Playing);
        }
        active.pipeline.send_event(gst::event::Eos::new());
        let failure = match active.ended.recv_timeout(STATE_TIMEOUT) {
            Ok(Ok(())) => None,
            Ok(Err(message)) => Some(message),
            Err(_) => Some("timed out while finishing the file".to_owned()),
        };
        self.shared.finish(active, failure)
    }

    /// Discards the recording in progress.
    pub fn cancel(&self) -> Result<()> {
        let active = self.shared.lock().take().ok_or(Error::NotRecording)?;
        active.done.store(true, Ordering::Relaxed);
        let _ = active.pipeline.set_state(gst::State::Null);
        let removed = match fs::remove_file(&active.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::io(format!(
                "could not remove {}",
                active.path.display()
            ))(e)),
            _ => Ok(()),
        };
        self.shared.emit(Event::Cancelled);
        removed
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, event: Event) {
        (self.on_event)(event);
    }

    /// Shuts down a recording that has already been taken out of `active`,
    /// saves what was recorded and reports how it ended.
    fn finish(&self, active: Active, failure: Option<String>) -> Result<PathBuf> {
        active.done.store(true, Ordering::Relaxed);
        let peaks = std::mem::take(&mut *active.peaks.lock().unwrap_or_else(|e| e.into_inner()));
        let duration_ms = active.pipeline.query_position::<gst::ClockTime>().map_or(
            peaks.microphone.len().max(peaks.system.len()) as u64
                * LEVEL_INTERVAL.as_millis() as u64,
            |t| t.mseconds(),
        );
        let _ = active.pipeline.set_state(gst::State::Null);

        let has_audio = fs::metadata(&active.path).is_ok_and(|m| m.len() > 0);
        let saved = if has_audio {
            let meta = Meta {
                created: active.created,
                duration_ms,
                source: Some(active.source),
                microphone_peaks: downsample_peaks(&peaks.microphone, STORED_PEAKS),
                system_peaks: downsample_peaks(&peaks.system, STORED_PEAKS),
            };
            // The audio is safe; losing its metadata only costs the waveform.
            if let Err(e) = Library::new(&active.dir).write_meta(&active.path, &meta) {
                eprintln!("earshot: {e}");
            }
            Some(active.path)
        } else {
            let _ = fs::remove_file(&active.path);
            None
        };

        match (failure, saved) {
            (None, Some(path)) => {
                self.emit(Event::Stopped { path: path.clone() });
                Ok(path)
            }
            (failure, path) => {
                let message = failure.unwrap_or_else(|| "no audio was recorded".to_owned());
                self.emit(Event::Failed {
                    message: message.clone(),
                    path,
                });
                Err(Error::Pipeline(message))
            }
        }
    }

    /// Called by the watcher thread when the pipeline ends without `stop`
    /// having asked for it: the source ran out, or something broke.
    fn ended_unrequested(&self, pipeline: &gst::Pipeline, failure: Option<String>) {
        let active = {
            let mut guard = self.lock();
            if guard.as_ref().is_some_and(|a| &a.pipeline == pipeline) {
                guard.take()
            } else {
                None
            }
        };
        if let Some(active) = active {
            // The outcome reaches listeners through the emitted event.
            let _ = self.finish(active, failure);
        }
    }
}

/// Follows the pipeline's messages for the lifetime of one recording.
fn watch(
    shared: &Shared,
    pipeline: &gst::Pipeline,
    bus: &gst::Bus,
    source: Source,
    peaks: &Mutex<Peaks>,
    done: &AtomicBool,
    ended: &mpsc::SyncSender<std::result::Result<(), String>>,
) {
    use gst::MessageView;

    while !done.load(Ordering::Relaxed) {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(250)) else {
            continue;
        };
        match message.view() {
            MessageView::Element(element) => {
                if let Some(channels) = element.structure().and_then(level_peaks) {
                    let (microphone, system) = split_tracks(source, &channels);
                    {
                        let mut peaks = peaks.lock().unwrap_or_else(|e| e.into_inner());
                        if source != Source::System {
                            peaks.microphone.push(microphone);
                        }
                        if source != Source::Microphone {
                            peaks.system.push(system);
                        }
                    }
                    shared.emit(Event::Level {
                        microphone,
                        system,
                        elapsed_ms: pipeline
                            .query_position::<gst::ClockTime>()
                            .map_or(0, |t| t.mseconds()),
                    });
                }
            }
            MessageView::Eos(_) => {
                let _ = ended.try_send(Ok(()));
                shared.ended_unrequested(pipeline, None);
                break;
            }
            MessageView::Error(_) => {
                let text = error_text(&message).unwrap_or_default();
                let _ = ended.try_send(Err(text.clone()));
                shared.ended_unrequested(pipeline, Some(text));
                break;
            }
            _ => {}
        }
    }
}

/// Reads the loudness of each channel, from 0 to 1, out of a `level` element
/// message.
fn level_peaks(structure: &gst::StructureRef) -> Option<Vec<f32>> {
    if structure.name() != "level" {
        return None;
    }
    let channels = structure.get::<glib::ValueArray>("peak").ok()?;
    Some(
        channels
            .iter()
            .filter_map(|v| v.get::<f64>().ok())
            .map(|decibels| 10f64.powf(decibels / 20.0).clamp(0.0, 1.0) as f32)
            .collect(),
    )
}

/// Turns per-channel loudness into the loudness of (microphone, system).
fn split_tracks(source: Source, channels: &[f32]) -> (f32, f32) {
    let loudest = |channels: &[f32]| channels.iter().copied().fold(0.0, f32::max);
    match source {
        Source::Microphone => (loudest(channels), 0.0),
        Source::System => (0.0, loudest(channels)),
        Source::Both => {
            let (front, back) = channels.split_at(channels.len() / 2);
            (loudest(front), loudest(back))
        }
    }
}

fn error_text(message: &gst::Message) -> Option<String> {
    match message.view() {
        gst::MessageView::Error(error) => Some(error.error().message().to_owned()),
        _ => None,
    }
}

/// Names a new recording after the current local time.
fn new_recording_path(dir: &Path, format: Format) -> Result<PathBuf> {
    let stamp = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y-%m-%d %H.%M.%S"))
        .map_err(|e| Error::Start(format!("could not read the local time: {e}")))?;
    let extension = format.extension();
    let mut path = dir.join(format!("{stamp}.{extension}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stamp} ({n}).{extension}"));
        n += 1;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const TEST_TONE: &str = "audiotestsrc is-live=true";

    fn recorder() -> (Recorder, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let recorder = Recorder::new(move |event| {
            if !matches!(event, Event::Level { .. }) {
                let _ = tx.send(event);
            }
        })
        .unwrap();
        (recorder, rx)
    }

    /// Uses the same test source for the microphone and for system audio.
    fn both(source: &str) -> Inputs<'_> {
        Inputs {
            microphone: source,
            system: source,
        }
    }

    fn options(dir: &Path, format: Format) -> StartOptions {
        StartOptions {
            source: Source::Microphone,
            format,
            dir: dir.to_owned(),
        }
    }

    fn next(events: &mpsc::Receiver<Event>) -> Event {
        events
            .recv_timeout(Duration::from_secs(10))
            .expect("an event")
    }

    fn files_in(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect()
    }

    #[test]
    fn records_pauses_and_saves_each_format() {
        for format in [Format::Opus, Format::Flac, Format::Wav] {
            let dir = tempfile::tempdir().unwrap();
            let (recorder, events) = recorder();

            let path = recorder
                .start_from(&both(TEST_TONE), options(dir.path(), format))
                .unwrap();
            assert_eq!(next(&events), Event::Started { path: path.clone() });
            assert_eq!(recorder.status().state, State::Recording);
            assert!(matches!(
                recorder.start_from(&both(TEST_TONE), options(dir.path(), format)),
                Err(Error::AlreadyRecording)
            ));

            std::thread::sleep(Duration::from_millis(400));
            recorder.pause().unwrap();
            assert_eq!(recorder.status().state, State::Paused);
            assert_eq!(next(&events), Event::Paused);
            recorder.resume().unwrap();
            assert_eq!(next(&events), Event::Resumed);
            std::thread::sleep(Duration::from_millis(200));

            let saved = recorder.stop().unwrap();
            assert_eq!(saved, path);
            assert_eq!(next(&events), Event::Stopped { path: path.clone() });
            assert_eq!(recorder.status().state, State::Idle);
            assert_eq!(path.extension().unwrap(), format.extension());
            assert!(fs::metadata(&path).unwrap().len() > 0);

            let meta = Library::new(dir.path())
                .meta(&path)
                .unwrap()
                .expect("metadata");
            assert!(
                meta.duration_ms >= 400,
                "{format}: duration {}",
                meta.duration_ms
            );
            assert!(
                meta.microphone_peaks.len() >= 5,
                "{format}: {} peaks",
                meta.microphone_peaks.len()
            );
            assert!(
                meta.microphone_peaks.iter().any(|p| *p > 0.1),
                "{format}: silent peaks"
            );
        }
    }

    #[test]
    fn a_recording_that_cannot_start_is_an_error_and_leaves_the_recorder_idle() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let (recorder, events) = recorder();

        let error = recorder
            .start_from(&both(TEST_TONE), options(&locked, Format::Opus))
            .expect_err("the file cannot be opened for writing");
        assert!(
            matches!(&error, Error::Start(m) if m.contains("for writing")),
            "{error}"
        );
        assert_eq!(recorder.status().state, State::Idle);
        assert!(
            events.try_recv().is_err(),
            "no Started event for a failed start"
        );
        assert!(matches!(recorder.stop(), Err(Error::NotRecording)));
        assert!(matches!(recorder.cancel(), Err(Error::NotRecording)));

        // The recorder is still usable afterwards.
        recorder
            .start_from(&both(TEST_TONE), options(dir.path(), Format::Opus))
            .unwrap();
        recorder.cancel().unwrap();
    }

    #[test]
    fn a_folder_that_cannot_be_created_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let dangling = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path().join("missing"), &dangling).unwrap();
        let (recorder, _events) = recorder();

        let error = recorder
            .start_from(&both(TEST_TONE), options(&dangling, Format::Opus))
            .expect_err("a dangling symlink is not a usable folder");
        assert!(
            matches!(&error, Error::Start(m) if m.contains("missing, which does not exist")),
            "{error}"
        );
        assert_eq!(recorder.status().state, State::Idle);
    }

    #[test]
    fn cancel_discards_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (recorder, events) = recorder();

        let path = recorder
            .start_from(&both(TEST_TONE), options(dir.path(), Format::Flac))
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        recorder.cancel().unwrap();

        assert_eq!(next(&events), Event::Started { path });
        assert_eq!(next(&events), Event::Cancelled);
        assert_eq!(recorder.status().state, State::Idle);
        assert_eq!(files_in(dir.path()), Vec::<PathBuf>::new());
    }

    #[test]
    fn an_error_while_recording_is_reported_and_keeps_the_audio() {
        let dir = tempfile::tempdir().unwrap();
        let (recorder, events) = recorder();

        let breaks = format!("{TEST_TONE} ! identity error-after=20");
        let path = recorder
            .start_from(&both(&breaks), options(dir.path(), Format::Opus))
            .unwrap();

        assert_eq!(next(&events), Event::Started { path: path.clone() });
        match next(&events) {
            Event::Failed {
                message,
                path: saved,
            } => {
                assert!(!message.is_empty());
                assert_eq!(saved, Some(path.clone()));
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(recorder.status().state, State::Idle);
        assert!(fs::metadata(&path).unwrap().len() > 0);
    }

    #[test]
    fn a_source_that_ends_by_itself_saves_the_recording() {
        let dir = tempfile::tempdir().unwrap();
        let (recorder, events) = recorder();

        let finite = format!("{TEST_TONE} num-buffers=20");
        let path = recorder
            .start_from(&both(&finite), options(dir.path(), Format::Wav))
            .unwrap();

        assert_eq!(next(&events), Event::Started { path: path.clone() });
        assert_eq!(next(&events), Event::Stopped { path: path.clone() });
        assert_eq!(recorder.status().state, State::Idle);
        assert!(Library::new(dir.path()).meta(&path).unwrap().is_some());
    }

    #[test]
    fn both_sources_are_kept_as_separate_tracks() {
        const TONE: &str = "audiotestsrc is-live=true";
        const SILENCE: &str = "audiotestsrc is-live=true wave=silence";
        let loudest = |peaks: &[f32]| peaks.iter().copied().fold(0.0, f32::max);

        for (microphone, system) in [(TONE, SILENCE), (SILENCE, TONE)] {
            let dir = tempfile::tempdir().unwrap();
            let (recorder, _events) = recorder();
            let options = StartOptions {
                source: Source::Both,
                format: Format::Flac,
                dir: dir.path().to_owned(),
            };

            let path = recorder
                .start_from(&Inputs { microphone, system }, options)
                .unwrap();
            std::thread::sleep(Duration::from_millis(500));
            recorder.stop().unwrap();

            let meta = Library::new(dir.path()).meta(&path).unwrap().unwrap();
            let (loud, silent) = if microphone == TONE {
                (&meta.microphone_peaks, &meta.system_peaks)
            } else {
                (&meta.system_peaks, &meta.microphone_peaks)
            };
            assert!(loudest(loud) > 0.5, "the tone is on its own track");
            assert!(loudest(silent) < 0.01, "the tone did not leak: {silent:?}");
            assert_eq!(silent.len(), loud.len());

            // FLAC stores the channel count, minus one, in three bits of byte 20.
            let header = fs::read(&path).unwrap();
            assert_eq!(&header[..4], b"fLaC");
            assert_eq!(((header[20] >> 1) & 0x07) + 1, 4, "two stereo pairs");
        }
    }
}
