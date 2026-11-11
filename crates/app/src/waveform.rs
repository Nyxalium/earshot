//! Draws a recording's loudness over time. With two tracks, the microphone is
//! drawn above the centre line and system audio below it.

use std::cell::RefCell;
use std::rc::Rc;

use earshot_core::Tracks as Heard;
use gtk::cairo;
use gtk::prelude::*;

/// Which sources a waveform shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tracks {
    #[default]
    Microphone,
    System,
    Both,
}

impl Tracks {
    /// Reads the service's name for a source. Files Earshot did not record
    /// have none and are drawn as a single track.
    pub fn from_source(source: Option<&str>) -> Self {
        match source {
            Some("both") => Self::Both,
            Some("system") => Self::System,
            _ => Self::Microphone,
        }
    }
}

type Rgb = (f64, f64, f64);
const MICROPHONE: Rgb = (0.925, 0.925, 0.933);
const SYSTEM: Rgb = (0.557, 0.639, 1.0);
const PLAYHEAD: Rgb = (0.898, 0.282, 0.302);
/// Opacity of the part of a recording that has not been played yet.
const UNPLAYED: f64 = 0.4;
/// Opacity of a track that is not being heard, relative to the others.
const MUTED: f64 = 0.25;

/// Distance between bars, and their thickness.
const PITCH: f64 = 4.0;
const BAR: f64 = 2.0;
/// While recording, how many level updates make up one bar.
const LEVELS_PER_BAR: usize = 3;
/// Bars kept while recording; more than the widest window can show.
const LIVE_BARS: usize = 2000;

#[derive(Default)]
struct State {
    tracks: Tracks,
    microphone: Vec<f32>,
    system: Vec<f32>,
    /// While recording: the newest bar is at the right edge.
    live: bool,
    /// While recording: level updates received for the newest bar.
    pending: usize,
    /// Fraction of the recording that has been played.
    progress: f64,
    /// For a two-track recording, which tracks are being heard. The other
    /// one is drawn faintly.
    heard: Heard,
}

#[derive(Clone)]
pub struct Waveform {
    area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
}

impl Waveform {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).build();
        let state = Rc::new(RefCell::new(State::default()));
        area.set_draw_func({
            let state = state.clone();
            move |_, cr, width, height| {
                // Drawing only fails if the surface is gone; nothing to show then.
                let _ = draw(&state.borrow(), cr, width.into(), height.into());
            }
        });
        Self { area, state }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Shows a saved recording, with nothing played yet.
    pub fn show(&self, tracks: Tracks, microphone: Vec<f32>, system: Vec<f32>) {
        *self.state.borrow_mut() = State {
            tracks,
            microphone,
            system,
            ..State::default()
        };
        self.area.queue_draw();
    }

    /// Clears the waveform for a recording that is starting.
    pub fn start_live(&self, tracks: Tracks) {
        *self.state.borrow_mut() = State {
            tracks,
            live: true,
            ..State::default()
        };
        self.area.queue_draw();
    }

    /// Adds one level update to a recording in progress.
    pub fn push(&self, microphone: f32, system: f32) {
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if state.pending == 0 {
                if state.microphone.len() >= LIVE_BARS {
                    state.microphone.remove(0);
                    state.system.remove(0);
                }
                state.microphone.push(microphone);
                state.system.push(system);
            } else if let (Some(m), Some(s)) =
                (state.microphone.last_mut(), state.system.last_mut())
            {
                *m = m.max(microphone);
                *s = s.max(system);
            }
            state.pending = (state.pending + 1) % LEVELS_PER_BAR;
        }
        self.area.queue_draw();
    }

    pub fn set_heard(&self, heard: Heard) {
        self.state.borrow_mut().heard = heard;
        self.area.queue_draw();
    }

    /// Calls `menu` with the fraction from 0 to 1 and the point that was
    /// right-clicked on a saved recording's waveform.
    pub fn connect_context_menu(&self, menu: impl Fn(f64, f64, f64) + 'static) {
        let click = gtk::GestureClick::builder().button(3).build();
        click.connect_pressed({
            let (area, state) = (self.area.clone(), self.state.clone());
            move |gesture, _, x, y| {
                let width = f64::from(area.width());
                if !state.borrow().live && width > 0.0 {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    menu((x / width).clamp(0.0, 1.0), x, y);
                }
            }
        });
        self.area.add_controller(click);
    }

    pub fn set_progress(&self, progress: f64) {
        self.state.borrow_mut().progress = progress.clamp(0.0, 1.0);
        self.area.queue_draw();
    }

    /// Calls `seek` with a fraction from 0 to 1 when a saved recording's
    /// waveform is clicked or dragged.
    pub fn connect_seek(&self, seek: impl Fn(f64) + 'static) {
        let seek = Rc::new(
            move |area: &gtk::DrawingArea, state: &RefCell<State>, x: f64| {
                let width = f64::from(area.width());
                if !state.borrow().live && width > 0.0 {
                    seek((x / width).clamp(0.0, 1.0));
                }
            },
        );
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin({
            let (area, state, seek) = (self.area.clone(), self.state.clone(), seek.clone());
            move |_, x, _| seek(&area, &state, x)
        });
        drag.connect_drag_update({
            let (area, state) = (self.area.clone(), self.state.clone());
            move |drag, offset, _| {
                if let Some((start, _)) = drag.start_point() {
                    seek(&area, &state, start + offset);
                }
            }
        });
        self.area.add_controller(drag);
    }
}

