//! Slice B3: the starter package-row tables' `sample-packages` writer and
//! the provenance review list. The tables themselves, their parser and
//! their checks live in the core (`library::templates::starter`), which
//! also lists them as the built-in, read-only source of the template
//! gallery; this command writes the same rows to a library folder so an
//! organisation can replace or extend them.
//!
//! The tables are original data typed from public package outline drawings
//! (JEDEC registered outlines, EIA case sizes and manufacturers' published
//! package drawings). A row either cites the drawing it was typed from or
//! is marked estimated with a note saying why. No row comes from an EDA
//! library, generator table or footprint file.
use super::{Result, execute, print};
use pcbcraft_automation::Session;
use pcbcraft_core::format::{EditorFile, FILE_VERSION};
use pcbcraft_core::library::templates::catalog::RowCitation;
use pcbcraft_core::library::templates::starter::{StarterRow, parse_all};
use pcbcraft_core::library::{DefinitionMeta, Library, Published};
use pcbcraft_core::{DefinitionId, DefinitionKind, DefinitionRef, Engine, Project, Revision};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The usage lines.
pub const USAGE: &str =
    "sample-packages NEW_DIR [NAME]   write the starter package-row library folder (about 190 rows)
  sample-packages --review         print the starter rows' provenance review list";

/// The library folder's default display name.
const DEFAULT_NAME: &str = "Pcbcraft Packages";

fn starter_rows() -> Result<Vec<StarterRow>> {
    Ok(parse_all()?)
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
        "cited_unverified": count("cited_unverified"),
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
    use std::collections::BTreeSet;

    type TestResult = Result<()>;

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
        let total: u64 = ["cited", "cited_unverified", "estimated", "held"]
            .iter()
            .filter_map(|k| list[*k].as_u64())
            .sum();
        assert_eq!(total, u64::try_from(rows.len())?);
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
        let engine = Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project,
            drafts: Default::default(),
        })?;
        assert_eq!(engine.project().library.package_rows.len(), publishable);
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
        let report = write(&path, DEFAULT_NAME, &rows)?;
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
