//! The Component 3D tab's placement gestures driven through real egui
//! frames over a detached viewport: a triad drag moves only the display
//! transform and sends one command on release, Esc cancels it, quick
//! actions and "Fit" are one command each, point-to-point moves a model
//! point onto a pad, and placement edits never change the preview's build
//! key (no rebuild). Original fixture: the Resistor template turned into a
//! typed chip whose terminations sit on its lands.
use super::model_place::PointToPoint;
use super::*;
use crate::scene::ModelKey;
use crate::viewport::Viewport;
use pcbcraft_core::geometry::Transform3;
use pcbcraft_core::library::{
    Axis, ChipForm, ChipKind, Definition, ModelPlacement, PackageEdit, PackageForm, QuickAction,
};
use solidcraft_ui_egui::gizmo;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Harness {
    ctx: egui::Context,
    engine: Engine,
    editor: PartEditor,
    viewport: Viewport,
    time: f64,
    sent: Vec<Operation>,
}

fn chip() -> PackageForm {
    PackageForm::Chip(Box::new(ChipForm {
        kind: ChipKind::Resistor,
        length: 2.8,
        width: 1.2,
        height: 0.55,
        termination: 0.4,
    }))
}

impl Harness {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::default();
        let mut editor = PartEditor::default();
        editor.load_template(Template::TwoTerminal);
        if let Some(request) = editor.take_request() {
            run(&mut engine, request)?;
        }
        editor.commit(&ctx, &engine);
        editor.on_tab("model");
        let draft = editor.draft.as_ref().ok_or("draft")?.ids.package;
        engine.apply(
            engine.revision(),
            model_place::edit(draft, vec![PackageEdit::SetForm(chip())]),
        )?;
        editor.commit(&ctx, &engine);
        let package = editor.preview_package().ok_or("package")?;
        let mut viewport = Viewport::detached();
        let built = pcbcraft_solidcraft::project::preview(&package)?;
        viewport.set_model(crate::model_scene::build(built.geometry)?);
        let mut h = Self {
            ctx,
            engine,
            editor,
            viewport,
            time: 0.0,
            sent: vec![],
        };
        h.frame(vec![]);
        h.viewport.fit();
        h.frame(vec![]);
        Ok(h)
    }

    /// One 800 × 600 3D frame, a tenth of a second after the last.
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.1;
        let (editor, engine, viewport, sent) = (
            &mut self.editor,
            &self.engine,
            &mut self.viewport,
            &mut self.sent,
        );
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                editor.model_view_prepare(viewport, engine);
                viewport.show(ui);
                sent.extend(editor.model_view(ui, viewport, engine));
            },
        );
        output.textures_delta.clear();
    }

    /// One inspector frame; the texts drawn and their centres.
    fn inspector(&mut self, events: Vec<egui::Event>) -> Vec<(String, Pos2)> {
        self.time += 0.1;
        let (editor, engine, sent) = (&mut self.editor, &self.engine, &mut self.sent);
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 2400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                sent.extend(editor.inspector(ui, engine));
            },
        );
        output.textures_delta.clear();
        fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.pos + text.galley.size() * 0.5,
                )),
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

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Click the inspector text `label`.
    fn click_label(&mut self, label: &str) -> TestResult {
        let texts = self.inspector(vec![]);
        let at = texts
            .iter()
            .find(|(t, _)| t == label)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("no {label:?} in the inspector"))?;
        self.time += 1.0;
        self.inspector(vec![egui::Event::PointerMoved(at)]);
        self.inspector(vec![Self::button(at, true)]);
        self.inspector(vec![Self::button(at, false)]);
        Ok(())
    }

    /// Click a 3D view position.
    fn click_view(&mut self, at: Pos2) {
        self.time += 1.0;
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![Self::button(at, true)]);
        self.frame(vec![Self::button(at, false)]);
    }

    /// Apply every sent command and reload the working copy.
    fn apply_sent(&mut self) -> Result<usize, Box<dyn std::error::Error>> {
        let sent = std::mem::take(&mut self.sent);
        for op in &sent {
            self.engine.apply(self.engine.revision(), op.clone())?;
        }
        self.editor.commit(&self.ctx, &self.engine);
        Ok(sent.len())
    }

    fn placement(&self) -> Result<ModelPlacement, Box<dyn std::error::Error>> {
        let id = self.editor.draft.as_ref().ok_or("draft")?.ids.package;
        match &self.engine.drafts().get(id).ok_or("package")?.definition {
            Definition::Package(p) => Ok(p.placement),
            _ => Err("not a package".into()),
        }
    }

    fn screen(&self, p: [f64; 3]) -> Result<Pos2, Box<dyn std::error::Error>> {
        let rect = self.viewport.rect().ok_or("not drawn")?;
        self.viewport
            .proj(rect)
            .to_screen(pcbcraft_solidcraft::geom::Vec3::new(p[0], p[1], p[2]))
            .ok_or_else(|| "behind the camera".into())
    }
}

