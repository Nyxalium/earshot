# Earshot

A sound recorder for Linux, written in Rust. A background service does the
recording. A window and the `earshot` terminal command both control it, so
they always show the same recording. An MCP server for AI agents is planned;
see [docs/plan.md](docs/plan.md).

Earshot is at an early stage. Today it can:

- record the microphone, the system's audio output, or both
- keep the two as separate tracks when recording both (see below)
- save as Opus, FLAC or WAV, as ordinary files in `~/Music/Earshot`
- pause, resume, stop and cancel a recording
- list, search, play, rename and delete saved recordings (delete moves them
  to the trash)
- show a desktop notification when a recording is saved or fails
- save the recording in progress if the service is shut down

## Build

You need:

- Rust 1.96 or newer
- GStreamer 1.x with the base and good plugin sets
- GTK 4.14 or newer, for the window
- PulseAudio, or PipeWire with `pipewire-pulse`

```sh
cargo build --release
```

This builds three programs in `target/release`: `earshot-app` (the window),
`earshot` (the command) and `earshotd` (the service). Keep them in the same
folder: the window and the command start the service when it is not already
running.

## The window

```sh
earshot-app
earshot-app ~/Music/Earshot/"Band practice.opus"   # open with a recording selected
```

Recordings are listed on the left. The right side shows the recording in
progress, or the one you select: its waveform, playback with seeking and
speed, and Rename and Delete. A recording keeps going if you close the window.

## The command

```sh
earshot start                           # record the microphone
earshot start --source system           # record what the computer is playing
earshot start --source both --format flac
earshot pause
earshot resume
earshot stop                            # save the recording
earshot cancel                          # discard it
earshot toggle                          # start, or stop and save
earshot status
earshot list
earshot rename "2026-11-11 12.50.47.opus" "Band practice"
earshot delete "Band practice.opus"
```

Add `--json` to any command for machine-readable output.

## Two tracks

Recording the microphone and system audio together does not mix them. The file
has four channels: the microphone on the front pair and system audio on the
back pair. Other players play that as a normal mix, and Earshot draws the
microphone above the line of the waveform and system audio below it.

## Settings

Settings are optional and live in `~/.config/earshot/config.toml`:

```toml
recordings_dir = "/home/you/Music/Earshot"
format = "opus"         # opus, flac or wav
source = "microphone"   # microphone, system or both
```

## Layout

| Crate | What it is |
| --- | --- |
| `crates/core` | Recording engine (GStreamer) and the recordings folder |
| `crates/client` | The service's D-Bus interface, for programs that control it |
| `crates/service` | `earshotd`, the background service |
| `crates/cli` | `earshot`, the terminal command |
| `crates/app` | `earshot-app`, the window (GTK4) |

The service owns the bus name `io.github.nyxalium.Earshot.Service` on the session bus.

## Tests

```sh
cargo test
```

The recorder tests run real GStreamer pipelines with a generated test tone, so
they need GStreamer installed but no microphone.

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
