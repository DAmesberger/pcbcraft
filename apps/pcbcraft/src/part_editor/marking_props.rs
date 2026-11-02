//! Slice D6: the 3D tab's MARKING section (founder decision F7). It edits
//! the component's generic marking (every instance) or, chosen under For,
//! the exact top mark of one of the component's Part drafts: the item
//! list (text over fields, value codes, polarity marks, colour bands and
//! SVG artwork), the finish, the area and the smallest legible height,
//! with a preview of what an instance with the component defaults shows.
//!
//! The component's marking is component body data: it edits the working
//! copy and commits as the working set's `library.edit`, one undo step per
//! finished gesture. A Part's top mark is sent as that Part draft's own
//! `library.edit` once the gesture finishes. Text items take field values
//! per instance through placeholders (Insert › Instance value), so a
//! resistor's code follows each instance's resistance. Markings are
//! display-only: they never change geometry, the saved `.solidcraft`
//! model or fabrication output.
//!
//! Add item › SVG artwork… picks an `.svg` file (read and sized off the UI
//! thread), adds an Artwork item sized to the area and stages the file;
//! the gesture's commit then travels as `library.pack_artwork`, so the
//! edit and the packed file are one undo step. The artwork's licence is
//! stated on its item; publishing needs it (`marking.artwork`).
use super::PartEditor;
use super::marking_place::{self, SurfaceChoice};
use super::part_props::PartProps;
use crate::kit;
use egui::Ui;
use pcbcraft_core::fields::{self, FieldValue};
use pcbcraft_core::library::bundle::{AssetBytes, asset_hash};
use pcbcraft_core::library::marking::{
    self, Align, Anchor, AreaChoice, CodeScheme, DEFAULT_MIN_TEXT_HEIGHT_MM, Ecc,
    MAX_MARKING_ITEMS, MAX_TEMPLATE, MarkingInputs, MarkingItem, MarkingSpec, PolarityMark,
    SurfaceRef, Symbology, Template,
};
use pcbcraft_core::library::{ComponentDefinition, Definition, DraftEdit, Library, PackedAsset};
use pcbcraft_core::{DefinitionId, Engine, FieldRef, LibraryCommand, Operation};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Marking sizes and positions, mm.
const AREA_MM: std::ops::RangeInclusive<f64> = -100.0..=100.0;
const SIZE_MM: std::ops::RangeInclusive<f64> = 0.01..=100.0;
const HEIGHT_MM: std::ops::RangeInclusive<f64> = 0.05..=20.0;

/// Which marking the section edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// The component's generic marking.
    #[default]
    Component,
    /// A Part draft's exact top mark.
    Part(DefinitionId),
}

/// A Part draft's top mark while the section edits it.
#[derive(Clone, Debug, PartialEq)]
struct PartMark {
    id: DefinitionId,
    /// The draft's top mark when last loaded or sent.
    loaded: Option<Box<MarkingSpec>>,
    mark: Option<Box<MarkingSpec>>,
}

/// A read SVG: its bytes and natural size, mm.
struct ReadArtwork {
    text: String,
    size_mm: [f64; 2],
}

/// Add item › SVG artwork… while it runs.
#[derive(Default)]
enum ArtworkPick {
    #[default]
    Idle,
    Picking(Scope, Receiver<Option<PathBuf>>),
    Reading(Scope, Receiver<Result<ReadArtwork, String>>),
}

/// The section's state.
#[derive(Default)]
pub struct MarkingProps {
    scope: Scope,
    part: Option<PartMark>,
    pick: ArtworkPick,
    /// SVG files an Artwork item names that are not packed yet; they
    /// travel with the edit that names them (`library.pack_artwork`).
    staged: Vec<PackedAsset>,
    /// Why the last artwork could not be added.
    artwork_error: Option<String>,
    /// Which of the marking (0) and its further decals the rows edit.
    decal: usize,
}

/// Fit artwork of `natural` size into 60 % of `area`, aspect kept.
pub fn artwork_size(natural: [f64; 2], area: [f64; 2]) -> [f64; 2] {
    let fits = |v: f64| v.is_finite() && v > 0.0;
    if !(fits(natural[0]) && fits(natural[1])) {
        return [1.0, 1.0];
    }
    let room = [area[0] * 0.6, area[1] * 0.6];
    let scale = if fits(room[0]) && fits(room[1]) {
        (room[0] / natural[0]).min(room[1] / natural[1])
    } else {
        1.0
    };
    let clamp = |v: f64| v.clamp(*SIZE_MM.start(), *SIZE_MM.end());
    [clamp(natural[0] * scale), clamp(natural[1] * scale)]
}

/// The packed asset and Artwork item of a read SVG, sized for `area`.
pub fn artwork_item(text: &str, natural: [f64; 2], area: [f64; 2]) -> (PackedAsset, MarkingItem) {
    let bytes = text.as_bytes().to_vec();
    let hash = asset_hash(&bytes);
    (
        PackedAsset {
            hash,
            media_type: marking::ARTWORK_MEDIA_TYPE.into(),
            data: AssetBytes::new(bytes),
        },
        MarkingItem::Artwork {
            svg: hash,
            size_mm: artwork_size(natural, area),
            anchor: Anchor::Centre,
            licence: None,
        },
    )
}

/// Read and size an SVG off the UI thread.
fn read_artwork(path: &std::path::Path) -> Result<ReadArtwork, String> {
    let text = pcbcraft_io::artwork::read_svg(path).map_err(|e| e.to_string())?;
    let options = pcbcraft_solidcraft::svg::Options::new(0.1);
    let artwork = pcbcraft_solidcraft::svg::convert(&text, &options).map_err(|e| e.to_string())?;
    Ok(ReadArtwork {
        text,
        size_mm: artwork.size,
    })
}

/// The kinds Add item offers. Artwork comes from an imported SVG asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Code,
    Polarity,
    Bands,
    Code2d,
    Electrolytic,
}

