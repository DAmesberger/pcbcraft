//! A 4-layer board with inner tracks and tented and open through vias:
//! inner copper and via barrels are saved copper at their stackup heights,
//! the laminate is physical around them, and the display layers (domes,
//! finished rings, bores, silkscreen and edge bands) carry their layers.
use super::*;
use crate::geom::Vec3;
use sparksmith_core::{Engine, Track, Via, ViaSpan};

fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

fn invalid(error: impl std::fmt::Display) -> Error {
    Error::Verification(error.to_string())
}

/// The demo on a 4-layer stackup: a track on each inner layer of the
/// demo's first net, a tented via at (30, 20) and an open one at (40, 20)
/// joining them.
fn four_layer() -> Result<(Project, [Id; 2], [Id; 2])> {
    let mut project = Engine::demo().map_err(invalid)?.project().clone();
    project.stackup = Stackup::preset(4).map_err(invalid)?;
    let net = project
        .nets
        .first()
        .map(|n| n.id)
        .ok_or_else(|| invalid("demo has no net"))?;
    let mut vias = [Id::default(); 2];
    for (k, (x, tented)) in [(30.0, true), (40.0, false)].into_iter().enumerate() {
        let id = project.allocate().map_err(invalid)?;
        project.vias.push(Via {
            id,
            net,
            position: [x, 20.0],
            drill: 0.3,
            diameter: 0.6,
            span: ViaSpan::Through,
            tented,
            locked: false,
        });
        vias[k] = id;
    }
    let mut tracks = [Id::default(); 2];
    for (k, (layer, y)) in [(CopperLayer::Inner(1), 20.0), (CopperLayer::Inner(2), 20.0)]
        .into_iter()
        .enumerate()
    {
        let id = project.allocate().map_err(invalid)?;
        project.tracks.push(Track {
            id,
            net,
            start: [30.0, y],
            end: [40.0, y],
            width: 0.3,
            layer,
            locked: false,
        });
        tracks[k] = id;
    }
    Ok((project, tracks, vias))
}

fn geometry_of(model: &NativeModel, pick: impl Fn(&Source) -> bool) -> Vec<&Geometry> {
    let picks: Vec<u64> = model
        .sources
        .iter()
        .filter(|s| pick(s))
        .map(|s| s.pick_id)
        .collect();
    model
        .geometry
        .iter()
        .filter(|g| g.item.is_some_and(|i| picks.contains(&i)))
        .collect()
}

