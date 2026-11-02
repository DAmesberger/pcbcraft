//! Radio and processor modules: a green soldermasked board standing on its
//! gold (ENIG) pads, a nickel-silver shield can, the printed antenna's
//! meander under the mask at the +y end, and castellated half-holes where
//! a pad meets the outline (the board cut round them, gold plating in the
//! notch). Bodies stay disjoint: pads under the board, the can and the
//! antenna trace on top of it, plating inside the notches.
//!
//! One seed pad and a pattern per pad row (and per castellation row), a
//! seed and a two-way pattern for the ground array (turned with it), one
//! body per single pad: a few commands per row, not per pad
//! ([`crate::models`]: build with one command per kind of thing).
//!
//! Looks: [`MaterialId::SolderMaskGreen`] board, [`MaterialId::NickelSilver`]
//! can, [`MaterialId::Gold`] pads and plating,
//! [`MaterialId::SolderMaskOverCopper`] antenna (docs/materials.md).
use super::profile::bent_strip;
use super::{Build, check_positive, mm, require, top_edges};
use crate::Result;
use crate::native::{Expr, Pattern, ProfileBody};
use crate::pad_outline::Contour;
use pcbcraft_core::library::package_form::module::{ModuleForm, ModuleSide};
use pcbcraft_core::library::{BodyRole, Outline, Segment};
use pcbcraft_core::materials::MaterialId;

/// The builder revision (`PackageModel::Generated { family_rev }`).
pub(super) const FAMILY_REV: Option<u16> = Some(1);

/// Pad thickness under the board (est.: ENIG over 35 µm copper).
const PAD_MM: f64 = 0.035;
/// Castellation plating thickness in the notch (est.: plated-through
/// copper 20–25 µm plus finish).
const PLATING_MM: f64 = 0.03;
/// Antenna trace thickness on the board (est.: 35 µm copper).
const TRACE_MM: f64 = 0.035;
/// Antenna trace width (est.: printed 2.4 GHz meanders run 0.3–0.6 mm).
const TRACE_WIDTH_MM: f64 = 0.45;
/// Round of the can's top edges (est.: a drawn can's bend radius).
const CAN_EDGE_MM: f64 = 0.3;
/// Margin of the meander inside the antenna region (est.).
const ANTENNA_MARGIN_MM: f64 = 0.8;
/// Meander turns per 10 mm of module width (est.: printed 2.4 GHz
/// meanders run a few millimetre-pitch turns across the board end).
const ANTENNA_TURNS_PER_10_MM: f64 = 4.0;

pub(super) fn check(f: &ModuleForm) -> Result<()> {
    check_positive(&[
        ("module width", f.width),
        ("module length", f.length),
        ("board thickness", f.pcb_thickness),
        ("pitch", f.pitch),
    ])?;
    require(
        f.height > f.pcb_thickness + PAD_MM,
        "the height must leave room for the can above the board and its pads",
    )?;
    require(!f.row_centres().is_empty(), "a module needs pad rows")
}

/// The pattern axis of a side's row.
fn step3(side: ModuleSide) -> [f64; 3] {
    let [x, y] = side.step();
    [x, y, 0.0]
}

/// A closed polygon contour.
fn polygon(points: &[[f64; 2]]) -> Option<Contour> {
    let (first, rest) = points.split_first()?;
    let mut segments: Vec<Segment> = rest.iter().map(|p| Segment::Line { to: *p }).collect();
    segments.push(Segment::Line { to: *first });
    Some(Contour::Outline(Outline {
        start: *first,
        segments,
    }))
}

