//! Copper fills (zones) and keepouts: commands, the deterministic fill
//! computation seam and its cache key, and fill validation.
//!
//! Computation is a pure function of the project ([`compute`]); its result
//! is saved in [`Fill::computed`] with the [`inputs_key`] it was computed
//! from, so a host can compute off the UI thread, commit the result with
//! `fill.store` (refused when the inputs moved on), and refill on load
//! only when a key is stale.
use crate::diagnostics::{Error, Result};
use crate::geometry::point_valid;
use crate::ids::Id;
use crate::library::{MAX_OUTLINE_SEGMENTS, Outline, Segment};
use crate::model::fill::{
    FillSettings, FillSpec, FilledCopper, KeepoutLayers, KeepoutSpec, MAX_FILLS, MAX_KEEPOUTS,
};
use crate::model::{Fill, Keepout, Project};
use crate::rules::MAX_RULE_MM;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The chord tolerance computed fill copper is flattened at, mm.
pub const FILL_CHORD_MM: f64 = 0.005;
/// Vertices of all computed regions of one fill.
pub const MAX_FILL_VERTICES: usize = 1 << 20;

/// Which fills to refill.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "fills", content = "id")]
pub enum RefillTarget {
    All,
    /// Only the stale ones (load, after edits).
    Stale,
    One(Id),
}

/// Fill and keepout mutations; each is one undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum FillCommand {
    /// Add a fill; the outcome names it (`Outcome::FillAdded`). Its
    /// copper is computed by a later `fill.refill` or `fill.store`.
    #[serde(rename = "fill.add")]
    Add { fill: Box<FillSpec> },
    /// Replace a fill's definition (identity kept; computed copper
    /// cleared). A locked fill is refused.
    #[serde(rename = "fill.edit")]
    Edit { fill: Box<Fill> },
    #[serde(rename = "fill.delete")]
    Delete { id: Id },
    /// Compute and store the copper of the target fills now.
    #[serde(rename = "fill.refill")]
    Refill { target: RefillTarget },
    /// Store copper the host computed with [`compute`]; refused unless
    /// `copper.key` equals the fill's current [`inputs_key`].
    #[serde(rename = "fill.store")]
    Store { fill: Id, copper: Box<FilledCopper> },
    /// Add a keepout; the outcome names it (`Outcome::KeepoutAdded`).
    #[serde(rename = "keepout.add")]
    AddKeepout { keepout: Box<KeepoutSpec> },
    #[serde(rename = "keepout.edit")]
    EditKeepout { keepout: Box<Keepout> },
    #[serde(rename = "keepout.delete")]
    DeleteKeepout { id: Id },
}

