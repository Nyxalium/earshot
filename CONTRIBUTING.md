# Contributing to Earshot

Contributions are welcome: code, bug reports, design feedback, testing on
hardware and desktops we don't have. You don't need permission to start. Open
an issue or a pull request.

## Why Earshot exists

Earshot started because GNOME Sound Recorder jammed. Its recordings folder was
a link to a folder that no longer existed. Recording silently failed, the
app stayed in "recording" mode, and the Delete dialog threw an error every time
it was pressed, so the window could not be closed. The app has since been
renamed Vocalis, and upstream development has mostly stopped.

Rather than patch it, Earshot is a new recorder built around two ideas:

- **Failures are states, not log lines.** A recording that cannot start is an
  error shown to whoever asked for it. One that breaks partway keeps the audio
  recorded so far and returns the recorder to idle. Nothing fails silently.
- **One recorder, many ways to control it.** A background service does the
  recording. The window, the `earshot` command and (soon) an MCP server for AI
  agents all control the same service. A recording an agent starts shows up in
  the window.

## What it aims to be

A recorder for everything you'd want to capture on a Linux desktop: voice
notes, meetings and calls, music, or whatever the computer is playing. When it
records the microphone and system audio together it keeps them as two tracks,
so later steps can tell your voice from everyone else's.

After the recorder itself, the plan is optional modules that work on a
recording:

- transcription with local models (whisper.cpp), downloaded and managed by the
  app
- search across transcripts
- summaries and titles from a local language model
- speaker labels
- audio clean-up
- export

The checklist, with what's done and what's next, is in
[docs/plan.md](docs/plan.md).

## AI contributions are welcome, and encouraged

Write your contribution however you like: by hand, with an AI coding agent,
or both. Pull requests written mostly or entirely by AI are welcome. Earshot
itself has been built largely with Claude, and the current models,
**Claude Fable 5.2** and **Claude Opus 5.5**, are a great fit for work like
this. Other capable models are welcome too.

AI-written changes are reviewed the same way as any other, and the same things
make a pull request easy to accept:

- It does one thing, and the description says what and why.
- It says what was actually run: which tests, and whether the change was tried
  in the real app. "I ran `cargo test` and recorded from the window" is more
  useful than "tested".
- Tests that can fail. If you broke the behaviour a test protects, it should go
  red. A test that only asserts a mock was called doesn't count.

You don't need to say whether you used AI. If you did, mentioning the model in
the pull request helps us learn what works.

[AGENTS.md](AGENTS.md) has notes written for coding agents: how to build, test
and run Earshot without touching the user's real recordings or their running
copy. Agents that read `AGENTS.md` or `CLAUDE.md` pick it up automatically.

## Getting started

You need Rust 1.96 or newer, GStreamer with the base and good plugin sets,
PipeWire (with `pipewire-pulse`) or PulseAudio, and GTK 4.14 or newer.

```sh
cargo build
cargo test
```

The recorder tests run real GStreamer pipelines on a generated test tone, so
they need no microphone.

| Crate | What it is |
| --- | --- |
| `crates/core` | Recording engine (GStreamer) and the recordings folder |
| `crates/client` | The service's D-Bus interface, shared by everything that controls it |
| `crates/service` | `earshotd`, the background service |
| `crates/cli` | `earshot`, the terminal command |
| `crates/app` | `earshot-app`, the window (GTK4, with its own stylesheet) |

## Good places to start

From [docs/plan.md](docs/plan.md):

- `earshot mcp`, an MCP server so agents can record, list and manage recordings
- a tray indicator showing that a recording is in progress, with a stop button
- choosing the input and output device
- waveforms for audio files Earshot did not record
- packaging: a D-Bus service file, a desktop file, an AUR package, a Flatpak
- a new theme: one entry in `crates/app/src/theme.rs`

Bug reports are just as valuable. Say which desktop, audio setup and version
you're on, what you did, and what happened. The output of `earshot status`
and the service's error message help most.

## Style

- Run `cargo fmt` before committing.
- Keep changes focused. A fix doesn't need a refactor of the code around it.
- Error messages say what went wrong and what it was about, for example "the
  recordings folder … is a link to …, which does not exist", not "I/O error".
- Interface text is plain and functional. No taglines or decorative copy.

## Licence

Earshot is GPL-3.0-or-later. By contributing you agree your contribution is
released under the same licence.
