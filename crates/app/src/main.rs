//! The Earshot window. It shows and controls what the Earshot service is
//! doing, and plays saved recordings.

mod backend;
mod icon;
mod player;
mod settings;
mod theme;
mod waveform;
mod window;

use std::cell::OnceCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use window::MainWindow;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(earshot_client::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|_| load_style());

    let window: Rc<OnceCell<MainWindow>> = Rc::default();
    app.connect_activate({
        let window = window.clone();
        move |app| window.get_or_init(|| MainWindow::new(app)).present()
    });
    // `earshot-app <file>` selects that recording.
    app.connect_open(move |app, files, _| {
        let window = window.get_or_init(|| MainWindow::new(app));
        if let Some(path) = files.first().and_then(|file| file.path()) {
            window.open(&path);
        }
        window.present();
    });
    app.run()
}

/// Installs Earshot's stylesheet and applies the theme chosen in Settings.
fn load_style() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    theme::install(&display);
    let theme = theme::saved().unwrap_or_else(|problem| {
        eprintln!("earshot: {problem}; using the default theme");
        String::new()
    });
    if !theme::apply(&theme) && !theme.is_empty() {
        eprintln!("earshot: there is no theme named \"{theme}\"; using the default");
    }
}
