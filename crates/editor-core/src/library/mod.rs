//! Library definitions, the project's immutable snapshot store, drafts and
//! publication. Definition bodies are plain data; the publish rules for each
//! kind live beside its type (field and quantity-kind rules in `fields`) and
//! are aggregated by [`publish_findings`], the one function `library.publish`,
//! document decode and `component.check` use.
mod binding;
pub mod bundle;
mod component;
pub mod drafts;
mod footprint;
pub mod footprint_edit;
pub mod footprint_source;
pub mod graphics;
pub mod import;
pub mod marking;
pub mod mechanical;
pub mod model3d;
pub mod names;
mod package;
pub mod package_fit;
pub mod package_form;
pub mod package_model;
pub mod pad_table;
mod part;
pub mod pinout;
mod publish;
pub mod samples;
pub mod standard_fields;
mod symbol;
pub mod symbol_style;
pub mod templates;

pub use binding::{BindingDefinition, TerminalMap};
pub use bundle::{Bundle, OriginRecord, PackedAsset, RevisionRecord, SourceRevision};
pub use component::{
    ComponentDefinition, MAX_DESIGNATOR_PREFIX, Terminal, TerminalClass, designator_prefix_valid,
};
pub use drafts::{Draft, DraftEdit, Drafts};
pub use footprint::{
    ApertureId, CHECK_CHORD_MM, CornerMask, Datum, FootprintDefinition, FootprintKeepout, Hole,
    HoleShape, KeepoutScope, MASK_EXPANSION_MM, MAX_FOOTPRINT_KEEPOUTS, MAX_OUTLINE_SEGMENTS,
    MAX_PAD_VIAS, MAX_PASTE_WINDOWS, MIN_MASK_WEB_MM, MIN_SILK_TEXT_HEIGHT_MM, MIN_SILK_WIDTH_MM,
    MaskRule, Outline, PAD_ARC_TOLERANCE_MM, Pad, PadDefaults, PadKind, PadShape, PadSide, PadVia,
    PasteRule, PasteWindow, Segment, TerminalRule, bulge_arc, offset_polygon,
};
pub use footprint_source::{FootprintEvaluator, FootprintSource};
pub use graphics::{
    Decorations, DrawingLayer, Fill, FootprintGraphic, Graphic, Grid, HAlign, Item, Justify, Pin,
    PinDirection, PinText, PlacedText, Polarity, Rotation, Side, SymbolHit, Text, TextContent,
    TextLine, TextMetric, TextSize, VAlign, designator_placeholder, lint, lint_with,
};
pub use model3d::{LibraryModel, ModelDefinition};
pub use package::{PackageDefinition, PackageTerminal};
pub use package_form::{
    ChipForm, ChipKind, GullWingForm, HeaderForm, LeadSides, NoLeadForm, PackageForm,
};
pub use package_model::{
    Axis, BodyRole, GeneratedMaterials, ModelPlacement, PackageEdit, PackageModel, QuickAction,
    RoleMaterial, model_frame,
};
pub use part::{
    Basis, DistributorOffer, MAX_OFFERS, MAX_REFERENCES, MAX_URL, Origin, PartDefinition,
    Provenance, Reference, ReferenceKind,
};
pub use publish::{
    MAX_PUBLISH_SET, candidate, pending_revision, publish, publish_findings, publish_set, stageable,
};
pub use symbol::{SymbolDefinition, Unit};
pub use templates::catalog::PackageRowDefinition;

use crate::diagnostics::{Error, Result};
use crate::fields::{FieldDefinition, QuantityKind};
use crate::ids::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, FieldRef, FootprintRef,
    ModelRef, PackageRef, PackageRowRef, PartRef, QuantityKindRef, Revision, SymbolRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Published revisions per kind.
pub const MAX_REVISIONS_PER_KIND: usize = 4096;
/// Names, labels and designations.
pub use crate::ids::MAX_LABEL;
/// Descriptions and provenance notes.
pub const MAX_NOTE: usize = 4096;
/// Terminals of one component (raised for MCUs, slice C0; a target).
pub const MAX_TERMINALS: usize = 2048;
pub const MAX_UNITS: usize = 64;
pub const MAX_PACKAGE_TERMINALS: usize = 2048;
pub const MAX_PADS: usize = 4096;
pub const MAX_FOOTPRINTS_PER_BINDING: usize = 16;
/// Representations or implementations listed by one component revision.
pub const MAX_COMPONENT_LISTS: usize = 64;

