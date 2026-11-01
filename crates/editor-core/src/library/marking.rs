//! Slice D4: instance markings, display-only vector decals
//! derived from fields and Part top-mark data (founder decision F7).
//!
//! Markings are never saved to `.solidcraft`, never in fabrication output
//! and never change geometry. Precedence: the instance's `replace`, then
//! the Part's `top_mark`, then the component's `marking`, then none. A
//! marking that cannot be represented draws nothing and gets an Info
//! finding (`marking.unrepresentable`, `marking.does_not_fit`); a wrong
//! code is never drawn. [`MarkingLayout`] is the 2D result the integration
//! crate (`pcbcraft-solidcraft::marking`, D5) turns into a decal mesh.
pub mod codes;
mod layout;
mod template;
#[cfg(test)]
mod tests;

use crate::diagnostics::{Finding, ObjectRef, Result, Stage};
use crate::ids::{ContentHash, DefinitionId, FieldRef, MAX_LABEL};
use crate::materials::MaterialId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use codes::CodeScheme;

/// Items one marking spec lists.
pub const MAX_MARKING_ITEMS: usize = 8;
/// Bytes of one text template.
pub const MAX_TEMPLATE: usize = 128;
/// Instance variables (`date_code`, `lot`) one instance sets.
pub const MAX_MARKING_VARS: usize = 8;
/// Bytes of one instance variable name or value.
pub const MAX_MARKING_VAR: usize = 32;
/// Smallest legible text height by default, mm (an estimate for laser
/// marking on a 0603; data, not a rule).
pub const DEFAULT_MIN_TEXT_HEIGHT_MM: f64 = 0.3;
/// Advance of one character as a fraction of the text height: the box
/// width a laid line may use is `chars × TEXT_ADVANCE × height_mm`. D5
/// scales glyph outlines to stay inside it.
pub const TEXT_ADVANCE: f64 = 0.6;
/// The gap between stacked lines as a fraction of the minimum height.
pub const LINE_GAP: f64 = 0.5;
/// The clear margin around the marking area's edge as a fraction of its
/// smaller side.
pub const AREA_MARGIN: f64 = 0.05;
/// Bytes of one rendered line.
pub const MAX_RENDERED_LINE: usize = 128;
/// Largest marking area side, mm (bounds arithmetic, not a rule).
pub const MAX_AREA_MM: f64 = 1000.0;
/// Media type of packed marking artwork.
pub const ARTWORK_MEDIA_TYPE: &str = "image/svg+xml";
/// Bytes of one packed artwork file (the SVG reader's limit).
pub const MAX_ARTWORK_BYTES: usize = 1 << 20;

/// A text template over fields: `{mpn}`, `{manufacturer}`,
/// `{field:<id>}`, `{field:<id>|code3}`, `{var:date_code}`,
/// `{designator}`. Checked at publish (`marking.template`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct Template(pub String);

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Centre,
    Start,
    End,
}

/// Where in the marking area an item sits.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    Centre,
    /// The pin-1 corner of the area.
    Pin1Corner,
    /// The −x (anode, pin 1) end.
    Start,
    End,
}

/// A polarity mark's shape.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolarityMark {
    Dot,
    Bar,
    Band,
}

/// One item of a marking.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "item", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkingItem {
    /// Auto-fit to the area when `height_mm` is `None`.
    Text {
        template: Template,
        #[serde(default)]
        height_mm: Option<f64>,
        #[serde(default)]
        align: Align,
    },
    /// A generated value code ([`codes`]).
    Code { scheme: CodeScheme, field: FieldRef },
    /// User-supplied SVG artwork, packed by hash in `Library::assets` as
    /// [`ARTWORK_MEDIA_TYPE`] (`library.pack_artwork`), licence-gated like
    /// models: publishing needs the artwork packed and its licence stated
    /// (`marking.artwork`). Pcbcraft ships no vendor logos.
    Artwork {
        svg: ContentHash,
        size_mm: [f64; 2],
        #[serde(default)]
        anchor: Anchor,
        /// The artwork's licence (an SPDX id or the owner's terms), at
        /// most [`MAX_LABEL`] bytes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        licence: Option<String>,
    },
    Polarity {
        mark: PolarityMark,
        #[serde(default)]
        at: Anchor,
    },
    /// Axial THT colour bands (IEC 60062).
    Bands {
        value: FieldRef,
        #[serde(default)]
        tolerance: Option<FieldRef>,
    },
}

