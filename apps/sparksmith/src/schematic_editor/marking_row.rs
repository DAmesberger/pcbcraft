//! Slice D6: the instance card's MARKING section (founder decision F7). It
//! shows the resolved marking of the selected instance ("01C") and where it
//! comes from (this instance, the Part's top mark or the component), and
//! edits the instance's choices: hide, the template variables (date code,
//! lot) and a custom text that replaces the library marking.
//!
//! Every change commits live as one `instance.set_marking` and one undo
//! step; text commits on Enter or focus loss and Esc reverts it, like the
//! card's other fields. The section keeps its text buffers in egui memory,
//! so `SchematicEditor` gains no field. Markings are display-only: nothing
//! here changes geometry, the saved `.solidcraft` model or fabrication
//! output.
use super::{Live, live};
use crate::kit;
use crate::refusal::{self, Origin};
use sparksmith_core::library::marking::{
    self, Align, InstanceMarking, LaidElement, MAX_MARKING_VAR, MAX_TEMPLATE, MarkingItem,
    MarkingLayout, MarkingSpec, PolarityMark, Template,
};
use sparksmith_core::{
    ComponentInstance, Engine, Id, InstanceCommand, Operation, Project, Query, QueryResult,
};
use sparksmith_ui::ui_kit::{self, Button};

/// The template variable a date code fills (`{var:date_code}`).
pub const DATE_CODE: &str = "date_code";
/// The template variable a lot number fills (`{var:lot}`).
pub const LOT: &str = "lot";

/// Where the marking an instance shows comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Hidden,
    Instance,
    Part,
    Component,
    None,
}

impl Source {
    /// The precedence of `marking::effective`, told apart.
    pub fn of(
        marking: &InstanceMarking,
        part: Option<&MarkingSpec>,
        component: Option<&MarkingSpec>,
    ) -> Self {
        if marking.hide {
            Self::Hidden
        } else if marking.replace.is_some() {
            Self::Instance
        } else if part.is_some() {
            Self::Part
        } else if component.is_some() {
            Self::Component
        } else {
            Self::None
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hidden => "Hidden on this instance",
            Self::Instance => "Custom for this instance",
            Self::Part => "The Part's top mark",
            Self::Component => "The component's marking",
            Self::None => "No marking",
        }
    }
}

/// The text a laid-out marking shows, in reading order: its text lines,
/// then its polarity marks and bands named.
pub fn layout_text(layout: &MarkingLayout) -> String {
    let mut words: Vec<String> = Vec::new();
    for element in layout.all().flat_map(|l| l.elements.iter()) {
        match element {
            LaidElement::Text(text) if !text.text.is_empty() => words.push(text.text.clone()),
            LaidElement::Text(_) => {}
            LaidElement::Artwork { .. } => words.push("artwork".into()),
            LaidElement::Matrix { .. } => words.push("QR code".into()),
            LaidElement::Polarity { mark, .. } => words.push(
                match mark {
                    PolarityMark::Dot => "dot",
                    PolarityMark::Bar => "bar",
                    PolarityMark::Band => "band",
                    PolarityMark::Stripe => "stripe",
                }
                .into(),
            ),
            LaidElement::Bands { colours, .. } => words.push(format!("{} bands", colours.len())),
        }
    }
    words.join(" · ")
}

/// The one text template of a custom marking that is exactly one text
/// line, which the Custom text row edits; `None` for any other spec.
pub fn custom_text(marking: &InstanceMarking) -> Option<String> {
    match marking.replace.as_ref().map(|s| s.items.as_slice()) {
        None => Some(String::new()),
        Some([MarkingItem::Text { template, .. }]) => Some(template.0.clone()),
        Some(_) => None,
    }
}

/// `marking` with the custom text `text`: empty uses the library marking
/// again; otherwise one text line, centred and auto-fitted, on the area and
/// finish the library marking would use (`inherited`).
pub fn with_custom_text(
    marking: &InstanceMarking,
    text: &str,
    inherited: Option<&MarkingSpec>,
) -> InstanceMarking {
    let mut out = marking.clone();
    let text = text.trim();
    if text.is_empty() {
        out.replace = None;
        return out;
    }
    let base = marking
        .replace
        .as_ref()
        .or(inherited)
        .cloned()
        .unwrap_or_default();
    let (height_nm, align) = match marking.replace.as_ref().map(|s| s.items.as_slice()) {
        Some(
            [
                MarkingItem::Text {
                    height_nm, align, ..
                },
            ],
        ) => (*height_nm, *align),
        _ => (None, Align::Centre),
    };
    out.replace = Some(MarkingSpec {
        items: vec![MarkingItem::Text {
            template: Template(text.to_owned()),
            height_nm,
            align,
        }],
        ..base
    });
    out
}

