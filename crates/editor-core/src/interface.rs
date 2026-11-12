//! The board interface: the datums an enclosure may reference, their
//! append-only slot table, the `interface.*` commands, the reports, the
//! findings and the exported sidecar (docs/architecture/board-interface.md).
//!
//! Slice B0 is the contract. This module owns the rules every later slice
//! shares and that are complete now: a key's syntax per role, a role's
//! kind, tokens built from ids only, slot allocation that never reuses a
//! slot, the slot table's invariants and the sidecar's versioned, bounded
//! reading. The commands, reports and findings are typed here; slice B1
//! wires the commands and `interface.list` into the engine, slice B2 adds
//! package datums, and slice B3 builds the exported design and the
//! sidecar. Until then no transport offers them.
//!
//! The stored data (`model::interface`) holds identity only. A datum's
//! geometry ([`DatumGeometry`]) is derived from the board and its parts
//! whenever it is needed, in board millimetres, and written to the sidecar.
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::library::Outline;
use crate::library::datum::{DatumKind, DatumName};
use crate::library::footprint_source::ParamName;
use crate::model::interface::{
    DatumKey, DatumOwner, DatumRole, InterfaceDatum, InterfaceTable, MAX_DATUMS, MAX_SLOTS, Slot,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub mod command;
pub mod findings;
pub mod report;
pub mod sidecar;
#[cfg(test)]
mod tests;

pub use command::{ExportTarget, InterfaceCommand, PublishParams, PublishPreviewParams};
pub use report::{
    ChangeKind, DatumChange, DatumRow, DryRun, EffectKind, EnclosureEffect, InterfaceList,
    PublishPreview, RuleState,
};
pub use sidecar::{FORMAT, Sidecar, SidecarDatum};

/// JSON schemas of the contract, as
/// `docs/architecture/board-interface.schema.json` records them: the
/// sidecar, the stored table, the commands, the host requests and the
/// reports.
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "sidecar": schemars::schema_for!(Sidecar),
        "table": schemars::schema_for!(InterfaceTable),
        "command": schemars::schema_for!(InterfaceCommand),
        "publish": schemars::schema_for!(PublishParams),
        "publish_preview": schemars::schema_for!(PublishPreviewParams),
        "list": schemars::schema_for!(InterfaceList),
        "preview": schemars::schema_for!(PublishPreview),
    })
}

/// Longest token, in bytes: `board.parameter.` and a parameter name.
pub const MAX_TOKEN: usize = 320;
/// Loops of one profile or volume datum, the outer one first.
pub const MAX_LOOPS: usize = 256;
/// Largest coordinate, length or value a datum's geometry may carry, in
/// millimetres (ten metres, well beyond any board).
pub const MAX_COORDINATE_MM: f64 = 10_000.0;

/// A point or a direction in board millimetres (x, y on the board's
/// frame, z up from the bottom face).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(transparent)]
pub struct Vec3(pub [f64; 3]);

impl Vec3 {
    /// Every component finite and within [`MAX_COORDINATE_MM`].
    pub fn valid(self) -> bool {
        self.0
            .iter()
            .all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE_MM)
    }
}

/// An origin and two perpendicular unit axes; the third is `x × y`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Frame {
    pub origin: Vec3,
    pub x: Vec3,
    pub y: Vec3,
}

/// A datum's geometry, in board millimetres. Profiles and volumes are
/// drawn in their frame's x–y plane; a volume rises along the frame's z.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DatumGeometry {
    Frame {
        frame: Frame,
    },
    Plane {
        origin: Vec3,
        normal: Vec3,
    },
    Axis {
        point: Vec3,
        direction: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diameter: Option<f64>,
    },
    Profile {
        frame: Frame,
        /// Closed loops, the outer one first, at most [`MAX_LOOPS`].
        loops: Vec<Outline>,
    },
    Volume {
        frame: Frame,
        loops: Vec<Outline>,
        height: f64,
    },
    Parameter {
        value: f64,
        /// `mm`, `deg` or empty for a count.
        unit: String,
    },
}

impl DatumGeometry {
    pub fn kind(&self) -> DatumKind {
        match self {
            Self::Frame { .. } => DatumKind::Frame,
            Self::Plane { .. } => DatumKind::Plane,
            Self::Axis { .. } => DatumKind::Axis,
            Self::Profile { .. } => DatumKind::Profile,
            Self::Volume { .. } => DatumKind::Volume,
            Self::Parameter { .. } => DatumKind::Parameter,
        }
    }

