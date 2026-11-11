//! The main window: recordings on the left, and on the right the recording in
//! progress or the one that is selected.

use std::cell::{Cell, OnceCell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use earshot_client::{EarshotProxy, RecordingInfo, StartOptions, Status, clock, file_size};
use earshot_core::{Format, Tracks as Heard};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use crate::backend::{Backend, Event};
use crate::icon::{self, Icon, IconView};
use crate::player::{Player, PlayerEvent};
use crate::settings;
use crate::waveform::{Tracks, Waveform};

/// The service's name for each source, and its label in the source menu.
const SOURCES: [(&str, &str); 3] = [
    ("microphone", "Microphone"),
    ("system", "System audio"),
    ("both", "Mic + system"),
];
const SPEEDS: [(f64, &str); 4] = [(1.0, "1×"), (1.25, "1.25×"), (1.5, "1.5×"), (2.0, "2×")];
const SKIP_MS: u64 = 10_000;

pub struct MainWindow {
    window: gtk::ApplicationWindow,
    /// Missing if the service could not be reached; the window then only
    /// shows why.
    ui: Option<Rc<Ui>>,
}

impl MainWindow {
    pub fn new(app: &gtk::Application) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Earshot")
            .default_width(920)
            .default_height(580)
            .build();

        let header = gtk::HeaderBar::new();
        let name = gtk::Label::new(Some("Earshot"));
        name.add_css_class("app-name");
        header.pack_start(&name);
        // The name sits at the start of the bar, so the centred title is empty.
        header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        window.set_titlebar(Some(&header));

        let ui = match Ui::new() {
            Ok((ui, root)) => {
                window.set_child(Some(&root));
                add_settings(app, &window, &header, &ui);
                Some(ui)
            }
            Err(message) => {
                let label = gtk::Label::builder()
                    .label(format!("Earshot could not start.\n\n{message}"))
                    .wrap(true)
                    .justify(gtk::Justification::Center)
                    .selectable(true)
                    .build();
                label.add_css_class("page");
                window.set_child(Some(&label));
                None
            }
        };
        Self { window, ui }
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Selects a recording in the recordings folder, given its file.
    pub fn open(&self, path: &Path) {
        if let Some(ui) = &self.ui {
            ui.open(path.to_string_lossy().into_owned());
        }
    }
}

/// Adds the settings button to the title bar, and Ctrl+, to open it.
fn add_settings(
    app: &gtk::Application,
    window: &gtk::ApplicationWindow,
    header: &gtk::HeaderBar,
    ui: &Rc<Ui>,
) {
    let action = gio::SimpleAction::new("settings", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        ui,
        move |_, _| {
            settings::show(
                &window,
                glib::clone!(
                    #[weak]
                    ui,
                    move |config| ui.settings_changed(config)
                ),
            );
        }
    ));
    window.add_action(&action);
    app.set_accels_for_action("win.settings", &["<Control>comma"]);

    let button = gtk::Button::builder()
        .child(IconView::new(Icon::Settings).widget())
        .action_name("win.settings")
        .tooltip_text("Settings (Ctrl+,)")
        .build();
    button.add_css_class("settings-button");
    header.pack_end(&button);
}

struct Ui {
    backend: Backend,
    player: Player,
    model: RefCell<Model>,

    // Sidebar
    top: gtk::Stack,
    source: gtk::DropDown,
    live_row_label: gtk::Label,
    live_row_time: gtk::Label,
    live_row_source: gtk::Label,
    search: gtk::SearchEntry,
    list: gtk::ListBox,

    // Right side
    banner: gtk::Revealer,
    banner_box: gtk::Box,
    banner_text: gtk::Label,
    pages: gtk::Stack,
    empty: gtk::Label,

    // The recording in progress
    time: gtk::Label,
    live_meta: gtk::Label,
    live_wave: Waveform,
    live_tracks: gtk::Box,
    pause_icon: IconView,
    pause_label: gtk::Label,

    // The selected recording
    title: gtk::Label,
    meta: gtk::Label,
    wave: Waveform,
    /// Beside a two-track waveform: click one to hear only that track.
    track_toggles: gtk::Box,
    mic_toggle: gtk::Button,
    system_toggle: gtk::Button,
    /// The waveform's right-click menu.
    wave_menu: gtk::Popover,
    /// The check marks beside Both tracks, Mic only and System only.
    heard_checks: [(Heard, gtk::Label); 3],
    /// Menu items that only apply to two-track recordings.
    two_track_items: gtk::Box,
    /// Where on the waveform the menu was opened, from 0 to 1.
    menu_fraction: Cell<f64>,
    position: gtk::Label,
    duration: gtk::Label,
    play_icon: IconView,
    speed: gtk::Button,

    // A recording's menu in the list, and the name field for renaming it
    row_menu: gtk::Popover,
    rename_popover: gtk::Popover,
    rename_entry: gtk::Entry,
    /// The recording the menu or name field is open for, and its row.
    menu_target: RefCell<Option<(String, gtk::ListBoxRow)>>,
    /// Updates the position display every frame, only while playing.
    progress_tick: RefCell<Option<gtk::TickCallbackId>>,
}

#[derive(Default)]
struct Model {
    recordings: Vec<RecordingInfo>,
    /// Path of the recording open on the right, if any.
    selected: Option<String>,
    /// Path to select once the list has been reloaded.
    select_next: Option<String>,
    /// The recorder's state as last reported: `idle`, `recording` or `paused`.
    state: String,
    /// Length of the open recording, or 0 while it is not known.
    duration_ms: u64,
    /// Index into `SPEEDS`.
    speed: usize,
    /// The list is being rebuilt, so selection changes are not the user's.
    populating: bool,
    /// The open recording is being renamed and will reappear under a new path.
    renaming: bool,
}