#[test]
fn inner_copper_and_via_barrels_are_saved_copper_at_their_stackup_heights() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let (project, tracks, vias) = four_layer()?;
    let stackup = project.stackup.clone();
    let model = build(&project)?;
    // The laminate is the stackup's dielectric.
    let substrate = model
        .geometry
        .iter()
        .find(|g| g.kind == BodyKind::Substrate)
        .ok_or_else(|| invalid("no substrate"))?;
    near(substrate.max[2], stackup.dielectric_mm());
    for (id, layer) in tracks
        .iter()
        .zip([CopperLayer::Inner(1), CopperLayer::Inner(2)])
    {
        let copper = geometry_of(&model, |s| s.track == Some(*id));
        assert_eq!(copper.len(), 1, "inner copper has no cap or finish");
        let g = copper[0];
        assert_eq!((g.kind, g.layer), (BodyKind::CopperTrack, Some(layer)));
        let (low, high) = stackup
            .copper_span(layer)
            .ok_or_else(|| invalid("no span"))?;
        near(g.min[2], low);
        near(g.max[2], high);
        let track = project
            .tracks
            .iter()
            .find(|t| t.id == *id)
            .ok_or_else(|| invalid("no track"))?;
        let outline = track_outline(track.start, track.end, track.width);
        let n = outline.len();
        let area = (0..n)
            .map(|i| {
                outline[i][0] * outline[(i + 1) % n][1] - outline[(i + 1) % n][0] * outline[i][1]
            })
            .sum::<f64>()
            / 2.0;
        near(g.volume(), area * (high - low));
    }
    for id in vias {
        let source = model
            .sources
            .iter()
            .find(|s| s.via == Some(id))
            .ok_or_else(|| invalid("via has no source"))?;
        assert!(source.net.is_some() && source.layer.is_none());
        let copper: Vec<&Geometry> = geometry_of(&model, |s| s.via == Some(id))
            .into_iter()
            .filter(|g| g.kind == BodyKind::ViaCopper)
            .collect();
        // The barrel and a ring on each of the four layers.
        assert_eq!(copper.len(), 5);
        let low = copper.iter().map(|g| g.min[2]).fold(f64::MAX, f64::min);
        let high = copper.iter().map(|g| g.max[2]).fold(f64::MIN, f64::max);
        near(low, -0.035);
        near(high, stackup.dielectric_mm() + 0.035);
        // The 0.3 mm drill is the finished bore; the barrel lines a hole
        // drilled wider by its wall.
        let (r, d, b) = (0.3, 0.15 + pad_outline::PLATING_MM, 0.15);
        let pi = std::f64::consts::PI;
        let barrel = pi * (d * d - b * b) * stackup.dielectric_mm();
        let outer = 2.0 * pi * (r * r - b * b) * 0.035;
        // Inner rings are exact annuli round the drill, the circle the
        // laminate's foil level leaves for them.
        let inner = 2.0 * pi * (r * r - d * d) * 0.035;
        let total: f64 = copper.iter().map(|g| g.volume()).sum();
        assert!(
            (total - (barrel + outer + inner)).abs() < 1e-4,
            "{total} vs {}",
            barrel + outer + inner
        );
    }
    Ok(())
}

#[test]
fn the_laminate_is_physical_around_inner_copper_and_drills() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let (project, _, _) = four_layer()?;
    let baseline = {
        let mut bare = project.clone();
        bare.tracks.retain(|t| t.layer.side().is_some());
        bare.vias.clear();
        build(&bare)?
    };
    let model = build(&project)?;
    let laminate = |m: &NativeModel| {
        m.geometry
            .iter()
            .filter(|g| g.kind == BodyKind::Substrate)
            .map(|g| g.volume())
            .sum::<f64>()
    };
    let pi = std::f64::consts::PI;
    let t = project.stackup.dielectric_mm();
    let foil = 0.035;
    // Each inner layer loses the union of its track and two via rings:
    // the track's 10 × 0.3 rectangle and the two ring discs less what of
    // each the track already covers; the drills go through everything.
    let disc = pi * 0.3 * 0.3;
    let drilled = 0.15 + crate::pad_outline::PLATING_MM;
    let drill = pi * drilled * drilled;
    let removed = baseline_removed(&project)?;
    let expected =
        laminate(&baseline) - 2.0 * drill * t - 2.0 * removed * foil + 2.0 * 2.0 * drill * foil;
    // Within the drills' tessellation (their circles are meshed as
    // polygons, about 0.2 % short of the disc): far below the 0.2 mm³ of
    // inner copper the laminate gives up.
    let error = (laminate(&model) - expected).abs();
    assert!(error < 1e-3, "{error}");
    assert!(2.0 * removed * foil > 200.0 * error);
    assert!(removed > 10.0 * 0.3 && removed < 10.0 * 0.3 + 2.0 * disc);
    Ok(())
}

