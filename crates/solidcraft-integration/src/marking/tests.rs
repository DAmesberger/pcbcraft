use super::board::{self, DECAL_KIND};
use super::fill::{Flat, fill};
use super::*;
use crate::material::BodyKind;
use crate::scene::Geometry;
use pcbcraft_core::library::marking::{LaidText, PolarityMark};
use pcbcraft_core::materials::MaterialId;
use std::collections::BTreeMap;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn square(c: Point, half: f64) -> Vec<Point> {
    vec![
        [c[0] - half, c[1] - half],
        [c[0] + half, c[1] - half],
        [c[0] + half, c[1] + half],
        [c[0] - half, c[1] + half],
    ]
}

fn covers(flat: &Flat, p: Point) -> bool {
    flat.triangles.iter().any(|t| {
        let q = t.map(|i| flat.points[i as usize]);
        let s = |a: Point, b: Point| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        s(q[0], q[1]) >= 0.0 && s(q[1], q[2]) >= 0.0 && s(q[2], q[0]) >= 0.0
    })
}

fn all_ccw(flat: &Flat) -> bool {
    flat.triangles.iter().all(|t| {
        let [a, b, c] = t.map(|i| flat.points[i as usize]);
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0
    })
}

#[test]
fn fills_follow_the_nonzero_rule() -> TestResult {
    // One square.
    let mut one = Flat::default();
    fill(&[square([0.0, 0.0], 1.0)], &mut one, 100)?;
    assert!((one.area() - 4.0).abs() < 1e-9);
    assert!(all_ccw(&one));
    // A counter (wound the other way) stays open.
    let mut ring = Flat::default();
    let mut hole = square([0.0, 0.0], 0.5);
    hole.reverse();
    fill(&[square([0.0, 0.0], 1.0), hole], &mut ring, 100)?;
    assert!((ring.area() - 3.0).abs() < 1e-9);
    assert!(!covers(&ring, [0.0, 0.0]));
    assert!(covers(&ring, [0.75, 0.0]));
    // Overlapping loops wound alike merge: the union, counted once.
    let mut union = Flat::default();
    fill(
        &[square([0.0, 0.0], 1.0), square([1.0, 1.0], 1.0)],
        &mut union,
        100,
    )?;
    assert!((union.area() - 7.0).abs() < 1e-9, "{}", union.area());
    assert!(all_ccw(&union));
    // A bow tie (edges crossing inside a slab) fills both lobes.
    let mut bow = Flat::default();
    fill(
        &[vec![[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]]],
        &mut bow,
        100,
    )?;
    assert!((bow.area() - 2.0).abs() < 1e-9, "{}", bow.area());
    Ok(())
}

#[test]
fn a_fill_beyond_its_budget_is_refused_and_adds_nothing() -> TestResult {
    let mut flat = Flat::default();
    fill(&[square([0.0, 0.0], 1.0)], &mut flat, 10)?;
    let before = flat.clone();
    let many: Vec<Vec<Point>> = (0..20)
        .map(|k| square([3.0 * k as f64, 0.0], 1.0))
        .collect();
    assert!(fill(&many, &mut flat, 5).is_err());
    assert_eq!(flat, before);
    Ok(())
}

fn text(text: &str, height: f64) -> LaidElement {
    LaidElement::Text(LaidText {
        text: text.into(),
        height_mm: height,
        origin: [0.0, 0.0],
    })
}

fn layout(elements: Vec<LaidElement>, finish: Finish) -> MarkingLayout {
    MarkingLayout {
        elements,
        finish,
        findings: Vec::new(),
    }
}

fn top(origin: [f64; 3], size: [f64; 2]) -> MarkingArea {
    MarkingArea {
        origin,
        normal: [0.0, 0.0, 1.0],
        u_axis: [1.0, 0.0, 0.0],
        size_mm: size,
    }
}

/// Every triangle of `mesh` faces its normal, and every vertex is in the
/// plane `DECAL_OFFSET_MM` off the area.
fn check_plane(mesh: &DecalMesh, area: &MarkingArea) {
    let n = mesh.normal.map(f64::from);
    let at = |i: u32| mesh.positions[i as usize].map(f64::from);
    let plane: f64 = (0..3).map(|k| area.origin[k] * n[k]).sum::<f64>() + DECAL_OFFSET_MM;
    for p in &mesh.positions {
        let d: f64 = (0..3).map(|k| f64::from(p[k]) * n[k]).sum();
        assert!((d - plane).abs() < 1e-5, "{d} vs {plane}");
    }
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [at(t[0]), at(t[1]), at(t[2])];
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let facing: f64 = (0..3).map(|k| cross[k] * n[k]).sum();
        assert!(facing > 0.0, "a triangle faces away from the normal");
    }
}

