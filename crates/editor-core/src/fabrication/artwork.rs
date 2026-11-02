//! The Gerber X2 layers of a board: one file per copper layer, solder mask
//! and paste per side, legend per side and the board profile.
//!
//! - **Copper** (`Copper,Ln,Top|Inr|Bot`, positive). Computed fill copper
//!   first, largest region first: each region dark, its holes clear, so a
//!   later (smaller) region inside a hole survives and nothing drawn after
//!   the fills is ever erased. Then tracks (round aperture draws, so the
//!   ends are exactly round), vias, board mounting-hole rings and pads.
//!   Pads are flashed: circles and axis-aligned rectangles and obrounds as
//!   standard apertures, rotated ones and rounded rectangles as macros of
//!   exact rectangles and circles; custom outlines are regions with exact
//!   arcs. Object attributes name the net (`.N`), the pad (`.P`) and the
//!   component (`.C`).
//! - **Solder mask** (`Soldermask`, negative: the image is the openings).
//!   Each pad's land grown by its mask rule (inherited pads by the design
//!   rules' mask expansion), open vias grown the same, mounting holes.
//! - **Paste** (`Paste`, positive). Each surface pad's land shrunk by its
//!   paste rule (inherited pads by the rules' paste reduction), or its
//!   explicit windows.
//! - **Legend** (`Legend`, positive). Footprint silkscreen strokes with a
//!   round pen and filled shapes as regions; text in Pcbcraft's stroke font
//!   (`layout::legend`). The legend is not clipped at mask openings; the
//!   pre-flight check reports ink over pads.
//! - **Profile** (`Profile,NP`). The outer outline, cutouts and non-plated
//!   slots, with exact arcs. Plated straight slots are routed in the plated
//!   drill file instead.
use super::gerber::{Aperture, Seg, Writer, escape, macro_body, units};
use super::{FabFile, FabricationOptions, FileGroup};
use crate::diagnostics::Result;
use crate::geometry::{Point, Shape, Transform2, rounded_path, signed_area2};
use crate::ids::Id;
use crate::layout::{self, PlacedPad};
use crate::layout::{BoardDrawing, PlacedItem};
use crate::library::{CHECK_CHORD_MM, MaskRule, Outline, PadKind, PadShape, PasteRule, Segment};
use crate::model::board::{CutoutKind, arc_geometry};
use crate::model::{CopperLayer, Layer, Project};
use std::collections::BTreeMap;

/// The pen the profile is drawn with, mm (its centre line is the edge).
pub const PROFILE_PEN_MM: f64 = 0.05;

/// The file attributes every layer of one set shares.
pub(super) fn common_attributes(
    project: &Project,
    options: &FabricationOptions,
) -> Vec<(&'static str, String)> {
    let mut out = vec![(
        "GenerationSoftware",
        format!("Pcbcraft,Pcbcraft,{}", env!("CARGO_PKG_VERSION")),
    )];
    if let Some(date) = &options.creation_date {
        out.push(("CreationDate", escape(date)));
    }
    out.push((
        "ProjectId",
        format!("{},{},1", escape(&project.name), project_guid(project)),
    ));
    out.push(("SameCoordinates", String::new()));
    out
}

/// A stable RFC 4122-shaped identifier of the project, derived from its
/// name (version nibble 8, custom), so the same design always carries the
/// same id.
pub(super) fn project_guid(project: &Project) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("pcbcraft project {}", project.name).as_bytes());
    let mut b = [0u8; 16];
    for (i, byte) in digest.iter().take(16).enumerate() {
        b[i] = *byte;
    }
    b[6] = (b[6] & 0x0f) | 0x80;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The X2 file function of a copper layer: `Copper,L2,Inr`.
pub fn copper_function(project: &Project, layer: CopperLayer) -> String {
    let n = project.stackup.index(layer).map_or(1, |i| i + 1);
    let side = match layer {
        CopperLayer::Top => "Top",
        CopperLayer::Bottom => "Bot",
        CopperLayer::Inner(_) => "Inr",
    };
    format!("Copper,L{n},{side}")
}

