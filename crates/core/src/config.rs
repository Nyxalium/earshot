//! User settings, read from `~/.config/earshot/config.toml`.

use std::path::{Path, PathBuf};

use gio::glib;
use serde::{Deserialize, Serialize};

use crate::{Error, Format, Result, Source};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Folder new recordings are saved in.
    pub recordings_dir: PathBuf,
    /// File format used when a recording does not ask for one.
    pub format: Format,
    /// Audio source used when a recording does not ask for one.
    pub source: Source,
}

impl Default for Config {
    fn default() -> Self {
        let music = glib::user_special_dir(glib::UserDirectory::Music)
            .unwrap_or_else(|| glib::home_dir().join("Music"));
        Self {
            recordings_dir: music.join("Earshot"),
            format: Format::Opus,
            source: Source::Microphone,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        glib::user_config_dir().join("earshot").join("config.toml")
    }

    /// Loads the user's config. A missing file gives the defaults; a file that
    /// cannot be read or parsed is an error, so a typo never silently changes
    /// where recordings are saved.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::io(format!("could not read {}", path.display()))(e)),
        };
        toml::from_str(&text)
            .map_err(|e| Error::Invalid(format!("invalid config {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load_from(&dir.path().join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert!(config.recordings_dir.ends_with("Earshot"));
    }

    #[test]
    fn partial_file_overrides_only_its_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "format = \"flac\"\nsource = \"both\"\n").unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.format, Format::Flac);
        assert_eq!(config.source, Source::Both);
        assert_eq!(config.recordings_dir, Config::default().recordings_dir);
    }

    #[test]
    fn unknown_key_or_value_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "recording_dir = \"/tmp\"\n").unwrap();
        assert!(Config::load_from(&path).is_err());
        std::fs::write(&path, "format = \"aiff\"\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }
}