impl Kind {
    const ADDABLE: [Kind; 6] = [
        Kind::Text,
        Kind::Code,
        Kind::Polarity,
        Kind::Bands,
        Kind::Code2d,
        Kind::Electrolytic,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Code => "Value code",
            Self::Polarity => "Polarity mark",
            Self::Bands => "Colour bands",
            Self::Code2d => "QR code",
            Self::Electrolytic => "Electrolytic µF/V and stripe",
        }
    }

    /// A new item of this kind; a field-reading item reads the first
    /// field, so it is complete as added.
    fn item(self, first_field: Option<FieldRef>) -> Option<MarkingItem> {
        Some(match self {
            Self::Text => MarkingItem::Text {
                template: Template("{mpn}".into()),
                height_mm: None,
                align: Align::Centre,
            },
            Self::Code => MarkingItem::Code {
                scheme: auto_resistor(),
                field: first_field?,
            },
            Self::Polarity => MarkingItem::Polarity {
                mark: PolarityMark::Dot,
                at: Anchor::Pin1Corner,
            },
            Self::Bands => MarkingItem::Bands {
                value: first_field?,
                tolerance: None,
            },
            Self::Code2d => MarkingItem::Code2d {
                symbology: Symbology::Qr,
                payload: Template("{designator}".into()),
                size_mm: 3.0,
                anchor: Anchor::End,
                ecc: Ecc::Medium,
            },
            Self::Electrolytic => MarkingItem::Electrolytic {
                capacitance: first_field?,
                voltage: None,
                stripe: true,
            },
        })
    }
}

/// The common chip-resistor pattern: the first scheme that represents the
/// value and fits (EIA-96 on small chips, then 4- and 3-digit codes).
fn auto_resistor() -> CodeScheme {
    CodeScheme::Auto {
        schemes: vec![
            CodeScheme::Eia96,
            CodeScheme::Resistor4Digit,
            CodeScheme::Resistor3Digit,
        ],
    }
}

fn item_label(item: &MarkingItem) -> &'static str {
    match item {
        MarkingItem::Text { .. } => "Text",
        MarkingItem::Code { .. } => "Value code",
        MarkingItem::Artwork { .. } => "Artwork",
        MarkingItem::Polarity { .. } => "Polarity mark",
        MarkingItem::Bands { .. } => "Colour bands",
        MarkingItem::Code2d { .. } => "QR code",
        MarkingItem::Electrolytic { .. } => "Electrolytic",
    }
}

fn schemes() -> Vec<(CodeScheme, String)> {
    vec![
        (auto_resistor(), "Resistor, best fit".into()),
        (CodeScheme::Eia96, "EIA-96 (01C)".into()),
        (CodeScheme::Resistor3Digit, "3-digit (103)".into()),
        (CodeScheme::Resistor4Digit, "4-digit (1002)".into()),
        (CodeScheme::CapacitorPf3Digit, "Capacitor pF (104)".into()),
    ]
}

fn anchors() -> Vec<(Anchor, String)> {
    vec![
        (Anchor::Centre, "Centre".into()),
        (Anchor::Pin1Corner, "Pin 1 corner".into()),
        (Anchor::Start, "Pin 1 end".into()),
        (Anchor::End, "Far end".into()),
    ]
}

/// The template placeholder of a field's instance value.
pub fn field_placeholder(field: FieldRef) -> String {
    format!("{{field:{}}}", field.id.0)
}

/// A template with field placeholders shown by field name.
pub fn named_template(template: &str, fields: &[(FieldRef, String)]) -> String {
    fields
        .iter()
        .fold(template.to_owned(), |text, (field, name)| {
            text.replace(&field_placeholder(*field), &format!("{{{name}}}"))
        })
}

/// The selected fields of `component`, by name.
fn field_names(component: &ComponentDefinition, library: &Library) -> Vec<(FieldRef, String)> {
    component
        .fields
        .iter()
        .map(|s| {
            let name = library.field_view(s.field).map_or_else(
                || format!("Field {}", s.field.id),
                |v| v.field.meta.name.clone(),
            );
            (s.field, name)
        })
        .collect()
}

/// What a marking reads for an instance with the component defaults: the
/// designator `<prefix>1`, the Part's MPN and manufacturer when a Part is
/// chosen, and each field's default.
fn preview_inputs(
    component: &ComponentDefinition,
    library: &Library,
    part: Option<(&str, &str)>,
    area_mm: [f64; 2],
) -> MarkingInputs {
    let mut inputs = MarkingInputs {
        designator: format!("{}1", component.designator_prefix),
        mpn: part.map(|(mpn, _)| mpn.to_owned()),
        manufacturer: part.map(|(_, manufacturer)| manufacturer.to_owned()),
        area_mm,
        ..Default::default()
    };
    for selection in &component.fields {
        let Some(view) = library.field_view(selection.field) else {
            continue;
        };
        let Some(value) = selection
            .default
            .as_ref()
            .or(view.field.definition.default.as_ref())
        else {
            continue;
        };
        let text = fields::display(value, &view.field.definition.value_type, view.kind_body());
        let si = match value {
            FieldValue::Quantity { si, .. } => Some(*si),
            _ => None,
        };
        inputs.fields.insert(selection.field.id, (text, si));
    }
    inputs
}

/// The preview line of `spec` and its advisory notes.
/// `spec` laid out for `inputs` on `package`: the marking and each decal
/// in its own surface's region.
pub(super) fn laid_out(
    spec: &MarkingSpec,
    inputs: &MarkingInputs,
    library: &Library,
    package: &pcbcraft_core::library::PackageDefinition,
) -> pcbcraft_core::Result<marking::MarkingLayout> {
    let region =
        |s: &MarkingSpec| marking::surface_region(library, Some(package), s.area, &s.placement);
    marking::layout_with(spec, inputs, &region)
}

