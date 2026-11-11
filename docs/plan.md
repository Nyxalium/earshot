# Plan

## Decisions

- **Platform:** Linux only.
- **Language:** Rust.
- **Shape:** one background service (`earshotd`) does all recording. The window,
  the `earshot` terminal command and the MCP server control it over D-Bus, so
  they all see the same recording, and recording continues when the window is
  closed.
- **Sources:** microphone, system audio, or both. When both are recorded they
  are kept as two tracks, not mixed, so later steps can tell the user's voice
  from everyone else's. The two tracks are the front and back channel pairs of
  one four-channel file, which other players play as a normal mix.
- **Storage:** ordinary audio files in `~/Music/Earshot` (changeable). Extra
  data per recording, such as its waveform and later its transcript, is kept in
  a hidden `.earshot` subfolder, so the files stay usable without Earshot.
- **Window:** GTK4 with Earshot's own stylesheet, not libadwaita. A theme is a
  CSS file plus visualiser settings. Two panes: recordings on the left, the
  recording in progress or the selected one on the right, with room under the
  waveform for its transcript.
- **Waveform:** the microphone is drawn above the centre line and system audio
  below it.
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
- [x] Record both sources as two tracks instead of mixing them
- [x] Recordings folder: list, rename, delete to trash, waveform data
- [x] Background service with a D-Bus interface
- [x] `earshot` terminal command, with `--json`
- [x] Notifications when a recording is saved or fails
- [ ] `earshot mcp`: MCP server for agents (record, list, rename, delete)
- [x] Window: record, pause, stop, list with search and playback, rename,
      delete
- [x] Two-track waveform, live and for saved recordings
- [ ] Waveform for audio files Earshot did not record
- [ ] More visualisers (spectrum, level meter) and themes
- [x] Settings window: recordings folder, format, default source
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
