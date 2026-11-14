//! Synthetic KiCad boards built in Rust (decision D5: no `.kicad_*`
//! fixture files). Every board here is original test data: invented
//! footprints exercising each pad shape, angle, side, drill and margin.
#![allow(dead_code)]

use std::fmt::Write as _;

/// `sparksmith_kicad::import` onto the core's test-fixture board, stackup
/// presets and prototype fab profile, without a sketch engine.
pub fn import(bytes: &[u8]) -> Result<sparksmith_kicad::Imported, sparksmith_kicad::ImportError> {
    let fixture = |e: sparksmith_core::Error| sparksmith_kicad::ImportError::Refused {
        step: "fixture",
        reason: e.to_string(),
    };
    let presets = sparksmith_core::StackupPresets::fixture().map_err(fixture)?;
    let profile = sparksmith_core::rules::fixtures::prototype().map_err(fixture)?;
    sparksmith_kicad::import(
        bytes,
        &sparksmith_kicad::Setup {
            board: &sparksmith_core::NewBoard::fixture(),
            presets: &presets,
            profile: Some(&profile),
            evaluators: Default::default(),
            project_file: None,
        },
    )
}

/// One pad as written in a board file.
pub struct Pad {
    pub number: &'static str,
    pub kind: &'static str,
    pub shape: &'static str,
    /// x, y, angle (the angle on the board, the footprint's included).
    pub at: [f64; 3],
    pub size: [f64; 2],
    pub layers: &'static str,
    /// Further tokens: drill, margins, ratios, primitives.
    pub extra: String,
}

impl Pad {
    pub fn smd(number: &'static str, shape: &'static str, at: [f64; 3], size: [f64; 2]) -> Self {
        Self {
            number,
            kind: "smd",
            shape,
            at,
            size,
            layers: "\"F.Cu\" \"F.Paste\" \"F.Mask\"",
            extra: String::new(),
        }
    }
    pub fn back(mut self) -> Self {
        self.layers = "\"B.Cu\" \"B.Paste\" \"B.Mask\"";
        self
    }
    pub fn layers(mut self, layers: &'static str) -> Self {
        self.layers = layers;
        self
    }
    pub fn with(mut self, extra: &str) -> Self {
        self.extra.push(' ');
        self.extra.push_str(extra);
        self
    }
    pub fn tht(
        number: &'static str,
        shape: &'static str,
        at: [f64; 3],
        size: [f64; 2],
        drill: &str,
    ) -> Self {
        Self {
            number,
            kind: "thru_hole",
            shape,
            at,
            size,
            layers: "\"*.Cu\" \"*.Mask\"",
            extra: format!(" (drill {drill})"),
        }
    }
    pub fn npth(at: [f64; 3], size: f64) -> Self {
        Self {
            number: "",
            kind: "np_thru_hole",
            shape: "circle",
            at,
            size: [size, size],
            layers: "\"*.Cu\" \"*.Mask\"",
            extra: format!(" (drill {size})"),
        }
    }
}

pub struct Footprint {
    pub reference: String,
    pub at: [f64; 3],
    pub back: bool,
    pub extra: String,
    pub pads: Vec<Pad>,
}

impl Footprint {
    pub fn new(reference: &str, at: [f64; 3], back: bool, pads: Vec<Pad>) -> Self {
        Self {
            reference: reference.to_owned(),
            at,
            back,
            extra: String::new(),
            pads,
        }
    }
}

pub struct Board {
    pub copper: usize,
    pub origin: Option<[f64; 2]>,
    /// Extra `setup` tokens.
    pub setup: String,
    pub footprints: Vec<Footprint>,
    /// Board-level items after the footprints (nets, outline drawings,
    /// tracks, vias, zones), as board-file text.
    pub items: String,
}

