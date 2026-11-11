//! Plays saved recordings.

use std::cell::Cell;
use std::path::Path;

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
    rate: Cell<f64>,
    playing: Cell<bool>,
    _watch: gst::bus::BusWatchGuard,
}

impl Player {
    pub fn new(
        on_event: impl Fn(PlayerEvent) + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        gst::init()?;
        // A recording of both sources has four channels; mixing down to stereo
        // here plays both tracks on any output device.
        let sink = gst::parse::bin_from_description(
            "audioconvert ! audio/x-raw,channels=2 ! autoaudiosink",
            true,
        )?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("audio-sink", &sink)
            // Keeps voices at their normal pitch when the speed changes.
            .property(
                "audio-filter",
                gst::ElementFactory::make("scaletempo").build()?,
            )
            .build()?;
        playbin.set_property_from_str("flags", "audio");

        let bus = playbin.bus().expect("playbin has a bus");
        let watch = bus.add_watch_local(move |_, message| {
            match message.view() {
                gst::MessageView::AsyncDone(_) => on_event(PlayerEvent::Loaded),
                gst::MessageView::Eos(_) => on_event(PlayerEvent::Ended),
                gst::MessageView::Error(error) => {
                    on_event(PlayerEvent::Failed(error.error().message().to_owned()));
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })?;

        Ok(Self {
            playbin,
            rate: Cell::new(1.0),
            playing: Cell::new(false),
            _watch: watch,
        })
    }

    /// Opens a recording, paused at its start and at normal speed.
    pub fn load(&self, path: &Path) -> Result<(), String> {
        self.unload();
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
        // A seek can fail while a recording is still opening; the next one works.
        let _ = self.playbin.seek(
            self.rate.get(),
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            gst::ClockTime::from_mseconds(ms),
            gst::SeekType::None,
            gst::ClockTime::NONE,
        );
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