impl PartEditor {
    /// The marking the Component 3D view shows: the one the section edits
    /// (the component's, or the chosen Part's top mark), with what an
    /// instance with the component's defaults reads, and the SVG text of
    /// its artwork (packed, or staged with the edit).
    pub(super) fn shown_marking(
        &self,
        engine: &Engine,
    ) -> Option<(
        MarkingSpec,
        MarkingInputs,
        Vec<(pcbcraft_core::ContentHash, String)>,
    )> {
        let set = self.draft.as_ref()?;
        let library = &engine.project().library;
        let state = &self.marking;
        let (spec, part) = match state.scope {
            Scope::Component => (set.component.marking.as_deref()?, None),
            Scope::Part(id) => {
                let mark = state
                    .part
                    .as_ref()
                    .filter(|p| p.id == id)?
                    .mark
                    .as_deref()?;
                let part = match &engine.drafts().get(id)?.definition {
                    Definition::Part(p) => Some((p.mpn.clone(), p.manufacturer.clone())),
                    _ => None,
                };
                (mark, part)
            }
        };
        let part = part.as_ref().map(|(m, f)| (m.as_str(), f.as_str()));
        let inputs = preview_inputs(&set.component, library, part, [0.0, 0.0]);
        let artwork =
            spec.artwork()
                .filter_map(|hash| {
                    let bytes =
                        library.assets.iter().chain(state.staged.iter()).find(|a| {
                            a.hash == hash && a.media_type == marking::ARTWORK_MEDIA_TYPE
                        })?;
                    let text = std::str::from_utf8(bytes.data.as_slice()).ok()?;
                    Some((hash, text.to_owned()))
                })
                .collect();
        Some((spec.clone(), inputs, artwork))
    }
}

fn preview(
    spec: &MarkingSpec,
    inputs: &MarkingInputs,
    library: &Library,
    package: &pcbcraft_core::library::PackageDefinition,
) -> (String, Vec<String>) {
    match laid_out(spec, inputs, library, package) {
        Ok(layout) => {
            let notes = layout.findings.iter().map(|f| f.message.clone()).collect();
            let text = layout_text(&layout);
            if text.is_empty() {
                ("Nothing drawn".into(), notes)
            } else {
                (text, notes)
            }
        }
        // A spec the core cannot lay out is still edited and saved; the
        // refusal says why (publishing names it as `marking.template`).
        Err(error) => ("Cannot be laid out".into(), vec![error.to_string()]),
    }
}

/// The text a laid-out marking shows: its lines, then its marks named.
fn layout_text(layout: &marking::MarkingLayout) -> String {
    use marking::LaidElement;
    let words: Vec<String> = layout
        .all()
        .flat_map(|l| l.elements.iter())
        .filter_map(|element| match element {
            LaidElement::Text(text) => (!text.text.is_empty()).then(|| text.text.clone()),
            LaidElement::Artwork { .. } => Some("artwork".into()),
            LaidElement::Matrix { .. } => Some("QR code".into()),
            LaidElement::Polarity { mark, .. } => Some(
                match mark {
                    PolarityMark::Dot => "dot",
                    PolarityMark::Bar => "bar",
                    PolarityMark::Band => "band",
                    PolarityMark::Stripe => "stripe",
                }
                .into(),
            ),
            LaidElement::Bands { colours, .. } => Some(format!("{} bands", colours.len())),
        })
        .collect();
    words.join(" · ")
}

/// A text item's template field, Insert menu, height and alignment.
fn text_rows(
    ui: &mut Ui,
    template: &mut Template,
    height_mm: &mut Option<f64>,
    align: &mut Align,
    fields: &[(FieldRef, String)],
) -> bool {
    let mut changed = false;
    let mut text = template.0.clone();
    let too_long = text.len() > MAX_TEMPLATE;
    let response = kit::row(ui, "Text", |ui| {
        ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
        kit::text_field(ui, "template", &mut text, "{mpn}", too_long)
            .on_hover_text("Placeholders: {mpn}, {manufacturer}, {designator}, {var:date_code}")
    });
    if response.changed() && text.len() <= MAX_TEMPLATE {
        template.0 = text;
        changed = true;
    }
    if too_long {
        kit::field_error(
            ui,
            &response,
            &format!("At most {MAX_TEMPLATE} bytes"),
            true,
        );
    }
    let shown = named_template(&template.0, fields);
    if shown != template.0 {
        kit::value_row(ui, "Reads", &shown);
    }
    let mut inserted = None;
    kit::row(ui, "Insert", |ui| {
        kit::dropdown_menu(ui, "insert", "Placeholder…", |ui| {
            for (field, name) in fields {
                if ui
                    .selectable_label(false, format!("Instance value: {name}"))
                    .clicked()
                {
                    inserted = Some(field_placeholder(*field));
                }
            }
            for (placeholder, label) in [
                ("{designator}", "Designator"),
                ("{mpn}", "Part number (MPN)"),
                ("{manufacturer}", "Manufacturer"),
                ("{var:date_code}", "Date code (per instance)"),
                ("{var:lot}", "Lot (per instance)"),
            ] {
                if ui.selectable_label(false, label).clicked() {
                    inserted = Some(placeholder.to_owned());
                }
            }
        });
    });
    if let Some(placeholder) = inserted
        && template.0.len() + placeholder.len() <= MAX_TEMPLATE
    {
        template.0.push_str(&placeholder);
        changed = true;
    }
    let mut auto = height_mm.is_none();
    if kit::check_row(ui, "Fit the height to the area", &mut auto) {
        *height_mm = if auto {
            None
        } else {
            Some(DEFAULT_MIN_TEXT_HEIGHT_MM * 2.0)
        };
        changed = true;
    }
    if let Some(height) = height_mm {
        changed |= kit::number_row(ui, "Height", height, "mm", HEIGHT_MM);
    }
    let aligns = [
        (Align::Centre, "Centre".to_owned()),
        (Align::Start, "Start".to_owned()),
        (Align::End, "End".to_owned()),
    ];
    changed |= kit::choice_row(ui, "Align", align, &aligns);
    changed
}