#[test]
fn placement_edits_keep_the_preview_build_key() -> TestResult {
    let mut h = Harness::new()?;
    let key = h
        .editor
        .preview_package()
        .as_ref()
        .and_then(ModelKey::package);
    let draft = h.editor.draft.as_ref().ok_or("draft")?.ids.package;
    h.sent.push(model_place::edit(
        draft,
        vec![PackageEdit::SetPlacement(ModelPlacement {
            translation: [0.3, 0.0, 0.1],
            rotation_deg: [0.0, 0.0, 90.0],
        })],
    ));
    h.apply_sent()?;
    assert_eq!(h.placement()?.rotation_deg, [0.0, 0.0, 90.0]);
    assert_eq!(
        key,
        h.editor
            .preview_package()
            .as_ref()
            .and_then(ModelKey::package)
    );
    // The display transform follows the stored placement instead.
    h.frame(vec![]);
    assert_eq!(h.viewport.model_transform(), h.placement()?.transform());
    Ok(())
}

#[test]
fn dragging_the_triad_moves_only_the_display_until_release() -> TestResult {
    let mut h = Harness::new()?;
    let generation = h.viewport.generation();
    let revision = h.engine.revision();
    let handle = gizmo::handle_point("x", pcbcraft_solidcraft::geom::Vec3::ZERO, 0.0)
        .ok_or("the triad was not drawn")?;
    let a = h.screen([handle.x, handle.y, handle.z])?;
    let far = h.screen([handle.x + 1.0, handle.y, handle.z])?;
    let b = far;
    h.frame(vec![egui::Event::PointerMoved(a)]);
    h.frame(vec![Harness::button(a, true)]);
    h.frame(vec![egui::Event::PointerMoved(a + (b - a) * 0.5)]);
    h.frame(vec![egui::Event::PointerMoved(b)]);
    // The view shows a frame's preview on the next frame.
    h.frame(vec![]);
    // Mid-drag: the model moved on screen, no command, no new model.
    assert!(h.sent.is_empty());
    assert_eq!(h.engine.revision(), revision);
    assert_eq!(h.viewport.generation(), generation);
    let moved = h.viewport.model_transform().translation;
    assert!(
        moved[0] > 0.5 && moved[1].abs() < 1e-9 && moved[2].abs() < 1e-9,
        "{moved:?}"
    );
    h.frame(vec![Harness::button(b, false)]);
    assert_eq!(h.apply_sent()?, 1, "one command on release");
    let placement = h.placement()?;
    assert!(placement.translation[0] > 0.5);
    assert_eq!(placement.rotation_deg, [0.0; 3]);
    assert_eq!(h.viewport.generation(), generation, "never rebuilt");
    h.engine.undo()?;
    h.editor.commit(&h.ctx, &h.engine);
    assert_eq!(h.placement()?, ModelPlacement::default());
    Ok(())
}

#[test]
fn escape_cancels_a_triad_drag() -> TestResult {
    let mut h = Harness::new()?;
    let handle = gizmo::handle_point("y", pcbcraft_solidcraft::geom::Vec3::ZERO, 0.0)
        .ok_or("the triad was not drawn")?;
    let a = h.screen([handle.x, handle.y, handle.z])?;
    let b = h.screen([handle.x, handle.y + 1.0, handle.z])?;
    h.frame(vec![egui::Event::PointerMoved(a)]);
    h.frame(vec![Harness::button(a, true)]);
    h.frame(vec![egui::Event::PointerMoved(b)]);
    h.frame(vec![]);
    assert_ne!(h.viewport.model_transform(), Transform3::IDENTITY);
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frame(vec![Harness::button(b, false)]);
    h.frame(vec![]);
    assert!(h.sent.is_empty());
    assert_eq!(h.viewport.model_transform(), Transform3::IDENTITY);
    Ok(())
}

