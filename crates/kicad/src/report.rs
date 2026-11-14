//! The import report: what mapped exactly, what was approximated, what is
//! only reported and what was refused, as counted rows under the case
//! codes of the importer design (§3), plus private details.
//!
//! Privacy: the rows and [`Report::summary`] hold fixed text and counts
//! only. Names from the file (designators, pad numbers, library names)
//! appear only in [`Detail`]s, which a host writes to a local file and
//! never prints.
use serde::Serialize;
use std::collections::BTreeMap;

/// How faithfully a case maps.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// Imported as it is.
    Exact,
    /// Imported close, within a stated bound.
    Approximate,
    /// Imported (or dropped) as it is and listed.
    Report,
    /// Not imported.
    Refused,
}

/// One case of the design's table.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    /// Pads whose copper, mask, paste and drill map one to one.
    PadsExact,
    /// F2: a token this importer does not know.
    UnknownToken,
    /// L1: every imported footprint is project-only.
    ProjectOnly,
    /// G2: footprint drawings on copper, mask or paste (slice K7).
    FootprintFabGraphics,
    /// G3: a custom pad whose union is not one outline without holes.
    CustomPadNotOneOutline,
    /// A custom-pad primitive this importer cannot draw (arcs, curves).
    CustomPadPrimitive,
    /// A pad with a token under it this importer does not know.
    PadUnknownToken,
    /// A pad shape this importer does not draw yet (trapezoid).
    PadShapeUnsupported,
    /// Custom pads whose round primitives became chords.
    CustomPadChords,
    /// Mask or paste openings of custom pads grown or shrunk with round
    /// joins that KiCad draws as chords.
    CustomPadOffset,
    /// A through-hole pad with a mask opening on one side only.
    OneSidedThroughMask,
    /// Paste with a relative margin on a pad that is not square, of a
    /// shape no window draws (custom): shrunk by the short side.
    PasteRatio,
    /// Paste with a relative margin on a pad that is not square, imported
    /// as a paste window of the opening KiCad draws.
    PasteRatioWindow,
    /// G4: footprint text and drawings (silkscreen, courtyard, fab).
    FootprintDrawings,
    /// G5: net-tie footprints: pads imported, tie behaviour missing.
    NetTie,
    /// G6: per-pad zone connection and clearance overrides.
    PadZoneOverrides,
    /// G6: footprint-wide zone connection and clearance overrides.
    FootprintZoneOverrides,
    /// G8: a pad with no copper (paste or mask only).
    PadWithoutCopper,
    /// G8: a paste-only pad, imported as a paste window of the pad under it.
    PasteWindow,
    /// A footprint with no electrical pad (a fiducial, a mounting hole):
    /// a part without terminals.
    NoTerminals,
    /// A footprint KiCad marks "not in schematic": a board-only part.
    BoardOnly,
    /// A board-only part's pad on a net: no pin on a sheet joins it.
    BoardOnlyNet,
    /// G8: a surface pad with copper on both outer sides.
    PadOnBothSides,
    /// A designator that was not valid or repeated, renumbered.
    Designator,
    /// G9/G10: board-level drawings, texts and dimensions (not imported).
    BoardObjects,
    /// G12: 3D model references.
    Models,
    /// A pad whose size, place or margins are beyond the coordinate bound.
    PadOutOfRange,
    /// No fab profile was given: no rules, so no outline, tracks or fills.
    NoRules,
    /// A board-wide setting the fab profile's bounds refused.
    RuleRefused,
    /// The physical stackup: copper, dielectric and mask thicknesses and
    /// Dk taken over (glass styles from the stackup family).
    StackupImported,
    /// A stated stackup outside the stackup bounds: the family's preset.
    StackupRefused,
    /// A dielectric of several sublayers, summed into one.
    StackupSublayers,
    /// The outline, imported as a board sketch.
    OutlineSketch,
    /// The outline as the rectangle from the origin around it (no board
    /// sketch engine).
    OutlineRectangle,
    /// `Edge.Cuts` drawings that do not close into one board: the board
    /// keeps its rectangle.
    OutlineNotClosed,
    /// No outline: the board keeps its new-board rectangle.
    OutlineMissing,
    /// An outline that cannot be imported (no rules, or reaching below the
    /// origin without a sketch engine).
    OutlineNotImported,
    /// Outline drawings that are curves or degenerate arcs.
    OutlineDrawing,
    /// `Edge.Cuts` drawings inside footprints.
    FootprintOutline,
    /// The project file's clearances and least track width, set over the
    /// fab profile.
    ProjectRules,
    /// No project file: the fab profile's clearances stand.
    NoProject,
    /// A project file that is not readable JSON: the profile's stand.
    ProjectUnreadable,
    /// A net class imported into the project's own rule set.
    NetClass,
    /// A net class the rules refused (a value out of bounds).
    NetClassRefused,
    /// A class pattern that is not a plain wildcard or names no imported
    /// class, or a net whose class was not imported.
    NetClassPatternRefused,
    /// Nets assigned to their class explicitly.
    NetClassAssigned,
    /// A KiCad limit looser than the fab profile's: the profile's kept.
    RuleLooserThanFab,
    /// Net names that are not label names (spaces, over 64 bytes, equal
    /// to another but for case), spelled anew.
    NetRenamed,
    /// Pads of one terminal on two nets: the first net kept.
    TerminalOnTwoNets,
    /// No schematic, or one that could not be read: pins labelled.
    SchematicUnreadable,
    /// Sheet symbols drawn (one per sheet instance).
    SheetSymbol,
    /// Symbols whose library symbol is missing from the file.
    SymbolWithoutLibrary,
    /// Symbols with no placed footprint of their reference.
    SymbolWithoutFootprint,
    /// Symbol pins with no pad of their number.
    PinWithoutPad,
    /// Footprints with no symbol in the schematic: their pins labelled.
    FootprintWithoutSymbol,
    /// Labels and power ports placed.
    SchematicLabel,
    /// Hidden power input pins, given a power port of their name.
    HiddenPowerPin,
    /// Pins of type "not connected", left unwired.
    NoConnectPin,
    /// Pins or labels on a wire's middle, with no wire end or junction
    /// there: joined to it.
    PinOnWire,
    /// Labels named as the net KiCad's wires join them into (KiCad lets
    /// one net carry several names; ours carry one).
    LabelAlias,
    /// Terminals no pin of their KiCad symbol names: a pin inside the body.
    TerminalWithoutPin,
    /// Units the engine refused to place or turn.
    UnitRefused,
    /// Symbol pins that meet no wire, label, other pin or no-connect flag.
    PinUnconnected,
    /// Labels, power ports and sheet pins the engine refused.
    SchematicItemRefused,
    /// Wires drawn.
    SchematicWire,
    /// Wires drawn straight, their corners refused.
    SchematicWireStraightened,
    /// Wires the engine refused.
    SchematicWireRefused,
    /// Arc tracks, imported as chords.
    ArcTrack,
    /// Tracks on no net with pads (or on no copper layer).
    TrackWithoutNet,
    /// Track pieces shorter than the engine's least track length, removed
    /// with their neighbours' ends joined.
    TinyTrackCleaned,
    /// Tracks the engine's checks refused (clearance, outline, width).
    TrackRefused,
    /// Vias on no net with pads.
    ViaWithoutNet,
    /// Vias the engine's checks refused.
    ViaRefused,
    /// Vias whose unused inner annular rings KiCad removes.
    ViaUnusedLayers,
    /// Zones imported as fills (one per copper layer).
    Zone,
    /// Rule areas imported as keepouts.
    Keepout,
    /// Zones not imported (arcs in the outline, no net with pads, no rules).
    ZoneRefused,
    /// Zones of several outline polygons, each a fill.
    ZonePolygons,
    /// Hatched zones, filled solid.
    ZoneHatched,
    /// Zones with corner smoothing, filled without it.
    ZoneSmoothing,
}

