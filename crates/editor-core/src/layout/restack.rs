//! Stackup changes: `layout.set_stackup` and `layout.edit_stackup` share
//! this one path. A copper layer's identity is its name (`Inner(2)` is the
//! second foil below the top), so a stackup that gains or loses layers
//! renames the foils below the change. An edit says explicitly where the
//! copper of each old layer goes ([`LayerMove`]); every track, fill,
//! keepout and blind-via end on a moved layer moves with it, all at once,
//! so swaps work. Copper left on a layer the new stackup lacks refuses the
//! whole edit, naming the layer and what it still carries; nothing is
//! deleted implicitly.
//!
//! [`StackupDraft`] is the editing model the desktop stackup editor (and
//! any other client) builds an edit with: it tracks which old layer each
//! foil came from, so the moves follow from the edits.
use super::{Error, Result};
use crate::model::fill::KeepoutLayers;
use crate::model::{
    CopperLayer, DielectricKind, MAX_COPPER_LAYERS, Project,
    Stackup, StackupPresets, ViaSpan,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Move the copper of layer `from` of the current stackup to layer `to` of
/// the new one.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LayerMove {
    pub from: CopperLayer,
    pub to: CopperLayer,
}

/// A new stackup and where the copper of the old layers goes. Layers no
/// move names keep their name, except the `removed` ones: old layers no
/// foil of the new stackup continues, whose copper refuses the edit unless
/// a move sends it elsewhere (a removed layer's name may now belong to a
/// foil from below it).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StackupEdit {
    pub stackup: Stackup,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moves: Vec<LayerMove>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<CopperLayer>,
}

/// Replace the stackup and move copper between layers, as the caller's one
/// step. Refused, changing nothing the caller keeps, when the stackup
/// breaks its rules, a move names a layer either stackup lacks or names a
/// layer twice, or copper would be left on a layer the new stackup lacks.
pub(super) fn apply(project: &mut Project, edit: StackupEdit) -> Result<()> {
    let StackupEdit {
        stackup,
        moves,
        removed,
    } = edit;
    let stackup = stackup.keeping_unstated(&project.stackup);
    stackup.check()?;
    let map = move_map(&project.stackup, &stackup, &moves)?;
    if removed.len() > MAX_COPPER_LAYERS {
        return Err(Error::Invalid(format!(
            "a stackup edit removes at most {MAX_COPPER_LAYERS} layers"
        )));
    }
    if let Some(layer) = removed.iter().find(|l| !project.stackup.has(**l)) {
        return Err(Error::Invalid(format!(
            "the board has no {} layer to remove",
            layer.name()
        )));
    }
    // The new layer of copper on `layer`; `Err` names the old layer whose
    // copper has nowhere to go.
    let to = |layer: CopperLayer| match map.get(&layer) {
        Some(to) => Ok(*to),
        None if removed.contains(&layer) || !stackup.has(layer) => Err(layer),
        None => Ok(layer),
    };
    let mut left = BTreeMap::<CopperLayer, Carried>::new();
    for track in &mut project.tracks {
        match to(track.layer) {
            Ok(layer) => track.layer = layer,
            Err(layer) => left.entry(layer).or_default().tracks += 1,
        }
    }
    for fill in &mut project.fills {
        match to(fill.layer) {
            Ok(layer) => fill.layer = layer,
            Err(layer) => left.entry(layer).or_default().fills += 1,
        }
    }
    for keepout in &mut project.keepouts {
        if let KeepoutLayers::Only(layers) = &mut keepout.layers {
            for layer in layers.iter_mut() {
                match to(*layer) {
                    Ok(moved) => *layer = moved,
                    Err(gone) => left.entry(gone).or_default().keepouts += 1,
                }
            }
            layers.sort();
            layers.dedup();
        }
    }
    for via in &mut project.vias {
        if let ViaSpan::Between { from, to: end } = &mut via.span {
            for layer in [from, end] {
                match to(*layer) {
                    Ok(moved) => *layer = moved,
                    Err(gone) => left.entry(gone).or_default().vias += 1,
                }
            }
        }
    }
    if let Some((layer, carried)) = left.into_iter().next() {
        return Err(Error::Conflict(format!(
            "{} still carries {}; move or delete it first",
            layer.name(),
            carried.describe()
        )));
    }
    project.stackup = stackup;
    Ok(())
}

