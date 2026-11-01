//! Slice B3: the starter package-row tables (`crates/editor-core/src/
//! library/templates/starter/*.json`), their `sample-packages` writer and
//! the provenance review list.
//!
//! The tables are original data typed from public package outline drawings
//! (JEDEC registered outlines, EIA case sizes and manufacturers' published
//! package drawings). A row either cites the drawing it was typed from or
//! is marked estimated with a note saying why. No row comes from an EDA
//! library, generator table or footprint file. They ship as a library
//! folder written by this command, never compiled into the app, so an
//! organisation can replace or extend them.
//!
//! A row whose family generator is not built yet (`TemplateKind::available`)
//! is **held**: it is parsed and listed for review but not published, so a
//! later family slice turns it on without editing this writer.
use super::{Result, execute, print};
use pcbcraft_automation::Session;
use pcbcraft_core::format::{EditorFile, FILE_VERSION};
use pcbcraft_core::library::templates::catalog::{
    FamilyKind, FamilyRef, MAX_ALIAS, MAX_PACKAGE_ROWS_PER_SOURCE, MAX_ROW_ALIASES, RowCitation,
    SizeCode, SkeletonHint,
};
use pcbcraft_core::library::templates::footprint_family::TemplateKind;
use pcbcraft_core::library::{
    Basis, DefinitionMeta, Library, MAX_LABEL, MAX_NOTE, PackageForm, PackageRowDefinition,
    Provenance, Published,
};
use pcbcraft_core::{DefinitionId, DefinitionKind, DefinitionRef, Engine, Project, Revision};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The usage lines.
pub const USAGE: &str = "sample-packages NEW_DIR [NAME]   write the starter package-row library folder (about 150 cited rows)
  sample-packages --review         print the starter rows' provenance review list";

/// The starter table format this reader accepts. Pre-release: one format,
/// no legacy readers.
const FORMAT: u64 = 1;

/// The library folder's default display name.
const DEFAULT_NAME: &str = "Pcbcraft Packages";

/// The starter rows' licence: original data, licensed as Pcbcraft.
const LICENCE: &str = "MIT OR Apache-2.0";

/// Bytes of one starter table (a bound on the parser's input).
const MAX_TABLE_BYTES: usize = 1024 * 1024;

/// The tables, one per family file.
const TABLES: [(&str, &str); 6] = [
    (
        "chip.json",
        include_str!("../../../crates/editor-core/src/library/templates/starter/chip.json"),
    ),
    (
        "gull_wing.json",
        include_str!("../../../crates/editor-core/src/library/templates/starter/gull_wing.json"),
    ),
    (
        "no_lead.json",
        include_str!("../../../crates/editor-core/src/library/templates/starter/no_lead.json"),
    ),
    (
        "header.json",
        include_str!("../../../crates/editor-core/src/library/templates/starter/header.json"),
    ),
    (
        "tabbed.json",
        include_str!("../../../crates/editor-core/src/library/templates/starter/tabbed.json"),
    ),
    (
        "molded.json",
        include_str!("../../../crates/editor-core/src/library/templates/starter/molded.json"),
    ),
];

/// One parsed starter row.
#[derive(Clone, Debug)]
struct StarterRow {
    file: &'static str,
    name: String,
    description: String,
    /// Why the row is estimated, or what to check (an exposed pad's size).
    note: Option<String>,
    definition: PackageRowDefinition,
    /// Why the row is not published yet: its family is not built.
    held: Option<String>,
}

impl StarterRow {
    fn status(&self) -> &'static str {
        if self.held.is_some() {
            "held"
        } else if self.definition.citation.is_some() {
            "cited"
        } else {
            "estimated"
        }
    }

    /// The definition's free-text provenance note.
    fn provenance_note(&self) -> String {
        let basis = match &self.definition.citation {
            Some(c) => format!(
                "Pcbcraft starter table (original data, {LICENCE}). Values typed from {}{}{}.",
                c.document,
                c.document_revision
                    .as_ref()
                    .map_or(String::new(), |r| format!(", revision {r}")),
                c.page.map_or(String::new(), |p| format!(", page {p}")),
            ),
            None => format!("Pcbcraft starter table (original data, {LICENCE}). Estimated values."),
        };
        match &self.note {
            Some(note) => format!("{basis} {note}"),
            None => basis,
        }
    }
}