impl Case {
    /// The design's code, fidelity and a fixed description.
    pub fn describe(self) -> (&'static str, Fidelity, &'static str) {
        use Fidelity::*;
        match self {
            Self::PadsExact => ("-", Exact, "pads mapped one to one"),
            Self::UnknownToken => ("F2", Report, "unknown tokens outside pads, skipped"),
            Self::ProjectOnly => (
                "L1",
                Report,
                "footprints imported project-only (never publishable to a shared library)",
            ),
            Self::FootprintFabGraphics => (
                "G2",
                Refused,
                "footprint drawings on copper, mask or paste (slice K7)",
            ),
            Self::CustomPadNotOneOutline => (
                "G3",
                Refused,
                "custom pads whose union is not one outline without holes",
            ),
            Self::CustomPadPrimitive => (
                "G3",
                Refused,
                "custom pads with a primitive this importer cannot draw",
            ),
            Self::PadUnknownToken => ("F2", Refused, "pads with an unknown token"),
            Self::PadShapeUnsupported => ("-", Refused, "trapezoid pads, not drawn yet"),
            Self::CustomPadChords => (
                "G3",
                Approximate,
                "custom pads whose round primitives became chords within 0.25 µm",
            ),
            Self::CustomPadOffset => (
                "-",
                Approximate,
                "custom-pad mask or paste openings offset with round joins (KiCad's are chords)",
            ),
            Self::OneSidedThroughMask => (
                "-",
                Approximate,
                "through-hole pads with a mask opening on one side, opened on both",
            ),
            Self::PasteRatio => (
                "-",
                Approximate,
                "relative paste margins on custom pads that are not square, by the short side",
            ),
            Self::PasteRatioWindow => (
                "-",
                Exact,
                "relative paste margins on pads that are not square, imported as paste windows",
            ),
            Self::FootprintDrawings => (
                "G4",
                Report,
                "footprint text and drawings, not imported in this slice",
            ),
            Self::NetTie => (
                "G5",
                Report,
                "net-tie footprints: pads imported, tie missing",
            ),
            Self::PadZoneOverrides => (
                "G6",
                Exact,
                "pads with zone connection, thermal or clearance overrides, imported as pad fill overrides",
            ),
            Self::FootprintZoneOverrides => (
                "G6",
                Exact,
                "footprints with zone connection, thermal or clearance overrides, imported as part fill overrides",
            ),
            Self::PadWithoutCopper => (
                "G8",
                Refused,
                "pads without copper over no other pad's copper",
            ),
            Self::PasteWindow => (
                "G8",
                Exact,
                "paste-only pads imported as paste windows of the pad under them",
            ),
            Self::NoTerminals => (
                "-",
                Exact,
                "footprints without electrical pads, imported as parts without terminals",
            ),
            Self::BoardOnly => (
                "-",
                Exact,
                "footprints not in the schematic, imported as board-only parts",
            ),
            Self::BoardOnlyNet => (
                "N1",
                Refused,
                "board-only parts' pads on a net (no schematic pin joins them)",
            ),
            Self::PadOnBothSides => ("G8", Refused, "surface pads on both outer sides"),
            Self::Designator => ("-", Report, "designators renumbered"),
            Self::BoardObjects => (
                "G9",
                Report,
                "board-level drawings, texts and dimensions, not imported (slice K7)",
            ),
            Self::Models => ("G12", Report, "3D model references, ignored"),
            Self::PadOutOfRange => (
                "-",
                Refused,
                "pads with a size, place or margin beyond the coordinate bound",
            ),
            Self::NoRules => (
                "-",
                Refused,
                "no fab profile given: outline, tracks and fills not imported",
            ),
            Self::RuleRefused => (
                "-",
                Report,
                "board-wide settings (mask margin, via tenting) the fab profile refused",
            ),
            Self::StackupImported => (
                "S1",
                Exact,
                "stackup: copper, dielectric and mask thicknesses and Dk (glass from the family)",
            ),
            Self::StackupRefused => (
                "S1",
                Refused,
                "stackup outside the stackup bounds: the family's preset of its copper count",
            ),
            Self::StackupSublayers => (
                "S1",
                Approximate,
                "dielectrics of several sublayers, summed into one (first Dk kept)",
            ),
            Self::OutlineSketch => ("B1", Exact, "board outline imported as a board sketch"),
            Self::OutlineRectangle => (
                "B1",
                Approximate,
                "board outline as the rectangle around it (no board sketch engine)",
            ),
            Self::OutlineNotClosed => (
                "B1",
                Refused,
                "outline drawings that do not close into one board (rectangle kept)",
            ),
            Self::OutlineMissing => ("B1", Report, "no board outline (new-board rectangle kept)"),
            Self::OutlineNotImported => (
                "B1",
                Refused,
                "board outline not imported (no rules, or below the origin without a sketch engine)",
            ),
            Self::OutlineDrawing => (
                "B1",
                Refused,
                "outline curves (Béziers) and degenerate arcs, not imported",
            ),
            Self::FootprintOutline => (
                "B1",
                Refused,
                "outline drawings inside footprints, not imported",
            ),
            Self::ProjectRules => (
                "D1",
                Exact,
                "project rules (default class, board minimums) set over the fab profile as the project's own rule set",
            ),
            Self::NoProject => (
                "D1",
                Report,
                "no project file: the fab profile's clearances stand",
            ),
            Self::ProjectUnreadable => (
                "D1",
                Refused,
                "project file not readable: the fab profile's clearances stand",
            ),
            Self::NetClass => ("D1", Exact, "net classes imported"),
            Self::NetClassRefused => ("D1", Refused, "net classes the rules refused"),
            Self::NetClassPatternRefused => (
                "D1",
                Refused,
                "net class patterns that are not plain wildcards or name no imported class",
            ),
            Self::NetClassAssigned => (
                "D1",
                Exact,
                "nets assigned to their class explicitly (no pattern gives it on our names)",
            ),
            Self::RuleLooserThanFab => (
                "D1",
                Report,
                "KiCad limits looser than the fab profile's (the fab limit kept)",
            ),
            Self::NetRenamed => (
                "N1",
                Report,
                "net names spelled anew as label names (spaces, length, case)",
            ),
            Self::TerminalOnTwoNets => (
                "N1",
                Refused,
                "pads of one terminal on two nets (first net kept)",
            ),
            Self::SchematicUnreadable => (
                "K5",
                Refused,
                "schematic not readable: pins labelled with the board's nets",
            ),
            Self::SheetSymbol => ("K5", Exact, "sheet instances drawn as sheet symbols"),
            Self::SymbolWithoutLibrary => (
                "K5",
                Refused,
                "symbols whose library symbol is missing from the file",
            ),
            Self::SymbolWithoutFootprint => (
                "K5",
                Report,
                "symbols without a placed footprint (not on the board)",
            ),
            Self::PinWithoutPad => ("K5", Report, "symbol pins without a pad of their number"),
            Self::FootprintWithoutSymbol => (
                "K5",
                Approximate,
                "footprints without a symbol: their pins labelled with the board's nets",
            ),
            Self::SchematicLabel => ("K5", Exact, "labels and power ports placed"),
            Self::NoConnectPin => (
                "K5",
                Exact,
                "pins of type \"not connected\", left unwired (they join nothing)",
            ),
            Self::HiddenPowerPin => (
                "K5",
                Exact,
                "hidden power input pins, given a power port of their name",
            ),
            Self::PinOnWire => (
                "K5",
                Report,
                "pins or labels on a wire's middle without a junction (joined to it)",
            ),
            Self::LabelAlias => (
                "K5",
                Approximate,
                "labels renamed to the one name of the net KiCad's wires join them into",
            ),
            Self::TerminalWithoutPin => (
                "K5",
                Report,
                "pads without a pin of their number on the KiCad symbol (a pin inside the body)",
            ),
            Self::UnitRefused => ("K5", Refused, "symbol units the engine refused to place or turn"),
            Self::PinUnconnected => (
                "K5",
                Report,
                "symbol pins that meet no wire, label, other pin or no-connect flag",
            ),
            Self::SchematicItemRefused => (
                "K5",
                Refused,
                "labels, power ports and sheet pins the engine refused",
            ),
            Self::SchematicWire => ("K5", Exact, "wires drawn"),
            Self::SchematicWireStraightened => (
                "K5",
                Approximate,
                "wires drawn straight (their corners refused)",
            ),
            Self::SchematicWireRefused => ("K5", Refused, "wires the engine refused"),
            Self::ArcTrack => (
                "R1",
                Approximate,
                "arc tracks imported as chords within 0.25 µm",
            ),
            Self::TrackWithoutNet => ("R1", Refused, "tracks on no net with pads, not imported"),
            Self::TinyTrackCleaned => (
                "R1",
                Approximate,
                "track pieces shorter than 0.01 mm removed, their neighbours' ends joined",
            ),
            Self::TrackRefused => ("R1", Refused, "tracks the engine's checks refused"),
            Self::ViaWithoutNet => ("R1", Refused, "vias on no net with pads, not imported"),
            Self::ViaRefused => ("R1", Refused, "vias the engine's checks refused"),
            Self::ViaUnusedLayers => (
                "R1",
                Approximate,
                "vias without unused inner rings in KiCad, imported with rings on every layer",
            ),
            Self::Zone => (
                "Z1",
                Exact,
                "zone layers imported as fills, filled by our engine",
            ),
            Self::Keepout => ("Z1", Exact, "rule areas imported as keepouts"),
            Self::ZoneRefused => (
                "Z1",
                Refused,
                "zones not imported (arcs in the outline, no net with pads, no rules)",
            ),
            Self::ZonePolygons => ("Z1", Approximate, "zones of several outlines, each a fill"),
            Self::ZoneHatched => ("Z1", Approximate, "hatched zones, filled solid"),
            Self::ZoneSmoothing => (
                "Z1",
                Approximate,
                "zones with corner smoothing, filled without",
            ),
        }
    }
}