/// Apply a fill command to a candidate; returns the id `fill.add` or
/// `keepout.add` allocated.
pub fn apply(project: &mut Project, command: FillCommand) -> Result<Option<Id>> {
    let missing = |what: &str, id: Id| Error::NotFound(format!("{what}.missing: no {what} {id}"));
    match command {
        FillCommand::Add { fill } => {
            let id = project.allocate()?;
            project.fills.push(Fill::from_spec(id, *fill));
            Ok(Some(id))
        }
        FillCommand::Edit { fill } => {
            let slot = project
                .fills
                .iter_mut()
                .find(|f| f.id == fill.id)
                .ok_or_else(|| missing("fill", fill.id))?;
            if slot.locked && fill.locked {
                return Err(Error::Conflict(
                    "fill.locked: unlock the fill to edit it".into(),
                ));
            }
            *slot = Fill {
                computed: None,
                ..*fill
            };
            Ok(None)
        }
        FillCommand::Delete { id } => {
            let before = project.fills.len();
            project.fills.retain(|f| f.id != id);
            if project.fills.len() == before {
                return Err(missing("fill", id));
            }
            Ok(None)
        }
        FillCommand::Refill { target } => {
            let ids: Vec<Id> = project
                .fills
                .iter()
                .filter(|f| match target {
                    RefillTarget::All => true,
                    RefillTarget::Stale => is_stale(project, f),
                    RefillTarget::One(id) => f.id == id,
                })
                .map(|f| f.id)
                .collect();
            if let RefillTarget::One(id) = target
                && ids.is_empty()
            {
                return Err(missing("fill", id));
            }
            for id in ids {
                let copper = compute(project, id)?;
                if let Some(f) = project.fills.iter_mut().find(|f| f.id == id) {
                    f.computed = Some(copper);
                }
            }
            Ok(None)
        }
        FillCommand::Store { fill, copper } => {
            let key = inputs_key(project, fill)?;
            if copper.key != key {
                return Err(Error::Conflict(
                    "fill.stale: the board changed since this fill was computed".into(),
                ));
            }
            if let Some(f) = project.fills.iter_mut().find(|f| f.id == fill) {
                f.computed = Some(*copper);
            }
            Ok(None)
        }
        FillCommand::AddKeepout { keepout } => {
            let id = project.allocate()?;
            project.keepouts.push(Keepout::from_spec(id, *keepout));
            Ok(Some(id))
        }
        FillCommand::EditKeepout { keepout } => {
            let slot = project
                .keepouts
                .iter_mut()
                .find(|k| k.id == keepout.id)
                .ok_or_else(|| missing("keepout", keepout.id))?;
            *slot = *keepout;
            Ok(None)
        }
        FillCommand::DeleteKeepout { id } => {
            let before = project.keepouts.len();
            project.keepouts.retain(|k| k.id != id);
            if project.keepouts.len() == before {
                return Err(missing("keepout", id));
            }
            Ok(None)
        }
    }
}

/// The copper of fill `id`: its outline less board edge and cutout
/// clearance, other-net copper and holes at the effective clearances
/// (rules and net classes), higher-priority fills and keepouts, with
/// thermal reliefs, minimum-width erosion and island removal. Pure and
/// deterministic.
pub fn compute(project: &Project, id: Id) -> Result<FilledCopper> {
    project
        .fill(id)
        .ok_or_else(|| Error::NotFound(format!("fill.missing: no fill {id}")))?;
    Err(Error::Unsupported(
        "fill.compute: copper fill computation is not available in this build yet".into(),
    ))
}

/// FNV-1a over bytes, as 16 hex digits.
fn fnv_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Everything [`compute`] reads for one fill.
#[derive(Serialize)]
struct Inputs<'a> {
    fill: (
        &'a Option<Id>,
        &'a crate::model::CopperLayer,
        &'a Outline,
        u32,
        &'a FillSettings,
    ),
    rules: &'a crate::rules::ProjectRules,
    board: (&'a crate::geometry::Point, &'a crate::model::BoardShape),
    stackup: &'a crate::model::Stackup,
    placements: &'a [crate::model::Placement],
    tracks: &'a [crate::model::Track],
    vias: &'a [crate::model::Via],
    nets: &'a [crate::model::Net],
    others: Vec<(Id, &'a crate::model::CopperLayer, &'a Outline, u32)>,
    keepouts: &'a [Keepout],
}

/// The cache key of fill `id`'s inputs. Conservative: any change to
/// copper, rules, board, placements, nets, keepouts or another fill's
/// outline makes it stale (incremental refill narrows this later).
pub fn inputs_key(project: &Project, id: Id) -> Result<String> {
    let f = project
        .fill(id)
        .ok_or_else(|| Error::NotFound(format!("fill.missing: no fill {id}")))?;
    let inputs = Inputs {
        fill: (&f.net, &f.layer, &f.outline, f.priority, &f.settings),
        rules: &project.rules,
        board: (&project.board_size, &project.board_shape),
        stackup: &project.stackup,
        placements: &project.placements,
        tracks: &project.tracks,
        vias: &project.vias,
        nets: &project.nets,
        others: project
            .fills
            .iter()
            .filter(|o| o.id != id)
            .map(|o| (o.id, &o.layer, &o.outline, o.priority))
            .collect(),
        keepouts: &project.keepouts,
    };
    Ok(fnv_hex(&serde_json::to_vec(&inputs)?))
}