/// The marking area: the model's (generated family top face or
/// `ModelDefinition::marking_area`), or an explicit rectangle on the top
/// face in the package frame.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "area", rename_all = "snake_case", deny_unknown_fields)]
pub enum AreaChoice {
    #[default]
    FromModel,
    Explicit {
        centre: [f64; 2],
        size_mm: [f64; 2],
    },
}

/// How the marking is applied, which picks its display material.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    /// `EpoxyLaserMarked`.
    #[default]
    LaserEtch,
    /// `MarkingInkWhite`. Further inks need a cited material first.
    InkWhite,
}

impl Finish {
    pub fn material(self) -> MaterialId {
        match self {
            Self::LaserEtch => MaterialId::EpoxyLaserMarked,
            Self::InkWhite => MaterialId::MarkingInkWhite,
        }
    }
}

/// A component's generic marking or a Part's exact top mark.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarkingSpec {
    /// At most [`MAX_MARKING_ITEMS`].
    pub items: Vec<MarkingItem>,
    #[serde(default)]
    pub area: AreaChoice,
    #[serde(default)]
    pub finish: Finish,
    /// Smallest legible text height, mm; [`DEFAULT_MIN_TEXT_HEIGHT_MM`]
    /// when `None`.
    #[serde(default)]
    pub min_text_height_mm: Option<f64>,
}

impl MarkingSpec {
    /// Persistence bounds.
    pub fn within_bounds(&self) -> bool {
        self.items.len() <= MAX_MARKING_ITEMS
            && self.items.iter().all(|item| match item {
                MarkingItem::Text { template, .. } => template.0.len() <= MAX_TEMPLATE,
                MarkingItem::Artwork { licence, .. } => {
                    licence.as_ref().is_none_or(|l| l.len() <= MAX_LABEL)
                }
                _ => true,
            })
    }

    /// The artwork hashes the spec names, in item order.
    pub fn artwork(&self) -> impl Iterator<Item = ContentHash> + '_ {
        self.items.iter().filter_map(|item| match item {
            MarkingItem::Artwork { svg, .. } => Some(*svg),
            _ => None,
        })
    }
}

/// Whether `library` packs `hash` as marking artwork: [`ARTWORK_MEDIA_TYPE`],
/// at most [`MAX_ARTWORK_BYTES`], UTF-8 text.
pub fn artwork_packed(library: &super::Library, hash: &ContentHash) -> Option<bool> {
    let asset = library.assets.iter().find(|a| a.hash == *hash)?;
    Some(
        asset.media_type == ARTWORK_MEDIA_TYPE
            && asset.data.len() <= MAX_ARTWORK_BYTES
            && std::str::from_utf8(asset.data.as_slice()).is_ok(),
    )
}

/// Publish findings of a spec's artwork against `library`
/// (`marking.artwork`, blocking): each artwork packed as SVG text and its
/// licence stated, as a model's must be.
pub fn artwork_findings(
    library: &super::Library,
    owner: DefinitionId,
    spec: &MarkingSpec,
) -> Vec<Finding> {
    let mut problems = Vec::new();
    for (index, item) in spec.items.iter().enumerate() {
        let MarkingItem::Artwork { svg, licence, .. } = item else {
            continue;
        };
        let n = index + 1;
        match artwork_packed(library, svg) {
            None => problems.push(format!(
                "item {n}: artwork {svg} is not packed in the project; import the SVG again"
            )),
            Some(false) => problems.push(format!(
                "item {n}: asset {svg} is not SVG artwork ({ARTWORK_MEDIA_TYPE} text of at most {} KiB)",
                MAX_ARTWORK_BYTES >> 10
            )),
            Some(true) => {}
        }
        if licence.as_deref().map(str::trim).is_none_or(str::is_empty) {
            problems.push(format!("item {n}: state the artwork's licence"));
        }
    }
    problems
        .into_iter()
        .map(|problem| {
            Finding::blocking(
                "marking.artwork",
                format!("Marking: {problem}."),
                vec![ObjectRef::Definition { id: owner }],
                Stage::Publish,
            )
        })
        .collect()
}

