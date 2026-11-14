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
use serde::{Deserialize, Serialize};
use sparksmith_core::catalog::EntryKey;
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
/// Recent project files kept (File › Open Recent), most recent first: 10,
/// the default length of macOS's own Recent Items list.
pub const MAX_RECENT_PROJECTS: usize = 10;
/// Projects whose production folder is kept, most recent first.
pub const MAX_PRODUCTION_FOLDERS: usize = 32;
/// Bytes of the settings file.
pub const MAX_SETTINGS_BYTES: usize = 1024 * 1024;
/// The widest panel width kept, in points.
pub const MAX_PANEL_WIDTH: f32 = 4096.0;
/// Bytes of one configured path.
pub const MAX_PATH: usize = 4096;
/// Overrides the config directory (tests, sandboxes).
pub const CONFIG_VAR: &str = "SPARKSMITH_CONFIG_DIR";
/// Overrides the cache directory (tests, sandboxes).
pub const CACHE_VAR: &str = "SPARKSMITH_CACHE_DIR";
/// Overrides the recovery directory (tests, sandboxes).
pub const RECOVERY_VAR: &str = "SPARKSMITH_RECOVERY_DIR";
/// The per-user folder name (config, cache, Linux state): the product's
/// short name.
const APP_DIR: &str = sparksmith_core::product::SHORT_NAME;
/// The recovery folder's parent on macOS and Windows: the product name.
const APP_NAME: &str = sparksmith_core::product::NAME;
/// The folder names before the rename (docs/rename-phase-b.md), read only by
/// [`UserDirs::adopt_former`].
const FORMER_APP_DIR: &str = "pcbcraft";
const FORMER_APP_NAME: &str = "Pcbcraft";

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
    /// Project files last opened or saved (File › Open Recent), most recent
    /// first: absolute paths, at most [`MAX_RECENT_PROJECTS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<PathBuf>,
    /// The fab profile library id a new board starts with when none is
    /// chosen (Settings); `None`: every new board asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_rule_profile: Option<String>,
    /// How far routed impedance may stray from a net class's target, in
    /// percent, when the class states no tolerance (Settings › Rules);
    /// `None`: the shipped value ([`SettingsDefaults`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance_tolerance: Option<f64>,
    /// Which kinds the Layout and 3D selection may take (the selection
    /// filter), as last set; every kind by default.
    #[serde(default, skip_serializing_if = "SelectionFilter::is_default")]
    pub selection_filter: SelectionFilter,
    /// Row density, reduced motion and saved list filters.
    #[serde(default, skip_serializing_if = "ListSettings::is_default")]
    pub lists: ListSettings,
    /// The former "Mesh the board directly" setting. The direct mesh is
    /// now the 3D view's only way to draw the board, so the setting means
    /// nothing: a settings file that turned it on still reads, and the
    /// next save leaves it out.
    #[serde(default, rename = "mesh_board_directly", skip_serializing)]
    pub former_mesh_board_directly: serde::de::IgnoredAny,
}

/// Saved list filters kept.
pub const MAX_SAVED_FILTERS: usize = 64;
/// Bytes of a saved filter's list name.
pub const MAX_FILTER_LIST: usize = 64;
/// Bytes of a saved filter's query: the catalog query's own bound.
pub const MAX_FILTER_QUERY: usize = sparksmith_core::catalog::MAX_QUERY;

/// How tall list rows are.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListDensity {
    #[default]
    Compact,
    /// Six points taller rows.
    Comfortable,
}

/// One saved filter of one list: a query the list's filter bar offers
/// as a named chip.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SavedFilter {
    /// The list's stable name (`pcb.parts`, `issues`).
    pub list: String,
    pub query: String,
}

/// The lists' settings (Settings › Lists).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ListSettings {
    #[serde(default)]
    pub density: ListDensity,
    /// `None` follows the system's Reduce motion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduced_motion: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_filters: Vec<SavedFilter>,
}