#[test]
fn a_value_code_becomes_glyph_outlines_on_the_face() -> TestResult {
    let area = top([10.0, 5.0, 0.55], [1.6, 0.8]);
    let mesh = decal(&layout(vec![text("103", 0.4)], Finish::LaserEtch), &area)?;
    assert!(mesh.triangle_count() > 20, "{}", mesh.triangle_count());
    assert_eq!(mesh.normal, [0.0, 0.0, 1.0]);
    assert_eq!(mesh.finish, Finish::LaserEtch);
    check_plane(&mesh, &area);
    // Centred on the area, inside it, about as tall as asked.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    assert!(lo[0] >= 9.2 && hi[0] <= 10.8, "{lo:?} {hi:?}");
    assert!(((lo[0] + hi[0]) / 2.0 - 10.0).abs() < 0.05);
    assert!(((lo[1] + hi[1]) / 2.0 - 5.0).abs() < 0.05);
    assert!((hi[1] - lo[1] - 0.4).abs() < 0.05, "{}", hi[1] - lo[1]);
    Ok(())
}

#[test]
fn glyph_counters_stay_open() -> TestResult {
    let flat = flatten(
        &layout(vec![text("O", 2.0)], Finish::InkWhite),
        [5.0, 5.0],
        &no_artwork,
    )?;
    assert!(!flat.is_empty());
    assert!(
        !covers(&flat.flat, [0.0, 0.0]),
        "the counter of O is filled"
    );
    assert!(all_ccw(&flat.flat));
    // The stroke of the O, left of its counter, is filled.
    let left = flat
        .flat
        .points
        .iter()
        .map(|p| p[0])
        .fold(f64::INFINITY, f64::min);
    assert!(covers(&flat.flat, [left + 0.08, 0.0]));
    Ok(())
}

#[test]
fn a_line_the_font_sets_wider_than_the_area_is_shrunk_to_fit() -> TestResult {
    let area = [1.0, 0.6];
    let flat = flatten(
        &layout(vec![text("10K0", 0.5)], Finish::LaserEtch),
        area,
        &no_artwork,
    )?;
    let xs = flat.flat.points.iter().map(|p| p[0]);
    let (lo, hi) = xs.fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| {
        (l.min(x), h.max(x))
    });
    assert!(hi - lo <= area[0] + 1e-9, "{}", hi - lo);
    assert!(hi - lo > 0.9 * area[0]);
    Ok(())
}

#[test]
fn the_decal_follows_the_area_frame() -> TestResult {
    // A bottom face, u along +y: the decal is below the face, faces down.
    let area = MarkingArea {
        origin: [3.0, 4.0, -1.6],
        normal: [0.0, 0.0, -1.0],
        u_axis: [0.0, 1.0, 0.0],
        size_mm: [3.0, 1.5],
    };
    let mesh = decal(&layout(vec![text("U1", 0.8)], Finish::InkWhite), &area)?;
    assert_eq!(mesh.normal, [0.0, 0.0, -1.0]);
    check_plane(&mesh, &area);
    assert!(
        mesh.positions
            .iter()
            .all(|p| (p[2] - (-1.601)).abs() < 1e-5)
    );
    // The line runs along u (+y): longer in y than in x.
    let span = |k: usize| {
        let v = mesh.positions.iter().map(|p| p[k]);
        let (l, h) = v.fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(x), h.max(x)));
        h - l
    };
    assert!(span(1) > span(0), "{} vs {}", span(1), span(0));
    Ok(())
}

