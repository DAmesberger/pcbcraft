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
    FillRegion, FillSpec, FilledCopper, KeepoutLayers, KeepoutSpec,
};
use crate::model::{Fill, Keepout, Project};
use crate::rules::MAX_RULE_MM;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

mod checks;
mod compute;
pub(crate) mod geom;
#[cfg(test)]
mod tests;

pub(crate) use checks::{Fresh, fresh, fresh_on};
pub use checks::{fill_label, findings};
pub(crate) use geom::RegionIndex;

/// The chord tolerance computed fill copper is flattened at, mm.
pub const FILL_CHORD_MM: f64 = 0.005;

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
    /// Delete a copper fill.
    #[serde(rename = "fill.delete")]
    Delete { id: Id },
    /// Compute and store the copper of the target fills now.
    #[serde(rename = "fill.refill")]
    Refill { target: RefillTarget },
    /// Store copper the host computed with [`compute`]; refused unless
    /// `copper.key` equals the fill's current [`inputs_key`].
    #[serde(rename = "fill.store")]
    Store { fill: Id, copper: Box<FilledCopper> },
    /// Store copper the host computed for several fills, as one undo step. An
    /// explicit refill done off the UI thread; refused, storing nothing, unless
    /// every fill exists and every key is current.
    #[serde(rename = "fill.store_all")]
    StoreAll { fills: Vec<StoredFill> },
    /// Add a keepout; the outcome names it (`Outcome::KeepoutAdded`).
    #[serde(rename = "keepout.add")]
    AddKeepout { keepout: Box<KeepoutSpec> },
    /// Replace a keepout with an edited one of the same id.
    #[serde(rename = "keepout.edit")]
    EditKeepout { keepout: Box<Keepout> },
    /// Delete a keepout.
    #[serde(rename = "keepout.delete")]
    DeleteKeepout { id: Id },
}

// A new command chooses its permission class here.
declare_classes!(FillCommand {
    Add "fill.add" Edit "Add a copper fill",
    Edit "fill.edit" Edit "Edit a copper fill",
    Delete "fill.delete" Edit "Delete a copper fill",
    Refill "fill.refill" Edit "Refill copper",
    Store "fill.store" Edit "Store a fill",
    StoreAll "fill.store_all" Edit "Store all fills",
    AddKeepout "keepout.add" Edit "Add a keep-out",
    EditKeepout "keepout.edit" Edit "Edit a keep-out",
    DeleteKeepout "keepout.delete" Edit "Delete a keep-out",
});