impl ListSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Bounds and duplicates.
    pub fn check(&self) -> Result<()> {
        if self.saved_filters.len() > MAX_SAVED_FILTERS {
            return Err(Error::Invalid(format!(
                "at most {MAX_SAVED_FILTERS} saved filters"
            )));
        }
        let mut seen = BTreeSet::new();
        for f in &self.saved_filters {
            let fits = |t: &str, max: usize| {
                !t.trim().is_empty() && t.len() <= max && !t.chars().any(char::is_control)
            };
            if !fits(&f.list, MAX_FILTER_LIST) || !fits(&f.query, MAX_FILTER_QUERY) {
                return Err(Error::Invalid(format!(
                    "a saved filter needs a list name of at most {MAX_FILTER_LIST} bytes and a \
                     query of at most {MAX_FILTER_QUERY}, printable"
                )));
            }
            if !seen.insert((&f.list, &f.query)) {
                return Err(Error::Invalid(format!(
                    "the saved filter ‘{}’ of {} is listed twice",
                    f.query, f.list
                )));
            }
        }
        Ok(())
    }

    /// Save `query` for `list`, at the end; refused past the bound. Saving
    /// one already saved changes nothing.
    pub fn save_filter(&mut self, list: &str, query: &str) -> Result<()> {
        let query = query.trim();
        if self
            .saved_filters
            .iter()
            .any(|f| f.list == list && f.query == query)
        {
            return Ok(());
        }
        let mut next = self.clone();
        next.saved_filters.push(SavedFilter {
            list: list.to_owned(),
            query: query.to_owned(),
        });
        next.check()?;
        *self = next;
        Ok(())
    }

    pub fn remove_filter(&mut self, list: &str, query: &str) {
        self.saved_filters
            .retain(|f| !(f.list == list && f.query == query));
    }
}

/// The board selection filter: whether a click, the rectangle and ⌘A may
/// take parts (with their pads), tracks, vias and fills. A file naming
/// any other kind is refused.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectionFilter {
    pub parts: bool,
    pub tracks: bool,
    pub vias: bool,
    pub fills: bool,
}

impl Default for SelectionFilter {
    fn default() -> Self {
        Self {
            parts: true,
            tracks: true,
            vias: true,
            fills: true,
        }
    }
}

