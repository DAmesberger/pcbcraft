//! What a new project starts with, read from the shipped library
//! ([`crate::shipped`]): `defaults/new-project.json` names the fab profile,
//! the stackup family and its layer count, and the board size; the
//! stackup family is a file in `stackups/`. The core holds none of these
//! values: it takes the loaded [`NewBoard`] and [`StackupPresets`].
//!
//! A missing or broken file is an error naming it, never a value written
//! in code. The user's Settings default fab profile still overrides the
//! shipped one (`rule_profiles::for_new_board`).
use crate::shipped::{Shelf, ShippedLibrary};
use crate::{Error, Result};
use serde::Deserialize;
use sparksmith_core::format::EditorFile;
use sparksmith_core::{Engine, NewBoard, StackupPresets};
use std::sync::OnceLock;

/// The new-project defaults file in the `defaults/` shelf.
pub const FILE: &str = "new-project.json";
/// The format of the defaults file and of a stackup family file.
pub const FORMAT: u32 = 1;
/// Bytes of the defaults file or one stackup family file.
pub const MAX_BYTES: u64 = 256 * 1024;
/// Largest new board side, mm (an input bound on the data).
const MAX_BOARD_MM: f64 = 2000.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultsFile {
    format: u32,
    /// A note for people reading the file.
    #[serde(default, rename = "about")]
    _about: String,
    fab_profile: String,
    stackup: String,
    copper_layers: usize,
    board_size_mm: [f64; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StackupFile {
    format: u32,
    stackup_presets: StackupPresets,
}

/// The shipped new-project defaults, loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct NewProjectDefaults {
    /// The fab profile id a new board is offered first, unless the user's
    /// Settings name another.
    pub fab_profile: String,
    /// The stackup family: presets of every layer count, added layers and
    /// the Dk estimates.
    pub presets: StackupPresets,
    /// The board a new project starts on.
    pub board: NewBoard,
}

fn invalid(path: &std::path::Path, message: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{}: {message}", path.display()))
}

fn check_format(path: &std::path::Path, format: u32) -> Result<()> {
    if format == FORMAT {
        Ok(())
    } else {
        Err(Error::Document(sparksmith_core::Error::Unsupported(
            format!("{}: unsupported format {format}", path.display()),
        )))
    }
}

/// The stackup family `name` (a file in the `stackups/` shelf).
pub fn read_stackups(shipped: &ShippedLibrary, name: &str) -> Result<StackupPresets> {
    let path = shipped.shelf(Shelf::Stackups).join(name);
    let bytes = shipped
        .read(Shelf::Stackups, name, MAX_BYTES)
        .map_err(|e| invalid(&path, e))?;
    let file: StackupFile = serde_json::from_slice(&bytes).map_err(|e| invalid(&path, e))?;
    check_format(&path, file.format)?;
    file.stackup_presets
        .check()
        .map_err(|e| invalid(&path, e))?;
    Ok(file.stackup_presets)
}

/// Load the new-project defaults from `shipped`.
pub fn load(shipped: &ShippedLibrary) -> Result<NewProjectDefaults> {
    let path = shipped.shelf(Shelf::Defaults).join(FILE);
    let bytes = shipped
        .read(Shelf::Defaults, FILE, MAX_BYTES)
        .map_err(|e| invalid(&path, e))?;
    let DefaultsFile {
        format,
        _about: _,
        fab_profile,
        stackup,
        copper_layers,
        board_size_mm,
    } = serde_json::from_slice(&bytes).map_err(|e| invalid(&path, e))?;
    check_format(&path, format)?;
    if !crate::rule_profiles::valid_id(&fab_profile) {
        return Err(invalid(
            &path,
            format!("{fab_profile:?} is not a fab profile id"),
        ));
    }
    let [w, h] = board_size_mm;
    if !(w.is_finite()
        && h.is_finite()
        && w > 0.0
        && h > 0.0
        && w <= MAX_BOARD_MM
        && h <= MAX_BOARD_MM)
    {
        return Err(invalid(
            &path,
            format!("the board size must be within 0–{MAX_BOARD_MM} mm"),
        ));
    }
    let presets = read_stackups(shipped, &stackup)?;
    let stackup = presets
        .preset(copper_layers)
        .map_err(|e| invalid(&path, e))?;
    Ok(NewProjectDefaults {
        fab_profile,
        presets,
        board: NewBoard {
            size: board_size_mm,
            stackup,
        },
    })
}

/// [`load`] from the located shipped library, once per process.
pub fn shipped() -> Result<NewProjectDefaults> {
    static LOADED: OnceLock<std::result::Result<NewProjectDefaults, String>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            ShippedLibrary::locate()
                .and_then(|library| load(&library))
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(Error::Invalid)
}

/// An engine on a new, empty project on the shipped new board.
pub fn engine() -> Result<Engine> {
    Ok(Engine::new(&shipped()?.board))
}

/// [`engine`] on the fab profile a new board takes: the user's Settings
/// default, else the shipped default (`rule_profiles::for_new_board`).
pub fn profiled_engine() -> Result<Engine> {
    let defaults = shipped()?;
    let mut project = sparksmith_core::Project::new(&defaults.board);
    sparksmith_core::rules::apply(
        &mut project.rules,
        sparksmith_core::rules::RulesCommand::SetProfile {
            profile: Box::new(crate::rule_profiles::for_new_board_from_env(None)?),
            keep_overrides: false,
        },
    )?;
    Ok(Engine::from_file_with(
        EditorFile {
            version: sparksmith_core::format::FILE_VERSION,
            project,
            drafts: Default::default(),
        },
        &defaults.presets.glass_dk,
    )?)
}

/// An engine for an already validated file, its stackup estimating Dk
/// with the shipped stackup data.
pub fn open(file: EditorFile) -> Result<Engine> {
    Ok(Engine::from_file_with(file, &shipped()?.presets.glass_dk)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_defaults_load_and_equal_the_core_fixture() -> Result<()> {
        let defaults = load(&ShippedLibrary::locate()?)?;
        assert!(
            crate::rule_profiles::ProfileLibrary::new(
                tempfile::tempdir()?.path().to_path_buf(),
                ShippedLibrary::locate()?
            )
            .get(&defaults.fab_profile)
            .is_ok(),
            "the default profile ships"
        );
        // The core's test fixtures stand for the shipped data: the tests
        // measured against them measure the product.
        let fixture = StackupPresets::fixture()?;
        let shipped = StackupPresets {
            id: fixture.id.clone(),
            name: fixture.name.clone(),
            sources: fixture.sources.clone(),
            ..defaults.presets.clone()
        };
        assert_eq!(shipped, fixture);
        assert_eq!(defaults.board, NewBoard::fixture());
        assert_eq!(
            defaults.board.stackup.dk_estimates.0,
            defaults.presets.glass_dk
        );
        Ok(())
    }

    #[test]
    fn a_broken_or_missing_file_is_an_error_naming_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("library");
        std::fs::create_dir_all(root.join("defaults"))?;
        std::fs::create_dir_all(root.join("stackups"))?;
        std::fs::write(root.join(crate::shipped::MARKER), "{\"format\":1}")?;
        let library = ShippedLibrary::at(&root)?;
        let missing = load(&library).map_err(|e| e.to_string());
        assert!(
            missing.as_ref().is_err_and(|e| e.contains(FILE)),
            "{missing:?}"
        );
        std::fs::write(
            root.join("defaults").join(FILE),
            r#"{"format":1,"fab_profile":"a","stackup":"s.json","copper_layers":2,"board_size_mm":[10,10]}"#,
        )?;
        let no_stackup = load(&library).map_err(|e| e.to_string());
        assert!(
            no_stackup.as_ref().is_err_and(|e| e.contains("s.json")),
            "{no_stackup:?}"
        );
        std::fs::write(root.join("stackups").join("s.json"), "{\"format\":1")?;
        let broken = load(&library).map_err(|e| e.to_string());
        assert!(
            broken.as_ref().is_err_and(|e| e.contains("s.json")),
            "{broken:?}"
        );
        Ok(())
    }
}
