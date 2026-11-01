//! Slice D4 (contract C0): instance markings, display-only vector decals
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

use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{ContentHash, DefinitionId, FieldRef};
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
    /// User-supplied SVG artwork, packed by hash, licence-gated like
    /// models. Pcbcraft ships no vendor logos.
    Artwork {
        svg: ContentHash,
        size_mm: [f64; 2],
        #[serde(default)]
        anchor: Anchor,
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
                _ => true,
            })
    }
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
}

/// One laid-out line of text, in the area's 2D frame (mm, origin at the
/// area centre, +x along the area's u axis).
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
    Artwork {
        svg: ContentHash,
        origin: [f64; 2],
        size_mm: [f64; 2],
    },
    Polarity {
        mark: PolarityMark,
        origin: [f64; 2],
        size_mm: [f64; 2],
    },
    /// Colour bands from the −x end: IEC 60062 colour indices.
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

/// Lay out `spec` for `inputs`. Contract stub until D4.
pub fn layout(_spec: &MarkingSpec, _inputs: &MarkingInputs) -> Result<MarkingLayout> {
    Err(Error::Unsupported(
        "marking.unsupported: instance markings are not available yet (slice D4)".into(),
    ))
}

/// Publish findings of a marking spec (`marking.template`, bounds).
/// Contract: any non-empty spec is refused with `marking.unsupported`
/// until D4.
pub fn spec_findings(owner: DefinitionId, spec: &MarkingSpec) -> Vec<Finding> {
    if spec.items.is_empty() {
        return Vec::new();
    }
    vec![Finding::blocking(
        "marking.unsupported",
        "Markings cannot be published yet.".into(),
        vec![ObjectRef::Definition { id: owner }],
        Stage::Publish,
    )]
}