/// The envelope every definition kind shares. Bodies carry no metadata.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefinitionMeta {
    pub name: String,
    pub description: String,
    /// Free text. Never a verification stamp.
    pub provenance: String,
}

/// A definition body of any kind. Drafts and published revisions use the same
/// types; drafts may be incomplete.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum Definition {
    Component(ComponentDefinition),
    Symbol(SymbolDefinition),
    Package(PackageDefinition),
    Footprint(FootprintDefinition),
    Binding(BindingDefinition),
    QuantityKind(QuantityKind),
    Field(FieldDefinition),
    /// Boxed: a Part body is several times larger than the others.
    Part(Box<PartDefinition>),
    /// D1: a 3D library model.
    Model(Box<ModelDefinition>),
    /// B1: a package-family dimension-table row.
    PackageRow(Box<PackageRowDefinition>),
}

impl Definition {
    pub fn kind(&self) -> DefinitionKind {
        match self {
            Self::Component(_) => DefinitionKind::Component,
            Self::Symbol(_) => DefinitionKind::Symbol,
            Self::Package(_) => DefinitionKind::Package,
            Self::Footprint(_) => DefinitionKind::Footprint,
            Self::Binding(_) => DefinitionKind::Binding,
            Self::QuantityKind(_) => DefinitionKind::QuantityKind,
            Self::Field(_) => DefinitionKind::Field,
            Self::Part(_) => DefinitionKind::Part,
            Self::Model(_) => DefinitionKind::Model,
            Self::PackageRow(_) => DefinitionKind::PackageRow,
        }
    }
    /// An empty body of `kind`, for a new lineage authored step by step.
    pub fn empty(kind: DefinitionKind) -> Self {
        match kind {
            DefinitionKind::Component => Self::Component(Default::default()),
            DefinitionKind::Symbol => Self::Symbol(Default::default()),
            DefinitionKind::Package => Self::Package(Default::default()),
            DefinitionKind::Footprint => Self::Footprint(Default::default()),
            DefinitionKind::Binding => Self::Binding(Default::default()),
            DefinitionKind::QuantityKind => Self::QuantityKind(Default::default()),
            DefinitionKind::Field => Self::Field(Default::default()),
            DefinitionKind::Part => Self::Part(Default::default()),
            DefinitionKind::Model => Self::Model(Default::default()),
            DefinitionKind::PackageRow => Self::PackageRow(Default::default()),
        }
    }
}

/// One immutable published revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Published<T> {
    pub id: DefinitionId,
    pub revision: Revision,
    pub meta: DefinitionMeta,
    pub definition: T,
}

/// Every published revision of every kind. Revisions are appended, never
/// rewritten, and every exact reference resolves inside this store, so a
/// project reopens offline with no other source.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub components: Vec<Published<ComponentDefinition>>,
    pub symbols: Vec<Published<SymbolDefinition>>,
    pub packages: Vec<Published<PackageDefinition>>,
    pub footprints: Vec<Published<FootprintDefinition>>,
    pub bindings: Vec<Published<BindingDefinition>>,
    pub quantity_kinds: Vec<Published<QuantityKind>>,
    pub fields: Vec<Published<FieldDefinition>>,
    /// Manufacturer parts (S2), published after the components they pin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Published<PartDefinition>>,
    /// D1: 3D library models, published before the packages that use them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Published<ModelDefinition>>,
    /// B1: package-family dimension-table rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_rows: Vec<Published<PackageRowDefinition>>,
    /// Where imported or promoted revisions came from or went to (S3, D1).
    /// Outside revision bodies: recording an origin is not a new revision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<OriginRecord>,
    /// Asset bytes packed by content hash (S3, D7), so the project reopens
    /// offline with attached copies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<PackedAsset>,
}

/// A field revision with the quantity kind its value type names, if any.
#[derive(Clone, Copy)]
pub struct FieldView<'a> {
    pub field: &'a Published<FieldDefinition>,
    pub kind: Option<&'a Published<QuantityKind>>,
}

impl FieldView<'_> {
    pub fn kind_body(&self) -> Option<&QuantityKind> {
        self.kind.map(|k| &k.definition)
    }
}

fn find<T>(items: &[Published<T>], id: DefinitionId, revision: Revision) -> Option<&Published<T>> {
    items.iter().find(|p| p.id == id && p.revision == revision)
}

fn latest_of<T>(items: &[Published<T>], id: DefinitionId) -> Option<&Published<T>> {
    items
        .iter()
        .filter(|p| p.id == id)
        .max_by_key(|p| p.revision)
}

