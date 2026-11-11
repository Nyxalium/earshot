//! The recordings folder: ordinary audio files, plus a small metadata file
//! per recording kept in a hidden `.earshot` subfolder.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use gio::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, Source};

const AUDIO_EXTENSIONS: &[&str] = &["opus", "ogg", "flac", "wav", "mp3", "m4a"];
const META_DIR: &str = ".earshot";

/// What Earshot knows about a recording beyond the audio file itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Start time, in seconds since the Unix epoch.
    pub created: u64,
    pub duration_ms: u64,
    pub source: Option<Source>,
    /// Loudness over time, each value from 0 to 1, for drawing a waveform.
    pub peaks: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recording {
    pub path: PathBuf,
    /// File name without its extension.
    pub name: String,
    /// Start time, in seconds since the Unix epoch.
    pub created: u64,
    /// Unknown for audio files Earshot did not record.
    pub duration_ms: Option<u64>,
    pub size_bytes: u64,
    pub source: Option<Source>,
}

pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Lists the audio files in the folder, newest first. A folder that does
    /// not exist yet is an empty library.
    pub fn list(&self) -> Result<Vec<Recording>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(self.io_error("could not read")(e)),
        };

        let mut recordings = Vec::new();
        for entry in entries {
            let entry = entry.map_err(self.io_error("could not read"))?;
            let path = entry.path();
            if !is_audio_file(&path) {
                continue;
            }
            let file = entry
                .metadata()
                .map_err(Error::io(format!("could not inspect {}", path.display())))?;
            if !file.is_file() {
                continue;
            }
            // Unreadable metadata must not hide the recording itself.
            let meta = self.meta(&path).ok().flatten();
            let modified = file
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            recordings.push(Recording {
                name: stem(&path),
                created: meta.as_ref().map_or(modified, |m| m.created),
                duration_ms: meta.as_ref().map(|m| m.duration_ms),
                size_bytes: file.len(),
                source: meta.and_then(|m| m.source),
                path,
            });
        }
        recordings.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.name.cmp(&a.name)));
        Ok(recordings)
    }

    /// Reads a recording's metadata, or `None` if it has none.
    pub fn meta(&self, recording: &Path) -> Result<Option<Meta>> {
        let path = self.meta_path(recording)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io(format!("could not read {}", path.display()))(e)),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| Error::Invalid(format!("invalid metadata {}: {e}", path.display())))
    }

    pub fn write_meta(&self, recording: &Path, meta: &Meta) -> Result<()> {
        let path = self.meta_path(recording)?;
        let dir = self.dir.join(META_DIR);
        fs::create_dir_all(&dir)
            .map_err(Error::io(format!("could not create {}", dir.display())))?;
        let json = serde_json::to_string(meta).expect("metadata serialises");
        fs::write(&path, json).map_err(Error::io(format!("could not write {}", path.display())))
    }

    /// Renames a recording, keeping its extension. Returns the new path.
    pub fn rename(&self, recording: &Path, new_name: &str) -> Result<PathBuf> {
        let from = self.resolve(recording)?;
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(Error::Invalid("a recording name cannot be empty".into()));
        }
        if new_name.starts_with('.') || new_name.contains(['/', '\0']) {
            return Err(Error::Invalid(format!(
                "\"{new_name}\" is not a valid recording name: it cannot start with a dot or contain a slash"
            )));
        }

        let mut file_name = new_name.to_owned();
        if let Some(ext) = from.extension() {
            file_name.push('.');
            file_name.push_str(&ext.to_string_lossy());
        }
        let to = self.dir.join(file_name);
        if to == from {
            return Ok(to);
        }
        if to.exists() {
            return Err(Error::Invalid(format!(
                "a recording named \"{new_name}\" already exists"
            )));
        }

        fs::rename(&from, &to)
            .map_err(Error::io(format!("could not rename {}", from.display())))?;
        let (meta_from, meta_to) = (self.meta_path(&from)?, self.meta_path(&to)?);
        if meta_from.exists() {
            fs::rename(&meta_from, &meta_to).map_err(Error::io(format!(
                "could not rename {}",
                meta_from.display()
            )))?;
        }
        Ok(to)
    }

    /// Moves a recording to the desktop's trash and removes its metadata.
    pub fn delete(&self, recording: &Path) -> Result<()> {
        let path = self.resolve(recording)?;
        gio::File::for_path(&path)
            .trash(gio::Cancellable::NONE)
            .map_err(|e| {
                Error::Invalid(format!(
                    "could not move {} to the trash: {e}",
                    path.display()
                ))
            })?;
        self.remove_meta(&path)
    }

    pub(crate) fn remove_meta(&self, recording: &Path) -> Result<()> {
        let meta = self.meta_path(recording)?;
        match fs::remove_file(&meta) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(Error::io(format!("could not remove {}", meta.display()))(e))
            }
            _ => Ok(()),
        }
    }

    /// Accepts a path or a bare file name and returns the path of that
    /// recording inside this library. Anything outside the recordings folder
    /// is refused, so a caller can never rename or trash unrelated files.
    pub fn resolve(&self, recording: &Path) -> Result<PathBuf> {
        let not_ours = || {
            Error::Invalid(format!(
                "{} is not a recording in {}",
                recording.display(),
                self.dir.display()
            ))
        };
        let file_name = recording.file_name().ok_or_else(not_ours)?;
        let parent = recording.parent().filter(|p| !p.as_os_str().is_empty());
        if let Some(parent) = parent {
            let same_dir = match (parent.canonicalize(), self.dir.canonicalize()) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if !same_dir {
                return Err(not_ours());
            }
        }
        let path = self.dir.join(file_name);
        if !is_audio_file(&path) || !path.is_file() {
            return Err(not_ours());
        }
        Ok(path)
    }

    fn meta_path(&self, recording: &Path) -> Result<PathBuf> {
        let mut name = recording
            .file_name()
            .ok_or_else(|| Error::Invalid(format!("{} has no file name", recording.display())))?
            .to_owned();
        name.push(".json");
        Ok(self.dir.join(META_DIR).join(name))
    }

    fn io_error(&self, action: &str) -> impl FnOnce(std::io::Error) -> Error {
        Error::io(format!(
            "{action} the recordings folder {}",
            self.dir.display()
        ))
    }
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Reduces a list of peaks to at most `max` values, keeping the loudest peak
/// of each group so short spikes stay visible in a waveform.
pub fn downsample_peaks(peaks: &[f32], max: usize) -> Vec<f32> {
    if max == 0 || peaks.len() <= max {
        return peaks.to_vec();
    }
    (0..max)
        .map(|i| {
            let start = i * peaks.len() / max;
            let end = ((i + 1) * peaks.len() / max).max(start + 1);
            peaks[start..end].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_with(files: &[&str]) -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            fs::write(dir.path().join(file), b"audio").unwrap();
        }
        let library = Library::new(dir.path());
        (dir, library)
    }

    fn meta(created: u64) -> Meta {
        Meta {
            created,
            duration_ms: 1500,
            source: Some(Source::Both),
            peaks: vec![0.25, 0.5],
        }
    }

    #[test]
    fn missing_folder_is_an_empty_library() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path().join("nope"));
        assert_eq!(library.list().unwrap(), Vec::new());
    }

    #[test]
    fn lists_only_audio_files_newest_first_with_metadata() {
        let (dir, library) = library_with(&["old.opus", "new.flac", "notes.txt"]);
        fs::create_dir(dir.path().join("folder.wav")).unwrap();
        library
            .write_meta(&dir.path().join("old.opus"), &meta(100))
            .unwrap();
        library
            .write_meta(&dir.path().join("new.flac"), &meta(200))
            .unwrap();

        let list = library.list().unwrap();
        let names: Vec<_> = list.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["new", "old"]);
        assert_eq!(list[0].duration_ms, Some(1500));
        assert_eq!(list[0].source, Some(Source::Both));
        assert_eq!(list[0].size_bytes, 5);
    }

    #[test]
    fn corrupt_metadata_does_not_hide_the_recording() {
        let (dir, library) = library_with(&["a.opus"]);
        fs::create_dir(dir.path().join(META_DIR)).unwrap();
        fs::write(dir.path().join(META_DIR).join("a.opus.json"), "{oops").unwrap();

        let list = library.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].duration_ms, None);
        assert!(library.meta(&dir.path().join("a.opus")).is_err());
    }

    #[test]
    fn rename_keeps_extension_and_moves_metadata() {
        let (dir, library) = library_with(&["a.opus"]);
        let original = dir.path().join("a.opus");
        library.write_meta(&original, &meta(100)).unwrap();

        let renamed = library.rename(&original, "  Band practice ").unwrap();

        assert_eq!(renamed, dir.path().join("Band practice.opus"));
        assert!(!original.exists());
        assert_eq!(library.meta(&renamed).unwrap(), Some(meta(100)));
        assert_eq!(library.meta(&original).unwrap(), None);
    }

    #[test]
    fn rename_refuses_to_overwrite_or_escape_the_folder() {
        let (dir, library) = library_with(&["a.opus", "b.opus"]);
        let a = dir.path().join("a.opus");

        assert!(library.rename(&a, "b").is_err());
        assert!(library.rename(&a, "../escaped").is_err());
        assert!(library.rename(&a, ".hidden").is_err());
        assert!(library.rename(&a, "   ").is_err());

        assert!(a.exists());
        assert_eq!(fs::read(dir.path().join("b.opus")).unwrap(), b"audio");
        assert!(!dir.path().parent().unwrap().join("escaped.opus").exists());
    }

    #[test]
    fn resolve_refuses_files_outside_the_folder() {
        let (dir, library) = library_with(&["a.opus", "notes.txt"]);
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("a.opus"), b"other").unwrap();

        assert_eq!(
            library.resolve(Path::new("a.opus")).unwrap(),
            dir.path().join("a.opus")
        );
        assert!(library.resolve(&dir.path().join("a.opus")).is_ok());
        assert!(library.resolve(&outside.path().join("a.opus")).is_err());
        assert!(library.resolve(&dir.path().join("notes.txt")).is_err());
        assert!(library.resolve(Path::new("missing.opus")).is_err());
        assert!(library.delete(&outside.path().join("a.opus")).is_err());
        assert!(outside.path().join("a.opus").exists());
    }

    #[test]
    fn downsample_keeps_the_loudest_peak_of_each_group() {
        let peaks = [0.1, 0.9, 0.2, 0.3, 0.8, 0.1];
        assert_eq!(downsample_peaks(&peaks, 3), [0.9, 0.3, 0.8]);
        assert_eq!(downsample_peaks(&peaks, 6), peaks);
        assert_eq!(downsample_peaks(&peaks, 100), peaks);
        assert_eq!(downsample_peaks(&[0.4, 0.7, 0.5], 2).len(), 2);
    }
}