/// The moves as a map, checked: each `from` a layer of `old`, each `to` a
/// layer of `new`, no `from` twice.
fn move_map(
    old: &Stackup,
    new: &Stackup,
    moves: &[LayerMove],
) -> Result<BTreeMap<CopperLayer, CopperLayer>> {
    if moves.len() > MAX_COPPER_LAYERS {
        return Err(Error::Invalid(format!(
            "a stackup edit moves at most {MAX_COPPER_LAYERS} layers"
        )));
    }
    let mut map = BTreeMap::new();
    for m in moves {
        if !old.has(m.from) {
            return Err(Error::Invalid(format!(
                "the board has no {} layer to move copper from",
                m.from.name()
            )));
        }
        if !new.has(m.to) {
            return Err(Error::Invalid(format!(
                "the new stackup has no {} layer to move copper to",
                m.to.name()
            )));
        }
        if map.insert(m.from, m.to).is_some() {
            return Err(Error::Invalid(format!(
                "the edit moves {} copper twice",
                m.from.name()
            )));
        }
    }
    Ok(map)
}

/// What a layer still carries.
#[derive(Default)]
struct Carried {
    tracks: usize,
    fills: usize,
    keepouts: usize,
    vias: usize,
}

impl Carried {
    fn describe(&self) -> String {
        let part = |n: usize, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        [
            part(self.tracks, "track", "tracks"),
            part(self.fills, "fill", "fills"),
            part(self.keepouts, "keepout", "keepouts"),
            part(self.vias, "via", "vias"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
    }
}

/// One row of a stackup draft: a foil or the dielectric below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DraftRow {
    Foil(usize),
    Dielectric(usize),
}

/// A stackup being edited, with the old layer each foil came from
/// (`None`: added in this edit). [`StackupDraft::edit`] turns it into the
/// one `layout.edit_stackup` that applies it. Every operation keeps the
/// draft's shape (one dielectric between neighbouring foils) and refuses,
/// changing nothing, what the rules could never accept; the count and
/// value rules are [`Stackup::check`]'s, shown while editing.
#[derive(Clone, Debug, PartialEq)]
pub struct StackupDraft {
    pub stackup: Stackup,
    origin: Vec<Option<CopperLayer>>,
    old: Stackup,
}

impl StackupDraft {
    /// A draft of the board's stackup, unchanged.
    pub fn of(stackup: &Stackup) -> Self {
        Self {
            stackup: stackup.clone(),
            origin: stackup.layers().into_iter().map(Some).collect(),
            old: stackup.clone(),
        }
    }

    /// The old layer foil `index` came from.
    pub fn origin(&self, index: usize) -> Option<CopperLayer> {
        self.origin.get(index).copied().flatten()
    }

    /// Replace the draft's layers with a preset; foils keep their origin
    /// by position from the top while both have them, and the bottom foil
    /// stays the bottom.
    pub fn use_preset(&mut self, presets: &StackupPresets, count: usize) -> Result<()> {
        let preset = presets.preset(count)?;
        let old = self.origin.clone();
        let n = preset.copper.len();
        self.origin = (0..n)
            .map(|i| {
                if i + 1 == n {
                    old.last().copied().flatten()
                } else if i + 1 < old.len() {
                    old.get(i).copied().flatten()
                } else {
                    None
                }
            })
            .collect();
        self.stackup = Stackup {
            mask_top: self.stackup.mask_top,
            mask_bottom: self.stackup.mask_bottom,
            mask_um: self.stackup.mask_um,
            mask_laminate_nm: self.stackup.mask_laminate_nm,
            silkscreen_top: self.stackup.silkscreen_top,
            silkscreen_bottom: self.stackup.silkscreen_bottom,
            dk_estimates: self.stackup.dk_estimates.clone(),
            ..preset
        };
        Ok(())
    }

    /// Add one inner foil above the bottom foil, with a dielectric of
    /// `kind` above it, both as `presets` add them.
    pub fn add_layer(&mut self, presets: &StackupPresets, kind: DielectricKind) -> Result<()> {
        let count = self.stackup.copper.len();
        if count >= MAX_COPPER_LAYERS {
            return Err(Error::Invalid(format!(
                "a stackup has at most {MAX_COPPER_LAYERS} copper layers"
            )));
        }
        let dielectric = presets.added(kind);
        let at = count.saturating_sub(1);
        self.stackup.copper.insert(at, presets.foil());
        self.origin.insert(at, None);
        // Dielectric `at - 1` lay above the bottom foil; the new one goes
        // between it and the new foil.
        self.stackup
            .dielectrics
            .insert(at.saturating_sub(1), dielectric);
        Ok(())
    }

