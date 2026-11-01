//! Slice A1: the workspace opens one subject explicitly, names it in the
//! identity bar, walks its history, and shows library-folder items
//! read-only until Edit a copy.
use super::folder_preview;
use super::identity::Subject;
use super::*;
use pcbcraft_core::ids::LibraryId;
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Texts one 900 × 600 canvas frame drew, the identity bar first.
fn frame(editor: &mut PartEditor, engine: &Engine, ctx: &egui::Context) -> Vec<String> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 600.0))),
            ..Default::default()
        },
        |ui| {
            editor.identity_strip(ui, engine);
            editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        texts(&shape.shape, &mut out);
    }
    out
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

/// A New request from the editor, carried out as the app does.
fn create(
    editor: &mut PartEditor,
    engine: &mut Engine,
    ctx: &egui::Context,
    template: Template,
) -> TestResult {
    editor.create_new(template);
    let request = editor.take_request().ok_or("a request")?;
    run(engine, request)?;
    editor.commit(ctx, engine);
    Ok(())
}

/// A project with two component drafts opens the start page, not either.
#[test]
fn a_project_with_drafts_opens_the_start_page() -> TestResult {
    let ctx = context();
    let mut engine = Engine::default();
    let mut author = PartEditor::default();
    create(&mut author, &mut engine, &ctx, Template::TwoTerminal)?;
    create(&mut author, &mut engine, &ctx, Template::Header)?;
    let components = engine
        .drafts()
        .items
        .iter()
        .filter(|d| d.definition.kind() == pcbcraft_core::DefinitionKind::Component)
        .count();
    assert_eq!(components, 2);
    let mut editor = PartEditor::default();
    editor.commit(&ctx, &engine);
    assert!(editor.draft.is_none() && editor.lineage().is_none());
    let shown = frame(&mut editor, &engine, &ctx);
    assert!(shown.iter().any(|t| t == "Open a component"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "No component open"), "{shown:?}");
    // Both drafts are listed, not shown.
    assert!(shown.iter().any(|t| t == "Resistor"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "Connector"), "{shown:?}");
    Ok(())
}

/// New opens the component it created; the bar names it and its state,
/// and publishing changes the state chip.
#[test]
fn the_bar_names_the_subject_and_its_state() -> TestResult {
    let ctx = context();
    let mut engine = Engine::default();
    let mut editor = PartEditor::default();
    editor.commit(&ctx, &engine);
    create(&mut editor, &mut engine, &ctx, Template::TwoTerminal)?;
    let lineage = editor.lineage().ok_or("the new component is open")?;
    assert_eq!(
        editor.draft.as_ref().map(|s| s.ids.component),
        Some(lineage)
    );
    assert_eq!(
        editor.identity_text(&engine),
        Some(("Project › Resistor".into(), "new · never published".into()))
    );
    let shown = frame(&mut editor, &engine, &ctx);
    assert!(shown.iter().any(|t| t == "Project › Resistor"), "{shown:?}");
    assert!(
        shown.iter().any(|t| t == "new · never published"),
        "{shown:?}"
    );
    let publish = editor.on_tool("publish", &engine).ok_or("publish")?;
    engine.apply(engine.revision(), publish)?;
    editor.commit(&ctx, &engine);
    assert_eq!(
        editor.identity_text(&engine).map(|(_, state)| state),
        Some("published r1".into())
    );
    if let Some(set) = &mut editor.draft {
        set.metas.component.name = "Resistor 0603".into();
    }
    let edit = editor.commit(&ctx, &engine).ok_or("the rename commits")?;
    engine.apply(engine.revision(), edit)?;
    editor.commit(&ctx, &engine);
    assert_eq!(
        editor.identity_text(&engine),
        Some(("Project › Resistor 0603".into(), "r1 → draft".into()))
    );
    Ok(())
}

/// The ribbon's Component group is New, Open and Publish.
#[test]
fn the_component_group_is_new_open_and_publish() -> TestResult {
    let ctx = context();
    let mut engine = Engine::default();
    let mut editor = PartEditor::default();
    create(&mut editor, &mut engine, &ctx, Template::TwoTerminal)?;
    for tab in ["details", "symbol", "footprint", "mapping", "model"] {
        editor.on_tab(tab);
        let ribbon = editor.ribbon(&engine);
        let group = ribbon
            .groups
            .iter()
            .find(|g| g.label == "Component")
            .ok_or("a Component group")?;
        let labels: Vec<_> = group.tools.iter().map(|t| t.label).collect();
        assert_eq!(labels, ["New", "Open", "Publish"], "{tab}");
    }
    // New opens the template gallery, Open the start page; nothing is
    // edited.
    let before = engine.revision();
    assert!(editor.on_tool("component-new", &engine).is_none());
    assert_eq!(
        editor.gallery.scope(),
        Some(&crate::template_gallery::Scope::NewComponent)
    );
    assert!(editor.on_tool("component-open", &engine).is_none());
    assert!(editor.lineage().is_none() && editor.draft.is_none());
    assert_eq!(engine.revision(), before);
    Ok(())
}

