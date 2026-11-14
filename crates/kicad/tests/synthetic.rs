//! The import on Rust-built synthetic boards, in CI (no KiCad needed).
//! The expected values follow the semantics `tests/oracle.rs` confirmed
//! against `kicad-cli` plots: a pad's `at` is in its footprint's frame and
//! its angle is its board orientation; back-side footprints are stored as
//! seen from the top; a drill offset moves the copper, not the hole; mask
//! and paste margins resolve pad, then footprint, then board (a stored 0
//! is unset); a relative paste margin scales the pad size.
mod support;

use sparksmith_core::geometry::{Transform2, polygon_contains};
use sparksmith_core::layout::{PlacedPad, placed_pads};
use sparksmith_core::library::{HoleShape, MaskRule, PadKind, PadShape, PasteRule, ShapeNm};
use sparksmith_core::{Layer, Project};
use sparksmith_kicad::ImportError;
use sparksmith_kicad::board::BoardError;
use sparksmith_kicad::report::Case;
use support::{Board, Footprint, Pad, import};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// KiCad's board point of a footprint-local point `p` (y down) of a
/// footprint at `fp` (x, y, angle), in our frame about `origin` (y up).
fn kicad_world(origin: [f64; 2], fp: [f64; 3], p: [f64; 2]) -> [f64; 2] {
    let (s, c) = fp[2].to_radians().sin_cos();
    // A counter-clockwise turn on screen in a y-down frame.
    let x = fp[0] + p[0] * c + p[1] * s;
    let y = fp[1] - p[0] * s + p[1] * c;
    [x - origin[0], -(y - origin[1])]
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6
}

/// The placed pad of footprint `k` (board order) with `number`.
fn pad<'a>(
    project: &Project,
    pads: &'a [PlacedPad],
    k: usize,
    number: &str,
) -> Option<&'a PlacedPad> {
    let instance = project.placements.get(k)?.instance;
    pads.iter()
        .find(|p| p.instance == instance && p.pad.number == number)
}

#[test]
fn the_showcase_board_imports_through_the_engine() -> Fallible<()> {
    let board = support::showcase();
    let imported = import(board.text().as_bytes())?;
    let project = imported.engine.project();
    let counts = &imported.report.counts;
    assert_eq!((counts.footprints, counts.footprints_back), (11, 5));
    // Every angle of one footprint shares a definition, on either side.
    assert_eq!(counts.definitions, 3);
    assert_eq!(counts.pads, counts.pads_imported);
    assert_eq!(project.placements.len(), 11);
    assert_eq!(project.stackup.layers().len(), 4);
    let origin = board.origin.ok_or("origin")?;
    for (placement, fp) in project.placements.iter().zip(&board.footprints) {
        let at = kicad_world(origin, fp.at, [0.0, 0.0]);
        assert!(
            near(placement.position, at),
            "{:?} {at:?}",
            placement.position
        );
        assert_eq!(placement.layer == Layer::Bottom, fp.back);
    }
    // The import is undoable command history, not a written document.
    assert!(imported.engine.can_undo());
    Ok(())
}

#[test]
fn pads_land_where_kicad_draws_them_on_both_sides_and_every_angle() -> Fallible<()> {
    let board = support::showcase();
    let imported = import(board.text().as_bytes())?;
    let project = imported.engine.project();
    let pads = placed_pads(project);
    let origin = board.origin.ok_or("origin")?;
    for (k, fp) in board.footprints.iter().enumerate().take(10) {
        for kpad in &fp.pads {
            if kpad.number.is_empty() {
                continue;
            }
            let ours = pad(project, &pads, k, kpad.number).ok_or("pad")?;
            let t = ours.pad_transform();
            // The hole (or the pad origin without one) is at `at`.
            let hole = ours.pad.hole.map_or([0.0, 0.0], |h| h.offset());
            let expect = kicad_world(origin, fp.at, [kpad.at[0], kpad.at[1]]);
            assert!(
                near(t.apply(hole), expect),
                "pad {} of footprint {k}: {:?} against {expect:?}",
                kpad.number,
                t.apply(hole)
            );
            // Copper on the footprint's side unless placed on the other.
            let side = if fp.back { Layer::Bottom } else { Layer::Top };
            if ours.pad.kind == PadKind::Smd {
                assert_eq!(ours.layer, side);
            }
        }
    }
    Ok(())
}

