//! A body's physical look as a material graph, for Solidcraft's graph renderer:
//! `standard_surface` with the look's base, metal and coat, and the procedural
//! detail (`material::Pattern`) as pattern nodes. The patterns follow Solidcraft's
//! shipped woven-laminate, orange-peel and printed-ink graphs (MIT; see
//! ATTRIBUTION.md). Display only: never saved.
//!
//! Every value is a uniform of the graph, so all looks of one pattern share a
//! compiled shape.
use crate::material::{Detail, Pattern};
use crate::{Error, Result};
use serde_json::{Value, json};
use solidcraft_engine::render::material_graph::{
    Ir, MaterialGraph, SurfaceSample, base_sample, lower, uniform_words, validate,
};
use solidcraft_engine::render::surface::OpaqueSurface;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A look lowered for the renderer: its shape, uniform words and base response.
#[derive(Clone, Debug)]
pub struct LoweredLook {
    pub ir: Arc<Ir>,
    pub words: Vec<u32>,
    pub base: SurfaceSample,
}

/// A look's identity as exact values: equal keys draw the same.
pub type LookKey = (u8, [u64; 11]);

/// The identity of `surface` with `detail`.
pub fn look_key(surface: OpaqueSurface, detail: Detail) -> LookKey {
    let b = surface.base_color();
    let (cw, cr, ci) = surface
        .clear_coat()
        .map_or((0.0, 1.0, 1.0), |c| (c.weight(), c.roughness(), c.ior()));
    (
        detail.pattern as u8,
        [
            b.x,
            b.y,
            b.z,
            surface.metallic(),
            surface.roughness(),
            surface.dielectric_ior(),
            cw,
            cr,
            ci,
            detail.strength,
            0.0,
        ]
        .map(f64::to_bits),
    )
}

/// The inputs of the surface node for `surface`; `base` is the base colour's
/// source, the constant colour when there is no pattern on it.
fn surface_node(surface: OpaqueSurface, base: Value, extra: &[(&str, Value)]) -> Value {
    let (cw, cr, ci) = surface
        .clear_coat()
        .map_or((0.0, 1.0, 1.0), |c| (c.weight(), c.roughness(), c.ior()));
    let mut inputs = json!({
        "base_color": base,
        "base_metalness": surface.metallic(),
        "specular_roughness": surface.roughness(),
        "specular_ior": surface.dielectric_ior(),
        "coat_weight": cw,
        "coat_roughness": cr,
        "coat_ior": ci,
    });
    for (name, value) in extra {
        inputs[*name] = value.clone();
    }
    json!({"id": "surface", "kind": "standard_surface", "inputs": inputs})
}

fn base_constant(surface: OpaqueSurface) -> Value {
    let b = surface.base_color();
    json!([b.x, b.y, b.z])
}

/// The weave on a triplanar projection, and its albedo tint `swing` × `strength`
/// (a quarter of it on a routed edge), ending at node `base`.
fn weave_nodes(surface: OpaqueSurface, swing: f64, strength: f64) -> Vec<Value> {
    let b = base_constant(surface);
    vec![
        json!({"id": "uv", "kind": "texcoord.triplanar", "params": {"space": "object"},
               "inputs": {"scale_mm": [1, 1, 0.173]}}),
        json!({"id": "weave", "kind": "pattern.weave", "params": {"kind": "plain"},
               "inputs": {"texcoord": {"from": "uv"}, "pitch_mm": [0.577, 0.819],
                          "fibres": 7, "fibre_contrast": 0.3}}),
        json!({"id": "n", "kind": "geometry.normal", "params": {"space": "object"}}),
        json!({"id": "n_parts", "kind": "separate3", "inputs": {"in": {"from": "n"}}}),
        json!({"id": "n_z", "kind": "abs", "inputs": {"in": {"from": "n_parts.z"}}}),
        json!({"id": "face", "kind": "smoothstep",
               "inputs": {"edge0": 0.65, "edge1": 0.75, "in": {"from": "n_z"}}}),
        json!({"id": "edge_k", "kind": "mix", "inputs": {"a": 0.25, "b": 1, "t": {"from": "face"}}}),
        json!({"id": "k", "kind": "multiply", "inputs": {"a": {"from": "edge_k"}, "b": strength}}),
        json!({"id": "swing", "kind": "multiply", "inputs": {"a": {"from": "k"}, "b": swing}}),
        json!({"id": "tint_lo", "kind": "subtract", "inputs": {"a": 1, "b": {"from": "swing"}}}),
        json!({"id": "tint_hi", "kind": "add", "inputs": {"a": 1, "b": {"from": "swing"}}}),
        json!({"id": "tint", "kind": "remap", "inputs": {"in": {"from": "weave"},
               "out_min": {"from": "tint_lo", "fallback": 1},
               "out_max": {"from": "tint_hi", "fallback": 1}}}),
        json!({"id": "base", "kind": "multiply",
               "inputs": {"a": b, "b": {"from": "tint", "fallback": 1}}}),
    ]
}