impl Library {
    pub fn component(&self, r: ComponentRef) -> Option<&Published<ComponentDefinition>> {
        find(&self.components, r.id, r.revision)
    }
    pub fn symbol(&self, r: SymbolRef) -> Option<&Published<SymbolDefinition>> {
        find(&self.symbols, r.id, r.revision)
    }
    pub fn package(&self, r: PackageRef) -> Option<&Published<PackageDefinition>> {
        find(&self.packages, r.id, r.revision)
    }
    pub fn footprint(&self, r: FootprintRef) -> Option<&Published<FootprintDefinition>> {
        find(&self.footprints, r.id, r.revision)
    }
    pub fn binding(&self, r: BindingRef) -> Option<&Published<BindingDefinition>> {
        find(&self.bindings, r.id, r.revision)
    }
    pub fn quantity_kind(&self, r: QuantityKindRef) -> Option<&Published<QuantityKind>> {
        find(&self.quantity_kinds, r.id, r.revision)
    }
    pub fn field(&self, r: FieldRef) -> Option<&Published<FieldDefinition>> {
        find(&self.fields, r.id, r.revision)
    }
    pub fn part(&self, r: PartRef) -> Option<&Published<PartDefinition>> {
        find(&self.parts, r.id, r.revision)
    }
    pub fn model(&self, r: ModelRef) -> Option<&Published<ModelDefinition>> {
        find(&self.models, r.id, r.revision)
    }
    pub fn package_row(&self, r: PackageRowRef) -> Option<&Published<PackageRowDefinition>> {
        find(&self.package_rows, r.id, r.revision)
    }
    /// The latest published revision of each Part lineage, oldest lineage
    /// first.
    pub fn latest_parts(&self) -> Vec<&Published<PartDefinition>> {
        let ids: BTreeSet<DefinitionId> = self.parts.iter().map(|p| p.id).collect();
        ids.into_iter()
            .filter_map(|id| latest_of(&self.parts, id))
            .collect()
    }
    /// A field revision and the quantity kind it names. `None` when either
    /// is missing from the snapshot.
    pub fn field_view(&self, r: FieldRef) -> Option<FieldView<'_>> {
        let field = self.field(r)?;
        let kind = match field.definition.value_type.kind() {
            Some(k) => Some(self.quantity_kind(k)?),
            None => None,
        };
        Some(FieldView { field, kind })
    }

    /// The kind and latest revision of a lineage, if any revision is published.
    pub fn latest(&self, id: DefinitionId) -> Option<(DefinitionKind, Revision)> {
        [
            (
                DefinitionKind::Component,
                latest_of(&self.components, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Symbol,
                latest_of(&self.symbols, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Package,
                latest_of(&self.packages, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Footprint,
                latest_of(&self.footprints, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Binding,
                latest_of(&self.bindings, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::QuantityKind,
                latest_of(&self.quantity_kinds, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Field,
                latest_of(&self.fields, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Part,
                latest_of(&self.parts, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::Model,
                latest_of(&self.models, id).map(|p| p.revision),
            ),
            (
                DefinitionKind::PackageRow,
                latest_of(&self.package_rows, id).map(|p| p.revision),
            ),
        ]
        .into_iter()
        .find_map(|(kind, revision)| revision.map(|r| (kind, r)))
    }

    /// The latest published revision of each component lineage, oldest
    /// lineage first.
    pub fn latest_components(&self) -> Vec<&Published<ComponentDefinition>> {
        let ids: BTreeSet<DefinitionId> = self.components.iter().map(|p| p.id).collect();
        ids.into_iter()
            .filter_map(|id| latest_of(&self.components, id))
            .collect()
    }

    /// A published revision's envelope and body, cloned into the kind-tagged
    /// form drafts use.
    pub fn definition(&self, r: DefinitionRef) -> Option<(DefinitionMeta, Definition)> {
        fn take<T: Clone>(
            item: Option<&Published<T>>,
            wrap: fn(T) -> Definition,
        ) -> Option<(DefinitionMeta, Definition)> {
            item.map(|p| (p.meta.clone(), wrap(p.definition.clone())))
        }
        match r.kind {
            DefinitionKind::Component => take(
                find(&self.components, r.id, r.revision),
                Definition::Component,
            ),
            DefinitionKind::Symbol => {
                take(find(&self.symbols, r.id, r.revision), Definition::Symbol)
            }
            DefinitionKind::Package => {
                take(find(&self.packages, r.id, r.revision), Definition::Package)
            }
            DefinitionKind::Footprint => take(
                find(&self.footprints, r.id, r.revision),
                Definition::Footprint,
            ),
            DefinitionKind::Binding => {
                take(find(&self.bindings, r.id, r.revision), Definition::Binding)
            }
            DefinitionKind::QuantityKind => take(
                find(&self.quantity_kinds, r.id, r.revision),
                Definition::QuantityKind,
            ),
            DefinitionKind::Field => take(find(&self.fields, r.id, r.revision), Definition::Field),
            DefinitionKind::Part => take(find(&self.parts, r.id, r.revision), |d| {
                Definition::Part(Box::new(d))
            }),
            DefinitionKind::Model => take(find(&self.models, r.id, r.revision), |d| {
                Definition::Model(Box::new(d))
            }),
            DefinitionKind::PackageRow => take(find(&self.package_rows, r.id, r.revision), |d| {
                Definition::PackageRow(Box::new(d))
            }),
        }
    }

    /// Every published revision, as kind-tagged references.
    pub fn revisions(&self) -> Vec<DefinitionRef> {
        fn refs<T>(items: &[Published<T>], kind: DefinitionKind) -> Vec<DefinitionRef> {
            items
                .iter()
                .map(|p| DefinitionRef {
                    kind,
                    id: p.id,
                    revision: p.revision,
                })
                .collect()
        }
        let mut all = refs(&self.components, DefinitionKind::Component);
        all.extend(refs(&self.symbols, DefinitionKind::Symbol));
        all.extend(refs(&self.packages, DefinitionKind::Package));
        all.extend(refs(&self.footprints, DefinitionKind::Footprint));
        all.extend(refs(&self.bindings, DefinitionKind::Binding));
        all.extend(refs(&self.quantity_kinds, DefinitionKind::QuantityKind));
        all.extend(refs(&self.fields, DefinitionKind::Field));
        all.extend(refs(&self.parts, DefinitionKind::Part));
        all.extend(refs(&self.models, DefinitionKind::Model));
        all.extend(refs(&self.package_rows, DefinitionKind::PackageRow));
        all
    }

    /// Append one revision. Callers check revision order and findings.
    fn append(
        &mut self,
        id: DefinitionId,
        revision: Revision,
        meta: DefinitionMeta,
        body: Definition,
    ) {
        match body {
            Definition::Component(definition) => self.components.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Symbol(definition) => self.symbols.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Package(definition) => self.packages.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Footprint(definition) => self.footprints.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Binding(definition) => self.bindings.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::QuantityKind(definition) => self.quantity_kinds.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Field(definition) => self.fields.push(Published {
                id,
                revision,
                meta,
                definition,
            }),
            Definition::Part(definition) => self.parts.push(Published {
                id,
                revision,
                meta,
                definition: *definition,
            }),
            Definition::Model(definition) => self.models.push(Published {
                id,
                revision,
                meta,
                definition: *definition,
            }),
            Definition::PackageRow(definition) => self.package_rows.push(Published {
                id,
                revision,
                meta,
                definition: *definition,
            }),
        }
    }

    fn count(&self, kind: DefinitionKind) -> usize {
        match kind {
            DefinitionKind::Component => self.components.len(),
            DefinitionKind::Symbol => self.symbols.len(),
            DefinitionKind::Package => self.packages.len(),
            DefinitionKind::Footprint => self.footprints.len(),
            DefinitionKind::Binding => self.bindings.len(),
            DefinitionKind::QuantityKind => self.quantity_kinds.len(),
            DefinitionKind::Field => self.fields.len(),
            DefinitionKind::Part => self.parts.len(),
            DefinitionKind::Model => self.models.len(),
            DefinitionKind::PackageRow => self.package_rows.len(),
        }
    }
}

fn label_bounded(text: &str) -> bool {
    text.len() <= MAX_LABEL
}

/// Persistence bounds and finite geometry: the only checks a draft must pass.
/// Structurally contradictory but finite content stays visible draft input.
pub fn check_bounds(meta: &DefinitionMeta, definition: &Definition) -> Result<()> {
    if !label_bounded(&meta.name)
        || meta.description.len() > MAX_NOTE
        || meta.provenance.len() > MAX_NOTE
    {
        return Err(Error::Limit(
            "definition name, description or provenance is too long".into(),
        ));
    }
    let within = match definition {
        Definition::Component(d) => d.within_bounds(),
        Definition::Symbol(d) => d.within_bounds(),
        Definition::Package(d) => d.within_bounds(),
        Definition::Footprint(d) => d.within_bounds(),
        Definition::Binding(d) => d.within_bounds(),
        Definition::QuantityKind(d) => d.within_bounds(),
        Definition::Field(d) => d.within_bounds(),
        Definition::Part(d) => d.within_bounds(),
        Definition::Model(d) => d.within_bounds(),
        Definition::PackageRow(d) => d.within_bounds(),
    };
    if !within {
        return Err(Error::Limit(
            "definition exceeds supported text or item limits".into(),
        ));
    }
    let finite = match definition {
        Definition::Binding(_) => true,
        Definition::Component(d) => d.values_finite(),
        Definition::QuantityKind(d) => d.values_finite(),
        Definition::Field(d) => d.values_finite(),
        Definition::Part(d) => d.values_finite(),
        Definition::Symbol(d) => d.geometry_finite(),
        Definition::Package(d) => d.geometry_finite(),
        Definition::Footprint(d) => d.geometry_finite(),
        Definition::Model(d) => d.values_finite(),
        Definition::PackageRow(d) => d.values_finite(),
    };
    if !finite {
        return Err(Error::Invalid(
            "definition geometry must be finite, within ±10000 mm and within its supported ranges, and values finite".into(),
        ));
    }
    Ok(())
}

/// The publication order: a revision may only depend on kinds before it.
pub(crate) fn publish_rank(kind: DefinitionKind) -> u8 {
    match kind {
        DefinitionKind::QuantityKind => 0,
        DefinitionKind::Field => 1,
        // D1: models rank before the packages that use them; B1: rows name
        // no lineage.
        DefinitionKind::PackageRow | DefinitionKind::Model => 2,
        DefinitionKind::Package => 3,
        DefinitionKind::Footprint => 4,
        DefinitionKind::Binding => 5,
        DefinitionKind::Symbol => 6,
        DefinitionKind::Component => 7,
        DefinitionKind::Part => 8,
    }
}

/// Validate a stored snapshot: identities, revision sequences, bounds and,
/// through [`publish_findings`], exactly the rules `library.publish` applies.
/// Advisory findings never fail a document.
pub fn validate(library: &Library) -> Result<()> {
    let mut kinds: BTreeMap<DefinitionId, (DefinitionKind, BTreeSet<Revision>)> = BTreeMap::new();
    let revisions = library.revisions();
    for kind in [
        DefinitionKind::Component,
        DefinitionKind::Symbol,
        DefinitionKind::Package,
        DefinitionKind::Footprint,
        DefinitionKind::Binding,
        DefinitionKind::QuantityKind,
        DefinitionKind::Field,
        DefinitionKind::Part,
        DefinitionKind::Model,
        DefinitionKind::PackageRow,
    ] {
        if library.count(kind) > MAX_REVISIONS_PER_KIND {
            return Err(Error::Invalid(format!(
                "too many published {} revisions",
                names::noun(kind)
            )));
        }
    }
    for r in &revisions {
        if r.id.0 == 0 || r.revision.0 == 0 {
            return Err(Error::Invalid(
                "published definition identities and revisions must be nonzero".into(),
            ));
        }
        let entry = kinds.entry(r.id).or_insert((r.kind, BTreeSet::new()));
        if entry.0 != r.kind {
            return Err(Error::Invalid(format!(
                "lineage {} is published as more than one kind",
                r.id
            )));
        }
        if !entry.1.insert(r.revision) {
            return Err(Error::Invalid(format!(
                "{} {} is published twice",
                r.id, r.revision
            )));
        }
    }
    for (id, (_, set)) in &kinds {
        let contiguous = set
            .iter()
            .enumerate()
            .all(|(i, r)| u32::try_from(i + 1).is_ok_and(|n| n == r.0));
        if !contiguous {
            return Err(Error::Invalid(format!(
                "revisions of {id} must run contiguously from r1"
            )));
        }
    }
    for r in revisions {
        let Some((meta, body)) = library.definition(r) else {
            return Err(Error::Invalid("missing published revision".into()));
        };
        check_bounds(&meta, &body)?;
        let blocking: Vec<String> = publish_findings(library, r)
            .into_iter()
            .filter(|f| f.blocks_stage(crate::diagnostics::Stage::Publish))
            .map(|f| f.message)
            .collect();
        if !blocking.is_empty() {
            return Err(Error::Invalid(format!(
                "published {} {} is invalid: {}",
                r.id,
                r.revision,
                blocking.join("; ")
            )));
        }
    }
    bundle::validate_snapshot(library)
}

/// Text that publication accepts as a name or label.
pub(crate) use crate::ids::printable;
