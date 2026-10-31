//! Per-user settings (S3): library sources, favourites and recents. Stored
//! as `settings.json` in the user config directory, never in a project.
//! Favourites and recents are keyed by `EntryKey::Library` (library id and
//! source lineage), never by path or name. A project lineage id names a
//! different item in every project, so per-user settings never hold
//! `EntryKey::Project`: a project copy of a library item is remembered by
//! its origin, and a project-only item only for the open project.
use crate::library_folder::{pretty, read_bounded};
use crate::persistence::atomic_write;
use crate::{Error, Result};
use pcbcraft_core::catalog::EntryKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
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
/// Bytes of the settings file.
pub const MAX_SETTINGS_BYTES: usize = 1024 * 1024;
/// Bytes of one configured path.
pub const MAX_PATH: usize = 4096;
/// Overrides the config directory (tests, sandboxes).
pub const CONFIG_VAR: &str = "PCBCRAFT_CONFIG_DIR";
/// Overrides the cache directory (tests, sandboxes).
pub const CACHE_VAR: &str = "PCBCRAFT_CACHE_DIR";
const APP_DIR: &str = "pcbcraft";

/// One configured library folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceSetting {
    pub path: PathBuf,
    /// Turned off sources stay listed with the reason "turned off".
    pub enabled: bool,
    /// Whether `library.promote` may write to it.
    pub writable: bool,
    /// The manifest's name when the folder was last read, so a source
    /// whose folder went missing is still listed by its library name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// `PCBCRAFT_CONFIG_DIR` / `PCBCRAFT_CACHE_DIR` when set, else the
    /// platform directories under `pcbcraft`.
    pub fn from_env() -> Result<Self> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// [`UserDirs::from_env`] over any variable lookup (tests). Every
    /// directory must be absolute.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = set("HOME");
        let platform = |kind: &str| -> Option<PathBuf> {
            if cfg!(target_os = "macos") {
                let base = home.as_ref()?.join("Library");
                Some(match kind {
                    "config" => base.join("Application Support").join(APP_DIR),
                    _ => base.join("Caches").join(APP_DIR),
                })
            } else if cfg!(windows) {
                Some(match kind {
                    "config" => set("APPDATA")?.join(APP_DIR),
                    _ => set("LOCALAPPDATA")?.join(APP_DIR).join("cache"),
                })
            } else {
                Some(match kind {
                    "config" => set("XDG_CONFIG_HOME")
                        .filter(|p| p.is_absolute())
                        .or_else(|| home.as_ref().map(|h| h.join(".config")))?
                        .join(APP_DIR),
                    _ => set("XDG_CACHE_HOME")
                        .filter(|p| p.is_absolute())
                        .or_else(|| home.as_ref().map(|h| h.join(".cache")))?
                        .join(APP_DIR),
                })
            }
        };
        let pick = |variable: &str, kind: &str| -> Result<PathBuf> {
            let path = set(variable).or_else(|| platform(kind)).ok_or_else(|| {
                Error::Invalid(format!(
                    "no {kind} directory: set {variable} or the platform's home directory"
                ))
            })?;
            if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
                return Err(Error::Invalid(format!(
                    "the {kind} directory {} must be an absolute path of at most {MAX_PATH} bytes",
                    path.display()
                )));
            }
            Ok(path)
        };
        Ok(Self {
            config: pick(CONFIG_VAR, "config")?,
            cache: pick(CACHE_VAR, "cache")?,
        })
    }

    pub fn settings_path(&self) -> PathBuf {
        self.config.join(SETTINGS_FILE)
    }
}

impl UserSettings {
    /// Put `key` first among the recents, dropping an older mention and
    /// the oldest beyond [`MAX_RECENT`].
    pub fn remember_recent(&mut self, key: EntryKey) {
        self.recent.retain(|k| *k != key);
        self.recent.insert(0, key);
        self.recent.truncate(MAX_RECENT);
    }