fn draw(state: &State, cr: &cairo::Context, width: f64, height: f64) -> Result<(), cairo::Error> {
    let bars = (width / PITCH) as usize;
    if bars == 0 {
        return Ok(());
    }
    let mid = (height / 2.0).round();
    cr.set_line_width(BAR);
    cr.set_line_cap(cairo::LineCap::Round);

    // The value drawn for a bar, or `None` where a recording in progress has
    // not reached yet.
    let value = |values: &[f32], bar: usize| -> Option<f64> {
        let value = if state.live {
            // The newest bar is at the right edge.
            *values.get((values.len() + bar).checked_sub(bars)?)?
        } else {
            resample(values, bar, bars)
        };
        // Speech rarely comes near full scale; this curve keeps it readable.
        Some(f64::from(value).powf(0.6))
    };

    let played = if state.live {
        bars
    } else {
        (state.progress * bars as f64).round() as usize
    };
    for (range, alpha) in [(0..played, 1.0), (played..bars, UNPLAYED)] {
        // Each entry: colour, values, an offset from the centre line, and how
        // far a full-scale bar reaches above and below that.
        let gap = 2.5;
        let reach = mid - gap - BAR;
        // Whether each track of a two-track recording is being heard.
        let (mic_heard, system_heard) = match state.heard {
            Heard::Both => (true, true),
            Heard::Microphone => (true, false),
            Heard::System => (false, true),
        };
        let tracks: &[(Rgb, &[f32], bool, f64, f64, f64)] = match state.tracks {
            Tracks::Both => &[
                (MICROPHONE, &state.microphone, mic_heard, -gap, -reach, 0.0),
                (SYSTEM, &state.system, system_heard, gap, 0.0, reach),
            ],
            Tracks::Microphone => &[(MICROPHONE, &state.microphone, true, 0.0, -reach, reach)],
            Tracks::System => &[(SYSTEM, &state.system, true, 0.0, -reach, reach)],
        };
        for &((r, g, b), values, heard, offset, up, down) in tracks {
            let alpha = if heard { alpha } else { alpha * MUTED };
            cr.set_source_rgba(r, g, b, alpha);
            for bar in range.clone() {
                let Some(v) = value(values, bar) else {
                    continue;
                };
                let x = bar as f64 * PITCH + PITCH / 2.0;
                cr.move_to(x, mid + offset + v * up);
                cr.line_to(x, mid + offset + v * down);
            }
            cr.stroke()?;
        }
    }

    if !state.live {
        let (r, g, b) = PLAYHEAD;
        cr.set_source_rgb(r, g, b);
        cr.set_line_cap(cairo::LineCap::Butt);
        let x = (state.progress * width).clamp(BAR / 2.0, width - BAR / 2.0);
        cr.move_to(x, 0.0);
        cr.line_to(x, height);
        cr.stroke()?;
    }
    Ok(())
}

/// The loudest of the values that fall on one bar when `values` is spread
/// across `bars` bars.
fn resample(values: &[f32], bar: usize, bars: usize) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let start = bar * values.len() / bars;
    let end = ((bar + 1) * values.len() / bars)
        .max(start + 1)
        .min(values.len());
    values[start..end].iter().copied().fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_spikes_when_shrinking() {
        let values = [0.1, 0.9, 0.2, 0.2, 0.1, 0.7];
        let bars: Vec<f32> = (0..3).map(|bar| resample(&values, bar, 3)).collect();
        assert_eq!(bars, [0.9, 0.2, 0.7]);
    }

    #[test]
    fn resample_stretches_a_short_recording_across_all_bars() {
        let values = [0.2, 0.8];
        let bars: Vec<f32> = (0..6).map(|bar| resample(&values, bar, 6)).collect();
        assert_eq!(bars, [0.2, 0.2, 0.2, 0.8, 0.8, 0.8]);
        assert_eq!(resample(&[], 0, 6), 0.0);
    }
}
