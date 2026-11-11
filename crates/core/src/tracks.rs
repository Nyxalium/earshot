//! Picking the tracks out of a two-track recording.
//!
//! A recording of both sources has four channels: the microphone on the front
//! pair and system audio on the back pair. These mixes turn that into stereo
//! for listening to, or saving, one track or both.

use std::path::Path;
use std::time::Duration;

use gst::prelude::*;
use gstreamer as gst;

use crate::library::{Library, Meta};
use crate::{Error, Format, Result, Source, named_enum};

named_enum! {
    /// Which tracks of a two-track recording to hear.
    Tracks {
        Both => "both",
        Microphone => "microphone",
        System => "system",
    }
}

impl Default for Tracks {
    fn default() -> Self {
        Tracks::Both
    }
}

impl Tracks {
    /// An `audioconvert` mix matrix from four channels to stereo. Rows are the
    /// left and right outputs; columns are the front left, front right, back
    /// left and back right inputs.
    pub fn mix_matrix(self) -> &'static str {
        match self {
            // Both at -3 dB, so speech on both tracks at once rarely clips.
            Tracks::Both => {
                "<<(float)0.7,(float)0,(float)0.7,(float)0>,\
                  <(float)0,(float)0.7,(float)0,(float)0.7>>"
            }
            Tracks::Microphone => {
                "<<(float)1,(float)0,(float)0,(float)0>,\
                  <(float)0,(float)1,(float)0,(float)0>>"
            }
            Tracks::System => {
                "<<(float)0,(float)0,(float)1,(float)0>,\
                  <(float)0,(float)0,(float)0,(float)1>>"
            }
        }
    }

    /// GStreamer elements that take a two-track recording's four channels and
    /// give stereo with only these tracks in it.
    pub fn stereo_mix(self) -> String {
        format!(
            "audioconvert mix-matrix=\"{}\" ! audio/x-raw,channels=2",
            self.mix_matrix()
        )
    }
}

/// Longest an export may take before it is abandoned.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);

/// Saves the chosen tracks of a two-track recording as a stereo file in
/// `format`. A partly written file is removed if the export fails.
pub fn export(source: &Path, tracks: Tracks, destination: &Path, format: Format) -> Result<()> {
    gst::init().map_err(|e| Error::Pipeline(format!("GStreamer could not be initialised: {e}")))?;
    if source == destination {
        return Err(Error::Invalid(
            "a track cannot be saved over the recording it comes from".into(),
        ));
    }

    let description = format!(
        // The caps straight after the decoder require four channels, so a
        // one-track file fails instead of being upmixed into a fake split.
        "filesrc name=source ! decodebin ! audio/x-raw,channels=4 ! {} ! \
         audioconvert ! audioresample ! {} ! filesink name=sink",
        tracks.stereo_mix(),
        format.encoder(),
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| Error::Pipeline(format!("the export pipeline could not be built: {e}")))?
        .downcast::<gst::Pipeline>()
        .expect("a pipeline description parses to a pipeline");
    let element = |name: &str| {
        pipeline
            .by_name(name)
            .expect("the description names this element")
    };
    element("source").set_property("location", source);
    element("sink").set_property("location", destination);

    let result = run_to_end(&pipeline);
    let _ = pipeline.set_state(gst::State::Null);
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
        return result;
    }
    copy_waveform(source, tracks, destination);
    Ok(())
}

/// A track saved next to its recording gets that track's length and
/// waveform, so the list shows them. Elsewhere no metadata folder is created.
fn copy_waveform(source: &Path, tracks: Tracks, destination: &Path) {
    let (Some(dir), Some(destination_dir)) = (source.parent(), destination.parent()) else {
        return;
    };
    if dir != destination_dir {
        return;
    }
    let library = Library::new(dir);
    let Ok(Some(meta)) = library.meta(source) else {
        return;
    };
    let (source, microphone_peaks, system_peaks) = match tracks {
        Tracks::Microphone => (Source::Microphone, meta.microphone_peaks, Vec::new()),
        Tracks::System => (Source::System, Vec::new(), meta.system_peaks),
        Tracks::Both => return,
    };
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(meta.created, |d| d.as_secs());
    let copied = Meta {
        created,
        duration_ms: meta.duration_ms,
        source: Some(source),
        microphone_peaks,
        system_peaks,
    };
    // The audio is saved; missing metadata only costs the waveform.
    if let Err(e) = library.write_meta(destination, &copied) {
        eprintln!("earshot: {e}");
    }
}

