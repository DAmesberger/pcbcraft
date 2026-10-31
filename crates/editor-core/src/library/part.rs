//! Manufacturer parts (S2): one exact orderable item, its own revisioned
//! lineage pinning a generic component revision and one of its bindings. An
//! "atomic part" elsewhere is a component plus exactly one Part.
//!
//! Only identity the user chose is stored (D9): manufacturer, MPN,
//! distributor, SKU and links. No price, stock or supplier observation.
use super::publish::{issue, object};
use super::{ComponentDefinition, Library, printable};
use crate::diagnostics::{Finding, ObjectRef};
use crate::fields::{self, MAX_RATED_SPECS, RatedSpec};
use crate::ids::{BindingRef, ComponentRef, DefinitionId, MAX_LABEL};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// References one Part revision lists.
pub const MAX_REFERENCES: usize = 32;
/// Distributor offers one Part revision lists.
pub const MAX_OFFERS: usize = 32;
/// Bytes of one URL.
pub const MAX_URL: usize = 2048;

/// A manufacturer part revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartDefinition {
    /// Next unused local ID (references and offers). Retired IDs are never
    /// reused.
    pub next_local: u32,
    /// The manufacturer's name as the user wrote it. Text, not identity.
    pub manufacturer: String,
    /// The complete manufacturer part number, ordering suffixes included.
    pub mpn: String,
    /// The generic component this part is an exact implementation of.
    pub component: ComponentRef,
    /// The physical implementation, listed by `component`.
    pub binding: BindingRef,
    /// Rated values, at most one per field lineage, each with its meaning.
    pub specs: Vec<RatedSpec>,
    pub references: Vec<Reference>,
    /// D3: offers are part of the revision.
    pub offers: Vec<DistributorOffer>,
    pub provenance: Provenance,
}

impl Default for PartDefinition {
    fn default() -> Self {
        Self {
            next_local: 1,
            manufacturer: String::new(),
            mpn: String::new(),
            component: ComponentRef::default(),
            binding: BindingRef::default(),
            specs: Vec::new(),
            references: Vec::new(),
            offers: Vec::new(),
            provenance: Provenance::default(),
        }
    }
}

/// What a reference document is.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Datasheet,
    ApplicationNote,
    Other,
}

/// A datasheet or application-note record. A link is not a copy: an
/// attached copy is named by `asset` (CE-07 keeps it when the URL is gone).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// Stable local ID within the Part lineage.
    pub id: u32,
    pub kind: ReferenceKind,
    pub title: String,
    pub url: Option<String>,
    /// The document's own revision label, as printed on it.
    pub document_revision: Option<String>,
    /// SHA-256 of an attached copy, lower-case hex (asset store: D7).
    pub asset: Option<String>,
    /// Licence of the document, as text (SPDX where one applies).
    pub licence: Option<String>,
    pub provenance: Provenance,
}

/// A distributor's identity for the Part, without price or stock (D9).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DistributorOffer {
    /// Stable local ID within the Part lineage.
    pub id: u32,
    pub distributor: String,
    pub sku: String,
    pub url: Option<String>,
    /// For example "cut tape" or "reel"; text only.
    pub packaging: Option<String>,
}

/// Where a revision or record came from. The free-text note stays in
/// `DefinitionMeta::provenance` for every kind.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Set when the item was imported from a library source (S3, D1).
    pub origin: Option<Origin>,
    pub basis: Basis,
    pub licence: Option<String>,
}

pub use super::bundle::Origin;

/// How the content was obtained.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "basis", content = "reference", rename_all = "snake_case")]
pub enum Basis {
    /// Written by the user; the default.
    #[default]
    Authored,
    /// Authored from one of this Part's references, by local ID.
    FromReference(u32),
    /// Copied from another library or file.
    Imported,
}

fn text_ok(text: &Option<String>, max: usize) -> bool {
    text.as_ref().is_none_or(|t| t.len() <= max)
}

impl Provenance {
    fn within_bounds(&self) -> bool {
        text_ok(&self.licence, MAX_LABEL)
    }
}

