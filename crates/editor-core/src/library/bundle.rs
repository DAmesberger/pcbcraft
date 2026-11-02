//! Library-folder exchange (S3, D1/D2/D7): revision records as a source
//! stores them, the bundle a dependency closure travels in, origins of
//! imported copies, packed assets, the reference-rewrite visitor and bundle
//! validation. Pure: io moves the bytes, this module gives them meaning.
//!
//! **Identity.** Inside a record, every reference to another lineage uses a
//! *record-local* [`DefinitionId`] that the record's [`Link`] table maps to
//! a [`SourceLineageId`]. A record is therefore self-contained, and its
//! [`content_hash`] does not depend on any project's numbering. Import maps
//! `(source lineage, source revision)` to fresh project lineages and
//! revisions and records an [`OriginRecord`] for every copy.
use super::graphics::{FootprintGraphic, Item, TextContent};
use super::package_form::PackageForm;
use super::package_model::{GeneratedMaterials, ModelPlacement, PackageModel};
use super::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, FootprintDefinition,
    Library, PackageDefinition, PartDefinition, SymbolDefinition, Unit,
};
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::fields::{
    FieldDefinition, FieldSelection, QuantityKind, RatedSpec, ToleranceBasis, ValueType,
};
use crate::ids::{
    BindingRef, ComponentRef, ContentHash, DefinitionId, DefinitionKind, DefinitionRef, FieldRef,
    FootprintRef, LibraryId, MAX_LABEL, PackageRef, QuantityKindRef, Revision, SourceLineageId,
    SymbolRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

/// Revision records one bundle carries (a root and its closure).
pub const MAX_BUNDLE_RECORDS: usize = 1024;
/// Links one record lists.
pub const MAX_LINKS: usize = 256;
/// Assets one bundle or project snapshot packs.
pub const MAX_ASSETS: usize = 256;
/// Bytes of one packed asset: the container's per-asset target (D0).
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
const _: () = assert!(MAX_ASSET_BYTES as u64 == crate::format::CONTAINER_MAX_ASSET_BYTES);
/// Encoded bytes all packed assets of one bundle or project snapshot may
/// take together: their base64 text, 4 bytes per 3 (padded), as bundles
/// and automation payloads carry them. The project file stores assets as
/// raw container entries (D0), so this no longer competes with the 16 MiB
/// document. It stays well below the 512 MiB container target because the
/// document's asset bytes are still copied by every command and kept by
/// every undo step; one asset of [`MAX_ASSET_BYTES`] fits. Raising it
/// toward the container target waits for shared asset bytes.
pub const MAX_PACKED_BYTES: usize = 96 * 1024 * 1024;
const _: () = assert!(MAX_ASSET_BYTES.div_ceil(3) * 4 <= MAX_PACKED_BYTES);
const _: () = assert!((MAX_PACKED_BYTES as u64) < crate::format::CONTAINER_MAX_BYTES);
/// Bytes of a media type.
pub const MAX_MEDIA_TYPE: usize = 128;

/// The library item an imported copy was made from (D1). The dedupe key of
/// an import: re-importing an equal origin reuses the existing copy.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub library: LibraryId,
    pub lineage: SourceLineageId,
    pub revision: Revision,
    /// [`content_hash`] of the source record at import time. A folder whose
    /// record no longer hashes to this is reported, never trusted.
    pub content_hash: ContentHash,
}

/// One project revision that was imported or promoted, and where it came
/// from or went to. Stored in `Library::origins`, outside revision bodies.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct OriginRecord {
    /// The project's copy.
    pub local: DefinitionRef,
    pub origin: Origin,
    /// Whether the revision came from the library or went to it.
    #[serde(default)]
    pub direction: Direction,
}

/// Which way an [`OriginRecord`] points.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Copied into the project by `library.import` or `library.place_from`.
    #[default]
    Imported,
    /// Published from the project by `library.promote`.
    Promoted,
}

/// A record-local lineage reference and the source lineage it names.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// The `DefinitionId` the record's body uses.
    pub local: DefinitionId,
    pub lineage: SourceLineageId,
}

/// One immutable revision as a library source stores it
/// (`lineages/<lineage>/r<NNNN>.json` holds exactly this, D2).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RevisionRecord {
    pub lineage: SourceLineageId,
    pub revision: Revision,
    pub kind: DefinitionKind,
    pub meta: DefinitionMeta,
    /// Every reference to another lineage uses a `local` id of `links`.
    pub definition: Definition,
    /// Sorted by `local`; locals are 1, 2, … in first-use order of
    /// [`references`], so equal content always gives equal bytes.
    pub links: Vec<Link>,
    /// Assets the body names by hash (a Part reference's attached copy).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<ContentHash>,
}

/// Asset bytes, written as standard base64 with padding. Immutable and
/// shared: cloning a project (each command's candidate, each undo
/// snapshot, each save) shares the bytes instead of copying up to
/// [`MAX_PACKED_BYTES`], and the bytes' [`asset_hash`] is computed once
/// per allocation and remembered with them, so revalidating an unchanged
/// asset never hashes it again. Equality is by identity first, then by
/// content.
#[derive(Clone, Default)]
pub struct AssetBytes(Arc<SharedBytes>);

#[derive(Clone, Default)]
struct SharedBytes {
    bytes: Vec<u8>,
    hash: OnceLock<ContentHash>,
}

