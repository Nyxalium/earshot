//! The settings window. Every change is saved to the config file straight
//! away, the same file the `earshot` command and the service read.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use earshot_core::{Config, Format, Source};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

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
    header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
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

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("settings");
    content.append(&problem);
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
        content.append(&section);
    }
    window.set_child(Some(&content));

    let settings = Rc::new(Settings {
        window,
        config: RefCell::new(config),
        folder,
        format_help,
        problem,
        on_saved: Box::new(on_saved),
    });

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