#[test]
fn quick_actions_and_fit_are_one_command_each() -> TestResult {
    let mut h = Harness::new()?;
    h.click_label("Z +90°")?;
    assert!(matches!(
        h.sent.as_slice(),
        [Operation::Library(LibraryCommand::EditPackage { edits, .. })]
            if edits == &[PackageEdit::ApplyQuick(QuickAction::Rotate { axis: Axis::Z, quarter_turns: 1 })]
    ));
    h.apply_sent()?;
    assert_eq!(h.placement()?.rotation_deg, [0.0, 0.0, 90.0]);
    // Turned a quarter, the terminations miss their lands: a finding, and
    // a fit that turns it back.
    let texts = h.inspector(vec![]);
    assert!(texts.iter().any(|(t, _)| t.contains("off their lands")));
    h.click_label("Fit")?;
    assert!(matches!(
        h.sent.as_slice(),
        [Operation::Library(LibraryCommand::FitLeads {
            confirm: true,
            ..
        })]
    ));
    h.apply_sent()?;
    let fitted = h.placement()?;
    assert_eq!(fitted.rotation_deg, [0.0; 3]);
    assert!(fitted.translation.iter().all(|v| v.abs() < 1e-9));
    let texts = h.inspector(vec![]);
    assert!(
        texts
            .iter()
            .any(|(t, _)| t.contains("Leads on their lands"))
    );
    Ok(())
}

#[test]
fn typed_values_commit_once_when_the_field_is_left() -> TestResult {
    let mut h = Harness::new()?;
    let texts = h.inspector(vec![]);
    // Placement X, Y and Z are the last three millimetre fields at zero.
    let x = texts
        .iter()
        .filter(|(t, _)| t == "0 mm")
        .map(|(_, p)| *p)
        .rev()
        .nth(2)
        .ok_or("no X field")?;
    h.time += 1.0;
    h.inspector(vec![egui::Event::PointerMoved(x)]);
    h.inspector(vec![Harness::button(x, true)]);
    h.inspector(vec![Harness::button(x, false)]);
    h.inspector(vec![
        egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        },
        egui::Event::Text("0.25 mm".into()),
    ]);
    assert!(h.sent.is_empty(), "nothing while typing");
    h.inspector(vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.inspector(vec![]);
    assert_eq!(h.apply_sent()?, 1);
    assert_eq!(h.placement()?.translation, [0.25, 0.0, 0.0]);
    Ok(())
}

#[test]
fn point_to_point_moves_a_model_point_onto_a_pad_point() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.model.point_to_point = PointToPoint::Source;
    h.frame(vec![]);
    // The −x termination's contact, then pad 2's centre (+x).
    let package = h.editor.draft.as_ref().ok_or("draft")?.package.clone();
    let lead = package_fit_lead(&package, 0)?;
    let pad = h.editor.draft.as_ref().ok_or("draft")?.footprint.pads[1].position;
    let source = h.screen(lead)?;
    h.click_view(source);
    assert!(matches!(
        h.editor.model.point_to_point,
        PointToPoint::Target(_)
    ));
    let target = h.screen([pad[0], pad[1], 0.0])?;
    h.click_view(target);
    assert_eq!(h.editor.model.point_to_point, PointToPoint::Off);
    assert_eq!(h.apply_sent()?, 1);
    let t = h.placement()?.translation;
    assert!((t[0] - (pad[0] - lead[0])).abs() < 1e-9, "{t:?}");
    Ok(())
}

fn package_fit_lead(
    package: &pcbcraft_core::library::PackageDefinition,
    k: usize,
) -> Result<[f64; 3], Box<dyn std::error::Error>> {
    package
        .form
        .lead_points()
        .get(k)
        .copied()
        .ok_or_else(|| "no lead".into())
}