impl AssetBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Arc::new(SharedBytes {
            bytes,
            hash: OnceLock::new(),
        }))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0.bytes
    }

    /// [`asset_hash`] of the bytes, computed on first use only.
    pub fn hash(&self) -> ContentHash {
        *self.0.hash.get_or_init(|| asset_hash(&self.0.bytes))
    }

    /// Whether `self` and `other` share one allocation (no bytes copied).
    pub fn shares(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The bytes to change, copied first when shared; the remembered hash
    /// is dropped.
    pub fn make_mut(&mut self) -> &mut Vec<u8> {
        let inner = Arc::make_mut(&mut self.0);
        inner.hash = OnceLock::new();
        &mut inner.bytes
    }

    /// The bytes, copied only when shared.
    pub fn into_vec(self) -> Vec<u8> {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => inner.bytes,
            Err(shared) => shared.bytes.clone(),
        }
    }
}

impl From<Vec<u8>> for AssetBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl std::ops::Deref for AssetBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl AsRef<[u8]> for AssetBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl PartialEq for AssetBytes {
    fn eq(&self, other: &Self) -> bool {
        self.shares(other) || self.0.bytes == other.0.bytes
    }
}

impl Eq for AssetBytes {}

impl std::fmt::Debug for AssetBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AssetBytes({} bytes)", self.0.bytes.len())
    }
}

impl Serialize for AssetBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&base64_encode(&self.0.bytes))
    }
}

impl<'de> Deserialize<'de> for AssetBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        base64_decode(&text)
            .map(Self::new)
            .ok_or_else(|| serde::de::Error::custom("asset data must be standard base64"))
    }
}

impl JsonSchema for AssetBytes {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AssetBytes".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as JsonSchema>::json_schema(generator)
    }
}

/// One asset packed by content hash (D7: the project file carries the
/// bytes, so it reopens offline; a folder stores them under
/// `assets/sha256/<hash>`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackedAsset {
    /// [`asset_hash`] of `data`.
    pub hash: ContentHash,
    pub media_type: String,
    pub data: AssetBytes,
}

/// A root revision and its complete dependency closure, carried inline in
/// `library.import` / `library.place_from` (keeps the core pure and replay
/// deterministic) and produced by `library.promote_preview`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub library: LibraryId,
    /// The source's display name, for messages only.
    pub name: String,
    pub root: SourceRevision,
    /// The root and every revision it reaches, dependencies first.
    pub records: Vec<RevisionRecord>,
    pub assets: Vec<PackedAsset>,
}

/// One revision of one source lineage.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct SourceRevision {
    pub lineage: SourceLineageId,
    pub revision: Revision,
}

/// One place a definition body names another lineage.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(tag = "site", rename_all = "snake_case")]
pub enum RefSite {
    /// An exact revision (`ComponentRef`, `PackageRef`, `FieldRef`, …).
    Exact { target: DefinitionRef },
    /// A lineage without a revision (a symbol's or binding's owning
    /// component).
    Lineage {
        kind: DefinitionKind,
        id: DefinitionId,
    },
}

/// How [`rewrite_refs`] maps each reference. Implementations decide what an
/// unknown target means (import refuses it).
pub trait RefMap {
    fn exact(&mut self, target: DefinitionRef) -> Result<DefinitionRef>;
    fn lineage(&mut self, kind: DefinitionKind, id: DefinitionId) -> Result<DefinitionId>;
}

