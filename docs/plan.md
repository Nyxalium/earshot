# Plan

## Decisions

- **Platform:** Linux only.
- **Language:** Rust.
- **Shape:** one background service (`earshotd`) does all recording. The window,
  the `earshot` terminal command and the MCP server control it over D-Bus, so
  they all see the same recording, and recording continues when the window is
  closed.
- **Sources:** microphone, system audio, or both mixed.
- **Storage:** ordinary audio files in `~/Music/Earshot` (changeable). Extra
  data per recording, such as its waveform and later its transcript, is kept in
  a hidden `.earshot` subfolder, so the files stay usable without Earshot.
- **Window:** GTK4 with Earshot's own stylesheet, not libadwaita. A theme is a
  CSS file plus visualiser settings.
- **Failures are states:** a recording that cannot start is an error returned
  to whoever asked for it. One that breaks later is reported, keeps the audio
  recorded so far and returns the recorder to idle. Nothing is only logged.
- **Modules:** post-recording steps (transcription and so on) are built into
  the app behind one interface and enabled in settings. Loading third-party
  plugins is deferred until the built-in modules have settled that interface.
- **Models:** modules that need model files download them through a shared
  model manager, into a folder the user can change.

## Version 0.1: the recorder

- [x] Recording engine: microphone, system audio, both; Opus, FLAC, WAV
- [x] Recordings folder: list, rename, delete to trash, waveform data
- [x] Background service with a D-Bus interface
- [x] `earshot` terminal command, with `--json`
- [x] Notifications when a recording is saved or fails
- [ ] `earshot mcp`: MCP server for agents (record, list, rename, delete)
- [ ] Window: record, pause, stop, list with playback, rename, delete
- [ ] Visualisers (waveform, spectrum, level meter) and themes
- [ ] Recording indicator in the system tray, with elapsed time and stop
- [ ] Global shortcut: a desktop shortcut bound to `earshot toggle`, which
      starts the service on demand, so nothing has to run at login
- [ ] Choice of input and output device
- [ ] Packaging: D-Bus service file, desktop file, AUR package, Flatpak

## Version 0.2: transcription

- [ ] Module interface and settings
- [ ] Model manager: download, verify, show progress, remove
- [ ] Transcription with whisper.cpp, on demand
- [ ] Transcribe automatically after each recording (setting)
- [ ] Search across transcripts, from the window, the command and MCP

## Later

- [ ] Live transcription while recording
- [ ] Summaries and titles from a local language model
- [ ] Speaker labels
- [ ] Audio clean-up: noise reduction, silence trimming, loudness
- [ ] Export: other audio formats; transcripts as text, Markdown, subtitles

## Open questions

- When recording both sources, keep the microphone and system audio as
  separate tracks instead of mixing them? That would make speaker labels much
  easier (microphone is you, system audio is everyone else), at the cost of
  files some players handle less well.