/// The board outline with a half-hole notch of `r` at every castellated
/// pad, counter-clockwise from the −x, −y corner.
fn board_outline(f: &ModuleForm, r: f64) -> Contour {
    let (hw, hl) = (f.width / 2.0, f.length / 2.0);
    let notches = |side: ModuleSide| -> Vec<f64> {
        let mut at: Vec<f64> = f
            .row_centres()
            .into_iter()
            .filter(|(s, c)| *s == side && r > 0.0 && f.at_edge(*s, *c))
            .map(|(_, c)| if side.across_x() { c[1] } else { c[0] })
            .collect();
        at.sort_by(f64::total_cmp);
        at
    };
    let mut segments = Vec::new();
    // Each edge runs from `a` to `b`; `point(t)` is the edge point at
    // coordinate t; a notch is a clockwise half circle (bulge −1) into
    // the board.
    let mut edge =
        |positions: Vec<f64>, ascending: bool, point: &dyn Fn(f64) -> [f64; 2], end: [f64; 2]| {
            let mut ts = positions;
            if !ascending {
                ts.reverse();
            }
            let dir = if ascending { 1.0 } else { -1.0 };
            for t in ts {
                segments.push(Segment::Line {
                    to: point(t - dir * r),
                });
                segments.push(Segment::Arc {
                    to: point(t + dir * r),
                    bulge: -1.0,
                });
            }
            segments.push(Segment::Line { to: end });
        };
    edge(notches(ModuleSide::Bottom), true, &|x| [x, -hl], [hw, -hl]);
    edge(notches(ModuleSide::Right), true, &|y| [hw, y], [hw, hl]);
    edge(notches(ModuleSide::Top), false, &|x| [x, hl], [-hw, hl]);
    edge(notches(ModuleSide::Left), false, &|y| [-hw, y], [-hw, -hl]);
    Contour::Outline(Outline {
        start: [-hw, -hl],
        segments,
    })
}

/// The plating in one castellation notch: a half ring from radius
/// `r − PLATING_MM` to `r` about the edge point `c`, opening outward.
fn plating(side: ModuleSide, c: [f64; 2], r: f64) -> Contour {
    let n = side.outward();
    // Tangent along the edge, counter-clockwise round the board.
    let t = [-n[1], n[0]];
    let at = |s: f64, radius: f64| [c[0] + t[0] * s * radius, c[1] + t[1] * s * radius];
    let ri = r - PLATING_MM;
    // The half circle inward of the edge: from −t to +t through −n
    // (clockwise along the tangent, so it bends into the board).
    Contour::Outline(Outline {
        start: at(-1.0, r),
        segments: vec![
            Segment::Arc {
                to: at(1.0, r),
                bulge: -1.0,
            },
            Segment::Line { to: at(1.0, ri) },
            Segment::Arc {
                to: at(-1.0, ri),
                bulge: 1.0,
            },
            Segment::Line { to: at(-1.0, r) },
        ],
    })
}

/// The meander's centreline across the antenna region: up and down turns
/// stepping along x, a feed leg down to the region's lower edge.
fn meander(f: &ModuleForm) -> Option<Vec<[f64; 2]>> {
    if f.antenna_length <= 2.0 * ANTENNA_MARGIN_MM + TRACE_WIDTH_MM {
        return None;
    }
    let (hw, hl) = (f.width / 2.0, f.length / 2.0);
    let y0 = hl - f.antenna_length + ANTENNA_MARGIN_MM;
    let y1 = hl - ANTENNA_MARGIN_MM;
    let x0 = -hw + ANTENNA_MARGIN_MM;
    let x1 = hw - ANTENNA_MARGIN_MM;
    let span = x1 - x0;
    let turns = (span / 10.0 * ANTENNA_TURNS_PER_10_MM)
        .floor()
        .clamp(1.0, 64.0);
    let step = span / (2.0 * turns);
    if step < 2.0 * TRACE_WIDTH_MM {
        return None;
    }
    let mut points = vec![[x0, y0], [x0, y1]];
    let mut x = x0;
    let mut up = true;
    // Every whole step: along the top or bottom, then across.
    let pairs = (2.0 * turns) as usize;
    for _ in 0..pairs {
        x += step;
        let y = if up { y1 } else { y0 + 0.4 * (y1 - y0) };
        let next = if up { y0 + 0.4 * (y1 - y0) } else { y1 };
        points.push([x, y]);
        points.push([x, next]);
        up = !up;
    }
    Some(points)
}