/// Rewrite every lineage reference in `definition`, of every kind, in a
/// fixed order. Local IDs (terminals, pads, units, …) are untouched. A
/// missed reference would corrupt an imported closure, so the round-trip
/// property test covers every kind, and every body is destructured without
/// `..`: a new field does not compile until this visitor decides on it.
///
/// Order: component representations, implementations, field selections;
/// symbol owner, then unit text fields; footprint package, then text
/// fields; binding owner, package, footprints; field quantity kind; Part
/// component, binding, rated-spec fields. Packages and quantity kinds name
/// no other lineage.
pub fn rewrite_refs(definition: &mut Definition, map: &mut dyn RefMap) -> Result<()> {
    match definition {
        Definition::Component(component) => {
            let ComponentDefinition {
                next_local: _,
                designator_prefix: _,
                terminals: _,
                resources: _,
                constraints: _,
                representations,
                implementations,
                fields,
                pin_rules: _,
                marking,
                review: _,
            } = component;
            if let Some(marking) = marking {
                marking_refs(marking, map)?;
            }
            for r in representations {
                let (id, revision) = exact(map, DefinitionKind::Symbol, r.id, r.revision)?;
                *r = SymbolRef::new(id, revision);
            }
            for r in implementations {
                let (id, revision) = exact(map, DefinitionKind::Binding, r.id, r.revision)?;
                *r = BindingRef::new(id, revision);
            }
            for selection in fields {
                let FieldSelection {
                    field,
                    default: _,
                    show_on_symbol: _,
                } = selection;
                let (id, revision) = exact(map, DefinitionKind::Field, field.id, field.revision)?;
                *field = FieldRef::new(id, revision);
            }
        }
        Definition::Symbol(symbol) => {
            let SymbolDefinition {
                component,
                next_local: _,
                pin_text_height: _,
                units,
            } = symbol;
            *component = map.lineage(DefinitionKind::Component, *component)?;
            for unit in units {
                let Unit {
                    id: _,
                    label: _,
                    graphics,
                    pins: _,
                } = unit;
                for graphic in graphics {
                    item_refs(&mut graphic.item, map)?;
                }
            }
        }
        Definition::Package(package) => {
            let PackageDefinition {
                next_local: _,
                terminals: _,
                form,
                envelope: _,
                model,
                placement,
            } = package;
            // The form, model and placement name no other lineage and no
            // asset: materials are built-in `MaterialId`s of the project's
            // `material_table`, not library references. Matched without
            // wildcards so a reference added to any of them (an imported
            // model's asset, M4) fails to compile here until it is rewritten.
            match form {
                PackageForm::Generic
                | PackageForm::Chip(_)
                | PackageForm::GullWing(_)
                | PackageForm::NoLead(_)
                | PackageForm::Header(_)
                | PackageForm::Tabbed(_)
                | PackageForm::Molded(_)
                | PackageForm::Grid(_)
                | PackageForm::Tht(_)
                | PackageForm::Module(_) => {}
            }
            match model {
                PackageModel::Envelope { material: _ } => {}
                PackageModel::Generated {
                    family_rev: _,
                    materials: GeneratedMaterials { roles: _ },
                } => {}
                // D1: a library model names an exact Model revision, so
                // `references` and `closure` carry the model with the
                // package; the lead map names model leads and package
                // terminals, which are local.
                PackageModel::Library(library) => {
                    let (id, revision) = exact(
                        map,
                        DefinitionKind::Model,
                        library.model.id,
                        library.model.revision,
                    )?;
                    library.model = crate::ids::ModelRef::new(id, revision);
                }
            }
            let ModelPlacement {
                translation: _,
                rotation_deg: _,
            } = placement;
        }
        Definition::Footprint(footprint) => {
            let FootprintDefinition {
                package,
                next_local: _,
                source: _,
                pads: _,
                graphics,
                polarity: _,
                datum: _,
                defaults: _,
                keepouts: _,
            } = footprint;
            let (id, revision) = exact(map, DefinitionKind::Package, package.id, package.revision)?;
            *package = PackageRef::new(id, revision);
            for graphic in graphics {
                let FootprintGraphic {
                    id: _,
                    layer: _,
                    side: _,
                    item,
                } = graphic;
                item_refs(item, map)?;
            }
        }
        Definition::Binding(binding) => {
            let BindingDefinition {
                component,
                package,
                mappings: _,
                footprints,
                unavailable: _,
            } = binding;
            *component = map.lineage(DefinitionKind::Component, *component)?;
            let (id, revision) = exact(map, DefinitionKind::Package, package.id, package.revision)?;
            *package = PackageRef::new(id, revision);
            for r in footprints {
                let (id, revision) = exact(map, DefinitionKind::Footprint, r.id, r.revision)?;
                *r = FootprintRef::new(id, revision);
            }
        }
        Definition::QuantityKind(kind) => {
            let QuantityKind {
                units: _,
                default_unit: _,
            } = kind;
        }
        Definition::Field(field) => {
            let FieldDefinition {
                next_local: _,
                value_type,
                default: _,
                required_at: _,
            } = field;
            match value_type {
                ValueType::Quantity { kind, .. }
                | ValueType::Range { kind }
                | ValueType::Tolerance {
                    basis: ToleranceBasis::Absolute { kind },
                } => {
                    let (id, revision) =
                        exact(map, DefinitionKind::QuantityKind, kind.id, kind.revision)?;
                    *kind = QuantityKindRef::new(id, revision);
                }
                ValueType::Integer { .. }
                | ValueType::Boolean
                | ValueType::Enum { .. }
                | ValueType::Text { .. }
                | ValueType::Tolerance {
                    basis: ToleranceBasis::Relative,
                } => {}
            }
        }
        Definition::Part(part) => {
            let PartDefinition {
                next_local: _,
                manufacturer: _,
                mpn: _,
                component,
                binding,
                specs,
                references: _,
                offers: _,
                provenance: _,
                top_mark,
            } = part.as_mut();
            if let Some(marking) = top_mark {
                marking_refs(marking, map)?;
            }
            let (id, revision) = exact(
                map,
                DefinitionKind::Component,
                component.id,
                component.revision,
            )?;
            *component = ComponentRef::new(id, revision);
            let (id, revision) = exact(map, DefinitionKind::Binding, binding.id, binding.revision)?;
            *binding = BindingRef::new(id, revision);
            for spec in specs {
                let RatedSpec {
                    field,
                    value: _,
                    meaning: _,
                } = spec;
                let (id, revision) = exact(map, DefinitionKind::Field, field.id, field.revision)?;
                *field = FieldRef::new(id, revision);
            }
        }
        // D1 / B1: models and rows name no other lineage (model assets are
        // named by hash, `named_assets`).
        Definition::Model(_) | Definition::PackageRow(_) => {}
    }
    Ok(())
}

