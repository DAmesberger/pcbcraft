//! The session's library sources (S3): local library folders listed in the
//! per-user settings, each opened and indexed through io and handed to core
//! queries as `CatalogContext::sources`. Folder effects (reading a bundle,
//! writing a promoted one) happen here; every document change still goes
//! through one `Engine::apply`. Sources never enter the project or undo
//! history.
use crate::{Error, Result};
use pcbcraft_core::catalog::{CatalogContext, EntryKey, SourceCatalog};
use pcbcraft_core::ids::{LibraryId, Revision, SourceLineageId};
use pcbcraft_core::library::bundle::{self, Bundle, SourceRevision};
use pcbcraft_io::library_folder::{self, Disabled, Folder};
use pcbcraft_io::library_folder_index::{self, IndexState};
use pcbcraft_io::settings::{
    self, MAX_FAVOURITES, MAX_RECENT, MAX_SOURCES, SourceSetting, UserDirs, UserSettings,
};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// How a source's catalog was obtained.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexOrigin {
    /// The per-user index cache matched the folder.
    Cached,
    /// The cache was missing or stale and was rebuilt.
    Rebuilt,
    /// No cache directory, or the cache could not be written: built in
    /// memory from the folder's records.
    Memory,
}

/// One configured source as `library.sources` reports it. A source that
/// cannot be searched stays listed with its reason.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SourceInfo {
    pub path: String,
    pub enabled: bool,
    pub writable: bool,
    /// `ready`, `disabled` (with `reason`) or `failed` (with `error`).
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<LibraryId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Catalog entries (component and Part lineages at their latest revision).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<usize>,
    /// Package template rows (latest revisions), which `library.templates`
    /// searches; a folder of rows alone has no entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_rows: Option<usize>,
    /// Revision records the folder holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexOrigin>,
    /// Why a disabled source is not searchable, as the Browser shows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why a source with a valid manifest could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<pcbcraft_core::Diagnostic>,
}

enum State {
    Ready {
        folder: Folder,
        catalog: Arc<SourceCatalog>,
        index: IndexOrigin,
    },
    Disabled(Disabled),
    Failed(pcbcraft_core::Diagnostic),
}

struct Loaded {
    setting: SourceSetting,
    state: State,
}

/// The source registry: settings (sources, favourites, recents) and the
/// loaded state of each source. With [`UserDirs`] the settings are read
/// from and written to the user's config directory and catalogs are cached
/// in the user's cache directory; without, everything stays in memory.
#[derive(Default)]
pub struct Sources {
    dirs: Option<UserDirs>,
    settings: UserSettings,
    loaded: Vec<Loaded>,
}

impl Sources {
    /// Read the user's settings and load every configured source.
    pub fn with_dirs(dirs: UserDirs) -> Result<Self> {
        let settings = settings::read_settings(&dirs.settings_path())?;
        let mut sources = Self {
            dirs: Some(dirs),
            settings,
            loaded: Vec::new(),
        };
        sources.rescan();
        Ok(sources)
    }

    /// [`Self::with_dirs`] of the user's directories (`PCBCRAFT_CONFIG_DIR`
    /// and `PCBCRAFT_CACHE_DIR` when set).
    pub fn from_env() -> Result<Self> {
        Self::with_dirs(UserDirs::from_env()?)
    }

    pub fn settings(&self) -> &UserSettings {
        &self.settings
    }

