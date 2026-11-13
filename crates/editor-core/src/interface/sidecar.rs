//! The interface sidecar, `<name>.interface.json`, written beside the
//! exported design and copied into its document metadata
//! (docs/architecture/board-interface.md, "The export format").
//!
//! `format` carries a major version. A reader refuses a major it does not
//! know and ignores fields it does not know, since fields may be added
//! within a major; so these types never deny unknown fields. Slice B3
//! writes the sidecar; reading and checking it are complete here.
use super::{DatumGeometry, DatumStatus, token_valid};
use crate::diagnostics::{Error, Result};
use crate::ids::{ContentHash, InterfaceSourceId, printable};
use crate::library::datum::DatumKind;
use crate::model::interface::{DatumId, DatumOwner, MAX_DATUMS, MAX_SLOTS, Slot, Tier};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The format this build writes.
pub const FORMAT: &str = "sparksmith-board-interface/1";
/// The format's name, before the `/`.
pub const FORMAT_NAME: &str = "sparksmith-board-interface";
/// The one major version this build reads.
pub const FORMAT_MAJOR: u32 = 1;
/// The sidecar's units; the only ones written.
pub const UNITS: &str = "mm";
/// Largest sidecar this build reads, in bytes.
pub const MAX_SIDECAR_BYTES: usize = 16 * 1024 * 1024;
/// Named faces of one datum's carrier.
pub const MAX_FACES: usize = 64;

/// One published datum.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SidecarDatum {
    pub id: DatumId,
    pub slot: Slot,
    /// Built from ids only (`i80.opening`); the name the linked design's link
    /// stores.
    pub token: String,
    /// For people ("J1 opening"); never identity.
    pub name: String,
    /// `board.top`, `hole`, `part.frame`, `part.opening`, `feature`, ….
    pub role: String,
    pub kind: DatumKind,
    pub owner: DatumOwner,
    pub geometry: DatumGeometry,
    /// The clearance a package datum asks for around an opening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_mm: Option<f64>,
    pub status: DatumStatus,
    /// SHA-256 of this datum's canonical content, so a diff is per datum.
    pub hash: ContentHash,
    /// The carrier's faces by their role (`front`, `side`, `top`) and
    /// their persistent names in the exported design.
    #[serde(default)]
    pub faces: BTreeMap<String, String>,
}

/// The whole sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Sidecar {
    /// [`FORMAT`].
    pub format: String,
    /// The board's permanent interface identity.
    pub source_id: InterfaceSourceId,
    /// Counts publishes whose content hash differed, from 1.
    pub revision: u32,
    /// SHA-256 of the canonical datum table.
    pub hash: ContentHash,
    /// Always [`UNITS`].
    pub units: String,
    pub tier: Tier,
    /// In slot order.
    pub datums: Vec<SidecarDatum>,
    /// Ascending.
    #[serde(default)]
    pub retired_slots: Vec<Slot>,
    /// Live datums the design carries nothing for, ascending: lost ones,
    /// and parameters whose value is an expression the core cannot
    /// evaluate. Their slots keep their place, and an assembly reference
    /// to one fails until it resolves again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_slots: Vec<Slot>,
}

#[derive(Deserialize)]
struct Header {
    format: String,
}

/// The major version of a `format` text, or `None` when it is not this
/// format's name followed by `/` and a number.
pub fn format_major(format: &str) -> Option<u32> {
    let (name, major) = format.split_once('/')?;
    if name != FORMAT_NAME || major.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    major.parse().ok()
}

