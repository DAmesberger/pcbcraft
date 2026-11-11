//! A JSON-lines replay (`*.jsonl`, one request per line, as
//! `pcbcraft-cli stdio` reads it). Requests carry lengths on the wire as
//! millimetres (`units::Length`) and need nothing, but a request can carry a
//! whole definition in its stored form (`library.new_draft`, `library.edit`:
//! `{"kind", "body"}`), which is converted as in a file. Lines that are not
//! JSON objects are copied unchanged.
use super::library;
use crate::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Definition kinds a tagged body may name.
const KINDS: &[&str] = &[
    "component",
    "symbol",
    "package",
    "footprint",
    "binding",
    "quantity_kind",
    "field",
    "part",
    "model",
    "package_row",
];

/// Every stored definition inside `value`; the number of values changed.
fn definitions(value: &mut Value) -> Result<usize> {
    match value {
        Value::Object(object) => {
            let tagged = object.len() == 2
                && object.contains_key("body")
                && object
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_some_and(|k| KINDS.contains(&k));
            if tagged {
                return library::tagged(value);
            }
            let mut n = 0;
            for child in object.values_mut() {
                n += definitions(child)?;
            }
            Ok(n)
        }
        Value::Array(items) => {
            let mut n = 0;
            for item in items {
                n += definitions(item)?;
            }
            Ok(n)
        }
        _ => Ok(0),
    }
}

/// Converts a replay in place, keeping the original at `backup`; the
/// number of values changed.
pub fn convert(path: &Path, backup: &Path) -> Result<usize> {
    let original = fs::read_to_string(path)?;
    let mut out = String::with_capacity(original.len());
    let mut n = 0;
    for line in original.lines() {
        match serde_json::from_str::<Value>(line) {
            Ok(mut request) if request.is_object() => {
                let changed = definitions(&mut request)?;
                if changed == 0 {
                    out.push_str(line);
                } else {
                    n += changed;
                    out.push_str(&serde_json::to_string(&request)?);
                }
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    if n == 0 {
        return Ok(0);
    }
    if !backup.exists() {
        fs::write(backup, &original)?;
    }
    pcbcraft_io::persistence::atomic_write(path, out.as_bytes())?;
    Ok(n)
}
