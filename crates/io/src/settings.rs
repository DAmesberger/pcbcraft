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
/// Recent part files kept (the part-file chooser), most recent first.
pub const MAX_RECENT_PART_FILES: usize = 8;
/// Projects whose production folder is kept, most recent first.
pub const MAX_PRODUCTION_FOLDERS: usize = 32;
/// Bytes of the settings file.
pub const MAX_SETTINGS_BYTES: usize = 1024 * 1024;
/// The widest panel width kept, in points.
pub const MAX_PANEL_WIDTH: f32 = 4096.0;
/// Bytes of one configured path.
pub const MAX_PATH: usize = 4096;
/// Overrides the config directory (tests, sandboxes).
pub const CONFIG_VAR: &str = "PCBCRAFT_CONFIG_DIR";
/// Overrides the cache directory (tests, sandboxes).
pub const CACHE_VAR: &str = "PCBCRAFT_CACHE_DIR";
/// Overrides the recovery directory (tests, sandboxes).
pub const RECOVERY_VAR: &str = "PCBCRAFT_RECOVERY_DIR";
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
    /// The Browser's width in points, as last dragged; `None` for the
    /// default. The app clamps it to what the window fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_width: Option<f32>,
    /// The user's keymap and the key-reveal overlays.
    #[serde(default, skip_serializing_if = "KeyboardSettings::is_default")]
    pub keyboard: KeyboardSettings,
    /// Part files opened in the part-file chooser, most recent first:
    /// absolute paths, at most [`MAX_RECENT_PART_FILES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_files: Vec<PathBuf>,
    /// The app's waivers of pre-flight findings need a reason (off by
    /// default: a reason is optional).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_waiver_reason: bool,
    /// The folder each project's production files were last written to,
    /// by project file, most recent first: a machine's own paths, so kept
    /// per user and never in the (portable) project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub production_folders: Vec<ProductionFolder>,
}

/// Where one project's production files go.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProductionFolder {
    /// The project file, absolute.
    pub project: PathBuf,
    /// The folder that holds its output folder, absolute.
    pub folder: PathBuf,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            format: SETTINGS_FORMAT,
            sources: Vec::new(),
            favourites: Vec::new(),
            recent: Vec::new(),
            browser_width: None,
            keyboard: KeyboardSettings::default(),
            part_files: Vec::new(),
            require_waiver_reason: false,
            production_folders: Vec::new(),
        }
    }
}

/// The keymap's own format, inside the settings and in an exported keymap
/// file; any other version is refused.
pub const KEYMAP_FORMAT: u32 = 1;
/// Changed keys kept.
pub const MAX_BINDINGS: usize = 1024;
/// Bytes of a command id or a chord name.
pub const MAX_BINDING_TEXT: usize = 128;
/// The hold-⌘ reveal's delay range, in milliseconds.
pub const HOLD_MS_RANGE: std::ops::RangeInclusive<u32> = 150..=3000;
/// The hold-⌘ reveal's default delay.
pub const HOLD_MS_DEFAULT: u32 = 500;

/// One changed key: the command's registry id and its chord as
/// Solidcraft's keymap names it (`Ctrl+Shift+S`), or `None` for no key.
/// Whether the id and chord mean something is the app's to say: an entry
/// it cannot apply is reported and kept, never dropped.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyBinding {
    pub command: String,
    pub chord: Option<String>,
}

/// Hold ⌘ to show key badges; tap ⌥ for key tips.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevealSettings {
    pub hold_badges: bool,
    pub hold_ms: u32,
    pub key_tips: bool,
}

impl Default for RevealSettings {
    fn default() -> Self {
        Self {
            hold_badges: true,
            hold_ms: HOLD_MS_DEFAULT,
            key_tips: true,
        }
    }
}

/// The keymap the user changed and the reveal overlays' settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyboardSettings {
    pub format: u32,
    /// Only the changed commands, in id order.
    #[serde(default)]
    pub bindings: Vec<KeyBinding>,
    #[serde(default)]
    pub reveal: RevealSettings,
}

impl Default for KeyboardSettings {
    fn default() -> Self {
        Self {
            format: KEYMAP_FORMAT,
            bindings: Vec::new(),
            reveal: RevealSettings::default(),
        }
    }
}

