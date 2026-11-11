# Earshot

A sound recorder for Linux, written in Rust. A background service does the
recording, and the `earshot` terminal command controls it. A window and an MCP
server for AI agents are planned; see [docs/plan.md](docs/plan.md).

Earshot is at an early stage. Today it can:

- record the microphone, the system's audio output, or both mixed together
- save as Opus, FLAC or WAV, as ordinary files in `~/Music/Earshot`
- pause, resume, stop and cancel a recording
- list, rename and delete saved recordings (delete moves them to the trash)
- show a desktop notification when a recording is saved or fails
- save the recording in progress if the service is shut down

## Build

You need:

- Rust 1.96 or newer
- GStreamer 1.x with the base and good plugin sets
- PulseAudio, or PipeWire with `pipewire-pulse`

```sh
cargo build --release
```

This builds `target/release/earshot` (the command) and `target/release/earshotd`
(the service). Keep the two in the same folder: the command starts the service
when it is not already running.

## Use

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

The service owns the bus name `io.github.nyxalium.Earshot` on the session bus.

## Tests

```sh
cargo test
```

The recorder tests run real GStreamer pipelines with a generated test tone, so
they need GStreamer installed but no microphone.

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