/// `marking` with variable `name` set to `value`; empty removes it.
pub fn with_var(marking: &InstanceMarking, name: &str, value: &str) -> InstanceMarking {
    let mut out = marking.clone();
    let value = value.trim();
    if value.is_empty() {
        out.vars.remove(name);
    } else {
        out.vars.insert(name.to_owned(), value.to_owned());
    }
    out
}

/// The Part's top mark and the component's marking an instance inherits.
fn library_specs<'a>(
    project: &'a Project,
    instance: &ComponentInstance,
) -> (Option<&'a MarkingSpec>, Option<&'a MarkingSpec>) {
    let part = instance
        .part
        .and_then(|r| project.library.part(r))
        .and_then(|p| p.definition.top_mark.as_deref());
    let component = project
        .library
        .component(instance.component)
        .and_then(|c| c.definition.marking.as_deref());
    (part, component)
}

/// What the instance shows: the resolved text, or why there is none.
fn resolved(
    engine: &Engine,
    instance: &ComponentInstance,
    source: Source,
) -> (String, Vec<String>) {
    if source == Source::Hidden {
        return ("Hidden".into(), Vec::new());
    }
    match engine.query(Query::Marking {
        instance: instance.id,
    }) {
        Ok(QueryResult::Marking(Some(layout))) => {
            let notes = layout.findings.iter().map(|f| f.message.clone()).collect();
            let text = layout_text(&layout);
            if text.is_empty() {
                ("Nothing drawn".into(), notes)
            } else {
                (text, notes)
            }
        }
        Ok(QueryResult::Marking(None)) => ("None".into(), Vec::new()),
        Ok(_) => ("—".into(), Vec::new()),
        // The core refuses until the marking layout exists; the choices
        // below still apply.
        Err(error) => ("Not available yet".into(), vec![error.to_string()]),
    }
}

/// The section's text buffers, by instance, in egui memory.
#[derive(Clone, Debug, Default, PartialEq)]
struct Buffers {
    instance: Option<Id>,
    /// The stored values the buffers were loaded from.
    loaded: [String; 3],
    text: [String; 3],
    refused: Option<(egui::Id, String)>,
}

/// One text row of the section: Enter or focus loss commits a valid
/// change, Esc reverts. Returns the text to commit.
fn text_row(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    tip: &str,
    buffers: &mut Buffers,
    index: usize,
    max: usize,
) -> Option<String> {
    let (Some(text), Some(loaded)) = (buffers.text.get_mut(index), buffers.loaded.get(index))
    else {
        return None;
    };
    let response = kit::row(ui, label, |ui| {
        ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
        let invalid = text.trim().len() > max;
        kit::text_field(ui, label, text, hint, invalid).on_hover_text(tip)
    });
    let changed = text.trim() != loaded.as_str();
    let too_long = format!("At most {max} bytes");
    let invalid = (text.trim().len() > max).then_some(too_long.as_str());
    match live(ui, &response, &mut buffers.refused, changed, invalid) {
        Live::Commit => {
            refusal::mark(ui.ctx(), Origin::Field(response.id));
            Some(text.trim().to_owned())
        }
        Live::Revert => {
            *text = loaded.clone();
            None
        }
        Live::Idle => None,
    }
}