impl KeyboardSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The format, bounds and shape of every entry: printable text of at
    /// most [`MAX_BINDING_TEXT`] bytes, each command once.
    pub fn check(&self) -> Result<()> {
        if self.format != KEYMAP_FORMAT {
            return Err(Error::Document(pcbcraft_core::Error::Unsupported(format!(
                "unsupported keymap format {}",
                self.format
            ))));
        }
        if self.bindings.len() > MAX_BINDINGS {
            return Err(Error::TooLarge(format!(
                "at most {MAX_BINDINGS} changed keys"
            )));
        }
        if !HOLD_MS_RANGE.contains(&self.reveal.hold_ms) {
            return Err(Error::Invalid(format!(
                "the key reveal delay must be {} to {} ms",
                HOLD_MS_RANGE.start(),
                HOLD_MS_RANGE.end()
            )));
        }
        let text = |t: &str| {
            !t.is_empty() && t.len() <= MAX_BINDING_TEXT && !t.chars().any(char::is_control)
        };
        let mut seen = BTreeSet::new();
        for binding in &self.bindings {
            if !text(&binding.command) || binding.chord.as_deref().is_some_and(|c| !text(c)) {
                return Err(Error::Invalid(format!(
                    "a changed key has a blank, overlong or non-printable command or key: {}",
                    binding.command.chars().take(40).collect::<String>()
                )));
            }
            if !seen.insert(binding.command.as_str()) {
                return Err(Error::Invalid(format!(
                    "the key of {} is set twice",
                    binding.command
                )));
            }
        }
        Ok(())
    }
}

/// An exported keymap file: the changed keys alone.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeymapFile {
    pub format: u32,
    pub bindings: Vec<KeyBinding>,
}

/// Read an exported keymap; its format and bounds are checked as in the
/// settings.
pub fn read_keymap(path: &Path) -> Result<Vec<KeyBinding>> {
    let file: KeymapFile = serde_json::from_slice(&read_bounded(path, MAX_SETTINGS_BYTES)?)?;
    KeyboardSettings {
        format: file.format,
        bindings: file.bindings.clone(),
        reveal: RevealSettings::default(),
    }
    .check()?;
    Ok(file.bindings)
}

