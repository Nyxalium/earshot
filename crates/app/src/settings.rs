//! The settings window. Every change is saved to the config file straight
//! away, the same file the `earshot` command and the service read.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use earshot_core::{Config, Format, Source};
use gtk::prelude::*;
use gtk::{cairo, gio, glib, pango};

use crate::theme::{self, THEMES, Theme};

const FORMATS: [(Format, &str, &str); 3] = [
    (
        Format::Opus,
        "Opus",
        "Small files. Speech sounds like the original; music is very close.",
    ),
    (
        Format::Flac,
        "FLAC",
        "Lossless, about half the size of WAV. Best for music and editing.",
    ),
    (
        Format::Wav,
        "WAV",
        "Uncompressed. Large files that open in any program.",
    ),
];

const SOURCES: [(Source, &str); 3] = [
    (Source::Microphone, "Microphone"),
    (Source::System, "System audio"),
    (Source::Both, "Mic + system"),
];

struct Settings {
    window: gtk::Window,
    config: RefCell<Config>,
    folder: gtk::Label,
    format_help: gtk::Label,
    problem: gtk::Label,
    /// The theme cards on the Appearance tab, by theme id.
    theme_cards: Vec<(&'static str, gtk::Button)>,
    on_saved: Box<dyn Fn(&Config)>,
}

/// Opens the settings window over `parent`. `on_saved` runs after each
/// change has been written to the config file.
pub fn show(parent: &impl IsA<gtk::Window>, on_saved: impl Fn(&Config) + 'static) {
    let window = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(480)
        .resizable(false)
        .build();
    let header = gtk::HeaderBar::new();
    let title = gtk::Label::new(Some("Settings"));
    title.add_css_class("app-name");
    header.pack_start(&title);
    let tabs = gtk::Stack::new();
    header.set_title_widget(Some(&gtk::StackSwitcher::builder().stack(&tabs).build()));
    window.set_titlebar(Some(&header));

    // A config file that cannot be read is shown, not silently replaced by
    // defaults; changing a setting here then writes a valid file.
    let (config, load_problem) = match Config::load() {
        Ok(config) => (config, None),
        Err(e) => (
            Config::default(),
            Some(format!(
                "{e}. Changing a setting here will replace the file."
            )),
        ),
    };

    let folder = label("", &["dim"]);
    folder.set_ellipsize(pango::EllipsizeMode::Middle);
    folder.set_hexpand(true);
    let change_folder = gtk::Button::with_label("Change…");
    let open_folder = gtk::Button::with_label("Open");
    let folder_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    folder_row.append(&folder);
    folder_row.append(&change_folder);
    folder_row.append(&open_folder);

    let format = gtk::DropDown::from_strings(&FORMATS.map(|(_, name, _)| name));
    format.set_halign(gtk::Align::Start);
    let format_help = label("", &["small", "dim"]);
    format_help.set_wrap(true);
    let source = gtk::DropDown::from_strings(&SOURCES.map(|(_, name)| name));
    source.set_halign(gtk::Align::Start);

    let problem = label("", &["problem"]);
    problem.set_wrap(true);
    problem.set_visible(false);

    let general = gtk::Box::new(gtk::Orientation::Vertical, 0);
    general.add_css_class("settings");
    for (heading, widgets) in [
        (
            "Recordings folder",
            vec![folder_row.upcast::<gtk::Widget>()],
        ),
        (
            "Format",
            vec![format.clone().upcast(), format_help.clone().upcast()],
        ),
        ("Default source", vec![source.clone().upcast()]),
    ] {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        section.add_css_class("setting");
        section.append(&label(heading, &["heading"]));
        for widget in widgets {
            section.append(&widget);
        }
        general.append(&section);
    }

    let appearance = gtk::Box::new(gtk::Orientation::Vertical, 4);
    appearance.add_css_class("settings");
    appearance.set_margin_top(8);
    let theme_cards: Vec<_> = THEMES
        .iter()
        .map(|theme| {
            let card = theme_card(theme);
            appearance.append(&card);
            (theme.id, card)
        })
        .collect();

    tabs.add_titled(&general, Some("general"), "General");
    tabs.add_titled(&appearance, Some("appearance"), "Appearance");
    // A problem is shown above whichever tab is open.
    problem.set_margin_start(20);
    problem.set_margin_end(20);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&problem);
    content.append(&tabs);
    window.set_child(Some(&content));