/// The file name part of a copper layer: `copper-l2-inner1`.
pub fn copper_name(project: &Project, layer: CopperLayer) -> String {
    let n = project.stackup.index(layer).map_or(1, |i| i + 1);
    match layer {
        CopperLayer::Top => format!("copper-l{n}-top"),
        CopperLayer::Bottom => format!("copper-l{n}-bottom"),
        CopperLayer::Inner(k) => format!("copper-l{n}-inner{k}"),
    }
}

fn side_word(side: Layer) -> &'static str {
    match side {
        Layer::Top => "top",
        Layer::Bottom => "bottom",
    }
}

fn side_code(side: Layer) -> &'static str {
    match side {
        Layer::Top => "Top",
        Layer::Bottom => "Bot",
    }
}

/// What the layers read once per set.
struct Board<'a> {
    project: &'a Project,
    pads: Vec<PlacedPad>,
    labels: BTreeMap<(Id, crate::ids::PadId), String>,
    designators: BTreeMap<Id, String>,
    mask_expansion: f64,
    paste_reduction: f64,
}

impl<'a> Board<'a> {
    fn of(project: &'a Project) -> Self {
        let rules = project.rules.effective();
        Self {
            project,
            pads: layout::placed_pads(project),
            labels: layout::pad_labels(project),
            designators: project
                .instances
                .iter()
                .map(|i| (i.id, i.designator.clone()))
                .collect(),
            mask_expansion: rules.mask.expansion,
            paste_reduction: rules.paste.reduction,
        }
    }

    fn net(&self, net: Option<Id>) -> String {
        net.map(|n| escape(self.project.net_name(n)))
            .unwrap_or_default()
    }

    fn designator(&self, instance: Id) -> String {
        escape(self.designators.get(&instance).map_or("", String::as_str))
    }

    fn pad_attributes(&self, pad: &PlacedPad) -> Vec<(&'static str, String)> {
        let designator = self.designator(pad.instance);
        let label = self
            .labels
            .get(&(pad.instance, pad.pad.id))
            .map(|l| escape(l))
            .unwrap_or_default();
        let mut out = vec![("C", designator.clone())];
        if !label.is_empty() {
            out.push(("P", format!("{designator},{label}")));
        }
        out.push(("N", self.net(pad.net)));
        out
    }
}

/// Every Gerber layer file of `project`, copper top to bottom, then mask,
/// paste and legend (top, bottom) and the profile.
pub fn layers(project: &Project, options: &FabricationOptions, stem: &str) -> Result<Vec<FabFile>> {
    let board = Board::of(project);
    let common = common_attributes(project, options);
    let mut files = Vec::new();
    for layer in project.stackup.layers() {
        let mut attributes = common.clone();
        attributes.push(("FileFunction", copper_function(project, layer)));
        attributes.push(("FilePolarity", "Positive".into()));
        let bytes = copper(&board, layer, &attributes)?;
        files.push(FabFile {
            name: format!("{stem}-{}.gbr", copper_name(project, layer)),
            group: FileGroup::Gerber,
            description: format!("{} copper", layer.name()),
            bytes,
        });
    }
    for side in [Layer::Top, Layer::Bottom] {
        let masked = match side {
            Layer::Top => project.stackup.mask_top,
            Layer::Bottom => project.stackup.mask_bottom,
        };
        if !masked {
            continue;
        }
        let mut attributes = common.clone();
        attributes.push(("FileFunction", format!("Soldermask,{}", side_code(side))));
        attributes.push(("FilePolarity", "Negative".into()));
        files.push(FabFile {
            name: format!("{stem}-mask-{}.gbr", side_word(side)),
            group: FileGroup::Gerber,
            description: format!("Solder mask {} (openings)", side_word(side)),
            bytes: mask(&board, side, &attributes)?,
        });
    }
    for side in [Layer::Top, Layer::Bottom] {
        let mut attributes = common.clone();
        attributes.push(("FileFunction", format!("Paste,{}", side_code(side))));
        attributes.push(("FilePolarity", "Positive".into()));
        if let Some(bytes) = paste(&board, side, &attributes)? {
            files.push(FabFile {
                name: format!("{stem}-paste-{}.gbr", side_word(side)),
                group: FileGroup::Gerber,
                description: format!("Solder paste {}", side_word(side)),
                bytes,
            });
        }
    }
    for side in [Layer::Top, Layer::Bottom] {
        let printed = match side {
            Layer::Top => project.stackup.silkscreen_top,
            Layer::Bottom => project.stackup.silkscreen_bottom,
        };
        if !printed {
            continue;
        }
        let mut attributes = common.clone();
        attributes.push(("FileFunction", format!("Legend,{}", side_code(side))));
        attributes.push(("FilePolarity", "Positive".into()));
        if let Some(bytes) = legend(&board, side, &attributes)? {
            files.push(FabFile {
                name: format!("{stem}-legend-{}.gbr", side_word(side)),
                group: FileGroup::Gerber,
                description: format!("Legend (silkscreen) {}", side_word(side)),
                bytes,
            });
        }
    }
    let mut attributes = common;
    attributes.push(("FileFunction", "Profile,NP".into()));
    attributes.push(("FilePolarity", "Positive".into()));
    files.push(FabFile {
        name: format!("{stem}-profile.gbr"),
        group: FileGroup::Gerber,
        description: "Board profile (outline, cutouts and slots)".into(),
        bytes: profile(project, &attributes)?,
    });
    Ok(files)
}