#[test]
fn back_side_custom_pads_keep_their_board_shape() -> Fallible<()> {
    let board = support::showcase();
    let imported = import(board.text().as_bytes())?;
    let project = imported.engine.project();
    let pads = placed_pads(project);
    let origin = board.origin.ok_or("origin")?;
    // Footprint 9 is the back side at 30°; pad 5 is the custom pad at
    // (-2.5, 0.5) with a polygon vertex at (-0.3, 0.6) of its own frame.
    let fp = &board.footprints[9];
    assert!(fp.back && fp.at[2] == 30.0);
    let ours = pad(project, &pads, 9, "5").ok_or("pad 5")?;
    let PadShape::Custom { outline } = &ours.pad.shape else {
        return Err("pad 5 is not custom".into());
    };
    let t = ours.pad_transform();
    let ring: Vec<[f64; 2]> = std::iter::once(outline.start)
        .chain(outline.segments.iter().map(|s| s.to()))
        .map(|p| t.apply(p))
        .collect();
    // KiCad draws pad-local points turned by the pad's board angle, with
    // no reflection: the stored data is already as seen from the top.
    let local = |q: [f64; 2]| {
        let (s, c) = fp.at[2].to_radians().sin_cos();
        [-2.5 + q[0] * c + q[1] * s, 0.5 - q[0] * s + q[1] * c]
    };
    let inside = kicad_world(origin, fp.at, local([-0.3, 0.6]));
    let mirrored = kicad_world(origin, fp.at, local([-0.3, -0.6]));
    let shrink = |p: [f64; 2], toward: [f64; 2]| {
        [
            p[0] + (toward[0] - p[0]) * 1e-3,
            p[1] + (toward[1] - p[1]) * 1e-3,
        ]
    };
    let center = t.apply([0.0, 0.0]);
    assert!(polygon_contains(&ring, shrink(inside, center)));
    assert!(!polygon_contains(&ring, mirrored));
    Ok(())
}

#[test]
fn a_drill_offset_moves_the_copper_not_the_hole() -> Fallible<()> {
    let board = support::showcase();
    let imported = import(board.text().as_bytes())?;
    let project = imported.engine.project();
    let pads = placed_pads(project);
    let origin = board.origin.ok_or("origin")?;
    for k in [0, 2, 8] {
        let fp = &board.footprints[k];
        let ours = pad(project, &pads, k, "10").ok_or("pad 10")?;
        let hole = ours.pad.hole.ok_or("no hole")?;
        assert!(
            matches!(hole.shape, HoleShape::Slot { width_nm, length_nm, .. }
                if width_nm.get() == 600_000 && length_nm.get() == 1_200_000)
        );
        let t = ours.pad_transform();
        // The hole at `at` (1.5, 2.5); the copper 0.2 along the pad's x.
        assert!(near(
            t.apply(hole.offset()),
            kicad_world(origin, fp.at, [1.5, 2.5])
        ));
        assert!(near(
            t.apply([0.0, 0.0]),
            kicad_world(origin, fp.at, [1.7, 2.5])
        ));
    }
    Ok(())
}