    /// Add or remove a favourite; returns whether it is now a favourite.
    /// Adding beyond [`MAX_FAVOURITES`] is refused.
    pub fn toggle_favourite(&mut self, key: EntryKey) -> Result<bool> {
        if let Some(at) = self.favourites.iter().position(|k| *k == key) {
            self.favourites.remove(at);
            return Ok(false);
        }
        if self.favourites.len() >= MAX_FAVOURITES {
            return Err(Error::TooLarge(format!(
                "at most {MAX_FAVOURITES} favourites"
            )));
        }
        self.favourites.push(key);
        Ok(true)
    }

    /// Bounds, path shapes and duplicates.
    pub fn check(&self) -> Result<()> {
        if self.format != SETTINGS_FORMAT {
            return Err(Error::Document(pcbcraft_core::Error::Unsupported(format!(
                "unsupported settings format {}",
                self.format
            ))));
        }
        if self.sources.len() > MAX_SOURCES
            || self.favourites.len() > MAX_FAVOURITES
            || self.recent.len() > MAX_RECENT
        {
            return Err(Error::TooLarge(
                "too many sources, favourites or recents".into(),
            ));
        }
        let mut paths = BTreeSet::new();
        for source in &self.sources {
            if !source.path.is_absolute() || source.path.as_os_str().len() > MAX_PATH {
                return Err(Error::Invalid(format!(
                    "library source {} must be an absolute path of at most {MAX_PATH} bytes",
                    source.path.display()
                )));
            }
            if source
                .name
                .as_deref()
                .is_some_and(|n| !pcbcraft_core::ids::printable(n))
            {
                return Err(Error::Invalid(format!(
                    "library source {} has a blank, overlong or non-printable name",
                    source.path.display()
                )));
            }
            if !paths.insert(&source.path) {
                return Err(Error::Invalid(format!(
                    "library source {} is listed twice",
                    source.path.display()
                )));
            }
        }
        if self
            .favourites
            .iter()
            .chain(&self.recent)
            .any(|k| matches!(k, EntryKey::Project { .. }))
        {
            return Err(Error::Invalid(
                "favourites and recents name library items; a project lineage id means something else in every project".into(),
            ));
        }
        let unique = |keys: &[EntryKey]| keys.iter().collect::<BTreeSet<_>>().len() == keys.len();
        if !unique(&self.favourites) || !unique(&self.recent) {
            return Err(Error::Invalid(
                "a favourite or recent entry is listed twice".into(),
            ));
        }
        Ok(())
    }
}

/// Read settings; a missing file is the defaults. An unsupported format is
/// refused, never rewritten; out-of-bounds content is refused.
pub fn read_settings(path: &Path) -> Result<UserSettings> {
    if !path.exists() {
        return Ok(UserSettings::default());
    }
    let settings: UserSettings = serde_json::from_slice(&read_bounded(path, MAX_SETTINGS_BYTES)?)?;
    settings.check()?;
    Ok(settings)
}