fn text(object: &Map<String, Value>, key: &str, context: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{context}: `{key}` must be text").into())
}

fn optional_text(object: &Map<String, Value>, key: &str, context: &str) -> Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{context}: `{key}` must be text or null").into()),
    }
}

/// Refuse keys this format does not define, so nothing is silently dropped.
fn only_keys(object: &Map<String, Value>, allowed: &[&str], context: &str) -> Result<()> {
    match object.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(format!("{context}: unknown key `{key}`").into()),
        None => Ok(()),
    }
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context}: expected an object").into())
}

/// The serde name of a family (`gull_wing`).
fn family_name(family: FamilyKind) -> Result<String> {
    match serde_json::to_value(family)? {
        Value::String(name) => Ok(name),
        _ => Err("a family name is not text".into()),
    }
}

/// The footprint template that builds a family; its availability gates
/// publishing the family's rows.
fn template_of(family: FamilyKind) -> TemplateKind {
    match family {
        FamilyKind::Chip => TemplateKind::Chip,
        FamilyKind::GullWing => TemplateKind::GullWing,
        FamilyKind::NoLead => TemplateKind::Qfn,
        FamilyKind::Header => TemplateKind::PinHeader,
        FamilyKind::Tabbed => TemplateKind::Tabbed,
        FamilyKind::Molded => TemplateKind::Molded,
        FamilyKind::Grid => TemplateKind::Grid,
        FamilyKind::Tht => TemplateKind::Tht,
    }
}

/// Parse one table: its header, citations and rows.
fn parse_table(file: &'static str, source: &str) -> Result<Vec<StarterRow>> {
    if source.len() > MAX_TABLE_BYTES {
        return Err(format!("{file}: larger than {MAX_TABLE_BYTES} bytes").into());
    }
    let root: Value = serde_json::from_str(source).map_err(|e| format!("{file}: {e}"))?;
    let root = object(&root, file)?;
    only_keys(
        root,
        &[
            "format",
            "family",
            "family_rev",
            "family_note",
            "citations",
            "rows",
        ],
        file,
    )?;
    if root.get("format").and_then(Value::as_u64) != Some(FORMAT) {
        return Err(format!(
            "{file}: unsupported starter table format (this reader is format {FORMAT})"
        )
        .into());
    }
    let family: FamilyKind =
        serde_json::from_value(root.get("family").cloned().unwrap_or_default())
            .map_err(|e| format!("{file}: family: {e}"))?;
    let family_rev = root
        .get("family_rev")
        .and_then(Value::as_u64)
        .and_then(|r| u16::try_from(r).ok())
        .filter(|r| *r >= 1)
        .ok_or_else(|| format!("{file}: `family_rev` must be 1–65535"))?;
    let family_note = optional_text(root, "family_note", file)?;
    let mut citations = BTreeMap::new();
    if let Some(list) = root.get("citations") {
        for (key, value) in object(list, file)? {
            let citation: RowCitation = serde_json::from_value(value.clone())
                .map_err(|e| format!("{file}: citation `{key}`: {e}"))?;
            if citation.document.trim().is_empty() || citation.document.len() > MAX_NOTE {
                return Err(format!("{file}: citation `{key}` needs a document").into());
            }
            citations.insert(key.clone(), citation);
        }
    }
    let rows = root
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{file}: `rows` must be a list"))?;
    let built = template_of(family).available();
    let family_text = family_name(family)?;
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
            return Err(format!("{context}: the name must be 1–{MAX_LABEL} bytes").into());
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
                        .ok_or_else(|| format!("{context}: no citation `{key}`"))?,
                )
            }
            None => None,
        };
        if citation.is_none() && note.as_deref().is_none_or(|n| n.trim().is_empty()) {
            return Err(format!("{context}: an estimated row needs a note saying why").into());
        }
        let aliases: Vec<String> =
            serde_json::from_value(row.get("aliases").cloned().unwrap_or(json!([])))
                .map_err(|e| format!("{context}: aliases: {e}"))?;
        if aliases.len() > MAX_ROW_ALIASES
            || aliases.iter().any(|a| a.is_empty() || a.len() > MAX_ALIAS)
        {
            return Err(format!(
                "{context}: at most {MAX_ROW_ALIASES} aliases of 1–{MAX_ALIAS} bytes"
            )
            .into());
        }
        let values: PackageForm =
            serde_json::from_value(row.get("values").cloned().unwrap_or_default())
                .map_err(|e| format!("{context}: values: {e}"))?;
        if FamilyKind::of(&values) != Some(family) {
            return Err(
                format!("{context}: the values are not of the {family_text} family").into(),
            );
        }
        let size_code: Option<SizeCode> =
            serde_json::from_value(row.get("size_code").cloned().unwrap_or_default())
                .map_err(|e| format!("{context}: size_code: {e}"))?;
        let skeleton: SkeletonHint =
            serde_json::from_value(row.get("skeleton").cloned().unwrap_or_default())
                .map_err(|e| format!("{context}: skeleton: {e}"))?;
        let lands = serde_json::from_value(row.get("lands").cloned().unwrap_or_default())
            .map_err(|e| format!("{context}: lands: {e}"))?;
        let problems = values.range_problems();
        let held = if built {
            if !problems.is_empty() {
                return Err(format!("{context}: {}", problems.join(" ")).into());
            }
            None
        } else {
            Some(family_note.clone().unwrap_or_else(|| {
                format!("Held until the {family_text} family generator is built.")
            }))
        };
        if !values.within_bounds() {
            return Err(format!("{context}: too many leads").into());
        }
        let definition = PackageRowDefinition {
            family: Some(FamilyRef { family, family_rev }),
            values,
            aliases,
            size_code,
            lands,
            skeleton,
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
            held,
        };
        if row.description.len() > MAX_NOTE || row.provenance_note().len() > MAX_NOTE {
            return Err(format!("{context}: description or provenance is too long").into());
        }
        out.push(row);
    }
    if let Some(unused) = citations.keys().find(|k| !used.contains(*k)) {
        return Err(format!("{file}: citation `{unused}` is not used by any row").into());
    }
    Ok(out)
}