/// Snapshot rule of markings (`Library::validate`): every published
/// component marking's and Part top mark's artwork is packed as SVG.
pub fn snapshot_problems(library: &super::Library) -> Result<()> {
    let specs = library
        .components
        .iter()
        .filter_map(|c| c.definition.marking.as_deref().map(|s| (&c.meta.name, s)))
        .chain(
            library
                .parts
                .iter()
                .filter_map(|p| p.definition.top_mark.as_deref().map(|s| (&p.meta.name, s))),
        );
    for (name, spec) in specs {
        for hash in spec.artwork() {
            if artwork_packed(library, &hash) != Some(true) {
                return Err(crate::diagnostics::Error::Invalid(format!(
                    "marking.artwork: the marking of ‘{name}’ names artwork {hash}, which the project does not pack as SVG"
                )));
            }
        }
    }
    Ok(())
}

/// An instance's marking choices.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstanceMarking {
    #[serde(default)]
    pub hide: bool,
    /// Template variables (`date_code`, `lot`): at most
    /// [`MAX_MARKING_VARS`] of at most [`MAX_MARKING_VAR`] bytes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<MarkingSpec>,
}

impl InstanceMarking {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Persistence bounds: the variable count and sizes, variable names,
    /// and the replacing spec's bounds.
    pub fn within_bounds(&self) -> bool {
        self.vars.len() <= MAX_MARKING_VARS
            && self.vars.iter().all(|(name, value)| {
                template::var_name_valid(name)
                    && value.len() <= MAX_MARKING_VAR
                    && !value.chars().any(char::is_control)
            })
            && self.replace.as_ref().is_none_or(MarkingSpec::within_bounds)
    }
}

/// One laid-out line of text, in the area's 2D frame (mm, origin at the
/// area centre, +x along the area's u axis, +y towards the pin-1 side).
/// `origin` is the bottom-left corner of the line's box, which is
/// `text.chars().count() × TEXT_ADVANCE × height_mm` wide and `height_mm`
/// tall.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LaidText {
    pub text: String,
    pub height_mm: f64,
    pub origin: [f64; 2],
}

/// One laid-out element of a marking.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "element", rename_all = "snake_case")]
pub enum LaidElement {
    Text(LaidText),
    /// `origin` is the artwork's centre.
    Artwork {
        svg: ContentHash,
        origin: [f64; 2],
        size_mm: [f64; 2],
    },
    /// `origin` is the mark's centre; a dot is a circle of diameter
    /// `size_mm[0]`, a bar and a band are rectangles.
    Polarity {
        mark: PolarityMark,
        origin: [f64; 2],
        size_mm: [f64; 2],
    },
    /// Colour bands from the −x end: IEC 60062 colour indices
    /// ([`codes::BAND_COLOURS`]). Band `i` of `n` is centred at
    /// `x = (i − (n − 1) / 2) × pitch_mm` and spans the area's height.
    Bands {
        colours: Vec<u8>,
        pitch_mm: f64,
    },
}

/// The resolved marking of one instance: what D5 draws.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarkingLayout {
    pub elements: Vec<LaidElement>,
    pub finish: Finish,
    /// Advisory findings (`marking.unrepresentable`, `marking.does_not_fit`).
    pub findings: Vec<Finding>,
}

/// What a marking reads: the resolved field texts and values the layout
/// needs, gathered by the caller (instances, D4).
#[derive(Clone, Debug, Default)]
pub struct MarkingInputs {
    pub designator: String,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    /// Field values by field lineage, as typed text and (for quantities)
    /// the SI value.
    pub fields: BTreeMap<DefinitionId, (String, Option<f64>)>,
    pub vars: BTreeMap<String, String>,
    /// The marking area's size, mm.
    pub area_mm: [f64; 2],
}

/// The spec that applies: instance `replace`, then Part, then component.
pub fn effective<'a>(
    instance: &'a InstanceMarking,
    part: Option<&'a MarkingSpec>,
    component: Option<&'a MarkingSpec>,
) -> Option<&'a MarkingSpec> {
    if instance.hide {
        return None;
    }
    instance.replace.as_ref().or(part).or(component)
}