/// One counted row.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Row {
    pub case: Case,
    pub code: &'static str,
    pub fidelity: Fidelity,
    pub title: &'static str,
    pub count: usize,
}

/// A private detail of a row: which footprint (by designator) and pad, or
/// which token path. Written only to the local report file.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Detail {
    pub case: Case,
    pub footprint: String,
    pub pad: String,
    pub path: String,
}

/// Overall counts.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Counts {
    pub copper_layers: usize,
    pub footprints: usize,
    pub footprints_back: usize,
    pub definitions: usize,
    pub pads: usize,
    pub pads_imported: usize,
    /// Cutouts of the imported board outline.
    pub cutouts: usize,
    /// Nets with pads, tracks and vias imported, fills made.
    pub nets: usize,
    pub tracks: usize,
    pub vias: usize,
    pub fills: usize,
    /// Schematic sheets drawn.
    pub sheets: usize,
    /// Board nets whose pads the schematic joins exactly, net for net;
    /// split over several schematic nets (or unjoined); joined with
    /// another board net's pads.
    pub schematic_nets_equal: usize,
    pub schematic_nets_split: usize,
    pub schematic_nets_merged: usize,
}

/// Most private details kept (the rows still count every case).
pub const MAX_DETAILS: usize = 100_000;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Report {
    pub format_version: u32,
    pub counts: Counts,
    rows: BTreeMap<Case, usize>,
    details: Vec<Detail>,
}