    /// Finite and within [`MAX_COORDINATE_MM`], loops within
    /// [`MAX_LOOPS`] and the outline segment bound.
    pub fn valid(&self) -> bool {
        let number = |v: f64| v.is_finite() && v.abs() <= MAX_COORDINATE_MM;
        let frame = |f: &Frame| f.origin.valid() && f.x.valid() && f.y.valid();
        let point = |p: Point| p.iter().all(|v| number(*v));
        let loops = |l: &[Outline]| {
            l.len() <= MAX_LOOPS
                && l.iter().all(|o| {
                    o.segments.len() <= crate::library::MAX_OUTLINE_SEGMENTS
                        && point(o.start)
                        && o.segments.iter().all(|s| point(s.to()))
                })
        };
        match self {
            Self::Frame { frame: f } => frame(f),
            Self::Plane { origin, normal } => origin.valid() && normal.valid(),
            Self::Axis {
                point,
                direction,
                diameter,
            } => point.valid() && direction.valid() && diameter.is_none_or(number),
            Self::Profile { frame: f, loops: l } => frame(f) && loops(l),
            Self::Volume {
                frame: f,
                loops: l,
                height,
            } => frame(f) && loops(l) && number(*height),
            Self::Parameter { value, unit } => number(*value) && unit.len() <= 16,
        }
    }
}

/// Whether a datum's reference still resolves, in the outcomes RFC 0001
/// and the model-reference trace share.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatumStatus {
    /// Exactly what was exported.
    Resolved,
    /// Found again, but by geometry rather than by name (a STEP face).
    Inexact,
    /// Its owner or feature is gone; the datum keeps its slot until it is
    /// unexported.
    Lost,
}

/// The kind a role always has, or `None` for a package datum or an
/// exported feature, whose kind comes from what they name.
pub fn role_kind(role: DatumRole) -> Option<DatumKind> {
    use DatumRole as R;
    match role {
        R::BoardFrame | R::PartFrame => Some(DatumKind::Frame),
        R::BoardTop | R::BoardBottom => Some(DatumKind::Plane),
        R::BoardOutline | R::BoardCutout => Some(DatumKind::Profile),
        R::BoardThickness | R::BoardParameter => Some(DatumKind::Parameter),
        R::Hole => Some(DatumKind::Axis),
        R::PartEnvelope | R::SideHeight => Some(DatumKind::Volume),
        R::PartDatum | R::Feature => None,
    }
}

/// The owner a role belongs to, and the syntax of its name: refused with
/// `interface.key` when the pair cannot occur.
pub fn check_key(key: &DatumKey) -> Result<()> {
    use DatumRole as R;
    let name = key.name.as_deref();
    let owner_ok = match key.role {
        R::BoardFrame
        | R::BoardTop
        | R::BoardBottom
        | R::BoardOutline
        | R::BoardThickness
        | R::BoardParameter
        | R::SideHeight => key.owner == DatumOwner::Board,
        R::BoardCutout => matches!(key.owner, DatumOwner::Curve { .. }),
        R::Hole => matches!(key.owner, DatumOwner::Hole { .. }),
        R::PartFrame | R::PartEnvelope | R::PartDatum => {
            matches!(key.owner, DatumOwner::Instance { .. })
        }
        R::Feature => !matches!(key.owner, DatumOwner::Curve { .. }),
    };
    let name_ok = match key.role {
        R::PartDatum => name.is_some_and(|n| DatumName(n.into()).valid_for_package()),
        R::BoardParameter => name.is_some_and(|n| ParamName(n.into()).valid()),
        R::SideHeight => matches!(name, Some("top" | "bottom")),
        _ => name.is_none(),
    };
    if owner_ok && name_ok {
        Ok(())
    } else {
        let role = serde_json::to_value(key.role)?;
        Err(Error::Invalid(format!(
            "interface.key: a {} datum cannot belong to this object with this name",
            role.as_str().unwrap_or("datum")
        )))
    }
}

/// The token a datum is written under in the exported design and sidecar,
/// built from ids only: `board.top`, `h12`, `i80.frame`, `i80.opening`,
/// `cut.l7`, `side.top`, `board.parameter.width`, `d412`. Renaming a part,
/// its designator or the datum never changes it. Refused with
/// `interface.token` when an id cannot be written in token syntax.
pub fn token(datum: &InterfaceDatum) -> Result<String> {
    check_key(&datum.key)?;
    let name = datum.key.name.as_deref().unwrap_or_default();
    use DatumRole as R;
    let text = match (&datum.key.owner, datum.key.role) {
        (_, R::Feature) => format!("d{}", datum.id.0),
        (DatumOwner::Board, R::BoardFrame) => "board.frame".into(),
        (DatumOwner::Board, R::BoardTop) => "board.top".into(),
        (DatumOwner::Board, R::BoardBottom) => "board.bottom".into(),
        (DatumOwner::Board, R::BoardOutline) => "board.outline".into(),
        (DatumOwner::Board, R::BoardThickness) => "board.thickness".into(),
        (DatumOwner::Board, R::BoardParameter) => format!("board.parameter.{name}"),
        (DatumOwner::Board, R::SideHeight) => format!("side.{name}"),
        (DatumOwner::Curve { id }, R::BoardCutout) => format!("cut.{}", id.0),
        (DatumOwner::Hole { id }, R::Hole) => format!("h{}", id.0),
        (DatumOwner::Instance { id }, R::PartFrame) => format!("i{id}.frame"),
        (DatumOwner::Instance { id }, R::PartEnvelope) => format!("i{id}.envelope"),
        (DatumOwner::Instance { id }, R::PartDatum) => format!("i{id}.{name}"),
        _ => String::new(),
    };
    if token_valid(&text) {
        Ok(text)
    } else {
        Err(Error::Invalid(format!(
            "interface.token: datum {} cannot be written as a token",
            datum.id
        )))
    }
}