    fn load(&self, setting: &SourceSetting) -> State {
        if !setting.enabled {
            return State::Disabled(Disabled::Off);
        }
        let folder = match library_folder::inspect_folder(&setting.path) {
            Ok(folder) => folder,
            Err(why) => return State::Disabled(why),
        };
        let built = match &self.dirs {
            Some(dirs) => {
                library_folder_index::load_or_build(&dirs.cache, &folder).map(|(catalog, state)| {
                    let index = match state {
                        IndexState::Cached => IndexOrigin::Cached,
                        IndexState::Rebuilt => IndexOrigin::Rebuilt,
                        // The cache could not be written; the catalog is
                        // the one built in memory.
                        IndexState::Uncached(_) => IndexOrigin::Memory,
                    };
                    (catalog, index)
                })
            }
            None => library_folder::read_records(&folder).and_then(|stored| {
                let records: Vec<_> = stored.into_iter().map(|s| s.record).collect();
                pcbcraft_core::catalog::source_catalog(
                    folder.manifest.library,
                    &folder.manifest.name,
                    &records,
                )
                .map(|catalog| (catalog, IndexOrigin::Memory))
                .map_err(pcbcraft_io::Error::from)
            }),
        };
        match built {
            Ok((catalog, index)) => State::Ready {
                folder,
                catalog: Arc::new(catalog),
                index,
            },
            Err(error) => State::Failed(error.diagnostic()),
        }
    }

    /// Reload every configured source (after another process changed a
    /// folder, or to pick up a folder that came back).
    pub fn rescan(&mut self) {
        let mut seen: Vec<(LibraryId, String)> = Vec::new();
        let mut loaded = Vec::with_capacity(self.settings.sources.len());
        for setting in &self.settings.sources {
            let mut state = self.load(setting);
            // One library id, one catalog: a second folder holding a
            // library already listed is not searched.
            if let State::Ready { folder, .. } = &state {
                let library = folder.manifest.library;
                if let Some((_, first)) = seen.iter().find(|(l, _)| *l == library) {
                    state = State::Failed(
                        pcbcraft_core::Error::Conflict(format!("same library as {first}"))
                            .diagnostic(),
                    );
                } else {
                    seen.push((library, setting.path.display().to_string()));
                }
            }
            loaded.push(Loaded {
                setting: setting.clone(),
                state,
            });
        }
        self.loaded = loaded;
    }

    fn persist(&self) -> Result<()> {
        if let Some(dirs) = &self.dirs {
            settings::write_settings(&dirs.settings_path(), &self.settings)?;
        }
        Ok(())
    }

    /// Replace the settings, persist them, then reload. A failed write
    /// leaves the registry unchanged.
    fn commit(&mut self, next: UserSettings) -> Result<()> {
        let previous = std::mem::replace(&mut self.settings, next);
        if let Err(error) = self.persist() {
            self.settings = previous;
            return Err(error);
        }
        self.rescan();
        Ok(())
    }

    /// Add a library folder. It must open (a valid manifest of this
    /// format) and must not repeat a configured path or library.
    pub fn add(&mut self, path: &Path, writable: bool) -> Result<SourceInfo> {
        let folder = library_folder::inspect_folder(path)
            .map_err(|why| refusal(&why, &format!("library folder {}: {why}", path.display())))?;
        let path = std::fs::canonicalize(path).map_err(pcbcraft_io::Error::from)?;
        if self.settings.sources.len() >= MAX_SOURCES {
            return Err(core(pcbcraft_core::Error::Limit(format!(
                "at most {MAX_SOURCES} library sources"
            ))));
        }
        if self.settings.sources.iter().any(|s| s.path == path) {
            return Err(core(pcbcraft_core::Error::Conflict(format!(
                "{} is already a library source",
                path.display()
            ))));
        }
        if let Some(other) = self
            .ready()
            .find(|(f, _)| f.manifest.library == folder.manifest.library)
        {
            return Err(core(pcbcraft_core::Error::Conflict(format!(
                "library {} ({}) is already a source at {}",
                folder.manifest.name,
                folder.manifest.library,
                other.0.root.display()
            ))));
        }
        let mut next = self.settings.clone();
        next.sources.push(SourceSetting {
            path: path.clone(),
            enabled: true,
            writable,
            name: Some(folder.manifest.name.clone()),
        });
        self.commit(next)?;
        self.info(&path)
    }

    /// Remove a configured source by path. Its folder is untouched.
    pub fn remove(&mut self, path: &Path) -> Result<()> {
        let index = self.position(path)?;
        let mut next = self.settings.clone();
        next.sources.remove(index);
        self.commit(next)
    }

