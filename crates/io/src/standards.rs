//! The standards from the shipped library (`docs/shipped-library.md`): the
//! standard fields, the marking-code tables, the generic-part search
//! grammar and the schematic symbol drawings. They are read here, checked
//! by the core's constructors and installed for the process, so every
//! engine (desktop, CLI, MCP) uses the same data. A missing or broken file
//! is an error naming it.
use crate::shipped::{Shelf, ShippedLibrary};
use crate::{Error, Result};
use serde::de::DeserializeOwned;
use sparksmith_core::catalog::generic;
use sparksmith_core::library::marking::codes::{
    ColourCode, Eia96Code, MarkingCodes, PreferredSeries, VoltageLetterTables,
};
use sparksmith_core::library::standard_fields::StandardFields;
use sparksmith_core::library::standards::{self, Standards};
use sparksmith_core::library::symbol_style::{
    KindDrawing, SymbolDrawings, SymbolKind, SymbolStyle,
};

/// The largest standards file read.
const MAX_FILE_BYTES: u64 = 1 << 20;

/// The standard fields bundle, as `library.import` takes it.
pub const STANDARD_FIELDS: (Shelf, &str) = (Shelf::StandardFields, "standard-fields.json");
/// What the standard fields are for.
pub const FIELD_ROLES: (Shelf, &str) = (Shelf::StandardFields, "roles.json");
/// The E96 preferred number series.
pub const E96: (Shelf, &str) = (Shelf::Standards, "e96.json");
/// The EIA-96 resistor code's multiplier letters.
pub const EIA96: (Shelf, &str) = (Shelf::Standards, "eia-96.json");
/// The named rated-voltage letter tables.
pub const VOLTAGE_LETTERS: (Shelf, &str) = (Shelf::Standards, "voltage-letters.json");
/// The resistor colour code.
pub const COLOUR_CODE: (Shelf, &str) = (Shelf::Standards, "colour-code.json");
/// The generic-part search grammar.
pub const GENERIC_FAMILIES: (Shelf, &str) = (Shelf::Catalog, "generic-families.json");
/// The shared sizes of generated symbols.
pub const SYMBOL_STYLE: (Shelf, &str) = (Shelf::Standards, "symbols/style.json");

/// The path a message names for `name` on `shelf`.
fn shown(shipped: &ShippedLibrary, (shelf, name): (Shelf, &str)) -> String {
    shipped.shelf(shelf).join(name).display().to_string()
}

/// `file` read and parsed, any failure naming the file.
fn read<T: DeserializeOwned>(shipped: &ShippedLibrary, file: (Shelf, &str)) -> Result<T> {
    let (shelf, name) = file;
    let bytes = shipped
        .read(shelf, name, MAX_FILE_BYTES)
        .map_err(|e| Error::Invalid(format!("{}: {e}", shown(shipped, file))))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::Invalid(format!("{}: {e}", shown(shipped, file))))
}

/// A core check of what `file` holds, failing with the file named.
fn checked<T>(
    shipped: &ShippedLibrary,
    file: (Shelf, &str),
    value: sparksmith_core::Result<T>,
) -> Result<T> {
    value.map_err(|e| Error::Invalid(format!("{}: {e}", shown(shipped, file))))
}

/// The drawing file of a drawn symbol kind: `symbols/<kind>.json`.
pub fn symbol_file(kind: SymbolKind) -> Result<String> {
    let value = serde_json::to_value(kind)?;
    let name = value
        .as_str()
        .ok_or_else(|| Error::Invalid("a symbol kind is not named by text".into()))?;
    Ok(format!("symbols/{name}.json"))
}