#[test]
fn a_changed_value_refills_only_its_own_decal() -> TestResult {
    let mut cache = DecalCache::default();
    let (a, b): (Id, Id) = (1, 2);
    let size = [1.6, 0.8];
    let ten_k = layout(vec![text("103", 0.4)], Finish::LaserEtch);
    let one_k = layout(vec![text("102", 0.4)], Finish::LaserEtch);
    let first = cache.flat(a, &ten_k, size, &no_artwork)?;
    cache.flat(b, &ten_k, size, &no_artwork)?;
    assert_eq!(cache.built(), 2);
    cache.sweep();
    // Same layouts again: nothing is built.
    let again = cache.flat(a, &ten_k, size, &no_artwork)?;
    assert!(Arc::ptr_eq(&first, &again));
    cache.flat(b, &ten_k, size, &no_artwork)?;
    assert_eq!(cache.built(), 2);
    cache.sweep();
    // b's value changes: only b is built, and a's decal is the same.
    cache.flat(a, &ten_k, size, &no_artwork)?;
    let changed = cache.flat(b, &one_k, size, &no_artwork)?;
    assert_eq!(cache.built(), 3);
    assert_ne!(*changed, *first);
    cache.sweep();
    assert_eq!(cache.len(), 2, "the old decal of b is dropped");
    // Findings are advisory: they do not change the decal's identity.
    let mut noted = ten_k.clone();
    noted.findings.push(pcbcraft_core::Finding::new(
        "marking.does_not_fit",
        "x".into(),
        vec![],
    ));
    assert_eq!(layout_hash(&noted)?, layout_hash(&ten_k)?);
    Ok(())
}

#[test]
fn finishes_draw_with_their_cited_materials() -> TestResult {
    let laser = finish_surface(Finish::LaserEtch)?;
    let ink = finish_surface(Finish::InkWhite)?;
    let mould = look::surface(&look::definition(MaterialId::EpoxyMould)?)?;
    let luminance = |s: OpaqueSurface| {
        let c = s.base_color();
        0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
    };
    // Laser etch: lighter and rougher than the moulding it is cut into.
    assert!(luminance(laser) > 2.0 * luminance(mould));
    assert!(laser.roughness() > mould.roughness());
    // White ink: far lighter still, diffuse.
    assert!(luminance(ink) > 0.5);
    assert_eq!(ink.metallic(), 0.0);
    for finish in [Finish::LaserEtch, Finish::InkWhite] {
        let def = look::definition(finish.material())?;
        let s = finish_surface(finish)?;
        assert_eq!(s.roughness(), def.roughness);
    }
    Ok(())
}

#[test]
fn polarity_marks_and_bands_fill_their_boxes() -> TestResult {
    let dot = flatten(
        &layout(
            vec![LaidElement::Polarity {
                mark: PolarityMark::Dot,
                origin: [-0.5, 0.2],
                size_mm: [0.3, 0.3],
            }],
            Finish::InkWhite,
        ),
        [2.0, 1.0],
        &no_artwork,
    )?;
    let disc = std::f64::consts::PI * 0.15 * 0.15;
    assert!(
        (dot.area_mm2() - disc).abs() < 0.01 * disc,
        "{}",
        dot.area_mm2()
    );
    assert!(covers(&dot.flat, [-0.5, 0.2]));

    let colours = vec![1, 0, 2, 10];
    let banded = flatten(
        &layout(
            vec![LaidElement::Bands {
                colours: colours.clone(),
                pitch_mm: 0.8,
            }],
            Finish::LaserEtch,
        ),
        [4.0, 1.6],
        &no_artwork,
    )?;
    let runs: Vec<u8> = banded.bands.iter().map(|(c, _)| *c).collect();
    assert_eq!(runs, colours);
    // Each band is half a pitch wide across the area.
    assert!((banded.area_mm2() - 4.0 * 0.4 * 1.6).abs() < 1e-9);
    // The first band is centred one pitch from the −x end.
    assert!(covers(&banded.flat, [-2.0 + 0.8, 0.0]));
    assert!(!covers(&banded.flat, [-2.0 + 0.4, 0.0]));
    let mesh = place(&banded, &top([0.0, 0.0, 1.0], [4.0, 1.6]))?;
    assert_eq!(mesh.bands.len(), 4);
    assert!(mesh.bands.iter().all(|r| r.indices.len() % 3 == 0));
    for colour in 0..12 {
        band_colour(colour)?;
        band_surface(colour)?;
    }
    assert!(band_colour(12).is_err());
    let bad = layout(
        vec![LaidElement::Bands {
            colours: vec![12],
            pitch_mm: 0.8,
        }],
        Finish::LaserEtch,
    );
    assert!(flatten(&bad, [4.0, 1.6], &no_artwork).is_err());
    Ok(())
}

/// Original artwork: a filled square with a stroked bar beside it.
const ARTWORK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10">
<rect x="0" y="0" width="10" height="10" fill="black"/>
<line x1="14" y1="0" x2="14" y2="10" stroke="black" stroke-width="2"/>
</svg>"#;