/// The field references of a marking spec (D4). Artwork names an asset by
/// hash, not a lineage.
fn marking_refs(spec: &mut super::marking::MarkingSpec, map: &mut dyn RefMap) -> Result<()> {
    use super::marking::MarkingItem;
    for decal in &mut spec.decals {
        marking_refs(decal, map)?;
    }
    for item in &mut spec.items {
        let fields: Vec<&mut FieldRef> = match item {
            MarkingItem::Code { field, .. } => vec![field],
            MarkingItem::Bands { value, tolerance } => {
                std::iter::once(value).chain(tolerance.as_mut()).collect()
            }
            MarkingItem::Electrolytic {
                capacitance,
                voltage,
                ..
            } => std::iter::once(capacitance)
                .chain(voltage.as_mut())
                .collect(),
            MarkingItem::Text { .. }
            | MarkingItem::Code2d { .. }
            | MarkingItem::Artwork { .. }
            | MarkingItem::Polarity { .. } => Vec::new(),
        };

        for field in fields {
            let (id, revision) = exact(map, DefinitionKind::Field, field.id, field.revision)?;
            *field = FieldRef::new(id, revision);
        }
    }
    Ok(())
}

/// One exact reference through `map`, which must keep its kind.
fn exact(
    map: &mut dyn RefMap,
    kind: DefinitionKind,
    id: DefinitionId,
    revision: Revision,
) -> Result<(DefinitionId, Revision)> {
    let mapped = map.exact(DefinitionRef { kind, id, revision })?;
    if mapped.kind != kind {
        return Err(Error::Invalid(format!(
            "a {} reference cannot become a {} reference",
            super::names::noun(kind),
            super::names::noun(mapped.kind)
        )));
    }
    Ok((mapped.id, mapped.revision))
}

/// The field lineage a drawn text shows, if any.
fn item_refs(item: &mut Item, map: &mut dyn RefMap) -> Result<()> {
    if let Item::Text(text) = item
        && let TextContent::Field(field) = &mut text.content
    {
        *field = map.lineage(DefinitionKind::Field, *field)?;
    }
    Ok(())
}

/// Every reference `definition` makes, in [`rewrite_refs`] order.
pub fn references(definition: &Definition) -> Result<Vec<RefSite>> {
    struct Record(Vec<RefSite>);
    impl RefMap for Record {
        fn exact(&mut self, target: DefinitionRef) -> Result<DefinitionRef> {
            self.0.push(RefSite::Exact { target });
            Ok(target)
        }
        fn lineage(&mut self, kind: DefinitionKind, id: DefinitionId) -> Result<DefinitionId> {
            self.0.push(RefSite::Lineage { kind, id });
            Ok(id)
        }
    }
    let mut copy = definition.clone();
    let mut record = Record(Vec::new());
    rewrite_refs(&mut copy, &mut record)?;
    Ok(record.0)
}

/// The exact revisions `root` reaches in `library`, dependencies first and
/// `root` last: sorted by publication rank (an exact reference always names
/// a lower rank), then lineage and revision, so equal libraries give equal
/// closures. Exact references are followed; a text that shows a field
/// lineage the closure does not otherwise hold brings in that field's
/// latest published revision, so the copy can resolve it. An owner
/// reference (a symbol's or binding's component) is not followed;
/// [`export`] maps it to a closure member or a lineage the target holds, or
/// refuses. A dangling reference is an error naming it.
///
/// [`export`]: super::import::export
pub fn closure(library: &Library, root: DefinitionRef) -> Result<Vec<DefinitionRef>> {
    let mut seen = BTreeSet::new();
    let mut shown: Vec<(DefinitionId, DefinitionRef)> = Vec::new();
    let mut stack = vec![(root, None::<DefinitionRef>)];
    loop {
        closure_step(library, &mut stack, &mut seen, &mut shown)?;
        // Shown fields the closure lacks, at their latest revision.
        let mut more = false;
        while let Some((id, from)) = shown.pop() {
            if seen.iter().any(|m| m.id == id) {
                continue;
            }
            let Some((DefinitionKind::Field, revision)) = library.latest(id) else {
                return Err(Error::NotFound(format!(
                    "{} shows a field that is missing from the project library",
                    library.describe(from)
                )));
            };
            stack.push((
                DefinitionRef {
                    kind: DefinitionKind::Field,
                    id,
                    revision,
                },
                Some(from),
            ));
            more = true;
            break;
        }
        if !more {
            break;
        }
    }
    let mut out: Vec<DefinitionRef> = seen.into_iter().collect();
    out.sort_by_key(|r| (super::publish_rank(r.kind), r.id, r.revision));
    Ok(out)
}

