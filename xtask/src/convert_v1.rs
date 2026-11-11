//! `cargo xtask convert-v1 FILE…`: a one-off converter for `.pcbcraft`
//! containers written before a units-migration slice changed the file
//! format. Pcbcraft is pre-release and keeps one file version with no
//! legacy readers (AGENTS.md), so this lives here, not in the app.
//!
//! U1 (schematic): stored f64 millimetres become integer nanometres. Each
//! `[x, y]` point becomes `{"x_nm", "y_nm"}`, and `radius`, `offset` and
//! `height` become `radius_nm`, `offset_nm` and `height_nm`. Rounding is
//! the app's own, half away from zero (`units::Nm::from_mm_f64_rounded`).
//!
//! Rule profiles as files ([`crate::convert_rules`]): the old profile
//! snapshot becomes `--profile FILE.json`'s profile, or, for one the user
//! entered from a fabricator, the same values in the new form. An old
//! profile library file (`FILE.json`) converts in place too.
//!
//! A value already converted is left alone, so running it twice changes
//! nothing. The file is replaced atomically, after the original is copied
//! to `FILE.pre-nm` (never overwritten). Other entries are copied raw.
use crate::Result;
use serde_json::{Map, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const DOCUMENT: &str = "project.json";
/// Bytes of a container's document, as `format::MAX_FILE_BYTES` bounds it.
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
/// `units::MAX_COORD_NM`.
const MAX_COORD_NM: f64 = 2_000_000_000.0;

pub fn run(args: &[String]) -> Result<()> {
    const USAGE: &str =
        "usage: cargo xtask convert-v1 [--profile PROFILE.json] FILE.pcbcraft|OLD_PROFILE.json…";
    let (pinned, paths) = match args {
        [flag, profile, rest @ ..] if flag == "--profile" => (Some(read_profile(profile)?), rest),
        _ => (None, args),
    };
    if paths.is_empty() {
        return Err(USAGE.into());
    }
    for path in paths {
        let path = Path::new(path);
        let changed = if path.extension().is_some_and(|e| e == "json") {
            convert_json(path)
        } else {
            convert_file(path, pinned.as_ref())
        }
        .map_err(|e| format!("{}: {e}", path.display()))?;
        if changed == 0 {
            println!("{}: already current", path.display());
        } else {
            println!(
                "{}: converted {changed} values (original kept as {})",
                path.display(),
                backup(path).display()
            );
        }
    }
    Ok(())
}

fn backup(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".pre-nm");
    PathBuf::from(name)
}

/// The `profile` of a new-format profile file.
fn read_profile(path: &str) -> Result<Value> {
    let file: Value = serde_json::from_slice(&fs::read(path)?)?;
    let profile = file
        .get("profile")
        .filter(|p| file["format"] == 1 && p.get("source").is_some() && p.get("rules").is_some())
        .ok_or_else(|| format!("{path} is not a format 1 rule profile file with a source"))?;
    Ok(profile.clone())
}

/// Converts an old profile library file in place.
fn convert_json(path: &Path) -> Result<usize> {
    let original = fs::read(path)?;
    let mut file: Value = serde_json::from_slice(&original)?;
    let changed = crate::convert_rules::convert_profile_file(&mut file)?;
    if changed > 0 {
        let mut text = String::new();
        pretty(&file, 0, &mut text)?;
        text.push('\n');
        replace(path, &original, text.as_bytes())?;
    }
    Ok(changed)
}

/// Keys a profile file writes first, in the order the app writes them
/// (`ProfileFile`, `ProfileData`, `ProfileSource`, `DrillFiles`,
/// `EntryData`); other keys follow sorted.
const PROFILE_KEY_ORDER: &[&str] = &[
    "format",
    "profile",
    "id",
    "name",
    "publisher",
    "document",
    "url",
    "date",
    "notes",
    "kind",
    "pth_allowance",
    "value",
    "formula",
    "source",
    "drill_files",
    "rules",
];