/// The X2 aperture function of a pad's copper.
fn pad_function(pad: &PlacedPad) -> &'static str {
    match pad.pad.kind {
        PadKind::PlatedThrough => "ComponentPad",
        PadKind::Fiducial => "FiducialPad,Local",
        PadKind::Smd | PadKind::Mechanical | PadKind::NonPlatedHole => {
            if pad.pad.hole.is_some() {
                "ComponentPad"
            } else {
                "SMDPad,CuDef"
            }
        }
    }
}

/// Whether a placed pad has copper on `layer`.
fn pad_on(pad: &PlacedPad, layer: CopperLayer) -> bool {
    match pad.pad.kind {
        PadKind::PlatedThrough => true,
        PadKind::NonPlatedHole => false,
        PadKind::Smd | PadKind::Fiducial | PadKind::Mechanical => {
            CopperLayer::from(pad.layer) == layer
        }
    }
}

fn copper(board: &Board<'_>, layer: CopperLayer, attributes: &[(&str, String)]) -> Result<Vec<u8>> {
    let project = board.project;
    let mut w = Writer::new(attributes);
    // Fill copper, largest region first (see the module notes).
    let mut regions: Vec<(f64, usize, Option<Id>, &crate::model::fill::FilledRegion)> = Vec::new();
    for fill in project.fills.iter().filter(|f| f.layer == layer) {
        for region in fill.computed.iter().flat_map(|c| c.regions.iter()) {
            let order = regions.len();
            regions.push((signed_area2(&region.outer).abs(), order, fill.net, region));
        }
    }
    regions.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, _, net, region) in &regions {
        w.object(&[("N", board.net(*net))]);
        w.polarity(true);
        polygon_region(&mut w, Some("Conductor"), &region.outer)?;
        if !region.holes.is_empty() {
            w.polarity(false);
            for hole in &region.holes {
                polygon_region(&mut w, None, hole)?;
            }
        }
    }
    w.polarity(true);
    for track in project.tracks.iter().filter(|t| t.layer == layer) {
        let d = w.aperture(Aperture::circle(track.width)?, Some("Conductor"));
        w.object(&[("N", board.net(Some(track.net)))]);
        w.stroke(d, track.start, &[Seg::Line(track.end)])?;
    }
    for via in &project.vias {
        let d = w.aperture(Aperture::circle(via.diameter)?, Some("ViaPad"));
        w.object(&[("N", board.net(Some(via.net)))]);
        w.flash(d, via.position)?;
    }
    for hole in project.board().holes.iter() {
        if let (true, Some(diameter)) = (hole.plated, hole.pad_diameter) {
            let d = w.aperture(Aperture::circle(diameter)?, Some("WasherPad"));
            w.object(&[]);
            w.flash(d, hole.at)?;
        }
    }
    for pad in board.pads.iter().filter(|p| pad_on(p, layer)) {
        let Some(shape) = pad.pad.shape.grown(0.0, CHECK_CHORD_MM) else {
            continue;
        };
        w.object(&board.pad_attributes(pad));
        let exact = match &pad.pad.shape {
            PadShape::Custom { outline } => Some(outline),
            _ => None,
        };
        land(
            &mut w,
            &shape,
            exact,
            &pad.pad_transform(),
            Some(pad_function(pad)),
        )?;
    }
    w.object(&[]);
    w.finish()
}