#[test]
fn margins_resolve_pad_then_footprint_then_board() -> Fallible<()> {
    let board = support::showcase();
    let imported = import(board.text().as_bytes())?;
    let project = imported.engine.project();
    let pads = placed_pads(project);
    let get = |k: usize, n: &str| pad(project, &pads, k, n).map(|p| p.pad.clone());
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let expansion = |p: &sparksmith_core::library::Pad| match p.mask {
        MaskRule::Expansion(e) => e.to_mm_f64(),
        _ => f64::NAN,
    };
    let reduction = |p: &sparksmith_core::library::Pad| match p.paste {
        PasteRule::Reduction(r) => r.to_mm_f64(),
        _ => f64::NAN,
    };
    // The board's 0.05 mask and −0.02 paste margins.
    let p1 = get(0, "1").ok_or("pad 1")?;
    assert!(close(expansion(&p1), 0.05) && close(reduction(&p1), 0.02));
    // The pad's own 0.1 and −0.05.
    let p7 = get(0, "7").ok_or("pad 7")?;
    assert!(close(expansion(&p7), 0.1) && close(reduction(&p7), 0.05));
    // The footprint's 0.07; a stored 0 on the pad is unset.
    let u99 = |n: &str| get(10, n).ok_or("U99 pad");
    assert!(close(expansion(&u99("1")?), 0.07));
    assert!(close(expansion(&u99("5")?), 0.07));
    // A relative margin of −0.1 of a 1 mm pad, with the board's −0.02.
    assert!(close(reduction(&u99("3")?), 0.12));
    // A plated pad on every side, mask open, no paste.
    let p9 = get(0, "9").ok_or("pad 9")?;
    assert_eq!(p9.kind, PadKind::PlatedThrough);
    assert_eq!(p9.paste, PasteRule::None);
    // The non-square round rect with a relative margin: KiCad takes the
    // margin per axis (−0.03 − 0.1 · 1.2 and −0.03 − 0.1 · 0.6) and rebuilds
    // the radius from the ratio on the new size, a paste window here.
    assert_eq!(imported.report.count(Case::PasteRatioWindow), 1);
    assert_eq!(imported.report.count(Case::PasteRatio), 0);
    let p4 = u99("4")?;
    let PasteRule::Windows(windows) = &p4.paste else {
        return Err("pad 4 has no paste window".into());
    };
    let [window] = &windows[..] else {
        return Err("one window expected".into());
    };
    let ShapeNm::Rect { a, b, radius_nm } = &window.shape else {
        return Err("a rectangular window expected".into());
    };
    let (w, h) = (
        (b.x_nm.get() - a.x_nm.get()) as f64 / 1e6,
        (b.y_nm.get() - a.y_nm.get()) as f64 / 1e6,
    );
    assert!(close(w, 0.9) && close(h, 0.42), "{w} × {h}");
    assert!(close(radius_nm.to_mm_f64(), 0.105));
    // A round rect's radius is a ratio of its short side.
    let p2 = get(0, "2").ok_or("pad 2")?;
    assert!(matches!(p2.shape, PadShape::RoundRect { radius_ratio, .. } if radius_ratio == 0.25));
    Ok(())
}

fn one_footprint(pads: Vec<Pad>) -> Board {
    Board {
        copper: 2,
        origin: None,
        setup: "(pad_to_mask_clearance 0)".into(),
        footprints: vec![Footprint::new("U1", [10.0, 10.0, 0.0], false, pads)],
        items: String::new(),
    }
}

#[test]
fn paste_only_pads_become_windows_of_the_pad_under_them() -> Fallible<()> {
    let mut pads = vec![
        Pad::smd("9", "rect", [0.0, 0.0, 0.0], [3.0, 3.0]).layers("\"F.Cu\" \"F.Mask\""),
        Pad::smd("1", "rect", [-2.5, 0.0, 0.0], [0.6, 0.3]),
    ];
    for (x, y) in [(-0.7, -0.7), (0.7, -0.7), (-0.7, 0.7), (0.7, 0.7)] {
        pads.push(Pad::smd("", "rect", [x, y, 0.0], [1.0, 1.0]).layers("\"F.Paste\""));
    }
    let imported = import(one_footprint(pads).text().as_bytes())?;
    let placed = placed_pads(imported.engine.project());
    let ep = placed.iter().find(|p| p.pad.number == "9").ok_or("EP")?;
    let PasteRule::Windows(windows) = &ep.pad.paste else {
        return Err("no windows".into());
    };
    assert_eq!(windows.len(), 4);
    assert_eq!(imported.report.count(Case::PasteWindow), 4);
    // Nothing else became a pad.
    assert_eq!(placed.len(), 2);
    Ok(())
}

#[test]
fn a_fiducial_is_a_part_without_terminals() -> Fallible<()> {
    let fiducial = Pad::smd("", "circle", [0.0, 0.0, 0.0], [1.0, 1.0])
        .layers("\"F.Cu\" \"F.Mask\"")
        .with("(property pad_prop_fiducial_loc) (solder_mask_margin 0.5)");
    let imported = import(one_footprint(vec![fiducial]).text().as_bytes())?;
    let project = imported.engine.project();
    let placed = placed_pads(project);
    assert_eq!(placed.len(), 1);
    // No made-up terminal: the pad stays a fiducial and lands none.
    assert_eq!(placed[0].pad.kind, PadKind::Fiducial);
    assert_eq!(placed[0].terminal, None);
    let instance = project.instance_by_designator("U1").ok_or("U1")?;
    let component = project
        .library
        .component(instance.component)
        .ok_or("component")?;
    assert!(component.definition.terminals.is_empty());
    assert_eq!(imported.report.count(Case::NoTerminals), 1);
    // On the sheet like any part KiCad has in its schematic.
    assert!(!instance.board_only);
    assert_eq!(project.occurrences_of(instance.id).count(), 1);
    Ok(())
}