/// Check and atomically write an exported keymap.
pub fn write_keymap(path: &Path, bindings: &[KeyBinding]) -> Result<()> {
    let file = KeymapFile {
        format: KEYMAP_FORMAT,
        bindings: bindings.to_vec(),
    };
    KeyboardSettings {
        format: file.format,
        bindings: file.bindings.clone(),
        reveal: RevealSettings::default(),
    }
    .check()?;
    atomic_write(path, &pretty(&file)?)
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

    /// Where crash-recovery snapshots live: `PCBCRAFT_RECOVERY_DIR` when
    /// set, else the platform's state directory (`~/Library/Application
    /// Support/Pcbcraft/Recovery`, `%LOCALAPPDATA%\Pcbcraft\Recovery`,
    /// `$XDG_STATE_HOME/pcbcraft/recovery` or `~/.local/state/pcbcraft/recovery`).
    /// Never a temporary or the current directory: without an absolute home
    /// this is an error, and the caller says recovery is unavailable.
    pub fn recovery_from_env() -> Result<PathBuf> {
        Self::recovery_from_vars(|name| std::env::var_os(name))
    }

    /// [`UserDirs::recovery_from_env`] over any variable lookup (tests).
    pub fn recovery_from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let absolute = |p: PathBuf| Some(p).filter(|p| p.is_absolute());
        let home = || absolute(set("HOME")?);
        let path = set(RECOVERY_VAR).or_else(|| {
            if cfg!(target_os = "macos") {
                Some(home()?.join("Library/Application Support/Pcbcraft/Recovery"))
            } else if cfg!(windows) {
                Some(
                    absolute(set("LOCALAPPDATA")?)?
                        .join("Pcbcraft")
                        .join("Recovery"),
                )
            } else {
                let state = set("XDG_STATE_HOME")
                    .and_then(absolute)
                    .or_else(|| Some(home()?.join(".local/state")))?;
                Some(state.join("pcbcraft/recovery"))
            }
        });
        let path = path.ok_or_else(|| {
            Error::Invalid(format!(
                "no recovery directory: set {RECOVERY_VAR} or the platform's home directory"
            ))
        })?;
        if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
            return Err(Error::Invalid(format!(
                "the recovery directory {} must be an absolute path of at most {MAX_PATH} bytes",
                path.display()
            )));
        }
        Ok(path)
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

    /// Put the part file `path` first among the recent part files, dropping
    /// an older mention and the oldest beyond [`MAX_RECENT_PART_FILES`]. A
    /// path the settings would refuse (relative or overlong) is not kept.
    pub fn remember_part_file(&mut self, path: PathBuf) {
        if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
            return;
        }
        self.part_files.retain(|p| *p != path);
        self.part_files.insert(0, path);
        self.part_files.truncate(MAX_RECENT_PART_FILES);
    }

    /// The production folder last chosen for the project file `project`.
    pub fn production_folder(&self, project: &Path) -> Option<&Path> {
        self.production_folders
            .iter()
            .find(|p| p.project == project)
            .map(|p| p.folder.as_path())
    }

    /// Keep `folder` as the production folder of the project file
    /// `project`, first, dropping the oldest beyond
    /// [`MAX_PRODUCTION_FOLDERS`]. Paths the settings would refuse
    /// (relative or overlong) are not kept.
    pub fn remember_production_folder(&mut self, project: PathBuf, folder: PathBuf) {
        let kept = |p: &Path| p.is_absolute() && p.as_os_str().len() <= MAX_PATH;
        if !kept(&project) || !kept(&folder) {
            return;
        }
        self.production_folders.retain(|p| p.project != project);
        self.production_folders
            .insert(0, ProductionFolder { project, folder });
        self.production_folders.truncate(MAX_PRODUCTION_FOLDERS);
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
        if self
            .browser_width
            .is_some_and(|w| !w.is_finite() || w <= 0.0 || w > MAX_PANEL_WIDTH)
        {
            return Err(Error::Invalid(format!(
                "the Browser width must be a positive number of at most {MAX_PANEL_WIDTH} points"
            )));
        }
        self.keyboard.check()?;
        if self.part_files.len() > MAX_RECENT_PART_FILES {
            return Err(Error::TooLarge(format!(
                "at most {MAX_RECENT_PART_FILES} recent part files"
            )));
        }
        for path in &self.part_files {
            if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
                return Err(Error::Invalid(format!(
                    "recent part file {} must be an absolute path of at most {MAX_PATH} bytes",
                    path.display()
                )));
            }
        }
        if self.part_files.iter().collect::<BTreeSet<_>>().len() != self.part_files.len() {
            return Err(Error::Invalid("a recent part file is listed twice".into()));
        }
        if self.production_folders.len() > MAX_PRODUCTION_FOLDERS {
            return Err(Error::TooLarge(format!(
                "at most {MAX_PRODUCTION_FOLDERS} production folders"
            )));
        }
        for p in &self.production_folders {
            for path in [&p.project, &p.folder] {
                if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
                    return Err(Error::Invalid(format!(
                        "production folder path {} must be absolute and at most {MAX_PATH} bytes",
                        path.display()
                    )));
                }
            }
        }
        let projects: BTreeSet<_> = self.production_folders.iter().map(|p| &p.project).collect();
        if projects.len() != self.production_folders.len() {
            return Err(Error::Invalid(
                "a project's production folder is listed twice".into(),
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

/// Read settings; only a file that is not there is the defaults. A file
/// that cannot be checked or read (a permission error, a bad byte, an
/// unsupported format, out-of-bounds content) is refused, so no caller
/// mistakes it for "no settings" and saves defaults over it.
pub fn read_settings(path: &Path) -> Result<UserSettings> {
    if !path.try_exists()? {
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

    /// `path` made absolute on this platform: Windows needs a drive, so
    /// `/work` becomes `C:/work` there.
    fn abs(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_owned()
        }
    }

    #[test]
    fn settings_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cfg").join(SETTINGS_FILE);
        assert_eq!(read_settings(&path)?, UserSettings::default());
        let settings = UserSettings {
            sources: vec![SourceSetting {
                path: abs("/libs/team").into(),
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
            browser_width: Some(312.5),
            part_files: vec![abs("/parts/nut.solidcraft").into()],
            require_waiver_reason: true,
            ..UserSettings::default()
        };
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        // Recent part files: absolute, unique, at most eight.
        let mut parts = settings.clone();
        for i in 0..12 {
            parts.remember_part_file(abs(&format!("/parts/p{i}.solidcraft")).into());
        }
        parts.remember_part_file("relative.solidcraft".into());
        parts.remember_part_file(abs("/parts/p5.solidcraft").into());
        assert_eq!(parts.part_files.len(), MAX_RECENT_PART_FILES);
        assert_eq!(
            parts.part_files[0],
            PathBuf::from(abs("/parts/p5.solidcraft"))
        );
        parts.check()?;
        let mut twice = parts.clone();
        twice.part_files[1] = twice.part_files[0].clone();
        assert!(twice.check().is_err(), "duplicate part file");
        // A width that is not a positive, bounded number is refused.
        for width in [f32::NAN, f32::INFINITY, 0.0, -1.0, MAX_PANEL_WIDTH + 1.0] {
            let mut wide = settings.clone();
            wide.browser_width = Some(width);
            assert!(write_settings(&path, &wide).is_err(), "{width}");
        }
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

    /// The keymap round-trips with entries the app cannot apply (they are
    /// its to report); a bad shape, a bad format or a duplicate is refused
    /// and the file is left as it was. An exported keymap reads back.
    #[test]
    fn keyboard_settings_round_trip_and_refuse_bad_shapes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        let binding = |command: &str, chord: Option<&str>| KeyBinding {
            command: command.into(),
            chord: chord.map(str::to_owned),
        };
        let settings = UserSettings {
            keyboard: KeyboardSettings {
                bindings: vec![
                    binding("pcb.route", Some("R")),
                    binding("route.width-next", None),
                    binding("no.such.command", Some("Ctrl+Nope")),
                ],
                reveal: RevealSettings {
                    hold_badges: false,
                    hold_ms: 800,
                    key_tips: true,
                },
                ..KeyboardSettings::default()
            },
            ..UserSettings::default()
        };
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        for bad in [
            binding("pcb.route", Some("")),
            binding("", Some("R")),
            binding("pcb.route", Some("R\u{7}")),
            binding(&"x".repeat(MAX_BINDING_TEXT + 1), None),
        ] {
            let mut wrong = settings.clone();
            wrong.keyboard.bindings.push(bad);
            assert!(write_settings(&path, &wrong).is_err());
        }
        let mut twice = settings.clone();
        twice
            .keyboard
            .bindings
            .push(binding("pcb.route", Some("X")));
        assert!(write_settings(&path, &twice).is_err(), "set twice");
        let mut slow = settings.clone();
        slow.keyboard.reveal.hold_ms = 10;
        assert!(write_settings(&path, &slow).is_err(), "delay out of range");
        let mut future = settings.clone();
        future.keyboard.format = 2;
        assert!(write_settings(&path, &future).is_err(), "keymap format");
        assert_eq!(read_settings(&path)?, settings, "refusals changed nothing");
        // Defaults are not written at all.
        write_settings(&path, &UserSettings::default())?;
        assert!(!std::fs::read_to_string(&path)?.contains("keyboard"));
        // Export and import.
        let file = dir.path().join("keys.json");
        write_keymap(&file, &settings.keyboard.bindings)?;
        assert_eq!(read_keymap(&file)?, settings.keyboard.bindings);
        std::fs::write(&file, "{\"format\":9,\"bindings\":[]}")?;
        assert!(read_keymap(&file).is_err());
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
        let (config, cache, c) = (abs("/sandbox/config"), abs("/sandbox/cache"), abs("/c"));
        let (appdata, local) = (abs("/appdata"), abs("/local"));
        let dirs = UserDirs::from_vars(vars(&[
            (CONFIG_VAR, &config),
            (CACHE_VAR, &cache),
            ("HOME", "/home/someone"),
        ]))?;
        assert_eq!(dirs.config, PathBuf::from(&config));
        assert_eq!(dirs.cache, PathBuf::from(&cache));
        assert_eq!(
            dirs.settings_path(),
            PathBuf::from(&config).join("settings.json")
        );
        assert!(UserDirs::from_vars(vars(&[(CONFIG_VAR, "relative"), (CACHE_VAR, &c)])).is_err());

        let platform = UserDirs::from_vars(vars(&[
            ("HOME", "/home/someone"),
            ("APPDATA", &appdata),
            ("LOCALAPPDATA", &local),
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
        } else if cfg!(windows) {
            assert_eq!(platform.config, PathBuf::from(&appdata).join("pcbcraft"));
            assert_eq!(
                platform.cache,
                PathBuf::from(&local).join("pcbcraft").join("cache")
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

    /// Run 3, finding 16: a project's production folder is kept per user
    /// by project file, survives a write and read, and stays bounded.
    #[test]
    fn production_folders_are_kept_per_project_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        let board = PathBuf::from(abs("/work/board.pcbcraft"));
        let mut settings = UserSettings::default();
        settings.remember_production_folder(board.clone(), abs("/work/out").into());
        settings.remember_production_folder(board.clone(), abs("/work/fab").into());
        settings.remember_production_folder("relative.pcbcraft".into(), abs("/x").into());
        settings.remember_production_folder(abs("/work/other.pcbcraft").into(), "x".into());
        assert_eq!(settings.production_folders.len(), 1, "one per project");
        write_settings(&path, &settings)?;
        let read = read_settings(&path)?;
        assert_eq!(
            read.production_folder(&board),
            Some(Path::new(&abs("/work/fab")))
        );
        assert_eq!(
            read.production_folder(Path::new(&abs("/work/x.pcbcraft"))),
            None
        );
        for n in 0..40 {
            settings.remember_production_folder(
                PathBuf::from(abs(&format!("/p/{n}.pcbcraft"))),
                PathBuf::from(abs("/out")),
            );
        }
        assert_eq!(settings.production_folders.len(), MAX_PRODUCTION_FOLDERS);
        assert_eq!(settings.production_folder(&board), None, "oldest dropped");
        settings.check()?;
        settings.production_folders.pop();
        settings.production_folders.push(ProductionFolder {
            project: "relative".into(),
            folder: abs("/out").into(),
        });
        assert!(settings.check().is_err());
        Ok(())
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
            sources: vec![source(&abs("/libs/a")), source(&abs("/libs/a"))],
            ..UserSettings::default()
        };
        assert!(
            write_settings(&path, &settings).is_err(),
            "duplicate source"
        );
        settings.sources = vec![source("libs/relative")];
        assert!(write_settings(&path, &settings).is_err(), "relative source");
        settings.sources = vec![source(&abs("/libs/a"))];
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

    /// A settings file that exists but cannot be checked (its folder
    /// refuses access) is an error, never "no settings": defaults read
    /// from it would be saved over the user's keymap and sources.
    #[cfg(unix)]
    #[test]
    fn an_inaccessible_settings_file_is_not_read_as_defaults() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let config = dir.path().join("config");
        std::fs::create_dir(&config)?;
        let path = config.join(SETTINGS_FILE);
        let settings = UserSettings {
            browser_width: Some(300.0),
            ..UserSettings::default()
        };
        write_settings(&path, &settings)?;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000))?;
        // A privileged user is not refused; there is nothing to test then.
        let refused = std::fs::metadata(&path).is_err();
        let read = read_settings(&path);
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700))?;
        if refused {
            assert!(read.is_err(), "read as {read:?}");
        }
        assert_eq!(read_settings(&path)?, settings);
        Ok(())
    }

    /// The recovery folder is explicit: the override or the platform's
    /// state folder under an absolute home, never a temporary or relative
    /// folder; without one it is an error the app reports.
    #[test]
    fn the_recovery_directory_is_explicit_or_an_error() -> Result<()> {
        let (r, local) = (abs("/r"), abs("/local"));
        assert_eq!(
            UserDirs::recovery_from_vars(vars(&[(RECOVERY_VAR, &r), ("HOME", "/h")]))?,
            PathBuf::from(&r)
        );
        assert!(UserDirs::recovery_from_vars(vars(&[(RECOVERY_VAR, "rel")])).is_err());
        assert!(UserDirs::recovery_from_vars(vars(&[])).is_err());
        assert!(UserDirs::recovery_from_vars(vars(&[("HOME", "relative")])).is_err());
        if cfg!(target_os = "macos") {
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("HOME", "/h")]))?,
                PathBuf::from("/h/Library/Application Support/Pcbcraft/Recovery")
            );
        } else if cfg!(windows) {
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("LOCALAPPDATA", &local)]))?,
                PathBuf::from(&local).join("Pcbcraft").join("Recovery")
            );
            assert!(UserDirs::recovery_from_vars(vars(&[("LOCALAPPDATA", "rel")])).is_err());
        } else {
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("HOME", "/h")]))?,
                PathBuf::from("/h/.local/state/pcbcraft/recovery")
            );
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("HOME", "/h"), ("XDG_STATE_HOME", "s")]))?,
                PathBuf::from("/h/.local/state/pcbcraft/recovery"),
                "a relative XDG_STATE_HOME is ignored"
            );
        }
        Ok(())
    }
}
