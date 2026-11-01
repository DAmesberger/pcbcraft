//! Updating a built design in place. A board build is a fixed sequence of
//! native commands for a given structure (which parts, pads, tracks and via
//! sizes there are); an edit mostly changes values inside it: a parameter's
//! expression, the drawing of a sketch, where a placement stands. A build
//! of the edited board is run against the recorded sequence instead of a
//! new session: each command is compared with the one at its step and
//! answered with that step's recorded reply, and only the differences are
//! applied to the design afterwards, so evaluation reuses every feature an
//! edit did not reach. Any other difference ends the replay and the board
//! is built anew.
use super::Native;
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Most placements an update moves one by one; a larger move is built anew.
const MAX_MOVES: usize = 256;

/// Most entities one `sketch.delete` names (Solidcraft's limit).
const MAX_DELETED: usize = 10_000;

/// Sketch commands whose drawing may change: the sketch is redrawn.
const DRAWING: [&str; 4] = [
    "sketch.insert_dxf",
    "sketch.line",
    "sketch.circle.center",
    "sketch.arc.three_point",
];

/// A placement moved by less than this (mm) stays where it is.
const STILL_MM: f64 = 1e-12;

#[derive(Clone, Debug)]
struct Ran {
    command: String,
    params: Value,
    reply: Value,
}

/// The commands a build ran, in order, and the runs of them that built a
/// named definition.
#[derive(Clone, Debug, Default)]
pub(crate) struct Log {
    ran: Vec<Ran>,
    marks: Vec<(String, usize, usize)>,
}

