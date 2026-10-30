//! FD2b probe: reach Solidcraft's private sketch-modify commands (trim,
//! fillet, offset, mirror) without a fork, through a throwaway `Session`.
//! The footprint sketch is placed in a scratch document, the public
//! `Session::execute` runs the command, and the sketch is read back.
//!
//! Parameter bindings are Pcbcraft's (footprint parameters, not document
//! parameters), so they are detached before the sketch enters the scratch
//! document and re-attached by constraint id afterwards; parameters the
//! command itself creates in the scratch document are reported so the
//! caller can adopt them as footprint parameters.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::sketch::Sketch;

use super::{Outcome, finding};

pub(crate) struct BridgeRun {
    pub sketch: Sketch,
    pub reply: Value,
    /// Parameters the command created in the scratch document: (constraint
    /// id, parameter name, expression).
    pub created: Vec<(String, String, String)>,
}

fn engine(command: &str, e: impl std::fmt::Display) -> super::Finding {
    finding("footprint.bridge", format!("{command}: {e}"))
}

/// Run one Solidcraft sketch command on a copy of `sketch`.
pub(crate) fn run(sketch: &Sketch, command: &str, params: Value) -> Outcome<BridgeRun> {
    run_all(sketch, vec![(command, params)])
}

/// Run Solidcraft sketch commands in order on a copy of `sketch`; the reply is
/// the last command's.
pub(crate) fn run_all(sketch: &Sketch, steps: Vec<(&str, Value)>) -> Outcome<BridgeRun> {
    if steps.is_empty() || steps.len() > 16 {
        return Err(finding("footprint.bridge", "1–16 commands"));
    }
    if let Some((command, _)) = steps.iter().find(|(c, _)| !c.starts_with("sketch.")) {
        return Err(finding(
            "footprint.bridge",
            format!("{command} is not a sketch command"),
        ));
    }
    let mut session = Session::default();
    let created = session
        .execute("sketch.create", &json!({"plane": "XY"}))
        .map_err(|e| engine("sketch.create", e))?;
    let id = created
        .get("sketch")
        .and_then(Value::as_u64)
        .ok_or_else(|| engine("sketch.create", "no sketch id"))?;
    let mut detached = sketch.clone();
    let mut bindings = BTreeMap::new();
    for c in &mut detached.constraints {
        if let Some(p) = c.param.take() {
            bindings.insert(c.id.clone(), p);
        }
    }
    // Point expressions name footprint parameters too; the points stay fixed
    // where they are while detached.
    let mut exprs = BTreeMap::new();
    for p in &mut detached.points {
        if let Some(e) = p.exprs.take() {
            exprs.insert(p.id.clone(), e);
        }
    }
    *Arc::make_mut(&mut session.doc)
        .sketch_mut(id)
        .map_err(|e| engine("sketch.replace", e))? = detached;
    session.revision += 1;
    let mut reply = Value::Null;
    for (command, mut params) in steps {
        if let Some(o) = params.as_object_mut() {
            o.insert("sketch".into(), json!(id));
        }
        reply = session
            .execute(command, &params)
            .map_err(|e| engine(command, e))?;
    }
    let mut out = session
        .doc
        .sketch(id)
        .map_err(|e| engine("sketch.read", e))?
        .clone();
    for p in &mut out.points {
        if let Some(e) = exprs.remove(&p.id) {
            p.exprs = Some(e);
        }
    }
    if let Some(id) = exprs.keys().next() {
        return Err(finding(
            "footprint.bridge",
            format!("the command removed expression-driven point {id}"),
        ));
    }
    let mut created = Vec::new();
    for c in &mut out.constraints {
        if let Some(p) = bindings.get(&c.id) {
            c.param = Some(p.clone());
        } else if let Some(name) = c.param.clone() {
            let expr = session
                .doc
                .params
                .iter()
                .find(|q| q.name == name)
                .map(|q| q.expr.clone())
                .unwrap_or_default();
            created.push((c.id.clone(), name, expr));
        }
    }
    Ok(BridgeRun {
        sketch: out,
        reply,
        created,
    })
}