pub(super) fn build(b: &mut Build<'_>, f: &ModuleForm) -> Result<()> {
    let w = b.length("width", f.width)?;
    let l = b.length("length", f.length)?;
    let t = b.length("board_thickness", f.pcb_thickness)?;
    let pad = b.length("pad_thickness", PAD_MM)?;
    let can_h = b.length("can_height", f.height - f.pcb_thickness - PAD_MM)?;
    let [cw, cl] = f.can_size;
    let cw_p = b.length("can_width", cw)?;
    let cl_p = b.length("can_length", cl)?;

    // The board, notched at castellations.
    let board = b.name("board");
    let body_material = b.material(BodyRole::Body);
    let r = f.castellation / 2.0;
    let castellated = r > 0.0 && f.row_centres().iter().any(|(s, c)| f.at_edge(*s, *c));
    if castellated {
        b.native.profile_body(ProfileBody {
            name: &board,
            offset: Expr::from(pad.clone()),
            contours: &[board_outline(f, r)],
            height: Expr::from(t.clone()),
        })?;
        b.push(board.clone(), BodyRole::Body, body_material);
    } else {
        b.solid(
            &board,
            BodyRole::Body,
            body_material,
            [format!("- {w} / 2"), format!("- {l} / 2"), pad.clone()],
            [w.clone(), l.clone(), t.clone()],
        )?;
    }

    // Pads: a seed and a pattern per row, numbered in row order.
    let lead_material = b.material(BodyRole::Termination);
    let mut numbered: Vec<(String, u32)> = Vec::new();
    let mut lead = 0u32;
    let centres = f.row_centres();
    let mut offset = 0usize;
    for (k, row) in f.rows.iter().enumerate() {
        let size = f.row_pad_size(row.side);
        let first = row.first;
        let name = b.name(&format!("row {} pad", k + 1));
        b.solid(
            &name,
            BodyRole::Termination,
            lead_material,
            [
                mm(first[0] - size[0] / 2.0),
                mm(first[1] - size[1] / 2.0),
                "0 mm".into(),
            ],
            [mm(size[0]), mm(size[1]), pad.clone()],
        )?;
        let all = if row.count > 1 {
            let all = b.native.pattern_bodies(
                &b.name(&format!("row {} pattern", k + 1)),
                &[&name],
                Pattern {
                    dir1: step3(row.side),
                    count1: Expr::from(i32::try_from(row.count).unwrap_or(1)),
                    spacing1: Expr::from(mm(f.pitch)),
                    second: None,
                },
            )?;
            b.push_copies(&all, BodyRole::Termination, lead_material);
            all
        } else {
            vec![name]
        };
        let (axis, ascending) = match row.side {
            ModuleSide::Left => (1, false),
            ModuleSide::Bottom => (0, true),
            ModuleSide::Right => (1, true),
            ModuleSide::Top => (0, false),
        };
        let ordered = b.order(&[(all, axis, ascending)])?;
        for name in ordered {
            lead += 1;
            numbered.push((name, lead));
        }
        // Castellation plating in this row's notches, if every pad of the
        // row meets the edge (a seed and the same pattern).
        let row_centres = centres
            .get(offset..offset + row.count as usize)
            .unwrap_or(&[]);
        offset += row.count as usize;
        if castellated && row_centres.iter().all(|(s, c)| f.at_edge(*s, *c)) {
            let n = row.side.outward();
            let (hw, hl) = (f.width / 2.0, f.length / 2.0);
            let edge = |c: [f64; 2]| match row.side {
                ModuleSide::Left | ModuleSide::Right => [n[0] * hw, c[1]],
                ModuleSide::Bottom | ModuleSide::Top => [c[0], n[1] * hl],
            };
            let plate = b.name(&format!("row {} castellation", k + 1));
            b.native.profile_body(ProfileBody {
                name: &plate,
                offset: Expr::from(pad.clone()),
                contours: &[plating(row.side, edge(first), r)],
                height: Expr::from(t.clone()),
            })?;
            b.push(plate.clone(), BodyRole::Termination, lead_material);
            let plates = if row.count > 1 {
                let all = b.native.pattern_bodies(
                    &b.name(&format!("row {} castellation pattern", k + 1)),
                    &[&plate],
                    Pattern {
                        dir1: step3(row.side),
                        count1: Expr::from(i32::try_from(row.count).unwrap_or(1)),
                        spacing1: Expr::from(mm(f.pitch)),
                        second: None,
                    },
                )?;
                b.push_copies(&all, BodyRole::Termination, lead_material);
                all
            } else {
                vec![plate]
            };
            let ordered = b.order(&[(plates, axis, ascending)])?;
            for (j, name) in ordered.into_iter().enumerate() {
                let first_lead = lead + 1 - row.count;
                numbered.push((name, first_lead + u32::try_from(j).unwrap_or(0)));
            }
        }
    }

    // The ground array: one terminal for every pad.
    if let Some(g) = &f.ground {
        lead += 1;
        let pad_material = b.material(BodyRole::ExposedPad);
        let centres = f.ground_centres();
        if let Some(first) = centres.first() {
            let (c, s) = pcbcraft_core::geometry::cos_sin(g.rotation_deg);
            let [hx, hy] = [g.size[0] / 2.0, g.size[1] / 2.0];
            let corner = |u: f64, v: f64| [first[0] + c * u - s * v, first[1] + s * u + c * v];
            let seed = b.name("ground pad");
            if let Some(square) = polygon(&[
                corner(-hx, -hy),
                corner(hx, -hy),
                corner(hx, hy),
                corner(-hx, hy),
            ]) {
                b.native.profile_body(ProfileBody {
                    name: &seed,
                    offset: Expr::from("0 mm"),
                    contours: &[square],
                    height: Expr::from(pad.clone()),
                })?;
            }
            b.push(seed.clone(), BodyRole::ExposedPad, pad_material);
            let all = if g.rows * g.cols > 1 {
                let all = b.native.pattern_bodies(
                    &b.name("ground pattern"),
                    &[&seed],
                    Pattern {
                        dir1: [c, s, 0.0],
                        count1: Expr::from(i32::try_from(g.cols).unwrap_or(1)),
                        spacing1: Expr::from(mm(g.pitch[0])),
                        second: (g.rows > 1).then(|| {
                            (
                                [s, -c, 0.0],
                                Expr::from(i32::try_from(g.rows).unwrap_or(1)),
                                Expr::from(mm(g.pitch[1])),
                            )
                        }),
                    },
                )?;
                b.push_copies(&all, BodyRole::ExposedPad, pad_material);
                all
            } else {
                vec![seed]
            };
            numbered.extend(all.into_iter().map(|n| (n, lead)));
        }
    }

    // Single pads, each its own terminal.
    for (k, p) in f.singles.iter().enumerate() {
        lead += 1;
        let name = b.name(&format!("pad {}", p.number.trim()));
        let name = if b.bodies.iter().any(|x| x.name == name) {
            b.name(&format!("single pad {}", k + 1))
        } else {
            name
        };
        b.solid(
            &name,
            BodyRole::Termination,
            lead_material,
            [
                mm(p.centre[0] - p.size[0] / 2.0),
                mm(p.centre[1] - p.size[1] / 2.0),
                "0 mm".into(),
            ],
            [mm(p.size[0]), mm(p.size[1]), pad.clone()],
        )?;
        numbered.push((name, lead));
    }
    for (name, k) in numbered {
        if let Some(body) = b.bodies.iter_mut().find(|x| x.name == name) {
            body.terminal = Some(k);
        }
    }

    // The shield can on the board, its top edges rounded.
    let can = b.name("shield can");
    let can_material = b.material(BodyRole::Other);
    let [cx, cy] = f.can_centre;
    let z_top = PAD_MM + f.pcb_thickness;
    b.solid(
        &can,
        BodyRole::Other,
        can_material,
        [
            format!("{} - {cw_p} / 2", mm(cx)),
            format!("{} - {cl_p} / 2", mm(cy)),
            format!("{pad} + {t}"),
        ],
        [cw_p.clone(), cl_p.clone(), can_h.clone()],
    )?;
    let lo = [cx - cw / 2.0, cy - cl / 2.0, z_top];
    let hi = [cx + cw / 2.0, cy + cl / 2.0, f.height];
    let edge = CAN_EDGE_MM
        .min(0.25 * (f.height - z_top))
        .min(0.1 * cw.min(cl));
    b.round("can_round", &can, &top_edges(lo, hi), edge)?;

    // The printed antenna under the mask.
    if let Some(points) = meander(f)
        && let Ok(strip) = bent_strip(&points, TRACE_WIDTH_MM, 0.2)
    {
        let trace = b.name("antenna");
        b.native.profile_body(ProfileBody {
            name: &trace,
            offset: Expr::from(format!("{pad} + {t}")),
            contours: &[strip],
            height: Expr::from(mm(TRACE_MM)),
        })?;
        b.push(trace, BodyRole::Other, MaterialId::SolderMaskOverCopper);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_outline_closes_with_a_notch_per_castellated_pad() -> Result<()> {
        let f = ModuleForm::typical();
        let Contour::Outline(o) = board_outline(&f, 0.3) else {
            return Err(crate::Error::Verification("not an outline".into()));
        };
        let arcs = o
            .segments
            .iter()
            .filter(|s| matches!(s, Segment::Arc { .. }))
            .count();
        assert_eq!(arcs, 38);
        assert_eq!(o.segments.last().map(Segment::to), Some(o.start));
        Ok(())
    }

    #[test]
    fn the_meander_stays_inside_the_antenna_region() {
        let f = ModuleForm::typical();
        let points = meander(&f).unwrap_or_default();
        assert!(points.len() > 4);
        for [x, y] in points {
            assert!(x.abs() <= 9.0 - ANTENNA_MARGIN_MM + 1e-9);
            assert!(y >= 12.5 - 6.0 && y <= 12.5);
        }
    }

    use super::super::{
        GeneratedBody, ModelDefinition, PackageDefinition, PackageForm, PackageModel,
        build_definition, family_rev,
    };
    use crate::native::Native;
    use pcbcraft_core::PackageTerminalId;
    use pcbcraft_core::library::package_form::module::{GroundPads, PadRow, SinglePad};
    use pcbcraft_core::library::{GeneratedMaterials, PackageTerminal};

    /// An original synthetic LGA module: 16 × 20 mm, 3 mm high, pads on four
    /// sides at 1.0 mm, a 2 × 2 ground array turned 45°, two corner pads.
    pub(crate) fn lga() -> ModuleForm {
        ModuleForm {
            width: 16.0,
            length: 20.0,
            height: 3.0,
            pcb_thickness: 0.8,
            antenna_length: 5.0,
            can_size: [15.0, 14.0],
            can_centre: [0.0, -2.5],
            pitch: 1.0,
            pad_size: [0.8, 0.5],
            rows: vec![
                PadRow {
                    side: ModuleSide::Left,
                    count: 10,
                    first: [-7.4, 3.5],
                },
                PadRow {
                    side: ModuleSide::Bottom,
                    count: 8,
                    first: [-3.5, -9.4],
                },
                PadRow {
                    side: ModuleSide::Right,
                    count: 10,
                    first: [7.4, -5.5],
                },
                PadRow {
                    side: ModuleSide::Top,
                    count: 8,
                    first: [3.5, 4.4],
                },
            ],
            castellation: 0.0,
            land_width: 0.5,
            land_extend: [0.0, 0.0],
            ground: Some(GroundPads {
                rows: 2,
                cols: 2,
                pitch: [2.0, 2.0],
                size: [1.2, 1.2],
                centre: [0.0, -2.5],
                rotation_deg: 45.0,
                number: "37".into(),
            }),
            singles: vec![
                SinglePad {
                    number: "38".into(),
                    centre: [-7.4, -9.4],
                    size: [0.8, 0.8],
                },
                SinglePad {
                    number: "39".into(),
                    centre: [7.4, -9.4],
                    size: [0.8, 0.8],
                },
            ],
            keepout: Some(crate::models::module::tests::keepout()),
        }
    }

    fn keepout() -> pcbcraft_core::library::package_form::module::AntennaKeepout {
        pcbcraft_core::library::package_form::module::AntennaKeepout {
            beyond: [0.0, 0.0, 0.0],
        }
    }

    fn package(form: ModuleForm) -> PackageDefinition {
        let names = form.designations();
        let form = PackageForm::Module(Box::new(form));
        PackageDefinition {
            next_local: u32::try_from(names.len()).unwrap_or(0) + 1,
            terminals: names
                .iter()
                .zip(1..)
                .map(|(n, k)| PackageTerminal {
                    id: PackageTerminalId(k),
                    designation: n.clone(),
                })
                .collect(),
            model: PackageModel::Generated {
                family_rev: family_rev(&form).unwrap_or(0),
                materials: GeneratedMaterials::default(),
            },
            form,
            ..PackageDefinition::default()
        }
    }

    fn built(form: ModuleForm) -> Result<(Native, ModelDefinition)> {
        let mut native = Native::new("models")?;
        let definition = build_definition(&mut native, "pkgm", &package(form))?;
        Ok((native, definition))
    }

    fn bounds(native: &Native, name: &str) -> Result<([f64; 3], [f64; 3])> {
        let state = native.session().model.state();
        let body = state
            .body(name)
            .ok_or_else(|| crate::Error::Verification(format!("no body {name}")))?;
        let m = body
            .measure()
            .map_err(|e| crate::Error::Verification(e.to_string()))?;
        Ok((
            [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
        ))
    }

    fn terminal_bodies(def: &ModelDefinition, k: u32) -> Vec<&GeneratedBody> {
        def.bodies
            .iter()
            .filter(|b| b.terminal == Some(k))
            .collect()
    }

    #[test]
    fn an_lga_module_has_its_pads_numbered_on_the_lead_points() -> Result<()> {
        let form = lga();
        let (native, def) = built(form.clone())?;
        let leads = form.row_centres();
        assert_eq!(leads.len(), 36);
        for (k, (_, c)) in leads.iter().enumerate() {
            let k = u32::try_from(k + 1).unwrap_or(0);
            let bodies = terminal_bodies(&def, k);
            assert_eq!(bodies.len(), 1, "terminal {k}: {bodies:?}");
            let (lo, hi) = bounds(&native, &bodies[0].name)?;
            assert!(((lo[0] + hi[0]) / 2.0 - c[0]).abs() < 1e-6, "pad {k} x");
            assert!(((lo[1] + hi[1]) / 2.0 - c[1]).abs() < 1e-6, "pad {k} y");
            assert!(lo[2].abs() < 1e-9 && (hi[2] - PAD_MM).abs() < 1e-9);
            assert_eq!(bodies[0].material, MaterialId::Gold);
        }
        // Terminal 37: the four turned ground pads; 38 and 39 the corners.
        assert_eq!(terminal_bodies(&def, 37).len(), 4);
        assert_eq!(terminal_bodies(&def, 38).len(), 1);
        assert_eq!(terminal_bodies(&def, 39).len(), 1);
        // The can tops the module at its seated height, nickel silver.
        let can = def
            .bodies
            .iter()
            .find(|b| b.material == MaterialId::NickelSilver)
            .ok_or_else(|| crate::Error::Verification("no can".into()))?;
        let (lo, hi) = bounds(&native, &can.name)?;
        assert!((hi[2] - 3.0).abs() < 1e-6 && (lo[2] - (PAD_MM + 0.8)).abs() < 1e-6);
        assert!(
            (hi[1] - 4.5).abs() < 1e-6,
            "clear of the antenna end: {hi:?}"
        );
        let board = def
            .bodies
            .iter()
            .find(|b| b.material == MaterialId::SolderMaskGreen)
            .ok_or_else(|| crate::Error::Verification("no board".into()))?;
        let (lo, hi) = bounds(&native, &board.name)?;
        assert!((hi[0] - lo[0] - 16.0).abs() < 1e-6 && (hi[1] - lo[1] - 20.0).abs() < 1e-6);
        assert!(
            def.bodies
                .iter()
                .any(|b| b.material == MaterialId::SolderMaskOverCopper),
            "the antenna meander"
        );
        Ok(())
    }

    #[test]
    fn a_castellated_module_plates_its_notches() -> Result<()> {
        let (native, def) = built(ModuleForm::typical())?;
        // Pin 1 has its land and its plated notch, both terminal 1.
        let one = terminal_bodies(&def, 1);
        assert_eq!(one.len(), 2, "{one:?}");
        let plate = one
            .iter()
            .find(|b| b.name.contains("castellation"))
            .ok_or_else(|| crate::Error::Verification("no plating".into()))?;
        let (lo, hi) = bounds(&native, &plate.name)?;
        // In the −x edge's notch, through the board's thickness.
        assert!(
            (lo[0] - (-9.0)).abs() < 1e-6 && hi[0] < -9.0 + 0.31,
            "{lo:?} {hi:?}"
        );
        assert!((hi[2] - lo[2] - 0.8).abs() < 1e-6);
        assert_eq!(terminal_bodies(&def, 39).len(), 9, "the ground array");
        Ok(())
    }
}