impl Ui {
    fn new() -> Result<(Rc<Self>, gtk::Box), String> {
        let (backend, mut events) = Backend::connect()?;

        // The player needs somewhere to report to before the window exists.
        let this: Rc<OnceCell<Weak<Ui>>> = Rc::new(OnceCell::new());
        let player = Player::new({
            let this = this.clone();
            move |event| {
                if let Some(ui) = this.get().and_then(Weak::upgrade) {
                    ui.player_event(event);
                }
            }
        })
        .map_err(|e| format!("Playback is not available: {e}"))?;

        // Sidebar: record controls, or the recording in progress.
        let record = icon::button(
            &IconView::new(Icon::Record),
            &gtk::Label::new(Some("Record")),
        );
        record.add_css_class("record");
        record.set_hexpand(true);
        let source = gtk::DropDown::from_strings(&SOURCES.map(|(_, label)| label));
        // A config that cannot be read is reported when a recording starts.
        let configured = earshot_core::Config::load()
            .ok()
            .map(|config| config.source.to_string());
        if let Some(index) = SOURCES
            .iter()
            .position(|(name, _)| Some(*name) == configured.as_deref())
        {
            source.set_selected(index as u32);
        }
        let idle = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        idle.append(&record);
        idle.append(&source);

        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_valign(gtk::Align::Center);
        let live_row_label = label("Recording", &[]);
        live_row_label.set_hexpand(true);
        let live_row_time = label("", &["dim", "tnum"]);
        let live_row_source = label("", &["small", "dim"]);
        let live_grid = gtk::Grid::builder().column_spacing(7).build();
        live_grid.attach(&dot, 0, 0, 1, 1);
        live_grid.attach(&live_row_label, 1, 0, 1, 1);
        live_grid.attach(&live_row_time, 2, 0, 1, 1);
        live_grid.attach(&live_row_source, 0, 1, 3, 1);
        let live_row = gtk::Button::builder().child(&live_grid).build();
        live_row.add_css_class("live-row");

        let top = gtk::Stack::builder().vhomogeneous(false).build();
        top.add_named(&idle, Some("idle"));
        top.add_named(&live_row, Some("recording"));

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search recordings")
            .build();
        let list = gtk::ListBox::new();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 8);
        sidebar.add_css_class("sidebar");
        sidebar.set_size_request(270, -1);
        // The record button inside expands; the sidebar itself must not.
        sidebar.set_hexpand(false);
        sidebar.append(&top);
        sidebar.append(&search);
        sidebar.append(&scroller);

        // Right side: an error banner above the pages.
        let banner_text = label("", &[]);
        banner_text.set_wrap(true);
        banner_text.set_hexpand(true);
        let dismiss = gtk::Button::with_label("Dismiss");
        dismiss.add_css_class("flat");
        let banner_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        banner_box.add_css_class("banner");
        banner_box.append(&banner_text);
        banner_box.append(&dismiss);
        let banner = gtk::Revealer::builder().child(&banner_box).build();

        let empty = gtk::Label::new(None);
        empty.add_css_class("dim");

        // The recording in progress.
        let time = label("0:00", &["time"]);
        let live_meta = label("", &["dim"]);
        let live_wave = Waveform::new();
        let live_tracks = track_labels();
        let pause_icon = IconView::new(Icon::Pause);
        let pause_label = gtk::Label::new(Some("Pause"));
        let pause = icon::button(&pause_icon, &pause_label);
        let stop = icon::button(
            &IconView::new(Icon::Stop),
            &gtk::Label::new(Some("Stop and save")),
        );
        stop.add_css_class("record");
        let discard = gtk::Button::with_label("Discard");
        discard.add_css_class("flat");
        let live_controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        live_controls.append(&pause);
        live_controls.append(&stop);
        live_controls.append(&discard);

        let live_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        live_page.add_css_class("page");
        live_page.append(&time);
        live_page.append(&live_meta);
        let live_area = wave_area(&live_wave, &live_tracks, 180);
        live_area.attach(&live_controls, 0, 1, 1, 1);
        live_page.append(&live_area);

        // The selected recording.
        let title = label("", &["title"]);
        title.set_hexpand(true);
        title.set_ellipsize(pango::EllipsizeMode::End);

        let meta = label("", &["dim"]);
        let wave = Waveform::new();
        let mic_toggle = track_toggle("Mic", "Hear only the mic. Click again for both.");
        let system_toggle = track_toggle("System", "Hear only system audio. Click again for both.");
        system_toggle.add_css_class("system");
        let track_toggles = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .homogeneous(true)
            .width_request(56)
            .build();
        track_toggles.append(&mic_toggle);
        track_toggles.append(&system_toggle);
        let position = label("0:00", &["small", "dim", "tnum"]);
        position.set_hexpand(true);
        let duration = label("", &["small", "dim", "tnum"]);
        let times = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        times.append(&position);
        times.append(&duration);

        let back = gtk::Button::with_label("−10");
        let forward = gtk::Button::with_label("+10");
        let play_icon = IconView::new(Icon::Play);
        let play = gtk::Button::builder().child(play_icon.widget()).build();
        play.add_css_class("play");
        let speed = gtk::Button::with_label(SPEEDS[0].1);
        for button in [&back, &forward, &speed] {
            button.add_css_class("small");
            button.set_valign(gtk::Align::Center);
        }
        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        transport.add_css_class("transport");
        transport.append(&back);
        transport.append(&play);
        transport.append(&forward);
        transport.append(&speed);