/// Follow exact references from `stack` until it is empty, noting shown
/// field lineages.
fn closure_step(
    library: &Library,
    stack: &mut Vec<(DefinitionRef, Option<DefinitionRef>)>,
    seen: &mut BTreeSet<DefinitionRef>,
    shown: &mut Vec<(DefinitionId, DefinitionRef)>,
) -> Result<()> {
    while let Some((r, from)) = stack.pop() {
        if seen.contains(&r) {
            continue;
        }
        let Some((_, body)) = library.definition(r) else {
            return Err(Error::NotFound(match from {
                Some(from) => format!(
                    "{} names {}, which is missing from the project library",
                    library.describe(from),
                    library.describe(r)
                ),
                None => format!(
                    "{} is missing from the project library",
                    library.describe(r)
                ),
            }));
        };
        seen.insert(r);
        if seen.len() > MAX_BUNDLE_RECORDS {
            return Err(Error::Limit(format!(
                "a closure holds at most {MAX_BUNDLE_RECORDS} revisions"
            )));
        }
        for site in references(&body)? {
            match site {
                RefSite::Exact { target } if !seen.contains(&target) => {
                    stack.push((target, Some(r)));
                }
                RefSite::Lineage {
                    kind: DefinitionKind::Field,
                    id,
                } => shown.push((id, r)),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Content hashes a body names: the attached copies of a Part's references,
/// a 3D model's asset and appearance pair (D1), and the SVG artwork of a
/// component's marking or a Part's top mark (D4), sorted and without
/// repeats. A hash that is not 64 lower-case hex digits is `invalid_input`.
pub fn named_assets(definition: &Definition) -> Result<Vec<ContentHash>> {
    if let Definition::Model(model) = definition {
        let hashes: BTreeSet<ContentHash> = model.assets().map(|a| a.hash).collect();
        return Ok(hashes.into_iter().collect());
    }
    if let Definition::Component(component) = definition {
        let hashes: BTreeSet<ContentHash> =
            component.marking.iter().flat_map(|m| m.artwork()).collect();
        return Ok(hashes.into_iter().collect());
    }
    let Definition::Part(part) = definition else {
        return Ok(Vec::new());
    };
    let mut out: BTreeSet<ContentHash> = part.top_mark.iter().flat_map(|m| m.artwork()).collect();
    for reference in &part.references {
        if let Some(text) = &reference.asset {
            let hash = ContentHash::parse(text).ok_or_else(|| {
                Error::Invalid(format!(
                    "reference {} names asset ‘{text}’, which is not a lower-case SHA-256",
                    reference.id
                ))
            })?;
            out.insert(hash);
        }
    }
    Ok(out.into_iter().collect())
}

/// Build the canonical record of one revision. `exact` maps each exact
/// reference to the source revision it becomes, `lineage` each lineage
/// reference to its source lineage; record-local ids are then numbered 1,
/// 2, … in first-use order of [`references`], one per source lineage.
pub fn to_record(
    lineage: SourceLineageId,
    revision: Revision,
    meta: DefinitionMeta,
    mut definition: Definition,
    exact: &mut dyn FnMut(DefinitionRef) -> Result<SourceRevision>,
    lineage_of: &mut dyn FnMut(DefinitionKind, DefinitionId) -> Result<SourceLineageId>,
) -> Result<RevisionRecord> {
    struct Localize<'a> {
        links: Vec<Link>,
        exact: &'a mut dyn FnMut(DefinitionRef) -> Result<SourceRevision>,
        lineage: &'a mut dyn FnMut(DefinitionKind, DefinitionId) -> Result<SourceLineageId>,
    }
    impl Localize<'_> {
        fn local(&mut self, lineage: SourceLineageId) -> Result<DefinitionId> {
            if let Some(link) = self.links.iter().find(|l| l.lineage == lineage) {
                return Ok(link.local);
            }
            if self.links.len() >= MAX_LINKS {
                return Err(Error::Limit(format!(
                    "a revision record lists at most {MAX_LINKS} links"
                )));
            }
            let local = DefinitionId(u64::try_from(self.links.len() + 1).unwrap_or(u64::MAX));
            self.links.push(Link { local, lineage });
            Ok(local)
        }
    }
    impl RefMap for Localize<'_> {
        fn exact(&mut self, target: DefinitionRef) -> Result<DefinitionRef> {
            let source = (self.exact)(target)?;
            Ok(DefinitionRef {
                kind: target.kind,
                id: self.local(source.lineage)?,
                revision: source.revision,
            })
        }
        fn lineage(&mut self, kind: DefinitionKind, id: DefinitionId) -> Result<DefinitionId> {
            let source = (self.lineage)(kind, id)?;
            self.local(source)
        }
    }
    let mut map = Localize {
        links: Vec::new(),
        exact,
        lineage: lineage_of,
    };
    rewrite_refs(&mut definition, &mut map)?;
    Ok(RevisionRecord {
        lineage,
        revision,
        kind: definition.kind(),
        meta,
        assets: named_assets(&definition)?,
        definition,
        links: map.links,
    })
}

/// A record's body with every record-local id resolved: `exact` maps a
/// source revision of a kind to the target's exact revision, `lineage` a
/// source lineage to the target's lineage. A local id the links do not list
/// is `invalid_input`.
pub fn from_record(
    record: &RevisionRecord,
    exact: &mut dyn FnMut(DefinitionKind, SourceRevision) -> Result<DefinitionRef>,
    lineage: &mut dyn FnMut(DefinitionKind, SourceLineageId) -> Result<DefinitionId>,
) -> Result<Definition> {
    struct Resolve<'a> {
        record: &'a RevisionRecord,
        exact: &'a mut dyn FnMut(DefinitionKind, SourceRevision) -> Result<DefinitionRef>,
        lineage: &'a mut dyn FnMut(DefinitionKind, SourceLineageId) -> Result<DefinitionId>,
    }
    impl Resolve<'_> {
        fn source(&self, local: DefinitionId) -> Result<SourceLineageId> {
            self.record
                .links
                .iter()
                .find(|l| l.local == local)
                .map(|l| l.lineage)
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "record {} {} uses {local}, which its links do not list",
                        self.record.lineage, self.record.revision
                    ))
                })
        }
    }
    impl RefMap for Resolve<'_> {
        fn exact(&mut self, target: DefinitionRef) -> Result<DefinitionRef> {
            let lineage = self.source(target.id)?;
            (self.exact)(
                target.kind,
                SourceRevision {
                    lineage,
                    revision: target.revision,
                },
            )
        }
        fn lineage(&mut self, kind: DefinitionKind, id: DefinitionId) -> Result<DefinitionId> {
            let lineage = self.source(id)?;
            (self.lineage)(kind, lineage)
        }
    }
    let mut definition = record.definition.clone();
    rewrite_refs(
        &mut definition,
        &mut Resolve {
            record,
            exact,
            lineage,
        },
    )?;
    Ok(definition)
}