fn run_to_end(pipeline: &gst::Pipeline) -> Result<()> {
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| Error::Pipeline("the export could not start".into()))?;
    let bus = pipeline.bus().expect("a pipeline has a bus");
    let timeout = gst::ClockTime::from_seconds(EXPORT_TIMEOUT.as_secs());
    let message = bus
        .timed_pop_filtered(timeout, &[gst::MessageType::Eos, gst::MessageType::Error])
        .ok_or_else(|| Error::Pipeline("the export timed out".into()))?;
    match message.view() {
        gst::MessageView::Error(error) => {
            let text = error.error().message().to_owned();
            let debug = error.debug().map(|d| d.to_string()).unwrap_or_default();
            // A file without four channels fails to negotiate; say why.
            let text = if debug.contains("not-negotiated") || text.contains("negotiat") {
                "only recordings of both the microphone and system audio have separate tracks"
                    .to_owned()
            } else {
                text
            };
            Err(Error::Pipeline(text))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a FLAC file whose front pair holds a tone and whose back pair
    /// holds a quieter tone, like a microphone and system recording.
    fn two_track_file(path: &Path) {
        gst::init().unwrap();
        let description = format!(
            "audiomixer name=mix ! audio/x-raw,rate=48000,channels=4,channel-mask=(bitmask)0x33 ! \
             flacenc ! filesink location=\"{}\"  \
             audiotestsrc num-buffers=20 volume=0.8 ! audio/x-raw,channels=2 ! \
             audioconvert mix-matrix=\"<<(float)1,(float)0>,<(float)0,(float)1>,<(float)0,(float)0>,<(float)0,(float)0>>\" ! \
             audio/x-raw,rate=48000,channels=4,channel-mask=(bitmask)0x33 ! mix.  \
             audiotestsrc num-buffers=20 volume=0.2 freq=880 ! audio/x-raw,channels=2 ! \
             audioconvert mix-matrix=\"<<(float)0,(float)0>,<(float)0,(float)0>,<(float)1,(float)0>,<(float)0,(float)1>>\" ! \
             audio/x-raw,rate=48000,channels=4,channel-mask=(bitmask)0x33 ! mix.",
            path.display()
        );
        let pipeline = gst::parse::launch(&description).unwrap();
        let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();
        run_to_end(&pipeline).unwrap();
        pipeline.set_state(gst::State::Null).unwrap();
    }

    /// The loudest sample of a file, from 0 to 1, and its channel count.
    fn loudness(path: &Path) -> (f64, i32) {
        let description = format!(
            "filesrc location=\"{}\" ! decodebin ! audioconvert ! \
             audio/x-raw,format=F32LE ! level name=level interval=1000000000000 post-messages=true ! fakesink",
            path.display()
        );
        let pipeline = gst::parse::launch(&description)
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let mut peak = f64::NEG_INFINITY;
        let mut channels = 0;
        loop {
            let message = bus
                .timed_pop(gst::ClockTime::from_seconds(10))
                .expect("the file plays to the end");
            match message.view() {
                gst::MessageView::Element(element) => {
                    let s = element.structure().unwrap();
                    if s.name() == "level" {
                        let values = s.get::<glib::ValueArray>("peak").unwrap();
                        channels = values.len() as i32;
                        for value in values.iter() {
                            peak = peak.max(value.get::<f64>().unwrap());
                        }
                    }
                }
                gst::MessageView::Eos(_) => break,
                gst::MessageView::Error(e) => panic!("{}", e.error()),
                _ => {}
            }
        }
        pipeline.set_state(gst::State::Null).unwrap();
        (10f64.powf(peak / 20.0), channels)
    }

    use gst::glib;

    #[test]
    fn exports_each_track_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("both.flac");
        two_track_file(&source);

        let mut results = Vec::new();
        for tracks in [Tracks::Microphone, Tracks::System, Tracks::Both] {
            let out = dir.path().join(format!("{tracks}.wav"));
            export(&source, tracks, &out, Format::Wav).unwrap();
            results.push(loudness(&out));
        }
        let [(mic, mic_channels), (system, system_channels), (both, _)] = results[..] else {
            unreachable!()
        };

        assert_eq!(
            (mic_channels, system_channels),
            (2, 2),
            "exports are stereo"
        );
        assert!(
            (0.7..0.9).contains(&mic),
            "microphone track at its own level: {mic}"
        );
        assert!(
            (0.15..0.25).contains(&system),
            "system track at its own level: {system}"
        );
        assert!(
            both > mic * 0.6 && both < 1.0,
            "both tracks mixed without clipping: {both}"
        );
    }

    #[test]
    fn a_track_saved_beside_its_recording_keeps_that_tracks_waveform() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("both.flac");
        two_track_file(&source);
        let library = Library::new(dir.path());
        let meta = Meta {
            created: 100,
            duration_ms: 400,
            source: Some(Source::Both),
            microphone_peaks: vec![0.9, 0.8],
            system_peaks: vec![0.1, 0.2],
        };
        library.write_meta(&source, &meta).unwrap();

        let beside = dir.path().join("system.flac");
        export(&source, Tracks::System, &beside, Format::Flac).unwrap();
        let copied = library
            .meta(&beside)
            .unwrap()
            .expect("metadata beside the recording");
        assert_eq!(copied.source, Some(Source::System));
        assert_eq!(copied.system_peaks, [0.1, 0.2]);
        assert!(copied.microphone_peaks.is_empty());
        assert_eq!(copied.duration_ms, 400);

        let elsewhere = tempfile::tempdir().unwrap();
        let away = elsewhere.path().join("mic.flac");
        export(&source, Tracks::Microphone, &away, Format::Flac).unwrap();
        assert!(!elsewhere.path().join(".earshot").exists());
    }

    #[test]
    fn a_single_track_recording_cannot_be_split_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("mono.flac");
        gst::init().unwrap();
        let pipeline = gst::parse::launch(&format!(
            "audiotestsrc num-buffers=10 ! audio/x-raw,channels=1 ! flacenc ! filesink location=\"{}\"",
            source.display()
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        run_to_end(&pipeline).unwrap();
        pipeline.set_state(gst::State::Null).unwrap();

        let out = dir.path().join("mic.wav");
        let error = export(&source, Tracks::Microphone, &out, Format::Wav).unwrap_err();
        assert!(error.to_string().contains("separate tracks"), "{error}");
        assert!(!out.exists());
        assert!(export(&source, Tracks::Microphone, &source, Format::Flac).is_err());
        assert!(source.exists());
    }
}