/// Back and forward walk the opened subjects; the start page returns to
/// the one left.
#[test]
fn back_and_forward_walk_the_open_history() -> TestResult {
    let ctx = context();
    let mut engine = Engine::default();
    let mut editor = PartEditor::default();
    create(&mut editor, &mut engine, &ctx, Template::TwoTerminal)?;
    let first = editor.lineage().ok_or("first")?;
    editor.show_start();
    create(&mut editor, &mut engine, &ctx, Template::Header)?;
    let second = editor.lineage().ok_or("second")?;
    assert_ne!(first, second);
    editor.go_back(&engine);
    editor.commit(&ctx, &engine);
    assert_eq!(editor.lineage(), Some(first));
    assert_eq!(editor.draft.as_ref().map(|s| s.ids.component), Some(first));
    editor.go_forward(&engine);
    editor.commit(&ctx, &engine);
    assert_eq!(editor.lineage(), Some(second));
    editor.show_start();
    assert_eq!(editor.lineage(), None);
    editor.go_back(&engine);
    assert_eq!(editor.lineage(), Some(second), "back from the start page");
    // Recents list the newest first.
    assert_eq!(
        editor.identity.recents()[..2],
        [Subject::Project(second), Subject::Project(first)]
    );
    Ok(())
}

use folder_preview::sample_item as folder_item;

/// A folder item opens read-only: the bar says so, the project stays
/// unchanged and nothing the views do reaches it. Edit a copy imports and
/// drafts it, and the copy's origin chip names the folder.
#[test]
fn a_folder_item_opens_read_only_and_edit_a_copy_drafts_it() -> TestResult {
    let ctx = context();
    let (pick, bundle) = folder_item()?;
    let mut project = Engine::default();
    let kept = (project.project().clone(), project.drafts().clone());
    let mut editor = PartEditor::default();
    editor.set_sources(
        vec![(LibraryId(7), "Sample Parts".into(), false)],
        1,
        Default::default,
    );
    editor.open_folder(pick.clone());
    assert_eq!(
        editor.take_folder_actions(),
        vec![identity::FolderAction::Preview(pick.clone())]
    );
    let preview = folder_preview::build(pick.clone(), bundle.clone())?;
    editor.show_preview(Box::new(preview));
    assert!(editor.commit(&ctx, &project).is_none());
    assert!(editor.is_read_only());
    assert_eq!(editor.lineage(), None, "no project lineage is open");
    let name = editor
        .draft
        .as_ref()
        .map(|s| s.metas.component.name.clone());
    assert_eq!(name.as_deref(), Some(pick.name.as_str()));
    let shown = frame(&mut editor, &project, &ctx);
    let state = format!("read-only · Sample Parts {}", pick.revision);
    assert!(shown.contains(&state), "{shown:?}");
    assert!(
        shown.iter().any(|t| t == "Edit a copy in this project"),
        "{shown:?}"
    );
    // Edits are refused or dropped; the project never changes.
    assert!(editor.on_tool("terminal", &project).is_none());
    if let Some(set) = &mut editor.draft {
        set.metas.component.name = "Changed".into();
    }
    assert!(editor.commit(&ctx, &project).is_none());
    assert_eq!(
        editor
            .draft
            .as_ref()
            .map(|s| s.metas.component.name.clone()),
        name
    );
    let labels: Vec<_> = editor
        .ribbon(&project)
        .groups
        .iter()
        .flat_map(|g| g.tools.iter().map(|t| (t.label, t.enabled)))
        .collect();
    assert_eq!(
        labels,
        [
            ("New", true),
            ("Open", true),
            ("Edit a copy", true),
            ("Publish", false)
        ]
    );
    assert_eq!((project.project().clone(), project.drafts().clone()), kept);
    // Edit a copy: the app imports and drafts the bundle, then opens it.
    assert!(editor.on_tool("component-edit-copy", &project).is_none());
    assert_eq!(
        editor.take_folder_actions(),
        vec![identity::FolderAction::EditCopy]
    );
    let copied = edit_copy(&mut project, editor.preview_bundle().ok_or("bundle")?)?;
    assert!(copied.changed, "an import and its drafts");
    editor.open(copied.lineage, &project);
    editor.commit(&ctx, &project);
    assert!(!editor.is_read_only());
    assert_eq!(editor.lineage(), Some(copied.lineage));
    assert_eq!(
        editor.identity_text(&project),
        Some((
            format!("Project › {}", pick.name),
            format!("published {}", pcbcraft_core::Revision::FIRST)
        ))
    );
    assert_eq!(
        editor.origin_text(&project),
        Some(format!("from Sample Parts {}", pick.revision))
    );
    // One undo (one `library.edit_copy`) restores the project exactly.
    project.undo()?;
    assert_eq!((project.project().clone(), project.drafts().clone()), kept);
    Ok(())
}

/// A preview lent for the 3D view comes back only under its own subject:
/// after a subject change it is dropped, never shown for another item.
#[test]
fn a_lent_preview_is_restored_only_under_its_own_subject() -> TestResult {
    let (pick, bundle) = folder_item()?;
    let mut editor = PartEditor::default();
    editor.set_sources(
        vec![(LibraryId(7), "Sample Parts".into(), false)],
        1,
        Default::default,
    );
    editor.open_folder(pick.clone());
    editor.show_preview(Box::new(folder_preview::build(
        pick.clone(),
        bundle.clone(),
    )?));
    let lent = editor.lend_preview();
    assert!(lent.is_some());
    editor.restore_preview(lent);
    assert!(editor.preview_bundle().is_some(), "same subject: restored");
    let lent = editor.lend_preview();
    editor.show_start();
    let mut other = pick.clone();
    other.revision = pcbcraft_core::Revision(other.revision.0 + 1);
    editor.open_folder(other);
    editor.restore_preview(lent);
    assert!(
        editor.preview_bundle().is_none(),
        "another subject: dropped"
    );
    Ok(())
}