/// Every starter row, checked across tables: names are unique and the
/// total fits one library source.
fn starter_rows() -> Result<Vec<StarterRow>> {
    let mut rows = Vec::new();
    for (file, source) in TABLES {
        rows.extend(parse_table(file, source)?);
    }
    if rows.len() > MAX_PACKAGE_ROWS_PER_SOURCE {
        return Err(format!("more than {MAX_PACKAGE_ROWS_PER_SOURCE} starter rows").into());
    }
    let mut names = BTreeSet::new();
    if let Some(dup) = rows.iter().find(|r| !names.insert(r.name.as_str())) {
        return Err(format!("two starter rows are named `{}`", dup.name).into());
    }
    Ok(rows)
}

/// The provenance review list: what a reviewer checks before the rows are
/// trusted. Cited rows are grouped by document with the citation details
/// still missing; estimated and held rows are listed with their reasons.
fn review(rows: &[StarterRow]) -> Value {
    let mut by_document: BTreeMap<&str, (Vec<&str>, Vec<&str>, &RowCitation)> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.held.is_none()) {
        if let Some(c) = &row.definition.citation {
            let entry = by_document
                .entry(c.document.as_str())
                .or_insert_with(|| (Vec::new(), Vec::new(), c));
            entry.0.push(row.name.as_str());
            if let Some(note) = &row.note
                && !entry.1.contains(&note.as_str())
            {
                entry.1.push(note.as_str());
            }
        }
    }
    let sources: Vec<Value> = by_document
        .into_iter()
        .map(|(document, (names, notes, c))| {
            let mut missing = Vec::new();
            if c.document_revision.is_none() {
                missing.push("document_revision");
            }
            if c.page.is_none() {
                missing.push("page");
            }
            json!({
                "document": document,
                "document_revision": c.document_revision,
                "page": c.page,
                "missing": missing,
                "checks": notes,
                "rows": names,
            })
        })
        .collect();
    let listed = |status: &str| -> Vec<Value> {
        rows.iter()
            .filter(|r| r.status() == status)
            .map(|r| {
                json!({
                    "name": r.name,
                    "file": r.file,
                    "reason": r.held.as_ref().or(r.note.as_ref()),
                    "citation": r.definition.citation.as_ref().map(|c| &c.document),
                })
            })
            .collect()
    };
    let count = |status: &str| rows.iter().filter(|r| r.status() == status).count();
    json!({
        "rows": rows.len(),
        "cited": count("cited"),
        "estimated": count("estimated"),
        "held": count("held"),
        "rule": "Values are typed from public outline drawings (JEDEC, EIA, manufacturers' package drawings) or marked estimated; never from an EDA library. Confirm each cited row against its drawing and record the revision and page.",
        "cited_sources": sources,
        "estimated_rows": listed("estimated"),
        "held_rows": listed("held"),
    })
}