        let playback_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        playback_page.add_css_class("page");
        playback_page.append(&title);
        playback_page.append(&meta);
        let wave_area = wave_area(&wave, &track_toggles, 150);
        wave_area.attach(&times, 0, 1, 1, 1);
        wave_area.attach(&transport, 0, 2, 1, 1);
        playback_page.append(&wave_area);

        let pages = gtk::Stack::builder().vexpand(true).build();
        pages.add_named(&empty, Some("empty"));
        pages.add_named(&live_page, Some("live"));
        pages.add_named(&playback_page, Some("playback"));

        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.set_hexpand(true);
        right.append(&banner);
        right.append(&pages);

        // A recording's menu in the list. Renaming opens a separate popup:
        // swapping the menu's contents would hide the focused item, and GTK
        // closes a popup that loses focus.
        let rename_item = menu_item("Rename…");
        let delete_item = menu_item("Move to trash");
        delete_item.add_css_class("destructive");
        let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        items.append(&rename_item);
        items.append(&delete_item);
        let rename_entry = gtk::Entry::builder().width_chars(26).build();
        let rename_confirm = gtk::Button::with_label("Rename");
        let rename_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        rename_box.append(&rename_entry);
        rename_box.append(&rename_confirm);
        let popover = |child: &gtk::Box| {
            let popover = gtk::Popover::builder()
                .child(child)
                .has_arrow(false)
                .position(gtk::PositionType::Bottom)
                .halign(gtk::Align::Start)
                .build();
            popover.add_css_class("row-menu");
            popover.set_parent(&list);
            popover
        };
        let row_menu = popover(&items);
        let rename_popover = popover(&rename_box);