/// Lay out `spec` for `inputs`: text lines stacked and auto-fitted in the
/// area, codes chosen (an `Auto` list picks the first representable code
/// that fits at the minimum legible height), polarity marks, artwork and
/// colour bands placed. An item that cannot be represented or does not fit
/// draws nothing and adds an Info finding; findings name no objects (the
/// caller adds the instance). Errors only on non-finite inputs.
pub fn layout(spec: &MarkingSpec, inputs: &MarkingInputs) -> Result<MarkingLayout> {
    layout::run(spec, inputs)
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0 && value <= MAX_AREA_MM
}

/// Publish findings of a marking spec (`marking.template`): bounds,
/// template syntax, code schemes and sizes. They block publishing.
pub fn spec_findings(owner: DefinitionId, spec: &MarkingSpec) -> Vec<Finding> {
    let mut problems = Vec::new();
    if spec.items.len() > MAX_MARKING_ITEMS {
        problems.push(format!("a marking lists at most {MAX_MARKING_ITEMS} items"));
    }
    if spec.min_text_height_mm.is_some_and(|h| !positive(h)) {
        problems.push("the minimum text height must be a positive size".into());
    }
    if let AreaChoice::Explicit { centre, size_mm } = spec.area
        && (!size_mm.iter().all(|s| positive(*s))
            || !centre
                .iter()
                .all(|c| c.is_finite() && c.abs() <= MAX_AREA_MM))
    {
        problems.push("the explicit marking area needs a finite centre and positive size".into());
    }
    for (index, item) in spec.items.iter().enumerate() {
        let n = index + 1;
        match item {
            MarkingItem::Text {
                template,
                height_mm,
                ..
            } => {
                if template.0.len() > MAX_TEMPLATE {
                    problems.push(format!(
                        "item {n}: a template is at most {MAX_TEMPLATE} bytes"
                    ));
                } else if let Err(problem) = template::parse(&template.0) {
                    problems.push(format!("item {n}: {problem} in \"{}\"", template.0));
                }
                if height_mm.is_some_and(|h| !positive(h)) {
                    problems.push(format!("item {n}: the text height must be a positive size"));
                }
            }
            MarkingItem::Code { scheme, .. } => {
                if let Some(problem) = scheme.problem() {
                    problems.push(format!("item {n}: {problem}"));
                }
            }
            MarkingItem::Artwork {
                size_mm, licence, ..
            } => {
                if !size_mm.iter().all(|s| positive(*s)) {
                    problems.push(format!("item {n}: artwork needs a positive size"));
                }
                if licence
                    .as_ref()
                    .is_some_and(|l| l.len() > MAX_LABEL || l.chars().any(char::is_control))
                {
                    problems.push(format!(
                        "item {n}: an artwork licence is at most {MAX_LABEL} bytes of text"
                    ));
                }
            }
            MarkingItem::Polarity { .. } | MarkingItem::Bands { .. } => {}
        }
    }
    problems
        .into_iter()
        .map(|problem| {
            Finding::blocking(
                "marking.template",
                format!("Marking: {problem}."),
                vec![ObjectRef::Definition { id: owner }],
                Stage::Publish,
            )
        })
        .collect()
}

/// The marking area size a package's model offers, mm: a library model's
/// `marking_area`, else the top of the typed body (a chip between its
/// terminations, a leaded or no-lead body's moulding, otherwise the
/// envelope's length and width). `None` when there is none.
pub fn package_area(
    library: &super::Library,
    package: &super::PackageDefinition,
) -> Option<[f64; 2]> {
    use super::package_form::PackageForm;
    use super::package_model::PackageModel;
    let size = match (&package.model, &package.form) {
        (PackageModel::Library(model), _) => library
            .model(model.model)
            .and_then(|m| m.definition.marking_area)
            .map(|area| area.size_mm)?,
        (_, PackageForm::Chip(c)) => [c.length - 2.0 * c.termination, c.width],
        (_, PackageForm::GullWing(g)) => [g.body_length, g.body_width],
        (_, PackageForm::NoLead(n)) => [n.body_length, n.body_width],
        (_, form) => {
            let [l, w, _] = form.envelope().unwrap_or(package.envelope);
            [l, w]
        }
    };
    size.iter().all(|s| positive(*s)).then_some(size)
}
