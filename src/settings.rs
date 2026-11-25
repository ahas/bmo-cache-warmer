//! The settings file (docs/api.md, "The settings file"): the address the service listens on and its
//! commands reach it at, its store and its token file, read by the service, `mcp` and `health` alike.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The file a release's binary reads where none is named, in the folder above the binary's own.
pub const BESIDE_THE_RELEASE: &str = "settings.toml";

/// What a settings file holds, each key optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsFile {
  /// The IP address and port the service listens on and is reached at.
  pub listen: Option<SocketAddr>,
  /// The store's SQLite file.
  pub store: Option<PathBuf>,
  /// The file holding the bearer token.
  pub token_file: Option<PathBuf>,
}

impl SettingsFile {
  /// The settings of the file named, which must read; else those of the file beside the running
  /// binary's release where it stands; else none.
  pub fn find(named: Option<&Path>) -> Result<Self, String> {
    match named {
      Some(file) => Self::read(file),
      None => match beside_the_release().filter(|file| file.is_file()) {
        Some(file) => Self::read(&file),
        None => Ok(Self::default()),
      },
    }
  }

  /// The settings a file holds; refused where it does not read or is not of the file's shape.
  pub fn read(file: &Path) -> Result<Self, String> {
    let text = std::fs::read_to_string(file)
      .map_err(|error| format!("The settings file {} does not read: {error}.", file.display()))?;
    toml::from_str(&text).map_err(|error| {
      format!("The settings file {} is not the warmer's settings: {error}", file.display())
    })
  }
}

/// `settings.toml` in the folder above the one holding the running binary, its path read with every link
/// resolved.
fn beside_the_release() -> Option<PathBuf> {
  let binary = std::env::current_exe().ok()?.canonicalize().ok()?;
  Some(binary.parent()?.parent()?.join(BESIDE_THE_RELEASE))
}