/// `value` as `serde_json::to_string_pretty` writes it, but with object
/// keys in [`PROFILE_KEY_ORDER`], so a converted profile file keeps the
/// app's own layout.
fn pretty(value: &Value, depth: usize, out: &mut String) -> Result<()> {
    let pad = |n: usize| "  ".repeat(n);
    match value {
        Value::Object(map) if !map.is_empty() => {
            let rank = |k: &str| {
                PROFILE_KEY_ORDER
                    .iter()
                    .position(|o| *o == k)
                    .unwrap_or(PROFILE_KEY_ORDER.len())
            };
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
            out.push_str("{\n");
            for (i, key) in keys.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                out.push_str(&serde_json::to_string(key)?);
                out.push_str(": ");
                pretty(&map[key.as_str()], depth + 1, out)?;
                out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push('}');
        }
        Value::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                pretty(item, depth + 1, out)?;
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other)?),
    }
    Ok(())
}

/// Converts one container in place; the number of values changed.
fn convert_file(path: &Path, pinned: Option<&Value>) -> Result<usize> {
    let original = fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&original[..]))?;
    let mut document = Vec::new();
    archive
        .by_name(DOCUMENT)?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut document)?;
    if document.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(format!("{DOCUMENT} exceeds {MAX_DOCUMENT_BYTES} bytes").into());
    }
    let mut file: Value = serde_json::from_slice(&document)?;
    if file.get("version") != Some(&Value::from(1)) {
        return Err("not a version 1 Pcbcraft document".into());
    }
    let project = file
        .get_mut("project")
        .and_then(Value::as_object_mut)
        .ok_or("the document has no project")?;
    let changed =
        convert_project(project)? + crate::convert_rules::convert_project_rules(project, pinned)?;
    if changed == 0 {
        return Ok(0);
    }
    let document = serde_json::to_vec(&file)?;

    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        if entry.name()? == DOCUMENT {
            drop(entry);
            out.start_file(DOCUMENT, options)?;
            out.write_all(&document)?;
        } else {
            out.raw_copy_file(entry)?;
        }
    }
    let bytes = out.finish()?.into_inner();
    replace(path, &original, &bytes)?;
    Ok(changed)
}

/// Replace `path` with `bytes` atomically, after keeping `original` as
/// its backup (never overwritten).
fn replace(path: &Path, original: &[u8], bytes: &[u8]) -> Result<()> {
    let backup = backup(path);
    if !backup.exists() {
        fs::write(&backup, original)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".convert-tmp");
    let temporary = PathBuf::from(temporary);
    {
        let mut f = fs::File::create(&temporary)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Rewrites the schematic's stored lengths; the number of values changed.
pub fn convert_project(project: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for occurrence in items(project, "occurrences") {
        n += point(occurrence, "position")?;
    }
    for junction in items(project, "junctions") {
        n += point(junction, "position")?;
    }
    for label in items(project, "labels") {
        n += point(label, "position")?;
    }
    for wire in items(project, "wires") {
        for end in ["from", "to"] {
            if let Some(end) = wire.get_mut(end).and_then(Value::as_object_mut) {
                n += point(end, "free")?;
            }
        }
        if let Some(corners) = wire.get_mut("corners").and_then(Value::as_array_mut) {
            for corner in corners.iter_mut().filter_map(Value::as_object_mut) {
                n += point(corner, "at")?;
                n += length(corner, "radius", "radius_nm", true)?;
            }
        }
    }
    for symbol in items(project, "sheet_symbols") {
        n += point(symbol, "position")?;
        n += point(symbol, "size")?;
        if let Some(pins) = symbol.get_mut("pins").and_then(Value::as_array_mut) {
            for pin in pins.iter_mut().filter_map(Value::as_object_mut) {
                n += length(pin, "offset", "offset_nm", false)?;
            }
        }
    }
    for drawing in items(project, "drawings") {
        let Some(item) = drawing.get_mut("item").and_then(Value::as_object_mut) else {
            continue;
        };
        match item.get("item").and_then(Value::as_str) {
            Some("text") => {
                n += point(item, "at")?;
                n += length(item, "height", "height_nm", false)?;
            }
            Some("line") => {
                n += point(item, "from")?;
                n += point(item, "to")?;
            }
            Some("rect") => {
                n += point(item, "min")?;
                n += point(item, "max")?;
            }
            other => return Err(format!("unknown drawing item {other:?}").into()),
        }
    }
    Ok(n)
}

fn items<'a>(
    project: &'a mut Map<String, Value>,
    key: &str,
) -> impl Iterator<Item = &'a mut Map<String, Value>> {
    project
        .get_mut(key)
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
}

