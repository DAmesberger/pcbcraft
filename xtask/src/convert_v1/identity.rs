//! Identity after a format change. Revision content hashes are SHA-256 of
//! a record's serde encoding, so converting a revision changes its hash
//! while its identity — lineage, revision number and every pin to it —
//! stays. A project remembers the hash of each revision it imported from or
//! promoted to a library ([`Origin::content_hash`]), and the hash of the
//! model design each 3D dimension reference was traced on
//! ([`ModelTrace::design`]). Both are recomputed here from the converted
//! project, exactly as import and tracing compute them, so a converted
//! project still recognises its revisions in a converted library folder
//! (no "edited after import" report, imports dedupe, updates are offered)
//! and its dimension references stay exact.
//!
//! Lineages imported from another project file are named by a hash of
//! their first revision in that file (`import::project_lineage_id`), which
//! includes that file's own id; the copy cannot recompute it, so it keeps
//! its old name and a later import of the converted file brings the
//! lineage in again under its new name. The run lists those lineages.
use crate::Result;
use pcbcraft_core::format::EditorFile;
use pcbcraft_core::ids::{DefinitionKind, DefinitionRef, LibraryId};
use pcbcraft_core::layout::dimensions::model::package_design_key;
use pcbcraft_core::library::bundle::{OriginRecord, SourceRevision, content_hash, to_record};
use pcbcraft_core::library::import::PROJECT_FILES;

/// What [`refresh`] changed, and what it could not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Origin hashes recomputed.
    pub origins: usize,
    /// Model-trace design keys recomputed.
    pub design_keys: usize,
    /// Revisions from other project files whose lineage names cannot be
    /// recomputed, described.
    pub project_file_lineages: Vec<String>,
}

impl Refreshed {
    pub fn changed(&self) -> usize {
        self.origins + self.design_keys
    }
}

/// Recompute the hashes a converted project remembers.
pub fn refresh(file: &mut EditorFile) -> Result<Refreshed> {
    let mut out = Refreshed::default();
    let origins: Vec<OriginRecord> = file.project.library.origins.to_vec();
    let mut next = origins.clone();
    for (i, record) in origins.iter().enumerate() {
        let hash = origin_hash(file, &origins, record)?;
        if hash != record.origin.content_hash {
            next[i].origin.content_hash = hash;
            out.origins += 1;
        }
        if record.origin.library == PROJECT_FILES {
            out.project_file_lineages.push(
                file.project
                    .library
                    .describe_lineage(record.local.kind, record.local.id),
            );
        }
    }
    if out.origins > 0 {
        *file.project.library.origins = next;
    }

    let traces = file.project.model_traces.to_vec();
    let mut next = traces.clone();
    for (i, trace) in traces.iter().enumerate() {
        let (Some(package), Some(design)) = (trace.package, trace.design) else {
            continue;
        };
        // A package the project no longer holds, or a design that cannot be
        // traced any more, keeps its key: the trace reports it as it did.
        let Ok(key) = package_design_key(&file.project, package) else {
            continue;
        };
        if key != design {
            next[i].design = Some(key);
            out.design_keys += 1;
        }
    }
    if out.design_keys > 0 {
        *file.project.model_traces = next;
    }
    out.project_file_lineages.sort();
    out.project_file_lineages.dedup();
    Ok(out)
}

/// The content hash of the source record a project revision was copied
/// from (or promoted as): the project's copy written back as a record of
/// its library, every reference mapped through that library's origins, as
/// `library::import::project_bundle` and promotion write records.
fn origin_hash(
    file: &EditorFile,
    origins: &[OriginRecord],
    record: &OriginRecord,
) -> Result<pcbcraft_core::ids::ContentHash> {
    let library = &file.project.library;
    let source = record.origin.library;
    let (meta, body) = library
        .definition(record.local)
        .ok_or_else(|| format!("{} is not in the library", library.describe(record.local)))?;
    let of = |source: LibraryId| origins.iter().filter(move |o| o.origin.library == source);
    let converted = to_record(
        record.origin.lineage,
        record.origin.revision,
        meta,
        body,
        &mut |r: DefinitionRef| {
            of(source)
                .find(|o| o.local == r)
                .map(|o| SourceRevision {
                    lineage: o.origin.lineage,
                    revision: o.origin.revision,
                })
                .ok_or_else(|| {
                    pcbcraft_core::Error::NotFound(format!(
                        "{} names {}, which came from no revision of its library",
                        library.describe(record.local),
                        library.describe(r)
                    ))
                })
        },
        &mut |kind: DefinitionKind, id| {
            of(source)
                .find(|o| o.local.kind == kind && o.local.id == id)
                .map(|o| o.origin.lineage)
                .ok_or_else(|| {
                    pcbcraft_core::Error::NotFound(format!(
                        "{} names {}, which came from no lineage of its library",
                        library.describe(record.local),
                        library.describe_lineage(kind, id)
                    ))
                })
        },
    )?;
    Ok(content_hash(&converted)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::ids::{ContentHash, DefinitionId};
    use pcbcraft_core::library::import;

    /// Every Part of the synthetic catalog imported into an empty project,
    /// with the origins import recorded.
    fn imported() -> Result<EditorFile> {
        let source = pcbcraft_core::catalog::synthetic::project_with_parts(4)?.library;
        let mut file = EditorFile {
            version: pcbcraft_core::format::FILE_VERSION,
            project: Default::default(),
            drafts: Default::default(),
        };
        let mut next = 1000;
        for part in source.latest_parts() {
            let root = DefinitionRef {
                kind: DefinitionKind::Part,
                id: part.id,
                revision: part.revision,
            };
            let bundle = import::project_bundle(&source, root, "source")?;
            import::import(&mut file.project.library, &bundle, &mut || {
                next += 1;
                Ok(DefinitionId(next))
            })?;
        }
        Ok(file)
    }

    #[test]
    fn origin_hashes_are_recomputed_as_import_records_them() -> Result<()> {
        let mut file = imported()?;
        let recorded = file.project.library.origins.to_vec();
        assert!(!recorded.is_empty());
        // Recomputing an up-to-date project changes nothing.
        assert_eq!(refresh(&mut file)?.changed(), 0);
        // Stale hashes, as a format change leaves them, come back exactly.
        for origin in file.project.library.origins.iter_mut() {
            origin.origin.content_hash = ContentHash([7; 32]);
        }
        let refreshed = refresh(&mut file)?;
        assert_eq!(refreshed.origins, recorded.len());
        assert!(!refreshed.project_file_lineages.is_empty());
        assert_eq!(*file.project.library.origins, recorded);
        Ok(())
    }
}
