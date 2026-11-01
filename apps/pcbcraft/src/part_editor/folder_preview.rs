//! Slice A1: the read-only preview of a library-folder item (decision F4)
//! and its "Edit a copy in this project".
//!
//! A preview never touches the open project. The folder bundle is read on
//! a worker and imported, with drafts of its closure, into a scratch,
//! in-memory engine; the Component workspace then shows that scratch
//! engine's working set with every edit refused. "Edit a copy" applies
//! `library.import` of the same bundle to the project and drafts the
//! imported revision with public commands; the app records those steps as
//! one undo step.
use super::working_set::{self, WorkingSet};
use crate::library_sources::FolderPick;
use pcbcraft_core::library::Bundle;
use pcbcraft_core::{
    ComponentRef, DefinitionId, DefinitionKind, Engine, LibraryCommand, Operation, Outcome, PartRef,
};
use pcbcraft_io::library_folder::{self as folder, Folder};
use std::sync::mpsc::{self, Receiver};

/// A folder item shown read-only.
pub struct Preview {
    /// The scratch engine the working set lives in; never the project's.
    pub engine: Engine,
    pub pick: FolderPick,
    /// The bundle read, kept for Edit a copy and Place in schematic.
    pub bundle: Bundle,
    /// The component lineage in the scratch engine.
    pub root: DefinitionId,
}

/// What a finished preview read reports.
pub type Built = (FolderPick, Result<Box<Preview>, String>);

/// Import `bundle` into a scratch engine and draft its root component, so
/// the workspace can show it with its usual views.
pub fn build(pick: FolderPick, bundle: Bundle) -> Result<Preview, String> {
    let mut engine = Engine::default();
    let component = import(&mut engine, bundle.clone())?;
    working_set::draft_closure(&mut engine, component).map_err(|e| e.to_string())?;
    if WorkingSet::of(engine.drafts(), component.id).is_none() {
        return Err(format!(
            "{} has no symbol, package and footprint to show",
            pick.name
        ));
    }
    Ok(Preview {
        engine,
        pick,
        bundle,
        root: component.id,
    })
}

/// `library.import` of `bundle` into `engine`; the component revision it
/// holds (a Part root names its component).
fn import(engine: &mut Engine, bundle: Bundle) -> Result<ComponentRef, String> {
    let outcome = engine
        .apply_with(
            engine.revision(),
            Operation::Library(LibraryCommand::Import {
                bundle: Box::new(bundle),
            }),
            &pcbcraft_solidcraft::footprint::SketchEvaluator,
        )
        .map_err(|e| e.to_string())?;
    let Outcome::Imported { report } = outcome else {
        return Err("library.import returned an unexpected outcome".into());
    };
    let root = report.root;
    match root.kind {
        DefinitionKind::Component => Ok(ComponentRef::new(root.id, root.revision)),
        DefinitionKind::Part => engine
            .project()
            .library
            .part(PartRef::new(root.id, root.revision))
            .map(|part| part.definition.component)
            .ok_or_else(|| "the imported Part is missing".into()),
        _ => Err("only components and Parts open in the Component workspace".into()),
    }
}

/// What Edit a copy did to the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Copied {
    /// The project's component lineage, now drafted.
    pub lineage: DefinitionId,
    /// How many engine steps it took (0 when the project already held the
    /// revision and its drafts); the app undoes them as one.
    pub steps: usize,
}

/// "Edit a copy in this project": `library.import` (deduplicated by
/// origin), then drafts of the imported revision. A failure undoes the
/// steps already taken, so the project is unchanged.
pub fn edit_copy(engine: &mut Engine, bundle: Bundle) -> Result<Copied, String> {
    let start = engine.revision();
    let result = import(engine, bundle).and_then(|component| {
        working_set::draft_closure(engine, component)
            .map(|_| component.id)
            .map_err(|e| e.to_string())
    });
    // Every committed command advances the revision by one.
    let steps = usize::try_from(engine.revision().saturating_sub(start)).unwrap_or(usize::MAX);
    match result {
        Ok(lineage) => Ok(Copied { lineage, steps }),
        Err(error) => {
            for _ in 0..steps {
                if engine.undo().is_err() {
                    break;
                }
            }
            Err(error)
        }
    }
}

/// Read `pick`'s bundle and build its preview on a worker; the result
/// arrives on the returned channel and repaints `ctx`.
pub fn spawn(folder: Folder, pick: FolderPick, ctx: egui::Context) -> Receiver<Built> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = folder::read_bundle(
            &folder,
            pcbcraft_core::library::bundle::SourceRevision {
                lineage: pick.lineage,
                revision: pick.revision,
            },
        )
        .map_err(|e| format!("Reading {} failed: {e}", pick.name))
        .and_then(|bundle| build(pick.clone(), bundle).map(Box::new));
        let _ = sender.send((pick, result));
        ctx.request_repaint();
    });
    receiver
}

/// Tests: the bundle of the demo's first component, as the library
/// folder Sample Parts (library 7) holds it.
#[cfg(test)]
pub fn sample_item() -> Result<(FolderPick, Bundle), Box<dyn std::error::Error>> {
    let author = Engine::demo()?;
    let component = author
        .project()
        .library
        .latest_components()
        .first()
        .map(|c| ComponentRef::new(c.id, c.revision))
        .ok_or("a component")?;
    let name = author
        .project()
        .library
        .component(component)
        .map(|c| c.meta.name.clone())
        .ok_or("a name")?;
    let pcbcraft_core::QueryResult::Promote(plan) =
        author.query(pcbcraft_core::Query::PromotePreview {
            root: component.erased(),
            library: pcbcraft_core::ids::LibraryId(7),
            name: "Sample Parts".into(),
            fresh: (1..65).map(pcbcraft_core::ids::SourceLineageId).collect(),
        })?
    else {
        return Err("not a promote plan".into());
    };
    let root = plan.bundle.root;
    let pick = FolderPick {
        library: pcbcraft_core::ids::LibraryId(7),
        lineage: root.lineage,
        revision: root.revision,
        name,
    };
    Ok((pick, plan.bundle))
}
