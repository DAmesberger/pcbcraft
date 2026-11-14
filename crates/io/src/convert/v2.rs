//! Step 2 to 3: generated parts and part-file values (part-generation
//! slices B1–B4, `docs/design/part-generation-plan.md`).
//!
//! - **Part-file values** become integers: `{"mm": 3.1}` becomes
//!   `{"nm": 3100000}`, `{"deg": 45}` `{"mdeg": 45000}` and
//!   `{"number": 0.5}` `{"milli": 500}`, rounded half away from zero as the
//!   app's one crossing does; `{"count": n}` stays. Every map of values in
//!   the document is converted (packages' part files, mechanical parts,
//!   drafts).
//! - **Generated parts are detached.** A version 2 record names a module
//!   of the generator ABI before the one plugin ABI (renamed exports, no
//!   capabilities, part key by module hash), which this build no longer
//!   runs, so the part could never pass its check on open. Its revisions
//!   stay as ordinary parts, as a draft of a generated part is; the records
//!   and the embedded modules nothing else uses are removed. Making the
//!   part again with the shipped generator gives it a record of version 3.
use crate::convert::{Options, Result};
use serde_json::{Map, Value};

const MODULE_MEDIA_TYPE: &str = "application/wasm";

/// Step 2 to 3 of a document; the number of stored values changed.
pub fn step(file: &mut Value, _options: &Options) -> Result<usize> {
    let mut n = values(file)?;
    if let Some(library) = file
        .get_mut("project")
        .and_then(|p| p.get_mut("library"))
        .and_then(Value::as_object_mut)
    {
        n += detach(library);
    }
    Ok(n)
}

/// What the step will say about `file` (a version 2 document) beyond
/// counts: the generated parts it detaches.
pub fn notes(file: &Value) -> Vec<String> {
    let parts = file
        .pointer("/project/library/generated")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if parts == 0 {
        return Vec::new();
    }
    vec![format!(
        "{parts} generated part(s) were detached: their revisions stay as ordinary parts, but a version 2 generator module can no longer be run, so they no longer claim to be generated; make them again with the shipped generator to link them"
    )]
}

/// Remove the generated-part records and the modules only they used.
fn detach(library: &mut Map<String, Value>) -> usize {
    let Some(Value::Array(records)) = library.remove("generated") else {
        return 0;
    };
    let modules: Vec<Value> = records
        .iter()
        .filter_map(|r| r.pointer("/from/module").cloned())
        .collect();
    let mut n = records.len();
    if let Some(Value::Array(assets)) = library.get_mut("assets") {
        let before = assets.len();
        assets.retain(|a| {
            let module = a.get("media_type").and_then(Value::as_str) == Some(MODULE_MEDIA_TYPE);
            !(module && a.get("hash").is_some_and(|h| modules.contains(h)))
        });
        n += before - assets.len();
    }
    n
}

/// Convert every map of part-file values under `value`.
fn values(value: &mut Value) -> Result<usize> {
    let mut n = 0;
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(values)) = map.get_mut("values")
                && is_part_values(values)
            {
                for (name, v) in values.iter_mut() {
                    n += part_value(name, v)?;
                }
            }
            for (key, child) in map.iter_mut() {
                if key != "values" {
                    n += values(child)?;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                n += values(item)?;
            }
        }
        _ => {}
    }
    Ok(n)
}

/// Whether `map` is a map of part-file values: every entry one of the
/// four forms of version 2, at least one of them with a number to convert.
fn is_part_values(map: &Map<String, Value>) -> bool {
    let form = |v: &Value| {
        v.as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.keys().next().cloned())
    };
    !map.is_empty()
        && map.values().all(|v| {
            form(v).is_some_and(|k| matches!(k.as_str(), "mm" | "deg" | "number" | "count"))
        })
        && map.values().any(|v| form(v).is_some_and(|k| k != "count"))
}

fn part_value(name: &str, value: &mut Value) -> Result<usize> {
    let Some(object) = value.as_object() else {
        return Ok(0);
    };
    let Some((form, number)) = object.iter().next() else {
        return Ok(0);
    };
    let (key, scale) = match form.as_str() {
        "mm" => ("nm", 1_000_000.0),
        "deg" => ("mdeg", 1000.0),
        "number" => ("milli", 1000.0),
        _ => return Ok(0),
    };
    let v = number
        .as_f64()
        .ok_or_else(|| format!("the part-file value {name} is not a number"))?;
    let scaled = (v * scale).round();
    if !scaled.is_finite() || scaled.abs() > 1e16 {
        return Err(format!("the part-file value {name} is out of range").into());
    }
    let mut converted = Map::new();
    // Finite and bounded by the check above.
    converted.insert(key.into(), Value::from(scaled as i64));
    *value = Value::Object(converted);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_become_integers_and_generated_records_are_detached() -> Result<()> {
        let mut file = json!({
            "version": 2,
            "project": {"library": {
                "packages": [{"definition": {"model": {"source": "part_file", "model": {"id": 4, "revision": 1},
                    "values": {"height": {"mm": 1.6}, "angle": {"deg": 12.5}, "ratio": {"number": 0.5}, "rows": {"count": 3}},
                    "derived": {}}}}],
                "generated": [{"root": {}, "expansion": [], "from": {"module": "aa"}}],
                "assets": [
                    {"hash": "aa", "media_type": "application/wasm", "data": ""},
                    {"hash": "bb", "media_type": "application/x-solidcraft", "data": ""}
                ]
            }},
            "drafts": {"items": [{"values": {"label": "not part values"}}]}
        });
        assert_eq!(notes(&file).len(), 1);
        let n = step(&mut file, &Options::default())?;
        assert_eq!(n, 3 + 1 + 1);
        assert_eq!(
            file.pointer("/project/library/packages/0/definition/model/values"),
            Some(
                &json!({"height": {"nm": 1600000}, "angle": {"mdeg": 12500}, "ratio": {"milli": 500}, "rows": {"count": 3}})
            )
        );
        assert_eq!(file.pointer("/project/library/generated"), None);
        assert_eq!(
            file.pointer("/project/library/assets"),
            Some(&json!([{"hash": "bb", "media_type": "application/x-solidcraft", "data": ""}]))
        );
        assert_eq!(
            file.pointer("/drafts/items/0/values"),
            Some(&json!({"label": "not part values"}))
        );
        Ok(())
    }
}
