//! Read-only preview of a library-folder item (slice A2).
//!
//! `library.open_preview` (automation) and the Component workspace's
//! catalog read a folder bundle through io and inspect it here, so a folder
//! item is shown with the same [`ItemReport`] `library.inspect` gives a
//! project item, without importing anything. "Edit a copy in this project"
//! is `library.import` plus a draft (F4); nothing here changes a document.
//!
//! The bundle is read like a source catalog reads a folder
//! ([`local_library`]): its records are put in bundle-local ids, so the
//! report's definition ids (`entry.lineage`, symbol, binding and footprint
//! ids) are local to this bundle. The folder identity is
//! `entry.source = Library(..)` with `entry.source_lineage`. `latest` and
//! `revisions` speak about the revisions the bundle holds; a bundle holds
//! the revision it was read for and its closure, so the folder may hold
//! newer ones (its catalog entry says which).
use super::{ItemReport, Source, inspect};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionKind, Revision, SourceLineageId};
use crate::library::bundle::{local_library, validate_bundle};
use crate::library::{Bundle, Drafts};
use crate::model::Project;

/// Inspect a folder bundle's root, or another component or Part lineage it
/// holds (`lineage`), at `revision` (the root's revision for the root, else
/// the newest the bundle holds). A Part is shown as the component revision
/// it implements, as the Place palette shows it. Refuses a bundle that does
/// not validate, a lineage it does not hold and any other kind.
pub fn inspect_bundle(
    bundle: &Bundle,
    lineage: Option<SourceLineageId>,
    revision: Option<Revision>,
) -> Result<ItemReport> {
    validate_bundle(bundle)?;
    let local = local_library(&bundle.records)?;
    if let Some(skipped) = local.skipped.first() {
        return Err(Error::Invalid(format!(
            "{} holds an unreadable record: lineage {} {}",
            bundle.name, skipped.lineage, skipped.revision
        )));
    }
    let wanted = lineage.unwrap_or(bundle.root.lineage);
    let revision = match revision {
        Some(revision) => Some(revision),
        None if wanted == bundle.root.lineage => Some(bundle.root.revision),
        None => None,
    };
    let local_of = |source: SourceLineageId| {
        local
            .lineages
            .iter()
            .find(|l| l.lineage == source)
            .map(|l| l.local)
    };
    let source_of = |id| {
        local
            .lineages
            .iter()
            .find(|l| l.local == id)
            .map(|l| l.lineage)
    };
    let id = local_of(wanted)
        .ok_or_else(|| Error::NotFound(format!("{} holds no lineage {wanted}", bundle.name)))?;
    let kind = bundle
        .records
        .iter()
        .find(|r| r.lineage == wanted)
        .map(|r| r.kind);
    let library = local.library;
    let (component, revision) = match kind {
        Some(DefinitionKind::Component) => (id, revision),
        Some(DefinitionKind::Part) => {
            let part = match revision {
                Some(revision) => library
                    .parts
                    .iter()
                    .find(|p| p.id == id && p.revision == revision),
                None => library
                    .parts
                    .iter()
                    .filter(|p| p.id == id)
                    .max_by_key(|p| p.revision),
            }
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "{} holds no such revision of Part lineage {wanted}",
                    bundle.name
                ))
            })?;
            let implements = part.definition.component;
            (implements.id, Some(implements.revision))
        }
        _ => {
            return Err(Error::Invalid(format!(
                "lineage {wanted} of {} is not a component or a Part; only those are previewed",
                bundle.name
            )));
        }
    };
    let project = Project {
        library,
        ..Project::default()
    };
    let mut report = inspect(&project, &Drafts::default(), component, revision)?;
    report.entry.source = Source::Library(bundle.library);
    report.entry.source_lineage = source_of(component);
    report.entry.draft = false;
    report.entry.used = 0;
    // The MPN badge, as a source catalog sets it: the bundle holds a Part
    // of this component.
    report.entry.badges.mpn = project
        .library
        .parts
        .iter()
        .any(|p| p.definition.component.id == component);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::synthetic;
    use crate::ids::{DefinitionRef, LibraryId};
    use crate::library::bundle::SourceRevision;
    use crate::library::import;

    type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    const LIBRARY: LibraryId = LibraryId(0xa2);

    /// The promote bundle of the first synthetic component and its Part.
    fn bundle_of(kind: DefinitionKind) -> Fallible<(Project, Bundle, DefinitionRef)> {
        let project = synthetic::project_with_parts(4)?;
        let root = match kind {
            DefinitionKind::Part => {
                let part = project.library.latest_parts()[0];
                DefinitionRef {
                    kind,
                    id: part.id,
                    revision: part.revision,
                }
            }
            _ => {
                let component = project.library.latest_components()[0];
                DefinitionRef {
                    kind: DefinitionKind::Component,
                    id: component.id,
                    revision: component.revision,
                }
            }
        };
        // Distinct, non-zero fresh source lineages for the promote.
        let fresh: Vec<SourceLineageId> = (1..=64u128)
            .map(|n| SourceLineageId(n.wrapping_mul(0x9e37_79b9_7f4a_7c15) | (1 << 100)))
            .collect();
        let plan = import::export(&project.library, root, LIBRARY, "Sample", &fresh)?;
        Ok((project, plan.bundle, root))
    }

    #[test]
    fn a_folder_component_reports_what_the_project_copy_reports() -> Fallible<()> {
        let (project, bundle, root) = bundle_of(DefinitionKind::Component)?;
        let preview = inspect_bundle(&bundle, None, None)?;
        let project_report = inspect(&project, &Drafts::default(), root.id, None)?;
        assert_eq!(preview.entry.name, project_report.entry.name);
        assert_eq!(preview.entry.source, Source::Library(LIBRARY));
        assert_eq!(preview.entry.source_lineage, Some(bundle.root.lineage));
        assert_eq!(preview.entry.revision, Some(bundle.root.revision));
        assert_eq!(preview.terminals, project_report.terminals);
        assert_eq!(preview.provenance, project_report.provenance);
        let names = |r: &ItemReport| {
            r.implementations
                .iter()
                .map(|i| (i.package_name.clone(), i.envelope, i.footprints.len()))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&preview), names(&project_report));
        let units = |r: &ItemReport| {
            r.symbols
                .iter()
                .map(|s| s.units.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(units(&preview), units(&project_report));
        assert!(preview.latest);
        assert_eq!(preview.entry.used, 0);
        Ok(())
    }

    #[test]
    fn a_folder_part_previews_the_component_it_implements() -> Fallible<()> {
        let (project, bundle, root) = bundle_of(DefinitionKind::Part)?;
        let preview = inspect_bundle(&bundle, None, None)?;
        let part = project
            .library
            .part(crate::ids::PartRef::new(root.id, root.revision))
            .ok_or("part")?;
        let component = project
            .library
            .component(part.definition.component)
            .ok_or("component")?;
        assert_eq!(preview.entry.name, component.meta.name);
        assert_eq!(preview.entry.kind, DefinitionKind::Component);
        assert!(preview.entry.badges.mpn);
        assert_ne!(preview.entry.source_lineage, Some(bundle.root.lineage));
        Ok(())
    }

    #[test]
    fn a_revision_or_lineage_the_bundle_lacks_is_refused() -> Fallible<()> {
        let (_, bundle, _) = bundle_of(DefinitionKind::Component)?;
        let other = inspect_bundle(&bundle, None, Some(Revision(99)));
        assert!(matches!(other, Err(Error::NotFound(_))), "{other:?}");
        let missing = inspect_bundle(&bundle, Some(SourceLineageId(u128::MAX)), None);
        assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");
        // A package lineage of the closure is not an item to preview.
        let package = bundle
            .records
            .iter()
            .find(|r| r.kind == DefinitionKind::Package)
            .ok_or("package")?;
        let wrong = inspect_bundle(&bundle, Some(package.lineage), None);
        assert!(matches!(wrong, Err(Error::Invalid(_))), "{wrong:?}");
        Ok(())
    }

    #[test]
    fn a_bundle_that_does_not_validate_is_refused() -> Fallible<()> {
        let (_, mut bundle, _) = bundle_of(DefinitionKind::Component)?;
        // A root the records do not hold.
        bundle.root = SourceRevision {
            lineage: bundle.root.lineage,
            revision: Revision(bundle.root.revision.0 + 7),
        };
        assert!(inspect_bundle(&bundle, None, None).is_err());
        Ok(())
    }
}