    let settings = Rc::new(Settings {
        window,
        config: RefCell::new(config),
        folder,
        format_help,
        problem,
        theme_cards,
        on_saved: Box::new(on_saved),
    });
    settings.mark_theme(theme::current().id);
    for (id, card) in &settings.theme_cards {
        let id = *id;
        card.connect_clicked(glib::clone!(
            #[weak]
            settings,
            move |_| {
                theme::apply(id);
                settings.mark_theme(id);
                match theme::save(id) {
                    Ok(()) => settings.problem.set_visible(false),
                    Err(e) => settings.show_problem(&format!("Could not save the theme: {e}")),
                }
            }
        ));
    }

    {
        let config = settings.config.borrow();
        settings.show_folder();
        let index = FORMATS.iter().position(|(f, _, _)| *f == config.format);
        format.set_selected(index.unwrap_or(0) as u32);
        settings.show_format_help(config.format);
        let index = SOURCES.iter().position(|(s, _)| *s == config.source);
        source.set_selected(index.unwrap_or(0) as u32);
    }
    if let Some(message) = load_problem {
        settings.show_problem(&message);
    }

    // Connected after the initial values are set, so opening the window
    // does not write the file.
    format.connect_selected_notify(glib::clone!(
        #[weak]
        settings,
        move |format| {
            let Some((chosen, _, _)) = FORMATS.get(format.selected() as usize) else {
                return;
            };
            settings.show_format_help(*chosen);
            settings.change(|config| config.format = *chosen);
        }
    ));
    source.connect_selected_notify(glib::clone!(
        #[weak]
        settings,
        move |source| {
            if let Some((chosen, _)) = SOURCES.get(source.selected() as usize) {
                settings.change(|config| config.source = *chosen);
            }
        }
    ));
    change_folder.connect_clicked(glib::clone!(
        #[weak]
        settings,
        move |_| settings.choose_folder()
    ));
    open_folder.connect_clicked(glib::clone!(
        #[weak]
        settings,
        move |_| settings.open_folder()
    ));

    // The window keeps the settings alive until it is closed.
    settings.window.connect_close_request({
        let settings = RefCell::new(Some(settings.clone()));
        move |_| {
            settings.take();
            glib::Propagation::Proceed
        }
    });
    settings.window.present();
}

impl Settings {
    fn mark_theme(&self, id: &str) {
        for (theme, card) in &self.theme_cards {
            if *theme == id {
                card.add_css_class("selected");
            } else {
                card.remove_css_class("selected");
            }
        }
    }

    /// Applies a change and saves it.
    fn change(&self, apply: impl FnOnce(&mut Config)) {
        let config = {
            let mut config = self.config.borrow_mut();
            apply(&mut config);
            config.clone()
        };
        match config.save() {
            Ok(()) => {
                self.problem.set_visible(false);
                (self.on_saved)(&config);
            }
            Err(e) => self.show_problem(&format!("Could not save settings: {e}")),
        }
    }

    fn show_problem(&self, message: &str) {
        let mut chars = message.chars();
        let message: String = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        self.problem.set_text(&message);
        self.problem.set_visible(true);
    }

    fn show_folder(&self) {
        let dir = self.config.borrow().recordings_dir.clone();
        // Shorten the home folder to ~, as file managers do.
        let home = glib::home_dir();
        let shown = match dir.strip_prefix(&home) {
            Ok(rest) => PathBuf::from("~").join(rest),
            Err(_) => dir.clone(),
        };
        self.folder.set_text(&shown.to_string_lossy());
        self.folder.set_tooltip_text(Some(&dir.to_string_lossy()));
    }

    fn show_format_help(&self, format: Format) {
        let help = FORMATS.iter().find(|(f, _, _)| *f == format);
        self.format_help
            .set_text(help.map_or("", |(_, _, help)| help));
    }

    fn choose_folder(self: &Rc<Self>) {
        let current = self.config.borrow().recordings_dir.clone();
        let mut dialog = gtk::FileDialog::builder()
            .title("Choose the recordings folder")
            .accept_label("Choose");
        // The current folder may not exist yet; start from the nearest one that does.
        if let Some(existing) = current.ancestors().find(|dir| dir.is_dir()) {
            dialog = dialog.initial_folder(&gio::File::for_path(existing));
        }
        let dialog = dialog.build();
        let settings = self.clone();
        glib::spawn_future_local(async move {
            match dialog.select_folder_future(Some(&settings.window)).await {
                Ok(folder) => match folder.path() {
                    Some(path) => {
                        settings.change(|config| config.recordings_dir = path);
                        settings.show_folder();
                    }
                    None => settings.show_problem("That folder is not on this computer's disks"),
                },
                Err(e) if e.matches(gtk::DialogError::Dismissed) => {}
                Err(e) => settings.show_problem(&format!("Could not choose a folder: {e}")),
            }
        });
    }