/// The MARKING section of the instance card. Folded, it still shows the
/// resolved text.
pub fn row(ui: &mut egui::Ui, engine: &Engine, instance: &ComponentInstance) -> Option<Operation> {
    let project = engine.project();
    let marking = &instance.marking;
    let (part, component) = library_specs(project, instance);
    let source = Source::of(marking, part, component);
    let (shown, notes) = resolved(engine, instance, source);
    let set = |marking: InstanceMarking| {
        Some(Operation::Instance(InstanceCommand::SetMarking {
            instance: instance.id,
            marking,
        }))
    };
    let custom = custom_text(marking);
    let stored = [
        marking.vars.get(DATE_CODE).cloned().unwrap_or_default(),
        marking.vars.get(LOT).cloned().unwrap_or_default(),
        custom.clone().unwrap_or_default(),
    ];
    let key = ui.scope_id().with(("instance-marking", instance.id));
    let mut buffers: Buffers = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    if buffers.instance != Some(instance.id) {
        buffers = Buffers {
            instance: Some(instance.id),
            ..Default::default()
        };
    }
    // Reload a buffer whose stored value changed elsewhere (undo, CLI).
    for (index, value) in stored.iter().enumerate() {
        if let (Some(loaded), Some(text)) =
            (buffers.loaded.get_mut(index), buffers.text.get_mut(index))
            && loaded != value
        {
            *loaded = value.clone();
            *text = value.clone();
        }
    }
    let mut output = None;
    kit::summary_section(ui, "instance-marking", "Marking", false, &shown, |ui| {
        kit::value_row(ui, "Shows", &shown);
        kit::value_row(ui, "From", source.label());
        for note in &notes {
            kit::note(ui, note);
        }
        let mut hide = marking.hide;
        if kit::check_row(ui, "Hide marking", &mut hide) {
            output = set(InstanceMarking {
                hide,
                ..marking.clone()
            });
        }
        if let Some(value) = text_row(
            ui,
            "Date code",
            "None",
            "Fills {var:date_code} in the marking",
            &mut buffers,
            0,
            MAX_MARKING_VAR,
        ) {
            output = set(with_var(marking, DATE_CODE, &value));
        }
        if let Some(value) = text_row(
            ui,
            "Lot",
            "None",
            "Fills {var:lot} in the marking",
            &mut buffers,
            1,
            MAX_MARKING_VAR,
        ) {
            output = set(with_var(marking, LOT, &value));
        }
        let none = InstanceMarking::default();
        let inherited = marking::effective(&none, part, component);
        match custom {
            Some(_) => {
                if let Some(value) = text_row(
                    ui,
                    "Custom text",
                    "Library marking",
                    "Replaces the library marking on this instance; \
                     empty uses the library marking",
                    &mut buffers,
                    2,
                    MAX_TEMPLATE,
                ) {
                    output = set(with_custom_text(marking, &value, inherited));
                }
            }
            None => {
                let items = marking.replace.as_ref().map_or(0, |s| s.items.len());
                let summary = format!("Custom marking · {items} items");
                kit::row(ui, "Custom", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&summary);
                        if ui_kit::button(ui, "Use library marking", Button::Small)
                            .on_hover_text("Remove this instance's custom marking")
                            .clicked()
                        {
                            output = set(InstanceMarking {
                                replace: None,
                                ..marking.clone()
                            });
                        }
                    });
                });
            }
        }
        kit::note(
            ui,
            "Display only: markings show in 3D and never reach the saved model or \
             fabrication output",
        );
    });
    ui.data_mut(|d| d.insert_temp(key, buffers));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_core::library::marking::{AreaChoice, Finish};

    fn nm(mm: f64) -> sparksmith_core::units::Nm {
        sparksmith_core::units::Nm::from_mm_f64_rounded(mm).unwrap_or_default()
    }

    fn spec(text: &str) -> MarkingSpec {
        MarkingSpec {
            items: vec![MarkingItem::Text {
                template: Template(text.into()),
                height_nm: Some(nm(0.4)),
                align: Align::Start,
            }],
            area: AreaChoice::Explicit {
                centre: sparksmith_core::units::PointNm::ORIGIN,
                size_nm: [nm(1.0), nm(0.5)],
            },
            finish: Finish::InkWhite,
            min_text_height_nm: None,
            ..MarkingSpec::default()
        }
    }

    #[test]
    fn the_source_follows_the_marking_precedence() {
        let part = spec("{mpn}");
        let component = spec("{field:1|code3}");
        let mut marking = InstanceMarking::default();
        assert_eq!(Source::of(&marking, None, None), Source::None);
        assert_eq!(
            Source::of(&marking, None, Some(&component)),
            Source::Component
        );
        assert_eq!(
            Source::of(&marking, Some(&part), Some(&component)),
            Source::Part
        );
        marking.replace = Some(spec("X"));
        assert_eq!(
            Source::of(&marking, Some(&part), Some(&component)),
            Source::Instance
        );
        marking.hide = true;
        assert_eq!(Source::of(&marking, Some(&part), None), Source::Hidden);
    }

    /// A custom text keeps the area and finish of the marking it replaces;
    /// empty text drops the replacement; variables are set and removed.
    #[test]
    fn custom_text_and_variables_edit_only_their_own_choice() {
        let inherited = spec("{mpn}");
        let marking = InstanceMarking::default();
        assert_eq!(custom_text(&marking).as_deref(), Some(""));
        let custom = with_custom_text(&marking, "  ABC ", Some(&inherited));
        let replace = custom.replace.as_ref();
        assert_eq!(replace.map(|s| s.area), Some(inherited.area));
        assert_eq!(replace.map(|s| s.finish), Some(Finish::InkWhite));
        assert_eq!(custom_text(&custom).as_deref(), Some("ABC"));
        assert!(!custom.hide && custom.vars.is_empty());
        // Editing the text again keeps the line's own height and alignment.
        let mut edited = custom.clone();
        if let Some(MarkingItem::Text { height_nm, .. }) =
            edited.replace.as_mut().and_then(|s| s.items.first_mut())
        {
            *height_nm = Some(nm(0.6));
        }
        let again = with_custom_text(&edited, "XYZ", Some(&inherited));
        assert_eq!(
            again.replace.as_ref().map(|s| s.items.clone()),
            Some(vec![MarkingItem::Text {
                template: Template("XYZ".into()),
                height_nm: Some(nm(0.6)),
                align: Align::Centre,
            }])
        );
        assert_eq!(with_custom_text(&custom, " ", Some(&inherited)), marking);
        let dated = with_var(&marking, DATE_CODE, "2611");
        assert_eq!(dated.vars.get(DATE_CODE).map(String::as_str), Some("2611"));
        assert_eq!(with_var(&dated, DATE_CODE, ""), marking);
        // A spec of several items is not a custom text.
        let mut two = spec("A");
        two.items.push(MarkingItem::Polarity {
            mark: PolarityMark::Dot,
            at: Default::default(),
        });
        let several = InstanceMarking {
            replace: Some(two),
            ..Default::default()
        };
        assert_eq!(custom_text(&several), None);
    }

    #[test]
    fn a_layout_reads_as_its_lines_and_marks() {
        use sparksmith_core::library::marking::LaidText;
        let layout = MarkingLayout {
            elements: vec![
                LaidElement::Text(LaidText {
                    text: "01C".into(),
                    height_mm: 0.4,
                    origin: [0.0, 0.0],
                }),
                LaidElement::Polarity {
                    mark: PolarityMark::Dot,
                    origin: [0.0, 0.0],
                    size_mm: [0.2, 0.2],
                },
            ],
            ..Default::default()
        };
        assert_eq!(layout_text(&layout), "01C · dot");
        assert_eq!(layout_text(&MarkingLayout::default()), "");
    }

    /// The instance card, driven through real frames.
    mod card {
        use super::super::super::SchematicEditor;
        use crate::kit;
        use egui::{Pos2, Rect};
        use sparksmith_core::{Engine, InstanceCommand, Operation};
        use sparksmith_ui::fonts;

        type TestResult = Result<(), Box<dyn std::error::Error>>;

        fn frame(
            editor: &mut SchematicEditor,
            engine: &Engine,
            ctx: &egui::Context,
            events: Vec<egui::Event>,
        ) -> (Vec<(String, Pos2)>, Option<Operation>) {
            let mut command = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 3000.0))),
                    events,
                    ..Default::default()
                },
                |ui| command = kit::form(ui, |ui| editor.inspector(ui, engine)),
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
            (out, command)
        }

        fn at(shown: &[(String, Pos2)], label: &str) -> Result<Pos2, String> {
            shown
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, p)| *p)
                .ok_or_else(|| format!("missing {label}: {shown:?}"))
        }

        fn press(point: Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }

        /// Click at `point`; the operation of the release frame.
        fn click_at(
            editor: &mut SchematicEditor,
            engine: &Engine,
            ctx: &egui::Context,
            point: Pos2,
        ) -> Option<Operation> {
            frame(
                editor,
                engine,
                ctx,
                vec![egui::Event::PointerMoved(point), press(point, true)],
            );
            frame(editor, engine, ctx, vec![press(point, false)]).1
        }

        fn click(
            editor: &mut SchematicEditor,
            engine: &Engine,
            ctx: &egui::Context,
            label: &str,
        ) -> Result<Option<Operation>, String> {
            let (shown, _) = frame(editor, engine, ctx, vec![]);
            let point = at(&shown, label)?;
            Ok(click_at(editor, engine, ctx, point))
        }

        fn ready()
        -> Result<(egui::Context, Engine, SchematicEditor, u64), Box<dyn std::error::Error>>
        {
            let ctx = egui::Context::default();
            fonts::setup_bundled_fonts(&ctx);
            let engine = Engine::demo()?;
            let r1 = engine
                .project()
                .instance_by_designator("R1")
                .ok_or("no R1")?
                .id;
            let mut editor = SchematicEditor::default();
            editor.select_instance(Some(r1), engine.project());
            Ok((ctx, engine, editor, r1))
        }

        /// Folded, the section names what the instance shows; opened, Hide
        /// marking sends one `instance.set_marking` with only `hide` set.
        /// Applied, it hides the marking in one undo step; while the core
        /// refuses markings, the refusal leaves the document unchanged.
        #[test]
        fn hide_marking_sends_one_set_marking() -> TestResult {
            let (ctx, mut engine, mut editor, r1) = ready()?;
            let (shown, _) = frame(&mut editor, &engine, &ctx, vec![]);
            assert!(shown.iter().any(|(t, _)| t == "MARKING"), "{shown:?}");
            click(&mut editor, &engine, &ctx, "MARKING")?;
            let (shown, _) = frame(&mut editor, &engine, &ctx, vec![]);
            assert!(shown.iter().any(|(t, _)| t == "No marking"), "{shown:?}");
            let operation = click(&mut editor, &engine, &ctx, "Hide marking")?
                .ok_or("Hide marking sent nothing")?;
            let Operation::Instance(InstanceCommand::SetMarking { instance, marking }) = &operation
            else {
                return Err(format!("not set_marking: {operation:?}").into());
            };
            assert_eq!(*instance, r1);
            assert!(marking.hide && marking.vars.is_empty() && marking.replace.is_none());
            let before = engine.revision();
            let project = engine.project().clone();
            match engine.apply(before, operation) {
                Ok(_) => {
                    let hidden = engine.project().instance(r1).map(|i| i.marking.hide);
                    assert_eq!(hidden, Some(true));
                    engine.undo()?;
                    assert_eq!(*engine.project(), project);
                }
                Err(error) => {
                    assert!(error.to_string().contains("marking"), "{error}");
                    assert_eq!(engine.revision(), before);
                    assert_eq!(*engine.project(), project);
                }
            }
            Ok(())
        }

        /// A date code typed into its row commits once, on Enter, as the
        /// `date_code` variable; Esc instead reverts the text.
        #[test]
        fn a_date_code_commits_on_enter() -> TestResult {
            let (ctx, engine, mut editor, r1) = ready()?;
            click(&mut editor, &engine, &ctx, "MARKING")?;
            let (shown, _) = frame(&mut editor, &engine, &ctx, vec![]);
            let label = at(&shown, "Date code")?;
            // The field is in the control column, right of its label.
            let field = egui::pos2(500.0, label.y);
            click_at(&mut editor, &engine, &ctx, field);
            frame(
                &mut editor,
                &engine,
                &ctx,
                vec![egui::Event::Text("2611".into())],
            );
            let enter = egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            let (_, operation) = frame(&mut editor, &engine, &ctx, vec![enter]);
            let Some(Operation::Instance(InstanceCommand::SetMarking { instance, marking })) =
                operation
            else {
                return Err(format!("Enter sent {operation:?}").into());
            };
            assert_eq!(instance, r1);
            assert_eq!(
                marking.vars.get(super::DATE_CODE).map(String::as_str),
                Some("2611")
            );
            assert!(!marking.hide && marking.replace.is_none());
            let (_, again) = frame(&mut editor, &engine, &ctx, vec![]);
            assert!(again.is_none(), "one commit");
            Ok(())
        }
    }
}