/// The area one inner layer's copper covers, exactly: the track's
/// rectangle (which runs from one via centre to the other) and each ring
/// disc less the part of it inside the rectangle, a half of the disc's
/// strip `|y| <= w / 2`.
fn baseline_removed(project: &Project) -> Result<f64> {
    let layer = CopperLayer::Inner(1);
    let track = project
        .tracks
        .iter()
        .find(|t| t.layer == layer)
        .ok_or_else(|| invalid("no inner track"))?;
    let length = (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
    let h = track.width / 2.0;
    let rings: f64 = project
        .vias
        .iter()
        .map(|v| {
            let r = v.diameter / 2.0;
            let strip_half = h * (r * r - h * h).sqrt() + r * r * (h / r).asin();
            std::f64::consts::PI * r * r - strip_half
        })
        .sum();
    Ok(length * track.width + rings)
}

#[test]
fn stackup_parameters_drive_the_saved_board_after_reopening() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let (project, tracks, vias) = four_layer()?;
    let original = build(&project)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("four.solidcraft")
        .to_string_lossy()
        .into_owned();
    original.save(&path)?;
    let reopened = Native::open(&path)?;
    let mut before = finish(reopened, original.sources.clone())?;
    assert!(before.geometry.iter().all(|g| !g.kind.is_appearance()));
    assert_eq!(
        before.geometry.len(),
        original
            .geometry
            .iter()
            .filter(|g| !g.kind.is_appearance())
            .count()
    );
    // Thicken the bottom prepreg: the inner layers, the top and the vias'
    // barrels follow; the bottom copper stays.
    let mut native = before.native.ok_or_else(crate::project::no_design)?;
    native.change_parameters(json!({"dielectric_3_thickness": "0.5 mm"}))?;
    before = finish(native, before.sources)?;
    let t = project.stackup.dielectric_mm() + 0.3;
    let substrate = before
        .geometry
        .iter()
        .find(|g| g.kind == BodyKind::Substrate)
        .ok_or_else(|| invalid("no substrate"))?;
    near(substrate.max[2], t);
    let inner2 = geometry_of(&before, |s| s.track == Some(tracks[1]));
    near(inner2[0].max[2], 0.5);
    let inner1 = geometry_of(&before, |s| s.track == Some(tracks[0]));
    near(inner1[0].max[2], 0.5 + 1.2);
    let via = geometry_of(&before, |s| s.via == Some(vias[0]));
    let high = via.iter().map(|g| g.max[2]).fold(f64::MIN, f64::max);
    near(high, t + 0.035);
    Ok(())
}

#[test]
fn tented_vias_dome_under_the_mask_and_open_ones_show_rings_and_a_dark_bore() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let (project, _, vias) = four_layer()?;
    let model = build(&project)?;
    let t = project.stackup.dielectric_mm();
    let shown = |id: Id| -> Vec<(BodyKind, Option<CopperLayer>)> {
        let mut kinds: Vec<_> = geometry_of(&model, |s| s.via == Some(id))
            .into_iter()
            .filter(|g| g.kind.is_appearance())
            .map(|g| (g.kind, g.layer))
            .collect();
        kinds.sort();
        kinds
    };
    assert_eq!(
        shown(vias[0]),
        [
            (BodyKind::MaskCap, Some(CopperLayer::Top)),
            (BodyKind::MaskCap, Some(CopperLayer::Bottom)),
        ]
    );
    // A via's finished rings belong to the via, on no single layer, so
    // the 3D Vias toggle hides them with its barrel.
    assert_eq!(
        shown(vias[1]),
        [
            (BodyKind::PadFinish, None),
            (BodyKind::PadFinish, None),
            // The bore's shadow tube, and its dark disc on each open side.
            (BodyKind::ViaBore, None),
            (BodyKind::ViaBore, None),
            (BodyKind::ViaBore, None),
        ]
    );
    // From above: the tented via is under mask, the open one shows its
    // finished ring through an opening and its bore is dark, not bright
    // copper.
    let hit = |at: [f64; 2]| -> Option<BodyKind> {
        model
            .geometry
            .iter()
            .filter(|g| !matches!(g.kind, BodyKind::Silkscreen))
            .filter_map(|g| {
                g.raycast(Vec3::new(at[0], at[1], 10.0), Vec3::new(0.0, 0.0, -1.0))
                    .map(|(d, _)| (d, g.kind))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| k)
    };
    assert_eq!(hit([30.25, 20.0]), Some(BodyKind::MaskCap));
    assert_eq!(hit([30.0, 20.0]), Some(BodyKind::MaskCap));
    assert_eq!(hit([40.25, 20.0]), Some(BodyKind::PadFinish));
    assert_eq!(hit([40.0 + 0.117, 20.0]), Some(BodyKind::ViaBore));
    // The bore is open to the finished drill (0.15 mm radius).
    assert_eq!(hit([40.0 + 0.14, 20.0]), Some(BodyKind::ViaBore));
    // Straight into the drill, the hole is dark, though the inner tracks
    // end at its centre.
    assert_eq!(hit([40.0, 20.0]), Some(BodyKind::ViaBore));
    // The dome stands over the ring as a cap over copper does.
    let dome = geometry_of(&model, |s| s.via == Some(vias[0]))
        .into_iter()
        .find(|g| g.kind == BodyKind::MaskCap && g.layer == Some(CopperLayer::Top))
        .ok_or_else(|| invalid("no dome"))?;
    near(
        dome.max[2],
        t + 0.035 + sparksmith_core::Stackup::default().mask_um / 1000.0,
    );
    Ok(())
}

