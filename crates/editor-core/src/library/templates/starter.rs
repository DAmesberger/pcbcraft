//! The starter package-row tables (`starter/*.json`): original data typed
//! from public package outline drawings (JEDEC registered outlines, EIA
//! case sizes and manufacturers' published package drawings). A row either
//! cites the drawing it was typed from or is marked estimated with a note
//! saying why. No row comes from an EDA library, generator table or
//! footprint file.
//!
//! The tables are compiled in as a **built-in, read-only source**: the
//! template catalog (`library.templates`) always lists them, so the
//! gallery is populated on a fresh install. A component built from one
//! records the row's name and the table's format in its provenance
//! (`RowSource::Starter`). `pcbcraft-cli sample-packages` still writes the
//! same rows to a library folder for an organisation to replace or extend.
use crate::diagnostics::{Error, Result};
use crate::ids::MAX_LABEL;
use crate::library::MAX_NOTE;
use crate::library::package_form::{PackageForm, PackageFormParams};
use crate::library::part::{Basis, Provenance};
use crate::library::symbol_style::SymbolKind;
use crate::library::templates::catalog::{
    FamilyKind, FamilyRef, MAX_ALIAS, MAX_PACKAGE_ROWS_PER_SOURCE, MAX_ROW_ALIASES,
    PackageRowDefinition, RowCitation, SizeCode, SkeletonHint,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The starter table format this reader accepts. Pre-release: one format,
/// no legacy readers.
pub const FORMAT: u64 = 1;

/// The starter rows' licence: original data, licensed as Pcbcraft.
pub const LICENCE: &str = "MIT OR Apache-2.0";

/// Bytes of one starter table (a bound on the parser's input).
pub const MAX_TABLE_BYTES: usize = 1024 * 1024;

/// The built-in source's display name.
pub const SOURCE_NAME: &str = "Pcbcraft starter rows";

/// The tables, one per family file.
pub const TABLES: [(&str, &str); 9] = [
    ("chip.json", include_str!("starter/chip.json")),
    ("gull_wing.json", include_str!("starter/gull_wing.json")),
    ("no_lead.json", include_str!("starter/no_lead.json")),
    ("header.json", include_str!("starter/header.json")),
    ("tabbed.json", include_str!("starter/tabbed.json")),
    ("molded.json", include_str!("starter/molded.json")),
    ("grid.json", include_str!("starter/grid.json")),
    ("tht.json", include_str!("starter/tht.json")),
    ("usb_c.json", include_str!("starter/usb_c.json")),
];

/// One parsed starter row.
#[derive(Clone, Debug, PartialEq)]
pub struct StarterRow {
    pub file: &'static str,
    /// Unique across the tables: the row's identity in `RowSource::Starter`.
    pub name: String,
    pub description: String,
    /// Why the row is estimated, or what to check (an exposed pad's size).
    pub note: Option<String>,
    pub definition: PackageRowDefinition,
}

impl StarterRow {
    /// `cited` (the citation records revision and page),
    /// `cited_unverified` (it names only the document) or `estimated`.
    pub fn status(&self) -> &'static str {
        match &self.definition.citation {
            Some(c) if c.checked() => "cited",
            Some(_) => "cited_unverified",
            None => "estimated",
        }
    }

    /// The definition's free-text provenance note.
    pub fn provenance_note(&self) -> String {
        let basis = match &self.definition.citation {
            Some(c) => format!(
                "Pcbcraft starter table (original data, {LICENCE}). Values typed from {}{}{}{}.",
                c.document,
                c.document_revision
                    .as_ref()
                    .map_or(String::new(), |r| format!(", revision {r}")),
                c.page.map_or(String::new(), |p| format!(", page {p}")),
                if c.checked() {
                    ""
                } else {
                    " (citation unverified: no drawing revision or page recorded)"
                },
            ),
            None => format!("Pcbcraft starter table (original data, {LICENCE}). Estimated values."),
        };
        match &self.note {
            Some(note) => format!("{basis} {note}"),
            None => basis,
        }
    }
}

fn invalid(message: String) -> Error {
    Error::Invalid(format!("starter: {message}"))
}

