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
use super::{Definition, DefinitionMeta, Library};
use crate::diagnostics::{Error, Result};
use crate::ids::{
    ContentHash, DefinitionId, DefinitionKind, DefinitionRef, LibraryId, MAX_LABEL, Revision,
    SourceLineageId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Revision records one bundle carries (a root and its closure).
pub const MAX_BUNDLE_RECORDS: usize = 1024;
/// Links one record lists.
pub const MAX_LINKS: usize = 256;
/// Assets one bundle or project snapshot packs.
pub const MAX_ASSETS: usize = 256;
/// Bytes of one packed asset. The project file's 16 MiB bound applies to
/// the total.
pub const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
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

/// Asset bytes, written as standard base64 with padding.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetBytes(pub Vec<u8>);

impl Serialize for AssetBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&base64_encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for AssetBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        base64_decode(&text)
            .map(Self)
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
/// property test covers every kind.
pub fn rewrite_refs(definition: &mut Definition, map: &mut dyn RefMap) -> Result<()> {
    let _ = (definition, map);
    Err(Error::Unsupported(
        "library.rewrite: the reference-rewrite visitor is not implemented yet".into(),
    ))
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
/// `root` last. A dangling reference is an error naming it.
pub fn closure(library: &Library, root: DefinitionRef) -> Result<Vec<DefinitionRef>> {
    let _ = (library, root);
    Err(Error::Unsupported(
        "library.closure: dependency closures are not implemented yet".into(),
    ))
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

/// Structural checks of a bundle before anything is imported: bounds, every
/// record's links resolve inside the bundle, the root is present, assets
/// hash to their names. Publish rules run again on the project candidate.
pub fn validate_bundle(bundle: &Bundle) -> Result<()> {
    if bundle.name.len() > MAX_LABEL {
        return Err(Error::Limit("library name is too long".into()));
    }
    if bundle.records.is_empty() || bundle.records.len() > MAX_BUNDLE_RECORDS {
        return Err(Error::Limit(format!(
            "a bundle carries 1–{MAX_BUNDLE_RECORDS} revision records"
        )));
    }
    if !bundle
        .records
        .iter()
        .any(|r| r.lineage == bundle.root.lineage && r.revision == bundle.root.revision)
    {
        return Err(Error::Invalid(
            "the bundle does not contain its root revision".into(),
        ));
    }
    for record in &bundle.records {
        if record.links.len() > MAX_LINKS {
            return Err(Error::Limit(format!(
                "a revision record lists at most {MAX_LINKS} links"
            )));
        }
        if record.definition.kind() != record.kind {
            return Err(Error::Invalid(format!(
                "record {} {} says {:?} but holds a {:?}",
                record.lineage,
                record.revision,
                record.kind,
                record.definition.kind()
            )));
        }
    }
    validate_assets(&bundle.assets)
}

/// Bounds, media types and hashes of packed assets.
pub fn validate_assets(assets: &[PackedAsset]) -> Result<()> {
    if assets.len() > MAX_ASSETS {
        return Err(Error::Limit(format!("at most {MAX_ASSETS} packed assets")));
    }
    let mut seen = BTreeSet::new();
    for asset in assets {
        if asset.data.0.len() > MAX_ASSET_BYTES {
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
        if asset_hash(&asset.data.0) != asset.hash {
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

/// Snapshot checks of `Library::origins` and `Library::assets`: each origin
/// names a published revision once, and assets are well formed.
pub fn validate_snapshot(library: &Library) -> Result<()> {
    let mut seen = BTreeSet::new();
    for record in &library.origins {
        if library.definition(record.local).is_none() {
            return Err(Error::Invalid(format!(
                "origin of {} {} names a revision the project does not hold",
                record.local.id, record.local.revision
            )));
        }
        if !seen.insert(record.local) {
            return Err(Error::Invalid(format!(
                "{} {} has more than one origin",
                record.local.id, record.local.revision
            )));
        }
    }
    validate_assets(&library.assets)
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