    /// Turn a configured source on or off; an off source stays listed.
    pub fn enable(&mut self, path: &Path, enabled: bool) -> Result<SourceInfo> {
        let index = self.position(path)?;
        let mut next = self.settings.clone();
        if let Some(setting) = next.sources.get_mut(index) {
            setting.enabled = enabled;
        }
        let stored = next
            .sources
            .get(index)
            .map(|s| s.path.clone())
            .unwrap_or_default();
        self.commit(next)?;
        self.info(&stored)
    }

    /// Add or remove a favourite.
    pub fn favourite(&mut self, key: EntryKey, favourite: bool) -> Result<()> {
        per_user(key)?;
        let mut next = self.settings.clone();
        next.favourites.retain(|k| *k != key);
        if favourite {
            if next.favourites.len() >= MAX_FAVOURITES {
                return Err(core(pcbcraft_core::Error::Limit(format!(
                    "at most {MAX_FAVOURITES} favourites"
                ))));
            }
            next.favourites.push(key);
        }
        self.commit(next)
    }

    /// Move `key` to the front of the recents, keeping at most
    /// [`MAX_RECENT`].
    pub fn used(&mut self, key: EntryKey) -> Result<()> {
        per_user(key)?;
        let mut next = self.settings.clone();
        next.recent.retain(|k| *k != key);
        next.recent.insert(0, key);
        next.recent.truncate(MAX_RECENT);
        // Recents are a convenience: the catalogs need no reload.
        let previous = std::mem::replace(&mut self.settings, next);
        if let Err(error) = self.persist() {
            self.settings = previous;
            return Err(error);
        }
        Ok(())
    }

    fn position(&self, path: &Path) -> Result<usize> {
        let canonical = std::fs::canonicalize(path).ok();
        self.settings
            .sources
            .iter()
            .position(|s| s.path == path || Some(&s.path) == canonical.as_ref())
            .ok_or_else(|| {
                core(pcbcraft_core::Error::NotFound(format!(
                    "{} is not a library source",
                    path.display()
                )))
            })
    }

    fn ready(&self) -> impl Iterator<Item = (&Folder, &Arc<SourceCatalog>)> {
        self.loaded.iter().filter_map(|loaded| match &loaded.state {
            State::Ready {
                folder, catalog, ..
            } => Some((folder, catalog)),
            _ => None,
        })
    }

    /// The ready source holding `library`, and whether it is writable.
    fn source(&self, library: LibraryId) -> Result<(&Folder, &Arc<SourceCatalog>, bool)> {
        self.loaded
            .iter()
            .find_map(|loaded| match &loaded.state {
                State::Ready {
                    folder, catalog, ..
                } if folder.manifest.library == library => {
                    Some((folder, catalog, loaded.setting.writable))
                }
                _ => None,
            })
            .ok_or_else(|| {
                core(pcbcraft_core::Error::NotFound(format!(
                    "library {library} is not a ready library source"
                )))
            })
    }

    /// Every configured source, in settings order.
    pub fn list(&self) -> Vec<SourceInfo> {
        self.loaded.iter().map(describe).collect()
    }

    fn info(&self, path: &Path) -> Result<SourceInfo> {
        self.loaded
            .iter()
            .find(|loaded| loaded.setting.path == path)
            .map(describe)
            .ok_or_else(|| {
                core(pcbcraft_core::Error::NotFound(format!(
                    "{} is not a library source",
                    path.display()
                )))
            })
    }

    /// The query context: favourites, recents and every ready catalog.
    pub fn context(&self) -> CatalogContext {
        CatalogContext {
            favourites: self.settings.favourites.iter().copied().collect(),
            recent: self.settings.recent.clone(),
            sources: self.ready().map(|(_, catalog)| catalog.clone()).collect(),
            without_starter_rows: false,
        }
    }

