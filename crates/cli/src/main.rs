//! The `earshot` terminal command: controls the Earshot service.

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use earshot_client::{RecordingInfo, StartOptions, Status};
use serde_json::json;

#[derive(Parser)]
#[command(name = "earshot", version, about = "Record audio from the terminal")]
struct Cli {
    /// Print results as JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a recording
    Start(StartArgs),
    /// Pause the recording in progress
    Pause,
    /// Resume a paused recording
    Resume,
    /// Finish the recording and save it
    Stop,
    /// Discard the recording in progress
    Cancel,
    /// Start a recording, or stop and save the one in progress
    Toggle(StartArgs),
    /// Show whether a recording is in progress
    Status,
    /// List saved recordings, newest first
    List,
    /// Rename a saved recording
    Rename {
        /// Path or file name of the recording
        recording: String,
        /// New name, without the file extension
        name: String,
    },
    /// Move a saved recording to the trash
    Delete {
        /// Path or file name of the recording
        recording: String,
    },
}

#[derive(clap::Args)]
struct StartArgs {
    /// Where the audio comes from [default: from the config file, else microphone]
    #[arg(long, value_enum)]
    source: Option<Source>,
    /// File format [default: from the config file, else opus]
    #[arg(long, value_enum)]
    format: Option<Format>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Source {
    Microphone,
    System,
    Both,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Opus,
    Flac,
    Wav,
}

impl From<StartArgs> for StartOptions {
    fn from(args: StartArgs) -> Self {
        Self {
            source: args.source.map(value_name),
            format: args.format.map(value_name),
        }
    }
}

fn value_name(value: impl ValueEnum) -> String {
    value
        .to_possible_value()
        .expect("no skipped values")
        .get_name()
        .to_owned()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("earshot: {}", error_text(&e));
            ExitCode::FAILURE
        }
    }
}

/// The service's own message, without the D-Bus error name in front of it.
fn error_text(error: &zbus::Error) -> String {
    match error {
        zbus::Error::FDO(error) => match &**error {
            zbus::fdo::Error::Failed(message) => message.clone(),
            other => other.to_string(),
        },
        zbus::Error::Failure(message) => message.clone(),
        other => other.to_string(),
    }
}

async fn run(cli: Cli) -> zbus::Result<()> {
    let earshot = earshot_client::connect().await?;
    let json = cli.json;
    match cli.command {
        Command::Start(args) => started(json, earshot.start(args.into()).await?),
        Command::Pause => {
            earshot.pause().await?;
            print_status(json, &earshot.status().await?);
        }
        Command::Resume => {
            earshot.resume().await?;
            print_status(json, &earshot.status().await?);
        }
        Command::Stop => saved(json, earshot.stop().await?),
        Command::Cancel => {
            earshot.cancel().await?;
            done(json, "Recording discarded");
        }
        Command::Toggle(args) => {
            if earshot.status().await?.state == "idle" {
                started(json, earshot.start(args.into()).await?);
            } else {
                saved(json, earshot.stop().await?);
            }
        }
        Command::Status => print_status(json, &earshot.status().await?),
        Command::List => print_list(json, &earshot.list().await?),
        Command::Rename { recording, name } => {
            let path = earshot.rename(&recording, &name).await?;
            if json {
                println!("{}", json!({ "path": path }));
            } else {
                println!("Renamed to {path}");
            }
        }
        Command::Delete { recording } => {
            earshot.delete(&recording).await?;
            done(json, "Moved to the trash");
        }
    }
    Ok(())
}

fn started(json: bool, path: String) {
    if json {
        println!("{}", json!({ "state": "recording", "path": path }));
    } else {
        println!("Recording to {path}");
    }
}

fn saved(json: bool, path: String) {
    if json {
        println!("{}", json!({ "state": "idle", "path": path }));
    } else {
        println!("Saved {path}");
    }
}

fn done(json: bool, message: &str) {
    if json {
        println!("{}", json!({ "ok": true }));
    } else {
        println!("{message}");
    }
}

fn print_status(json: bool, status: &Status) {
    if json {
        println!(
            "{}",
            json!({
                "state": status.state,
                "elapsed_ms": status.elapsed_ms,
                "path": status.path,
                "source": status.source,
            })
        );
        return;
    }
    match &status.path {
        Some(path) => println!(
            "{} {} ({}) to {path}",
            capitalised(&status.state),
            clock(status.elapsed_ms),
            status.source.as_deref().unwrap_or("unknown source"),
        ),
        None => println!("{}", capitalised(&status.state)),
    }
}

fn print_list(json: bool, recordings: &[RecordingInfo]) {
    if json {
        let list: Vec<_> = recordings
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "name": r.name,
                    "created": r.created,
                    "duration_ms": r.duration_ms,
                    "size_bytes": r.size_bytes,
                    "source": r.source,
                })
            })
            .collect();
        println!("{}", json!(list));
        return;
    }
    if recordings.is_empty() {
        println!("No recordings");
        return;
    }
    for recording in recordings {
        println!(
            "{:>8}  {:>9}  {}",
            recording.duration_ms.map_or_else(|| "-".to_owned(), clock),
            size(recording.size_bytes),
            file_name(&recording.path),
        );
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Formats a duration as `m:ss`, or `h:mm:ss` from an hour up.
fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
