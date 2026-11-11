# Notes for coding agents

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. This file covers what is easy to
get wrong when working on Earshot from an agent session, usually on a machine
where someone is also using it.

## Build and check

```sh
cargo fmt
cargo build
cargo test
```

## Never touch the user's real Earshot

The person running you may have Earshot open, with recordings in
`~/Music/Earshot`. Keep your testing away from both.

- **Use a scratch config.** Settings come from
  `$XDG_CONFIG_HOME/earshot/config.toml`. Point `XDG_CONFIG_HOME` at a temporary
  folder whose `config.toml` sets `recordings_dir` to a temporary folder, for
  every `earshotd`, `earshot` and `earshot-app` you start.
- **Use a private D-Bus session that cannot start services.** The service
  owns `io.github.nyxalium.Earshot.Service` and the window registers
  `io.github.nyxalium.Earshot` on the session bus. On the user's bus, your test
  window would just raise their window, and your commands would control their
  recorder. Run tests inside
  `dbus-run-session --config-file=dev/test-bus.conf -- <script>`. That config
  has no service directories. With the default config, the private bus starts
  desktop services on demand (portals, the secret service, KDE Connect), and
  they outlive the test and can draw dialogs on the user's real screen.
- **Turn off portals** with `GDK_DEBUG=no-portals` when starting the window,
  so file pickers are GTK's own and appear inside the test display.
- **Don't press Open in Settings** during tests. It launches the user's real
  file manager.
- **Trash only works under the home folder.** Recordings in `/tmp` cannot be
  moved to the trash, so delete fails there by design. To test delete, use a
  scratch folder under `~/.cache`. Afterwards, remove from
  `~/.local/share/Trash` only the entries whose `.trashinfo` `Path=` points
  into that folder.
- Only stop processes you started, by PID.

## Testing the window without a desktop

These setups have worked:

- **Broadway and headless Chromium:** run `gtk4-broadwayd :7`, start the app
  with `GDK_BACKEND=broadway BROADWAY_DISPLAY=:7`, open
  `http://127.0.0.1:8087/` in headless Chromium, and drive it with the DevTools
  protocol (`Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`,
  `Page.captureScreenshot`). Popups are placed correctly and input is reliable.
  This is the best option for anything that opens a menu.
- **cage:** `WLR_BACKENDS=headless cage -- earshot-app`, with screenshots taken
  by `grim`. Fine for screenshots. With `GDK_BACKEND=x11` you can click with
  `xdotool`, but popups appear at the screen's top-left corner and clicks
  inside them are unreliable.

Check results in the files (was the recording renamed, trashed, saved?), not
only in screenshots.

## Design decisions to keep

These are explained in [docs/plan.md](docs/plan.md):

- The service does all recording. Clients never record on their own.
- A failure is a state the user sees, never only a log line.
- Recording both sources gives one four-channel file: microphone on the front
  pair, system audio on the back pair. Don't mix them down.
- The waveform draws the microphone above the centre line and system audio
  below it.
- Rename and delete live in the recordings list's right-click menu, not on the
  open recording's page.
- Colours come only from the theme's palette (`crates/app/src/theme.rs`).
  `style.css` uses the named colours and never a literal one, so every theme
  restyles everything. A new theme is one entry in `THEMES`.
- Settings only the window uses go in `window.toml`, not in `config.toml`. The
  service refuses config keys it does not know, and a service left running
  from an older version would refuse new ones.