impl PartDefinition {
    pub(super) fn within_bounds(&self) -> bool {
        self.manufacturer.len() <= MAX_LABEL
            && self.mpn.len() <= MAX_LABEL
            && self.specs.len() <= MAX_RATED_SPECS
            && self.specs.iter().all(|s| s.value.within_bounds())
            && self.references.len() <= MAX_REFERENCES
            && self.references.iter().all(|r| {
                r.title.len() <= MAX_LABEL
                    && text_ok(&r.url, MAX_URL)
                    && text_ok(&r.document_revision, MAX_LABEL)
                    && text_ok(&r.asset, 64)
                    && text_ok(&r.licence, MAX_LABEL)
                    && r.provenance.within_bounds()
            })
            && self.offers.len() <= MAX_OFFERS
            && self.offers.iter().all(|o| {
                o.distributor.len() <= MAX_LABEL
                    && o.sku.len() <= MAX_LABEL
                    && text_ok(&o.url, MAX_URL)
                    && text_ok(&o.packaging, MAX_LABEL)
            })
            && self.provenance.within_bounds()
    }

    pub(super) fn values_finite(&self) -> bool {
        self.specs.iter().all(|s| s.value.values_finite())
    }

    /// Reference and offer local IDs.
    pub fn local_ids(&self) -> Vec<u32> {
        self.references
            .iter()
            .map(|r| r.id)
            .chain(self.offers.iter().map(|o| o.id))
            .collect()
    }

