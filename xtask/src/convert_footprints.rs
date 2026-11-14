//! The footprint part of the version 1 to 2 converter
//! (`sparksmith_io::convert`), solved with the real evaluator. It lives
//! here because `sparksmith-io` cannot depend on `sparksmith-solidcraft`
//! (the same layer), while the xtask may, as a dev-dependency.
use crate::Result;
use serde_json::{Map, Value, json};
use sparksmith_core::library::FootprintDefinition;
use sparksmith_core::units::Nm;

fn definition(kind: &str, body: &mut Map<String, Value>) -> Result<usize> {
    sparksmith_io::convert::v1::footprint_definition(kind, body).map_err(|e| e.to_string().into())
}

fn object<'a>(o: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Map<String, Value>> {
    o.get_mut(key).and_then(Value::as_object_mut)
}

fn items<'a>(
    o: &'a mut Map<String, Value>,
    key: &str,
) -> impl Iterator<Item = &'a mut Map<String, Value>> {
    o.get_mut(key)
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
}

/// Writes back the pre-nanometre form of a footprint body's pads,
/// datum and defaults (what files held before this conversion), and a
/// stale solve hash as the format change leaves it.
fn unconverted(body: &mut Value) -> Result<()> {
    fn mm(v: &Value) -> Value {
        json!(v.as_f64().unwrap_or(f64::NAN) / 1e6)
    }
    fn deg(v: &Value) -> Value {
        json!(v.as_f64().unwrap_or(f64::NAN) / 1e3)
    }
    fn back(o: &mut Map<String, Value>, from: &str, to: &str, f: fn(&Value) -> Value) {
        if let Some(v) = o.remove(from) {
            o.insert(to.into(), if v.is_null() { v } else { f(&v) });
        }
    }
    fn point_back(o: &mut Map<String, Value>, key: &str) {
        if let Some(p) = o.get(key).cloned().filter(Value::is_object) {
            o.insert(key.into(), json!([mm(&p["x_nm"]), mm(&p["y_nm"])]));
        }
    }
    let o = body.as_object_mut().ok_or("object")?;
    if let Some(d) = object(o, "datum") {
        point_back(d, "offset");
        back(d, "rotation_mdeg", "rotation_deg", deg);
    }
    if let Some(d) = object(o, "defaults") {
        back(d, "mask_expansion_nm", "mask_expansion_mm", mm);
        back(d, "paste_reduction_nm", "paste_reduction_mm", mm);
    }
    for p in items(o, "pads") {
        point_back(p, "position");
        back(p, "rotation_mdeg", "rotation_deg", deg);
        if let Some(s) = object(p, "shape") {
            for tag in ["rect", "round_rect", "oblong"] {
                if let Some(t) = object(s, tag) {
                    point_back(t, "size");
                    back(t, "max_radius_nm", "max_radius", mm);
                }
            }
            if let Some(c) = object(s, "circle") {
                back(c, "diameter_nm", "diameter", mm);
            }
        }
        if let Some(h) = object(p, "hole") {
            point_back(h, "offset");
            if let Some(s) = object(h, "shape") {
                if let Some(r) = object(s, "round") {
                    back(r, "diameter_nm", "diameter", mm);
                }
                if let Some(r) = object(s, "slot") {
                    back(r, "width_nm", "width", mm);
                    back(r, "length_nm", "length", mm);
                    back(r, "angle_mdeg", "angle_deg", deg);
                }
            }
        }
        if let Some(m) = object(p, "mask") {
            back(m, "expansion_nm", "expansion", mm);
        }
        if let Some(m) = object(p, "paste") {
            back(m, "reduction_nm", "reduction", mm);
        }
        for v in items(p, "vias") {
            point_back(v, "at");
            back(v, "drill_nm", "drill", mm);
        }
    }
    if let Some(solve) = object(o, "source").and_then(|s| object(s, "solve")) {
        solve.insert("result_hash".into(), json!("0123456789abcdef"));
    }
    Ok(())
}

/// Sourced footprints solved by the real evaluator (a gull-wing SOIC-8
/// with its pin-1 override and a plated pin header), written back in
/// the old form with a stale hash, convert to exactly what was solved
/// and stay publishable: no `footprint.stale`, no blocking finding.
#[test]
fn a_sourced_footprint_stays_fresh() -> Result<()> {
    use sparksmith_core::ids::PackageTerminalId;
    use sparksmith_core::library::footprint_edit::check_fresh;
    use sparksmith_core::library::templates::footprint_family::{
        self, FootprintTemplate, TemplateKind,
    };
    use sparksmith_core::library::{PackageDefinition, PackageTerminal};
    use sparksmith_solidcraft::footprint::SketchEvaluator;
    let package = |n: u32, body: [f64; 3]| -> Result<PackageDefinition> {
        let mut envelope_nm = [Nm::ZERO; 3];
        for (e, mm) in envelope_nm.iter_mut().zip(body) {
            *e = Nm::from_mm_f64_rounded(mm)?;
        }
        Ok(PackageDefinition {
            envelope_nm,
            terminals: (1..=n)
                .map(|k| PackageTerminal {
                    id: PackageTerminalId(k + 100),
                    designation: k.to_string(),
                })
                .collect(),
            next_local: n + 101,
            ..PackageDefinition::default()
        })
    };
    for (kind, pkg) in [
        (TemplateKind::GullWing, package(8, [4.9, 3.9, 1.5])?),
        (TemplateKind::PinHeader, package(4, [2.54, 10.16, 8.5])?),
    ] {
        let empty = FootprintDefinition {
            next_local: 1,
            ..FootprintDefinition::default()
        };
        let template = FootprintTemplate::defaults(kind, &pkg);
        let (solved, _) = footprint_family::build(&empty, &pkg, &template, &SketchEvaluator)?;
        assert!(solved.source.is_some());
        check_fresh(&solved, &pkg, &SketchEvaluator)?;
        let mut body = serde_json::to_value(&solved)?;
        unconverted(&mut body)?;
        assert!(
            serde_json::from_value::<FootprintDefinition>(body.clone()).is_err(),
            "the old form no longer reads"
        );
        let o = body.as_object_mut().ok_or("object")?;
        assert!(definition("footprint", o)? > 0);
        assert_eq!(
            definition("footprint", o)?,
            0,
            "a second run changes nothing"
        );
        let converted: FootprintDefinition = serde_json::from_value(body)?;
        assert_eq!(converted, solved, "{kind:?}");
        check_fresh(&converted, &pkg, &SketchEvaluator)?;
    }
    Ok(())
}