/// One fill's computed copper for `fill.store_all`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoredFill {
    pub fill: Id,
    pub copper: FilledCopper,
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
                .fill(fill.id)
                .ok_or_else(|| missing("fill", fill.id))?;
            if slot.locked && fill.locked {
                return Err(Error::Conflict(
                    "fill.locked: unlock the fill to edit it".into(),
                ));
            }
            // A board-following fill's outline is derived: editing it
            // means making the fill a drawn one.
            if slot.region == FillRegion::Board
                && fill.region == FillRegion::Board
                && fill.outline != slot.outline
            {
                return Err(Error::Conflict(format!(
                    "fill.follows_board: the {} follows the board outline; make it a drawn fill to edit its outline",
                    fill_label(project, slot)
                )));
            }
            if let Some(slot) = project.fills.iter_mut().find(|f| f.id == fill.id) {
                *slot = Fill {
                    computed: None,
                    ..*fill
                };
            }
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
            // Fill copper keeps the profile's clearances; none to keep.
            project.rules.require()?;
            if let RefillTarget::One(id) = target
                && project.fill(id).is_none()
            {
                return Err(missing("fill", id));
            }
            for (id, copper) in refill(project, target)? {
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
        FillCommand::StoreAll { fills } => {
            // Each names a distinct fill of the board, so the board bounds
            // how many one request stores.
            if fills.is_empty() || fills.len() > project.fills.len() {
                return Err(Error::Limit(format!(
                    "fill.store_all: 1–{} fills at a time, the board's fills",
                    project.fills.len()
                )));
            }
            if let Some(gone) = fills.iter().find(|f| project.fill(f.fill).is_none()) {
                return Err(missing("fill", gone.fill));
            }
            let keys = inputs_keys_of(project, fills.iter().map(|f| f.fill))?;
            if fills
                .iter()
                .any(|f| keys.get(&f.fill) != Some(&f.copper.key))
            {
                return Err(Error::Conflict(
                    "fill.stale: the board changed since these fills were computed".into(),
                ));
            }
            for stored in fills {
                if let Some(f) = project.fills.iter_mut().find(|f| f.id == stored.fill) {
                    f.computed = Some(stored.copper);
                }
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

/// Give every board-following fill ([`FillRegion::Board`]) the solved
/// board outline, after any operation: a board parameter or shape edit
/// moves its region, and the next refill (the desktop's background
/// refill, or `fill.refill`) fills it. A part dimension on such a fill is
/// refused: the board, not a dimension, places it.
pub fn follow_board(project: &mut Project) -> Result<()> {
    if project.fills.iter().all(|f| f.region.is_drawn()) {
        return Ok(());
    }
    let dimensioned = project.part_dimensions.iter().find_map(|d| {
        d.refs.iter().find_map(|r| match r {
            crate::model::dimension::BoardRef::Part(f) => match f.target() {
                crate::model::board::ConstraintTarget::Fill(id) => {
                    project.fill(id).filter(|f| f.region == FillRegion::Board)
                }
                _ => None,
            },
            _ => None,
        })
    });
    if let Some(f) = dimensioned {
        return Err(Error::Conflict(format!(
            "fill.follows_board: the {} follows the board outline; a dimension cannot place it",
            fill_label(project, f)
        )));
    }
    let outer = project.board().outer.clone();
    for f in &mut project.fills {
        if f.region == FillRegion::Board && f.outline != outer {
            f.outline = outer.clone();
        }
    }
    Ok(())
}

/// The copper of fill `id`: its outline less board edge and cutout
/// clearance, other-net copper and holes at the effective clearances
/// (rules and net classes), higher-priority fills and keepouts, with
/// thermal reliefs, minimum-width erosion and island removal. Pure and
/// deterministic; fresh stored copper of higher-ranking fills is reused.
/// See [`compute`](self) for the steps.
pub fn compute(project: &Project, id: Id) -> Result<FilledCopper> {
    compute::Ctx::new(project).copper(id, false)
}

/// The fills `target` names, in rank order (highest priority first).
fn ranked(project: &Project, target: RefillTarget) -> Result<Vec<Id>> {
    let mut fills: Vec<&Fill> = project.fills.iter().collect();
    fills.sort_by(|a, b| {
        if compute::outranks(a, b) {
            std::cmp::Ordering::Less
        } else if compute::outranks(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    let mut ctx = compute::Ctx::new(project);
    let mut out = Vec::new();
    for f in fills {
        let wanted = match target {
            RefillTarget::All => true,
            RefillTarget::One(id) => f.id == id,
            RefillTarget::Stale => f.computed.as_ref().map(|c| &c.key) != Some(&ctx.key(f.id)?),
        };
        if wanted {
            out.push(f.id);
        }
    }
    Ok(out)
}

/// Compute the copper of the `target` fills in one pass (rank order, so a
/// fill sees the new copper of the fills above it). Pure: the caller
/// stores the results, as `fill.refill` does.
pub fn refill(project: &Project, target: RefillTarget) -> Result<Vec<(Id, FilledCopper)>> {
    let ids = ranked(project, target)?;
    let mut ctx = compute::Ctx::new(project);
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push((id, ctx.copper(id, false)?));
    }
    Ok(out)
}

/// Computed copper by input key, kept by a host across revisions so that
/// undo, redo and repeated edits reuse earlier results instead of
/// recomputing them. Bounded: the oldest entries go first.
#[derive(Debug, Default)]
pub struct FillCache {
    entries: BTreeMap<String, FilledCopper>,
    order: VecDeque<String>,
}

impl FillCache {
    /// Most results kept.
    pub const CAPACITY: usize = 64;

    pub fn get(&self, key: &str) -> Option<&FilledCopper> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, copper: FilledCopper) {
        if self.entries.contains_key(&copper.key) {
            return;
        }
        while self.order.len() >= Self::CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
        self.order.push_back(copper.key.clone());
        self.entries.insert(copper.key.clone(), copper);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The copper of every stale fill, for a host to compute off the UI
/// thread and commit with [`crate::Engine::store_fills`] (or `fill.store`).
/// Only fills whose inputs changed are computed; results already in
/// `cache` are reused, and new ones are added to it.
pub fn compute_stale(project: &Project, cache: &mut FillCache) -> Result<Vec<(Id, FilledCopper)>> {
    let ids = ranked(project, RefillTarget::All)?;
    let mut ctx = compute::Ctx::new(project);
    let mut out = Vec::new();
    for id in ids {
        let key = ctx.key(id)?;
        let stored = project.fill(id).and_then(|f| f.computed.as_ref());
        if stored.is_some_and(|c| c.key == key) {
            continue;
        }
        let copper = match cache.get(&key) {
            Some(hit) => {
                ctx.seed(id, hit.clone());
                hit.clone()
            }
            None => {
                let c = ctx.copper(id, true)?;
                cache.insert(c.clone());
                c
            }
        };
        out.push((id, copper));
    }
    Ok(out)
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

/// The cache key of fill `id`'s inputs: the fill's definition, the
/// rules, the board shape and stackup, and only the pads, vias, tracks,
/// keepouts and higher-ranking other-net fills within its reach. An edit
/// out of that reach (another layer, far away, a fill it does not clear)
/// leaves the key, so only affected fills go stale.
pub fn inputs_key(project: &Project, id: Id) -> Result<String> {
    compute::Ctx::new(project).key(id)
}

/// [`inputs_key`] of every fill, sharing one pass.
pub fn inputs_keys(project: &Project) -> Result<BTreeMap<Id, String>> {
    inputs_keys_of(project, project.fills.iter().map(|f| f.id))
}

/// [`inputs_key`] of the fills `ids` only, sharing one context (the keys
/// of higher-priority fills they depend on are computed as needed).
pub fn inputs_keys_of(
    project: &Project,
    ids: impl IntoIterator<Item = Id>,
) -> Result<BTreeMap<Id, String>> {
    let mut ctx = compute::Ctx::new(project);
    ids.into_iter().map(|id| Ok((id, ctx.key(id)?))).collect()
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
    let keys = inputs_keys(project).unwrap_or_default();
    project
        .fills
        .iter()
        .map(|f| FillStatus {
            fill: f.id,
            stale: f.computed.as_ref().map(|c| &c.key) != keys.get(&f.id),
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
        let copper_ok = f.computed.as_ref().is_none_or(|c| {
            c.key.len() <= 64
                && c.regions.iter().all(|r| {
                    r.outer.len() >= 3
                        && r.holes.iter().all(|h| h.len() >= 3)
                        && r.outer
                            .iter()
                            .chain(r.holes.iter().flatten())
                            .all(|p| point_valid(*p))
                })
        });
        if !outline_ok(&f.outline)
            || !project.stackup.has(f.layer)
            || f.name.len() > crate::rules::MAX_PROFILE_TEXT
            || !mm_ok(s.clearance)
            || !mm_ok(s.width_min)
            || !thermal_ok
            || !hatch_ok
            || !copper_ok
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

/// Where fill copper lies on one layer, for the 3D model's foil level:
/// whether a round-capped shape (a via ring, a track) overlaps it, found
/// through a row index of the regions' edges.
pub struct FillContact {
    index: geom::RegionIndex,
}

impl FillContact {
    pub fn new(regions: &[crate::model::fill::FilledRegion]) -> Self {
        let regions: Vec<geom::Region> = regions.iter().map(as_region).collect();
        Self {
            index: geom::RegionIndex::new(&regions),
        }
    }

    /// Whether the points within `radius` of segment `a`–`b` (a disc
    /// when they are one point) overlap the fill copper or come within
    /// `margin` of its edge.
    pub fn touches(
        &self,
        a: crate::geometry::Point,
        b: crate::geometry::Point,
        radius: f64,
        margin: f64,
    ) -> bool {
        let reach = radius + margin;
        self.index.segment_distance(a, b, reach) < reach || self.index.region_at(a).is_some()
    }
}

fn as_region(region: &crate::model::fill::FilledRegion) -> geom::Region {
    std::iter::once(region.outer.clone())
        .chain(region.holes.iter().cloned())
        .collect()
}

/// The union of fill regions and closed `rings` (copper outlines of either
/// orientation): disjoint regions, each an outer boundary
/// (counter-clockwise) and its holes. Each region keeps its holes as
/// holes; overlapping copper is one region.
pub fn copper_union(
    regions: &[crate::model::fill::FilledRegion],
    rings: &[Vec<crate::geometry::Point>],
) -> Vec<crate::model::fill::FilledRegion> {
    let mut all: Vec<geom::Region> = Vec::with_capacity(regions.len() + rings.len());
    for region in regions {
        let Some(outer) = geom::ccw(region.outer.clone()) else {
            continue;
        };
        let mut shape = vec![outer];
        for hole in &region.holes {
            if let Some(mut hole) = geom::ccw(hole.clone()) {
                hole.reverse();
                shape.push(hole);
            }
        }
        all.push(shape);
    }
    all.extend(
        rings
            .iter()
            .filter_map(|r| geom::ccw(r.clone()))
            .map(|r| vec![r]),
    );
    geom::union_regions(&all)
        .into_iter()
        .filter_map(|mut region| {
            if region.is_empty() {
                return None;
            }
            let outer = region.remove(0);
            Some(crate::model::fill::FilledRegion {
                outer,
                holes: region,
            })
        })
        .collect()
}

/// The outlines (counter-clockwise, flattened within [`FILL_CHORD_MM`]) of
/// the board and footprint keepouts that forbid fills on `layer`: where no
/// fill flows, as fill compute subtracts them.
pub(crate) fn fill_keepouts(
    project: &Project,
    layer: crate::model::CopperLayer,
) -> Vec<Vec<crate::geometry::Point>> {
    compute::effective_keepouts(project)
        .into_iter()
        .filter(|(k, _)| k.on(layer) && k.forbid.fills)
        .filter_map(|(k, _)| geom::ccw(k.outline.flatten(FILL_CHORD_MM)))
        .collect()
}