    /// Publish rules. Identity: a printable manufacturer and MPN. Pins: the
    /// component revision resolves and lists the binding revision exactly.
    /// Specs: each names a field revision that component selects, at most
    /// one per field lineage, with a value that satisfies that field and a
    /// meaning that fits its value type. Records: printable titles, names
    /// and SKUs, well-formed links and hashes, and a basis that names one of
    /// this Part's references.
    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        let named = |r: crate::ids::DefinitionRef| super::names::capitalised(&library.describe(r));
        if !printable(&self.manufacturer) || !printable(&self.mpn) {
            out.push(issue(
                "part.identity",
                "A Part needs a printable manufacturer and MPN of 1–256 bytes.".into(),
                vec![this],
            ));
        }
        let component = library.component(self.component);
        if component.is_none() {
            out.push(issue(
                "reference.missing",
                format!(
                    "{} is missing from the project library.",
                    named(self.component.erased())
                ),
                vec![
                    this,
                    ObjectRef::Definition {
                        id: self.component.id,
                    },
                ],
            ));
        }
        if library.binding(self.binding).is_none() {
            out.push(issue(
                "reference.missing",
                format!(
                    "{} is missing from the project library.",
                    named(self.binding.erased())
                ),
                vec![
                    this,
                    ObjectRef::Definition {
                        id: self.binding.id,
                    },
                ],
            ));
        }
        if let Some(component) = component {
            let component = &component.definition;
            if !component.implementations.contains(&self.binding) {
                out.push(issue(
                    "part.binding_unlisted",
                    format!(
                        "{} does not list {}.",
                        named(self.component.erased()),
                        library.describe(self.binding.erased())
                    ),
                    vec![
                        this,
                        ObjectRef::Definition {
                            id: self.binding.id,
                        },
                    ],
                ));
            }
            self.spec_findings(id, component, library, out);
        }
        self.record_findings(id, out);
    }

    fn spec_findings(
        &self,
        id: DefinitionId,
        component: &ComponentDefinition,
        library: &Library,
        out: &mut Vec<Finding>,
    ) {
        let mut lineages = BTreeSet::new();
        for spec in &self.specs {
            let objects = vec![
                ObjectRef::Definition { id },
                ObjectRef::Definition { id: spec.field.id },
            ];
            if !lineages.insert(spec.field.id) {
                out.push(issue(
                    "part.spec_duplicate",
                    format!(
                        "{} has more than one rated value.",
                        super::names::capitalised(&library.describe(spec.field.erased()))
                    ),
                    objects.clone(),
                ));
            }
            if component.field(spec.field.id).map(|s| s.field) != Some(spec.field) {
                out.push(issue(
                    "part.spec_field",
                    format!(
                        "{} does not select {}.",
                        super::names::capitalised(&library.describe(self.component.erased())),
                        library.describe(spec.field.erased())
                    ),
                    objects,
                ));
                continue;
            }
            let Some(view) = library.field_view(spec.field) else {
                out.push(issue(
                    "reference.missing",
                    format!(
                        "{} or its quantity kind is missing from the project library.",
                        super::names::capitalised(&library.describe(spec.field.erased()))
                    ),
                    objects,
                ));
                continue;
            };
            let value_type = &view.field.definition.value_type;
            let name = &view.field.meta.name;
            if let Err(error) = fields::check_value(&spec.value, value_type, view.kind_body()) {
                out.push(issue(
                    "part.spec_value",
                    format!("The rated {name} is invalid: {error}."),
                    objects.clone(),
                ));
            }
            if !spec.meaning.fits(value_type) {
                out.push(issue(
                    "part.spec_meaning",
                    format!(
                        "A {} cannot compare values of {name}.",
                        spec.meaning.label()
                    ),
                    objects,
                ));
            }
        }
    }

    fn record_findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let local = |l: u32| vec![object(id, l)];
        for reference in &self.references {
            if !printable(&reference.title)
                || !optional_label(&reference.document_revision)
                || !optional_label(&reference.licence)
            {
                out.push(issue(
                    "part.reference_text",
                    "A reference needs a printable title, revision and licence.".into(),
                    local(reference.id),
                ));
            }
            if !reference.url.as_deref().is_none_or(link) {
                out.push(issue(
                    "part.link",
                    format!(
                        "Reference ‘{}’ needs an http(s) link without spaces.",
                        reference.title
                    ),
                    local(reference.id),
                ));
            }
            if !reference.asset.as_deref().is_none_or(sha256_hex)
                || !reference.provenance.well_formed()
            {
                out.push(issue(
                    "part.hash",
                    format!(
                        "Reference ‘{}’ names a hash that is not 64 lower-case hex digits.",
                        reference.title
                    ),
                    local(reference.id),
                ));
            }
        }
        for offer in &self.offers {
            if !printable(&offer.distributor)
                || !printable(&offer.sku)
                || !optional_label(&offer.packaging)
            {
                out.push(issue(
                    "part.offer_text",
                    "An offer needs a printable distributor, SKU and packaging.".into(),
                    local(offer.id),
                ));
            }
            if !offer.url.as_deref().is_none_or(link) {
                out.push(issue(
                    "part.link",
                    format!(
                        "The offer of ‘{}’ needs an http(s) link without spaces.",
                        offer.distributor
                    ),
                    local(offer.id),
                ));
            }
        }
        let this = vec![ObjectRef::Definition { id }];
        if !self.provenance.well_formed() || !optional_label(&self.provenance.licence) {
            out.push(issue(
                "part.hash",
                "The provenance needs a printable source and a 64-digit lower-case hex hash."
                    .into(),
                this.clone(),
            ));
        }
        let bases =
            std::iter::once(&self.provenance).chain(self.references.iter().map(|r| &r.provenance));
        for provenance in bases {
            if let Basis::FromReference(r) = provenance.basis
                && !self.references.iter().any(|x| x.id == r)
            {
                out.push(issue(
                    "part.provenance_basis",
                    "The provenance names a reference this Part does not list.".into(),
                    this.clone(),
                ));
            }
        }
    }
}

fn optional_label(text: &Option<String>) -> bool {
    text.as_deref().is_none_or(printable)
}

/// An absolute http(s) link: bounded, without whitespace or control
/// characters. The link is never fetched.
fn link(url: &str) -> bool {
    let lower = url
        .get(..8)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && url.len() > 8
        && url.len() <= MAX_URL
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A SHA-256 digest as 64 lower-case hex digits.
fn sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Provenance {
    /// Origins are typed (`LibraryId`, `ContentHash`), so any decoded one
    /// is well formed.
    fn well_formed(&self) -> bool {
        true
    }
}