fn text(object: &Map<String, Value>, key: &str, context: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{context}: `{key}` must be text")))
}

fn optional_text(object: &Map<String, Value>, key: &str, context: &str) -> Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(format!("{context}: `{key}` must be text or null"))),
    }
}

/// Refuse keys this format does not define, so nothing is silently dropped.
fn only_keys(object: &Map<String, Value>, allowed: &[&str], context: &str) -> Result<()> {
    match object.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(invalid(format!("{context}: unknown key `{key}`"))),
        None => Ok(()),
    }
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{context}: expected an object")))
}

fn typed<T: serde::de::DeserializeOwned>(value: Option<&Value>, what: &str) -> Result<T> {
    serde_json::from_value(value.cloned().unwrap_or_default())
        .map_err(|e| invalid(format!("{what}: {e}")))
}

/// The serde name of a family (`gull_wing`).
fn family_name(family: FamilyKind) -> String {
    match serde_json::to_value(family) {
        Ok(Value::String(name)) => name,
        _ => family.label().to_owned(),
    }
}

/// Parse one table: its header, citations and rows.
pub fn parse_table(file: &'static str, source: &str) -> Result<Vec<StarterRow>> {
    if source.len() > MAX_TABLE_BYTES {
        return Err(Error::Limit(format!(
            "starter: {file}: larger than {MAX_TABLE_BYTES} bytes"
        )));
    }
    let root: Value = serde_json::from_str(source).map_err(|e| invalid(format!("{file}: {e}")))?;
    let root = object(&root, file)?;
    only_keys(
        root,
        &["format", "family", "family_rev", "citations", "rows"],
        file,
    )?;
    if root.get("format").and_then(Value::as_u64) != Some(FORMAT) {
        return Err(invalid(format!(
            "{file}: unsupported starter table format (this reader is format {FORMAT})"
        )));
    }
    let family: FamilyKind = typed(root.get("family"), &format!("{file}: family"))?;
    let family_rev = root
        .get("family_rev")
        .and_then(Value::as_u64)
        .and_then(|r| u16::try_from(r).ok())
        .filter(|r| *r >= 1)
        .ok_or_else(|| invalid(format!("{file}: `family_rev` must be 1–65535")))?;
    let mut citations = BTreeMap::new();
    if let Some(list) = root.get("citations") {
        for (key, value) in object(list, file)? {
            let citation: RowCitation = typed(Some(value), &format!("{file}: citation `{key}`"))?;
            if citation.document.trim().is_empty() || citation.document.len() > MAX_LABEL {
                return Err(invalid(format!(
                    "{file}: citation `{key}` needs a document"
                )));
            }
            citations.insert(key.clone(), citation);
        }
    }
    let rows = root
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("{file}: `rows` must be a list")))?;
    let family_text = family_name(family);
    let mut used = BTreeSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for (index, value) in rows.iter().enumerate() {
        let context = format!("{file} row {}", index + 1);
        let row = object(value, &context)?;
        only_keys(
            row,
            &[
                "name",
                "description",
                "aliases",
                "size_code",
                "skeleton",
                "symbol",
                "cite",
                "note",
                "values",
                "lands",
            ],
            &context,
        )?;
        let name = text(row, "name", &context)?;
        let context = format!("{file} `{name}`");
        if name.trim().is_empty() || name.len() > MAX_LABEL {
            return Err(invalid(format!(
                "{context}: the name must be 1–{MAX_LABEL} bytes"
            )));
        }
        let description = text(row, "description", &context)?;
        let note = optional_text(row, "note", &context)?;
        let citation = match optional_text(row, "cite", &context)? {
            Some(key) => {
                used.insert(key.clone());
                Some(
                    citations
                        .get(&key)
                        .cloned()
                        .ok_or_else(|| invalid(format!("{context}: no citation `{key}`")))?,
                )
            }
            None => None,
        };
        if citation.is_none() && note.as_deref().is_none_or(|n| n.trim().is_empty()) {
            return Err(invalid(format!(
                "{context}: an estimated row needs a note saying why"
            )));
        }
        let aliases: Vec<String> = typed(
            Some(row.get("aliases").unwrap_or(&json!([]))),
            &format!("{context}: aliases"),
        )?;
        if aliases.len() > MAX_ROW_ALIASES
            || aliases.iter().any(|a| a.is_empty() || a.len() > MAX_ALIAS)
        {
            return Err(invalid(format!(
                "{context}: at most {MAX_ROW_ALIASES} aliases of 1–{MAX_ALIAS} bytes"
            )));
        }
        // The tables are hand-edited millimetres: the form's wire form.
        let values: PackageForm =
            typed::<PackageFormParams>(row.get("values"), &format!("{context}: values"))?.into();
        if FamilyKind::of(&values) != Some(family) {
            return Err(invalid(format!(
                "{context}: the values are not of the {family_text} family"
            )));
        }
        let size_code: Option<SizeCode> =
            typed(row.get("size_code"), &format!("{context}: size_code"))?;
        let skeleton: SkeletonHint = typed(row.get("skeleton"), &format!("{context}: skeleton"))?;
        let symbol: Option<SymbolKind> = typed(row.get("symbol"), &format!("{context}: symbol"))?;
        // Hand-edited millimetres, as the values.
        let lands = typed::<Option<super::catalog::LandValuesParams>>(
            row.get("lands"),
            &format!("{context}: lands"),
        )?
        .map(super::catalog::LandValues::from);
        let problems = values.range_problems();
        if !problems.is_empty() {
            return Err(invalid(format!("{context}: {}", problems.join(" "))));
        }
        if !values.within_bounds() {
            return Err(Error::Limit(format!("starter: {context}: too many leads")));
        }
        let definition = PackageRowDefinition {
            family: Some(FamilyRef { family, family_rev }),
            values,
            aliases,
            size_code,
            lands,
            skeleton,
            symbol,
            provenance: Provenance {
                origin: None,
                basis: Basis::Authored,
                licence: Some(LICENCE.into()),
            },
            citation,
        };
        let row = StarterRow {
            file,
            name,
            description,
            note,
            definition,
        };
        if row.description.len() > MAX_NOTE || row.provenance_note().len() > MAX_NOTE {
            return Err(invalid(format!(
                "{context}: description or provenance is too long"
            )));
        }
        out.push(row);
    }
    if let Some(unused) = citations.keys().find(|k| !used.contains(*k)) {
        return Err(invalid(format!(
            "{file}: citation `{unused}` is not used by any row"
        )));
    }
    Ok(out)
}

