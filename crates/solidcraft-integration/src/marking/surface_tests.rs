//! D7: decals on any surface: box faces and named anchors, cylinder
//! wraps, placements, finishes and generated codes, on built models.
use super::board::{self, DECAL_KIND, Marked};
use super::*;
use crate::material::BodyKind;
use crate::scene::Geometry;
use pcbcraft_core::library::marking::surface::{self as core_surface, Wrap};
use pcbcraft_core::library::marking::{
    BoxFace, Ecc, LaidText, MarkingItem, MarkingSpec, SurfaceRef, Symbology, Template,
};
use pcbcraft_core::library::package_form::module::ModuleForm;
use pcbcraft_core::library::package_form::tht::{ThtForm, ThtKind};
use pcbcraft_core::library::package_form::{FamilyForm, PackageForm};
use pcbcraft_core::library::{GeneratedMaterials, PackageDefinition, PackageModel};
use pcbcraft_core::materials::MaterialId;
use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn text_line(text: &str, height: f64, origin: [f64; 2]) -> LaidElement {
    LaidElement::Text(LaidText {
        text: text.into(),
        height_mm: height,
        origin,
    })
}

fn laid(elements: Vec<LaidElement>, finish: Finish, placement: Placement) -> MarkingLayout {
    MarkingLayout {
        elements,
        finish,
        placement,
        ..MarkingLayout::default()
    }
}

fn on(name: &str) -> Placement {
    Placement {
        on: SurfaceRef::Anchor { name: name.into() },
        ..Placement::default()
    }
}

fn radial_form() -> ThtForm {
    ThtForm {
        kind: ThtKind::Radial {
            body_diameter: 5.0,
            body_height: 11.0,
        },
        ..ThtForm::typical()
    }
}

/// The demo board with its package made a generated radial can (two
/// leads, as the demo's parts have), every part turned a quarter and the
/// second one on the bottom.
fn radial_board() -> std::result::Result<pcbcraft_core::Project, Box<dyn std::error::Error>> {
    let mut project = pcbcraft_core::Engine::demo()?.project().clone();
    let package = &mut project.library.packages[0].definition;
    let form = radial_form();
    package.envelope = form.envelope().ok_or("envelope")?;
    package.form = PackageForm::Tht(Box::new(form));
    package.model = PackageModel::Generated {
        family_rev: 1,
        materials: GeneratedMaterials::default(),
    };
    for p in &mut project.placements {
        p.rotation_deg = 90.0;
    }
    if let Some(bottom) = project.placements.get_mut(1) {
        bottom.layer = pcbcraft_core::Layer::Bottom;
    }
    Ok(project)
}

fn module_package() -> std::result::Result<PackageDefinition, Box<dyn std::error::Error>> {
    let mut package = pcbcraft_core::Engine::demo()?.project().library.packages[0]
        .definition
        .clone();
    package.form = PackageForm::Module(Box::new(ModuleForm::typical()));
    package.envelope = package.form.envelope().ok_or("envelope")?;
    package.model = PackageModel::Generated {
        family_rev: 1,
        materials: GeneratedMaterials::default(),
    };
    Ok(package)
}

fn bounds(geometry: &[Geometry]) -> ([f64; 3], [f64; 3]) {
    geometry
        .iter()
        .fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), g| {
            (
                std::array::from_fn(|k| lo[k].min(g.min[k])),
                std::array::from_fn(|k| hi[k].max(g.max[k])),
            )
        })
}

/// The sleeve of a 5 × 11 mm radial can as a wrap map.
fn sleeve(
    wrap: Wrap,
) -> std::result::Result<(CylinderArea, WrapChart), Box<dyn std::error::Error>> {
    let form = PackageForm::Tht(Box::new(radial_form()));
    let Some(core_surface::SurfaceGeometry::Cylinder(c)) =
        core_surface::anchor_of(&form, "sleeve").map(|a| a.geometry)
    else {
        return Err("no sleeve".into());
    };
    let chart = c.chart(&wrap).ok_or("chart")?;
    Ok((c, chart))
}

