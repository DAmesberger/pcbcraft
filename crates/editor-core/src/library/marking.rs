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
pub mod matrix;
pub mod surface;
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
pub use matrix::{Ecc, Symbol, Symbology};
pub use surface::{BoxFace, Chart, Placement, SurfaceGeometry, SurfaceRef, Wrap};

/// Further decals one marking spec carries, each on its own surface.
pub const MAX_DECALS: usize = 4;
/// Largest generated 2D code side, mm.
pub const MAX_CODE_MM: f64 = 50.0;

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
    /// An electrolytic's negative stripe: a band with minus signs left
    /// open in it.
    Stripe,
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
    /// A generated 2D code (QR) whose payload is a text template over the
    /// instance's fields (`{designator}`, `{field:<id>}`, `{var:lot}`),
    /// `size_mm` the symbol's side without its quiet zone.
    Code2d {
        #[serde(default)]
        symbology: Symbology,
        payload: Template,
        size_mm: f64,
        #[serde(default)]
        anchor: Anchor,
        #[serde(default)]
        ecc: Ecc,
    },
    /// An aluminium electrolytic's marking from its values: the
    /// capacitance in µF and the rated voltage as text lines, and the
    /// negative stripe with minus signs on the negative lead's side
    /// (lead 2, +x; angle 0 of a radial sleeve).
    Electrolytic {
        capacitance: FieldRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voltage: Option<FieldRef>,
        #[serde(default = "default_true")]
        stripe: bool,
    },
}

fn default_true() -> bool {
    true
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

/// How the marking is applied, which picks its display material and how
/// far it stands off its face.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    /// Laser marking on a moulded body: `EpoxyLaserMarked`.
    #[default]
    LaserEtch,
    /// Laser marking on metal (a shield can): `MetalLaserMarked`, darker,
    /// matte and slightly rough, still metal, so it follows the can's
    /// lighting.
    LaserEtchMetal,
    /// White marking ink or silkscreen: `MarkingInkWhite`, raised.
    InkWhite,
    /// Ink or silkscreen of a chosen sRGB colour: `MarkingInkWhite`'s
    /// optics with that colour, raised.
    Ink { colour: [u8; 3] },
    /// Pad-printed ink of a chosen sRGB colour: `PadPrintInk`'s optics
    /// (thinner, glossier than screen ink).
    PadPrint { colour: [u8; 3] },
    /// Engraved: no material of its own; the face's own look with its
    /// walls shaded by tilted normals (bump only, no geometry).
    Engraved,
}

impl Finish {
    /// The cited material whose optics the finish takes ([`Self::colour`]
    /// replaces its colour). An engraving takes its face's material; this
    /// is the look used where that is unknown.
    pub fn material(self) -> MaterialId {
        match self {
            Self::LaserEtch | Self::Engraved => MaterialId::EpoxyLaserMarked,
            Self::LaserEtchMetal => MaterialId::MetalLaserMarked,
            Self::InkWhite | Self::Ink { .. } => MaterialId::MarkingInkWhite,
            Self::PadPrint { .. } => MaterialId::PadPrintInk,
        }
    }

    /// The chosen sRGB colour replacing the material's.
    pub fn colour(self) -> Option<[u8; 3]> {
        match self {
            Self::Ink { colour } | Self::PadPrint { colour } => Some(colour),
            _ => None,
        }
    }

    /// How far the decal stands off its face, mm: an etch or engraving
    /// is drawn just clear of it (no depth of its own, 5 µm), screen ink
    /// on a raised layer of a typical legend thickness (est. 15 µm), pad
    /// print thinner (est. 8 µm).
    pub fn lift_mm(self) -> f64 {
        match self {
            Self::LaserEtch | Self::LaserEtchMetal | Self::Engraved => 0.005,
            Self::InkWhite | Self::Ink { .. } => 0.015,
            Self::PadPrint { .. } => 0.008,
        }
    }