impl Log {
    pub(super) fn push(&mut self, command: &str, params: Value, reply: Value) {
        self.ran.push(Ran {
            command: command.to_owned(),
            params,
            reply,
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.ran.len()
    }

    pub(super) fn mark(&mut self, name: &str, start: usize, end: usize) {
        self.marks.push((name.to_owned(), start, end));
    }
}

/// A build compared with a recorded one ([`Native::replaying`]).
pub(crate) struct Replay {
    previous: Log,
    now: Log,
    /// Steps whose parameters differ from the recorded ones.
    changed: Vec<usize>,
}

fn diverged(step: usize, why: impl std::fmt::Display) -> Error {
    Error::Verification(format!(
        "the board changed in more than values at native step {step}: {why}"
    ))
}

/// The rows of a `parameters.import`.
fn rows(params: &Value) -> Option<Vec<(String, String)>> {
    let text = params.get("text")?.as_str()?;
    let rows: Vec<Value> = serde_json::from_str(text).ok()?;
    rows.iter()
        .map(|r| {
            Some((
                r.get("name")?.as_str()?.to_owned(),
                r.get("expression")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// A 3 × 4 matrix's rotation and translation.
fn matrix(params: &Value) -> Option<([[f64; 3]; 3], [f64; 3])> {
    let rows: [[f64; 4]; 3] = serde_json::from_value(params.get("matrix")?.clone()).ok()?;
    Some((
        rows.map(|r| [r[0], r[1], r[2]]),
        [rows[0][3], rows[1][3], rows[2][3]],
    ))
}

fn translations(params: &Value) -> Option<Vec<[f64; 3]>> {
    serde_json::from_value(params.get("translations")?.clone()).ok()
}

/// Whether a command whose parameters went from `old` to `new` changes
/// only values an update applies in place.
fn diffable(command: &str, old: &Value, new: &Value) -> bool {
    let same = |key: &str| old.get(key) == new.get(key);
    match command {
        "parameters.add" => same("name") && same("unit"),
        "parameters.import" => match (rows(old), rows(new)) {
            (Some(old), Some(new)) => {
                old.len() == new.len() && old.iter().zip(&new).all(|(a, b)| a.0 == b.0)
            }
            _ => false,
        },
        c if DRAWING.contains(&c) => true,
        "occurrence.move" => {
            same("occurrence")
                && same("capture")
                && matches!((matrix(old), matrix(new)), (Some(a), Some(b)) if a.0 == b.0)
        }
        "occurrence.copy" => {
            same("component")
                && matches!(
                    (translations(old), translations(new)),
                    (Some(a), Some(b)) if a.len() == b.len()
                )
        }
        "document.metadata.set" => {
            let keys = |v: &Value| {
                v.get("values")
                    .and_then(Value::as_object)
                    .map(|m| m.keys().cloned().collect::<Vec<_>>())
            };
            keys(old).is_some() && keys(old) == keys(new)
        }
        _ => false,
    }
}

fn minus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

impl Replay {
    pub(super) fn new(previous: Log) -> Self {
        Self {
            previous,
            now: Log::default(),
            changed: Vec::new(),
        }
    }

    pub(super) fn step(&self) -> usize {
        self.now.ran.len()
    }

    /// Compare the next command with the recorded one; answer with its
    /// recorded reply.
    pub(super) fn next(&mut self, command: &str, params: Value) -> Result<Value> {
        let step = self.step();
        let Some(old) = self.previous.ran.get(step) else {
            return Err(diverged(step, format!("{command} is new")));
        };
        if old.command != command {
            return Err(diverged(
                step,
                format!("{command} where {} ran", old.command),
            ));
        }
        if old.params != params {
            if !diffable(command, &old.params, &params) {
                return Err(diverged(step, format!("{command} changed")));
            }
            self.changed.push(step);
        }
        let reply = old.reply.clone();
        self.now.push(command, params, reply.clone());
        Ok(reply)
    }

    pub(super) fn mark(&mut self, name: &str, start: usize, end: usize) {
        self.now.mark(name, start, end);
    }

    /// Pass over the recorded commands that built the definition `name`.
    pub(super) fn pass(&mut self, name: &str) -> bool {
        let step = self.step();
        let Some((_, start, end)) = self
            .previous
            .marks
            .iter()
            .find(|(n, start, _)| n == name && *start == step)
            .cloned()
        else {
            return false;
        };
        let Some(ran) = self.previous.ran.get(start..end) else {
            return false;
        };
        self.now.ran.extend_from_slice(ran);
        self.now.mark(name, start, end);
        true
    }

    /// Apply every difference to `native` (the recorded design); returns the
    /// new command stream and how many commands the update ran.
    pub(super) fn apply(self, native: &mut Native) -> Result<(Log, usize)> {
        let Self {
            previous,
            mut now,
            changed,
        } = self;
        if now.ran.len() != previous.ran.len() {
            return Err(diverged(now.ran.len(), "the build ran fewer commands"));
        }
        let old = |step: usize| previous.ran.get(step);
        // The sketch (its creating step) each changed drawing step is in.
        let mut sketch_of: BTreeMap<usize, usize> = BTreeMap::new();
        let mut open: Option<usize> = None;
        let wanted: BTreeSet<usize> = changed.iter().copied().collect();
        for (step, ran) in now.ran.iter().enumerate() {
            match ran.command.as_str() {
                "sketch.create" => open = Some(step),
                "sketch.finish" => open = None,
                c if DRAWING.contains(&c) && wanted.contains(&step) => match open {
                    Some(start) => {
                        sketch_of.insert(step, start);
                    }
                    None => return Err(diverged(step, "a drawing outside a sketch")),
                },
                _ => {}
            }
        }
        let mut parameters = Map::new();
        // Changed drawing steps, by the sketch they draw in.
        let mut sketches: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut metadata = Vec::new();
        // First occurrences moved, by occurrence: how far.
        let mut firsts: BTreeMap<u64, [f64; 3]> = BTreeMap::new();
        let mut moves: Vec<(u64, [f64; 3])> = Vec::new();
        for &step in &changed {
            let (Some(before), Some(after)) = (old(step), now.ran.get(step)) else {
                return Err(diverged(step, "missing step"));
            };
            let (a, b) = (&before.params, &after.params);
            match after.command.as_str() {
                "parameters.add" => {
                    if let (Some(name), Some(expression)) = (
                        b.get("name").and_then(Value::as_str),
                        b.get("expression").cloned(),
                    ) {
                        parameters.insert(name.to_owned(), expression);
                    }
                }
                "parameters.import" => {
                    let (Some(a), Some(b)) = (rows(a), rows(b)) else {
                        return Err(diverged(step, "unreadable parameters"));
                    };
                    for ((_, was), (name, now)) in a.iter().zip(b) {
                        if *was != now {
                            parameters.insert(name, Value::String(now));
                        }
                    }
                }
                c if DRAWING.contains(&c) => {
                    let sketch = sketch_of
                        .get(&step)
                        .ok_or_else(|| diverged(step, "a drawing outside a sketch"))?;
                    sketches.entry(*sketch).or_default().push(step);
                }
                "occurrence.move" => {
                    let (Some(occurrence), Some((_, was)), Some((_, now))) = (
                        b.get("occurrence").and_then(Value::as_u64),
                        matrix(a),
                        matrix(b),
                    ) else {
                        return Err(diverged(step, "unreadable move"));
                    };
                    let by = minus(now, was);
                    firsts.insert(occurrence, by);
                    moves.push((occurrence, by));
                }
                "occurrence.copy" => {
                    let (Some(component), Some(was), Some(now)) = (
                        b.get("component").and_then(Value::as_u64),
                        translations(a),
                        translations(b),
                    ) else {
                        return Err(diverged(step, "unreadable copies"));
                    };
                    let placed: Vec<u64> = after
                        .reply
                        .get("occurrences")
                        .and_then(Value::as_array)
                        .map(|o| o.iter().filter_map(Value::as_u64).collect())
                        .unwrap_or_default();
                    if placed.len() != now.len() {
                        return Err(diverged(step, "copies without occurrences"));
                    }
                    // A copy stood where the first occurrence stood, plus its
                    // translation: it moves as far as both changed.
                    let first = native
                        .session
                        .doc
                        .occurrence_of(component)
                        .map(|o| o.id)
                        .and_then(|o| firsts.get(&o).copied())
                        .unwrap_or_default();
                    for ((occurrence, was), now) in placed.into_iter().zip(was).zip(now) {
                        let d = minus(now, was);
                        let by: [f64; 3] = std::array::from_fn(|i| d[i] + first[i]);
                        if by.iter().any(|v| v.abs() > STILL_MM) {
                            moves.push((occurrence, by));
                        }
                    }
                }
                "document.metadata.set" => metadata.push(b.clone()),
                other => return Err(diverged(step, format!("{other} cannot change"))),
            }
        }
        moves.retain(|(_, by)| by.iter().any(|v| v.abs() > STILL_MM));
        if moves.len() > MAX_MOVES {
            return Err(diverged(
                previous.ran.len(),
                format!("{} placements moved", moves.len()),
            ));
        }
        let mut ran = 0usize;
        // Every command that changes the design evaluates it again from the
        // first feature it changed: while sketches are redrawn, the history
        // is rolled back to the first of them, so the features after it
        // are evaluated once, when it is rolled forward again.
        let first = sketches
            .keys()
            .filter_map(|start| {
                let id = now.ran.get(*start)?.reply.get("sketch")?.as_u64()?;
                native.session.doc.features.iter().position(|f| f.id == id)
            })
            .min();
        if let Some(position) = first {
            native.run("timeline.rollback", &json!({"position": position}))?;
            ran += 1;
        }
        if !parameters.is_empty() {
            native.run("parameters.change", &json!({"changes": parameters}))?;
            ran += 1;
        }
        // A changed drawing is erased (what its step created) and drawn
        // again; the sketch's other curves keep their ids, so the profiles
        // its feature picked by them (an outline) are found again.
        for (start, steps) in sketches {
            let sketch = now
                .ran
                .get(start)
                .and_then(|r| r.reply.get("sketch"))
                .and_then(Value::as_u64)
                .ok_or_else(|| diverged(start, "a sketch without an id"))?;
            native.run("sketch.edit", &json!({"sketch": sketch}))?;
            ran += 1;
            for step in steps {
                let Some(r) = now.ran.get_mut(step) else {
                    continue;
                };
                let created = |kind: &str| {
                    r.reply
                        .get("created")
                        .and_then(|c| c.get(kind))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                };
                let mut doomed = created("curves");
                doomed.extend(created("points"));
                // In chunks; a chunk can take entities of a later one with
                // it (a curve's points), so each names what is left.
                for chunk in doomed.chunks(MAX_DELETED) {
                    let left: BTreeSet<&str> = {
                        let sketch = native
                            .session
                            .doc
                            .sketch(sketch)
                            .map_err(|e| Error::Verification(e.to_string()))?;
                        sketch
                            .points
                            .iter()
                            .map(|p| p.id.as_str())
                            .chain(sketch.curves.iter().map(|c| c.id.as_str()))
                            .collect()
                    };
                    let chunk: Vec<&Value> = chunk
                        .iter()
                        .filter(|id| id.as_str().is_some_and(|id| left.contains(id)))
                        .collect();
                    if chunk.is_empty() {
                        continue;
                    }
                    let entities = json!(chunk);
                    native.run("sketch.delete", &json!({"entities": entities}))?;
                    ran += 1;
                }
                r.reply = native.run(&r.command, &r.params)?;
                ran += 1;
            }
            native.run("sketch.finish", &json!({}))?;
            ran += 1;
        }
        if first.is_some() {
            native.run("timeline.rollback", &json!({"position": "end"}))?;
            ran += 1;
        }
        for (occurrence, by) in moves {
            native.run(
                "occurrence.move",
                &json!({"occurrence": occurrence, "translate": by, "capture": true}),
            )?;
            ran += 1;
        }
        for values in metadata {
            native.run("document.metadata.set", &values)?;
            ran += 1;
        }
        Ok((now, ran))
    }
}