/// An open via's mask opening is its ring grown by the rules' mask
/// expansion, as the Gerber mask draws it (`Via::mask_opening`).
#[test]
fn an_open_via_s_mask_opening_follows_the_mask_expansion_rule() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    use sparksmith_core::rules::{RuleKey, RuleOverride, RuleValue};
    let (mut project, _, _) = four_layer()?;
    project.rules = sparksmith_core::rules::fixtures::rules().map_err(invalid)?;
    project.rules.overrides.push(RuleOverride {
        key: RuleKey::MaskExpansion,
        value: RuleValue::Length(0.2),
    });
    let model = build(&project)?;
    let top = |at: [f64; 2]| -> Option<BodyKind> {
        model
            .geometry
            .iter()
            .filter(|g| !matches!(g.kind, BodyKind::Silkscreen))
            .filter_map(|g| {
                g.raycast(Vec3::new(at[0], at[1], 10.0), Vec3::new(0.0, 0.0, -1.0))
                    .map(|(d, _)| (d, g.kind))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| k)
    };
    // The open via's 0.6 mm ring at (40, 20): bare laminate shows out to
    // 0.3 + 0.2 mm, the mask beyond.
    assert_eq!(top([40.0, 20.45]), Some(BodyKind::OpenLaminate));
    assert_eq!(top([40.0, 20.55]), Some(BodyKind::MaskTop));
    Ok(())
}

#[test]
fn every_display_layer_names_its_copper_layer_or_side() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let (project, _, _) = four_layer()?;
    let model = build(&project)?;
    let mut seen = BTreeSet::new();
    let via_picks: BTreeSet<u64> = model
        .sources
        .iter()
        .filter(|s| s.via.is_some())
        .map(|s| s.pick_id)
        .collect();
    for g in model.geometry.iter().filter(|g| g.kind.is_appearance()) {
        seen.insert(g.kind);
        let of_via = g.item.is_some_and(|i| via_picks.contains(&i));
        let layered = match g.kind {
            BodyKind::MaskTop
            | BodyKind::MaskBottom
            | BodyKind::MaskCap
            | BodyKind::Silkscreen
            | BodyKind::OpenLaminate
            | BodyKind::EdgeCopper => true,
            // A via's finished ring is the via's, on no single layer.
            BodyKind::PadFinish => !of_via,
            _ => false,
        };
        assert_eq!(g.layer.is_some(), layered, "{:?}", g.kind);
        match g.kind {
            BodyKind::MaskTop => assert_eq!(g.layer, Some(CopperLayer::Top)),
            BodyKind::MaskBottom => assert_eq!(g.layer, Some(CopperLayer::Bottom)),
            BodyKind::Silkscreen => assert!(g.layer.and_then(CopperLayer::side).is_some()),
            _ => {}
        }
    }
    for kind in [
        BodyKind::Silkscreen,
        BodyKind::EdgeCopper,
        BodyKind::EdgeDielectric,
        BodyKind::ViaBore,
    ] {
        assert!(seen.contains(&kind), "{kind:?}");
    }
    let edge_copper: BTreeSet<_> = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::EdgeCopper)
        .filter_map(|g| g.layer)
        .collect();
    assert_eq!(edge_copper.len(), 4);
    Ok(())
}