/// A wrapped decal lies on the cylinder its finish's lift away, keeps
/// arc length (a line's angular span times the radius is its chart
/// width), and its triangles follow the curve.
#[test]
fn a_wrapped_line_keeps_its_arc_length_on_the_sleeve() -> TestResult {
    let (c, chart) = sleeve(Wrap::default())?;
    let region = Region::wrap(&c, &chart);
    assert_eq!(region.periodic, [true, false]);
    let line = laid(
        vec![text_line("100µF 25V", 1.2, [0.0, 0.0])],
        Finish::InkWhite,
        on("sleeve"),
    );
    let flat = flatten(&line, &region, &no_artwork)?;
    let [x0, _, x1, _] = flat.bounds().ok_or("drawn")?;
    let mesh = place_on(&flat, &SurfaceMap::cylinder(c, chart)?)?;
    let lift = Finish::InkWhite.lift_mm();
    let mut angles = Vec::new();
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let (x, y) = (f64::from(p[0]), f64::from(p[1]));
        let r = x.hypot(y);
        assert!((r - (2.5 + lift)).abs() < 1e-4, "radius {r}");
        // The normal is the radial direction.
        let (nx, ny) = (f64::from(n[0]), f64::from(n[1]));
        assert!((nx * x + ny * y) / r > 0.999);
        angles.push(y.atan2(x).rem_euclid(TAU));
    }
    // The line is centred at 180° (the default seam's far side, −x).
    let (lo, hi) = angles
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), a| (l.min(*a), h.max(*a)));
    // (Within the glyphs' own asymmetry.)
    assert!(((lo + hi) / 2.0 - PI).abs() < 0.05, "{lo} {hi}");
    let arc = (hi - lo) * (2.5 + lift);
    let width = x1 - x0;
    // Arc length on the lifted radius over chart width: (r + lift) / r.
    assert!(
        (arc / width - (2.5 + lift) / 2.5).abs() < 1e-3,
        "{arc} vs {width}"
    );
    // Subdivided: more placed triangles than flat ones, chords within
    // the tolerance (every edge spans at most the split step round).
    assert!(mesh.triangle_count() > flat.triangle_count());
    Ok(())
}