/// One field-choice row; `none` adds a "None" choice.
fn field_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Option<FieldRef>,
    fields: &[(FieldRef, String)],
    none: bool,
) -> bool {
    let mut options: Vec<(Option<FieldRef>, String)> = Vec::new();
    if none {
        options.push((None, "None".into()));
    }
    options.extend(fields.iter().map(|(f, n)| (Some(*f), n.clone())));
    if value.is_some_and(|v| !fields.iter().any(|(f, _)| *f == v)) {
        options.push((*value, "Field not selected".into()));
    }
    kit::choice_row(ui, label, value, &options)
}

/// The rows of one item; whether it changed.
fn item_rows(ui: &mut Ui, item: &mut MarkingItem, fields: &[(FieldRef, String)]) -> bool {
    match item {
        MarkingItem::Text {
            template,
            height_mm,
            align,
        } => text_rows(ui, template, height_mm, align, fields),
        MarkingItem::Code { scheme, field } => {
            let mut changed = kit::choice_row(ui, "Code", scheme, &schemes());
            let mut chosen = Some(*field);
            if field_row(ui, "Value", &mut chosen, fields, false)
                && let Some(chosen) = chosen
            {
                *field = chosen;
                changed = true;
            }
            kit::note(ui, "Each instance shows the code of its own value");
            changed
        }
        MarkingItem::Polarity { mark, at } => {
            let marks = [
                (PolarityMark::Dot, "Dot".to_owned()),
                (PolarityMark::Bar, "Bar".to_owned()),
                (PolarityMark::Band, "Band".to_owned()),
                (PolarityMark::Stripe, "Stripe".to_owned()),
            ];
            kit::choice_row(ui, "Mark", mark, &marks) | kit::choice_row(ui, "At", at, &anchors())
        }
        MarkingItem::Bands { value, tolerance } => {
            let mut chosen = Some(*value);
            let mut changed = false;
            if field_row(ui, "Value", &mut chosen, fields, false)
                && let Some(chosen) = chosen
            {
                *value = chosen;
                changed = true;
            }
            changed |= field_row(ui, "Tolerance", tolerance, fields, true);
            changed
        }
        MarkingItem::Code2d {
            payload,
            size_mm,
            anchor,
            ecc,
            ..
        } => {
            let mut text = payload.0.clone();
            let too_long = text.len() > MAX_TEMPLATE;
            let response = kit::row(ui, "Payload", |ui| {
                ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                kit::text_field(ui, "payload", &mut text, "{designator}", too_long)
                    .on_hover_text("What the code reads: {designator}, {mpn}, {var:serial}, text")
            });
            let mut changed = false;
            if response.changed() && text.len() <= MAX_TEMPLATE {
                payload.0 = text;
                changed = true;
            }
            if too_long {
                kit::field_error(
                    ui,
                    &response,
                    &format!("At most {MAX_TEMPLATE} bytes"),
                    true,
                );
            }
            let eccs = [
                (Ecc::Low, "Low (7 %)".to_owned()),
                (Ecc::Medium, "Medium (15 %)".to_owned()),
                (Ecc::Quartile, "Quartile (25 %)".to_owned()),
                (Ecc::High, "High (30 %)".to_owned()),
            ];
            changed |= kit::number_row(ui, "Side", size_mm, "mm", SIZE_MM)
                | kit::choice_row(ui, "At", anchor, &anchors())
                | kit::choice_row(ui, "Recovery", ecc, &eccs);
            kit::note(ui, "A QR code; its quiet zone is kept clear");
            changed
        }
        MarkingItem::Electrolytic {
            capacitance,
            voltage,
            stripe,
        } => {
            let mut changed = false;
            let mut chosen = Some(*capacitance);
            if field_row(ui, "Capacitance", &mut chosen, fields, false)
                && let Some(chosen) = chosen
            {
                *capacitance = chosen;
                changed = true;
            }
            changed |= field_row(ui, "Voltage", voltage, fields, true);
            changed |= kit::check_row(ui, "Negative stripe", stripe);
            kit::note(ui, "Each instance shows its own µF and V");
            changed
        }
        MarkingItem::Artwork {
            svg,
            size_mm,
            anchor,
            licence,
        } => {
            let hash: String = svg.to_string().chars().take(12).collect();
            kit::value_row(ui, "SVG", &hash);
            let mut changed = kit::pair_row(ui, "Size", size_mm, "mm", SIZE_MM)
                | kit::choice_row(ui, "At", anchor, &anchors());
            let mut text = licence.clone().unwrap_or_default();
            let missing = text.trim().is_empty();
            let response = kit::row(ui, "Licence", |ui| {
                ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                kit::text_field(ui, "artwork-licence", &mut text, "CC0-1.0", false)
                    .on_hover_text("The artwork's licence: an SPDX id or the owner's terms")
            });
            if response.changed() && text.len() <= pcbcraft_core::MAX_LABEL {
                *licence = (!text.trim().is_empty()).then_some(text);
                changed = true;
            }
            if missing {
                kit::note(ui, "State the artwork's licence before publishing");
            }
            changed
        }
    }
}