#[test]
fn silkscreen_prints_the_legend_on_the_mask_clear_of_openings() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let project = Engine::demo().map_err(invalid)?.project().clone();
    let model = build(&project)?;
    let films: Vec<&Geometry> = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Silkscreen)
        .collect();
    assert!(!films.is_empty());
    let mask_top = 1.6 + project.stackup.mask_laminate_nm.to_mm_f64();
    for film in &films {
        // Each part's legend shares the part's pick identity.
        let source = model
            .sources
            .iter()
            .find(|s| Some(s.pick_id) == film.item)
            .ok_or_else(|| invalid("legend without its part"))?;
        assert!(source.instance.is_some() && source.pad.is_none());
        assert_eq!(film.layer, Some(CopperLayer::Top));
        assert!(film.max[2] > mask_top && film.min[2] < mask_top);
        assert!(film.max[2] - film.min[2] < 0.05);
        // Matte ink with its grain.
        assert_eq!(film.detail.pattern, crate::material::Pattern::Silkscreen);
    }
    // No ink inside a mask opening: every opening's centre is clear.
    let pads = layout::placed_pads(&project);
    for pad in &pads {
        let c = pad.center();
        let z = 1.6 + project.stackup.mask_laminate_nm.to_mm_f64();
        for film in &films {
            assert!(
                !film.world_mesh().contains(Vec3::new(c[0], c[1], z)),
                "{c:?}"
            );
        }
    }
    // White ink by default; the colour follows the fabrication.
    let mut black = project.clone();
    black.fabrication.silkscreen_colour = sparksmith_core::SilkscreenColour::Black;
    let dark = build(&black)?;
    let ink = |m: &NativeModel| {
        m.geometry
            .iter()
            .find(|g| g.kind == BodyKind::Silkscreen)
            .map(|g| g.surface.base_color().x)
    };
    assert!(ink(&model) > Some(0.5) && ink(&dark) < Some(0.1));
    Ok(())
}

/// A through-hole pad's bodies lie on their own layers: its land on the
/// placement side, its opposite land on the other side and its plated
/// barrels on none, so hiding one side's copper in 3D hides that side's
/// lands only (the barrels go with the vias).
#[test]
fn a_through_hole_pad_s_lands_and_barrels_carry_their_own_layers() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let project = Engine::showcase().map_err(invalid)?.project().clone();
    let model = build(&project)?;
    let through: BTreeSet<u64> = model
        .sources
        .iter()
        .filter(|s| s.pad.is_some() && s.layer == Some(CopperLayer::Top))
        .map(|s| s.pick_id)
        .collect();
    let mut layers = BTreeSet::new();
    for g in model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::PadCopper && g.item.is_some_and(|i| through.contains(&i)))
    {
        layers.insert(g.layer);
    }
    assert_eq!(
        layers,
        BTreeSet::from([Some(CopperLayer::Top), Some(CopperLayer::Bottom), None]),
        "top lands, the header's bottom lands and its barrels"
    );
    // The bottom lands lie under the board, the top ones on it.
    for g in model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::PadCopper && g.item.is_some_and(|i| through.contains(&i)))
    {
        match g.layer {
            Some(CopperLayer::Bottom) => assert!(g.max[2] <= 1e-6, "{:?}", g.max),
            Some(CopperLayer::Top) => assert!(g.min[2] >= project.stackup.dielectric_mm() - 1e-6),
            _ => {}
        }
    }
    Ok(())
}