        // The waveform's right-click menu.
        let play_here = menu_item("Play from here");
        let copy_time = menu_item("Copy timestamp");
        let mut heard_items = Vec::new();
        let two_track_items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        two_track_items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let heard_checks = [
            (Heard::Both, "Both tracks"),
            (Heard::Microphone, "Mic only"),
            (Heard::System, "System only"),
        ]
        .map(|(heard, text)| {
            let (item, check) = check_item(text);
            two_track_items.append(&item);
            heard_items.push((heard, item));
            (heard, check)
        });
        two_track_items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let save_mic = menu_item("Save mic track as…");
        let save_system = menu_item("Save system track as…");
        two_track_items.append(&save_mic);
        two_track_items.append(&save_system);
        let wave_items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wave_items.append(&play_here);
        wave_items.append(&copy_time);
        wave_items.append(&two_track_items);
        let wave_menu = gtk::Popover::builder()
            .child(&wave_items)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .build();
        wave_menu.add_css_class("row-menu");
        wave_menu.set_parent(wave.widget());

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&sidebar);
        root.append(&right);

        let ui = Rc::new(Ui {
            backend,
            player,
            model: RefCell::default(),
            top,
            source,
            live_row_label,
            live_row_time,
            live_row_source,
            search,
            list,
            banner,
            banner_box,
            banner_text,
            pages,
            empty,
            time,
            live_meta,
            live_wave,
            live_tracks,
            pause_icon,
            pause_label,
            title,
            meta,
            wave,
            track_toggles,
            mic_toggle,
            system_toggle,
            wave_menu,
            heard_checks,
            two_track_items,
            menu_fraction: Cell::new(0.0),
            position,
            duration,
            play_icon,
            speed,
            row_menu,
            rename_popover,
            rename_entry,
            menu_target: RefCell::default(),
            progress_tick: RefCell::default(),
        });
        let _ = this.set(Rc::downgrade(&ui));

        // The source chosen here becomes the default, as in Settings.
        ui.source.connect_selected_notify(glib::clone!(
            #[weak]
            ui,
            move |_| ui.remember_source()
        ));
        record.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.start()
        ));
        live_row.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.show_live()
        ));
        pause.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.toggle_pause()
        ));
        stop.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.stop()
        ));
        discard.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.request(|service| async move { service.cancel().await }, |_, _| {})
        ));
        dismiss.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.banner.set_reveal_child(false)
        ));

        ui.search.connect_search_changed(glib::clone!(
            #[weak]
            ui,
            move |_| ui.list.invalidate_filter()
        ));
        ui.list.set_filter_func(glib::clone!(
            #[weak]
            ui,
            #[upgrade_or]
            true,
            move |row| {
                let query = ui.search.text().to_lowercase();
                let model = ui.model.borrow();
                let name = model.recordings.get(row.index() as usize).map(|r| &r.name);
                name.is_none_or(|name| name.to_lowercase().contains(&query))
            }
        ));
        ui.list.set_header_func(glib::clone!(
            #[weak]
            ui,
            move |row, before| {
                let model = ui.model.borrow();
                let day = |row: &gtk::ListBoxRow| {
                    let recording = model.recordings.get(row.index() as usize)?;
                    Some(started(recording.created).0)
                };
                let day_of_row = day(row);
                let starts_group = day_of_row != before.and_then(day);
                let header = day_of_row
                    .filter(|_| starts_group)
                    .map(|day| label(day.label(), &["group"]));
                row.set_header(header.as_ref());
            }
        ));
        ui.list.connect_row_selected(glib::clone!(
            #[weak]
            ui,
            move |_, row| ui.row_selected(row)
        ));

        play.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.set_playing(!ui.player.is_playing())
        ));
        back.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.skip(false)
        ));
        forward.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.skip(true)
        ));
        ui.speed.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.next_speed()
        ));
        ui.wave.connect_seek(glib::clone!(
            #[weak]
            ui,
            move |fraction| ui.seek(fraction)
        ));
        ui.wave.connect_context_menu(glib::clone!(
            #[weak]
            ui,
            move |fraction, x, y| {
                ui.menu_fraction.set(fraction);
                let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                ui.wave_menu.set_pointing_to(Some(&rect));
                ui.wave_menu.popup();
            }
        ));
        play_here.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| {
                ui.wave_menu.popdown();
                ui.seek(ui.menu_fraction.get());
                ui.set_playing(true);
            }
        ));
        copy_time.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| {
                ui.wave_menu.popdown();
                let ms = (ui.menu_fraction.get() * ui.duration_ms() as f64) as u64;
                ui.wave.widget().clipboard().set_text(&clock(ms));
            }
        ));
        for (heard, item) in heard_items {
            item.connect_clicked(glib::clone!(
                #[weak]
                ui,
                move |_| {
                    ui.wave_menu.popdown();
                    ui.hear(heard);
                }
            ));
        }
        for (heard, item) in [
            (Heard::Microphone, &save_mic),
            (Heard::System, &save_system),
        ] {
            item.connect_clicked(glib::clone!(
                #[weak]
                ui,
                move |_| {
                    ui.wave_menu.popdown();
                    ui.save_track(heard);
                }
            ));
        }
        for (heard, toggle) in [
            (Heard::Microphone, &ui.mic_toggle),
            (Heard::System, &ui.system_toggle),
        ] {
            toggle.connect_clicked(glib::clone!(
                #[weak]
                ui,
                move |_| {
                    // Clicking the track already heard alone goes back to both.
                    let current = ui.player.tracks();
                    ui.hear(if current == Some(heard) {
                        Heard::Both
                    } else {
                        heard
                    });
                }
            ));
        }

        // Right-click, the Menu key or Shift+F10 opens a recording's menu;
        // F2 and Delete go straight to renaming and trashing it.
        let right_click = gtk::GestureClick::builder().button(3).build();
        right_click.connect_pressed(glib::clone!(
            #[weak]
            ui,
            move |gesture, _, x, y| {
                if let Some(row) = ui.list.row_at_y(y as i32) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    ui.open_row_menu(&row, Some((x, y)));
                }
            }
        ));
        ui.list.add_controller(right_click);
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak]
            ui,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| {
                // The focused row, or the selected one if focus is elsewhere
                // in the list.
                let Some(row) = ui
                    .list
                    .focus_child()
                    .and_then(|child| child.downcast::<gtk::ListBoxRow>().ok())
                    .or_else(|| ui.list.selected_row())
                else {
                    return glib::Propagation::Proceed;
                };
                let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                match key {
                    gtk::gdk::Key::Menu => ui.open_row_menu(&row, None),
                    gtk::gdk::Key::F10 if shift => ui.open_row_menu(&row, None),
                    gtk::gdk::Key::F2 => ui.show_rename(&row),
                    gtk::gdk::Key::Delete => {
                        if let Some(path) = ui.path_of(&row) {
                            ui.delete(path);
                        }
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        ));
        ui.list.add_controller(keys);
        rename_item.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| {
                let row = ui.menu_target.borrow().as_ref().map(|(_, row)| row.clone());
                ui.row_menu.popdown();
                if let Some(row) = row {
                    ui.show_rename(&row);
                }
            }
        ));
        delete_item.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| {
                let target = ui.menu_target.borrow().clone();
                ui.row_menu.popdown();
                if let Some((path, _)) = target {
                    ui.delete(path);
                }
            }
        ));
        for popover in [&ui.row_menu, &ui.rename_popover] {
            popover.connect_closed(glib::clone!(
                #[weak]
                ui,
                move |_| {
                    // Choosing Rename closes the menu and opens the name field;
                    // the mark stays while either is open.
                    if !ui.row_menu.is_visible() && !ui.rename_popover.is_visible() {
                        ui.set_target(None);
                    }
                }
            ));
        }
        ui.rename_entry.connect_activate(glib::clone!(
            #[weak]
            ui,
            move |_| ui.rename()
        ));
        rename_confirm.connect_clicked(glib::clone!(
            #[weak]
            ui,
            move |_| ui.rename()
        ));

        glib::spawn_future_local({
            let ui = Rc::downgrade(&ui);
            async move {
                while let Some(event) = events.recv().await {
                    let Some(ui) = ui.upgrade() else { break };
                    ui.service_event(event);
                }
            }
        });
        ui.request(
            |service| async move { service.status().await },
            |ui, status| ui.apply_status(status),
        );
        ui.reload();

        Ok((ui, root))
    }

    /// Shows a problem above the pages until it is dismissed.
    fn error(&self, message: &str) {
        self.banner_box.remove_css_class("notice");
        self.show_banner(message);
    }

    /// Shows the outcome of something that finished in the background.
    fn notice(&self, message: &str) {
        self.banner_box.add_css_class("notice");
        self.show_banner(message);
    }

    fn show_banner(&self, message: &str) {
        // The service words its messages for a terminal, without a capital.
        let mut chars = message.chars();
        let message: String = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        self.banner_text.set_text(&message);
        self.banner.set_reveal_child(true);
    }

    /// Makes a call to the service, then runs `then` with its result. A
    /// failure is shown in the banner.
    fn request<T, F, Fut>(self: &Rc<Self>, call: F, then: impl FnOnce(&Rc<Self>, T) + 'static)
    where
        T: Send + 'static,
        F: FnOnce(EarshotProxy<'static>) -> Fut + Send + 'static,
        Fut: Future<Output = zbus::Result<T>> + Send,
    {
        let ui = self.clone();
        glib::spawn_future_local(async move {
            match ui.backend.call(call).await {
                Ok(value) => then(&ui, value),
                Err(message) => ui.error(&message),
            }
        });
    }

    fn service_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::State(status) => self.apply_status(status),
            Event::Level {
                microphone,
                system,
                elapsed_ms,
            } => {
                self.live_wave.push(microphone, system);
                self.show_elapsed(elapsed_ms);
            }
            Event::Failed(message) => self.error(&format!("Recording failed: {message}")),
            Event::RecordingsChanged => self.reload(),
        }
    }

    /// Settings were saved: show the new default source, and the list of the
    /// recordings folder, which may have changed.
    fn settings_changed(self: &Rc<Self>, config: &earshot_core::Config) {
        let name = config.source.to_string();
        if let Some(index) = SOURCES.iter().position(|(n, _)| *n == name) {
            if self.source.selected() != index as u32 {
                self.source.set_selected(index as u32);
            }
        }
        self.reload();
    }

    /// Saves the source picked in the sidebar as the default.
    fn remember_source(&self) {
        let Some(source) = SOURCES
            .get(self.source.selected() as usize)
            .and_then(|(name, _)| name.parse::<earshot_core::Source>().ok())
        else {
            return;
        };
        let saved = earshot_core::Config::load().and_then(|mut config| {
            if config.source == source {
                return Ok(());
            }
            config.source = source;
            config.save()
        });
        if let Err(e) = saved {
            self.error(&format!("Could not save the source as the default: {e}"));
        }
    }

    // Recording

    fn start(self: &Rc<Self>) {
        let Some((source, _)) = SOURCES.get(self.source.selected() as usize) else {
            return;
        };
        let options = StartOptions {
            source: Some((*source).to_owned()),
            format: None,
        };
        self.request(
            |service| async move { service.start(options).await },
            |_, _| {},
        );
    }

    fn toggle_pause(self: &Rc<Self>) {
        let paused = self.model.borrow().state == "paused";
        self.request(
            move |service| async move {
                if paused {
                    service.resume().await
                } else {
                    service.pause().await
                }
            },
            |_, _| {},
        );
    }

    fn stop(self: &Rc<Self>) {
        self.request(
            |service| async move { service.stop().await },
            |ui, path| {
                ui.model.borrow_mut().select_next = Some(path);
                ui.reload();
            },
        );
    }

    fn apply_status(self: &Rc<Self>, status: Status) {
        let before = std::mem::replace(&mut self.model.borrow_mut().state, status.state.clone());
        if status.state == "idle" {
            self.top.set_visible_child_name("idle");
            if self.showing("live") {
                self.pages.set_visible_child_name("empty");
            }
            return;
        }

        if before != "recording" && before != "paused" {
            // A recording has started, or was already running when the
            // window opened.
            let tracks = Tracks::from_source(status.source.as_deref());
            self.live_wave.start_live(tracks);
            self.live_tracks.set_visible(tracks == Tracks::Both);
            self.live_meta.set_text(match tracks {
                Tracks::Microphone => "Microphone",
                Tracks::System => "System audio",
                Tracks::Both => "Microphone and system audio",
            });
            self.live_row_source
                .set_text(source_label(status.source.as_deref()));
            self.top.set_visible_child_name("recording");
            self.show_live();
        }

        let paused = status.state == "paused";
        self.live_row_label
            .set_text(if paused { "Paused" } else { "Recording" });
        self.pause_label
            .set_text(if paused { "Resume" } else { "Pause" });
        self.pause_icon
            .set(if paused { Icon::Record } else { Icon::Pause });
        if paused {
            self.time.add_css_class("paused");
        } else {
            self.time.remove_css_class("paused");
        }
        self.show_elapsed(status.elapsed_ms);
    }

    fn show_elapsed(&self, elapsed_ms: u64) {
        let elapsed = clock(elapsed_ms);
        self.time.set_text(&elapsed);
        self.live_row_time.set_text(&elapsed);
    }

    /// Switches the right side to the recording in progress.
    fn show_live(&self) {
        self.close_recording();
        self.list.unselect_all();
        self.pages.set_visible_child_name("live");
    }

    fn showing(&self, page: &str) -> bool {
        self.pages.visible_child_name().as_deref() == Some(page)
    }

    // The list

    fn reload(self: &Rc<Self>) {
        self.request(
            |service| async move { service.list().await },
            |ui, recordings| ui.populate(recordings),
        );
    }

    fn open(self: &Rc<Self>, path: String) {
        self.request(
            |service| async move { service.list().await },
            move |ui, recordings| {
                if recordings.iter().any(|r| r.path == path) {
                    ui.model.borrow_mut().select_next = Some(path);
                } else {
                    ui.error(&format!("{path} is not in the recordings folder"));
                }
                ui.populate(recordings);
            },
        );
    }

    fn populate(self: &Rc<Self>, recordings: Vec<RecordingInfo>) {
        let rows: Vec<gtk::Grid> = recordings.iter().map(row).collect();
        let wanted = {
            let mut model = self.model.borrow_mut();
            model.recordings = recordings;
            model.populating = true;
            model.select_next.take().or_else(|| model.selected.clone())
        };

        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for row in &rows {
            self.list.append(row);
        }
        self.model.borrow_mut().populating = false;

        self.empty.set_text(if rows.is_empty() {
            "No recordings yet"
        } else {
            "Select a recording"
        });

        let index = wanted.and_then(|path| {
            let model = self.model.borrow();
            model.recordings.iter().position(|r| r.path == path)
        });
        match index.and_then(|index| self.list.row_at_index(index as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None if self.model.borrow().renaming => {}
            None => {
                self.close_recording();
                if self.showing("playback") {
                    self.pages.set_visible_child_name("empty");
                }
            }
        }
    }

    fn row_selected(self: &Rc<Self>, row: Option<&gtk::ListBoxRow>) {
        let recording = {
            let model = self.model.borrow();
            if model.populating {
                return;
            }
            row.and_then(|row| model.recordings.get(row.index() as usize).cloned())
        };
        if let Some(recording) = recording {
            self.open_recording(recording);
        }
    }

    fn path_of(&self, row: &gtk::ListBoxRow) -> Option<String> {
        let model = self.model.borrow();
        model
            .recordings
            .get(row.index() as usize)
            .map(|r| r.path.clone())
    }

    /// Marks the row a menu or name field is open for, or clears the mark.
    /// Returns the path of its recording.
    fn set_target(&self, row: Option<&gtk::ListBoxRow>) -> Option<String> {
        let target = row.and_then(|row| Some((self.path_of(row)?, row.clone())));
        if let Some((_, previous)) = self.menu_target.replace(target.clone()) {
            previous.remove_css_class("menu-target");
        }
        let (path, row) = target?;
        row.add_css_class("menu-target");
        Some(path)
    }

    /// Points a popup at `at`, or below the row's name if there is no point.
    fn point_at(&self, popover: &gtk::Popover, row: &gtk::ListBoxRow, at: Option<(f64, f64)>) {
        let point = at.or_else(|| {
            let bounds = row.compute_bounds(&self.list)?;
            Some((
                f64::from(bounds.x()) + 8.0,
                f64::from(bounds.y() + bounds.height()),
            ))
        });
        if let Some((x, y)) = point {
            let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
        }
    }

    /// Opens the menu for a recording in the list, at the pointer if it was
    /// right-clicked, or below the row if it was opened from the keyboard.
    fn open_row_menu(&self, row: &gtk::ListBoxRow, at: Option<(f64, f64)>) {
        if self.set_target(Some(row)).is_some() {
            self.point_at(&self.row_menu, row, at);
            self.row_menu.popup();
        }
    }

    /// Opens the name field for a recording in the list, below its row.
    fn show_rename(&self, row: &gtk::ListBoxRow) {
        let Some(path) = self.set_target(Some(row)) else {
            return;
        };
        let name = {
            let model = self.model.borrow();
            let recording = model.recordings.iter().find(|r| r.path == path);
            recording.map(|r| r.name.clone()).unwrap_or_default()
        };
        self.rename_entry.set_text(&name);
        self.point_at(&self.rename_popover, row, None);
        self.rename_popover.popup();
        self.rename_entry.grab_focus();
    }

    // The selected recording

    fn open_recording(self: &Rc<Self>, recording: RecordingInfo) {
        self.pages.set_visible_child_name("playback");
        if self.model.borrow().selected.as_ref() == Some(&recording.path) {
            return;
        }
        {
            let mut model = self.model.borrow_mut();
            model.selected = Some(recording.path.clone());
            model.duration_ms = recording.duration_ms.unwrap_or(0);
            model.speed = 0;
        }

        self.title.set_text(&recording.name);
        self.meta.set_text(&details(&recording));
        self.position.set_text("0:00");
        self.duration
            .set_text(&recording.duration_ms.map(clock).unwrap_or_default());
        self.speed.set_label(SPEEDS[0].1);
        self.play_icon.set(Icon::Play);

        let tracks = Tracks::from_source(recording.source.as_deref());
        self.wave.show(tracks, Vec::new(), Vec::new());
        let two_tracks = tracks == Tracks::Both;
        self.track_toggles.set_visible(two_tracks);
        self.two_track_items.set_visible(two_tracks);
        if let Err(message) = self.player.load(Path::new(&recording.path), two_tracks) {
            self.error(&message);
        }
        self.show_heard(Heard::Both);

        let path = recording.path;
        self.request(
            {
                let path = path.clone();
                move |service| async move { service.waveform(&path).await }
            },
            move |ui, (microphone, system)| {
                // Ignore the answer if another recording was opened meanwhile.
                if ui.model.borrow().selected.as_ref() == Some(&path) {
                    let narrow = |peaks: Vec<f64>| peaks.into_iter().map(|p| p as f32).collect();
                    ui.wave.show(tracks, narrow(microphone), narrow(system));
                }
            },
        );
    }

    /// Stops playing and forgets the open recording.
    fn close_recording(&self) {
        self.model.borrow_mut().selected = None;
        self.player.unload();
        if let Some(tick) = self.progress_tick.take() {
            tick.remove();
        }
    }

    /// Length of the open recording, asking the player if the service did
    /// not know it.
    fn duration_ms(&self) -> u64 {
        let known = self.model.borrow().duration_ms;
        if known > 0 {
            return known;
        }
        let found = self.player.duration_ms().unwrap_or(0);
        if found > 0 {
            self.model.borrow_mut().duration_ms = found;
            self.duration.set_text(&clock(found));
        }
        found
    }

    fn show_position(&self, position_ms: u64) {
        self.position.set_text(&clock(position_ms));
        let duration = self.duration_ms();
        if duration > 0 {
            self.wave.set_progress(position_ms as f64 / duration as f64);
        }
    }

    /// Starts or pauses playback. The position display is only kept up to
    /// date, and the window only redrawn for it, while a recording plays.
    fn set_playing(self: &Rc<Self>, playing: bool) {
        if let Err(message) = self.player.set_playing(playing) {
            self.error(&message);
            return;
        }
        self.play_icon
            .set(if playing { Icon::Pause } else { Icon::Play });
        if let Some(tick) = self.progress_tick.take() {
            tick.remove();
        }
        if playing {
            let tick = self.wave.widget().add_tick_callback(glib::clone!(
                #[weak(rename_to = ui)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |_, _| {
                    if let Some(position) = ui.player.position_ms() {
                        ui.show_position(position);
                    }
                    glib::ControlFlow::Continue
                }
            ));
            self.progress_tick.replace(Some(tick));
        }
    }

    fn seek_to(&self, position_ms: u64) {
        self.player.seek_ms(position_ms);
        self.show_position(position_ms);
    }

    fn seek(&self, fraction: f64) {
        let duration = self.duration_ms();
        if duration > 0 {
            self.seek_to((fraction * duration as f64) as u64);
        }
    }

    fn skip(&self, forward: bool) {
        let position = self.player.position_ms().unwrap_or(0);
        let target = if forward {
            (position + SKIP_MS).min(self.duration_ms())
        } else {
            position.saturating_sub(SKIP_MS)
        };
        self.seek_to(target);
    }

    fn next_speed(&self) {
        let (rate, label) = {
            let mut model = self.model.borrow_mut();
            model.speed = (model.speed + 1) % SPEEDS.len();
            SPEEDS[model.speed]
        };
        self.player.set_rate(rate);
        self.speed.set_label(label);
    }

    fn player_event(self: &Rc<Self>, event: PlayerEvent) {
        match event {
            PlayerEvent::Loaded => {
                // Learns the length of a file the service had none for.
                if self.model.borrow().selected.is_some() {
                    self.duration_ms();
                }
            }
            PlayerEvent::Ended => {
                // Leave the recording ready to play again from the start.
                self.set_playing(false);
                self.seek_to(0);
            }
            PlayerEvent::Failed(message) => self.error(&format!("Playback failed: {message}")),
        }
    }

    fn rename(self: &Rc<Self>) {
        let name = self.rename_entry.text().to_string();
        let Some((path, _)) = self.menu_target.borrow().clone() else {
            return;
        };
        self.rename_popover.popdown();
        let open = self.model.borrow().selected.as_ref() == Some(&path);
        self.model.borrow_mut().renaming = open;

        let ui = self.clone();
        glib::spawn_future_local(async move {
            let renamed = ui
                .backend
                .call(move |service| async move { service.rename(&path, &name).await })
                .await;
            ui.model.borrow_mut().renaming = false;
            match renamed {
                Ok(new_path) if open => {
                    // The file has moved, so it is reopened under its new path.
                    ui.close_recording();
                    ui.model.borrow_mut().select_next = Some(new_path);
                }
                Ok(_) => {}
                Err(message) => ui.error(&message),
            }
            ui.reload();
        });
    }

    /// Chooses which tracks of a two-track recording are heard.
    fn hear(&self, heard: Heard) {
        match self.player.set_tracks(heard) {
            Ok(()) => self.show_heard(heard),
            Err(message) => self.error(&message),
        }
    }

    fn show_heard(&self, heard: Heard) {
        self.wave.set_heard(heard);
        for (option, check) in &self.heard_checks {
            check.set_text(if *option == heard { "✓" } else { "" });
        }
        for (track, toggle) in [
            (Heard::Microphone, &self.mic_toggle),
            (Heard::System, &self.system_toggle),
        ] {
            if heard == Heard::Both || heard == track {
                toggle.remove_css_class("muted");
            } else {
                toggle.add_css_class("muted");
            }
        }
    }

    /// Asks where to save one track of the open recording, then saves it in
    /// the format the chosen file name ends with.
    fn save_track(self: &Rc<Self>, track: Heard) {
        let Some(recording) = self.selected_recording() else {
            return;
        };
        let source = PathBuf::from(&recording.path);
        let extension = source
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "opus".to_owned());
        let which = if track == Heard::Microphone {
            "mic"
        } else {
            "system"
        };
        let mut dialog = gtk::FileDialog::builder()
            .title(format!("Save the {which} track"))
            .accept_label("Save")
            .initial_name(format!("{} ({which}).{extension}", recording.name));
        if let Some(folder) = source.parent() {
            dialog = dialog.initial_folder(&gio::File::for_path(folder));
        }
        let dialog = dialog.build();
        let window = self.wave.widget().root().and_downcast::<gtk::Window>();

        let ui = self.clone();
        glib::spawn_future_local(async move {
            let chosen = match dialog.save_future(window.as_ref()).await {
                Ok(file) => file.path(),
                Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
                Err(e) => {
                    ui.error(&format!("Could not choose where to save: {e}"));
                    return;
                }
            };
            let Some(mut destination) = chosen else {
                ui.error("That location is not on this computer's disks");
                return;
            };
            // The file name's extension picks the format; without a known
            // one, the recording's own format is used.
            let format = match destination
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| e.to_ascii_lowercase().parse::<Format>().ok())
            {
                Some(format) => format,
                None => {
                    let format = extension.parse::<Format>().unwrap_or(Format::Opus);
                    let mut name = destination.file_name().unwrap_or_default().to_owned();
                    name.push(format!(".{}", format.extension()));
                    destination.set_file_name(name);
                    format
                }
            };

            let saved = destination.clone();
            let result = gio::spawn_blocking(move || {
                earshot_core::tracks::export(&source, track, &destination, format)
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    let name = saved.file_name().unwrap_or_default().to_string_lossy();
                    ui.notice(&format!("Saved {name}"));
                    // It may have been saved into the recordings folder.
                    ui.reload();
                }
                Ok(Err(e)) => ui.error(&format!("Could not save the {which} track: {e}")),
                Err(_) => ui.error(&format!("Could not save the {which} track")),
            }
        });
    }

    fn selected_recording(&self) -> Option<RecordingInfo> {
        let model = self.model.borrow();
        let path = model.selected.as_ref()?;
        model.recordings.iter().find(|r| &r.path == path).cloned()
    }

    fn delete(self: &Rc<Self>, path: String) {
        if self.model.borrow().selected.as_ref() == Some(&path) {
            self.close_recording();
            self.list.unselect_all();
            self.pages.set_visible_child_name("empty");
        }
        self.request(
            move |service| async move { service.delete(&path).await },
            |_, _| {},
        );
    }
}

