//! The shipped library: the data that comes with Sparksmith (fab profiles,
//! stackups, materials, standards, standard fields, assistant profiles, part
//! files, templates and generator modules). It is loaded as data from one
//! folder, never compiled into the program, so the program holds no rules,
//! profiles or library data of its own.
//!
//! The folder is found once, in this order:
//!
//! 1. `SPARKSMITH_LIBRARY_DIR`, when set (it must hold the marker file);
//! 2. the macOS app bundle, `Contents/Resources/library`;
//! 3. `<prefix>/share/sparksmith/library` beside `<prefix>/bin`;
//! 4. `library` beside the executable;
//! 5. a `library` folder in the executable's ancestors, which is how a
//!    build or test run inside the source tree finds the repository's
//!    `library/`.
//!
//! Each candidate counts only when it holds [`MARKER`]. Per-user files
//! (for example `fab-profiles/` in the config folder) still override a
//! shipped entry with the same id; that stays with each loader.
use crate::{Error, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The environment variable that names the shipped library folder.
pub const ENV: &str = "SPARKSMITH_LIBRARY_DIR";
/// The file that marks a folder as the shipped library.
pub const MARKER: &str = "sparksmith-shipped.json";
/// The folder name the library has in an install and in the source tree.
pub const FOLDER: &str = "library";
/// How many ancestors of the executable are searched for a source tree.
const MAX_ANCESTORS: usize = 8;

/// One kind of shipped data, each in its own subfolder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shelf {
    /// Fab profiles (`profiles/*.json`).
    Profiles,
    /// Layer stackups (`stackups/`).
    Stackups,
    /// Body, board and finish materials (`materials/`).
    Materials,
    /// Published standards: preferred number series, marking codes
    /// (`standards/`).
    Standards,
    /// Standard quantity fields (`standard-fields/`).
    StandardFields,
    /// Assistant run profiles and prompts (`assist/`).
    Assist,
    /// Part files (`parts/*.solidcraft` with their sidecars).
    Parts,
    /// Package templates and starter rows (`templates/`).
    Templates,
    /// Built generator modules (`generators/`).
    Generators,
}

impl Shelf {
    /// The subfolder name.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Profiles => "profiles",
            Self::Stackups => "stackups",
            Self::Materials => "materials",
            Self::Standards => "standards",
            Self::StandardFields => "standard-fields",
            Self::Assist => "assist",
            Self::Parts => "parts",
            Self::Templates => "templates",
            Self::Generators => "generators",
        }
    }
}

/// The located shipped library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShippedLibrary {
    root: PathBuf,
}

impl ShippedLibrary {
    /// Find the shipped library from the environment and the running
    /// executable.
    pub fn locate() -> Result<Self> {
        Self::locate_from(|name| std::env::var_os(name), std::env::current_exe().ok())
    }

    /// [`locate`](Self::locate) with the environment and executable path
    /// given, for tests.
    pub fn locate_from(
        var: impl Fn(&str) -> Option<OsString>,
        exe: Option<PathBuf>,
    ) -> Result<Self> {
        if let Some(dir) = var(ENV).filter(|value| !value.is_empty()) {
            let dir = PathBuf::from(dir);
            return Self::at(&dir).map_err(|_| {
                Error::Invalid(format!(
                    "{ENV} names {}, which is not Sparksmith's shipped library: it holds no {MARKER}.",
                    dir.display()
                ))
            });
        }
        let candidates = exe.as_deref().map(candidates).unwrap_or_default();
        for dir in &candidates {
            if let Ok(found) = Self::at(dir) {
                return Ok(found);
            }
        }
        let looked = if candidates.is_empty() {
            "nowhere (the program's own path is unknown)".to_string()
        } else {
            candidates
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        Err(Error::Invalid(format!(
            "Sparksmith's shipped library was not found. Looked in {looked}. Reinstall Sparksmith, or set {ENV} to the folder that holds {MARKER}."
        )))
    }

    /// The library at `root`, which must hold [`MARKER`].
    pub fn at(root: &Path) -> Result<Self> {
        if root.join(MARKER).is_file() {
            Ok(Self {
                root: root.to_path_buf(),
            })
        } else {
            Err(Error::Invalid(format!(
                "{} holds no {MARKER}.",
                root.display()
            )))
        }
    }

    /// The library's folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of one kind of data. It may not exist when nothing of
    /// that kind ships.
    pub fn shelf(&self, shelf: Shelf) -> PathBuf {
        self.root.join(shelf.dir_name())
    }

    /// Read `name` from `shelf`, refusing a file over `max_bytes` and a
    /// name that leaves the shelf.
    pub fn read(&self, shelf: Shelf, name: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let relative = Path::new(name);
        if name.is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(Error::Invalid(format!(
                "{name:?} is not a file name inside the shipped {} folder.",
                shelf.dir_name()
            )));
        }
        let path = self.shelf(shelf).join(relative);
        let size = std::fs::metadata(&path)?.len();
        if size > max_bytes {
            return Err(Error::TooLarge(format!(
                "{} is {size} bytes; the limit for shipped {} files is {max_bytes}.",
                path.display(),
                shelf.dir_name()
            )));
        }
        Ok(std::fs::read(path)?)
    }

    /// The files directly in `shelf` whose names end with `suffix`, sorted
    /// by name. A missing shelf lists nothing.
    pub fn list(&self, shelf: Shelf, suffix: &str) -> Result<Vec<String>> {
        let dir = self.shelf(shelf);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && name.ends_with(suffix)
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }
}

/// The extension of a built generator module on the Generators shelf.
pub const GENERATOR_EXTENSION: &str = ".wasm";