impl Report {
    /// Count one occurrence of `case`.
    pub fn add(&mut self, case: Case) {
        *self.rows.entry(case).or_default() += 1;
    }

    /// Count `n` occurrences of `case`.
    pub fn add_n(&mut self, case: Case, n: usize) {
        if n > 0 {
            *self.rows.entry(case).or_default() += n;
        }
    }

    /// Count one occurrence with its private detail.
    pub fn add_detail(&mut self, case: Case, footprint: &str, pad: &str, path: &str) {
        self.add(case);
        if self.details.len() < MAX_DETAILS {
            self.details.push(Detail {
                case,
                footprint: footprint.to_owned(),
                pad: pad.to_owned(),
                path: path.to_owned(),
            });
        }
    }

    /// An unknown token outside pads, by path.
    pub fn unknown(&mut self, path: &str) {
        self.add_detail(Case::UnknownToken, "", "", path);
    }

    pub fn count(&self, case: Case) -> usize {
        self.rows.get(&case).copied().unwrap_or(0)
    }

    /// The counted rows, in case order.
    pub fn rows(&self) -> Vec<Row> {
        self.rows
            .iter()
            .map(|(case, count)| {
                let (code, fidelity, title) = case.describe();
                Row {
                    case: *case,
                    code,
                    fidelity,
                    title,
                    count: *count,
                }
            })
            .collect()
    }