/// The rows editing `spec`: items, then finish, area and smallest height.
/// Whether it changed.
pub fn spec_rows(
    ui: &mut Ui,
    spec: &mut Option<Box<MarkingSpec>>,
    fields: &[(FieldRef, String)],
    pick_artwork: &mut bool,
    surfaces: &[SurfaceChoice],
    editing: &mut usize,
) -> bool {
    let mut changed = false;
    if let Some(root) = spec.as_deref_mut() {
        changed |= marking_place::decal_row(ui, root, editing);
        let body = marking_place::edited(root, *editing);
        let mut remove = None;
        for (index, item) in body.items.iter_mut().enumerate() {
            kit::rule(ui);
            ui.push_id(("marking-item", index), |ui| {
                kit::row(ui, item_label(item), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui_kit::button(ui, "Remove", Button::Small)
                            .on_hover_text("Remove this item from the marking")
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
                changed |= item_rows(ui, item, fields);
            });
        }
        if let Some(index) = remove
            && index < body.items.len()
        {
            body.items.remove(index);
            changed = true;
        }
        kit::rule(ui);
        changed |= marking_place::finish_rows(ui, &mut body.finish);
        changed |= marking_place::placement_rows(ui, &mut body.placement, surfaces);
        let mut explicit = matches!(body.area, AreaChoice::Explicit { .. });
        let on_model = body.placement.on == SurfaceRef::Model;
        let areas = [
            (false, "The model's top face".to_owned()),
            (true, "A rectangle".to_owned()),
        ];
        if on_model && kit::choice_row(ui, "Area", &mut explicit, &areas) {
            body.area = if explicit {
                AreaChoice::Explicit {
                    centre: [0.0, 0.0],
                    size_mm: [1.0, 0.5],
                }
            } else {
                AreaChoice::FromModel
            };
            changed = true;
        }
        if let (true, AreaChoice::Explicit { centre, size_mm }) = (on_model, &mut body.area) {
            changed |= kit::pair_row(ui, "Centre", centre, "mm", AREA_MM);
            changed |= kit::pair_row(ui, "Size", size_mm, "mm", SIZE_MM);
        }
        let mut smallest = body
            .min_text_height_mm
            .unwrap_or(DEFAULT_MIN_TEXT_HEIGHT_MM);
        if kit::number_row(ui, "Smallest text", &mut smallest, "mm", HEIGHT_MM) {
            body.min_text_height_mm = Some(smallest);
            changed = true;
        }
    }
    let count = spec
        .as_deref()
        .map_or(0, |s| s.all().nth(*editing).map_or(0, |d| d.items.len()));
    if count >= MAX_MARKING_ITEMS {
        kit::hint(
            ui,
            &format!("A marking has at most {MAX_MARKING_ITEMS} items"),
            "",
        );
    } else {
        let mut added = None;
        let first_field = fields.first().map(|(f, _)| *f);
        kit::row(ui, "Add item", |ui| {
            kit::dropdown_menu(ui, "add-marking-item", "Choose an item", |ui| {
                for kind in Kind::ADDABLE {
                    let item = kind.item(first_field);
                    let response = ui.add_enabled(
                        item.is_some(),
                        egui::Button::selectable(false, kind.label()),
                    );
                    if response.clicked() {
                        added = item;
                    }
                }
                if ui
                    .selectable_label(false, "SVG artwork…")
                    .on_hover_text(
                        "Your own SVG drawn on the part, such as a logo; Pcbcraft ships none",
                    )
                    .clicked()
                {
                    *pick_artwork = true;
                }
            });
        });
        if let Some(item) = added {
            let root = spec.get_or_insert_with(Default::default);
            marking_place::edited(root, *editing).items.push(item);
            changed = true;
        }

        if first_field.is_none() {
            kit::note(
                ui,
                "Value codes and bands read a field: add one under Details",
            );
        }
    }
    if spec.is_some() {
        let remove = kit::row(ui, "", |ui| {
            ui_kit::button(ui, "Remove marking", Button::Small)
                .on_hover_text("Leave this marking empty")
                .clicked()
        });
        if remove {
            *spec = None;
            changed = true;
        }
    }
    changed
}

impl PartEditor {
    /// The Marking section of the 3D tab.
    pub(super) fn marking_section(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let library = &engine.project().library;
        let set = self.draft.as_mut()?;
        let state = &mut self.marking;
        let parts: Vec<(DefinitionId, String, String)> =
            PartProps::draft_ids(engine, set.ids.component)
                .into_iter()
                .filter_map(|id| match &engine.drafts().get(id)?.definition {
                    Definition::Part(p) => Some((id, p.mpn.clone(), p.manufacturer.clone())),
                    _ => None,
                })
                .collect();
        if let Scope::Part(id) = state.scope
            && !parts.iter().any(|(p, _, _)| *p == id)
        {
            state.scope = Scope::Component;
        }
        state.sync_part(engine);
        // A read SVG joins the marking it was picked for.
        if let Some((scope, read)) = state.poll_pick() {
            let area = match scope {
                Scope::Component => set.component.marking.as_deref(),
                Scope::Part(_) => state.part.as_ref().and_then(|p| p.mark.as_deref()),
            }
            .and_then(|m| match m.area {
                AreaChoice::Explicit { size_mm, .. } => Some(size_mm),
                AreaChoice::FromModel => None,
            })
            .unwrap_or([set.package.envelope[0], set.package.envelope[1]]);
            let (asset, item) = artwork_item(&read.text, read.size_mm, area);
            let spec = match scope {
                Scope::Component => Some(&mut set.component.marking),
                Scope::Part(id) => state
                    .part
                    .as_mut()
                    .filter(|p| p.id == id)
                    .map(|p| &mut p.mark),
            };
            match spec {
                Some(spec)
                    if spec
                        .as_ref()
                        .is_none_or(|s| s.items.len() < MAX_MARKING_ITEMS) =>
                {
                    spec.get_or_insert_with(Default::default).items.push(item);
                    if state.staged.iter().all(|a| a.hash != asset.hash) {
                        state.staged.push(asset);
                    }
                }
                Some(_) => {
                    state.artwork_error =
                        Some(format!("a marking has at most {MAX_MARKING_ITEMS} items"));
                }
                None => state.artwork_error = Some("the Part draft is gone".into()),
            }
        }
        let fields = field_names(&set.component, library);
        let envelope = set.package.envelope;
        let surfaces = marking_place::surfaces(&set.package);
        let package = set.package.clone();
        let part_open = self.parts.is_open();
        let mut operation = None;
        let summary = match &set.component.marking {
            Some(spec) => format!("{} items", spec.items.len()),
            None => "None".to_owned(),
        };
        let mut pick = false;
        kit::summary_section(ui, "model-marking", "Marking", false, &summary, |ui| {
            if !parts.is_empty() {
                let mut options = vec![(Scope::Component, "Every instance".to_owned())];
                options.extend(parts.iter().map(|(id, mpn, _)| {
                    let mpn = if mpn.is_empty() { "New part" } else { mpn };
                    (Scope::Part(*id), format!("Part {mpn} (top mark)"))
                }));
                if kit::choice_row(ui, "For", &mut state.scope, &options) {
                    state.part = None;
                    state.sync_part(engine);
                }
            }
            let part = match state.scope {
                Scope::Part(id) => parts
                    .iter()
                    .find(|(p, _, _)| *p == id)
                    .map(|(_, mpn, manufacturer)| (mpn.as_str(), manufacturer.as_str())),
                Scope::Component => None,
            };
            let inputs = preview_inputs(&set.component, library, part, [envelope[0], envelope[1]]);
            let reading = matches!(
                state.pick,
                ArtworkPick::Picking(..) | ArtworkPick::Reading(..)
            );
            let artwork_error = state.artwork_error.clone();
            let scope = state.scope;
            let spec = match state.scope {
                Scope::Component => &mut set.component.marking,
                Scope::Part(id) => {
                    if part_open {
                        kit::note(ui, "Close the Part dialog to edit its top mark here");
                        return;
                    }
                    let Some(edit) = state.part.as_mut().filter(|p| p.id == id) else {
                        return;
                    };
                    &mut edit.mark
                }
            };
            spec_rows(ui, spec, &fields, &mut pick, &surfaces, &mut state.decal);
            if reading {
                kit::note(ui, "Reading the SVG…");
            }
            if let Some(error) = &artwork_error {
                kit::note(ui, &format!("Artwork not added: {error}"));
            }
            if let Some(body) = spec.as_deref() {
                let (shown, notes) = preview(body, &inputs, library, &package);
                kit::rule(ui);
                kit::value_row(ui, "Preview", &shown);
                for note in &notes {
                    kit::note(ui, note);
                }
            }
            if scope == Scope::Component {
                kit::note(
                    ui,
                    "A Part's top mark replaces this on instances of that Part",
                );
            }
            kit::note(
                ui,
                "Display only: never in the saved 3D model or fabrication output",
            );
        });
        if pick {
            state.start_pick(ui.ctx());
        }
        let interacting =
            ui.ctx().memory(|m| m.focused().is_some()) || ui.ctx().input(|i| i.pointer.any_down());
        if !interacting && !part_open {
            operation = state.take_part_edit(engine);
        }
        state.with_artwork(operation)
    }
}

impl MarkingProps {
    /// Ask for an SVG with the native picker (made here, awaited on a
    /// worker, as the other pickers are).
    fn start_pick(&mut self, ctx: &egui::Context) {
        if !matches!(self.pick, ArtworkPick::Idle) {
            return;
        }
        self.artwork_error = None;
        let future = rfd::AsyncFileDialog::new()
            .set_title("Add SVG artwork to the marking")
            .add_filter("SVG drawing", &["svg", "SVG"])
            .pick_file();
        let (sender, receiver) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = pollster::block_on(future).map(|h| h.path().to_path_buf());
            let _ = sender.send(picked);
            ctx.request_repaint();
        });
        self.pick = ArtworkPick::Picking(self.scope, receiver);
    }

    /// Advance the pick; a read SVG and the marking it is for.
    fn poll_pick(&mut self) -> Option<(Scope, ReadArtwork)> {
        match std::mem::take(&mut self.pick) {
            ArtworkPick::Idle => None,
            ArtworkPick::Picking(scope, receiver) => match receiver.try_recv() {
                Ok(Some(path)) => {
                    let (sender, reading) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(read_artwork(&path));
                    });
                    self.pick = ArtworkPick::Reading(scope, reading);
                    None
                }
                Ok(None) => None,
                Err(TryRecvError::Empty) => {
                    self.pick = ArtworkPick::Picking(scope, receiver);
                    None
                }
                Err(TryRecvError::Disconnected) => {
                    self.artwork_error = Some("the file picker closed unexpectedly".into());
                    None
                }
            },
            ArtworkPick::Reading(scope, receiver) => match receiver.try_recv() {
                Ok(Ok(read)) => Some((scope, read)),
                Ok(Err(error)) => {
                    self.artwork_error = Some(error);
                    None
                }
                Err(TryRecvError::Empty) => {
                    self.pick = ArtworkPick::Reading(scope, receiver);
                    None
                }
                Err(TryRecvError::Disconnected) => {
                    self.artwork_error = Some("reading the SVG stopped unexpectedly".into());
                    None
                }
            },
        }
    }

    /// `operation` carrying the staged SVG files its edits name: a
    /// `library.edit` becomes `library.pack_artwork`, one undo step.
    /// Files no edit names stay staged.
    pub fn with_artwork(&mut self, operation: Option<Operation>) -> Option<Operation> {
        let Some(Operation::Library(LibraryCommand::Edit { edits })) = operation else {
            return operation;
        };
        let named: Vec<pcbcraft_core::ContentHash> = edits
            .iter()
            .flat_map(|e| match &e.definition {
                Definition::Component(c) => c.marking.iter().flat_map(|m| m.artwork()).collect(),
                Definition::Part(p) => p.top_mark.iter().flat_map(|m| m.artwork()).collect(),
                _ => Vec::new(),
            })
            .collect();
        let (artwork, kept): (Vec<PackedAsset>, Vec<PackedAsset>) =
            std::mem::take(&mut self.staged)
                .into_iter()
                .partition(|a| named.contains(&a.hash));
        self.staged = kept;
        if artwork.is_empty() {
            return Some(Operation::Library(LibraryCommand::Edit { edits }));
        }
        Some(Operation::Library(LibraryCommand::PackArtwork {
            edits,
            artwork,
        }))
    }

    /// Load the chosen Part draft's top mark, unless the section holds an
    /// unsent edit of it.
    fn sync_part(&mut self, engine: &Engine) {
        let Scope::Part(id) = self.scope else {
            self.part = None;
            return;
        };
        let stored = match engine.drafts().get(id).map(|d| &d.definition) {
            Some(Definition::Part(p)) => p.top_mark.clone(),
            _ => {
                self.part = None;
                return;
            }
        };
        match &mut self.part {
            Some(edit) if edit.id == id => {
                if edit.mark == edit.loaded && edit.loaded != stored {
                    edit.loaded = stored.clone();
                    edit.mark = stored;
                }
            }
            _ => {
                self.part = Some(PartMark {
                    id,
                    loaded: stored.clone(),
                    mark: stored,
                });
            }
        }
    }

    /// The Part draft's `library.edit` for a finished top-mark edit.
    fn take_part_edit(&mut self, engine: &Engine) -> Option<Operation> {
        let edit = self.part.as_mut().filter(|e| e.mark != e.loaded)?;
        let draft = engine.drafts().get(edit.id)?;
        let Definition::Part(part) = &draft.definition else {
            return None;
        };
        let mut part = (**part).clone();
        part.top_mark = edit.mark.clone();
        edit.loaded = edit.mark.clone();
        Some(Operation::Library(LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id: edit.id,
                meta: draft.meta.clone(),
                definition: Definition::Part(Box::new(part)),
            }],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect};
    use solidcraft_ui_egui::fonts;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct Harness {
        ctx: egui::Context,
        engine: Engine,
        editor: PartEditor,
    }

    impl Harness {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let ctx = egui::Context::default();
            fonts::setup_bundled_fonts(&ctx);
            let engine = Engine::demo()?;
            let mut editor = PartEditor::default();
            editor.open_first(&engine);
            editor.commit(&ctx, &engine);
            editor.on_tab("model");
            // The section starts folded: open it.
            let mut h = Self {
                ctx,
                engine,
                editor,
            };
            h.click("MARKING")?;
            Ok(h)
        }

        fn card(&mut self, events: Vec<egui::Event>) -> (Vec<(String, Pos2)>, Option<Operation>) {
            let mut operation = None;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 4000.0))),
                    events,
                    ..Default::default()
                },
                |ui| operation = self.editor.inspector(ui, &self.engine),
            );
            output.textures_delta.clear();
            let mut out = Vec::new();
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
            for shape in &output.shapes {
                texts(&shape.shape, &mut out);
            }
            (out, operation)
        }

        fn texts(&mut self) -> Vec<String> {
            self.card(vec![]).0.into_iter().map(|(t, _)| t).collect()
        }

        fn click(&mut self, label: &str) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
            let (shown, _) = self.card(vec![]);
            let at = shown
                .iter()
                .rev()
                .find(|(t, _)| t == label)
                .map(|(_, p)| *p)
                .ok_or_else(|| {
                    format!(
                        "{label} not in {:?}",
                        shown.iter().map(|(t, _)| t).collect::<Vec<_>>()
                    )
                })?;
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.card(vec![egui::Event::PointerMoved(at)]);
            self.card(vec![button(true)]);
            let (_, operation) = self.card(vec![button(false)]);
            Ok(operation)
        }

        /// Apply the editor's pending commit; whether one was sent.
        fn commit(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
            match self.editor.commit(&self.ctx, &self.engine) {
                Some(op) => {
                    self.engine.apply(self.engine.revision(), op)?;
                    self.editor.commit(&self.ctx, &self.engine);
                    Ok(true)
                }
                None => Ok(false),
            }
        }

        fn stored(&self) -> Result<Option<MarkingSpec>, Box<dyn std::error::Error>> {
            let id = self.editor.draft.as_ref().ok_or("no set")?.ids.component;
            match &self.engine.drafts().get(id).ok_or("no draft")?.definition {
                Definition::Component(c) => Ok(c.marking.as_deref().cloned()),
                _ => Err("not a component".into()),
            }
        }
    }

    /// Add item › Text adds a text line to the component's marking in the
    /// working copy; the editor's commit stores it as one `library.edit`
    /// with the default finish and area. Remove marking takes it out again.
    #[test]
    fn add_item_puts_a_text_line_on_the_component_marking() -> TestResult {
        let mut h = Harness::new()?;
        let shown = h.texts();
        assert!(shown.iter().any(|t| t == "MARKING"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "Add item"), "{shown:?}");
        assert_eq!(h.stored()?, None);
        h.click("Choose an item")?;
        assert!(h.click("Text")?.is_none(), "a working-copy edit");
        assert!(h.commit()?, "one commit");
        let stored = h.stored()?.ok_or("no marking stored")?;
        assert_eq!(
            stored.items,
            vec![MarkingItem::Text {
                template: Template("{mpn}".into()),
                height_mm: None,
                align: Align::Centre,
            }]
        );
        assert_eq!(stored.finish, marking::Finish::LaserEtch);
        assert_eq!(stored.area, AreaChoice::FromModel);
        let shown = h.texts();
        assert!(shown.iter().any(|t| t == "Finish"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "Preview"), "{shown:?}");
        // The core lays the line out: a component has no MPN, so nothing
        // is drawn and the preview says why rather than drawing a guess.
        assert!(shown.iter().any(|t| t == "Nothing drawn"), "{shown:?}");
        assert!(
            shown
                .iter()
                .any(|t| t.contains("no manufacturer part number")),
            "{shown:?}"
        );
        h.click("Remove marking")?;
        assert!(h.commit()?);
        assert_eq!(h.stored()?, None);
        Ok(())
    }

    /// A Code item reads a selected field; with no field selected the
    /// Add item menu offers it disabled.
    #[test]
    fn a_value_code_reads_a_selected_field() -> TestResult {
        let mut h = Harness::new()?;
        let fields = h
            .editor
            .draft
            .as_ref()
            .map(|s| s.component.fields.len())
            .unwrap_or(0);
        h.click("Choose an item")?;
        h.click("Value code")?;
        h.commit()?;
        let stored = h.stored()?;
        if fields == 0 {
            assert_eq!(stored, None, "no field to read: nothing added");
            return Ok(());
        }
        let spec = stored.ok_or("no marking")?;
        let field = h
            .editor
            .draft
            .as_ref()
            .and_then(|s| s.component.fields.first())
            .map(|s| s.field)
            .ok_or("no field")?;
        assert_eq!(
            spec.items,
            vec![MarkingItem::Code {
                scheme: auto_resistor(),
                field,
            }]
        );
        Ok(())
    }

    /// Add item › SVG artwork… is offered; a read SVG joins the
    /// component's marking sized to the area, and the commit packs it with
    /// the edit as one `library.pack_artwork` (one undo step).
    #[test]
    fn svg_artwork_joins_the_marking_and_packs_with_its_edit() -> TestResult {
        let mut h = Harness::new()?;
        h.click("Choose an item")?;
        let shown = h.texts();
        assert!(shown.iter().any(|t| t == "SVG artwork…"), "{shown:?}");
        h.card(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4mm" height="2mm" viewBox="0 0 4 2"><rect width="4" height="2"/></svg>"#;
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok(ReadArtwork {
            text: svg.into(),
            size_mm: [4.0, 2.0],
        }))?;
        h.editor.marking.pick = ArtworkPick::Reading(Scope::Component, receiver);
        h.card(vec![]);
        let revision = h.engine.revision();
        let before = h.engine.to_file();
        let operation = h.editor.commit(&h.ctx, &h.engine).ok_or("no commit")?;
        let Operation::Library(LibraryCommand::PackArtwork { artwork, .. }) = &operation else {
            return Err(format!("expected library.pack_artwork, got {operation:?}").into());
        };
        assert_eq!(artwork.len(), 1);
        let hash = artwork[0].hash;
        h.engine.apply(revision, operation)?;
        h.editor.commit(&h.ctx, &h.engine);
        let stored = h.stored()?.ok_or("no marking stored")?;
        let [
            MarkingItem::Artwork {
                svg: named,
                size_mm,
                licence,
                ..
            },
        ] = stored.items.as_slice()
        else {
            return Err(format!("{:?}", stored.items).into());
        };
        assert_eq!(*named, hash);
        assert!(
            (size_mm[0] / size_mm[1] - 2.0).abs() < 1e-9,
            "aspect kept: {size_mm:?}"
        );
        assert_eq!(licence, &None);
        assert!(
            h.engine
                .project()
                .library
                .assets
                .iter()
                .any(|a| a.hash == hash && a.media_type == marking::ARTWORK_MEDIA_TYPE)
        );
        let shown = h.texts();
        assert!(
            shown
                .iter()
                .any(|t| t == "State the artwork's licence before publishing"),
            "{shown:?}"
        );
        h.engine.undo()?;
        assert_eq!(h.engine.to_file(), before, "one undo step");
        Ok(())
    }

    #[test]
    fn artwork_is_sized_into_the_area_with_its_aspect() {
        assert_eq!(artwork_size([4.0, 2.0], [2.0, 2.0]), [1.2, 0.6]);
        assert_eq!(artwork_size([1.0, 4.0], [10.0, 2.0]), [0.3, 1.2]);
        assert_eq!(artwork_size([f64::NAN, 1.0], [1.0, 1.0]), [1.0, 1.0]);
    }

    #[test]
    fn templates_name_their_fields() {
        let field = FieldRef::new(DefinitionId(7), pcbcraft_core::Revision::FIRST);
        let fields = vec![(field, "Resistance".to_owned())];
        let placeholder = field_placeholder(field);
        assert_eq!(placeholder, "{field:7}");
        assert_eq!(
            named_template(&format!("R {placeholder} {{mpn}}"), &fields),
            "R {Resistance} {mpn}"
        );
    }

    /// The preview reads the component defaults: designator `<prefix>1`
    /// and each field's default value with its SI magnitude.
    #[test]
    fn the_preview_reads_the_component_defaults() -> TestResult {
        let engine = Engine::demo()?;
        let library = &engine.project().library;
        // The demo resistor selects its quantity field without a default:
        // give the selection one, as a component author would.
        let mut component = library
            .components
            .iter()
            .find(|c| {
                c.definition.fields.iter().any(|s| {
                    library.field_view(s.field).is_some_and(|v| {
                        matches!(
                            v.field.definition.value_type,
                            fields::ValueType::Quantity { .. }
                        )
                    })
                })
            })
            .ok_or("no component with a quantity field")?
            .clone();
        for selection in &mut component.definition.fields {
            selection.default = Some(FieldValue::Quantity {
                si: 10_000.0,
                display_unit: None,
            });
        }
        let body = &component.definition;
        let inputs = preview_inputs(body, library, Some(("X1", "Maker")), [1.6, 0.8]);
        assert_eq!(inputs.designator, format!("{}1", body.designator_prefix));
        assert_eq!(inputs.mpn.as_deref(), Some("X1"));
        assert_eq!(inputs.area_mm, [1.6, 0.8]);
        assert!(
            inputs
                .fields
                .values()
                .any(|(text, si)| si.is_some() && !text.is_empty()),
            "{:?}",
            inputs.fields
        );
        Ok(())
    }
}
