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

    /// Saves the config to the user's config file.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Writes the whole config to a temporary file and moves it into place,
    /// so a crash mid-write never leaves a half-written config behind.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self)
            .map_err(|e| Error::Invalid(format!("could not write the config: {e}")))?;
        write_whole(path, &text)
    }
}

/// Replaces a settings file in one step, creating its folder if needed: the
/// text goes to a temporary file that is then moved into place.
pub fn write_whole(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(Error::io(format!("could not create {}", dir.display())))?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, text).map_err(Error::io(format!(
        "could not write {}",
        temporary.display()
    )))?;
    std::fs::rename(&temporary, path)
        .map_err(Error::io(format!("could not replace {}", path.display())))
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
    fn saved_config_loads_back_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new folder").join("config.toml");
        let config = Config {
            recordings_dir: PathBuf::from("/srv/recordings/with \"quotes\" and spaces"),
            format: Format::Wav,
            source: Source::System,
        };

        config.save_to(&path).unwrap();

        assert_eq!(Config::load_from(&path).unwrap(), config);
        let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1, "no temporary file is left behind");
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