    /// The newest revision `library` holds of `lineage`.
    fn latest(&self, library: LibraryId, lineage: SourceLineageId) -> Result<Revision> {
        let (_, catalog, _) = self.source(library)?;
        catalog
            .revisions
            .iter()
            .filter(|r| r.lineage == lineage)
            .map(|r| r.revision)
            .max()
            .ok_or_else(|| {
                core(pcbcraft_core::Error::NotFound(format!(
                    "library {} has no lineage {lineage}",
                    catalog.name
                )))
            })
    }

    /// The bundle of one revision (latest when `revision` is absent) and its
    /// closure, read from the folder.
    pub fn bundle(
        &self,
        library: LibraryId,
        lineage: SourceLineageId,
        revision: Option<Revision>,
    ) -> Result<Bundle> {
        let revision = match revision {
            Some(revision) => revision,
            None => self.latest(library, lineage)?,
        };
        let (folder, _, _) = self.source(library)?;
        let bundle = library_folder::read_bundle(folder, SourceRevision { lineage, revision })?;
        bundle::validate_bundle(&bundle)?;
        Ok(bundle)
    }

    /// The folder `library` is in, refusing a read-only source.
    pub fn writable(&self, library: LibraryId) -> Result<&Folder> {
        let (folder, _, writable) = self.source(library)?;
        if !writable {
            return Err(core(pcbcraft_core::Error::Conflict(format!(
                "library {} is a read-only source",
                folder.manifest.name
            ))));
        }
        Ok(folder)
    }

    /// Create an empty folder library and add it as a writable source.
    pub fn create(&mut self, path: &Path, name: &str) -> Result<SourceInfo> {
        library_folder::create_folder(path, name)?;
        self.add(path, true)
    }

    /// [`Self::create`] under a library id the caller supplies.
    pub fn create_as(&mut self, path: &Path, name: &str, library: LibraryId) -> Result<SourceInfo> {
        library_folder::create_folder_as(path, name, library)?;
        self.add(path, true)
    }
}

fn describe(loaded: &Loaded) -> SourceInfo {
    let mut info = SourceInfo {
        path: loaded.setting.path.display().to_string(),
        enabled: loaded.setting.enabled,
        writable: loaded.setting.writable,
        state: "ready",
        library: None,
        name: None,
        entries: None,
        package_rows: None,
        revisions: None,
        index: None,
        reason: None,
        error: None,
    };
    match &loaded.state {
        State::Ready {
            folder,
            catalog,
            index,
        } => {
            info.library = Some(folder.manifest.library);
            info.name = Some(folder.manifest.name.clone());
            info.entries = Some(catalog.entries.len());
            info.package_rows = Some(catalog.rows.len());
            info.revisions = Some(catalog.revisions.len());
            info.index = Some(*index);
        }
        State::Disabled(why) => {
            info.state = "disabled";
            info.reason = Some(why.to_string());
        }
        State::Failed(diagnostic) => {
            info.state = "failed";
            info.error = Some(diagnostic.clone());
        }
    }
    info
}

/// Per-user favourites and recents name library items: a project lineage
/// id means something else in every project.
fn per_user(key: EntryKey) -> Result<()> {
    match key {
        EntryKey::Library { .. } => Ok(()),
        EntryKey::Project { .. } => Err(core(pcbcraft_core::Error::Invalid(
            "favourites and recents name library items (source library and lineage); a project lineage id means something else in every project".into(),
        ))),
    }
}

fn core(error: pcbcraft_core::Error) -> Error {
    Error::Document(error)
}

/// A missing folder is `not_found`; anything else the manifest says is
/// `unsupported` (another format or feature) or `invalid_input`.
fn refusal(why: &Disabled, message: &str) -> Error {
    core(match why {
        Disabled::PathMissing => pcbcraft_core::Error::NotFound(message.into()),
        Disabled::UnsupportedFormat(_) | Disabled::UnknownFeatures(_) => {
            pcbcraft_core::Error::Unsupported(message.into())
        }
        Disabled::Off | Disabled::Malformed(_) => pcbcraft_core::Error::Invalid(message.into()),
    })
}