#[test]
fn artwork_is_scaled_into_its_box() -> TestResult {
    let hash = ContentHash([7; 32]);
    let source = |h: &ContentHash| (*h == hash).then(|| ARTWORK.to_string());
    let marked = layout(
        vec![LaidElement::Artwork {
            svg: hash,
            origin: [0.0, 0.0],
            size_mm: [2.0, 2.0],
        }],
        Finish::LaserEtch,
    );
    let flat = flatten(&marked, [3.0, 3.0], &source)?;
    assert!(!flat.is_empty());
    let xs = flat.flat.points.iter().map(|p| p[0]);
    let (lo, hi) = xs.fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| {
        (l.min(x), h.max(x))
    });
    // 20 × 10 artwork (plus its stroke's round ends) into a 2 mm box.
    assert!(hi - lo <= 2.0 + 1e-6 && hi - lo > 1.5, "{}", hi - lo);
    // The square is filled: 1 mm × 1 mm at a tenth of its size.
    assert!(flat.area_mm2() > 1.0, "{}", flat.area_mm2());
    // Without the asset, artwork draws nothing.
    assert!(flatten(&marked, [3.0, 3.0], &no_artwork)?.is_empty());
    Ok(())
}

#[test]
fn bad_layouts_and_areas_are_refused() {
    let area = top([0.0, 0.0, 0.0], [2.0, 1.0]);
    let refused =
        |elements: Vec<LaidElement>| decal(&layout(elements, Finish::LaserEtch), &area).is_err();
    assert!(refused(vec![text("1", 0.0)]));
    assert!(refused(vec![text("1", f64::NAN)]));
    assert!(refused(vec![text(&"8".repeat(MAX_TEXT_CHARS + 1), 0.3)]));
    assert!(refused(vec![text("1", 0.3); MAX_DECAL_ELEMENTS + 1]));
    assert!(refused(vec![LaidElement::Polarity {
        mark: PolarityMark::Bar,
        origin: [0.0, 0.0],
        size_mm: [-1.0, 0.2],
    }]));
    let one = layout(vec![text("1", 0.3)], Finish::LaserEtch);
    for bad in [
        MarkingArea {
            normal: [0.0, 0.0, 0.0],
            ..area
        },
        MarkingArea {
            u_axis: [0.0, 0.0, 2.0],
            ..area
        },
        MarkingArea {
            size_mm: [0.0, 1.0],
            ..area
        },
        MarkingArea {
            origin: [f64::INFINITY, 0.0, 0.0],
            ..area
        },
    ] {
        assert!(decal(&one, &bad).is_err(), "{bad:?}");
    }
    // An empty layout is an empty decal, not an error.
    assert_eq!(
        decal(&layout(vec![], Finish::LaserEtch), &area)
            .map(|m| m.triangle_count())
            .ok(),
        Some(0)
    );
}

/// The demo board's built geometry, sources and each placed instance.
fn demo() -> std::result::Result<
    (pcbcraft_core::Project, crate::project::NativeModel),
    Box<dyn std::error::Error>,
> {
    let project = pcbcraft_core::Engine::demo()?.project().clone();
    let model = crate::project::build(&project)?;
    Ok((project, model))
}