fn mask(board: &Board<'_>, side: Layer, attributes: &[(&str, String)]) -> Result<Vec<u8>> {
    let project = board.project;
    let mut w = Writer::new(attributes);
    let e = board.mask_expansion;
    for via in project.vias.iter().filter(|v| !v.tented) {
        let d = w.aperture(Aperture::circle(via.diameter + 2.0 * e)?, None);
        w.flash(d, via.position)?;
    }
    for hole in project.board().holes.iter() {
        let size = if hole.plated {
            hole.pad_diameter.unwrap_or(hole.drill)
        } else {
            hole.drill
        };
        let d = w.aperture(Aperture::circle(size + 2.0 * e)?, None);
        w.flash(d, hole.at)?;
    }
    for pad in &board.pads {
        let on = match pad.pad.kind {
            PadKind::PlatedThrough | PadKind::NonPlatedHole => true,
            _ => pad.layer == side,
        };
        if !on {
            continue;
        }
        let by = match pad.pad.mask {
            MaskRule::Inherit => e,
            MaskRule::Tented => continue,
            MaskRule::Expansion(x) => x,
        };
        let Some(shape) = pad.pad.shape.grown(by, CHECK_CHORD_MM) else {
            continue;
        };
        let exact = match &pad.pad.shape {
            PadShape::Custom { outline } if by == 0.0 => Some(outline),
            _ => None,
        };
        land(&mut w, &shape, exact, &pad.pad_transform(), None)?;
    }
    w.finish()
}

fn paste(board: &Board<'_>, side: Layer, attributes: &[(&str, String)]) -> Result<Option<Vec<u8>>> {
    let mut w = Writer::new(attributes);
    let mut any = false;
    for pad in board.pads.iter().filter(|p| p.layer == side) {
        let t = pad.pad_transform();
        let shrink = |by: f64| pad.pad.shape.grown(-by, CHECK_CHORD_MM);
        let shapes: Vec<Shape> = match &pad.pad.paste {
            PasteRule::Inherit => match pad.pad.kind {
                PadKind::Smd | PadKind::Mechanical if pad.pad.hole.is_none() => {
                    shrink(board.paste_reduction).into_iter().collect()
                }
                _ => Vec::new(),
            },
            PasteRule::None => Vec::new(),
            PasteRule::Reduction(r) => shrink(*r).into_iter().collect(),
            PasteRule::Windows(windows) => windows.iter().map(|w| w.shape.clone()).collect(),
        };
        for shape in &shapes {
            let exact = match (&pad.pad.shape, &pad.pad.paste) {
                (PadShape::Custom { outline }, PasteRule::Reduction(r)) if *r == 0.0 => {
                    Some(outline)
                }
                (PadShape::Custom { outline }, PasteRule::Inherit)
                    if board.paste_reduction == 0.0 =>
                {
                    Some(outline)
                }
                _ => None,
            };
            land(&mut w, shape, exact, &t, None)?;
            any = true;
        }
    }
    if !any {
        return Ok(None);
    }
    w.finish().map(Some)
}

fn legend(
    board: &Board<'_>,
    side: Layer,
    attributes: &[(&str, String)],
) -> Result<Option<Vec<u8>>> {
    let mut w = Writer::new(attributes);
    let mut any = false;
    for graphic in layout::placed_graphics(board.project) {
        if graphic.layer != BoardDrawing::Silkscreen(side) {
            continue;
        }
        w.object(&[("C", board.designator(graphic.instance))]);
        match &graphic.item {
            PlacedItem::Shape {
                shape,
                width,
                filled,
            } => {
                for (start, segs, closed) in shape_paths(shape) {
                    if *filled && closed {
                        w.region(None, start, &segs)?;
                        any = true;
                    }
                    if *width > 0.0 {
                        let d = w.aperture(Aperture::circle(*width)?, None);
                        w.stroke(d, start, &segs)?;
                        any = true;
                    }
                }
            }
            PlacedItem::Text(text) => {
                let strokes = layout::legend::text_strokes(text);
                if strokes.strokes.is_empty() {
                    continue;
                }
                let d = w.aperture(Aperture::circle(strokes.width)?, None);
                for stroke in &strokes.strokes {
                    let Some((first, rest)) = stroke.split_first() else {
                        continue;
                    };
                    if rest.is_empty() {
                        w.flash(d, *first)?;
                    } else {
                        let segs: Vec<Seg> = rest.iter().map(|p| Seg::Line(*p)).collect();
                        w.stroke(d, *first, &segs)?;
                    }
                    any = true;
                }
            }
        }
    }
    w.object(&[]);
    if !any {
        return Ok(None);
    }
    w.finish().map(Some)
}