impl Sidecar {
    /// Read a sidecar: at most [`MAX_SIDECAR_BYTES`]; refused with
    /// `interface.format` when it is not a board interface, and with
    /// `interface.format_newer` when its major version is not
    /// [`FORMAT_MAJOR`]; then checked ([`Self::check`]).
    pub fn read(text: &str) -> Result<Self> {
        if text.len() > MAX_SIDECAR_BYTES {
            return Err(Error::Limit(format!(
                "interface.sidecar_size: a board interface file is at most {MAX_SIDECAR_BYTES} bytes"
            )));
        }
        let header: Header = serde_json::from_str(text).map_err(|_| {
            Error::Invalid("interface.format: this is not a board interface file".into())
        })?;
        match format_major(&header.format) {
            Some(FORMAT_MAJOR) => {}
            Some(_) => {
                return Err(Error::Unsupported(format!(
                    "interface.format_newer: this board interface was written in a format \
                     (`{}`) this Sparksmith cannot read",
                    header.format.chars().take(64).collect::<String>()
                )));
            }
            None => {
                return Err(Error::Invalid(
                    "interface.format: this is not a board interface file".into(),
                ));
            }
        }
        let sidecar: Self = serde_json::from_str(text)?;
        sidecar.check()?;
        Ok(sidecar)
    }

    /// Bounds and identity: units `mm`; at most [`MAX_DATUMS`] datums in
    /// ascending slot order, below [`MAX_SLOTS`]; tokens valid and unique;
    /// ids unique; no slot both live and retired; names printable;
    /// geometry finite and bounded and of the datum's kind; faces bounded.
    pub fn check(&self) -> Result<()> {
        let bad = |what: &str| Err(Error::Invalid(format!("interface.sidecar: {what}")));
        if self.units != UNITS {
            return bad("units must be mm");
        }
        if self.datums.len() > MAX_DATUMS || self.retired_slots.len() > MAX_SLOTS as usize {
            return Err(Error::Limit(format!(
                "interface.datums: at most {MAX_DATUMS} datums and {MAX_SLOTS} slots"
            )));
        }
        if !self.datums.windows(2).all(|w| w[0].slot < w[1].slot)
            || !self.retired_slots.windows(2).all(|w| w[0] < w[1])
            || !self.unresolved_slots.windows(2).all(|w| w[0] < w[1])
        {
            return bad("slots must be ascending and unique");
        }
        let mut tokens = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for datum in &self.datums {
            if datum.slot.0 >= MAX_SLOTS
                || self.retired_slots.binary_search(&datum.slot).is_ok()
                || self.unresolved_slots.binary_search(&datum.slot).is_ok()
            {
                return bad("a datum's slot is out of range or retired");
            }
            if !token_valid(&datum.token) || !tokens.insert(datum.token.as_str()) {
                return bad("a token is malformed or used twice");
            }
            if !ids.insert(datum.id) {
                return bad("two datums share an id");
            }
            if !printable(&datum.name) || !printable(&datum.role) {
                return bad("a name or role is blank, too long or not printable");
            }
            if !datum.geometry.valid()
                || datum.geometry.kind() != datum.kind
                || datum.clearance_mm.is_some_and(|c| {
                    !c.is_finite() || !(0.0..=super::MAX_COORDINATE_MM).contains(&c)
                })
            {
                return bad("a datum's geometry is out of range or not of its kind");
            }
            if datum.faces.len() > MAX_FACES
                || datum
                    .faces
                    .iter()
                    .any(|(role, name)| !printable(role) || !printable(name))
            {
                return bad("a datum names too many faces, or a face name is not printable");
            }
        }
        if self.retired_slots.last().is_some_and(|s| s.0 >= MAX_SLOTS)
            || self
                .unresolved_slots
                .last()
                .is_some_and(|s| s.0 >= MAX_SLOTS)
            || self.unresolved_slots.len() > MAX_DATUMS
            || self
                .unresolved_slots
                .iter()
                .any(|s| self.retired_slots.binary_search(s).is_ok())
        {
            return bad("a retired or unresolved slot is out of range or listed twice");
        }
        Ok(())
    }

    /// The number of slots the exported design lays out: one past the
    /// highest live, retired or unresolved slot.
    pub fn slot_count(&self) -> u32 {
        let last = self
            .datums
            .last()
            .map(|d| d.slot)
            .into_iter()
            .chain(self.retired_slots.last().copied())
            .chain(self.unresolved_slots.last().copied())
            .max();
        last.map_or(0, |s| s.0.saturating_add(1))
    }

    /// The sidecar as written: pretty JSON with a final newline. Field
    /// order is the types' and maps are sorted, so the same sidecar is the
    /// same bytes on every machine.
    pub fn text(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }
}