/// An electrolytic's stripe at angle 0 (lead 2, +x, the negative lead)
/// straddles the default seam: on a full turn it continues round, with
/// no gap and no overlap, and its minus signs are left open.
#[test]
fn the_stripe_wraps_the_seam_on_the_negative_lead_side() -> TestResult {
    let (c, chart) = sleeve(Wrap::default())?;
    let region = Region::wrap(&c, &chart);
    let w = region.size[0];
    let stripe_w = 0.22 * w;
    let height = region.size[1];
    // As the core lays it: centred at u = πr, half past the chart's edge.
    let stripe = laid(
        vec![LaidElement::Polarity {
            mark: PolarityMark::Stripe,
            origin: [w / 2.0, 0.0],
            size_mm: [stripe_w, height],
        }],
        Finish::Ink {
            colour: [235, 235, 230],
        },
        on("sleeve"),
    );
    let flat = flatten(&stripe, &region, &no_artwork)?;
    // Nothing is clipped round the turn: the whole stripe less its minus
    // signs is drawn, once.
    let minus = {
        let pitch = 1.6 * stripe_w;
        let count = (height / pitch).floor();
        count * (0.5 * stripe_w) * (0.14 * stripe_w)
    };
    assert!(
        (flat.area_mm2() - (stripe_w * height - minus)).abs() < 1e-6,
        "{} vs {}",
        flat.area_mm2(),
        stripe_w * height - minus
    );
    let map = SurfaceMap::cylinder(c, chart)?;
    let mesh = place_on(&flat, &map)?;
    // Every vertex is within ±(stripe/2r) of angle 0: the +x side.
    let half = stripe_w / 2.0 / 2.5;
    for p in &mesh.positions {
        let a = f64::from(p[1]).atan2(f64::from(p[0]));
        assert!(a.abs() <= half + 1e-4, "{a} beyond ±{half}");
        assert!(p[0] > 0.0, "on the +x side");
    }
    // Both sides of the seam are covered right up to it: points just
    // past either chart edge (the same place on the can) are drawn, at
    // the stripe's mid-height between minus signs.
    let v = (0.14 * stripe_w).max(0.01) + 1.6 * stripe_w / 2.0 - height / 2.0 + 0.8 * stripe_w;
    let inside = |u: f64| flat.covers([u, v]) || flat.covers([u - w, v]) || flat.covers([u + w, v]);
    for du in [0.01, 0.2, stripe_w / 2.0 - 0.01] {
        assert!(inside(w / 2.0 - du), "before the seam at {du}");
        assert!(inside(w / 2.0 + du), "after the seam at {du}");
    }
    // The surface area is the chart area (arc-length mapping, lifted).
    let surface: f64 = mesh
        .indices
        .chunks_exact(3)
        .map(|t| {
            let p = |i: u32| mesh.positions[i as usize].map(f64::from);
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let e = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let f = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let x = [
                e[1] * f[2] - e[2] * f[1],
                e[2] * f[0] - e[0] * f[2],
                e[0] * f[1] - e[1] * f[0],
            ];
            (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt() / 2.0
        })
        .sum();
    let lift = (2.5 + Finish::InkWhite.lift_mm()) / 2.5;
    assert!(
        (surface / flat.area_mm2() - lift).abs() < 2e-3,
        "{surface} vs {}",
        flat.area_mm2()
    );
    // A minus sign is open: the stripe's centre column between dashes is
    // drawn, a dash's centre is not.
    let first = 0.5 * 1.6 * stripe_w - (height / (1.6 * stripe_w)).floor() * 1.6 * stripe_w / 2.0;
    assert!(!inside(w / 2.0 + 0.001) || !flat.covers([w / 2.0, first]));
    Ok(())
}

/// The generated electrolytic marking end to end: the core lays out the
/// µF/V lines opposite the stripe on the sleeve, the decal draws both.
#[test]
fn a_generated_electrolytic_marking_wraps_its_sleeve() -> TestResult {
    use pcbcraft_core::FieldRef;
    use pcbcraft_core::ids::{DefinitionId, Revision};
    use pcbcraft_core::library::marking::{self as core, MarkingInputs};
    let (cap, volt) = (DefinitionId(5), DefinitionId(6));
    let spec = MarkingSpec {
        items: vec![MarkingItem::Electrolytic {
            capacitance: FieldRef::new(cap, Revision::FIRST),
            voltage: Some(FieldRef::new(volt, Revision::FIRST)),
            stripe: true,
        }],
        finish: Finish::Ink {
            colour: [230, 230, 225],
        },
        placement: on("sleeve"),
        ..Default::default()
    };
    let form = PackageForm::Tht(Box::new(radial_form()));
    let mut inputs = MarkingInputs {
        designator: "C1".into(),
        ..Default::default()
    };
    inputs.fields.insert(cap, ("470u".into(), Some(470e-6)));
    inputs.fields.insert(volt, ("16".into(), Some(16.0)));
    let region = |s: &MarkingSpec| {
        let found = core_surface::anchor_of(&form, "sleeve")?;
        core_surface::region(&found.geometry, &s.placement)
    };
    let layout = core::layout_with(&spec, &inputs, &region)?;
    let lines: Vec<&str> = layout
        .elements
        .iter()
        .filter_map(|e| match e {
            LaidElement::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(lines, ["470µF", "16V"]);
    let (c, chart) = sleeve(Wrap::default())?;
    let flat = flatten(&layout, &Region::wrap(&c, &chart), &no_artwork)?;
    let mesh = place_on(&flat, &SurfaceMap::cylinder(c, chart)?)?;
    // Text on the −x side, stripe on the +x side.
    assert!(mesh.positions.iter().any(|p| p[0] < -2.0));
    assert!(mesh.positions.iter().any(|p| p[0] > 2.0));
    Ok(())
}

/// A partial wrap is clipped to its arc; repeats sit side by side round
/// the turn.
#[test]
fn partial_wraps_clip_and_repeats_go_round() -> TestResult {
    let (c, chart) = sleeve(Wrap {
        wrap_deg: 90.0,
        along_mm: Some([2.0, 6.0]),
        ..Wrap::default()
    })?;
    let region = Region::wrap(&c, &chart);
    assert_eq!(region.periodic, [false, false]);
    let wide = laid(
        vec![LaidElement::Polarity {
            mark: PolarityMark::Bar,
            origin: [0.0, 0.0],
            size_mm: [50.0, 50.0],
        }],
        Finish::LaserEtch,
        on("sleeve"),
    );
    let flat = flatten(&wide, &region, &no_artwork)?;
    assert!((flat.area_mm2() - region.size[0] * region.size[1]).abs() < 1e-9);
    let (c2, chart2) = sleeve(Wrap {
        repeat: 3,
        ..Wrap::default()
    })?;
    let region = Region::wrap(&c2, &chart2);
    assert_eq!(region.copies.len(), 3);
    let dot = laid(
        vec![LaidElement::Polarity {
            mark: PolarityMark::Dot,
            origin: [0.0, 0.0],
            size_mm: [1.0, 1.0],
        }],
        Finish::LaserEtch,
        on("sleeve"),
    );
    let flat = flatten(&dot, &region, &no_artwork)?;
    let disc = PI * 0.25;
    assert!((flat.area_mm2() - 3.0 * disc).abs() < 0.05 * disc);
    let mesh = place_on(&flat, &SurfaceMap::cylinder(c2, chart2)?)?;
    // Three dots 120° apart.
    let mut seen = [false; 3];
    for p in &mesh.positions {
        let a = f64::from(p[1]).atan2(f64::from(p[0])).rem_euclid(TAU);
        // Copies centred at 60°, 180° and 300° (the chart starts at 0°).
        for (k, centre) in [PI / 3.0, PI, 5.0 * PI / 3.0].iter().enumerate() {
            if (a - centre).abs() < 0.3 {
                seen[k] = true;
            }
        }
    }
    assert_eq!(seen, [true; 3]);
    Ok(())
}

/// Placement on a plane: a quarter turn, a mirror and a scale move the
/// layout; clip cuts what leaves the face.
#[test]
fn placements_move_and_clip_the_layout_on_a_face() -> TestResult {
    let bar = |placement: Placement| {
        laid(
            vec![LaidElement::Polarity {
                mark: PolarityMark::Bar,
                origin: [1.0, 0.0],
                size_mm: [2.0, 0.5],
            }],
            Finish::LaserEtch,
            placement,
        )
    };
    let region = Region::plane([4.0, 4.0]);
    let turned = flatten(
        &bar(Placement {
            rotation_deg: 90.0,
            ..Placement::default()
        }),
        &region,
        &no_artwork,
    )?;
    let b = turned.bounds().ok_or("drawn")?;
    // (0..2, −0.25..0.25) turned to (−0.25..0.25, 0..2).
    assert!((b[0] + 0.25).abs() < 1e-9 && (b[2] - 0.25).abs() < 1e-9);
    assert!(b[1].abs() < 1e-9 && (b[3] - 2.0).abs() < 1e-9);
    let mirrored = flatten(
        &bar(Placement {
            mirror: true,
            ..Placement::default()
        }),
        &region,
        &no_artwork,
    )?;
    let b = mirrored.bounds().ok_or("drawn")?;
    assert!((b[0] + 2.0).abs() < 1e-9 && b[2].abs() < 1e-9);
    // Mirrored triangles still face out.
    for t in mirrored.triangles() {
        let [p, q, r] = t.map(|i| mirrored.points()[i as usize]);
        assert!((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]) > 0.0);
    }
    let big = flatten(
        &bar(Placement {
            scale: 2.0,
            offset_mm: [1.0, 0.0],
            ..Placement::default()
        }),
        &region,
        &no_artwork,
    )?;
    // Scaled to (0..4) + 1 = 1..5, clipped at the face's edge x = 2.
    let b = big.bounds().ok_or("drawn")?;
    assert!((b[0] - 1.0).abs() < 1e-9 && (b[2] - 2.0).abs() < 1e-9);
    assert!((big.area_mm2() - 1.0 * 1.0).abs() < 1e-9);
    let free = flatten(
        &bar(Placement {
            scale: 2.0,
            offset_mm: [1.0, 0.0],
            clip: false,
            ..Placement::default()
        }),
        &region,
        &no_artwork,
    )?;
    assert!((free.area_mm2() - 4.0).abs() < 1e-9);
    Ok(())
}

/// Finishes map to their materials: metal etch darker, rougher, still
/// metal; inks take the chosen colour; pad print glossier than screen
/// ink; an engraving darkens its face; inks stand higher.
#[test]
fn finishes_map_to_their_materials() -> TestResult {
    let can = look::surface(&look::definition(MaterialId::NickelSilver)?)?;
    let etched = finish_surface(Finish::LaserEtchMetal)?;
    let lum = |s: OpaqueSurface| {
        let c = s.base_color();
        0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
    };
    assert_eq!(
        Finish::LaserEtchMetal.material(),
        MaterialId::MetalLaserMarked
    );
    assert_eq!(etched.metallic(), 1.0, "it follows the can's lighting");
    assert!(lum(etched) < 0.5 * lum(can));
    assert!(etched.roughness() > can.roughness() + 0.2);
    let red = finish_surface(Finish::Ink {
        colour: [255, 0, 0],
    })?;
    let c = red.base_color();
    assert!((c.x - 1.0).abs() < 1e-9 && c.y == 0.0 && c.z == 0.0);
    let white = finish_surface(Finish::InkWhite)?;
    assert_eq!(red.roughness(), white.roughness(), "same ink optics");
    let pad = finish_surface(Finish::PadPrint {
        colour: [255, 255, 255],
    })?;
    assert!(pad.roughness() < white.roughness());
    // An engraving takes its face's look, darker and rougher; its walls
    // are lighter than its floor.
    let floor = finish_surface_on(Finish::Engraved, Some(can))?;
    assert!((lum(floor) - 0.55 * lum(can)).abs() < 1e-9);
    assert!(floor.roughness() > can.roughness());
    assert_eq!(floor.metallic(), can.metallic());
    let wall = ink_surface(Ink::Wall, Finish::Engraved, Some(can))?;
    assert!(lum(wall) > lum(floor));
    // Lifts: etches flush, pad print above, screen ink highest; all clear
    // of the face.
    let lifts = [
        Finish::LaserEtch,
        Finish::LaserEtchMetal,
        Finish::Engraved,
        Finish::PadPrint { colour: [0; 3] },
        Finish::InkWhite,
    ]
    .map(Finish::lift_mm);
    assert!(lifts.iter().all(|l| *l >= DECAL_OFFSET_MM));
    assert!(lifts[3] > lifts[0] && lifts[4] > lifts[3]);
    // Every finish's material is a cited built-in.
    for finish in [
        Finish::LaserEtch,
        Finish::LaserEtchMetal,
        Finish::InkWhite,
        Finish::Ink { colour: [1, 2, 3] },
        Finish::PadPrint { colour: [1, 2, 3] },
        Finish::Engraved,
    ] {
        let def = look::definition(finish.material())?;
        assert!(!def.source.is_empty(), "{finish:?}");
        finish_surface(finish)?;
        finish_detail(finish)?;
    }
    Ok(())
}

/// An engraving draws walls inside its outlines whose normals tilt into
/// the recess, standing just above the floor; no geometry is cut.
#[test]
fn engravings_shade_their_walls_with_tilted_normals() -> TestResult {
    let mark = laid(
        vec![LaidElement::Polarity {
            mark: PolarityMark::Bar,
            origin: [0.0, 0.0],
            size_mm: [1.0, 1.0],
        }],
        Finish::Engraved,
        Placement::default(),
    );
    let flat = flatten(&mark, &Region::plane([4.0, 4.0]), &no_artwork)?;
    let walls = flat.ink_area_mm2(Ink::Wall);
    // Four walls 0.03 mm wide inside a 1 mm square (corners overlap).
    assert!((walls - 4.0 * ENGRAVE_WALL_MM).abs() < 1e-9, "{walls}");
    assert!((flat.ink_area_mm2(Ink::Finish) - 1.0).abs() < 1e-9);
    let area = MarkingArea {
        origin: [0.0, 0.0, 1.0],
        normal: [0.0, 0.0, 1.0],
        u_axis: [1.0, 0.0, 0.0],
        size_mm: [4.0, 4.0],
    };
    let mesh = place(&flat, &area)?;
    let wall = mesh
        .runs
        .iter()
        .find(|r| r.ink == Ink::Wall)
        .ok_or("walls")?;
    for &i in &mesh.indices[wall.indices.clone()] {
        let n = mesh.normals[i as usize];
        let p = mesh.positions[i as usize];
        // Tilted towards the square's centre, up still dominant.
        assert!(n[2] > 0.5 && n[2] < 0.7, "{n:?}");
        let inward = -(f64::from(n[0]) * f64::from(p[0]) + f64::from(n[1]) * f64::from(p[1]));
        assert!(inward > 0.0);
        assert!(f64::from(p[2]) > 1.0 + Finish::Engraved.lift_mm());
    }
    let surfaces = board::decal_geometry(&mesh, Some(9), None)?;
    assert_eq!(surfaces.len(), 2, "floor and walls");
    Ok(())
}

/// A QR code's dark modules are drawn as merged rectangles of exactly
/// the dark area.
#[test]
fn a_2d_code_fills_its_dark_modules() -> TestResult {
    let symbol = pcbcraft_core::library::marking::matrix::encode(Symbology::Qr, "C7", Ecc::Medium)?;
    let dark = (0..symbol.size)
        .flat_map(|y| (0..symbol.size).map(move |x| (x, y)))
        .filter(|(x, y)| symbol.dark(*x, *y))
        .count();
    let module = 0.2;
    let code = laid(
        vec![LaidElement::Matrix {
            symbol: symbol.clone(),
            origin: [0.0, 0.0],
            module_mm: module,
        }],
        Finish::LaserEtchMetal,
        Placement::default(),
    );
    let flat = flatten(&code, &Region::plane([8.0, 8.0]), &no_artwork)?;
    assert!((flat.area_mm2() - dark as f64 * module * module).abs() < 1e-9);
    // The top-left finder's corner module (row 0) is at the top left.
    let side = module * f64::from(symbol.size);
    assert!(flat.covers([-side / 2.0 + module / 2.0, side / 2.0 - module / 2.0]));
    assert!(!flat.covers([-side / 2.0 + 1.5 * module, side / 2.0 - 1.5 * module]));
    Ok(())
}

/// On a module preview, anchors place decals exactly: the can's top, and
/// the board beyond the can in silkscreen; box faces are found in the
/// mesh (the +x side of the outermost body).
#[test]
fn module_anchors_place_the_can_and_board_markings() -> TestResult {
    let package = module_package()?;
    let preview = crate::project::preview(&package)?;
    let form = ModuleForm::typical();
    let layout = MarkingLayout {
        decals: vec![
            laid(
                vec![text_line("ANT", 1.0, [0.0, 0.0])],
                Finish::InkWhite,
                on("pcb_top"),
            ),
            laid(
                vec![text_line("SIDE", 0.5, [0.0, 0.0])],
                Finish::LaserEtch,
                Placement {
                    on: SurfaceRef::Face {
                        face: BoxFace::PosX,
                    },
                    ..Placement::default()
                },
            ),
        ],
        ..laid(
            vec![text_line("MODULE-1", 1.5, [0.0, 0.0])],
            Finish::LaserEtchMetal,
            on("can_top"),
        )
    };
    let mut cache = DecalCache::default();
    let (decals, problems) = board::preview_decals(
        &layout,
        &preview.geometry,
        Some(&package),
        &no_artwork,
        &mut cache,
    );
    assert!(problems.is_empty(), "{problems:?}");
    let can_top = form.height + Finish::LaserEtchMetal.lift_mm();
    let pcb_top = core_surface::MODULE_PAD_MM + form.pcb_thickness + Finish::InkWhite.lift_mm();
    let can: Vec<&Geometry> = decals
        .iter()
        .filter(|g| (g.min[2] - can_top).abs() < 1e-4)
        .collect();
    let pcb: Vec<&Geometry> = decals
        .iter()
        .filter(|g| (g.min[2] - pcb_top).abs() < 1e-4)
        .collect();
    assert!(
        !can.is_empty() && !pcb.is_empty(),
        "{:?}",
        decals.iter().map(|g| g.min).collect::<Vec<_>>()
    );
    let (lo, hi) = bounds(&pcb.iter().map(|g| (*g).clone()).collect::<Vec<_>>());
    // Beyond the can (y > 6.2) on the board (y < 12.5).
    assert!(lo[1] > 6.2 && hi[1] < 12.5, "{lo:?} {hi:?}");
    let (lo, hi) = bounds(&can.iter().map(|g| (*g).clone()).collect::<Vec<_>>());
    assert!(lo[1] > -2.5 - 8.7 && hi[1] < 6.2);
    // The +x face is the board's edge (the outermost body along +x).
    let side: Vec<&Geometry> = decals
        .iter()
        .filter(|g| (g.min[0] - (9.0 + Finish::LaserEtch.lift_mm())).abs() < 1e-4)
        .collect();
    assert!(!side.is_empty());
    for g in decals {
        assert_eq!(g.kind, DECAL_KIND);
    }
    // An anchor the package does not offer is reported, not guessed.
    let lost = laid(
        vec![text_line("X", 1.0, [0.0, 0.0])],
        Finish::LaserEtch,
        on("sleeve"),
    );
    let (none, problems) = board::preview_decals(
        &lost,
        &preview.geometry,
        Some(&package),
        &no_artwork,
        &mut cache,
    );
    assert!(none.is_empty() && problems.len() == 1);
    Ok(())
}

/// On a board, an anchored wrap follows each instance's placement: turned
/// a quarter, and turned over on the bottom side, the sleeve decal stays
/// on its can with its stripe towards that instance's lead 2.
#[test]
fn board_wraps_follow_each_instance_s_placement() -> TestResult {
    let project = radial_board()?;
    let model = crate::project::build(&project)?;
    let stripe = |w: f64, h: f64| LaidElement::Polarity {
        mark: PolarityMark::Stripe,
        origin: [w / 2.0, 0.0],
        size_mm: [0.22 * w, h],
    };
    let (c, chart) = sleeve(Wrap::default())?;
    let region = Region::wrap(&c, &chart);
    let layouts: BTreeMap<_, _> = project
        .placements
        .iter()
        .map(|p| {
            (
                p.instance,
                laid(
                    vec![stripe(region.size[0], region.size[1])],
                    Finish::InkWhite,
                    on("sleeve"),
                ),
            )
        })
        .collect();
    let mut cache = DecalCache::default();
    let (decals, problems) = board::board_decals_reporting(
        &project,
        &model.geometry,
        &model.sources,
        &layouts,
        &mut cache,
    );
    assert!(problems.is_empty(), "{problems:?}");
    // One flat decal serves both instances.
    assert_eq!(cache.built(), 1);
    for placement in &project.placements {
        let picks: Vec<u64> = model
            .sources
            .iter()
            .filter(|s| s.instance == Some(placement.instance) && s.pad.is_none())
            .map(|s| s.pick_id)
            .collect();
        let mine: Vec<Geometry> = decals
            .iter()
            .filter(|g| g.item.is_some_and(|i| picks.contains(&i)))
            .cloned()
            .collect();
        assert!(!mine.is_empty());
        let body = model
            .geometry
            .iter()
            .find(|g| g.kind == BodyKind::ModelBody && g.item.is_some_and(|i| picks.contains(&i)))
            .ok_or("body")?;
        let place = body.place.ok_or("placed")?;
        // Lead 2 is at package +x: where is it on the board?
        let lead2 = place.point(Vec3::new(1.25, 0.0, 5.0));
        let axis = place.point(Vec3::new(0.0, 0.0, 5.0));
        let toward = lead2 - axis;
        let (lo, hi) = bounds(&mine);
        let centre = Vec3::new(
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        ) - axis;
        assert!(
            centre.x * toward.x + centre.y * toward.y > 0.0,
            "{:?}: stripe {centre:?} vs lead 2 {toward:?}",
            placement.layer
        );
        // On the can's side, within its height.
        assert!(lo[2] >= body.min[2] - 1e-6 && hi[2] <= body.max[2] + 1e-6);
    }
    Ok(())
}

/// Two hundred marked instances: every decal is laid out, filled, placed
/// and meshed, and a second pass with one value changed rebuilds one.
/// Debug-build timings are printed as evidence, not asserted.
#[test]
fn two_hundred_marked_instances_build_their_decals() -> TestResult {
    let project = pcbcraft_core::Engine::demo()?.project().clone();
    let model = crate::project::build(&project)?;
    let area = MarkingArea {
        origin: [0.0, 0.0, 1.0],
        normal: [0.0, 0.0, 1.0],
        u_axis: [1.0, 0.0, 0.0],
        size_mm: [6.0, 4.0],
    };
    let symbol =
        pcbcraft_core::library::marking::matrix::encode(Symbology::Qr, "U000", Ecc::Medium)?;
    let marking = |k: usize| {
        laid(
            vec![
                text_line(&format!("U{k:03}"), 0.6, [-1.2, 0.8]),
                text_line("ESP-MOD", 0.5, [-1.2, -0.4]),
                LaidElement::Matrix {
                    symbol: pcbcraft_core::library::marking::matrix::encode(
                        Symbology::Qr,
                        &format!("U{k:03}"),
                        Ecc::Medium,
                    )
                    .unwrap_or_else(|_| symbol.clone()),
                    origin: [1.8, 0.0],
                    module_mm: 0.09,
                },
            ],
            Finish::LaserEtchMetal,
            Placement::default(),
        )
    };
    let layouts: Vec<MarkingLayout> = (0..200).map(marking).collect();
    let mut cache = DecalCache::default();
    let started = std::time::Instant::now();
    let mut triangles = 0;
    let mut bodies = Vec::new();
    for (k, l) in layouts.iter().enumerate() {
        let flat = cache.flat_in(l, &Region::plane(area.size_mm), &no_artwork)?;
        let mesh = place(&flat, &area)?;
        triangles += mesh.triangle_count();
        bodies.extend(board::decal_geometry(&mesh, Some(k as u64), None)?);
    }
    let cold = started.elapsed();
    assert_eq!(cache.built(), 200);
    cache.sweep();
    let started = std::time::Instant::now();
    let mut changed = layouts.clone();
    changed[7] = marking(999);
    for l in &changed {
        let flat = cache.flat_in(l, &Region::plane(area.size_mm), &no_artwork)?;
        place(&flat, &area)?;
    }
    let warm = started.elapsed();
    assert_eq!(cache.built(), 201, "one changed decal rebuilt");
    // The decal geometry packs into the model's stream unchanged.
    let mut geometry = model.geometry.clone();
    let before = geometry.len();
    geometry.extend(bodies);
    assert!(geometry.len() > before);
    println!(
        "200 marked instances: {triangles} decal triangles, cold {cold:?}, one changed {warm:?}"
    );
    Ok(())
}

/// Markings never enter the saved model: a marked board saves exactly
/// what the unmarked board saves.
#[test]
fn markings_are_absent_from_the_saved_model() -> TestResult {
    let mut project = pcbcraft_core::Engine::demo()?.project().clone();
    let plain = crate::project::build(&project)?;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("plain.solidcraft");
    plain.save(&a.to_string_lossy())?;
    // Mark every component with a can-style marking.
    for c in &mut project.library.components {
        c.definition.marking = Some(Box::new(MarkingSpec {
            items: vec![
                MarkingItem::Text {
                    template: Template("{designator}".into()),
                    height_mm: None,
                    align: Default::default(),
                },
                MarkingItem::Code2d {
                    symbology: Symbology::Qr,
                    payload: Template("{designator}".into()),
                    size_mm: 0.9,
                    anchor: Default::default(),
                    ecc: Ecc::Low,
                },
            ],
            ..Default::default()
        }));
    }
    let marked = crate::project::build(&project)?;
    let mut markings = board::BoardMarkings::default();
    let decals = markings.decals(&project, &marked.geometry, &marked.sources);
    assert!(!decals.is_empty(), "{:?}", markings.failures());
    assert!(
        decals
            .iter()
            .all(|g| g.kind == DECAL_KIND && g.kind.is_appearance())
    );
    let b = dir.path().join("marked.solidcraft");
    marked.save(&b.to_string_lossy())?;
    let (pa, pb) = (std::fs::read(&a)?, std::fs::read(&b)?);
    // The saved bodies are the same; no marking body or decal mesh.
    let saved = |p: &std::path::Path| -> std::result::Result<
        Vec<crate::project::Source>,
        Box<dyn std::error::Error>,
    > {
        let native = crate::native::Native::open(&p.to_string_lossy())?;
        Ok(crate::project::saved_sources(&native)?)
    };
    assert_eq!(saved(&a)?.len(), saved(&b)?.len());
    assert!(!String::from_utf8_lossy(&pb).contains("Marking"));
    assert_eq!(pa.len() / 1024, pb.len() / 1024, "same size to the KiB");
    assert!(marked.geometry.iter().all(|g| g.kind != DECAL_KIND));
    Ok(())
}

/// What `Marked` resolves without placement data: a preview's box faces
/// in its model frame.
#[test]
fn box_faces_resolve_in_the_model_frame() -> TestResult {
    let package = module_package()?;
    let preview = crate::project::preview(&package)?;
    let bodies: Vec<&Geometry> = preview
        .geometry
        .iter()
        .filter(|g| {
            matches!(
                g.kind,
                BodyKind::ModelBody | BodyKind::PartBody | BodyKind::PackageBody
            )
        })
        .collect();
    let marked = Marked {
        bodies,
        frame: None,
        top: true,
        u: [1.0, 0.0],
        package: Some(&package),
    };
    for face in BoxFace::ALL {
        let resolved = marked.resolve(&Placement {
            on: SurfaceRef::Face { face },
            ..Placement::default()
        });
        assert!(resolved.is_some(), "{face:?}");
    }
    Ok(())
}