fn profile(project: &Project, attributes: &[(&str, String)]) -> Result<Vec<u8>> {
    let board = project.board();
    let mut w = Writer::new(attributes);
    let d = w.aperture(Aperture::circle(PROFILE_PEN_MM)?, Some("Profile"));
    let (start, segs) = outline_path(&board.outer, &Transform2::default());
    w.stroke(d, start, &segs)?;
    for cutout in &board.cutouts {
        let routed = matches!(cutout.kind, CutoutKind::Slot { plated: true })
            && super::drill::stadium(&cutout.outline).is_some();
        if routed {
            continue;
        }
        let (start, segs) = outline_path(&cutout.outline, &Transform2::default());
        w.stroke(d, start, &segs)?;
    }
    w.finish()
}

/// An exact outline (lines and bulge arcs) through `t` as a path; a
/// reflection reverses every arc.
pub fn outline_path(outline: &Outline, t: &Transform2) -> (Point, Vec<Seg>) {
    let start = t.apply(outline.start);
    let mut from = start;
    let mut segs = Vec::with_capacity(outline.segments.len());
    for s in &outline.segments {
        let to = t.apply(s.to());
        match *s {
            Segment::Arc { bulge, .. } if bulge.abs() > 1e-12 => {
                let bulge = if t.mirror { -bulge } else { bulge };
                match arc_geometry(from, to, bulge) {
                    Some(arc) => segs.push(Seg::Arc {
                        to,
                        center: arc.center,
                        ccw: arc.sweep > 0.0,
                    }),
                    None => segs.push(Seg::Line(to)),
                }
            }
            _ => segs.push(Seg::Line(to)),
        }
        from = to;
    }
    (start, segs)
}

/// A flattened ring as a closed region.
fn polygon_region(w: &mut Writer, function: Option<&str>, ring: &[Point]) -> Result<()> {
    let Some((first, rest)) = ring.split_first() else {
        return Ok(());
    };
    if rest.len() < 2 {
        return Ok(());
    }
    let mut segs: Vec<Seg> = rest.iter().map(|p| Seg::Line(*p)).collect();
    segs.push(Seg::Line(*first));
    w.region(function, *first, &segs)
}

