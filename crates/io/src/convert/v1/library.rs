//! Where library definitions live in each file, and which slice converts
//! what inside them. A definition body is the same JSON in a project's
//! library (one array per kind, bodies untagged), in its drafts and in a
//! library folder's revision records (both `{"kind", "body"}`), so each
//! domain converts a body once, by kind, wherever it is.
//!
//! U2 (library geometry) domains, one module each:
//! - [`symbols`]: the `Shape` vocabulary, symbol graphics and pins, footprint
//!   graphics, and the authoring grid;
//! - [`footprints`]: pads, holes, mask and paste rules, the datum, pad
//!   tables' results and mechanical lands;
//! - [`forms`]: package forms, envelopes and package rows;
//! - [`marking`]: marking specs and their placement;
//! - [`models`]: 3D model definitions, model placement and lead fits.
use super::{footprints, forms, marking, models, symbols};
use crate::convert::Result;
use serde_json::{Map, Value};

/// The library arrays of a project and the kind each holds.
const KINDS: &[(&str, &str)] = &[
    ("components", "component"),
    ("symbols", "symbol"),
    ("packages", "package"),
    ("footprints", "footprint"),
    ("bindings", "binding"),
    ("quantity_kinds", "quantity_kind"),
    ("fields", "field"),
    ("parts", "part"),
    ("models", "model"),
    ("package_rows", "package_row"),
];

/// One definition body of `kind`; the number of values changed.
pub fn definition(kind: &str, body: &mut Value) -> Result<usize> {
    let Some(body) = body.as_object_mut() else {
        return Ok(0);
    };
    Ok(symbols::definition(kind, body)?
        + footprints::definition(kind, body)?
        + forms::definition(kind, body)?
        + marking::definition(kind, body)?
        + models::definition(kind, body)?)
}

/// A tagged definition, `{"kind", "body"}`, as drafts and records hold it.
pub fn tagged(definition: &mut Value) -> Result<usize> {
    let Some(kind) = definition
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Err("a definition has no kind".into());
    };
    match definition.get_mut("body") {
        Some(body) => self::definition(&kind, body),
        None => Ok(0),
    }
}

/// Every published revision of a project's library.
pub fn library(library: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for (key, kind) in KINDS {
        for published in super::json::items(library, key) {
            if let Some(body) = published.get_mut("definition") {
                n += definition(kind, body)?;
            }
        }
    }
    Ok(n)
}

/// A project file's drafts and the authoring settings saved with them.
pub fn drafts(drafts: &mut Map<String, Value>) -> Result<usize> {
    let mut n = 0;
    for draft in super::json::items(drafts, "items") {
        if let Some(definition) = draft.get_mut("definition") {
            n += tagged(definition)?;
        }
    }
    Ok(n + symbols::drafts(drafts)?
        + footprints::drafts(drafts)?
        + forms::drafts(drafts)?
        + marking::drafts(drafts)?
        + models::drafts(drafts)?)
}

/// Library data a project holds outside its library (an instance's text
/// placements, a marking on an instance).
pub fn project(project: &mut Map<String, Value>) -> Result<usize> {
    Ok(symbols::project(project)?
        + footprints::project(project)?
        + forms::project(project)?
        + marking::project(project)?
        + models::project(project)?)
}

/// One revision record of a library folder.
pub fn record(record: &mut Map<String, Value>) -> Result<usize> {
    match record.get_mut("definition") {
        Some(definition) => tagged(definition),
        None => Err("a revision record has no definition".into()),
    }
}