    /// A short name for the UI and messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::LaserEtch => "Laser etched",
            Self::LaserEtchMetal => "Laser etched (metal)",
            Self::InkWhite => "White ink",
            Self::Ink { .. } => "Ink",
            Self::PadPrint { .. } => "Pad print",
            Self::Engraved => "Engraved",
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
    /// The surface the marking maps onto and how it is placed there
    /// ([`surface`]); the model's top marking area when default.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// Further decals of the same part, each its own items, surface and
    /// finish (a can marking and a silkscreen on the module board): at
    /// most [`MAX_DECALS`], and they carry no decals of their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decals: Vec<MarkingSpec>,
}

impl MarkingSpec {
    /// Persistence bounds.
    pub fn within_bounds(&self) -> bool {
        self.items.len() <= MAX_MARKING_ITEMS
            && self.items.iter().all(|item| match item {
                MarkingItem::Text { template, .. } => template.0.len() <= MAX_TEMPLATE,
                MarkingItem::Code2d { payload, .. } => payload.0.len() <= MAX_TEMPLATE,
                MarkingItem::Artwork { licence, .. } => {
                    licence.as_ref().is_none_or(|l| l.len() <= MAX_LABEL)
                }
                _ => true,
            })
            && self.decals.len() <= MAX_DECALS
            && self
                .decals
                .iter()
                .all(|d| d.decals.is_empty() && d.within_bounds())
    }

    /// The artwork hashes the spec and its decals name, in item order.
    pub fn artwork(&self) -> impl Iterator<Item = ContentHash> + '_ {
        self.all()
            .flat_map(|spec| spec.items.iter())
            .filter_map(|item| match item {
                MarkingItem::Artwork { svg, .. } => Some(*svg),
                _ => None,
            })
    }

    /// This spec and its decals, the spec first.
    pub fn all(&self) -> impl Iterator<Item = &MarkingSpec> + '_ {
        std::iter::once(self).chain(self.decals.iter())
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
    let items = spec.all().flat_map(|s| s.items.iter());
    for (index, item) in items.enumerate() {
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
    /// A generated 2D code: `origin` is the symbol's centre, each module
    /// `module_mm` square; row 0 of `symbol` is at +y.
    Matrix {
        symbol: Symbol,
        origin: [f64; 2],
        module_mm: f64,
    },
    /// `origin` is the artwork's centre.
    Artwork {
        svg: ContentHash,
        origin: [f64; 2],
        size_mm: [f64; 2],
    },
    /// `origin` is the mark's centre; a dot is a circle of diameter
    /// `size_mm[0]`, a bar, a band and a stripe are rectangles (a stripe
    /// with minus signs open in it). On a full-turn wrap a stripe may
    /// reach past the chart's edge: it continues round the seam.
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
    /// Where the layout goes ([`MarkingSpec::placement`]).
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// The layouts of the spec's further decals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decals: Vec<MarkingLayout>,
}

impl MarkingLayout {
    /// This layout and its decals' layouts.
    pub fn all(&self) -> impl Iterator<Item = &MarkingLayout> + '_ {
        std::iter::once(self).chain(self.decals.iter())
    }

    /// Whether nothing at all is drawn.
    pub fn is_blank(&self) -> bool {
        self.all().all(|l| l.elements.is_empty())
    }
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
    /// The chart the area lies in (flat, or round a cylinder).
    pub chart: Chart,
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

/// Lay out `spec` and its decals, each in the area and chart `region`
/// gives for it (the region of its surface; [`Placement::layout_area`] is
/// applied here). A decal whose surface `region` cannot find lays out in
/// no area and says so.
pub fn layout_with(
    spec: &MarkingSpec,
    inputs: &MarkingInputs,
    region: &dyn Fn(&MarkingSpec) -> Option<([f64; 2], Chart)>,
) -> Result<MarkingLayout> {
    let one = |s: &MarkingSpec| -> Result<MarkingLayout> {
        let mut inputs = inputs.clone();
        let (area, chart) = region(s).unwrap_or(([0.0, 0.0], Chart::Flat));
        inputs.area_mm = s.placement.layout_area(area);
        // A turned or mirrored layout is no longer square to the wrap's
        // angles: its marks are placed as on a flat chart.
        inputs.chart = match chart {
            Chart::Wrap {
                radius_mm,
                centre,
                span,
                along_u,
            } if s.placement.rotation_deg.abs() < 1e-9 && !s.placement.mirror => Chart::Wrap {
                radius_mm: radius_mm / s.placement.scale.max(1e-9),
                centre,
                span,
                along_u,
            },
            _ => Chart::Flat,
        };
        let mut laid = layout::run(s, &inputs)?;
        laid.placement = s.placement.clone();
        Ok(laid)
    };
    let mut out = one(spec)?;
    for decal in spec.decals.iter().take(MAX_DECALS) {
        let mut laid = one(decal)?;
        out.findings.append(&mut laid.findings);
        out.decals.push(laid);
    }
    Ok(out)
}