/// Any layer count: on twelve layers a track on Inner 9 is saved copper at
/// its stackup height, and the laminate is the twelve-layer dielectric.
#[test]
fn a_twelve_layer_board_saves_deep_inner_copper_at_its_height() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let (mut project, tracks, _) = four_layer()?;
    project.stackup = Stackup::preset(12).map_err(invalid)?;
    let layer = CopperLayer::Inner(9);
    for track in &mut project.tracks {
        track.layer = layer;
    }
    let model = build(&project)?;
    let substrate = model
        .geometry
        .iter()
        .find(|g| g.kind == BodyKind::Substrate)
        .ok_or_else(|| invalid("no substrate"))?;
    near(substrate.max[2], project.stackup.dielectric_mm());
    let (low, high) = project
        .stackup
        .copper_span(layer)
        .ok_or_else(|| invalid("no span"))?;
    for id in tracks {
        let copper = geometry_of(&model, |s| s.track == Some(id));
        assert_eq!(copper.len(), 1);
        assert_eq!(copper[0].layer, Some(layer));
        near(copper[0].min[2], low);
        near(copper[0].max[2], high);
    }
    let foils = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::EdgeCopper)
        .count();
    assert_eq!(foils, 12, "every foil shows at the routed edge");
    Ok(())
}

/// One z model: on a stackup whose foils and dielectrics all differ, every
/// copper body the native model builds (its height parameters and the
/// inner foils' `inner_base` expressions, evaluated by Solidcraft) lies
/// where [`crate::stack::Stack`] and `Stackup::copper_span` put that layer,
/// the laminate ends at the stack's laminate face, and each side's mask
/// over copper stands the stackup's mask thickness over its foil.
#[test]
fn native_heights_agree_with_the_stack() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    use crate::stack::{Slot, Stack};
    let (mut project, _, _) = four_layer()?;
    let mut stackup = Stackup::preset(4).map_err(invalid)?;
    for (foil, um) in stackup.copper.iter_mut().zip([70.0, 18.0, 50.0, 105.0]) {
        foil.thickness_um = um;
    }
    for (d, mm) in stackup.dielectrics.iter_mut().zip([0.18, 0.9, 0.25]) {
        d.thickness_mm = mm;
    }
    stackup.mask_um = 22.0;
    project.stackup = stackup.clone();
    let model = build(&project)?;
    let stack = Stack::of(&stackup, project.fabrication.pad_finish)?;
    assert_eq!(model.stack, stack);
    let mut seen = std::collections::BTreeSet::new();
    for g in model
        .geometry
        .iter()
        .filter(|g| matches!(g.kind, BodyKind::CopperTrack | BodyKind::EdgeCopper))
    {
        let layer = g.layer.ok_or_else(|| invalid("copper without a layer"))?;
        let (low, high) = stackup
            .copper_span(layer)
            .ok_or_else(|| invalid("no span"))?;
        let span = stack
            .span_of(Slot::Copper(layer))
            .ok_or_else(|| invalid("no slot"))?;
        near(span[0], low);
        near(span[1], high);
        if g.kind == BodyKind::CopperTrack {
            near(g.min[2], low);
            near(g.max[2], high);
            seen.insert(layer);
        }
    }
    assert!(seen.contains(&CopperLayer::Inner(1)) && seen.contains(&CopperLayer::Inner(2)));
    // The laminate is stacked slabs, one per dielectric and inner foil.
    let slabs = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Substrate);
    let low = slabs
        .clone()
        .map(|g| g.min[2])
        .fold(f64::INFINITY, f64::min);
    let high = slabs.map(|g| g.max[2]).fold(f64::NEG_INFINITY, f64::max);
    near(low, 0.0);
    near(high, stack.laminate());
    for (side, z) in [
        (Layer::Top, stack.laminate() + 0.070 + 0.022),
        (Layer::Bottom, -(0.105 + 0.022)),
    ] {
        let span = stack
            .span_of(Slot::Mask(side))
            .ok_or_else(|| invalid("no mask"))?;
        near(if side == Layer::Top { span[1] } else { span[0] }, z);
    }
    Ok(())
}
