//! Manufacturer parts (S2): one exact orderable item, its own revisioned
//! lineage pinning a generic component revision and one of its bindings. An
//! "atomic part" elsewhere is a component plus exactly one Part.
//!
//! Only identity the user chose is stored (D9): manufacturer, MPN,
//! distributor, SKU and links. No price, stock or supplier observation.
use super::publish::issue;
use super::{Library, printable};
use crate::diagnostics::{Finding, ObjectRef};
use crate::fields::{MAX_RATED_SPECS, RatedSpec};
use crate::ids::{BindingRef, ComponentRef, DefinitionId, MAX_LABEL, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

/// The library item an imported copy was made from (D1).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// The source library's identity, as text until S3 defines `LibraryId`.
    pub library: String,
    pub lineage: DefinitionId,
    pub revision: Revision,
    /// SHA-256 of the source revision's canonical content, lower-case hex.
    pub content_hash: String,
}

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
            && self
                .origin
                .as_ref()
                .is_none_or(|o| o.library.len() <= MAX_LABEL && o.content_hash.len() <= 64)
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

    /// Contract-level publish rules: identity text and pinned revisions that
    /// resolve. Spec typing, binding listing and the rest belong to S2 task 1.
    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        if !printable(&self.manufacturer) || !printable(&self.mpn) {
            out.push(issue(
                "part.identity",
                "A Part needs a printable manufacturer and MPN of 1–256 bytes.".into(),
                vec![this],
            ));
        }
        if library.component(self.component).is_none() {
            out.push(issue(
                "reference.missing",
                format!(
                    "Component {} is not in the project library.",
                    self.component
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
                format!("Binding {} is not in the project library.", self.binding),
                vec![
                    this,
                    ObjectRef::Definition {
                        id: self.binding.id,
                    },
                ],
            ));
        }
    }
}