    /// Add two inner foils above the bottom foil: a prepreg, then a core
    /// (in a 4-layer foil build, prepreg · core · prepreg becomes
    /// prepreg · core · prepreg · core · prepreg).
    pub fn add_pair(&mut self, presets: &StackupPresets) -> Result<()> {
        if self.stackup.copper.len() + 2 > MAX_COPPER_LAYERS {
            return Err(Error::Invalid(format!(
                "a stackup has at most {MAX_COPPER_LAYERS} copper layers"
            )));
        }
        self.add_layer(presets, DielectricKind::Prepreg)?;
        self.add_layer(presets, DielectricKind::Core)
    }

    /// Remove inner foil `index` and the dielectric below it. Outer foils
    /// stay: a board always has its two sides.
    pub fn remove_layer(&mut self, index: usize) -> Result<()> {
        if index == 0 || index + 1 >= self.stackup.copper.len() {
            return Err(Error::Invalid(
                "only an inner copper layer can be removed".into(),
            ));
        }
        self.stackup.copper.remove(index);
        self.origin.remove(index);
        if index < self.stackup.dielectrics.len() {
            self.stackup.dielectrics.remove(index);
        }
        Ok(())
    }

    /// Swap row `row` with the row of its kind below it (`up`: above):
    /// two dielectrics trade places, or two inner foils, their copper going
    /// with them. Outer foils keep their place.
    pub fn move_row(&mut self, row: DraftRow, up: bool) -> Result<()> {
        let step = |i: usize| {
            if up {
                i.checked_sub(1)
            } else {
                i.checked_add(1)
            }
        };
        match row {
            DraftRow::Dielectric(i) => {
                let j = step(i)
                    .filter(|j| *j < self.stackup.dielectrics.len())
                    .ok_or_else(|| Error::Invalid("no dielectric to swap with".into()))?;
                self.stackup.dielectrics.swap(i, j);
            }
            DraftRow::Foil(i) => {
                let count = self.stackup.copper.len();
                let inner = |k: usize| k > 0 && k + 1 < count;
                let j = step(i)
                    .filter(|j| inner(i) && inner(*j))
                    .ok_or_else(|| Error::Invalid("only inner layers trade places".into()))?;
                self.stackup.copper.swap(i, j);
                self.origin.swap(i, j);
            }
        }
        Ok(())
    }

    /// The old layers this draft drops: no foil comes from them.
    pub fn removed(&self) -> Vec<CopperLayer> {
        self.old
            .layers()
            .into_iter()
            .filter(|l| !self.origin.contains(&Some(*l)))
            .collect()
    }

    /// Whether the draft differs from the stackup it started from.
    pub fn changed(&self) -> bool {
        self.stackup != self.old || !self.moves(&BTreeMap::new()).is_empty()
    }

    /// The moves the draft implies: each kept foil's copper follows it to
    /// its new name, and `dropped` sends a removed layer's copper to a
    /// layer of the new stackup.
    pub fn moves(&self, dropped: &BTreeMap<CopperLayer, CopperLayer>) -> Vec<LayerMove> {
        let layers = self.stackup.layers();
        let mut out: Vec<LayerMove> = self
            .origin
            .iter()
            .zip(&layers)
            .filter_map(|(from, to)| from.map(|from| LayerMove { from, to: *to }))
            .filter(|m| m.from != m.to)
            .collect();
        let removed = self.removed();
        out.extend(
            dropped
                .iter()
                .filter(|(from, to)| removed.contains(from) && layers.contains(to))
                .map(|(from, to)| LayerMove {
                    from: *from,
                    to: *to,
                }),
        );
        out
    }

    /// The edit that applies the draft.
    pub fn edit(&self, dropped: &BTreeMap<CopperLayer, CopperLayer>) -> StackupEdit {
        StackupEdit {
            stackup: self.stackup.clone(),
            moves: self.moves(dropped),
            removed: self.removed(),
        }
    }
}

#[cfg(test)]
#[path = "restack/tests.rs"]
mod tests;