impl SelectionFilter {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
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
            projects: Vec::new(),
            default_rule_profile: None,
            impedance_tolerance: None,
            selection_filter: SelectionFilter::default(),
            lists: ListSettings::default(),
            former_mesh_board_directly: serde::de::IgnoredAny,
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
            return Err(Error::Document(sparksmith_core::Error::Unsupported(
                format!("unsupported keymap format {}", self.format),
            )));
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

/// Where per-user files live: `SPARKSMITH_CONFIG_DIR` / `SPARKSMITH_CACHE_DIR`
/// when set (tests, sandboxes), else the platform's config and cache
/// directories under the product's short name (`sparksmith`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserDirs {
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl UserDirs {
    /// `SPARKSMITH_CONFIG_DIR` / `SPARKSMITH_CACHE_DIR` when set, else the
    /// platform directories under the product's short name.
    pub fn from_env() -> Result<Self> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// [`UserDirs::from_env`] over any variable lookup (tests). Every
    /// directory must be absolute.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let pick = |variable: &str, kind: &str| -> Result<PathBuf> {
            let path = set(variable)
                .or_else(|| platform_dir(&var, kind, APP_DIR))
                .ok_or_else(|| {
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

    /// Where crash-recovery snapshots live: `SPARKSMITH_RECOVERY_DIR` when
    /// set, else the platform's state directory (`~/Library/Application
    /// Support/Sparksmith/Recovery`, `%LOCALAPPDATA%\Sparksmith\Recovery`,
    /// `$XDG_STATE_HOME/sparksmith/recovery` or `~/.local/state/sparksmith/recovery`).
    /// Never a temporary or the current directory: without an absolute home
    /// this is an error, and the caller says recovery is unavailable.
    pub fn recovery_from_env() -> Result<PathBuf> {
        Self::recovery_from_vars(|name| std::env::var_os(name))
    }

    /// [`UserDirs::recovery_from_env`] over any variable lookup (tests).
    pub fn recovery_from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let path = set(RECOVERY_VAR).or_else(|| recovery_dir(&var, APP_NAME, APP_DIR));
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

    /// Once, on the first launch after the rename from Pcbcraft: copy the
    /// config folder and the recovery folder kept under the former name to
    /// the current ones ([`UserDirs::adopt_former_from_vars`] over the
    /// process environment).
    pub fn adopt_former() -> Vec<Result<Adopted>> {
        Self::adopt_former_from_vars(|name| std::env::var_os(name))
    }

    /// [`UserDirs::adopt_former`] over any variable lookup (tests). A folder
    /// is copied only when no override variable names it, the current
    /// folder does not exist and the former one does; so it happens once,
    /// and a fresh install or an already adopted folder is left alone. The
    /// former folder is never changed or removed. The copy is made in a
    /// temporary sibling and renamed into place, so an interrupted copy
    /// leaves no partial folder and is tried again on the next launch. The
    /// cache is not copied: it is rebuilt from the library folders.
    pub fn adopt_former_from_vars(var: impl Fn(&str) -> Option<OsString>) -> Vec<Result<Adopted>> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty());
        let mut pairs = Vec::new();
        if set(CONFIG_VAR).is_none() {
            pairs.push((
                platform_dir(&var, "config", FORMER_APP_DIR),
                platform_dir(&var, "config", APP_DIR),
            ));
        }
        if set(RECOVERY_VAR).is_none() {
            pairs.push((
                recovery_dir(&var, FORMER_APP_NAME, FORMER_APP_DIR),
                recovery_dir(&var, APP_NAME, APP_DIR),
            ));
        }
        // In order: on a case-insensitive macOS volume the recovery folder
        // sits inside the config folder, so copying the config folder first
        // also brings the recovery folder, which is then left alone.
        pairs
            .into_iter()
            .filter_map(|pair| match pair {
                (Some(from), Some(to)) => crate::former_name::adopt(&from, &to).transpose(),
                _ => None,
            })
            .collect()
    }
}

/// [`UserDirs::adopt_former`] at the start of the app or the CLI, each
/// outcome a line on stderr. A failed copy leaves the former folder as it
/// is and is tried again on the next start.
pub fn adopt_former_folders() {
    let app = sparksmith_core::product::SHORT_NAME;
    for adopted in UserDirs::adopt_former() {
        match adopted {
            Ok(Adopted { from, to }) => eprintln!(
                "{app}: copied {} to {}; the former folder is kept",
                from.display(),
                to.display()
            ),
            Err(error) => {
                eprintln!("{app}: settings from the former name were not copied: {error}")
            }
        }
    }
}

/// A folder copied from its former name by [`UserDirs::adopt_former`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adopted {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// The platform's config or cache folder named `app_dir`; `None` without
/// the home or platform variables it is built from.
fn platform_dir(
    var: &impl Fn(&str) -> Option<OsString>,
    kind: &str,
    app_dir: &str,
) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = set("HOME");
    if cfg!(target_os = "macos") {
        let base = home.as_ref()?.join("Library");
        Some(match kind {
            "config" => base.join("Application Support").join(app_dir),
            _ => base.join("Caches").join(app_dir),
        })
    } else if cfg!(windows) {
        Some(match kind {
            "config" => set("APPDATA")?.join(app_dir),
            _ => set("LOCALAPPDATA")?.join(app_dir).join("cache"),
        })
    } else {
        Some(match kind {
            "config" => set("XDG_CONFIG_HOME")
                .filter(|p| p.is_absolute())
                .or_else(|| home.as_ref().map(|h| h.join(".config")))?
                .join(app_dir),
            _ => set("XDG_CACHE_HOME")
                .filter(|p| p.is_absolute())
                .or_else(|| home.as_ref().map(|h| h.join(".cache")))?
                .join(app_dir),
        })
    }
}