/// A project holding every publishable starter row as a first revision.
fn project(rows: &[StarterRow]) -> Result<(Project, Vec<DefinitionRef>)> {
    let mut library = Library::default();
    let mut roots = Vec::new();
    let mut next: u64 = 1;
    for row in rows.iter().filter(|r| r.held.is_none()) {
        let id = DefinitionId(next);
        next = next.checked_add(1).ok_or("identity space exhausted")?;
        library.package_rows.push(Published {
            id,
            revision: Revision::FIRST,
            meta: DefinitionMeta {
                name: row.name.clone(),
                description: row.description.clone(),
                provenance: row.provenance_note(),
            },
            definition: row.definition.clone(),
        });
        roots.push(DefinitionRef {
            kind: DefinitionKind::PackageRow,
            id,
            revision: Revision::FIRST,
        });
    }
    Ok((
        Project {
            name: DEFAULT_NAME.into(),
            next_id: next,
            library,
            ..Project::default()
        },
        roots,
    ))
}

/// Write the publishable rows to a new library folder through the same
/// requests the JSON transports use; the user's settings are not changed.
fn write(path: &str, name: &str, rows: &[StarterRow]) -> Result<Value> {
    if std::path::Path::new(path).try_exists()? {
        return Err("output exists; choose a new folder".into());
    }
    let (project, roots) = project(rows)?;
    let mut session = Session::new(Engine::from_file(EditorFile {
        version: FILE_VERSION,
        project,
        drafts: Default::default(),
    })?);
    let created = execute(
        &mut session,
        json!({"command": "library.source.create", "params": {"path": path, "name": name}}),
    )?;
    let library = created["created"]["library"].clone();
    let mut records = 0usize;
    for root in roots {
        let expected = session.engine().revision();
        let reply = execute(
            &mut session,
            json!({"command": "library.publish_to_folder", "params": {
                "expected_revision": expected,
                "root": root,
                "library": library,
            }}),
        )?;
        records += reply["written"]["records"].as_array().map_or(0, Vec::len);
    }
    Ok(json!({
        "library": library,
        "path": path,
        "rows_published": rows.iter().filter(|r| r.held.is_none()).count(),
        "records_written": records,
        "review": review(rows),
    }))
}

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    match args {
        [flag] if flag == "--review" => print(&review(&starter_rows()?)),
        [path, rest @ ..] if rest.len() <= 1 && !path.starts_with('-') => print(&write(
            path,
            rest.first().map_or(DEFAULT_NAME, String::as_str),
            &starter_rows()?,
        )?),
        _ => Err(format!("usage: pcbcraft-cli {USAGE}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::{ChipKind, LeadSides};

    type TestResult = Result<()>;

    fn find<'a>(rows: &'a [StarterRow], name: &str) -> Result<&'a StarterRow> {
        rows.iter()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("no row {name}").into())
    }

    /// Until slice B1 replaces `PackageRowDefinition::findings`, every row
    /// is refused at publish with `package_row.unsupported`; the writer's
    /// tests accept exactly that contract refusal and nothing else. Once
    /// B1 is merged this branch is dead and the full checks run.
    fn awaiting_row_publishing(error: &str) -> TestResult {
        if error.contains("Package template rows cannot be published yet") {
            eprintln!("starter rows await B1's row publishing: {error}");
            Ok(())
        } else {
            Err(error.to_owned().into())
        }
    }

    /// A fresh path under the system temp directory, removed on drop.
    struct TempPath(std::path::PathBuf);

    impl TempPath {
        fn new(tag: &str) -> Result<Self> {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            Ok(Self(std::env::temp_dir().join(format!(
                "pcbcraft-b3-{tag}-{}-{nanos}",
                std::process::id()
            ))))
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn about_150_rows_over_the_families_each_cited_or_estimated() -> TestResult {
        let rows = starter_rows()?;
        assert!((140..=200).contains(&rows.len()), "{} rows", rows.len());
        let mut per_family = BTreeMap::new();
        for row in &rows {
            let family = row.definition.family.map(|f| f.family);
            *per_family.entry(family).or_insert(0) += 1;
            match &row.definition.citation {
                Some(c) => assert!(!c.document.trim().is_empty(), "{}", row.name),
                None => assert!(
                    row.note
                        .as_deref()
                        .is_some_and(|n| n.starts_with("Estimated")),
                    "an uncited row says it is estimated: {}",
                    row.name
                ),
            }
            assert_eq!(row.definition.provenance.licence.as_deref(), Some(LICENCE));
            assert_eq!(row.definition.provenance.origin, None);
        }
        for family in [
            FamilyKind::Chip,
            FamilyKind::GullWing,
            FamilyKind::NoLead,
            FamilyKind::Header,
            FamilyKind::Tabbed,
            FamilyKind::Molded,
        ] {
            assert!(
                per_family.get(&Some(family)).copied().unwrap_or(0) >= 5,
                "{family:?}: {per_family:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn built_families_publish_and_later_families_are_held() -> TestResult {
        for row in starter_rows()? {
            let family = row.definition.family.map(|f| f.family);
            let built = family.is_some_and(|f| template_of(f).available());
            assert_eq!(row.held.is_none(), built, "{}", row.name);
            if built {
                assert_eq!(
                    row.definition.values.range_problems(),
                    Vec::<String>::new(),
                    "{}",
                    row.name
                );
            }
        }
        Ok(())
    }

    /// The name's pin count is the form's lead count, so a typo in a table
    /// shows up as a mismatch, not as a wrong footprint.
    #[test]
    fn names_agree_with_lead_counts() -> TestResult {
        let digits = |s: &str| -> Option<u64> {
            s.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        };
        let mut checked = 0;
        for row in starter_rows()?.iter().filter(|r| r.held.is_none()) {
            let leads = row.definition.values.lead_count();
            let name = &row.name;
            let family = row.definition.family.map(|f| f.family);
            // Outlines whose names carry no lead count: the registered
            // counts (a tab counts as a lead).
            let named = [("SOT-223", 4), ("DPAK (TO-252)", 3), ("D2PAK (TO-263)", 3)];
            if let Some((_, count)) = named.iter().find(|(n, _)| n == name) {
                assert_eq!(leads, *count, "{name}");
            } else if name.starts_with("Chip") || family == Some(FamilyKind::Molded) {
                assert_eq!(leads, 2, "{name}");
            } else if let Some(rest) = name.strip_prefix("SOT-23-") {
                assert_eq!(digits(rest), Some(leads), "{name}");
            } else if let Some((rows_text, rest)) = name
                .strip_prefix("Pin header ")
                .and_then(|s| s.split_once('×'))
            {
                let count = digits(rows_text).zip(digits(rest)).map(|(a, b)| a * b);
                assert_eq!(count, Some(leads), "{name}");
            } else {
                let rest = name.split_once('-').map(|(_, rest)| rest);
                assert_eq!(rest.and_then(digits), Some(leads), "{name}");
            }
            checked += 1;
        }
        assert!(checked >= 130);
        Ok(())
    }

    /// Chip rows carry both size codes explicitly, the metric code is the
    /// body's length and width in tenths of a millimetre, and the codes the
    /// two systems share (0402, 0603) are both listed.
    #[test]
    fn chip_size_codes_are_explicit_and_match_the_body() -> TestResult {
        let rows = starter_rows()?;
        let mut imperial = BTreeSet::new();
        let mut metric = BTreeSet::new();
        for row in &rows {
            let size = row.definition.size_code.as_ref();
            let PackageForm::Chip(chip) = &row.definition.values else {
                assert!(size.is_none(), "only chips have size codes: {}", row.name);
                continue;
            };
            let size = size.ok_or_else(|| format!("{} has no size code", row.name))?;
            imperial.insert(size.imperial.clone());
            metric.insert(size.metric.clone());
            // Metric codes name the nominal size; a drawing's nominal may
            // differ by a tenth (3.1 mm for a "32" resistor length).
            let tenths = |v: f64| (v * 10.0).round();
            let code = |i: usize| {
                size.metric
                    .get(2 * i..2 * i + 2)
                    .and_then(|t| t.parse::<f64>().ok())
            };
            let (l, w) = (code(0).ok_or("metric code")?, code(1).ok_or("metric code")?);
            assert!(
                size.metric.len() == 4
                    && (l - tenths(chip.length)).abs() <= 1.0
                    && (w - tenths(chip.width)).abs() <= 1.0,
                "{}",
                row.name
            );
            assert!(
                row.definition.aliases.contains(&size.imperial),
                "{}",
                row.name
            );
        }
        for shared in ["0402", "0603"] {
            assert!(
                imperial.contains(shared) && metric.contains(shared),
                "{shared}"
            );
        }
        let r0402 = find(&rows, "Chip resistor 0402 (1005 metric)")?;
        assert!(
            matches!(&r0402.definition.values, PackageForm::Chip(c) if c.kind == ChipKind::Resistor)
        );
        Ok(())
    }

    #[test]
    fn known_outlines_keep_their_registered_values() -> TestResult {
        let rows = starter_rows()?;
        let soic = find(&rows, "SOIC-8 3.9 mm")?;
        let PackageForm::GullWing(g) = &soic.definition.values else {
            return Err("SOIC-8 is gull-wing".into());
        };
        assert_eq!(
            (g.sides, g.leads_per_side, g.pitch),
            (LeadSides::Two, [4, 0], 1.27)
        );
        assert_eq!((g.body_length, g.body_width, g.span[0]), (4.90, 3.90, 6.00));
        assert!(
            soic.definition
                .citation
                .as_ref()
                .is_some_and(|c| c.document.contains("MS-012"))
        );
        let qfn = find(&rows, "QFN-32 5×5 0.5 EP 3.45")?;
        assert_eq!(qfn.definition.values.lead_count(), 32);
        assert_eq!(qfn.definition.values.terminal_counts(), vec![32, 33]);
        assert!(
            qfn.note
                .as_deref()
                .is_some_and(|n| n.contains("exposed pad"))
        );
        let lqfp = find(&rows, "LQFP-128 14×20 0.5")?;
        assert_eq!(lqfp.definition.values.envelope().map(|e| e[0]), Some(22.0));
        let header = find(&rows, "Pin header 2×5 1.27 mm")?;
        assert_eq!(header.status(), "estimated");
        assert_eq!(header.definition.skeleton, SkeletonHint::Connector);
        Ok(())
    }

    #[test]
    fn review_list_covers_every_row_once() -> TestResult {
        let rows = starter_rows()?;
        let list = review(&rows);
        let mut named = Vec::new();
        for source in list["cited_sources"].as_array().into_iter().flatten() {
            assert!(source["missing"].is_array() && source["document"].is_string());
            named.extend(source["rows"].as_array().into_iter().flatten().cloned());
        }
        for key in ["estimated_rows", "held_rows"] {
            for row in list[key].as_array().into_iter().flatten() {
                assert!(row["reason"].is_string(), "{row}");
                named.push(row["name"].clone());
            }
        }
        assert_eq!(named.len(), rows.len());
        let unique: BTreeSet<String> = named.iter().map(Value::to_string).collect();
        assert_eq!(unique.len(), rows.len());
        let total: u64 = ["cited", "estimated", "held"]
            .iter()
            .filter_map(|k| list[*k].as_u64())
            .sum();
        assert_eq!(total, u64::try_from(rows.len())?);
        Ok(())
    }

    #[test]
    fn malformed_tables_are_refused() -> TestResult {
        let base = json!({
            "format": 1, "family": "chip", "family_rev": 1,
            "citations": {"d": {"document": "A drawing", "document_revision": null, "page": null}},
            "rows": [{"name": "R", "description": "d", "aliases": [], "skeleton": {"skeleton": "blank"},
                      "cite": "d", "values": {"family": "chip", "kind": "resistor", "length": 1.0,
                      "width": 0.5, "height": 0.35, "termination": 0.25}}]
        });
        let parse = |v: &Value| parse_table("t.json", &v.to_string());
        assert_eq!(parse(&base)?.len(), 1);
        let refused = |edit: &dyn Fn(&mut Value), why: &str| {
            let mut v = base.clone();
            edit(&mut v);
            assert!(parse(&v).is_err(), "{why}");
        };
        refused(&|v| v["format"] = json!(2), "unsupported format");
        refused(&|v| v["rows"][0]["extra"] = json!(1), "unknown key");
        refused(
            &|v| v["rows"][0]["cite"] = json!("missing"),
            "unknown citation",
        );
        refused(
            &|v| {
                v["rows"][0]["cite"] = Value::Null;
                v["citations"] = json!({});
            },
            "estimated row without a note",
        );
        refused(
            &|v| v["rows"][0]["values"]["termination"] = json!(0.6),
            "range problem in a built family",
        );
        refused(
            &|v| v["family"] = json!("header"),
            "values of another family",
        );
        refused(
            &|v| v["rows"][0]["aliases"] = json!(["x".repeat(MAX_ALIAS + 1)]),
            "alias bound",
        );
        refused(&|v| v["rows"][0]["name"] = json!(""), "empty name");
        assert!(parse_table("t.json", &" ".repeat(MAX_TABLE_BYTES + 1)).is_err());
        Ok(())
    }

    #[test]
    fn the_project_holds_every_publishable_row_once() -> TestResult {
        let rows = starter_rows()?;
        let (project, roots) = project(&rows)?;
        let publishable = rows.iter().filter(|r| r.held.is_none()).count();
        assert_eq!(roots.len(), publishable);
        assert_eq!(project.library.package_rows.len(), publishable);
        assert!(project.next_id > u64::try_from(roots.len())?);
        match Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project,
            drafts: Default::default(),
        }) {
            Ok(engine) => assert_eq!(engine.project().library.package_rows.len(), publishable),
            Err(error) => awaiting_row_publishing(&error.to_string())?,
        }
        Ok(())
    }

    /// The writer publishes every row to a new folder through the session
    /// requests and refuses an existing path.
    #[test]
    fn the_writer_publishes_the_rows_to_a_new_folder() -> TestResult {
        let rows = starter_rows()?;
        assert!(write(&std::env::temp_dir().to_string_lossy(), DEFAULT_NAME, &rows).is_err());
        let temp = TempPath::new("write")?;
        let path = temp.0.to_string_lossy().into_owned();
        let report = match write(&path, DEFAULT_NAME, &rows) {
            Ok(report) => report,
            Err(error) => return awaiting_row_publishing(&error.to_string()),
        };
        let publishable = rows.iter().filter(|r| r.held.is_none()).count();
        assert_eq!(
            report["rows_published"].as_u64(),
            Some(u64::try_from(publishable)?)
        );
        assert_eq!(
            report["records_written"].as_u64(),
            Some(u64::try_from(publishable)?)
        );
        let lineages = std::fs::read_dir(temp.0.join("lineages"))?.count();
        assert_eq!(lineages, publishable);
        assert!(
            write(&path, DEFAULT_NAME, &rows).is_err(),
            "the folder now exists"
        );
        Ok(())
    }
}
