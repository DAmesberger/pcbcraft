//! Per-user settings (S3): library sources, favourites and recents. Stored
//! as `settings.json` in the user config directory, never in a project.
//! Favourites and recents are keyed by `EntryKey` (library id or project,
//! and lineage), never by path or name.
use crate::persistence::{atomic_write, read_bytes};
use crate::{Error, Result};
use pcbcraft_core::catalog::EntryKey;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The settings file's format; any other version is refused.
pub const SETTINGS_FORMAT: u32 = 1;
pub const SETTINGS_FILE: &str = "settings.json";
/// Configured sources.
pub const MAX_SOURCES: usize = 64;
/// Favourites kept.
pub const MAX_FAVOURITES: usize = 256;
/// Recents kept, most recent first.
pub const MAX_RECENT: usize = 32;

/// One configured library folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceSetting {
    pub path: PathBuf,
    /// Turned off sources stay listed with the reason "turned off".
    pub enabled: bool,
    /// Whether `library.promote` may write to it.
    pub writable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserSettings {
    pub format: u32,
    pub sources: Vec<SourceSetting>,
    pub favourites: Vec<EntryKey>,
    /// Most recent first.
    pub recent: Vec<EntryKey>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            format: SETTINGS_FORMAT,
            sources: Vec::new(),
            favourites: Vec::new(),
            recent: Vec::new(),
        }
    }
}

/// Where per-user files live: `PCBCRAFT_CONFIG_DIR` / `PCBCRAFT_CACHE_DIR`
/// when set (tests, sandboxes), else the platform's config and cache
/// directories under `pcbcraft`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserDirs {
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl UserDirs {
    pub fn from_env() -> Result<Self> {
        Err(crate::unsupported(
            "locating the user config and cache directories",
        ))
    }
    pub fn settings_path(&self) -> PathBuf {
        self.config.join(SETTINGS_FILE)
    }
}

/// Read settings; a missing file is the defaults. An unsupported format is
/// refused, never rewritten.
pub fn read_settings(path: &Path) -> Result<UserSettings> {
    if !path.exists() {
        return Ok(UserSettings::default());
    }
    let settings: UserSettings = serde_json::from_slice(&read_bytes(path)?)?;
    if settings.format != SETTINGS_FORMAT {
        return Err(Error::Document(pcbcraft_core::Error::Unsupported(format!(
            "unsupported settings format {}",
            settings.format
        ))));
    }
    Ok(settings)
}

/// Bound and atomically replace the settings file.
pub fn write_settings(path: &Path, settings: &UserSettings) -> Result<()> {
    if settings.sources.len() > MAX_SOURCES
        || settings.favourites.len() > MAX_FAVOURITES
        || settings.recent.len() > MAX_RECENT
    {
        return Err(Error::TooLarge(
            "too many sources, favourites or recents".into(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(settings)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::ids::{DefinitionId, LibraryId, SourceLineageId};

    #[test]
    fn settings_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cfg").join(SETTINGS_FILE);
        assert_eq!(read_settings(&path)?, UserSettings::default());
        let settings = UserSettings {
            sources: vec![SourceSetting {
                path: "/libs/team".into(),
                enabled: true,
                writable: false,
            }],
            favourites: vec![EntryKey::Library {
                library: LibraryId(1),
                lineage: SourceLineageId(2),
            }],
            recent: vec![EntryKey::Project {
                lineage: DefinitionId(3),
            }],
            ..UserSettings::default()
        };
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        Ok(())
    }
}