/// A menu item with a check mark column in front of its text.
fn check_item(text: &str) -> (gtk::Button, gtk::Label) {
    let check = gtk::Label::builder().width_chars(2).xalign(0.0).build();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.append(&check);
    content.append(&label(text, &[]));
    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("menu-item");
    (button, check)
}

/// A track name beside the waveform that can be clicked to hear only it.
fn track_toggle(text: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(&label(text, &[]))
        .tooltip_text(tooltip)
        .build();
    button.add_css_class("track-toggle");
    button
}

fn menu_item(text: &str) -> gtk::Button {
    let button = gtk::Button::builder().child(&label(text, &[])).build();
    button.add_css_class("menu-item");
    button
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

/// The names of the two tracks, shown beside a two-track waveform.
fn track_labels() -> gtk::Box {
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .homogeneous(true)
        .width_request(56)
        .build();
    labels.append(&label("Mic", &["small", "dim"]));
    labels.append(&label("System", &["small", "system"]));
    labels
}

/// A waveform with its track names beside it. Rows attached below it, in
/// column 0, line up with the waveform and not with the names.
fn wave_area(wave: &Waveform, tracks: &impl IsA<gtk::Widget>, height: i32) -> gtk::Grid {
    let area = gtk::Grid::builder().column_spacing(10).build();
    area.add_css_class("wave");
    wave.widget().set_height_request(height);
    area.attach(wave.widget(), 0, 0, 1, 1);
    area.attach(tracks, 1, 0, 1, 1);
    area
}

/// One recording in the list.
fn row(recording: &RecordingInfo) -> gtk::Grid {
    let name = label(&recording.name, &[]);
    name.set_hexpand(true);
    name.set_ellipsize(pango::EllipsizeMode::End);
    let duration = label(
        &recording.duration_ms.map(clock).unwrap_or_default(),
        &["dim", "tnum"],
    );
    let source = source_label(recording.source.as_deref());
    let mut details = started(recording.created).1;
    if !source.is_empty() {
        details = format!("{details} · {source}");
    }

    let grid = gtk::Grid::builder().column_spacing(8).build();
    grid.attach(&name, 0, 0, 1, 1);
    grid.attach(&duration, 1, 0, 1, 1);
    grid.attach(&label(&details, &["small", "dim"]), 0, 1, 2, 1);
    grid
}

fn source_label(source: Option<&str>) -> &'static str {
    match source {
        Some("microphone") => "Mic",
        Some("system") => "System",
        Some("both") => "Mic + system",
        _ => "",
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Day {
    Today,
    Yesterday,
    Earlier,
}

impl Day {
    fn label(self) -> &'static str {
        match self {
            Day::Today => "Today",
            Day::Yesterday => "Yesterday",
            Day::Earlier => "Earlier",
        }
    }
}

/// Which group of the list a recording belongs to, and how its start time
/// reads within that group.
fn started(created: u64) -> (Day, String) {
    let (Ok(now), Ok(then)) = (
        glib::DateTime::now_local(),
        glib::DateTime::from_unix_local(created as i64),
    ) else {
        return (Day::Earlier, String::new());
    };
    let day = if then.ymd() == now.ymd() {
        Day::Today
    } else if now.add_days(-1).is_ok_and(|d| d.ymd() == then.ymd()) {
        Day::Yesterday
    } else {
        Day::Earlier
    };
    let format = if day == Day::Earlier {
        "%a %-d %b"
    } else {
        "%H:%M"
    };
    let text = then.format(format).map(String::from).unwrap_or_default();
    (day, text)
}

/// The line under an open recording's name.
fn details(recording: &RecordingInfo) -> String {
    let (day, started) = started(recording.created);
    let when = match day {
        Day::Earlier => glib::DateTime::from_unix_local(recording.created as i64)
            .and_then(|then| then.format("%a %-d %b %H:%M"))
            .map(String::from)
            .unwrap_or(started),
        _ => format!("{} {started}", day.label()),
    };
    let mut parts = vec![when];
    if let Some(duration) = recording.duration_ms {
        parts.push(clock(duration));
    }
    let source = source_label(recording.source.as_deref());
    if !source.is_empty() {
        parts.push(source.to_owned());
    }
    parts.push(file_size(recording.size_bytes));
    parts.join(" · ")
}