/// Millimetres to whole nanometres, as `Nm::from_mm_f64_rounded` does.
fn nm(mm: &Value) -> Result<i64> {
    let mm = mm.as_f64().ok_or_else(|| format!("{mm} is not a number"))?;
    let nm = (mm * 1_000_000.0).round();
    if !nm.is_finite() || nm.abs() > MAX_COORD_NM {
        return Err(format!("{mm} mm is outside ±2 m").into());
    }
    // Integral and within ±2e9: the conversion is exact.
    Ok(nm as i64)
}

/// `[x, y]` mm at `key` to `{"x_nm", "y_nm"}`; 1 if it changed.
fn point(object: &mut Map<String, Value>, key: &str) -> Result<usize> {
    let Some(value) = object.get_mut(key) else {
        return Ok(0);
    };
    let Some([x, y]) = value.as_array().map(Vec::as_slice) else {
        return Ok(0);
    };
    let (x, y) = (nm(x)?, nm(y)?);
    *value = serde_json::json!({"x_nm": x, "y_nm": y});
    Ok(1)
}

/// A mm length at `from` to nanometres at `to`; 1 if it changed. With
/// `omit_zero`, a zero length is left out, as the app writes it.
fn length(object: &mut Map<String, Value>, from: &str, to: &str, omit_zero: bool) -> Result<usize> {
    let Some(value) = object.remove(from) else {
        return Ok(0);
    };
    let nm = nm(&value)?;
    if !(omit_zero && nm == 0) {
        object.insert(to.into(), Value::from(nm));
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_schematic_lengths_once() -> Result<()> {
        let mut project = json!({
            "occurrences": [{"position": [12.7, -5.08]}],
            "junctions": [{"position": [0.1, 0.2]}],
            "labels": [{"position": [1.0, 2.0]}],
            "wires": [{
                "from": {"free": [3.3, 4.4]},
                "to": {"junction": 7},
                "corners": [{"at": [1.5, 2.5], "radius": 0.635}, {"at": [0.0, 0.0], "radius": 0.0}]
            }],
            "sheet_symbols": [{"position": [10.0, 20.0], "size": [30.0, 15.24], "pins": [{"offset": 2.54}]}],
            "drawings": [
                {"item": {"item": "text", "at": [1.0, 1.0], "height": 2.5}},
                {"item": {"item": "line", "from": [0.0, 0.0], "to": [1.0, 0.0]}},
                {"item": {"item": "rect", "min": [0.0, 0.0], "max": [1.0, 1.0]}}
            ],
            "placements": [{"position": [1.25, 2.5]}]
        });
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project(map)?, 17);
        assert_eq!(
            project["occurrences"][0]["position"],
            json!({"x_nm": 12_700_000, "y_nm": -5_080_000})
        );
        assert_eq!(
            project["wires"][0]["from"]["free"]["y_nm"],
            json!(4_400_000)
        );
        assert_eq!(project["wires"][0]["to"], json!({"junction": 7}));
        assert_eq!(
            project["wires"][0]["corners"][0]["radius_nm"],
            json!(635_000)
        );
        assert_eq!(
            project["wires"][0]["corners"][1],
            json!({"at": {"x_nm": 0, "y_nm": 0}})
        );
        assert_eq!(
            project["sheet_symbols"][0]["pins"][0],
            json!({"offset_nm": 2_540_000})
        );
        assert_eq!(
            project["drawings"][0]["item"]["height_nm"],
            json!(2_500_000)
        );
        // Board data is a later slice's.
        assert_eq!(project["placements"][0]["position"], json!([1.25, 2.5]));
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project(map)?, 0, "a second run changes nothing");
        Ok(())
    }

    #[test]
    fn refuses_out_of_range_lengths() {
        let mut project = json!({"labels": [{"position": [2001.0, 0.0]}]});
        let result = project
            .as_object_mut()
            .ok_or_else(|| "object".into())
            .and_then(convert_project);
        assert!(result.is_err());
    }
}
