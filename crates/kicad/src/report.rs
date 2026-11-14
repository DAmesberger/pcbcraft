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
    /// G8: a pad with no copper (paste or mask only).
    PadWithoutCopper,
    /// G8: a paste-only pad, imported as a paste window of the pad under it.
    PasteWindow,
    /// A part with no electrical pad (a fiducial, a mounting pad): its first
    /// copper pad lands a terminal, as the core needs one.
    TerminalAdded,
    /// A footprint with no electrical pad (bare holes only): the core needs
    /// a part file for a part without terminals.
    FootprintWithoutTerminals,
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
    /// Several net classes, patterns or assignments: only the default
    /// class's clearance is taken (classes are not imported yet).
    NetClasses,
    /// Net names that are not label names (spaces, over 64 bytes, equal
    /// to another but for case), spelled anew.
    NetRenamed,
    /// Pads of one terminal on two nets: the first net kept.
    TerminalOnTwoNets,
    /// Arc tracks, imported as chords.
    ArcTrack,
    /// Tracks on no net with pads (or on no copper layer).
    TrackWithoutNet,
    /// Track pieces shorter than the engine's least track length.
    TrackTooShort,
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
                Report,
                "pads with zone connection or clearance overrides, not imported",
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
            Self::TerminalAdded => (
                "-",
                Report,
                "parts without an electrical pad: one terminal added on a copper pad",
            ),
            Self::FootprintWithoutTerminals => (
                "-",
                Refused,
                "footprints without copper pads (holes or drawings only; a part without terminals needs a part file)",
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
                "project clearances (net class, edge, hole) and least track width set over the fab profile",
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
            Self::NetClasses => (
                "D1",
                Approximate,
                "several net classes: only the default class's clearance taken",
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
            Self::ArcTrack => (
                "R1",
                Approximate,
                "arc tracks imported as chords within 0.25 µm",
            ),
            Self::TrackWithoutNet => ("R1", Refused, "tracks on no net with pads, not imported"),
            Self::TrackTooShort => ("R1", Refused, "track pieces shorter than 0.01 mm"),
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