#[test]
fn a_part_not_in_the_schematic_is_board_only() -> Fallible<()> {
    let hole = Pad::npth([0.0, 0.0, 0.0], 3.2);
    let mut board = one_footprint(vec![hole]);
    board.footprints[0].extra = " (attr board_only exclude_from_bom)".into();
    let imported = import(board.text().as_bytes())?;
    let project = imported.engine.project();
    let instance = project.instance_by_designator("U1").ok_or("U1")?;
    assert!(instance.board_only);
    assert_eq!(project.occurrences_of(instance.id).count(), 0);
    assert!(project.placement_of(instance.id).is_some());
    assert_eq!(imported.report.count(Case::BoardOnly), 1);
    Ok(())
}

#[test]
fn unsupported_versions_tokens_and_malformed_files_are_refused() -> Fallible<()> {
    let text = support::showcase().text();
    let newer = text.replacen("(version 20240108)", "(version 20250114)", 1);
    assert!(matches!(
        import(newer.as_bytes()),
        Err(ImportError::Board(BoardError::Version(20250114)))
    ));
    let older = text.replacen("(version 20240108)", "(version 20211014)", 1);
    assert!(matches!(
        import(older.as_bytes()),
        Err(ImportError::Board(BoardError::Version(_)))
    ));
    assert!(matches!(
        import(&text.as_bytes()[..text.len() - 3]),
        Err(ImportError::Board(BoardError::Syntax(_)))
    ));
    assert!(matches!(
        import(b"(kicad_sch (version 20231120))"),
        Err(ImportError::Board(BoardError::NotABoard))
    ));
    // A pad with a token we do not know is refused, not guessed.
    let pads = vec![
        Pad::smd("1", "rect", [0.0, 0.0, 0.0], [1.0, 1.0]),
        Pad::smd("2", "rect", [2.0, 0.0, 0.0], [1.0, 1.0]).with("(future_shape_token 1)"),
    ];
    let imported = import(one_footprint(pads).text().as_bytes())?;
    assert_eq!(imported.report.count(Case::PadUnknownToken), 1);
    assert_eq!(placed_pads(imported.engine.project()).len(), 1);
    Ok(())
}

#[test]
fn a_definition_frame_is_independent_of_placement() -> Fallible<()> {
    // The same footprint at 0° and 90° gives one definition whose pads sit
    // where an unturned footprint has them.
    let pads = |a: f64| {
        vec![
            Pad::smd("1", "rect", [1.0, 0.5, a], [0.8, 0.4]),
            Pad::smd("2", "rect", [-1.0, 0.5, a], [0.8, 0.4]),
        ]
    };
    let board = Board {
        copper: 2,
        origin: None,
        setup: String::new(),
        footprints: vec![
            Footprint::new("R1", [10.0, 10.0, 0.0], false, pads(0.0)),
            Footprint::new("R2", [20.0, 10.0, 90.0], false, pads(90.0)),
        ],
        items: String::new(),
    };
    let imported = import(board.text().as_bytes())?;
    assert_eq!(imported.report.counts.definitions, 1);
    let placed = placed_pads(imported.engine.project());
    let def = &placed[0].pad;
    // KiCad's (1, 0.5) with y down is (1, −0.5) with y up.
    assert_eq!(def.position(), [1.0, -0.5]);
    assert_eq!(def.rotation_deg(), 0.0);
    let turned = Transform2 {
        translate: [20.0, -10.0],
        rotation_deg: 90.0,
        mirror: false,
    };
    let second = placed
        .iter()
        .find(|p| p.instance != placed[0].instance)
        .ok_or("R2")?;
    assert!(near(
        second.pad_transform().apply([0.0, 0.0]),
        turned.apply(def.position())
    ));
    Ok(())
}