/// What [`local_library`] built: a library in catalog-local ids.
pub struct LocalLibrary {
    pub library: Library,
    /// Catalog-local ids and the source lineages they stand for, sorted by
    /// local id (source lineages are numbered 1, 2, … in id order).
    pub lineages: Vec<Link>,
    /// Records left out: repeats of a revision, a lineage of two kinds or
    /// links to lineages the records do not hold.
    pub skipped: Vec<SourceRevision>,
}

/// Every record of one source as a library in catalog-local ids, so the
/// catalog can read it like a project snapshot. Deterministic: the same
/// records in any order give the same library.
pub fn local_library(records: &[RevisionRecord]) -> Result<LocalLibrary> {
    let mut order: Vec<&RevisionRecord> = records.iter().collect();
    order.sort_by_key(|r| (r.lineage, r.revision));
    let mut kinds: BTreeMap<SourceLineageId, DefinitionKind> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut kept = Vec::new();
    let mut seen = BTreeSet::new();
    for record in order {
        let at = SourceRevision {
            lineage: record.lineage,
            revision: record.revision,
        };
        let kind_ok = *kinds.entry(record.lineage).or_insert(record.kind) == record.kind;
        if !kind_ok
            || record.definition.kind() != record.kind
            || record.revision.0 == 0
            || !seen.insert(at)
        {
            skipped.push(at);
        } else {
            kept.push(record);
        }
    }
    let mut ids: BTreeMap<SourceLineageId, DefinitionId> = BTreeMap::new();
    for (index, lineage) in kinds.keys().enumerate() {
        let id = u64::try_from(index + 1)
            .map_err(|_| Error::Limit("too many source lineages".into()))?;
        ids.insert(*lineage, DefinitionId(id));
    }
    let mut library = Library::default();
    for record in kept {
        let body = from_record(
            record,
            &mut |kind, source| {
                let id = ids.get(&source.lineage).copied().ok_or_else(|| {
                    Error::NotFound(format!("lineage {} is not in the source", source.lineage))
                })?;
                Ok(DefinitionRef {
                    kind,
                    id,
                    revision: source.revision,
                })
            },
            &mut |_, lineage| {
                ids.get(&lineage).copied().ok_or_else(|| {
                    Error::NotFound(format!("lineage {lineage} is not in the source"))
                })
            },
        );
        match (body, ids.get(&record.lineage)) {
            (Ok(body), Some(id)) => {
                library.append(*id, record.revision, record.meta.clone(), body);
            }
            _ => skipped.push(SourceRevision {
                lineage: record.lineage,
                revision: record.revision,
            }),
        }
    }
    Ok(LocalLibrary {
        library,
        lineages: ids
            .into_iter()
            .map(|(lineage, local)| Link { local, lineage })
            .collect(),
        skipped,
    })
}

/// SHA-256 of a record's canonical JSON (its serde encoding, which has a
/// fixed field order and sorted links). io compares it with the hash an
/// origin recorded to detect an edited rN.
pub fn content_hash(record: &RevisionRecord) -> Result<ContentHash> {
    let bytes = serde_json::to_vec(record)?;
    Ok(ContentHash(Sha256::digest(&bytes).into()))
}

/// SHA-256 of asset bytes.
pub fn asset_hash(bytes: &[u8]) -> ContentHash {
    ContentHash(Sha256::digest(bytes).into())
}

/// Structural checks of a bundle before anything is imported: bounds, one
/// record per revision and one kind per lineage, canonical links (locals 1,
/// 2, … in first-use order, one per source lineage), every exact reference
/// naming a record of the bundle with its kind and every lineage reference a
/// lineage of the bundle, the root present, each record's `assets` exactly
/// the hashes its body names and packed once, nothing else packed, and
/// assets hashing to their names. Publish rules run again on the project
/// candidate.
pub fn validate_bundle(bundle: &Bundle) -> Result<()> {
    if bundle.name.len() > MAX_LABEL {
        return Err(Error::Limit("library name is too long".into()));
    }
    if bundle.records.is_empty() || bundle.records.len() > MAX_BUNDLE_RECORDS {
        return Err(Error::Limit(format!(
            "a bundle carries 1–{MAX_BUNDLE_RECORDS} revision records"
        )));
    }
    let mut revisions: BTreeMap<SourceRevision, DefinitionKind> = BTreeMap::new();
    let mut kinds: BTreeMap<SourceLineageId, DefinitionKind> = BTreeMap::new();
    for record in &bundle.records {
        if record.links.len() > MAX_LINKS {
            return Err(Error::Limit(format!(
                "a revision record lists at most {MAX_LINKS} links"
            )));
        }
        if record.definition.kind() != record.kind {
            return Err(Error::Invalid(format!(
                "the record of ‘{}’ {} says {} but holds a {}",
                record.meta.name,
                record.revision,
                super::names::noun(record.kind),
                super::names::noun(record.definition.kind())
            )));
        }
        if record.revision.0 == 0 {
            return Err(Error::Invalid(format!(
                "record {} has revision 0; revisions start at r1",
                record.lineage
            )));
        }
        let at = SourceRevision {
            lineage: record.lineage,
            revision: record.revision,
        };
        if revisions.insert(at, record.kind).is_some() {
            return Err(Error::Invalid(format!(
                "the bundle carries {} {} twice",
                record.lineage, record.revision
            )));
        }
        if *kinds.entry(record.lineage).or_insert(record.kind) != record.kind {
            return Err(Error::Invalid(format!(
                "lineage {} is carried as more than one kind",
                record.lineage
            )));
        }
    }
    if !revisions.contains_key(&bundle.root) {
        return Err(Error::Invalid(
            "the bundle does not contain its root revision".into(),
        ));
    }
    validate_assets(&bundle.assets)?;
    let packed: BTreeSet<ContentHash> = bundle.assets.iter().map(|a| a.hash).collect();
    let mut named = BTreeSet::new();
    for record in &bundle.records {
        check_links(record, &revisions, &kinds)?;
        let assets = named_assets(&record.definition)?;
        if assets != record.assets {
            return Err(Error::Invalid(format!(
                "record {} {} lists assets other than the ones its body names",
                record.lineage, record.revision
            )));
        }
        for hash in assets {
            if !packed.contains(&hash) {
                return Err(Error::Invalid(format!(
                    "record {} {} names asset {hash}, which the bundle does not pack",
                    record.lineage, record.revision
                )));
            }
            named.insert(hash);
        }
    }
    if let Some(extra) = packed.difference(&named).next() {
        return Err(Error::Invalid(format!(
            "the bundle packs asset {extra}, which no record names"
        )));
    }
    Ok(())
}

