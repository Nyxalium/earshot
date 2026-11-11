//! Themes. A theme is a palette, a few style rules on top of the shared
//! stylesheet, and the colours waveforms are drawn in.

use std::cell::Cell;
use std::path::PathBuf;

use gtk::prelude::*;
use gtk::{gdk, glib};
use serde::{Deserialize, Serialize};

pub struct Theme {
    /// The name saved in the config file.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    dark: bool,
    /// The window, behind the list.
    pub bg: &'static str,
    /// The raised panel that holds the open recording.
    pub panel: &'static str,
    /// Controls, and the selected recording.
    pub raised: &'static str,
    pub line: &'static str,
    pub text: &'static str,
    pub dim: &'static str,
    /// Recording, and the playhead.
    pub red: &'static str,
    /// The microphone and system audio in waveforms.
    pub microphone: &'static str,
    pub system: &'static str,
    /// Rules that come after the shared stylesheet.
    overrides: &'static str,
}

const MONO: &str = "\"JetBrainsMono NF\", \"JetBrains Mono\", monospace";

pub const THEMES: &[Theme] = &[
    Theme {
        id: "calm-dark",
        name: "Calm dark",
        description: "Dark and borderless.",
        dark: true,
        bg: "#0F1012",
        panel: "#15161A",
        raised: "#1F2025",
        line: "#2C2D33",
        text: "#E9E9EC",
        dim: "#7B7D84",
        red: "#F04D4D",
        microphone: "#E9E9EC",
        system: "#7F95FF",
        overrides: "",
    },
    Theme {
        id: "plain-light",
        name: "Plain light",
        description: "Light, with a white panel.",
        dark: false,
        bg: "#ECECE8",
        panel: "#FBFBFA",
        raised: "#DFDFDA",
        line: "#CFCFC9",
        text: "#1C1C1E",
        dim: "#7C7C80",
        red: "#D9393E",
        microphone: "#1C1C1E",
        system: "#3F63E6",
        overrides: "",
    },
    Theme {
        id: "tape",
        name: "Tape",
        description: "Cream and orange, with chunky keys.",
        dark: false,
        bg: "#DDD7C8",
        panel: "#EFEADF",
        raised: "#CFC8B8",
        line: "#B9B2A1",
        text: "#1E1E1C",
        dim: "#6D685C",
        red: "#E8552B",
        microphone: "#1E1E1C",
        system: "#2F5D8A",
        overrides: "
            .panel { border-radius: 6px; }
            button, entry, list > row { border-radius: 4px; }
            .bar > button:not(.flat),
            .side button.record {
              box-shadow: 0 2px 0 alpha(black, 0.55);
              font-weight: 600;
            }
            .bar > button.chip,
            .bar > button:not(.record):not(.flat):not(.play) {
              background-color: #2A2A27;
              color: #F3EEE2;
            }
            button.play { border-radius: 4px; }
            .clock, .time, .tnum { font-family: MONO; }
        ",
    },
    Theme {
        id: "scope",
        name: "Scope",
        description: "An instrument look, in a monospace font.",
        dark: true,
        bg: "#06080B",
        panel: "#0A0D12",
        raised: "#131A24",
        line: "#223043",
        text: "#C9D4E3",
        dim: "#7D8A9C",
        red: "#FF4D4D",
        microphone: "#EAF2FF",
        system: "#6FA8FF",
        overrides: "
            window, popover { font-family: MONO; }
            .panel { border-radius: 3px; box-shadow: inset 0 0 0 1px @line; }
            button, entry, list > row, popover > contents { border-radius: 3px; }
            button, button.record, button.play {
              background-color: transparent;
              border-color: @line;
            }
            button:hover, button.play:hover { background-color: @raised; }
            button.record { border-color: @red; color: @red; }
            button.record:hover { background-color: alpha(@red, 0.12); }
            button.play { border-radius: 3px; border-color: @text; color: @text; }
            button.flat, button.menu-item, button.theme-card,
            windowcontrols button, headerbar button.settings-button {
              border-color: transparent;
            }
            entry { background-color: transparent; border-color: @line; }
        ",
    },
];

thread_local! {
    static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
    static CURRENT: Cell<&'static Theme> = const { Cell::new(&THEMES[0]) };
}

