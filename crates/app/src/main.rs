//! The Earshot window. It shows and controls what the Earshot service is
//! doing, and plays saved recordings.

mod backend;
mod icon;
mod player;
mod settings;
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

/// Earshot has its own look. It starts from GTK's built-in dark theme,
/// whatever GTK theme the desktop has set, and restyles from there.
fn load_style() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_theme_name(Some("Default"));
        // GTK 4.20 replaced the dark-theme switch with a colour scheme setting.
        let scheme = settings
            .find_property("gtk-interface-color-scheme")
            .and_then(|property| glib::EnumClass::with_type(property.value_type()))
            .and_then(|class| class.to_value_by_nick("dark"));
        match scheme {
            Some(dark) => settings.set_property_from_value("gtk-interface-color-scheme", &dark),
            None => settings.set_gtk_application_prefer_dark_theme(true),
        }
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