/// Whether a fill has no computed copper or copper of other inputs.
pub fn is_stale(project: &Project, fill: &Fill) -> bool {
    match (&fill.computed, inputs_key(project, fill.id)) {
        (Some(c), Ok(key)) => c.key != key,
        _ => true,
    }
}

/// Per-fill state for `fill.status`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FillStatus {
    pub fill: Id,
    pub stale: bool,
    pub regions: usize,
}

pub fn status(project: &Project) -> Vec<FillStatus> {
    project
        .fills
        .iter()
        .map(|f| FillStatus {
            fill: f.id,
            stale: is_stale(project, f),
            regions: f.computed.as_ref().map_or(0, |c| c.regions.len()),
        })
        .collect()
}

fn outline_ok(o: &Outline) -> bool {
    point_valid(o.start)
        && (3..=MAX_OUTLINE_SEGMENTS).contains(&o.segments.len())
        && o.closed()
        && o.segments.iter().all(|s| match s {
            Segment::Line { to } => point_valid(*to),
            Segment::Arc { to, bulge } => {
                point_valid(*to) && bulge.is_finite() && bulge.abs() <= 1e3
            }
        })
}

fn mm_ok(v: Option<f64>) -> bool {
    v.is_none_or(|mm| mm.is_finite() && (0.0..=MAX_RULE_MM).contains(&mm))
}

/// Structural bounds of fills and keepouts. A fill whose net is gone is
/// a check finding, not an invalid document.
pub fn validate(project: &Project) -> Result<()> {
    if project.fills.len() > MAX_FILLS || project.keepouts.len() > MAX_KEEPOUTS {
        return Err(Error::Limit(format!(
            "fill.limit: at most {MAX_FILLS} fills and {MAX_KEEPOUTS} keepouts"
        )));
    }
    for f in &project.fills {
        let s = &f.settings;
        let thermal_ok = s.thermal.is_none_or(|t| {
            mm_ok(Some(t.spoke_width))
                && mm_ok(Some(t.gap))
                && (1..=8).contains(&t.spoke_count)
                && t.angle_deg.is_finite()
        });
        let hatch_ok = s.hatch.is_none_or(|h| {
            h.width > 0.0
                && mm_ok(Some(h.width))
                && h.gap > 0.0
                && mm_ok(Some(h.gap))
                && h.angle_deg.is_finite()
        });
        let vertices = f.computed.as_ref().map_or(0, |c| {
            c.regions
                .iter()
                .map(|r| r.outer.len() + r.holes.iter().map(Vec::len).sum::<usize>())
                .sum()
        });
        if !outline_ok(&f.outline)
            || !project.stackup.has(f.layer)
            || f.name.len() > crate::rules::MAX_PROFILE_TEXT
            || !mm_ok(s.clearance)
            || !mm_ok(s.width_min)
            || !thermal_ok
            || !hatch_ok
            || vertices > MAX_FILL_VERTICES
        {
            return Err(Error::Invalid(
                "fill.bounds: a fill needs a closed finite outline on a stackup layer and settings within bounds"
                    .into(),
            ));
        }
    }
    for k in &project.keepouts {
        let layers_ok = match &k.layers {
            KeepoutLayers::All => true,
            KeepoutLayers::Only(layers) => {
                !layers.is_empty() && layers.iter().all(|l| project.stackup.has(*l))
            }
        };
        if !outline_ok(&k.outline) || !layers_ok || k.name.len() > crate::rules::MAX_PROFILE_TEXT {
            return Err(Error::Invalid(
                "keepout.bounds: a keepout needs a closed finite outline on stackup layers".into(),
            ));
        }
    }
    Ok(())
}
