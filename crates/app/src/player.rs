//! Plays saved recordings.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use earshot_core::Tracks;

use gst::prelude::*;
use gstreamer as gst;
use gtk::glib;

pub enum PlayerEvent {
    /// A recording has been opened, or a seek has finished.
    Loaded,
    /// Playback reached the end of the recording.
    Ended,
    Failed(String),
}

pub struct Player {
    playbin: gst::Element,
    rate: Rc<Cell<f64>>,
    playing: Rc<Cell<bool>>,
    /// Which tracks are heard, for a two-track recording; `None` otherwise.
    tracks: Cell<Option<Tracks>>,
    /// Where to go back to, and whether to play, once the player has
    /// reopened after its output was rebuilt.
    restore: Rc<Restore>,
    _watch: gst::bus::BusWatchGuard,
}

#[derive(Default)]
struct Restore {
    position_ms: Cell<Option<u64>>,
    play: Cell<bool>,
}

/// The output: two-track recordings are mixed to stereo with the chosen
/// tracks, anything else is converted to stereo as it is.
fn output(tracks: Option<Tracks>) -> Result<gst::Bin, glib::Error> {
    let mix = match tracks {
        Some(tracks) => format!("{} ! audioconvert", tracks.stereo_mix()),
        None => "audioconvert ! audio/x-raw,channels=2".to_owned(),
    };
    gst::parse::bin_from_description(&format!("{mix} ! autoaudiosink"), true)
}

fn seek(playbin: &gst::Element, rate: f64, ms: u64) {
    // A seek can fail while a recording is still opening; the next one works.
    let _ = playbin.seek(
        rate,
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::SeekType::Set,
        gst::ClockTime::from_mseconds(ms),
        gst::SeekType::None,
        gst::ClockTime::NONE,
    );
}

impl Player {
    pub fn new(
        on_event: impl Fn(PlayerEvent) + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        gst::init()?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("audio-sink", output(None)?)
            // Keeps voices at their normal pitch when the speed changes.
            .property(
                "audio-filter",
                gst::ElementFactory::make("scaletempo").build()?,
            )
            .build()?;
        playbin.set_property_from_str("flags", "audio");

        let rate = Rc::new(Cell::new(1.0));
        let playing = Rc::new(Cell::new(false));
        let restore = Rc::new(Restore::default());
        let bus = playbin.bus().expect("playbin has a bus");
        let watch = bus.add_watch_local({
            let (playbin, rate, restore) = (playbin.clone(), rate.clone(), restore.clone());
            let playing = playing.clone();
            move |_, message| {
                match message.view() {
                    gst::MessageView::AsyncDone(_) => {
                        if let Some(position) = restore.position_ms.take() {
                            // Another AsyncDone follows once the seek lands.
                            seek(&playbin, rate.get(), position);
                            return glib::ControlFlow::Continue;
                        }
                        if restore.play.take() {
                            let _ = playbin.set_state(gst::State::Playing);
                        }
                        on_event(PlayerEvent::Loaded);
                    }
                    gst::MessageView::Eos(_) => on_event(PlayerEvent::Ended),
                    gst::MessageView::Error(error) => {
                        // An error ends playback.
                        playing.set(false);
                        on_event(PlayerEvent::Failed(error.error().message().to_owned()));
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            }
        })?;

        Ok(Self {
            playbin,
            rate,
            playing,
            tracks: Cell::new(None),
            restore,
            _watch: watch,
        })
    }

    /// Opens a recording, paused at its start and at normal speed. A
    /// two-track recording starts with both tracks heard.
    pub fn load(&self, path: &Path, two_tracks: bool) -> Result<(), String> {
        self.unload();
        let tracks = two_tracks.then_some(Tracks::Both);
        self.set_output(tracks)?;
        let uri = glib::filename_to_uri(path, None).map_err(|e| e.to_string())?;
        self.playbin.set_property("uri", uri.as_str());
        self.playbin
            .set_state(gst::State::Paused)
            .map_err(|_| format!("could not open {}", path.display()))?;
        Ok(())
    }

    pub fn unload(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.playing.set(false);
        self.rate.set(1.0);
        self.restore.position_ms.set(None);
        self.restore.play.set(false);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.get()
    }

    pub fn set_playing(&self, playing: bool) -> Result<(), String> {
        let state = if playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        self.playbin
            .set_state(state)
            .map_err(|_| "could not change playback".to_owned())?;
        self.playing.set(playing);
        Ok(())
    }

    pub fn position_ms(&self) -> Option<u64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.mseconds())
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.mseconds())
    }

    pub fn seek_ms(&self, ms: u64) {
        seek(&self.playbin, self.rate.get(), ms);
    }

    /// Which tracks of a two-track recording are heard; `None` for any
    /// other recording.
    pub fn tracks(&self) -> Option<Tracks> {
        self.tracks.get()
    }

    /// Changes which tracks are heard, keeping the position and whether it
    /// is playing. Does nothing for a recording without two tracks.
    pub fn set_tracks(&self, tracks: Tracks) -> Result<(), String> {
        if self.tracks.get().is_none_or(|current| current == tracks) {
            return Ok(());
        }
        // The output can only be swapped while the player is stopped.
        let position = self.position_ms().unwrap_or(0);
        let _ = self.playbin.set_state(gst::State::Null);
        self.set_output(Some(tracks))?;
        self.restore.position_ms.set(Some(position));
        self.restore.play.set(self.playing.get());
        self.playbin
            .set_state(gst::State::Paused)
            .map_err(|_| "could not reopen the recording".to_owned())?;
        Ok(())
    }

    fn set_output(&self, tracks: Option<Tracks>) -> Result<(), String> {
        let sink = output(tracks).map_err(|e| format!("could not set up playback: {e}"))?;
        self.playbin.set_property("audio-sink", &sink);
        self.tracks.set(tracks);
        Ok(())
    }

    pub fn set_rate(&self, rate: f64) {
        self.rate.set(rate);
        if let Some(position) = self.position_ms() {
            self.seek_ms(position);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}