/// A record's links are canonical and resolve inside the bundle.
fn check_links(
    record: &RevisionRecord,
    revisions: &BTreeMap<SourceRevision, DefinitionKind>,
    kinds: &BTreeMap<SourceLineageId, DefinitionKind>,
) -> Result<()> {
    let at = || format!("record {} {}", record.lineage, record.revision);
    let mut order: Vec<DefinitionId> = Vec::new();
    for site in references(&record.definition)? {
        let (kind, local, revision) = match site {
            RefSite::Exact { target } => (target.kind, target.id, Some(target.revision)),
            RefSite::Lineage { kind, id } => (kind, id, None),
        };
        if !order.contains(&local) {
            order.push(local);
        }
        let link = record
            .links
            .iter()
            .find(|l| l.local == local)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{} uses {local}, which its links do not list",
                    at()
                ))
            })?;
        let found = match revision {
            Some(revision) => revisions
                .get(&SourceRevision {
                    lineage: link.lineage,
                    revision,
                })
                .copied(),
            None => kinds.get(&link.lineage).copied(),
        };
        match found {
            Some(found) if found == kind => {}
            Some(found) => {
                return Err(Error::Invalid(format!(
                    "{} names a {} that the bundle carries as a {}",
                    at(),
                    super::names::noun(kind),
                    super::names::noun(found)
                )));
            }
            None => {
                return Err(Error::Invalid(format!(
                    "{} names {}{}, which the bundle does not carry",
                    at(),
                    link.lineage,
                    revision.map(|r| format!(" {r}")).unwrap_or_default()
                )));
            }
        }
    }
    let canonical = order
        .iter()
        .enumerate()
        .all(|(i, local)| u64::try_from(i + 1).is_ok_and(|n| local.0 == n));
    let lineages: BTreeSet<SourceLineageId> = record.links.iter().map(|l| l.lineage).collect();
    if !canonical || record.links.len() != order.len() || lineages.len() != order.len() {
        return Err(Error::Invalid(format!(
            "{} is not canonical: links must number each named lineage once, 1, 2, … in first-use order",
            at()
        )));
    }
    if record
        .links
        .iter()
        .enumerate()
        .any(|(i, l)| u64::try_from(i + 1).is_ok_and(|n| l.local.0 != n))
    {
        return Err(Error::Invalid(format!(
            "{} lists its links out of order",
            at()
        )));
    }
    Ok(())
}

/// Bounds, media types and hashes of packed assets.
pub fn validate_assets(assets: &[PackedAsset]) -> Result<()> {
    if assets.len() > MAX_ASSETS {
        return Err(Error::Limit(format!("at most {MAX_ASSETS} packed assets")));
    }
    let mut seen = BTreeSet::new();
    let mut encoded: usize = 0;
    for asset in assets {
        encoded = encoded
            .checked_add(encoded_len(asset.data.len()))
            .filter(|n| *n <= MAX_PACKED_BYTES)
            .ok_or_else(|| {
                Error::Limit(format!(
                    "packed assets take at most {} MiB in the project file (as base64); remove or replace an attached copy first",
                    MAX_PACKED_BYTES / (1024 * 1024)
                ))
            })?;
        if asset.data.len() > MAX_ASSET_BYTES {
            return Err(Error::Limit(format!(
                "asset {} exceeds {MAX_ASSET_BYTES} bytes",
                asset.hash
            )));
        }
        if asset.media_type.is_empty()
            || asset.media_type.len() > MAX_MEDIA_TYPE
            || asset.media_type.chars().any(char::is_control)
        {
            return Err(Error::Invalid(format!(
                "asset {} has an invalid media type",
                asset.hash
            )));
        }
        if asset.data.hash() != asset.hash {
            return Err(Error::Invalid(format!(
                "asset {} does not match its content hash",
                asset.hash
            )));
        }
        if !seen.insert(asset.hash) {
            return Err(Error::Invalid(format!(
                "asset {} is packed twice",
                asset.hash
            )));
        }
    }
    Ok(())
}

