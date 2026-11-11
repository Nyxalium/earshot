//! Recording engine and recordings library shared by the Earshot service,
//! terminal command and window.

pub mod config;
pub mod library;
pub mod recorder;

pub use config::Config;
pub use library::{Library, Meta, Recording};
pub use recorder::{Event, Format, Recorder, Source, StartOptions, State, Status};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("a recording is already in progress")]
    AlreadyRecording,
    #[error("no recording is in progress")]
    NotRecording,
    #[error("could not start recording: {0}")]
    Start(String),
    #[error("recording failed: {0}")]
    Pipeline(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> Error {
        let context = context.into();
        move |source| Error::Io { context, source }
    }
}

/// Declares a unit enum that has a stable lowercase name, used in the config
/// file, over D-Bus and on the command line.
macro_rules! named_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const NAMES: &[&str] = &[$($text),+];

            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::Error;

            fn from_str(s: &str) -> $crate::Result<Self> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    other => Err($crate::Error::Invalid(format!(
                        "unknown {} \"{other}\" (expected one of: {})",
                        stringify!($name).to_lowercase(),
                        Self::NAMES.join(", "),
                    ))),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
pub(crate) use named_enum;