/// Token syntax: dot-separated segments of `[A-Za-z0-9_]`, at most
/// [`MAX_TOKEN`] bytes.
pub fn token_valid(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TOKEN
        && text.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Allocate the next slot. Slots are never reused: the counter only grows,
/// and a retired slot stays retired. Refused with `interface.slots` at
/// [`MAX_SLOTS`].
pub fn allocate_slot(table: &mut InterfaceTable) -> Result<Slot> {
    if table.next_slot >= MAX_SLOTS {
        return Err(Error::Limit(format!(
            "interface.slots: a board's interface holds at most {MAX_SLOTS} slots over its \
             history; compacting them makes every enclosure reference to be picked again"
        )));
    }
    let slot = Slot(table.next_slot);
    table.next_slot = table.next_slot.saturating_add(1);
    Ok(slot)
}

/// Remove a live datum and retire its slot. Refused with
/// `interface.unknown` when no live datum has this slot.
pub fn retire(table: &mut InterfaceTable, slot: Slot) -> Result<InterfaceDatum> {
    let Some(at) = table.datums.iter().position(|d| d.slot == slot) else {
        return Err(Error::NotFound(format!(
            "interface.unknown: no exported datum has {slot}"
        )));
    };
    let datum = table.datums.remove(at);
    if let Err(i) = table.retired.binary_search(&slot) {
        table.retired.insert(i, slot);
    }
    Ok(datum)
}

/// The slot table's invariants, which document decode and every interface
/// command hold: at most [`MAX_DATUMS`] live datums in slot order; every
/// slot below `next_slot` and at most [`MAX_SLOTS`]; no slot both live and
/// retired, or used twice; unique datum ids and keys (feature datums are
/// told apart by their feature); valid keys and display names; a feature
/// only on feature datums; rules switched off listed once, ascending.
pub fn check_table(table: &InterfaceTable) -> Result<()> {
    let bad = |what: &str| Err(Error::Invalid(format!("interface.table: {what}")));
    if table.datums.len() > MAX_DATUMS {
        return Err(Error::Limit(format!(
            "interface.datums: at most {MAX_DATUMS} exported datums"
        )));
    }
    if table.next_slot > MAX_SLOTS {
        return bad("the slot counter is past its bound");
    }
    if !table.retired.windows(2).all(|w| w[0] < w[1])
        || !table.rules_off.windows(2).all(|w| w[0] < w[1])
        || !table.datums.windows(2).all(|w| w[0].slot < w[1].slot)
    {
        return bad("slots, retired slots and rules must be ascending and unique");
    }
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut features = BTreeSet::new();
    for datum in &table.datums {
        check_key(&datum.key)?;
        if datum.slot.0 >= table.next_slot {
            return bad("a datum's slot was never allocated");
        }
        if table.retired.binary_search(&datum.slot).is_ok() {
            return bad("a retired slot is in use again");
        }
        if !ids.insert(datum.id) {
            return bad("two datums share an id");
        }
        let is_feature = datum.key.role == DatumRole::Feature;
        if is_feature != datum.feature.is_some()
            || matches!(
                datum.feature,
                Some(crate::model::dimension::BoardRef::Housing(_))
            )
        {
            return bad("only a feature datum names a feature, and never a housing reference");
        }
        if !is_feature && !keys.insert(&datum.key) {
            return bad("two datums share a key");
        }
        if let Some(feature) = &datum.feature
            && !features.insert(serde_json::to_string(feature)?)
        {
            return bad("two datums export the same feature");
        }
        if datum
            .name
            .as_deref()
            .is_some_and(|n| !crate::ids::printable(n))
        {
            return bad("a display name is blank, too long or not printable");
        }
    }
    if table.retired.last().is_some_and(|s| s.0 >= table.next_slot) {
        return bad("a retired slot was never allocated");
    }
    Ok(())
}