/// Check and atomically replace the settings file.
pub fn write_settings(path: &Path, settings: &UserSettings) -> Result<()> {
    settings.check()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(path, &pretty(settings)?)
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
                name: Some("Team".into()),
            }],
            favourites: vec![EntryKey::Library {
                library: LibraryId(1),
                lineage: SourceLineageId(2),
            }],
            recent: vec![EntryKey::Library {
                library: LibraryId(1),
                lineage: SourceLineageId(3),
            }],
            ..UserSettings::default()
        };
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        // Project lineage ids are per project: never kept per user.
        let mut project = settings.clone();
        project.recent.push(EntryKey::Project {
            lineage: DefinitionId(3),
        });
        assert!(write_settings(&path, &project).is_err());
        assert_eq!(
            read_settings(&path)?,
            settings,
            "the refused write changed nothing"
        );
        Ok(())
    }

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn user_dirs_honour_overrides_and_refuse_relative_paths() -> Result<()> {
        let dirs = UserDirs::from_vars(vars(&[
            (CONFIG_VAR, "/sandbox/config"),
            (CACHE_VAR, "/sandbox/cache"),
            ("HOME", "/home/someone"),
        ]))?;
        assert_eq!(dirs.config, PathBuf::from("/sandbox/config"));
        assert_eq!(dirs.cache, PathBuf::from("/sandbox/cache"));
        assert_eq!(
            dirs.settings_path(),
            PathBuf::from("/sandbox/config/settings.json")
        );
        assert!(UserDirs::from_vars(vars(&[(CONFIG_VAR, "relative"), (CACHE_VAR, "/c")])).is_err());

        let platform = UserDirs::from_vars(vars(&[
            ("HOME", "/home/someone"),
            ("APPDATA", "/appdata"),
            ("LOCALAPPDATA", "/local"),
        ]))?;
        if cfg!(target_os = "macos") {
            assert_eq!(
                platform.config,
                PathBuf::from("/home/someone/Library/Application Support/pcbcraft")
            );
            assert_eq!(
                platform.cache,
                PathBuf::from("/home/someone/Library/Caches/pcbcraft")
            );
        } else if cfg!(unix) {
            assert_eq!(
                platform.config,
                PathBuf::from("/home/someone/.config/pcbcraft")
            );
            assert_eq!(
                platform.cache,
                PathBuf::from("/home/someone/.cache/pcbcraft")
            );
        }
        if !cfg!(windows) {
            assert!(
                UserDirs::from_vars(vars(&[])).is_err(),
                "no home, no override"
            );
        }
        Ok(())
    }

    #[test]
    fn recents_and_favourites_are_bounded_sets() -> Result<()> {
        let key = |n: u128| EntryKey::Library {
            library: LibraryId(1),
            lineage: SourceLineageId(n),
        };
        let mut settings = UserSettings::default();
        for n in 0..40 {
            settings.remember_recent(key(n));
        }
        settings.remember_recent(key(20));
        assert_eq!(settings.recent.len(), MAX_RECENT);
        assert_eq!(settings.recent.first(), Some(&key(20)));
        assert_eq!(settings.recent.get(1), Some(&key(39)));
        assert_eq!(settings.recent.iter().filter(|k| **k == key(20)).count(), 1);

        assert!(settings.toggle_favourite(key(5))?);
        assert!(!settings.toggle_favourite(key(5))?);
        assert!(settings.favourites.is_empty());
        for n in 0..MAX_FAVOURITES as u128 {
            settings.toggle_favourite(key(n))?;
        }
        assert!(settings.toggle_favourite(key(9999)).is_err());
        settings.check()
    }

    #[test]
    fn bad_settings_are_refused_never_rewritten() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        std::fs::write(
            &path,
            "{\"format\":2,\"sources\":[],\"favourites\":[],\"recent\":[]}",
        )?;
        assert_eq!(
            read_settings(&path).map(|_| ()).map_err(|e| e.code()),
            Err("unsupported")
        );
        std::fs::write(&path, "{\"format\":1")?;
        assert!(read_settings(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "{\"format\":1");

        let source = |p: &str| SourceSetting {
            path: p.into(),
            enabled: true,
            writable: true,
            name: None,
        };
        let mut settings = UserSettings {
            sources: vec![source("/libs/a"), source("/libs/a")],
            ..UserSettings::default()
        };
        assert!(
            write_settings(&path, &settings).is_err(),
            "duplicate source"
        );
        settings.sources = vec![source("libs/relative")];
        assert!(write_settings(&path, &settings).is_err(), "relative source");
        settings.sources = vec![source("/libs/a")];
        settings.recent = vec![
            EntryKey::Library {
                library: LibraryId(1),
                lineage: SourceLineageId(1),
            };
            2
        ];
        assert!(
            write_settings(&path, &settings).is_err(),
            "duplicate recent"
        );
        settings.recent.truncate(1);
        write_settings(&path, &settings)?;
        let text = std::fs::read_to_string(&path)?;
        assert!(text.ends_with('\n'));
        assert_eq!(read_settings(&path)?, settings);
        Ok(())
    }
}