/// The closed or open paths a drawn shape strokes, with exact arcs:
/// `(start, segments, closed)`.
pub fn shape_paths(shape: &Shape) -> Vec<(Point, Vec<Seg>, bool)> {
    let arc_end = |center: Point, radius: f64, deg: f64| {
        let (c, s) = crate::geometry::cos_sin(deg);
        [center[0] + radius * c, center[1] + radius * s]
    };
    let from_primitives = |prims: &[Shape], closed: bool| -> Vec<(Point, Vec<Seg>, bool)> {
        let mut start = None;
        let mut segs = Vec::new();
        for p in prims {
            match *p {
                Shape::Line { a, b } => {
                    start.get_or_insert(a);
                    segs.push(Seg::Line(b));
                }
                Shape::Arc {
                    center,
                    radius,
                    start_deg,
                    sweep_deg,
                } => {
                    start.get_or_insert(arc_end(center, radius, start_deg));
                    segs.push(Seg::Arc {
                        to: arc_end(center, radius, start_deg + sweep_deg),
                        center,
                        ccw: sweep_deg > 0.0,
                    });
                }
                _ => {}
            }
        }
        start.map(|s| vec![(s, segs, closed)]).unwrap_or_default()
    };
    let corners = |points: &[Point], radii: &[f64], closed: bool| {
        if radii.iter().any(|r| *r > 0.0) {
            return from_primitives(&rounded_path(points, radii, closed), closed);
        }
        let Some((first, rest)) = points.split_first() else {
            return Vec::new();
        };
        let mut segs: Vec<Seg> = rest.iter().map(|p| Seg::Line(*p)).collect();
        if closed {
            segs.push(Seg::Line(*first));
        }
        vec![(*first, segs, closed)]
    };
    match shape {
        Shape::Line { a, b } => vec![(*a, vec![Seg::Line(*b)], false)],
        Shape::Polyline { points, radii } => corners(points, radii, false),
        Shape::Polygon { points, radii } => corners(points, radii, true),
        Shape::Rect { a, b, radius } => {
            let box_ = crate::geometry::Bounds::point(*a).including(*b);
            corners(&box_.corners(), &[*radius; 4], true)
        }
        Shape::Circle { center, radius } => {
            let start = [center[0] + radius, center[1]];
            vec![(
                start,
                vec![Seg::Arc {
                    to: start,
                    center: *center,
                    ccw: true,
                }],
                true,
            )]
        }
        Shape::Arc {
            center,
            radius,
            start_deg,
            sweep_deg,
        } => {
            let start = arc_end(*center, *radius, *start_deg);
            let full = sweep_deg.abs() >= 360.0 - 1e-9;
            let to = if full {
                start
            } else {
                arc_end(*center, *radius, start_deg + sweep_deg)
            };
            vec![(
                start,
                vec![Seg::Arc {
                    to,
                    center: *center,
                    ccw: *sweep_deg > 0.0,
                }],
                full,
            )]
        }
    }
}

/// Draw one land (pad-frame `shape`, centred on the pad origin) placed by
/// `t`: a standard or macro flash where the shape allows, else a region
/// (with `exact`'s arcs for an unchanged custom outline).
fn land(
    w: &mut Writer,
    shape: &Shape,
    exact: Option<&Outline>,
    t: &Transform2,
    function: Option<&str>,
) -> Result<()> {
    if let Some(outline) = exact {
        let (start, segs) = outline_path(outline, t);
        return w.region(function, start, &segs);
    }
    match shape {
        Shape::Circle { center, radius } => {
            let d = w.aperture(Aperture::circle(2.0 * radius)?, function);
            w.flash(d, t.apply(*center))
        }
        Shape::Polygon { points, radii } if box_size(points).is_some() => {
            let size = box_size(points).unwrap_or([0.0, 0.0]);
            let mut r = [0.0; 4];
            for (k, v) in radii.iter().take(4).enumerate() {
                r[k] = *v;
            }
            let aperture = box_aperture(size, r, t.rotation_deg, t.mirror)?;
            let d = w.aperture(aperture, function);
            w.flash(d, t.translate)
        }
        other => {
            for (start, segs, _) in shape_paths(&other.transformed(t)) {
                w.region(function, start, &segs)?;
            }
            Ok(())
        }
    }
}