#[test]
fn board_decals_sit_on_each_marked_instance_s_top_face() -> TestResult {
    let (project, model) = demo()?;
    let mut layouts = BTreeMap::new();
    for placement in &project.placements {
        layouts.insert(
            placement.instance,
            layout(vec![text("103", 0.3)], Finish::LaserEtch),
        );
    }
    assert!(!layouts.is_empty());
    let mut cache = DecalCache::default();
    let decals = board::board_decals(
        &project,
        &model.geometry,
        &model.sources,
        &layouts,
        &mut cache,
    );
    assert_eq!(cache.built() as usize, layouts.len());
    for placement in &project.placements {
        let picks: Vec<u64> = model
            .sources
            .iter()
            .filter(|s| s.instance == Some(placement.instance) && s.pad.is_none())
            .map(|s| s.pick_id)
            .collect();
        let mine: Vec<&Geometry> = decals
            .iter()
            .filter(|g| g.item.is_some_and(|i| picks.contains(&i)))
            .collect();
        assert!(
            !mine.is_empty(),
            "instance {} has no decal",
            placement.instance
        );
        let bodies: Vec<&Geometry> = model
            .geometry
            .iter()
            .filter(|g| g.item.is_some_and(|i| picks.contains(&i)) && g.kind.is_envelope())
            .collect();
        let roof = bodies.iter().map(|g| g.max[2]).fold(f64::MIN, f64::max);
        for g in &mine {
            assert_eq!(g.kind, DECAL_KIND);
            assert_eq!(g.volume, 0.0);
            assert!(g.mesh.triangles.len() > 10);
            // On the roof (or the overcoat on it), inside the body's plan.
            assert!(g.min[2] > roof - 0.1 && (g.max[2] - g.min[2]).abs() < 1e-5);
            assert!(bodies.iter().any(|b| {
                g.min[0] >= b.min[0] - 1e-3
                    && g.max[0] <= b.max[0] + 1e-3
                    && g.min[1] >= b.min[1] - 1e-3
                    && g.max[1] <= b.max[1] + 1e-3
            }));
            for n in &g.mesh.normals {
                assert!((n.z - 1.0).abs() < 1e-9);
            }
        }
    }
    // A second pass with one value changed refills only that decal.
    let first = project
        .placements
        .first()
        .map(|p| p.instance)
        .ok_or("no placement")?;
    layouts.insert(first, layout(vec![text("472", 0.3)], Finish::LaserEtch));
    let again = board::board_decals(
        &project,
        &model.geometry,
        &model.sources,
        &layouts,
        &mut cache,
    );
    assert_eq!(cache.built() as usize, layouts.len() + 1);
    assert_eq!(cache.len(), layouts.len());
    assert_eq!(again.len(), decals.len());
    // The native model is untouched: decals are never part of it.
    assert!(model.geometry.iter().all(|g| g.volume > 0.0));
    Ok(())
}

#[test]
fn unmarked_projects_read_no_layouts() -> TestResult {
    let (project, _) = demo()?;
    assert!(!board::any_marked(&project));
    assert!(board::instance_layouts(&project).is_empty());
    Ok(())
}

#[test]
fn a_package_preview_gets_its_decal_on_top() -> TestResult {
    let (project, model) = demo()?;
    let placement = project.placements.first().ok_or("no placement")?;
    let instance = project.instance(placement.instance).ok_or("no instance")?;
    let physical = pcbcraft_core::instances::resolve(&project.library, instance)?
        .physical
        .ok_or("no physical")?;
    let preview = crate::project::preview(&physical.package.definition)?;
    let decal = board::preview_decal(
        &layout(vec![text("A1", 0.3)], Finish::InkWhite),
        &preview.geometry,
        &no_artwork,
    )?;
    assert!(!decal.is_empty());
    let roof = preview
        .geometry
        .iter()
        .filter(|g| g.kind.is_envelope() || g.kind == BodyKind::Overcoat)
        .map(|g| g.max[2])
        .fold(f64::MIN, f64::max);
    for g in &decal {
        assert!(
            (g.min[2] - roof - DECAL_OFFSET_MM).abs() < 1e-5,
            "{} vs {roof}",
            g.min[2]
        );
    }
    let _ = model;
    Ok(())
}

#[test]
fn bottom_faces_are_marked_from_below() -> TestResult {
    use solidcraft_engine::geom::Mesh;
    // A unit cube body from z = −1.6 down to −2.6 (a part under the board).
    let p = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let positions = vec![
        p(0.0, 0.0, -2.6),
        p(2.0, 0.0, -2.6),
        p(2.0, 1.0, -2.6),
        p(0.0, 1.0, -2.6),
    ];
    // The underside, facing −z.
    let mesh = Mesh {
        normals: vec![Vec3::new(0.0, 0.0, -1.0); 4],
        triangles: vec![[0, 2, 1], [0, 3, 2]],
        tri_face: vec![0, 0],
        positions,
        ..Mesh::default()
    };
    let body = Geometry {
        item: Some(9),
        kind: BodyKind::PackageBody,
        layer: None,
        surface: finish_surface(Finish::LaserEtch)?,
        detail: Detail::NONE,
        mesh: Arc::new(mesh),
        volume: 1.0,
        min: [0.0, 0.0, -2.6],
        max: [2.0, 1.0, -1.6],
    };
    let area = board::face_area(&[&body], false, [1.0, 0.0]).ok_or("no face")?;
    assert_eq!(area.normal, [0.0, 0.0, -1.0]);
    assert_eq!(area.size_mm, [2.0, 1.0]);
    assert_eq!(area.origin, [1.0, 0.5, -2.6]);
    // A body has no up-facing flat face at its top here.
    assert!(board::face_area(&[&body], true, [1.0, 0.0]).is_none());
    Ok(())
}