    /// Shows the recordings folder in the file manager, creating it first if
    /// nothing has been recorded yet.
    fn open_folder(self: &Rc<Self>) {
        let dir = self.config.borrow().recordings_dir.clone();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.show_problem(&format!("Could not create {}: {e}", dir.display()));
            return;
        }
        let settings = self.clone();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(
            Some(&self.window),
            gio::Cancellable::NONE,
            move |result| {
                if let Err(e) = result {
                    settings.show_problem(&format!("Could not open the folder: {e}"));
                }
            },
        );
    }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

/// A theme's name and description beside a small drawing of the window in
/// its colours.
fn theme_card(theme: &'static Theme) -> gtk::Button {
    let preview = gtk::DrawingArea::builder()
        .content_width(132)
        .content_height(84)
        .build();
    preview.set_draw_func(move |_, cr, _, _| {
        // Drawing only fails if the surface is gone; nothing to show then.
        let _ = draw_preview(theme, cr);
    });

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_valign(gtk::Align::Center);
    text.append(&label(theme.name, &["heading"]));
    text.append(&label(theme.description, &["small", "dim"]));
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    content.append(&preview);
    content.append(&text);

    let card = gtk::Button::builder().child(&content).build();
    card.add_css_class("theme-card");
    card
}

fn draw_preview(theme: &Theme, cr: &cairo::Context) -> Result<(), cairo::Error> {
    let colour = |cr: &cairo::Context, hex: &str, alpha: f64| {
        let (r, g, b) = theme::rgb(hex);
        cr.set_source_rgba(r, g, b, alpha);
    };
    let rounded = |cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64| {
        use std::f64::consts::{FRAC_PI_2, PI};
        cr.new_sub_path();
        cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
        cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
        cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
        cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
        cr.close_path();
    };

    // The window, with an outline so it stands out on any background.
    rounded(cr, 0.5, 0.5, 131.0, 83.0, 6.0);
    colour(cr, theme.bg, 1.0);
    cr.fill_preserve()?;
    colour(cr, theme.line, 1.0);
    cr.set_line_width(1.0);
    cr.stroke()?;

    // The list: the record button, then three recordings, one selected.
    rounded(cr, 7.0, 8.0, 24.0, 8.0, 4.0);
    colour(cr, theme.red, 1.0);
    cr.fill()?;
    rounded(cr, 5.0, 37.0, 38.0, 15.0, 3.0);
    colour(cr, theme.raised, 1.0);
    cr.fill()?;
    for row in 0..3 {
        let y = 25.0 + f64::from(row) * 16.0;
        colour(cr, theme.text, 0.9);
        cr.rectangle(8.0, y, 26.0, 2.5);
        cr.fill()?;
        colour(cr, theme.dim, 0.9);
        cr.rectangle(8.0, y + 5.0, 17.0, 2.0);
        cr.fill()?;
    }

    // The panel: a name, a two-track waveform, and the play button.
    rounded(cr, 48.0, 5.0, 79.0, 74.0, 5.0);
    colour(cr, theme.panel, 1.0);
    cr.fill()?;
    colour(cr, theme.text, 0.9);
    cr.rectangle(55.0, 12.0, 30.0, 3.0);
    cr.fill()?;
    const MICROPHONE: [f64; 16] = [
        3.0, 9.0, 12.0, 7.0, 2.0, 1.0, 8.0, 13.0, 11.0, 4.0, 1.0, 1.0, 6.0, 10.0, 5.0, 2.0,
    ];
    const SYSTEM: [f64; 16] = [
        1.0, 1.0, 2.0, 6.0, 10.0, 8.0, 3.0, 1.0, 1.0, 7.0, 11.0, 9.0, 4.0, 1.0, 2.0, 6.0,
    ];
    cr.set_line_width(2.0);
    cr.set_line_cap(cairo::LineCap::Round);
    for (heights, hex, direction) in [
        (MICROPHONE, theme.microphone, -1.0),
        (SYSTEM, theme.system, 1.0),
    ] {
        colour(cr, hex, 1.0);
        for (i, height) in heights.iter().enumerate() {
            let x = 57.0 + i as f64 * 4.2;
            cr.move_to(x, 40.0 + direction * 2.0);
            cr.line_to(x, 40.0 + direction * (2.0 + height));
        }
        cr.stroke()?;
    }
    colour(cr, theme.text, 1.0);
    cr.arc(60.0, 68.0, 5.0, 0.0, std::f64::consts::TAU);
    cr.fill()?;
    cr.rectangle(70.0, 66.5, 20.0, 3.0);
    cr.fill()?;
    Ok(())
}