/// What the window remembers. It has a file of its own, apart from the
/// config the service reads: the service refuses settings it does not know,
/// and one left running from an older version would refuse these.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Saved {
    theme: String,
}

fn saved_path() -> PathBuf {
    glib::user_config_dir().join("earshot").join("window.toml")
}

/// The id of the theme chosen in Settings, or of the default theme if none
/// has been chosen yet.
pub fn saved() -> Result<String, String> {
    let path = saved_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(THEMES[0].id.to_owned()),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    let saved: Saved =
        toml::from_str(&text).map_err(|e| format!("invalid settings {}: {e}", path.display()))?;
    Ok(saved.theme)
}

/// Remembers the chosen theme for the next time the window opens.
pub fn save(id: &str) -> Result<(), String> {
    let saved = Saved {
        theme: id.to_owned(),
    };
    let text = toml::to_string(&saved).map_err(|e| e.to_string())?;
    earshot_core::config::write_whole(&saved_path(), &text).map_err(|e| e.to_string())
}

pub fn current() -> &'static Theme {
    CURRENT.get()
}

/// Makes Earshot's stylesheet apply to every window on the display. Earshot
/// has its own look: it starts from GTK's built-in theme, whatever GTK theme
/// the desktop has set, and restyles from there.
pub fn install(display: &gdk::Display) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_theme_name(Some("Default"));
    }
    PROVIDER.with(|provider| {
        gtk::style_context_add_provider_for_display(
            display,
            provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

/// Switches to the theme with this id, on every open window. Returns `false`
/// and uses the default theme if there is no such theme.
pub fn apply(id: &str) -> bool {
    let found = THEMES.iter().find(|theme| theme.id == id);
    let theme = found.unwrap_or(&THEMES[0]);
    CURRENT.set(theme);

    // The parts of GTK's own theme that show through, such as scrollbars,
    // follow the theme's brightness.
    if let Some(settings) = gtk::Settings::default() {
        // GTK 4.20 replaced the dark-theme switch with a colour scheme setting.
        let scheme = settings
            .find_property("gtk-interface-color-scheme")
            .and_then(|property| glib::EnumClass::with_type(property.value_type()))
            .and_then(|class| class.to_value_by_nick(if theme.dark { "dark" } else { "light" }));
        match scheme {
            Some(scheme) => settings.set_property_from_value("gtk-interface-color-scheme", &scheme),
            None => settings.set_gtk_application_prefer_dark_theme(theme.dark),
        }
    }

    let palette: String = [
        ("bg", theme.bg),
        ("panel", theme.panel),
        ("raised", theme.raised),
        ("line", theme.line),
        ("text", theme.text),
        ("dim", theme.dim),
        ("red", theme.red),
        ("system", theme.system),
    ]
    .iter()
    .map(|(name, colour)| format!("@define-color {name} {colour};\n"))
    .collect();
    let css = format!(
        "{palette}{}{}",
        include_str!("style.css"),
        theme.overrides.replace("MONO", MONO)
    );
    PROVIDER.with(|provider| provider.load_from_string(&css));

    // Waveforms take their colours from the theme, not from the stylesheet.
    for window in gtk::Window::list_toplevels() {
        redraw(&window);
    }
    found.is_some()
}

fn redraw(widget: &gtk::Widget) {
    widget.queue_draw();
    let mut child = widget.first_child();
    while let Some(widget) = child {
        redraw(&widget);
        child = widget.next_sibling();
    }
}

/// A theme colour as red, green and blue from 0 to 1.
pub fn rgb(colour: &str) -> (f64, f64, f64) {
    let rgba = gdk::RGBA::parse(colour).unwrap_or(gdk::RGBA::WHITE);
    (rgba.red().into(), rgba.green().into(), rgba.blue().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_ids_are_unique_and_colours_parse() {
        for (i, theme) in THEMES.iter().enumerate() {
            assert!(
                THEMES[i + 1..].iter().all(|other| other.id != theme.id),
                "{} is used twice",
                theme.id
            );
            for colour in [
                theme.bg,
                theme.panel,
                theme.raised,
                theme.line,
                theme.text,
                theme.dim,
                theme.red,
                theme.microphone,
                theme.system,
            ] {
                assert!(gdk::RGBA::parse(colour).is_ok(), "{}: {colour}", theme.id);
            }
        }
    }
}