/// The region and chart of `placement`'s surface on `package`: the model
/// area for [`SurfaceRef::Model`] (`area`'s explicit size, else
/// [`package_area`]), a box face of the form's envelope, or a named
/// anchor ([`surface::anchors`]).
pub fn surface_region(
    library: &super::Library,
    package: Option<&super::PackageDefinition>,
    area: AreaChoice,
    placement: &Placement,
) -> Option<([f64; 2], Chart)> {
    match &placement.on {
        SurfaceRef::Model => match area {
            AreaChoice::Explicit { size_mm, .. } => Some((size_mm, Chart::Flat)),
            AreaChoice::FromModel => package
                .and_then(|p| package_area(library, p))
                .map(|a| (a, Chart::Flat)),
        },
        SurfaceRef::Face { face } => {
            let package = package?;
            let envelope = package.form.envelope().unwrap_or(package.envelope);
            let size = face.of_box(envelope).size_mm;
            size.iter()
                .all(|s| positive(*s))
                .then_some((size, Chart::Flat))
        }
        SurfaceRef::Anchor { name } => {
            let found = surface::anchor_of(&package?.form, name)?;
            surface::region(&found.geometry, placement)
        }
    }
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0 && value <= MAX_AREA_MM
}

/// Publish findings of a marking spec (`marking.template`): bounds,
/// template syntax, code schemes and sizes. They block publishing.
pub fn spec_findings(owner: DefinitionId, spec: &MarkingSpec) -> Vec<Finding> {
    let mut problems = Vec::new();
    if spec.decals.len() > MAX_DECALS {
        problems.push(format!(
            "a marking carries at most {MAX_DECALS} further decals"
        ));
    }
    if spec.decals.iter().any(|d| !d.decals.is_empty()) {
        problems.push("a marking's decals carry no decals of their own".into());
    }
    for (k, s) in spec.all().enumerate() {
        let whose = if k == 0 {
            String::new()
        } else {
            format!("decal {k}: ")
        };
        problems.extend(
            one_spec_problems(s)
                .into_iter()
                .map(|p| format!("{whose}{p}")),
        );
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

fn one_spec_problems(spec: &MarkingSpec) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(problem) = spec.placement.problem() {
        problems.push(problem);
    }
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
            MarkingItem::Code2d {
                payload, size_mm, ..
            } => {
                if payload.0.len() > MAX_TEMPLATE {
                    problems.push(format!(
                        "item {n}: a payload template is at most {MAX_TEMPLATE} bytes"
                    ));
                } else if let Err(problem) = template::parse(&payload.0) {
                    problems.push(format!("item {n}: {problem} in \"{}\"", payload.0));
                }
                if !(size_mm.is_finite() && *size_mm > 0.0 && *size_mm <= MAX_CODE_MM) {
                    problems.push(format!(
                        "item {n}: a 2D code's side is a positive size of at most {MAX_CODE_MM} mm"
                    ));
                }
            }
            MarkingItem::Polarity { .. }
            | MarkingItem::Bands { .. }
            | MarkingItem::Electrolytic { .. } => {}
        }
    }
    problems
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
        (_, PackageForm::Module(m)) => m.can_size,
        (_, form) => {
            let [l, w, _] = form.envelope().unwrap_or(package.envelope);
            [l, w]
        }
    };
    size.iter().all(|s| positive(*s)).then_some(size)
}