/// Orange peel on a 2D projection `uv`, ending at normal node `coat_n`.
fn peel_nodes(strength: f64) -> Vec<Value> {
    vec![
        json!({"id": "peel_a", "kind": "pattern.noise",
               "params": {"kind": "value", "dimensions": 2, "octaves": 1},
               "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 2.4, "seed": 1}}),
        json!({"id": "peel_b", "kind": "pattern.noise",
               "params": {"kind": "value", "dimensions": 2, "octaves": 1},
               "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 0.9, "seed": 2}}),
        json!({"id": "peel_b6", "kind": "multiply", "inputs": {"a": {"from": "peel_b"}, "b": 0.1667}}),
        json!({"id": "peel_ab", "kind": "add", "inputs": {"a": {"from": "peel_a"}, "b": {"from": "peel_b6"}}}),
        json!({"id": "peel", "kind": "multiply", "inputs": {"a": {"from": "peel_ab"}, "b": strength}}),
        json!({"id": "coat_n", "kind": "normal.from_height",
               "inputs": {"height": {"from": "peel"}, "relief_mm": 0.006}}),
    ]
}

/// The look as a graph in Solidcraft's JSON form.
pub fn look_graph_json(surface: OpaqueSurface, detail: Detail) -> Value {
    let strength = if detail.strength.is_finite() {
        detail.strength.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let from_base = json!({"from": "base", "fallback": base_constant(surface)});
    let mut nodes: Vec<Value> = Vec::new();
    let surface_json = match detail.pattern {
        Pattern::None => surface_node(surface, base_constant(surface), &[]),
        Pattern::Weave => {
            nodes.extend(weave_nodes(surface, 0.2, strength));
            nodes.push(json!({"id": "relief_h", "kind": "multiply",
                              "inputs": {"a": {"from": "weave"}, "b": {"from": "k"}}}));
            nodes.push(json!({"id": "relief", "kind": "normal.from_height",
                              "inputs": {"height": {"from": "relief_h"}, "relief_mm": 0.006}}));
            surface_node(
                surface,
                from_base,
                &[("geometry_normal", json!({"from": "relief"}))],
            )
        }
        Pattern::MaskPeel => {
            nodes.push(
                json!({"id": "uv", "kind": "texcoord.triplanar", "params": {"space": "object"},
                              "inputs": {"scale_mm": [1, 1, 1]}}),
            );
            nodes.extend(peel_nodes(strength));
            surface_node(
                surface,
                base_constant(surface),
                &[("geometry_coat_normal", json!({"from": "coat_n"}))],
            )
        }
        // The strength is the weave's show-through; the peel stays at full.
        Pattern::MaskPeelOverWeave => {
            nodes.extend(weave_nodes(surface, 0.2, strength));
            nodes.extend(peel_nodes(1.0));
            surface_node(
                surface,
                from_base,
                &[("geometry_coat_normal", json!({"from": "coat_n"}))],
            )
        }
        Pattern::Silkscreen => {
            let b = base_constant(surface);
            nodes.extend([
                json!({"id": "uv", "kind": "texcoord.triplanar", "params": {"space": "object"},
                       "inputs": {"scale_mm": [1, 1, 1]}}),
                json!({"id": "fine", "kind": "pattern.noise",
                       "params": {"kind": "value", "dimensions": 2, "octaves": 1},
                       "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 0.05, "seed": 3}}),
                json!({"id": "coarse", "kind": "pattern.noise",
                       "params": {"kind": "value", "dimensions": 2, "octaves": 1},
                       "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 0.16, "seed": 4}}),
                json!({"id": "grain", "kind": "add", "inputs": {"a": {"from": "fine"}, "b": {"from": "coarse"}}}),
                json!({"id": "swing", "kind": "multiply", "inputs": {"a": strength, "b": 0.36}}),
                json!({"id": "tint_lo", "kind": "subtract", "inputs": {"a": 1, "b": {"from": "swing"}}}),
                json!({"id": "tint_hi", "kind": "add", "inputs": {"a": 1, "b": {"from": "swing"}}}),
                json!({"id": "tint", "kind": "remap", "inputs": {"in": {"from": "grain"},
                       "in_min": 0, "in_max": 2,
                       "out_min": {"from": "tint_lo", "fallback": 1},
                       "out_max": {"from": "tint_hi", "fallback": 1}}}),
                json!({"id": "base", "kind": "multiply",
                       "inputs": {"a": b, "b": {"from": "tint", "fallback": 1}}}),
                json!({"id": "relief_h", "kind": "multiply", "inputs": {"a": {"from": "grain"}, "b": strength}}),
                json!({"id": "relief", "kind": "normal.from_height",
                       "inputs": {"height": {"from": "relief_h"}, "relief_mm": 0.002}}),
            ]);
            surface_node(
                surface,
                from_base,
                &[("geometry_normal", json!({"from": "relief"}))],
            )
        }
    };
    nodes.push(surface_json);
    json!({"name": "pcbcraft look", "nodes": nodes, "output": "surface"})
}

/// The look lowered for the renderer. The graphs are this module's own, so a
/// refusal is a bug here, reported as an error rather than drawn plain.
pub fn lower_look(surface: OpaqueSurface, detail: Detail) -> Result<LoweredLook> {
    let invalid = |e: String| Error::Invalid(format!("look graph: {e}"));
    let graph: MaterialGraph = serde_json::from_value(look_graph_json(surface, detail))?;
    let valid = validate(&graph).map_err(|e| invalid(e.to_string()))?;
    let ir = lower(&valid).map_err(|e| invalid(e.to_string()))?;
    let base = base_sample(&valid).map_err(|e| invalid(e.to_string()))?;
    let words = uniform_words(&ir, &BTreeMap::new()).map_err(|e| invalid(e.to_string()))?;
    Ok(LoweredLook {
        ir: Arc::new(ir),
        words,
        base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{BodyKind, Pattern, detail, material};
    use pcbcraft_core::Fabrication;
    use std::collections::HashSet;

    #[test]
    fn every_body_kind_s_look_lowers_and_keeps_its_base_response() -> Result<()> {
        let fabrication = Fabrication::default();
        for kind in BodyKind::ALL {
            let look = material(kind, &fabrication, &crate::stack::Stack::default()).surface()?;
            let d = detail(kind, &fabrication);
            let lowered = lower_look(look, d)?;
            let got = lowered.base.opaque;
            // The base response is what the renderer falls back on: the look itself.
            assert_eq!(got.metallic(), look.metallic(), "{kind:?}");
            assert_eq!(got.roughness(), look.roughness(), "{kind:?}");
            assert_eq!(
                got.clear_coat().map(|c| c.weight()),
                look.clear_coat().map(|c| c.weight()),
                "{kind:?}"
            );
            let (a, b) = (got.base_color(), look.base_color());
            for (x, y) in [(a.x, b.x), (a.y, b.y), (a.z, b.z)] {
                assert!((x - y).abs() < 1e-3, "{kind:?}: base {x} against {y}");
            }
        }
        Ok(())
    }

    #[test]
    fn looks_of_one_pattern_share_a_shape_so_a_board_compiles_at_most_one_per_pattern() -> Result<()>
    {
        let fabrication = Fabrication::default();
        let mut shapes = HashSet::new();
        let mut patterns = HashSet::new();
        for kind in BodyKind::ALL {
            let d = detail(kind, &fabrication);
            patterns.insert(d.pattern);
            shapes.insert(
                lower_look(
                    material(kind, &fabrication, &crate::stack::Stack::default()).surface()?,
                    d,
                )?
                .ir
                .shape,
            );
        }
        assert!(patterns.contains(&Pattern::None));
        assert_eq!(shapes.len(), patterns.len());
        Ok(())
    }
}
