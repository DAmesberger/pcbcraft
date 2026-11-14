//! The schematic files beside a KiCad board, read for the importer
//! (`sparksmith_kicad`, which reads only the bytes it is given): every
//! `.kicad_sch` in the board's folder, by file name, and the root's name
//! (the board's stem). Read only; nothing in the folder is written.
use std::collections::BTreeMap;
use std::path::Path;

/// Largest schematic file read, bytes.
pub const MAX_SCHEMATIC: u64 = 64 * 1024 * 1024;
/// Most schematic files read from one folder.
pub const MAX_FILES: usize = 512;

/// The root schematic's name and every schematic file's bytes by name.
pub type SchematicFiles = (String, BTreeMap<String, Vec<u8>>);

/// The schematic files beside `board`, or `None` when it has no root
/// schematic beside it.
pub fn schematic_files(board: &Path) -> std::io::Result<Option<SchematicFiles>> {
    let Some(dir) = board.parent() else {
        return Ok(None);
    };
    let root = board.with_extension("kicad_sch");
    let Some(root_name) = root.file_name().and_then(|n| n.to_str()) else {
        return Ok(None);
    };
    if !root.is_file() {
        return Ok(None);
    }
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !path.is_file() || !name.ends_with(".kicad_sch") || name.starts_with(['.', '_']) {
            continue;
        }
        if files.len() >= MAX_FILES || std::fs::metadata(&path)?.len() > MAX_SCHEMATIC {
            return Err(std::io::Error::other(
                "kicad.schematic_limit: too many or too large schematic files",
            ));
        }
        files.insert(name.to_owned(), std::fs::read(&path)?);
    }
    Ok(Some((root_name.to_owned(), files)))
}