/// The full size of a box centred on the origin with corners
/// counter-clockwise from the lower left (`[-x,-y], [x,-y], [x,y], [-x,y]`).
fn box_size(points: &[Point]) -> Option<Point> {
    let [a, b, c, d] = points else {
        return None;
    };
    let (x, y) = (c[0], c[1]);
    let close = |p: &Point, q: [f64; 2]| (p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9;
    (x > 0.0 && y > 0.0 && close(a, [-x, -y]) && close(b, [x, -y]) && close(d, [-x, y]))
        .then_some([2.0 * x, 2.0 * y])
}

/// A box `size` with corner radii `r` (lower left, lower right, upper
/// right, upper left), reflected across its x axis when `mirror`, then
/// turned `rotation_deg`, as one aperture.
fn box_aperture(size: Point, r: [f64; 4], rotation_deg: f64, mirror: bool) -> Result<Aperture> {
    let r = if mirror { [r[3], r[2], r[1], r[0]] } else { r };
    let [w, h] = size;
    let rot = rotation_deg.rem_euclid(360.0);
    let quarter = {
        let q = rot / 90.0;
        ((q - q.round()).abs() < 1e-9).then(|| (q.round() as i64).rem_euclid(4))
    };
    let tol = 1e-9;
    let half_short = w.min(h) / 2.0;
    if r.iter().all(|v| *v <= tol) {
        if let Some(q) = quarter {
            let (a, b) = if q % 2 == 1 { (h, w) } else { (w, h) };
            return Ok(Aperture::Rect(units(a)?, units(b)?));
        }
        return Ok(Aperture::Macro(macro_body(&[vec![
            21.0, 1.0, w, h, 0.0, 0.0, rot,
        ]])));
    }
    if r.iter().all(|v| (*v - half_short).abs() <= tol) {
        if (w - h).abs() <= tol {
            return Aperture::circle(w);
        }
        if let Some(q) = quarter {
            let (a, b) = if q % 2 == 1 { (h, w) } else { (w, h) };
            return Ok(Aperture::Obround(units(a)?, units(b)?));
        }
        let (long, short, turn) = if w >= h {
            (w, h, rot)
        } else {
            (h, w, rot + 90.0)
        };
        let turn = turn.rem_euclid(360.0);
        let c = (long - short) / 2.0;
        return Ok(Aperture::Macro(macro_body(&[
            vec![21.0, 1.0, long - short, short, 0.0, 0.0, turn],
            vec![1.0, 1.0, short, c, 0.0, turn],
            vec![1.0, 1.0, short, -c, 0.0, turn],
        ])));
    }
    // A rounded rectangle whose rounded corners share one radius: two
    // crossing bands, a circle at each rounded corner and a square at each
    // sharp one.
    let radius = r.iter().copied().fold(0.0, f64::max);
    let (x, y) = (w / 2.0, h / 2.0);
    let mut prims = Vec::new();
    if h - 2.0 * radius > tol {
        prims.push(vec![21.0, 1.0, w, h - 2.0 * radius, 0.0, 0.0, rot]);
    }
    if w - 2.0 * radius > tol {
        prims.push(vec![21.0, 1.0, w - 2.0 * radius, h, 0.0, 0.0, rot]);
    }
    let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    for (k, [sx, sy]) in corners.iter().enumerate() {
        if r[k] > tol {
            prims.push(vec![
                1.0,
                1.0,
                2.0 * radius,
                sx * (x - radius),
                sy * (y - radius),
                rot,
            ]);
        } else {
            prims.push(vec![
                21.0,
                1.0,
                radius,
                radius,
                sx * (x - radius / 2.0),
                sy * (y - radius / 2.0),
                rot,
            ]);
        }
    }
    Ok(Aperture::Macro(macro_body(&prims)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_become_standard_apertures_at_quarter_turns_and_macros_otherwise() -> Result<()> {
        let mm = |v: f64| units(v);
        assert_eq!(
            box_aperture([2.0, 1.0], [0.0; 4], 90.0, false)?,
            Aperture::Rect(mm(1.0)?, mm(2.0)?)
        );
        assert_eq!(
            box_aperture([2.0, 1.0], [0.5; 4], 180.0, true)?,
            Aperture::Obround(mm(2.0)?, mm(1.0)?)
        );
        assert_eq!(
            box_aperture([1.0, 1.0], [0.5; 4], 33.0, false)?,
            Aperture::Circle(mm(1.0)?)
        );
        assert_eq!(
            box_aperture([2.0, 1.0], [0.0; 4], 30.0, false)?,
            Aperture::Macro("21,1,2,1,0,0,30*\n".into())
        );
        // A rotated obround: the long axis turns with it.
        assert_eq!(
            box_aperture([1.0, 3.0], [0.5; 4], 10.0, false)?,
            Aperture::Macro("21,1,2,1,0,0,100*\n1,1,1,1,0,100*\n1,1,1,-1,0,100*\n".into())
        );
        // A D-shape (left corners rounded), reflected: still the left
        // corners, but upper and lower swap.
        let Aperture::Macro(body) = box_aperture([2.0, 1.0], [0.25, 0.0, 0.0, 0.25], 0.0, false)?
        else {
            return Err(crate::Error::Invalid("not a macro".into()));
        };
        assert_eq!(
            body,
            "21,1,2,0.5,0,0,0*\n21,1,1.5,1,0,0,0*\n1,1,0.5,-0.75,-0.25,0*\n21,1,0.25,0.25,0.875,-0.375,0*\n21,1,0.25,0.25,0.875,0.375,0*\n1,1,0.5,-0.75,0.25,0*\n"
        );
        Ok(())
    }
}