/// Bytes of the padded base64 text of `n` bytes.
pub fn encoded_len(n: usize) -> usize {
    n.div_ceil(3).saturating_mul(4)
}

/// Snapshot checks of `Library::origins` and `Library::assets`. Each origin
/// names a published revision of its own kind; per library a project
/// revision has at most one origin, a source revision is copied at most
/// once, and project and source lineages correspond one to one (so import
/// and promote always agree on where a lineage lives). A revision may have
/// origins in several libraries (imported from one, promoted to another).
/// Assets are well formed, and every 3D model's assets are packed and every
/// package's library model resolves (`model3d::snapshot_problems`).
pub fn validate_snapshot(library: &Library) -> Result<()> {
    let mut locals = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut forward: BTreeMap<(LibraryId, DefinitionId), SourceLineageId> = BTreeMap::new();
    let mut backward: BTreeMap<(LibraryId, SourceLineageId), DefinitionId> = BTreeMap::new();
    for record in &library.origins {
        let OriginRecord { local, origin, .. } = record;
        if library.definition(*local).is_none() {
            return Err(Error::Invalid(format!(
                "origin of {} {} names a revision the project does not hold",
                local.id, local.revision
            )));
        }
        if !locals.insert((origin.library, *local)) {
            return Err(Error::Invalid(format!(
                "{} {} has more than one origin in library {}",
                local.id, local.revision, origin.library
            )));
        }
        if !sources.insert((origin.library, origin.lineage, origin.revision)) {
            return Err(Error::Invalid(format!(
                "{} {} of library {} is copied more than once",
                origin.lineage, origin.revision, origin.library
            )));
        }
        let one_to_one = *forward
            .entry((origin.library, local.id))
            .or_insert(origin.lineage)
            == origin.lineage
            && *backward
                .entry((origin.library, origin.lineage))
                .or_insert(local.id)
                == local.id;
        if !one_to_one {
            return Err(Error::Invalid(format!(
                "lineage {} and source lineage {} of library {} do not correspond one to one",
                local.id, origin.lineage, origin.library
            )));
        }
    }
    validate_assets(&library.assets)?;
    super::model3d::snapshot_problems(library)?;
    super::marking::snapshot_problems(library)
}

/// Advisory findings of the asset store (D7): `asset.missing` for each Part
/// reference whose attached copy is not packed in `library.assets`, so the
/// project would not reopen offline with it.
pub fn asset_findings(library: &Library) -> Vec<Finding> {
    let packed: BTreeSet<ContentHash> = library.assets.iter().map(|a| a.hash).collect();
    let mut out = Vec::new();
    for part in &library.parts {
        for reference in &part.definition.references {
            let Some(text) = &reference.asset else {
                continue;
            };
            if ContentHash::parse(text).is_some_and(|h| packed.contains(&h)) {
                continue;
            }
            out.push(Finding::new(
                "asset.missing",
                format!(
                    "Reference ‘{}’ of Part ‘{}’ {} names an attached copy the project does not pack.",
                    reference.title, part.meta.name, part.revision
                ),
                vec![ObjectRef::LibraryObject {
                    definition: part.id,
                    local: reference.id,
                }],
            ));
        }
    }
    out
}

/// The packed assets `hashes` name, in hash order. A hash the project does
/// not pack is `not_found`.
pub fn packed(library: &Library, hashes: &BTreeSet<ContentHash>) -> Result<Vec<PackedAsset>> {
    hashes
        .iter()
        .map(|hash| {
            library
                .assets
                .iter()
                .find(|a| a.hash == *hash)
                .cloned()
                .ok_or_else(|| {
                    Error::NotFound(format!(
                        "asset {hash} is not packed in the project; attach the file again"
                    ))
                })
        })
        .collect()
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let index = usize::try_from((n >> (18 - 6 * i)) & 0x3f).unwrap_or(0);
                out.push(char::from(ALPHABET[index]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        ALPHABET
            .iter()
            .position(|a| *a == c)
            .and_then(|p| u32::try_from(p).ok())
    }
    let raw = text.as_bytes();
    if !raw.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(raw.len() / 4 * 3);
    let chunks = raw.len() / 4;
    for (index, chunk) in raw.chunks_exact(4).enumerate() {
        let padding = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != chunks) {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = if i >= 4 - padding { 0 } else { value(*c)? };
            n = (n << 6) | v;
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - padding]);
    }
    // Reject non-canonical encodings (stray bits under the padding).
    (base64_encode(&out) == text).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_and_rejects_noise() {
        for len in 0..10 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 5) as u8).collect();
            let text = base64_encode(&bytes);
            assert_eq!(base64_decode(&text), Some(bytes));
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_decode("Zm9=x"), None);
        assert_eq!(base64_decode("Zm9"), None);
        assert_eq!(base64_decode("Zm9="), None, "stray bits under padding");
    }

    #[test]
    fn hashes_are_stable_hex() -> Result<()> {
        let hash = asset_hash(b"abc");
        assert_eq!(
            hash.to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(ContentHash::parse(&hash.to_string()), Some(hash));
        let json = serde_json::to_string(&hash)?;
        let back: ContentHash = serde_json::from_str(&json)?;
        assert_eq!(back, hash);
        Ok(())
    }
}
