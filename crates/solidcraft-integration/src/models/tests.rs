use super::*;
use crate::material::BodyKind;
use crate::project;
use pcbcraft_core::library::{
    ChipForm, ChipKind, GullWingForm, HeaderForm, LeadSides, ModelPlacement, NoLeadForm,
    PackageTerminal, RoleMaterial,
};
use pcbcraft_core::{Engine, Layer, PackageTerminalId};

fn near(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what}: {actual} != {expected}"
    );
}

/// A package with `terminals` terminals, the generated model of `form`.
fn package(form: PackageForm, terminals: u32) -> PackageDefinition {
    PackageDefinition {
        next_local: terminals + 1,
        terminals: (1..=terminals)
            .map(|i| PackageTerminal {
                id: PackageTerminalId(i),
                designation: i.to_string(),
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

fn chip() -> PackageForm {
    PackageForm::Chip(Box::new(ChipForm {
        kind: ChipKind::Resistor,
        length: 1.6,
        width: 0.8,
        height: 0.45,
        termination: 0.3,
    }))
}

fn soic8() -> PackageForm {
    PackageForm::GullWing(Box::new(GullWingForm {
        sides: LeadSides::Two,
        leads_per_side: [4, 0],
        pitch: 1.27,
        body_length: 4.9,
        body_width: 3.9,
        body_height: 1.45,
        standoff: 0.1,
        span: [6.0, 0.0],
        lead_width: 0.41,
        lead_thickness: 0.2,
        foot_length: 0.6,
    }))
}

fn tqfp32() -> PackageForm {
    PackageForm::GullWing(Box::new(GullWingForm {
        sides: LeadSides::Four,
        leads_per_side: [8, 8],
        pitch: 0.8,
        body_length: 7.0,
        body_width: 7.0,
        body_height: 1.0,
        standoff: 0.1,
        span: [9.0, 9.0],
        lead_width: 0.37,
        lead_thickness: 0.15,
        foot_length: 0.6,
    }))
}

fn qfn16() -> PackageForm {
    PackageForm::NoLead(Box::new(NoLeadForm {
        sides: LeadSides::Four,
        terminals_per_side: [4, 4],
        pitch: 0.5,
        body_length: 3.0,
        body_width: 3.0,
        body_height: 0.85,
        terminal_length: 0.4,
        terminal_width: 0.25,
        exposed_pad: Some([1.7, 1.7]),
    }))
}

fn dfn8() -> PackageForm {
    PackageForm::NoLead(Box::new(NoLeadForm {
        sides: LeadSides::Two,
        terminals_per_side: [4, 0],
        pitch: 0.5,
        body_length: 2.0,
        body_width: 2.0,
        body_height: 0.75,
        terminal_length: 0.35,
        terminal_width: 0.25,
        exposed_pad: None,
    }))
}

fn header2x5() -> PackageForm {
    PackageForm::Header(Box::new(HeaderForm {
        rows: 2,
        pins_per_row: 5,
        pitch: 2.54,
        row_pitch: 2.54,
        body_height: 2.5,
        pin_side: 0.64,
        pin_above: 6.0,
        pin_below: 3.0,
    }))
}

/// A built definition and the native design holding it.
fn built(form: PackageForm, terminals: u32) -> Result<(Native, ModelDefinition)> {
    let mut native = Native::new("models")?;
    let definition = build_definition(&mut native, "pkgt", &package(form, terminals))?;
    Ok((native, definition))
}

/// Box of a definition body: (min, max).
fn bounds(native: &Native, name: &str) -> Result<([f64; 3], [f64; 3])> {
    let state = native.session().model.state();
    let body = state
        .body(name)
        .ok_or_else(|| Error::Verification(format!("no body {name}")))?;
    let m = body
        .measure()
        .map_err(|e| Error::Verification(e.to_string()))?;
    Ok((
        [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
        [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
    ))
}

fn volume(native: &Native, name: &str) -> Result<f64> {
    let state = native.session().model.state();
    let body = state
        .body(name)
        .ok_or_else(|| Error::Verification(format!("no body {name}")))?;
    Ok(body
        .measure()
        .map_err(|e| Error::Verification(e.to_string()))?
        .volume)
}

/// Lead bodies in terminal order.
fn leads(definition: &ModelDefinition) -> Vec<&GeneratedBody> {
    let mut out: Vec<&GeneratedBody> = definition
        .bodies
        .iter()
        .filter(|b| b.terminal.is_some())
        .collect();
    out.sort_by_key(|b| b.terminal);
    out
}

fn centre(native: &Native, name: &str) -> Result<[f64; 3]> {
    let (lo, hi) = bounds(native, name)?;
    Ok(std::array::from_fn(|i| (lo[i] + hi[i]) / 2.0))
}

/// Every body pair is disjoint: the definition's bodies' volumes add up to
/// the volume of their union.
fn disjoint(native: &mut Native, definition: &ModelDefinition) -> Result<()> {
    let total: f64 = definition
        .bodies
        .iter()
        .map(|b| volume(native, &b.name))
        .sum::<Result<f64>>()?;
    let names: Vec<&str> = definition.bodies.iter().map(|b| b.name.as_str()).collect();
    let Some((first, rest)) = names.split_first() else {
        return Err(Error::Verification("no bodies".into()));
    };
    native.call(
        "solid.combine",
        serde_json::json!({"target": first, "tools": rest, "operation": "join",
            "keep_tools": true, "new_component": false}),
    )?;
    let union = volume(native, first)?;
    assert!(
        (union - total).abs() < 1e-6 * total.max(1.0),
        "bodies overlap: {total} mm³ apart, {union} mm³ joined"
    );
    Ok(())
}

#[test]
fn a_chip_resistor_is_alumina_with_an_overcoat_between_tin_terminations() -> Result<()> {
    let (mut native, definition) = built(chip(), 2)?;
    let role = |r: BodyRole| definition.bodies.iter().filter(move |b| b.role == r);
    assert_eq!(role(BodyRole::Body).count(), 1);
    assert_eq!(role(BodyRole::Termination).count(), 2);
    let material = |name: &str| {
        definition
            .bodies
            .iter()
            .find(|b| b.name == name)
            .map(|b| b.material)
    };
    assert_eq!(material("pkgt body"), Some(MaterialId::Alumina96));
    assert_eq!(
        material("pkgt overcoat"),
        Some(MaterialId::ResistorOvercoat)
    );
    assert_eq!(material("pkgt termination 1"), Some(MaterialId::TinMatte));
    let (lo, hi) = bounds(&native, "pkgt termination 1")?;
    near(lo[0], -0.8, "termination 1 outer end");
    near(hi[0], -0.5, "termination 1 inner end");
    near(lo[2], 0.0, "seated on z = 0");
    near(hi[2], 0.45, "full height");
    let (_, top) = bounds(&native, "pkgt overcoat")?;
    near(top[2], 0.45, "overcoat at the top");
    assert_eq!(
        leads(&definition)
            .iter()
            .map(|b| b.name.as_str())
            .collect::<Vec<_>>(),
        ["pkgt termination 1", "pkgt termination 2"]
    );
    disjoint(&mut native, &definition)
}

#[test]
fn soic_leads_are_numbered_counter_clockwise_and_stand_on_the_seating_plane() -> Result<()> {
    let (mut native, definition) = built(soic8(), 8)?;
    assert_eq!(definition.bodies.len(), 9);
    let leads = leads(&definition);
    assert_eq!(leads.len(), 8);
    let c: Vec<[f64; 3]> = leads
        .iter()
        .map(|b| centre(&native, &b.name))
        .collect::<Result<_>>()?;
    // 1–4 along −y in +x, 5–8 along +y in −x.
    for (k, p) in c.iter().enumerate() {
        assert_eq!(p[1] < 0.0, k < 4, "lead {} side", k + 1);
    }
    near(c[0][0], -1.5 * 1.27, "lead 1 x");
    near(c[1][0] - c[0][0], 1.27, "pitch");
    near(c[3][0], 1.5 * 1.27, "lead 4 x");
    near(c[4][0], 1.5 * 1.27, "lead 5 x");
    near(c[7][0], -1.5 * 1.27, "lead 8 x");
    for lead in &leads {
        let (lo, hi) = bounds(&native, &lead.name)?;
        near(lo[2], 0.0, "foot on the seating plane");
        assert_eq!(lead.material, MaterialId::TinMatte);
        let tip = if hi[1] > 0.0 { hi[1] } else { -lo[1] };
        near(tip, 3.0, "tip at half the span");
        near(hi[0] - lo[0], 0.41, "lead width");
        // The root reaches into the moulding above the standoff.
        let inner = if hi[1] > 0.0 { lo[1] } else { -hi[1] };
        assert!(inner < 1.95, "root inside the body: {inner}");
    }
    let (lo, hi) = bounds(&native, "pkgt body")?;
    near(lo[2], 0.1, "standoff");
    near(hi[2], 1.55, "body top");
    // Cut around the lead roots and the pin-1 dimple.
    assert!(volume(&native, "pkgt body")? < 4.9 * 3.9 * 1.45 - 1e-3);
    // Terminals bind in the package's order.
    assert_eq!(
        leads.iter().map(|b| b.terminal).collect::<Vec<_>>(),
        (1..=8).map(Some).collect::<Vec<_>>()
    );
    disjoint(&mut native, &definition)
}

#[test]
fn qfp_leads_run_round_all_four_sides() -> Result<()> {
    let (native, definition) = built(tqfp32(), 32)?;
    let leads = leads(&definition);
    assert_eq!(leads.len(), 32);
    let c = |k: usize| centre(&native, &leads[k - 1].name);
    let [x1, y1, _] = c(1)?;
    assert!(x1 < 0.0 && y1 < -3.5, "lead 1 at −x on the −y row");
    let [x9, y9, _] = c(9)?;
    assert!(x9 > 3.5 && y9 < 0.0, "lead 9 starts the +x row at −y");
    let [x17, y17, _] = c(17)?;
    assert!(x17 > 0.0 && y17 > 3.5, "lead 17 starts the +y row at +x");
    let [x25, y25, _] = c(25)?;
    assert!(x25 < -3.5 && y25 > 0.0, "lead 25 starts the −x row at +y");
    near(c(10)?[1] - y9, 0.8, "pitch on the +x row");
    for lead in &leads {
        let (lo, hi) = bounds(&native, &lead.name)?;
        near(lo[2], 0.0, "foot on the seating plane");
        let reach = [lo[0].abs(), hi[0].abs(), lo[1].abs(), hi[1].abs()];
        near(
            reach.iter().copied().fold(0.0, f64::max),
            4.5,
            "tip at half the span",
        );
    }
    Ok(())
}

#[test]
fn qfn_terminals_and_exposed_pad_are_flush_with_the_underside() -> Result<()> {
    let (mut native, definition) = built(qfn16(), 17)?;
    let leads = leads(&definition);
    assert_eq!(leads.len(), 17);
    let pad = leads[16];
    assert_eq!(pad.role, BodyRole::ExposedPad);
    assert_eq!(pad.terminal, Some(17));
    let (lo, hi) = bounds(&native, &pad.name)?;
    near(hi[0] - lo[0], 1.7, "pad length");
    for lead in &leads {
        let (lo, hi) = bounds(&native, &lead.name)?;
        near(lo[2], 0.0, "flush with the underside");
        near(hi[2], 0.2, "leadframe thickness");
    }
    // Terminals reach the body's side faces (wettable flanks).
    let (lo, _) = bounds(&native, &leads[0].name)?;
    near(lo[1], -1.5, "lead 1 at the −y face");
    let (_, hi) = bounds(&native, &leads[4].name)?;
    near(hi[0], 1.5, "lead 5 at the +x face");
    disjoint(&mut native, &definition)?;
    let (_, definition) = built(dfn8(), 8)?;
    assert_eq!(leads_count(&definition), 8);
    Ok(())
}

fn leads_count(definition: &ModelDefinition) -> usize {
    definition
        .bodies
        .iter()
        .filter(|b| b.role == BodyRole::Termination)
        .count()
}

/// The builders and the core's placement fit share one lead order: lead
/// `k`'s body stands over `PackageForm::lead_points()[k]` in every family.
#[test]
fn every_lead_body_stands_over_the_core_s_lead_point() -> Result<()> {
    for (form, terminals) in [
        (chip(), 2),
        (soic8(), 8),
        (tqfp32(), 32),
        (qfn16(), 17),
        (dfn8(), 8),
        (header2x5(), 10),
    ] {
        let points = form.lead_points();
        let (native, definition) = built(form, terminals)?;
        let leads = leads(&definition);
        assert_eq!(leads.len(), points.len());
        for (lead, point) in leads.iter().zip(&points) {
            let (lo, hi) = bounds(&native, &lead.name)?;
            for axis in 0..2 {
                assert!(
                    lo[axis] - 1e-6 <= point[axis] && point[axis] <= hi[axis] + 1e-6,
                    "{}: lead point {point:?} outside {lo:?}..{hi:?}",
                    lead.name
                );
            }
            assert!(lo[2] <= 1e-6, "{} reaches the seating plane", lead.name);
        }
    }
    Ok(())
}

#[test]
fn header_pins_are_gold_through_a_pbt_base_numbered_across_rows() -> Result<()> {
    let (mut native, definition) = built(header2x5(), 10)?;
    let leads = leads(&definition);
    assert_eq!(leads.len(), 10);
    let c = |k: usize| centre(&native, &leads[k - 1].name);
    let p1 = c(1)?;
    let p2 = c(2)?;
    let p3 = c(3)?;
    near(p1[0], -2.0 * 2.54, "pin 1 x");
    near(p1[1], -1.27, "pin 1 row");
    near(p2[0], p1[0], "pin 2 beside pin 1");
    near(p2[1], 1.27, "pin 2 row");
    near(p3[0] - p1[0], 2.54, "pin 3 one pitch on");
    for lead in &leads {
        assert_eq!(lead.material, MaterialId::Gold);
        let (lo, hi) = bounds(&native, &lead.name)?;
        near(lo[2], -3.0, "pin below the seating plane");
        near(hi[2], 8.5, "pin above the base");
    }
    let body = definition
        .bodies
        .iter()
        .find(|b| b.role == BodyRole::Body)
        .ok_or_else(|| Error::Verification("no base".into()))?;
    assert_eq!(body.material, MaterialId::Pbt);
    disjoint(&mut native, &definition)
}

#[test]
fn role_materials_override_the_family_defaults() -> Result<()> {
    let mut p = package(header2x5(), 10);
    p.model = PackageModel::Generated {
        family_rev: 1,
        materials: GeneratedMaterials {
            roles: vec![RoleMaterial {
                role: BodyRole::Body,
                material: MaterialId::Lcp,
            }],
        },
    };
    let mut native = Native::new("models")?;
    let definition = build_definition(&mut native, "pkgt", &p)?;
    assert!(
        definition
            .bodies
            .iter()
            .any(|b| b.role == BodyRole::Body && b.material == MaterialId::Lcp)
    );
    Ok(())
}

#[test]
fn named_parameters_drive_the_pitch_and_body() -> Result<()> {
    let (mut native, definition) = built(soic8(), 8)?;
    native.change_parameters(
        serde_json::json!({"pkgt_pitch": "1.0 mm", "pkgt_body_height": "1.2 mm"}),
    )?;
    let leads = leads(&definition);
    let a = centre(&native, &leads[0].name)?;
    let b = centre(&native, &leads[1].name)?;
    near(b[0] - a[0], 1.0, "pitch follows its parameter");
    near(a[0], -1.5, "the row stays centred");
    let (_, hi) = bounds(&native, "pkgt body")?;
    near(hi[2], 1.3, "body height follows its parameter");
    Ok(())
}

#[test]
fn the_terminal_binding_is_never_guessed() -> Result<()> {
    // Seven terminals for eight leads: no lead claims a terminal.
    let (_, definition) = built(soic8(), 7)?;
    assert!(definition.bodies.iter().all(|b| b.terminal.is_none()));
    Ok(())
}

#[test]
fn unbuildable_requests_are_refused() -> Result<()> {
    let mut native = Native::new("models")?;
    let mut envelope = package(chip(), 2);
    envelope.model = PackageModel::default();
    let refused = |r: Result<ModelDefinition>, code: &str| -> Result<()> {
        match r {
            Err(e) if e.to_string().contains(code) => Ok(()),
            Err(e) => Err(Error::Verification(format!("{code} expected, got {e}"))),
            Ok(_) => Err(Error::Verification(format!("{code}: it was built"))),
        }
    };
    refused(
        build_definition(&mut native, "pkgt", &envelope),
        "model.unsupported",
    )?;
    refused(
        build_definition(&mut native, "pkgt", &package(PackageForm::Generic, 2)),
        "model.unsupported",
    )?;
    let mut stale = package(chip(), 2);
    stale.model = PackageModel::Generated {
        family_rev: 9,
        materials: GeneratedMaterials::default(),
    };
    refused(
        build_definition(&mut native, "pkgt", &stale),
        "model.family_rev",
    )?;
    refused(
        build_definition(&mut native, "bad name", &package(chip(), 2)),
        "model.name",
    )?;
    let PackageForm::GullWing(mut g) = soic8() else {
        return Err(Error::Verification("not gull-wing".into()));
    };
    g.span = [4.0, 0.0];
    refused(
        build_definition(&mut native, "pkgt", &package(PackageForm::GullWing(g), 8)),
        "model.form",
    )?;
    let PackageForm::NoLead(mut n) = qfn16() else {
        return Err(Error::Verification("not no-lead".into()));
    };
    n.exposed_pad = Some([2.5, 2.5]);
    refused(
        build_definition(&mut native, "pkgt", &package(PackageForm::NoLead(n), 17)),
        "model.form",
    )?;
    Ok(())
}

#[test]
fn metadata_chunks_round_trip_and_bound_their_size() -> Result<()> {
    let (mut native, definition) = built(tqfp32(), 32)?;
    record(&mut native, std::slice::from_ref(&definition))?;
    let back = recorded(&native)?;
    assert_eq!(back.len(), definition.bodies.len());
    for body in &definition.bodies {
        assert_eq!(back.get(&body.name), Some(body));
    }
    // Many bodies split into chunks below the limit; too many are refused.
    let big = ModelDefinition {
        component: "pkgbig".into(),
        bodies: (0..4000)
            .map(|i| GeneratedBody {
                name: format!("pkgbig lead ({i})"),
                role: BodyRole::Lead,
                material: MaterialId::TinMatte,
                terminal: Some(i),
            })
            .collect(),
    };
    let split = chunks(std::slice::from_ref(&big))?;
    assert!(split.len() > 1);
    for chunk in &split {
        let stored = serde_json::to_string(&serde_json::json!({ "bodies": chunk }))?;
        assert!(stored.len() <= CHUNK_BYTES);
    }
    let huge = vec![big; 60];
    assert!(chunks(&huge).is_err());
    Ok(())
}

#[test]
fn the_saved_model_reopens_parametric_with_materials_in_plain_solidcraft() -> Result<()> {
    let model = project::preview(&package(soic8(), 8))?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("soic.solidcraft")
        .to_string_lossy()
        .into_owned();
    model.save(&path)?;
    let mut reopened = Native::open(&path)?;
    let bodies = recorded(&reopened)?;
    assert_eq!(bodies.len(), 9);
    let doc = &reopened.session().doc;
    for body in &reopened.session().world_state().bodies {
        let look = doc
            .model_body_look(body)
            .and_then(|l| l.opaque_surface().ok().flatten())
            .ok_or_else(|| Error::Verification(format!("{} has no look", body.name)))?;
        let recorded = bodies
            .get(body.definition_name())
            .ok_or_else(|| Error::Verification("unrecorded body".into()))?;
        let metal =
            recorded.material.category() == pcbcraft_core::materials::MaterialCategory::Metal;
        assert_eq!(look.metallic() > 0.5, metal, "{}", body.name);
    }
    reopened.change_parameters(serde_json::json!({"pkgdraft_pitch": "1.1 mm"}))?;
    assert!(reopened.evaluation_error().is_none());
    Ok(())
}

#[test]
fn the_preview_draws_each_body_with_its_material_s_product_look() -> Result<()> {
    for (form, terminals) in [
        (chip(), 2),
        (soic8(), 8),
        (tqfp32(), 32),
        (qfn16(), 17),
        (dfn8(), 8),
        (header2x5(), 10),
    ] {
        let model = project::preview(&package(form, terminals))?;
        assert_eq!(model.models.len(), 1);
        let bodies = &model.models[0].bodies;
        assert_eq!(model.geometry.len(), bodies.len());
        for g in &model.geometry {
            assert!(g.kind.is_generated());
            assert!(g.item.is_none());
        }
        assert!(model.geometry.iter().any(|g| g.kind == BodyKind::ModelLead));
        assert!(model.geometry.iter().any(|g| g.kind == BodyKind::ModelBody));
    }
    Ok(())
}

#[test]
fn the_preview_applies_the_placement() -> Result<()> {
    let mut p = package(chip(), 2);
    p.placement = ModelPlacement {
        translation: [1.0, 2.0, 0.5],
        rotation_deg: [0.0, 0.0, 90.0],
    };
    let model = project::preview(&p)?;
    let min = model
        .geometry
        .iter()
        .map(|g| g.min)
        .fold([f64::MAX; 3], |a, b| {
            std::array::from_fn(|i| a[i].min(b[i]))
        });
    let max = model
        .geometry
        .iter()
        .map(|g| g.max)
        .fold([f64::MIN; 3], |a, b| {
            std::array::from_fn(|i| a[i].max(b[i]))
        });
    // Turned a quarter about z: the length runs along y.
    near(min[0], 0.6, "x min");
    near(max[0], 1.4, "x max");
    near(min[1], 1.2, "y min");
    near(max[1], 2.8, "y max");
    near(min[2], 0.5, "lifted");
    Ok(())
}

/// The demo board with its chip package given a generated model.
fn generated_demo() -> Result<pcbcraft_core::Project> {
    let mut project = Engine::demo()
        .map_err(|e| Error::Verification(e.to_string()))?
        .project()
        .clone();
    let package = &mut project.library.packages[0].definition;
    package.form = PackageForm::Chip(Box::new(ChipForm {
        kind: ChipKind::Resistor,
        length: 2.0,
        width: 1.2,
        height: 0.6,
        termination: 0.4,
    }));
    package.model = PackageModel::Generated {
        family_rev: 1,
        materials: GeneratedMaterials::default(),
    };
    if let Some(bottom) = project.placements.get_mut(1) {
        bottom.layer = Layer::Bottom;
    }
    Ok(project)
}

#[test]
fn every_instance_shares_one_definition_and_keeps_its_own_identity() -> Result<()> {
    let project = generated_demo()?;
    let model = project::build(&project)?;
    assert_eq!(model.models.len(), 1, "one definition per package revision");
    let definition = &model.models[0];
    let shared: Vec<&project::Source> = model
        .sources
        .iter()
        .filter(|s| s.occurrence.is_some())
        .collect();
    assert_eq!(shared.len(), project.placements.len());
    let component = shared[0].component;
    assert!(shared.iter().all(|s| s.component == component));
    let mut occurrences: Vec<u64> = shared.iter().filter_map(|s| s.occurrence).collect();
    occurrences.dedup();
    assert_eq!(
        occurrences.len(),
        shared.len(),
        "one occurrence per instance"
    );
    // The native file holds the definition's bodies once.
    let state = model.native().session().model.state();
    let defined = state
        .bodies
        .iter()
        .filter(|b| b.component == component)
        .count();
    assert_eq!(defined, definition.bodies.len());
    for source in &shared {
        let bodies: Vec<_> = model
            .geometry
            .iter()
            // Its model's bodies; display-only solder fillets share the pick.
            .filter(|g| g.item == Some(source.pick_id) && g.kind.is_generated())
            .collect();
        assert_eq!(bodies.len(), definition.bodies.len(), "{}", source.label);
        let low = bodies.iter().map(|g| g.min[2]).fold(f64::MAX, f64::min);
        let high = bodies.iter().map(|g| g.max[2]).fold(f64::MIN, f64::max);
        match source.layer {
            // Seated on the land copper on top of the 1.6 mm board.
            Some(pcbcraft_core::CopperLayer::Top) => near(low, 1.635, "top seating plane"),
            _ => near(high, -0.035, "bottom seating plane"),
        }
    }
    Ok(())
}

#[test]
fn generated_parts_land_where_their_footprints_are() -> Result<()> {
    let project = generated_demo()?;
    let model = project::build(&project)?;
    for source in model.sources.iter().filter(|s| s.occurrence.is_some()) {
        let instance = source
            .instance
            .ok_or_else(|| Error::Verification("no instance".into()))?;
        let pads: Vec<_> = model
            .geometry
            .iter()
            .filter(|g| g.kind == BodyKind::PadCopper)
            .filter(|g| {
                model
                    .sources
                    .iter()
                    .any(|s| Some(s.pick_id) == g.item && s.instance == Some(instance))
            })
            .collect();
        let ends: Vec<_> = model
            .geometry
            .iter()
            .filter(|g| g.item == Some(source.pick_id) && g.kind == BodyKind::ModelLead)
            .collect();
        assert_eq!(ends.len(), 2);
        // Each termination stands over one of its instance's pads.
        for end in ends {
            let c = [
                (end.min[0] + end.max[0]) / 2.0,
                (end.min[1] + end.max[1]) / 2.0,
            ];
            assert!(
                pads.iter().any(|p| (p.min[0]..=p.max[0]).contains(&c[0])
                    && (p.min[1]..=p.max[1]).contains(&c[1])),
                "{} termination at {c:?} misses its pads",
                source.label
            );
        }
    }
    Ok(())
}

#[test]
fn every_form_the_core_accepts_is_built() -> Result<()> {
    // Gull-wing variants near the builders' limits: lead thickness against
    // the body height and the bend room outside the body. The core's form
    // rules are the builders' rules, so a form without range problems
    // builds and one with problems is refused before building.
    let PackageForm::GullWing(base) = soic8() else {
        return Err(Error::Verification("not gull-wing".into()));
    };
    let mut cases = Vec::new();
    for (span, foot, thickness, standoff) in [
        (6.0, 0.6, 0.2, 0.1),
        (6.0, 0.8, 0.2, 0.1),
        (6.0, 0.9, 0.2, 0.1),
        (5.0, 0.3, 0.15, 0.0),
        (7.0, 0.6, 0.6, 0.3),
        (7.0, 0.6, 0.6, 0.0),
        (8.0, 0.6, 1.4, 0.1),
    ] {
        let mut g = base.clone();
        g.span = [span, 0.0];
        g.foot_length = foot;
        g.lead_thickness = thickness;
        g.standoff = standoff;
        cases.push(PackageForm::GullWing(g));
    }
    let PackageForm::Header(mut h) = header2x5() else {
        return Err(Error::Verification("not a header".into()));
    };
    h.pin_above = 0.0;
    cases.push(PackageForm::Header(h));
    let (mut accepted, mut refused) = (0, 0);
    for form in cases {
        let problems = form.range_problems();
        let terminals = u32::try_from(form.lead_count()).unwrap_or(0);
        let mut native = Native::new("models")?;
        let built = build_definition(&mut native, "pkgt", &package(form, terminals));
        if problems.is_empty() {
            built?;
            accepted += 1;
        } else {
            match built {
                Err(e) if e.to_string().contains("model.form") => refused += 1,
                other => {
                    return Err(Error::Verification(format!(
                        "{problems:?} but the builder gave {:?}",
                        other.map(|d| d.bodies.len())
                    )));
                }
            }
        }
    }
    assert_eq!((accepted, refused), (4, 4), "built and refused");
    Ok(())
}