/// Every standard the shipped library holds, checked.
pub fn load(shipped: &ShippedLibrary) -> Result<Standards> {
    let fields = checked(
        shipped,
        STANDARD_FIELDS,
        StandardFields::new(read(shipped, STANDARD_FIELDS)?, read(shipped, FIELD_ROLES)?),
    )?;
    let e96: PreferredSeries = read(shipped, E96)?;
    checked(shipped, E96, e96.check())?;
    let eia96: Eia96Code = read(shipped, EIA96)?;
    checked(shipped, EIA96, eia96.check())?;
    let letters: VoltageLetterTables = read(shipped, VOLTAGE_LETTERS)?;
    checked(shipped, VOLTAGE_LETTERS, letters.check())?;
    let colours: ColourCode = read(shipped, COLOUR_CODE)?;
    checked(shipped, COLOUR_CODE, colours.check())?;
    let marking = MarkingCodes::new(e96, eia96, letters, colours)?;
    let grammar = read(shipped, GENERIC_FAMILIES)?;
    checked(shipped, GENERIC_FAMILIES, generic::check(&grammar))?;
    let mut kinds = Vec::new();
    for kind in SymbolKind::ALL {
        if kind.terminal_count().is_none() {
            continue;
        }
        let name = symbol_file(kind)?;
        let file = (Shelf::Standards, name.as_str());
        let drawing: KindDrawing = read(shipped, file)?;
        if drawing.kind != kind {
            return Err(Error::Invalid(format!(
                "{}: it holds the {} drawing",
                shown(shipped, file),
                drawing.kind.label()
            )));
        }
        checked(shipped, file, drawing.check())?;
        kinds.push(drawing);
    }
    let style: SymbolStyle = read(shipped, SYMBOL_STYLE)?;
    checked(shipped, SYMBOL_STYLE, style.check())?;
    let symbols = SymbolDrawings::new(style, kinds)?;
    checked(
        shipped,
        GENERIC_FAMILIES,
        Standards::new(fields, marking, grammar, symbols),
    )
}

/// Load the standards of `shipped` and install them for this process.
pub fn install(shipped: &ShippedLibrary) -> Result<()> {
    standards::install(load(shipped)?)?;
    Ok(())
}

/// Find the shipped library and install its standards: what each program
/// does at startup, before any engine runs.
pub fn install_shipped() -> Result<()> {
    install(&ShippedLibrary::locate()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn repository() -> Result<ShippedLibrary> {
        ShippedLibrary::locate_from(|_| None, std::env::current_exe().ok())
    }

    /// A copy of the shipped library's standards in a temporary folder.
    fn copy(to: &Path) -> Result<ShippedLibrary> {
        let from = repository()?;
        std::fs::write(to.join(crate::shipped::MARKER), "{\"format\":1}\n")?;
        for shelf in [Shelf::StandardFields, Shelf::Standards, Shelf::Catalog] {
            copy_dir(&from.shelf(shelf), &to.join(shelf.dir_name()))?;
        }
        ShippedLibrary::at(to)
    }

    fn copy_dir(from: &Path, to: &Path) -> Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    /// The error of a load that must fail.
    fn message(loaded: Result<Standards>) -> String {
        match loaded {
            Ok(_) => "loaded".into(),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn the_shipped_standards_load_and_match_what_the_core_tests_use() -> Result<()> {
        let loaded = load(&repository()?)?;
        assert_eq!(&loaded, standards::get()?);
        // Installing the same standards again is a no-op.
        install(&repository()?)?;
        Ok(())
    }

    #[test]
    fn a_missing_file_is_named() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let shipped = copy(temp.path())?;
        std::fs::remove_file(temp.path().join("standards/symbols/zener.json"))?;
        let error = message(load(&shipped));
        assert!(error.contains("zener.json"), "{error}");
        Ok(())
    }

    #[test]
    fn a_broken_file_is_named() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let shipped = copy(temp.path())?;
        std::fs::write(temp.path().join("standards/e96.json"), "{\"format\": 1")?;
        let error = message(load(&shipped));
        assert!(error.contains("e96.json"), "{error}");

        let shipped = copy(temp.path())?;
        let grammar = temp.path().join("catalog/generic-families.json");
        let text = std::fs::read_to_string(&grammar)?.replace("{value:ohm}?", "{value:watt}?");
        std::fs::write(&grammar, text)?;
        let error = message(load(&shipped));
        assert!(
            error.contains("generic-families.json") && error.contains("watt"),
            "{error}"
        );

        let shipped = copy(temp.path())?;
        let colours = temp.path().join("standards/colour-code.json");
        let text = std::fs::read_to_string(&colours)?.replace("\"digit\": 3", "\"digit\": 2");
        std::fs::write(&colours, text)?;
        let error = message(load(&shipped));
        assert!(error.contains("colour-code.json"), "{error}");
        Ok(())
    }
}