fn n(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

impl Board {
    /// The board as KiCad 8 board-file text.
    pub fn text(&self) -> String {
        let mut uuid = 0u32;
        let mut next = || {
            uuid += 1;
            format!("00000000-0000-4000-8000-{uuid:012}")
        };
        let mut s = String::new();
        s.push_str("(kicad_pcb (version 20240108) (generator \"sparksmith_test\") (generator_version \"8.0\")\n");
        s.push_str(
            "  (general (thickness 1.6) (legacy_teardrops no))\n  (paper \"A4\")\n  (layers\n",
        );
        s.push_str("    (0 \"F.Cu\" signal)\n");
        for k in 1..self.copper.saturating_sub(1) {
            let _ = writeln!(s, "    ({k} \"In{k}.Cu\" signal)");
        }
        s.push_str("    (31 \"B.Cu\" signal)\n");
        for (id, name, kind) in [
            (34, "B.Paste", "user"),
            (35, "F.Paste", "user"),
            (36, "B.SilkS", "user \"B.Silkscreen\""),
            (37, "F.SilkS", "user \"F.Silkscreen\""),
            (38, "B.Mask", "user"),
            (39, "F.Mask", "user"),
            (44, "Edge.Cuts", "user"),
            (46, "B.CrtYd", "user \"B.Courtyard\""),
            (47, "F.CrtYd", "user \"F.Courtyard\""),
            (48, "B.Fab", "user"),
            (49, "F.Fab", "user"),
        ] {
            let _ = writeln!(s, "    ({id} \"{name}\" {kind})");
        }
        s.push_str("  )\n  (setup\n");
        let _ = writeln!(s, "    {}", self.setup);
        if let Some(o) = self.origin {
            let _ = writeln!(s, "    (aux_axis_origin {} {})", n(o[0]), n(o[1]));
        }
        s.push_str("  )\n  (net 0 \"\")\n");
        for fp in &self.footprints {
            let side = if fp.back { "B" } else { "F" };
            let _ = writeln!(
                s,
                "  (footprint \"Synthetic:{}\" (layer \"{side}.Cu\") (uuid \"{}\") (at {} {} {}){}",
                fp.reference,
                next(),
                n(fp.at[0]),
                n(fp.at[1]),
                n(fp.at[2]),
                fp.extra
            );
            let _ = writeln!(
                s,
                "    (property \"Reference\" \"{}\" (at 0 -2 0) (layer \"{side}.SilkS\") (hide yes) (uuid \"{}\") (effects (font (size 1 1) (thickness 0.15))))",
                fp.reference,
                next()
            );
            for pad in &fp.pads {
                let _ = writeln!(
                    s,
                    "    (pad \"{}\" {} {} (at {} {} {}) (size {} {}) (layers {}){} (uuid \"{}\"))",
                    pad.number,
                    pad.kind,
                    pad.shape,
                    n(pad.at[0]),
                    n(pad.at[1]),
                    n(pad.at[2]),
                    n(pad.size[0]),
                    n(pad.size[1]),
                    pad.layers,
                    pad.extra,
                    next()
                );
            }
            s.push_str("  )\n");
        }
        s.push_str(&self.items);
        s.push_str(")\n");
        s
    }
}

/// The footprint set every synthetic test uses: each pad shape at several
/// angles on both sides, custom pads with polygon and circle primitives,
/// chamfers, plated round and oval drills with offsets, non-plated holes,
/// and pad, footprint and board margins.
pub fn showcase() -> Board {
    let mut footprints = Vec::new();
    for (k, angle) in [0.0, 90.0, 180.0, 270.0, 30.0].into_iter().enumerate() {
        for back in [false, true] {
            let x = 110.0 + 12.0 * k as f64;
            let y = if back { 130.0 } else { 115.0 };
            let side = |p: Pad| if back { p.back() } else { p };
            // Pad angles on the board: the footprint's plus the local turn.
            let a = |local: f64| angle + local;
            let custom = "(options (clearance outline) (anchor rect)) (primitives (gr_poly (pts (xy -0.3 -0.3) (xy 1.2 -0.3) (xy 1.2 0.1) (xy -0.3 0.6)) (width 0) (fill yes)) (gr_circle (center 0 0.5) (end 0.35 0.5) (width 0) (fill yes)))";
            let pads = vec![
                side(Pad::smd("1", "rect", [-3.0, -2.0, a(0.0)], [1.0, 0.6])),
                side(Pad::smd("2", "roundrect", [-1.0, -2.0, a(90.0)], [1.2, 0.5]).with("(roundrect_rratio 0.25)")),
                side(Pad::smd("3", "circle", [1.0, -2.0, a(0.0)], [0.7, 0.7])),
                side(Pad::smd("4", "oval", [3.0, -2.0, a(45.0)], [1.4, 0.6])),
                side(Pad::smd("5", "custom", [-2.5, 0.5, a(0.0)], [0.4, 0.4]).with(custom)),
                side(Pad::smd("6", "roundrect", [0.0, 0.5, a(0.0)], [1.0, 0.8]).with("(roundrect_rratio 0) (chamfer_ratio 0.2) (chamfer top_left bottom_right)")),
                side(Pad::smd("12", "roundrect", [0.0, -0.8, a(90.0)], [1.0, 0.8]).with("(roundrect_rratio 0.25) (chamfer_ratio 0.15) (chamfer top_right)")),
                side(Pad::smd("7", "rect", [2.5, 0.5, a(0.0)], [0.8, 0.8]).with("(solder_mask_margin 0.1) (solder_paste_margin -0.05)")),
                side(Pad::smd("8", "roundrect", [-3.0, 2.5, a(0.0)], [1.0, 1.0]).with("(roundrect_rratio 0.5)")),
                side(Pad::smd("13", "custom", [4.5, -0.5, a(0.0)], [0.3, 0.3]).with("(options (clearance outline) (anchor rect)) (primitives (gr_poly (pts (xy -0.2 -0.4) (xy 0.6 -0.4) (xy 0.6 0.4) (xy -0.2 0.2)) (width 0.15) (fill yes)))")),
                Pad::tht("9", "rect", [-1.0, 2.5, a(0.0)], [1.6, 1.6], "0.8"),
                Pad::tht("10", "oval", [1.5, 2.5, a(0.0)], [2.0, 1.2], "oval 1.2 0.6 (offset 0.2 0)"),
                Pad::npth([3.5, 2.5, a(0.0)], 1.0),
            ];
            footprints.push(Footprint::new(
                &format!("U{}", footprints.len() + 1),
                [x, y, angle],
                back,
                pads,
            ));
        }
    }
    // A footprint-wide mask margin, and a surface pad on the far side.
    let mut fp = Footprint::new(
        "U99",
        [100.0, 100.0, 0.0],
        false,
        vec![
            Pad::smd("1", "rect", [0.0, 0.0, 0.0], [1.0, 1.0]),
            Pad::smd("2", "rect", [2.0, 0.0, 0.0], [1.0, 1.0]).back(),
            Pad::smd("3", "rect", [4.0, 0.0, 0.0], [1.0, 1.0]).with("(solder_paste_margin_ratio -0.1)"),
            Pad::smd("4", "roundrect", [6.0, 0.0, 0.0], [1.2, 0.6]).with("(roundrect_rratio 0.25) (solder_paste_margin -0.03) (solder_paste_margin_ratio -0.1)"),
            Pad::smd("5", "rect", [8.0, 0.0, 0.0], [0.6, 0.6]).with("(solder_mask_margin 0)"),
        ],
    );
    fp.extra = " (solder_mask_margin 0.07)".into();
    footprints.push(fp);
    Board {
        copper: 4,
        origin: Some([95.0, 140.0]),
        setup: "(pad_to_mask_clearance 0.05) (pad_to_paste_clearance -0.02)".into(),
        footprints,
        items: String::new(),
    }
}