/// The platform's recovery folder for an app called `name` (`short` on
/// Linux); `None` without an absolute home or platform variable.
fn recovery_dir(
    var: &impl Fn(&str) -> Option<OsString>,
    name: &str,
    short: &str,
) -> Option<PathBuf> {
    let set = |v: &str| var(v).filter(|v| !v.is_empty()).map(PathBuf::from);
    let absolute = |p: PathBuf| Some(p).filter(|p| p.is_absolute());
    let home = || absolute(set("HOME")?);
    if cfg!(target_os = "macos") {
        Some(
            home()?
                .join("Library/Application Support")
                .join(name)
                .join("Recovery"),
        )
    } else if cfg!(windows) {
        Some(absolute(set("LOCALAPPDATA")?)?.join(name).join("Recovery"))
    } else {
        let state = set("XDG_STATE_HOME")
            .and_then(absolute)
            .or_else(|| Some(home()?.join(".local/state")))?;
        Some(state.join(short).join("recovery"))
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

    /// Put the project file `path` first among the recent projects,
    /// dropping an older mention and the oldest beyond
    /// [`MAX_RECENT_PROJECTS`]. A path the settings would refuse (relative or
    /// overlong) is not kept.
    pub fn remember_project(&mut self, path: PathBuf) {
        if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
            return;
        }
        self.projects.retain(|p| *p != path);
        self.projects.insert(0, path);
        self.projects.truncate(MAX_RECENT_PROJECTS);
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
            return Err(Error::Document(sparksmith_core::Error::Unsupported(
                format!("unsupported settings format {}", self.format),
            )));
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
        if self.projects.len() > MAX_RECENT_PROJECTS {
            return Err(Error::TooLarge(format!(
                "at most {MAX_RECENT_PROJECTS} recent projects"
            )));
        }
        for path in &self.projects {
            if !path.is_absolute() || path.as_os_str().len() > MAX_PATH {
                return Err(Error::Invalid(format!(
                    "recent project {} must be an absolute path of at most {MAX_PATH} bytes",
                    path.display()
                )));
            }
        }
        if self.projects.iter().collect::<BTreeSet<_>>().len() != self.projects.len() {
            return Err(Error::Invalid("a recent project is listed twice".into()));
        }
        if self
            .default_rule_profile
            .as_deref()
            .is_some_and(|id| !crate::rule_profiles::valid_id(id))
        {
            return Err(Error::Invalid(
                "the default fab profile must be a profile library id".into(),
            ));
        }
        sparksmith_core::layout::CheckSettings {
            impedance_tolerance: self.impedance_tolerance,
        }
        .check()?;
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
                .is_some_and(|n| !sparksmith_core::ids::printable(n))
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
        self.lists.check()
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

/// The shipped Settings defaults file in the `defaults/` shelf.
pub const DEFAULTS_FILE: &str = "settings.json";
/// Bytes of the shipped Settings defaults file.
const MAX_DEFAULTS_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultsFile {
    format: u32,
    /// A note for people reading the file.
    #[serde(default, rename = "about")]
    _about: String,
    impedance_tolerance_pct: f64,
    /// Where each value comes from, for people reading the file.
    #[serde(default, rename = "sources")]
    _sources: serde_json::Map<String, serde_json::Value>,
}

/// The values Settings start with until the user sets their own, read
/// from the shipped library (`defaults/settings.json`): the code assumes
/// none of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsDefaults {
    /// Impedance tolerance, percent, for a class that states none.
    pub impedance_tolerance: f64,
}

impl SettingsDefaults {
    /// Load the defaults from `shipped`; a missing, broken or
    /// out-of-bounds file is an error naming it.
    pub fn load(shipped: &crate::shipped::ShippedLibrary) -> Result<Self> {
        use crate::shipped::Shelf;
        let path = shipped.shelf(Shelf::Defaults).join(DEFAULTS_FILE);
        let invalid =
            |e: &dyn std::fmt::Display| Error::Invalid(format!("{}: {e}", path.display()));
        let bytes = shipped
            .read(Shelf::Defaults, DEFAULTS_FILE, MAX_DEFAULTS_BYTES)
            .map_err(|e| invalid(&e))?;
        let file: DefaultsFile = serde_json::from_slice(&bytes).map_err(|e| invalid(&e))?;
        if file.format != SETTINGS_FORMAT {
            return Err(Error::Document(sparksmith_core::Error::Unsupported(
                format!("{}: unsupported format {}", path.display(), file.format),
            )));
        }
        let defaults = Self {
            impedance_tolerance: file.impedance_tolerance_pct,
        };
        sparksmith_core::layout::CheckSettings {
            impedance_tolerance: Some(defaults.impedance_tolerance),
        }
        .check()
        .map_err(|e| invalid(&e))?;
        Ok(defaults)
    }

    /// [`Self::load`] from the located shipped library, once per process.
    pub fn shipped() -> Result<Self> {
        static LOADED: std::sync::OnceLock<std::result::Result<SettingsDefaults, String>> =
            std::sync::OnceLock::new();
        LOADED
            .get_or_init(|| {
                crate::shipped::ShippedLibrary::locate()
                    .and_then(|library| Self::load(&library))
                    .map_err(|e| e.to_string())
            })
            .clone()
            .map_err(Error::Invalid)
    }
}

impl UserSettings {
    /// The impedance tolerance the checks use: the user's own, else the
    /// shipped default.
    pub fn impedance_tolerance_or(&self, defaults: &SettingsDefaults) -> f64 {
        self.impedance_tolerance
            .unwrap_or(defaults.impedance_tolerance)
    }

    /// What the engine's checks take from these settings
    /// ([`sparksmith_core::Engine::set_check_settings`]), the unset values
    /// from the shipped defaults.
    pub fn check_settings(
        &self,
        defaults: &SettingsDefaults,
    ) -> sparksmith_core::layout::CheckSettings {
        sparksmith_core::layout::CheckSettings {
            impedance_tolerance: Some(self.impedance_tolerance_or(defaults)),
        }
    }
}

/// The check settings of the user's settings file and the located shipped
/// library, as every headless host (CLI, MCP) sets them. An unreadable
/// settings file or a missing defaults file is an error naming it.
pub fn check_settings_from_env() -> Result<sparksmith_core::layout::CheckSettings> {
    let settings = read_settings(&UserDirs::from_env()?.settings_path())?;
    Ok(settings.check_settings(&SettingsDefaults::shipped()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_core::ids::{DefinitionId, LibraryId, SourceLineageId};

    /// `path` made absolute on this platform: Windows needs a drive, so
    /// `/work` becomes `C:/work` there.
    fn abs(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_owned()
        }
    }

    /// Density, reduced motion and saved filters round-trip; saved filters
    /// are bounded, printable and unique, and saving one twice keeps one.
    #[test]
    fn list_settings_round_trip_and_bound_saved_filters() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        let mut settings = UserSettings::default();
        settings.lists.density = ListDensity::Comfortable;
        settings.lists.reduced_motion = Some(true);
        settings
            .lists
            .save_filter("pcb.parts", "side:bottom pkg:0603")?;
        settings
            .lists
            .save_filter("pcb.parts", "side:bottom pkg:0603")?;
        settings.lists.save_filter("issues", "clearance")?;
        assert_eq!(settings.lists.saved_filters.len(), 2);
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        settings.lists.remove_filter("issues", "clearance");
        assert_eq!(settings.lists.saved_filters.len(), 1);
        for (list, query) in [("", "x"), ("pcb.parts", " "), ("pcb.parts", "a\u{7}")] {
            let mut bad = settings.lists.clone();
            assert!(bad.save_filter(list, query).is_err(), "{list:?} {query:?}");
            assert_eq!(bad, settings.lists, "a refused save changes nothing");
        }
        let long = "x".repeat(MAX_FILTER_QUERY + 1);
        assert!(
            settings
                .lists
                .clone()
                .save_filter("pcb.parts", &long)
                .is_err()
        );
        let mut full = ListSettings::default();
        for i in 0..MAX_SAVED_FILTERS {
            full.save_filter("pcb.parts", &format!("x>{i}"))?;
        }
        assert!(full.save_filter("pcb.parts", "y>1").is_err());
        Ok(())
    }

    /// A settings file written while "Mesh the board directly" was a
    /// setting still reads (the setting is gone: the 3D view always meshes
    /// the board), and is saved without it.
    #[test]
    fn the_former_direct_mesh_setting_reads_and_is_not_saved() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        let mut written = serde_json::to_value(UserSettings::default())?;
        if let Some(map) = written.as_object_mut() {
            map.insert("mesh_board_directly".into(), serde_json::json!(true));
        }
        std::fs::write(&path, serde_json::to_vec(&written)?)?;
        let read = read_settings(&path)?;
        assert_eq!(read, UserSettings::default());
        write_settings(&path, &read)?;
        let saved = std::fs::read_to_string(&path)?;
        assert!(!saved.contains("mesh_board_directly"), "{saved}");
        Ok(())
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

    /// The selection filter round-trips, a default one is not written, and
    /// a file naming a kind the filter does not have is refused.
    #[test]
    fn the_selection_filter_round_trips_and_refuses_unknown_kinds() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        let settings = UserSettings {
            selection_filter: SelectionFilter {
                tracks: false,
                fills: false,
                ..SelectionFilter::default()
            },
            ..UserSettings::default()
        };
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        write_settings(&path, &UserSettings::default())?;
        assert!(!std::fs::read_to_string(&path)?.contains("selection_filter"));
        std::fs::write(
            &path,
            r#"{"format":1,"sources":[],"favourites":[],"recent":[],
            "selection_filter":{"parts":true,"tracks":true,"vias":true,"fills":true,"pads":false}}"#,
        )?;
        assert!(read_settings(&path).is_err(), "an unknown kind");
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
                PathBuf::from("/home/someone/Library/Application Support/sparksmith")
            );
            assert_eq!(
                platform.cache,
                PathBuf::from("/home/someone/Library/Caches/sparksmith")
            );
        } else if cfg!(unix) {
            assert_eq!(
                platform.config,
                PathBuf::from("/home/someone/.config/sparksmith")
            );
            assert_eq!(
                platform.cache,
                PathBuf::from("/home/someone/.cache/sparksmith")
            );
        } else if cfg!(windows) {
            assert_eq!(platform.config, PathBuf::from(&appdata).join("sparksmith"));
            assert_eq!(
                platform.cache,
                PathBuf::from(&local).join("sparksmith").join("cache")
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
        let board = PathBuf::from(abs("/work/board.sparksmith"));
        let mut settings = UserSettings::default();
        settings.remember_production_folder(board.clone(), abs("/work/out").into());
        settings.remember_production_folder(board.clone(), abs("/work/fab").into());
        settings.remember_production_folder("relative.sparksmith".into(), abs("/x").into());
        settings.remember_production_folder(abs("/work/other.sparksmith").into(), "x".into());
        assert_eq!(settings.production_folders.len(), 1, "one per project");
        write_settings(&path, &settings)?;
        let read = read_settings(&path)?;
        assert_eq!(
            read.production_folder(&board),
            Some(Path::new(&abs("/work/fab")))
        );
        assert_eq!(
            read.production_folder(Path::new(&abs("/work/x.sparksmith"))),
            None
        );
        for n in 0..40 {
            settings.remember_production_folder(
                PathBuf::from(abs(&format!("/p/{n}.sparksmith"))),
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

    /// Night finding N7: recent projects are newest first, each once,
    /// bounded, absolute, and survive a write and read.
    #[test]
    fn recent_projects_are_ordered_unique_and_persist() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(SETTINGS_FILE);
        let mut settings = UserSettings::default();
        for i in 0..12 {
            settings.remember_project(format!("/work/p{i}.sparksmith").into());
        }
        settings.remember_project("relative.sparksmith".into());
        settings.remember_project("/work/p5.sparksmith".into());
        assert_eq!(settings.projects.len(), MAX_RECENT_PROJECTS);
        assert_eq!(settings.projects[0], PathBuf::from("/work/p5.sparksmith"));
        assert_eq!(settings.projects[1], PathBuf::from("/work/p11.sparksmith"));
        let p5 = PathBuf::from("/work/p5.sparksmith");
        assert_eq!(settings.projects.iter().filter(|p| **p == p5).count(), 1);
        assert!(
            !settings
                .projects
                .contains(&PathBuf::from("/work/p0.sparksmith"))
        );
        write_settings(&path, &settings)?;
        assert_eq!(read_settings(&path)?, settings);
        let mut twice = settings.clone();
        twice.projects[1] = twice.projects[0].clone();
        assert!(write_settings(&path, &twice).is_err(), "duplicate");
        let mut relative = settings.clone();
        relative.projects[0] = "relative.sparksmith".into();
        assert!(write_settings(&path, &relative).is_err(), "relative");
        assert_eq!(read_settings(&path)?, settings, "refusals changed nothing");
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
                PathBuf::from("/h/Library/Application Support/Sparksmith/Recovery")
            );
        } else if cfg!(windows) {
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("LOCALAPPDATA", &local)]))?,
                PathBuf::from(&local).join("Sparksmith").join("Recovery")
            );
            assert!(UserDirs::recovery_from_vars(vars(&[("LOCALAPPDATA", "rel")])).is_err());
        } else {
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("HOME", "/h")]))?,
                PathBuf::from("/h/.local/state/sparksmith/recovery")
            );
            assert_eq!(
                UserDirs::recovery_from_vars(vars(&[("HOME", "/h"), ("XDG_STATE_HOME", "s")]))?,
                PathBuf::from("/h/.local/state/sparksmith/recovery"),
                "a relative XDG_STATE_HOME is ignored"
            );
        }
        Ok(())
    }

    /// The impedance tolerance is the user's own, else the shipped
    /// default; a value out of bounds is refused, and a missing defaults
    /// file is an error naming it.
    #[test]
    fn the_impedance_tolerance_is_the_users_else_the_shipped_default() -> Result<()> {
        let shipped = SettingsDefaults::load(&crate::shipped::ShippedLibrary::locate()?)?;
        let mut settings = UserSettings::default();
        assert_eq!(
            settings.check_settings(&shipped).impedance_tolerance,
            Some(shipped.impedance_tolerance)
        );
        settings.impedance_tolerance = Some(7.5);
        settings.check()?;
        assert_eq!(
            settings.check_settings(&shipped).impedance_tolerance,
            Some(7.5)
        );
        let json = serde_json::to_string(&settings)?;
        let read: UserSettings = serde_json::from_str(&json)?;
        assert_eq!(read.impedance_tolerance, Some(7.5));
        settings.impedance_tolerance = Some(0.0);
        assert!(settings.check().is_err());

        let temp = tempfile::tempdir()?;
        let root = temp.path().join("library");
        std::fs::create_dir_all(root.join("defaults"))?;
        std::fs::write(root.join(crate::shipped::MARKER), "{\"format\":1}")?;
        let library = crate::shipped::ShippedLibrary::at(&root)?;
        let missing = SettingsDefaults::load(&library).map_err(|e| e.to_string());
        assert!(
            missing.as_ref().is_err_and(|e| e.contains(DEFAULTS_FILE)),
            "{missing:?}"
        );
        std::fs::write(
            root.join("defaults").join(DEFAULTS_FILE),
            r#"{"format":1,"impedance_tolerance_pct":90}"#,
        )?;
        let out_of_bounds = SettingsDefaults::load(&library).map_err(|e| e.to_string());
        assert!(
            out_of_bounds
                .as_ref()
                .is_err_and(|e| e.contains(DEFAULTS_FILE)),
            "{out_of_bounds:?}"
        );
        Ok(())
    }
}