/// Every starter row, parsed and checked across tables: names are unique
/// and the total fits one library source.
pub fn parse_all() -> Result<Vec<StarterRow>> {
    let mut rows = Vec::new();
    for (file, source) in TABLES {
        rows.extend(parse_table(file, source)?);
    }
    if rows.len() > MAX_PACKAGE_ROWS_PER_SOURCE {
        return Err(Error::Limit(format!(
            "starter: more than {MAX_PACKAGE_ROWS_PER_SOURCE} rows"
        )));
    }
    let mut names = BTreeSet::new();
    if let Some(dup) = rows.iter().find(|r| !names.insert(r.name.as_str())) {
        return Err(invalid(format!("two rows are named `{}`", dup.name)));
    }
    Ok(rows)
}

/// The built-in rows, parsed once. The tables are compiled in and a test
/// parses them, so an error here is a build defect; it is kept (not
/// panicked on) and reported by [`row`].
pub fn rows() -> std::result::Result<&'static [StarterRow], &'static str> {
    static ROWS: OnceLock<std::result::Result<Vec<StarterRow>, String>> = OnceLock::new();
    match ROWS.get_or_init(|| parse_all().map_err(|e| e.to_string())) {
        Ok(rows) => Ok(rows),
        Err(error) => Err(error),
    }
}

/// The built-in row named `name`.
pub fn row(name: &str) -> Result<&'static StarterRow> {
    let rows = rows().map_err(|e| Error::Invalid(e.to_owned()))?;
    rows.iter().find(|r| r.name == name).ok_or_else(|| {
        Error::NotFound(format!(
            "template.row_missing: the built-in starter tables hold no row ‘{name}’"
        ))
    })
}

#[cfg(test)]
mod tests;