impl ShippedLibrary {
    /// The names of the shipped generator modules (`generators/<name>.wasm`),
    /// sorted.
    pub fn generators(&self) -> Result<Vec<String>> {
        Ok(self
            .list(Shelf::Generators, GENERATOR_EXTENSION)?
            .into_iter()
            .filter_map(|file| file.strip_suffix(GENERATOR_EXTENSION).map(str::to_string))
            .collect())
    }

    /// The bytes of shipped generator module `name`, at most the host's
    /// module limit. A missing module names the folder it was looked for in.
    pub fn generator(&self, name: &str) -> Result<Vec<u8>> {
        let valid = !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !valid {
            return Err(Error::Invalid(format!(
                "{name:?} is not a generator name: lower-case letters, digits, - and _."
            )));
        }
        let file = format!("{name}{GENERATOR_EXTENSION}");
        if !self.shelf(Shelf::Generators).join(&file).is_file() {
            let shipped = self.generators().unwrap_or_default();
            return Err(Error::Invalid(format!(
                "No generator module {name} is shipped in {}; shipped: {}.",
                self.shelf(Shelf::Generators).display(),
                if shipped.is_empty() {
                    "none".to_string()
                } else {
                    shipped.join(", ")
                }
            )));
        }
        let limit = u64::try_from(sparksmith_core::library::generated::MAX_MODULE_BYTES)
            .unwrap_or(u64::MAX);
        self.read(Shelf::Generators, &file, limit)
    }
}

/// Where an executable at `exe` may find the library, in search order.
fn candidates(exe: &Path) -> Vec<PathBuf> {
    let Some(bin) = exe.parent() else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    if let Some(contents) = bin.parent() {
        dirs.push(contents.join("Resources").join(FOLDER));
        dirs.push(contents.join("share").join("sparksmith").join(FOLDER));
    }
    dirs.push(bin.join(FOLDER));
    for ancestor in bin.ancestors().skip(1).take(MAX_ANCESTORS) {
        let dir = ancestor.join(FOLDER);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(dir: &Path) -> Result<PathBuf> {
        let root = dir.join(FOLDER);
        std::fs::create_dir_all(root.join("profiles"))?;
        std::fs::write(root.join(MARKER), "{\"format\":1}\n")?;
        Ok(root)
    }

    #[test]
    fn the_variable_wins_and_must_name_a_library() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = library(temp.path())?;
        let found = ShippedLibrary::locate_from(
            |name| (name == ENV).then(|| root.clone().into_os_string()),
            None,
        )?;
        assert_eq!(found.root(), root);
        let wrong = temp.path().to_path_buf();
        let error = ShippedLibrary::locate_from(
            |name| (name == ENV).then(|| wrong.clone().into_os_string()),
            None,
        );
        assert!(matches!(error, Err(Error::Invalid(message)) if message.contains(MARKER)));
        Ok(())
    }

    #[test]
    fn a_bundle_and_a_source_tree_are_found_from_the_executable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let contents = temp.path().join("Sparksmith.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS"))?;
        let bundled = library(&contents.join("Resources"))?;
        let found = ShippedLibrary::locate_from(|_| None, Some(contents.join("MacOS/app")))?;
        assert_eq!(found.root(), bundled);

        let tree = temp.path().join("src");
        let in_tree = library(&tree)?;
        let exe = tree.join("target/nix/ci/deps/test-binary");
        let found = ShippedLibrary::locate_from(|_| None, Some(exe))?;
        assert_eq!(found.root(), in_tree);
        Ok(())
    }

    #[test]
    fn a_missing_library_says_where_it_looked() {
        let error = ShippedLibrary::locate_from(|_| None, Some(PathBuf::from("/nowhere/bin/app")));
        assert!(
            matches!(error, Err(Error::Invalid(message)) if message.contains("/nowhere/bin/library") && message.contains(ENV))
        );
    }

    #[test]
    fn reads_stay_inside_their_shelf_and_within_the_limit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = library(temp.path())?;
        std::fs::write(root.join("profiles/a.json"), "{}")?;
        std::fs::write(root.join("profiles/b.txt"), "x")?;
        let shipped = ShippedLibrary::at(&root)?;
        assert_eq!(shipped.read(Shelf::Profiles, "a.json", 16)?, b"{}");
        assert!(matches!(
            shipped.read(Shelf::Profiles, "a.json", 1),
            Err(Error::TooLarge(_))
        ));
        assert!(
            shipped
                .read(Shelf::Profiles, "../sparksmith-shipped.json", 64)
                .is_err()
        );
        assert_eq!(
            shipped.list(Shelf::Profiles, ".json")?,
            vec!["a.json".to_string()]
        );
        assert!(shipped.list(Shelf::Materials, ".json")?.is_empty());
        Ok(())
    }

    #[test]
    fn generator_modules_are_read_by_name_within_the_shelf() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = library(temp.path())?;
        std::fs::create_dir_all(root.join("generators"))?;
        std::fs::write(root.join("generators/chip.wasm"), b"\0asm")?;
        let shipped = ShippedLibrary::at(&root)?;
        assert_eq!(shipped.generators()?, vec!["chip".to_string()]);
        assert_eq!(shipped.generator("chip")?, b"\0asm");
        assert!(matches!(
            shipped.generator("../chip"),
            Err(Error::Invalid(_))
        ));
        assert!(
            matches!(shipped.generator("gone"), Err(Error::Invalid(m)) if m.contains("shipped: chip"))
        );
        Ok(())
    }

    #[test]
    fn the_repository_library_is_found_from_this_test() -> Result<()> {
        let found = ShippedLibrary::locate_from(|_| None, std::env::current_exe().ok())?;
        assert!(found.root().ends_with(FOLDER));
        Ok(())
    }
}