    pub fn details(&self) -> &[Detail] {
        &self.details
    }

    /// The report for people: fixed text and counts only, never a name or
    /// value from the file.
    pub fn summary(&self) -> String {
        let c = &self.counts;
        let mut out = format!(
            "KiCad board format {}: {} copper layers, {} footprints ({} on the back), {} footprint definitions, {} of {} pads imported, {} nets, {} track pieces, {} vias, {} fills\n",
            self.format_version,
            c.copper_layers,
            c.footprints,
            c.footprints_back,
            c.definitions,
            c.pads_imported,
            c.pads,
            c.nets,
            c.tracks,
            c.vias,
            c.fills
        );
        if c.sheets > 0 {
            out.push_str(&format!(
                "  schematic: {} sheets; board nets equal {}, split {}, joined with another {}\n",
                c.sheets, c.schematic_nets_equal, c.schematic_nets_split, c.schematic_nets_merged
            ));
        }
        let mut rows = self.rows();
        rows.sort_by_key(|r| (r.fidelity, r.case));
        for row in rows {
            let fidelity = match row.fidelity {
                Fidelity::Exact => "exact",
                Fidelity::Approximate => "approximate",
                Fidelity::Report => "report",
                Fidelity::Refused => "refused",
            };
            out.push_str(&format!(
                "  {fidelity:<11} {:>3}  {:>6}  {}\n",
                row.code, row.count, row.title
            ));
        }
        out
    }

    /// The full report with its private details, for the local file.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "format_version": self.format_version,
            "counts": self.counts,
            "rows": self.rows(),
            "details": self.details,
        })
    }
}
