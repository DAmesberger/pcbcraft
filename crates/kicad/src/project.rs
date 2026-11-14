//! The design rules of a KiCad project file (`.kicad_pro`, JSON) that the
//! board's copper and fills depend on: the default net class's clearance
//! and the board-wide copper-to-edge and copper-to-hole clearances and
//! least track width. Read for facts only; nothing else of the file is
//! used.
use crate::report::{Case, Report};
use sparksmith_core::rules::{CopperObject, RuleKey, RuleValue};

/// Largest project file read, bytes.
pub const MAX_PROJECT: usize = 16 * 1024 * 1024;

/// The rules a project file states, mm.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectRules {
    /// The default net class's clearance.
    pub clearance: Option<f64>,
    pub edge_clearance: Option<f64>,
    pub hole_clearance: Option<f64>,
    pub track_width_min: Option<f64>,
}

fn mm(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}

/// Read a project file; `None` when it is not one (too large, not JSON).
pub fn read(bytes: &[u8], report: &mut Report) -> Option<ProjectRules> {
    if bytes.len() > MAX_PROJECT {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let rules = &json["board"]["design_settings"]["rules"];
    let classes = json["net_settings"]["classes"].as_array();
    let default = classes.and_then(|c| {
        c.iter()
            .find(|c| c["name"].as_str() == Some("Default"))
            .or_else(|| c.first())
    });
    let assigned = |key: &str| match &json["net_settings"][key] {
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
        _ => false,
    };
    if classes.is_some_and(|c| c.len() > 1)
        || assigned("netclass_patterns")
        || assigned("netclass_assignments")
    {
        report.add(Case::NetClasses);
    }
    Some(ProjectRules {
        clearance: default.and_then(|c| mm(&c["clearance"])),
        edge_clearance: mm(&rules["min_copper_edge_clearance"]),
        hole_clearance: mm(&rules["min_hole_clearance"]),
        track_width_min: mm(&rules["min_track_width"]).filter(|w| *w > 0.0),
    })
}

/// The rule overrides the project's rules give.
pub fn overrides(rules: &ProjectRules) -> Vec<(RuleKey, RuleValue)> {
    use CopperObject::*;
    let copper = [Track, Pad, Via, Fill];
    let mut out = Vec::new();
    let mut pair = |a: CopperObject, b: CopperObject, value: Option<f64>| {
        if let Some(v) = value {
            out.push((RuleKey::Clearance { a, b }, RuleValue::Length(v)));
        }
    };
    for (i, a) in copper.iter().enumerate() {
        for b in &copper[i..] {
            pair(*a, *b, rules.clearance);
        }
        pair(*a, BoardEdge, rules.edge_clearance);
        for hole in [ViaHole, PthHole, NpthHole] {
            pair(*a, hole, rules.hole_clearance);
        }
    }
    if let Some(w) = rules.track_width_min {
        out.push((RuleKey::TrackWidthMin, RuleValue::Length(w)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_class_and_board_rules_become_overrides() {
        let json = br#"{"board":{"design_settings":{"rules":{
            "min_copper_edge_clearance":0.5,"min_hole_clearance":0.25,"min_track_width":0.0}}},
            "net_settings":{"classes":[{"name":"Default","clearance":0.2},{"name":"Power","clearance":0.4}],
            "netclass_patterns":[]}}"#;
        let mut report = Report::default();
        let rules = read(json, &mut report);
        assert_eq!(
            rules,
            Some(ProjectRules {
                clearance: Some(0.2),
                edge_clearance: Some(0.5),
                hole_clearance: Some(0.25),
                track_width_min: None,
            })
        );
        assert_eq!(report.count(Case::NetClasses), 1);
        let overrides = rules.map(|r| overrides(&r)).unwrap_or_default();
        // Ten copper pairs, four edge and twelve hole pairs.
        assert_eq!(overrides.len(), 10 + 4 + 12);
        assert!(overrides.contains(&(
            RuleKey::Clearance {
                a: CopperObject::Track,
                b: CopperObject::BoardEdge
            },
            RuleValue::Length(0.5)
        )));
        assert_eq!(read(b"not json", &mut report), None);
    }
}
