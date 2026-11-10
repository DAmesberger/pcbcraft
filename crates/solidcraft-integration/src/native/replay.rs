//! Updating a built design in place. A board build is a sequence of
//! native commands; an edit mostly changes values inside it: a parameter's
//! expression, the drawing of a sketch, where a placement stands. A build
//! of the edited board is run against the recorded sequence instead of a
//! new session: each command is compared with the one at its step and
//! answered with that step's recorded reply, and only the differences are
//! applied to the design afterwards, so evaluation reuses every feature an
//! edit did not reach.
//!
//! The commands that build one object (a track) can be marked as a
//! [`Unit`] of its name. An object the edit added is a unit the recorded
//! build did not have: its commands run on the design as they come (its
//! features go at the end of the history, where evaluating them reads
//! nothing evaluated before); an object the edit removed is a recorded unit
//! the build no longer runs, and what it created is deleted. A unit
//! recorded with a digest of its content (a fill tile) that comes again
//! with another digest is deleted and run again, instead of being redrawn.
//! The board's aggregate commands follow: parameters added or dropped,
//! joints added, looks assigned again, metadata recorded again. Any other
//! difference ends the replay and the board is built anew.
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
    units: Vec<Unit>,
}

/// The commands `start..end` of a log that built the object `name`
/// ([`super::Native::unit`]); `content` is a digest of what it builds when
/// another one is to replace it rather than be updated in place.
#[derive(Clone, Debug)]
struct Unit {
    name: String,
    content: Option<u64>,
    start: usize,
    end: usize,
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

    /// Begin the unit `name` at the next command.
    pub(super) fn begin_unit(&mut self, name: &str, content: Option<u64>) {
        let at = self.ran.len();
        self.units.push(Unit {
            name: name.to_owned(),
            content,
            start: at,
            end: at,
        });
    }

    /// End the unit begun last.
    pub(super) fn end_unit(&mut self) {
        let at = self.ran.len();
        if let Some(unit) = self.units.last_mut() {
            unit.end = at;
        }
    }

    /// The parameters and reply recorded at `step`.
    pub(super) fn get(&self, step: usize) -> Option<(&Value, &Value)> {
        self.ran.get(step).map(|r| (&r.params, &r.reply))
    }

    /// Record what `step` (a `command`) would have recorded in a fresh
    /// build: its parameters, its reply, or both; `false` if the step is
    /// another command.
    pub(super) fn amend(
        &mut self,
        step: usize,
        command: &str,
        params: Option<Value>,
        reply: Option<Value>,
    ) -> bool {
        let Some(ran) = self.ran.get_mut(step).filter(|r| r.command == command) else {
            return false;
        };
        if let Some(params) = params {
            ran.params = params;
        }
        if let Some(reply) = reply {
            ran.reply = reply;
        }
        true
    }
}

/// A build compared with a recorded one ([`Native::replaying`]).
pub(crate) struct Replay {
    previous: Log,
    now: Log,
    /// The next recorded step to compare with.
    cursor: usize,
    /// Each step of `now`: the recorded step it was compared with, `None`
    /// for one run on the design (an added unit's).
    origin: Vec<Option<usize>>,
    /// Steps of `now` whose parameters differ from the recorded ones.
    changed: Vec<usize>,
    /// The recorded units by name.
    by_name: BTreeMap<String, usize>,
    /// The first recorded unit not yet reached.
    next_unit: usize,
    /// The unit being built: the recorded one it is compared with, or
    /// `None` for one that runs on the design.
    open: Option<Option<usize>>,
    /// Parameters the build no longer adds, dropped once their users are.
    dropped: Vec<String>,
    /// Bodies of units removed (not built again), whose looks go too.
    gone: BTreeSet<String>,
    /// Steps of `now` to run again once the design is changed (looks
    /// assigned to other bodies).
    again: Vec<usize>,
    /// The component active at this point of the build, and the one last
    /// activated on the design.
    active: Value,
    synced: Option<Value>,
    /// Whether a command ran on the design (its evaluation is put off),
    /// and how many did.
    live: bool,
    counted: usize,
    /// Each component's first occurrence (where its copies are placed
    /// from), and how far an edit moves a first occurrence.
    first_of: BTreeMap<u64, u64>,
    moved: BTreeMap<u64, [f64; 3]>,
    /// Copies being placed ([`Next::instead`]).
    placing: Option<Placing>,
}

/// What the replay makes of the next command: commands to run on the
/// design first, then the recorded reply, or `None` to run the command
/// itself ([`Replay::ran`]).
pub(super) struct Next {
    pub before: Vec<(String, Value)>,
    pub reply: Option<Value>,
    /// Run the command with these parameters instead, and answer with
    /// what [`Replay::complete`] makes of its reply.
    pub instead: Option<Value>,
}

/// Copies of a definition placed again in another number: each placement
/// the build asks for, the recorded occurrence that stands there or `None`
/// for one copied now ([`Replay::complete`]).
struct Placing {
    params: Value,
    reply: Value,
    slots: Vec<Option<u64>>,
}

/// A translation on a nanometre grid, to find a copy by where it stands.
fn grid(t: [f64; 3]) -> [i64; 3] {
    t.map(|v| (v * 1e6).round() as i64)
}

/// A joint list's joints by name.
fn joints(params: &Value) -> Option<BTreeMap<&str, &Value>> {
    params
        .get("joints")?
        .as_array()?
        .iter()
        .map(|j| Some((j.get("name")?.as_str()?, j)))
        .collect()
}

/// Whether `old` and `new` differ only in `key`.
fn same_but(old: &Value, new: &Value, key: &str) -> bool {
    match (old.as_object(), new.as_object()) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| k == key || b.get(k).is_some_and(|w| w == v))
        }
        _ => false,
    }
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

/// Whether extrusion `feature`'s stored pick may not name the profiles it
/// named before its sketch was redrawn: a pick of several profiles (by
/// index, curve or point, which a redrawn sketch numbers and places anew),
/// or a pick by a curve the sketch no longer has.
fn pick_moved(native: &Native, feature: u64, sketch: u64) -> bool {
    use solidcraft_engine::doc::{FeatureKind, ProfileSel};
    let Some(f) = native.session.doc.features.iter().find(|f| f.id == feature) else {
        return false;
    };
    let FeatureKind::Extrude { profiles, .. } = &f.kind else {
        return false;
    };
    match profiles {
        ProfileSel::All => false,
        ProfileSel::Indices { indices } => indices.len() > 1,
        ProfileSel::Points { points } => points.len() > 1,
        ProfileSel::Curves { loops } => {
            let Ok(drawn) = native.session.doc.sketch(sketch) else {
                return true;
            };
            let have: BTreeSet<&str> = drawn.curves.iter().map(|c| c.id.as_str()).collect();
            loops.len() > 1 || loops.iter().flatten().any(|c| !have.contains(c.as_str()))
        }
        _ => true,
    }
}

fn minus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

impl Replay {
    pub(super) fn new(previous: Log) -> Self {
        let by_name = previous
            .units
            .iter()
            .enumerate()
            .map(|(k, u)| (u.name.clone(), k))
            .collect();
        Self {
            previous,
            now: Log::default(),
            cursor: 0,
            origin: Vec::new(),
            changed: Vec::new(),
            by_name,
            next_unit: 0,
            open: None,
            dropped: Vec::new(),
            gone: BTreeSet::new(),
            again: Vec::new(),
            active: json!("root"),
            synced: None,
            live: false,
            counted: 0,
            first_of: BTreeMap::new(),
            moved: BTreeMap::new(),
            placing: None,
        }
    }

    pub(super) fn step(&self) -> usize {
        self.now.ran.len()
    }

    /// Note that a command runs on the design; returns whether one did
    /// before (whose evaluation is already put off).
    pub(super) fn count(&mut self, commands: usize) {
        self.counted = self.counted.saturating_add(commands);
    }

    pub(super) fn go_live(&mut self) -> bool {
        std::mem::replace(&mut self.live, true)
    }

    /// The commands that delete what recorded unit `k` created: its
    /// components (with their features, occurrences and joints), else its
    /// features.
    fn remove(&self, k: usize) -> Vec<(String, Value)> {
        let Some(unit) = self.previous.units.get(k) else {
            return Vec::new();
        };
        let ran = self.previous.ran.get(unit.start..unit.end).unwrap_or(&[]);
        let components: Vec<u64> = ran
            .iter()
            .filter(|r| r.command == "component.create")
            .filter_map(|r| r.reply.get("component").and_then(Value::as_u64))
            .collect();
        if !components.is_empty() {
            return components
                .into_iter()
                .map(|c| ("component.delete".to_owned(), json!({"component": c})))
                .collect();
        }
        let features: Vec<u64> = ran
            .iter()
            .filter_map(|r| match r.command.as_str() {
                "sketch.create" => r.reply.get("sketch").and_then(Value::as_u64),
                _ => r.reply.get("feature").and_then(Value::as_u64),
            })
            .collect();
        if features.is_empty() {
            return Vec::new();
        }
        vec![("timeline.delete".to_owned(), json!({"features": features}))]
    }

    /// Note the bodies recorded unit `k` made, which a removal takes away.
    fn forget(&mut self, k: usize) {
        let Some(unit) = self.previous.units.get(k) else {
            return;
        };
        for r in self.previous.ran.get(unit.start..unit.end).unwrap_or(&[]) {
            let made = r.reply.get("created_bodies").and_then(Value::as_array);
            self.gone.extend(
                made.into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
            if r.command.starts_with("solid.")
                && let Some(name) = r.params.get("body_name").and_then(Value::as_str)
            {
                self.gone.insert(name.to_owned());
            }
        }
    }

    /// The recorded units starting at the cursor, which the build passed
    /// over: removed.
    fn skip_units(&mut self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        while let Some(unit) = self.previous.units.get(self.next_unit) {
            if unit.start != self.cursor {
                break;
            }
            let end = unit.end;
            self.forget(self.next_unit);
            out.extend(self.remove(self.next_unit));
            self.cursor = end;
            self.next_unit += 1;
        }
        out
    }

    /// Activate the build's active component on the design before
    /// `command` runs there, unless it is active.
    fn sync(&mut self, command: &str) -> Vec<(String, Value)> {
        if command == "component.activate" || self.synced.as_ref() == Some(&self.active) {
            return Vec::new();
        }
        self.synced = Some(self.active.clone());
        vec![(
            "component.activate".to_owned(),
            json!({"component": self.active}),
        )]
    }

    /// Follow the component the build activates.
    fn follow(&mut self, command: &str, params: &Value, reply: &Value) {
        if command == "component.create"
            && let (Some(c), Some(o)) = (
                reply.get("component").and_then(Value::as_u64),
                reply.get("occurrence").and_then(Value::as_u64),
            )
        {
            self.first_of.insert(c, o);
        }
        match command {
            "component.activate" => {
                if let Some(c) = params.get("component") {
                    self.active = c.clone();
                }
            }
            "component.create" if params.get("activate") != Some(&Value::Bool(false)) => {
                if let Some(c) = reply.get("component") {
                    self.active = c.clone();
                }
            }
            _ => {}
        }
    }

    /// Begin the unit `name` ([`Log::begin_unit`]): the recorded one of
    /// that name when it is next (the units recorded before it removed),
    /// else a unit that runs on the design. Returns the commands to run
    /// first: deleting the removed units, and the recorded one when its
    /// content changed.
    pub(super) fn begin_unit(
        &mut self,
        name: &str,
        content: Option<u64>,
    ) -> Result<Vec<(String, Value)>> {
        let step = self.step();
        if self.open.is_some() {
            return Err(diverged(step, format!("unit {name} inside another")));
        }
        let mut out = Vec::new();
        let mut open = None;
        if let Some(&u) = self.by_name.get(name) {
            let units = &self.previous.units;
            let contiguous = u >= self.next_unit
                && units.get(self.next_unit..=u).is_some_and(|run| {
                    run.first().is_some_and(|f| f.start == self.cursor)
                        && run.windows(2).all(|w| w[0].end == w[1].start)
                });
            if !contiguous {
                return Err(diverged(step, format!("{name} built out of order")));
            }
            let (start, end, same) = match units.get(u) {
                Some(unit) => (unit.start, unit.end, unit.content == content),
                None => return Err(diverged(step, "a unit out of range")),
            };
            for k in self.next_unit..u {
                self.forget(k);
                out.extend(self.remove(k));
            }
            self.next_unit = u + 1;
            if same {
                self.cursor = start;
                open = Some(u);
            } else {
                out.extend(self.remove(u));
                self.cursor = end;
            }
        }
        self.open = Some(open);
        self.now.begin_unit(name, content);
        Ok(out)
    }

    /// End the unit begun last.
    pub(super) fn end_unit(&mut self) -> Result<()> {
        let step = self.step();
        match self.open.take() {
            Some(Some(u)) => {
                let end = self.previous.units.get(u).map(|unit| unit.end);
                if end != Some(self.cursor) {
                    return Err(diverged(step, "a unit ran fewer commands"));
                }
            }
            Some(None) => {}
            None => return Err(diverged(step, "a unit ended twice")),
        }
        self.now.end_unit();
        Ok(())
    }

    /// Compare the next command with the recorded one and answer with its
    /// recorded reply, or (in a unit that runs on the design) have it run.
    pub(super) fn next(&mut self, command: &str, params: Value) -> Result<Next> {
        let step = self.step();
        if self.open == Some(None) {
            return Ok(Next {
                before: self.sync(command),
                reply: None,
                instead: None,
            });
        }
        let mut before = Vec::new();
        if self.open.is_none() {
            before = self.skip_units();
        }
        if let Some(Some(u)) = self.open
            && self
                .previous
                .units
                .get(u)
                .is_some_and(|unit| self.cursor >= unit.end)
        {
            return Err(diverged(step, format!("{command} is new in its unit")));
        }
        let Some(old) = self.previous.ran.get(self.cursor) else {
            return Err(diverged(step, format!("{command} is new")));
        };
        if old.command != command {
            return Err(diverged(
                step,
                format!("{command} where {} ran", old.command),
            ));
        }
        if old.params != params {
            match command {
                "parameters.import" => {
                    let (Some(was), Some(now)) = (rows(&old.params), rows(&params)) else {
                        return Err(diverged(step, "unreadable parameters"));
                    };
                    let known: BTreeSet<&str> = was.iter().map(|r| r.0.as_str()).collect();
                    let kept: BTreeSet<&str> = now.iter().map(|r| r.0.as_str()).collect();
                    let added: Vec<Value> = now
                        .iter()
                        .filter(|r| !known.contains(r.0.as_str()))
                        .map(|(name, expression)| json!({"name": name, "expression": expression}))
                        .collect();
                    self.dropped.extend(
                        was.iter()
                            .filter(|r| !kept.contains(r.0.as_str()))
                            .map(|r| r.0.clone()),
                    );
                    if !added.is_empty() {
                        before.push((
                            "parameters.import".to_owned(),
                            json!({"text": serde_json::to_string(&added)?}),
                        ));
                    }
                }
                "joint.create" => {
                    let (Some(was), Some(now)) = (joints(&old.params), joints(&params)) else {
                        return Err(diverged(step, format!("{command} changed")));
                    };
                    let mut added = Vec::new();
                    for (name, joint) in &now {
                        match was.get(name) {
                            Some(recorded) if recorded == joint => {}
                            Some(_) => return Err(diverged(step, format!("{command} changed"))),
                            None => added.push((*joint).clone()),
                        }
                    }
                    // A joint no longer made held an occurrence deleted with
                    // its unit's component, and went with it.
                    if !added.is_empty() {
                        before.push(("joint.create".to_owned(), json!({"joints": added})));
                    }
                }
                "occurrence.move" => {
                    if let (Some(o), Some((_, was)), Some((_, now))) = (
                        params.get("occurrence").and_then(Value::as_u64),
                        matrix(&old.params),
                        matrix(&params),
                    ) {
                        self.moved.insert(o, minus(now, was));
                    }
                    if !diffable(command, &old.params, &params) {
                        return Err(diverged(step, format!("{command} changed")));
                    }
                }
                "occurrence.copy"
                    if same_but(&old.params, &params, "translations")
                        && translations(&old.params).map(|t| t.len())
                            != translations(&params).map(|t| t.len()) =>
                {
                    let (reply, was) = (old.reply.clone(), old.params.clone());
                    return self.place(step, reply, &was, params, before);
                }
                "appearance.assign" if same_but(&old.params, &params, "bodies") => {
                    self.again.push(step);
                }
                "document.metadata.set" => {}
                _ if diffable(command, &old.params, &params) => {}
                _ => return Err(diverged(step, format!("{command} changed"))),
            }
            self.changed.push(step);
        }
        let reply = old.reply.clone();
        self.follow(command, &params, &reply);
        self.now.push(command, params, reply.clone());
        self.origin.push(Some(self.cursor));
        self.cursor += 1;
        Ok(Next {
            before,
            reply: Some(reply),
            instead: None,
        })
    }

    /// Copies of a definition in another number than recorded (a via added
    /// or removed): every recorded copy that stands where a copy is asked
    /// for is kept, the others are deleted, and the new ones are copied on
    /// the design ([`Next::instead`], then [`Self::complete`]). A copy is
    /// placed from where its first occurrence stands, which an edit may
    /// move (that move is applied last, with the others): a copy stands
    /// where it stood when its translation and that move cancel out.
    fn place(
        &mut self,
        step: usize,
        recorded: Value,
        was: &Value,
        params: Value,
        mut before: Vec<(String, Value)>,
    ) -> Result<Next> {
        let (Some(component), Some(was), Some(now)) = (
            params.get("component").and_then(Value::as_u64),
            translations(was),
            translations(&params),
        ) else {
            return Err(diverged(step, "unreadable copies"));
        };
        let placed: Vec<u64> = recorded
            .get("occurrences")
            .and_then(Value::as_array)
            .map(|o| o.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        if placed.len() != was.len() {
            return Err(diverged(step, "copies without occurrences"));
        }
        let first = self
            .first_of
            .get(&component)
            .and_then(|o| self.moved.get(o))
            .copied()
            .unwrap_or_default();
        let mut stood: BTreeMap<[i64; 3], Vec<u64>> = BTreeMap::new();
        for (t, o) in was.iter().zip(&placed).rev() {
            stood.entry(grid(*t)).or_default().push(*o);
        }
        let slots: Vec<Option<u64>> = now
            .iter()
            .map(|t| {
                let at: [f64; 3] = std::array::from_fn(|i| t[i] + first[i]);
                stood.get_mut(&grid(at)).and_then(Vec::pop)
            })
            .collect();
        let gone: Vec<u64> = stood.into_values().flatten().collect();
        if !gone.is_empty() {
            before.push(("selection.delete".to_owned(), json!({"occurrences": gone})));
        }
        // The new copies from where the first occurrence stands now.
        let new: Vec<[f64; 3]> = now
            .iter()
            .zip(&slots)
            .filter(|(_, s)| s.is_none())
            .map(|(t, _)| std::array::from_fn(|i| t[i] + first[i]))
            .collect();
        let placing = Placing {
            params,
            reply: recorded,
            slots,
        };
        if new.is_empty() {
            let reply = self.complete_with(placing, &[])?;
            return Ok(Next {
                before,
                reply: Some(reply),
                instead: None,
            });
        }
        let mut instead = placing.params.clone();
        if let Some(map) = instead.as_object_mut() {
            map.insert("translations".into(), json!(new));
        }
        self.placing = Some(placing);
        Ok(Next {
            before,
            reply: None,
            instead: Some(instead),
        })
    }

    /// Finish placing copies ([`Self::place`]) with the reply of the
    /// command that copied the new ones.
    pub(super) fn complete(&mut self, reply: Value) -> Result<Value> {
        let step = self.step();
        let placing = self
            .placing
            .take()
            .ok_or_else(|| diverged(step, "no copies being placed"))?;
        let made: Vec<u64> = reply
            .get("occurrences")
            .and_then(Value::as_array)
            .map(|o| o.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        self.complete_with(placing, &made)
    }

    fn complete_with(&mut self, placing: Placing, made: &[u64]) -> Result<Value> {
        let step = self.step();
        let mut made = made.iter();
        let occurrences: Vec<u64> = placing
            .slots
            .iter()
            .map(|slot| slot.or_else(|| made.next().copied()))
            .collect::<Option<_>>()
            .ok_or_else(|| diverged(step, "copies went missing"))?;
        let mut reply = placing.reply;
        if let Some(map) = reply.as_object_mut() {
            map.insert("occurrences".into(), json!(occurrences));
        }
        self.now
            .push("occurrence.copy", placing.params, reply.clone());
        self.origin.push(Some(self.cursor));
        self.cursor += 1;
        Ok(reply)
    }

    /// Record `command`, which ran on the design ([`Next::reply`] `None`).
    pub(super) fn ran(&mut self, command: &str, params: Value, reply: Value) {
        self.follow(command, &params, &reply);
        self.synced = Some(self.active.clone());
        self.now.push(command, params, reply);
        self.origin.push(None);
    }

    /// Commands that ran on the design leave its active component unknown.
    pub(super) fn unsynced(&mut self) {
        self.synced = None;
    }

    pub(super) fn mark(&mut self, name: &str, start: usize, end: usize) {
        self.now.mark(name, start, end);
    }

    /// Pass over the recorded commands that built the definition `name`.
    pub(super) fn pass(&mut self, name: &str) -> bool {
        let Some((_, start, end)) = self
            .previous
            .marks
            .iter()
            .find(|(n, start, _)| n == name && *start == self.cursor)
            .cloned()
        else {
            return false;
        };
        let Some(ran) = self.previous.ran.get(start..end).map(<[Ran]>::to_vec) else {
            return false;
        };
        let at = self.now.ran.len();
        for (k, r) in ran.iter().enumerate() {
            self.follow(&r.command, &r.params, &r.reply);
            self.origin.push(Some(start + k));
        }
        let len = ran.len();
        self.now.ran.extend(ran);
        self.now.mark(name, at, at + len);
        self.cursor = end;
        true
    }

    /// Apply every difference to `native` (the recorded design); returns the
    /// new command stream and how many commands the update ran.
    pub(super) fn apply(mut self, native: &mut Native) -> Result<(Log, usize)> {
        let mut ran = self.counted;
        // The recorded units after the last one built are removed too.
        let removed = self.skip_units();
        if self.cursor != self.previous.ran.len() {
            return Err(diverged(self.now.ran.len(), "the build ran fewer commands"));
        }
        let mut live = self.live;
        if !removed.is_empty() {
            if !live {
                native.defer()?;
                live = true;
            }
            ran += removed.len();
            native.run_live(removed)?;
        }
        // Parameters no longer used, once what used them is gone.
        for name in &self.dropped {
            if !live {
                native.defer()?;
                live = true;
            }
            native.run("parameters.delete", &json!({"name": name}))?;
            ran += 1;
        }
        // A removed body keeps no look (Solidcraft's `appearance.assign`
        // takes only bodies that are there).
        if !self.gone.is_empty() {
            let looks = &mut native.session_mut().doc_mut().appearances.bodies;
            looks.retain(|name, _| !self.gone.contains(name));
        }
        // What ran on the design is evaluated once.
        if live {
            native.evaluate_deferred()?;
        }
        let Self {
            previous,
            mut now,
            changed,
            origin,
            again,
            ..
        } = self;
        let old = |step: usize| {
            origin
                .get(step)
                .copied()
                .flatten()
                .and_then(|at| previous.ran.get(at))
        };
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
                    // Parameters added or dropped were added or are
                    // dropped on their own.
                    let was: BTreeMap<String, String> = a.into_iter().collect();
                    for (name, now) in b {
                        if was.get(&name).is_some_and(|w| *w != now) {
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
                "document.metadata.set" => {
                    // Values recorded before and no longer are removed.
                    let mut values = b.get("values").cloned().unwrap_or_else(|| json!({}));
                    if let (Some(set), Some(was)) = (
                        values.as_object_mut(),
                        a.get("values").and_then(Value::as_object),
                    ) {
                        for key in was.keys() {
                            if !set.contains_key(key) {
                                set.insert(key.clone(), Value::Null);
                            }
                        }
                    }
                    metadata.push(json!({"values": values}));
                }
                // Added joints were made as they came, and a joint the
                // build no longer makes went with its component.
                "joint.create" => {}
                // Assigned again below.
                "appearance.assign" if again.contains(&step) => {}
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
        // Units deleted may have rolled the history back already
        // ([`Native::delete_rolled_back`]); it is rolled back further only.
        let marker = |native: &Native| {
            let doc = &native.session.doc;
            doc.marker.unwrap_or(doc.features.len())
        };
        if let Some(position) = first.filter(|p| *p < marker(native)) {
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
        let mut redrawn = BTreeSet::new();
        for (start, steps) in sketches {
            let sketch = now
                .ran
                .get(start)
                .and_then(|r| r.reply.get("sketch"))
                .and_then(Value::as_u64)
                .ok_or_else(|| diverged(start, "a sketch without an id"))?;
            redrawn.insert(sketch);
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
        // An extrusion that left its profiles to the default stored its pick
        // (a laminate slab in pieces, a fill region with holes) by index or
        // by curve, and a redrawn drawing numbers its profiles and curves
        // anew: the stored pick would fail, fall back to a best match or
        // name other profiles. Rebuilt from its command, the extrusion picks
        // the redrawn profiles as a fresh build does. A pick of one profile
        // by an outline drawn apart from the redrawn holes (a slab's) keeps
        // its curves, and the extrusion as it is. Each is redefined while
        // the history is still rolled back: a redefinition evaluates the
        // design from its feature to the marker, so behind the marker it
        // evaluates only its own extrusion (and what comes before it), and
        // the features after it are evaluated once, rolled forward below.
        // Rolled forward first, every redefinition would evaluate every
        // feature after it again (the four fills and the laminate of the
        // ESP32 example about three times over).
        for r in &now.ran {
            let sketch = r.params.get("sketch").and_then(Value::as_u64);
            if r.command != "solid.extrude"
                || r.params.get("profiles").is_some()
                || !sketch.is_some_and(|s| redrawn.contains(&s))
            {
                continue;
            }
            let Some(feature) = r.reply.get("feature").and_then(Value::as_u64) else {
                continue;
            };
            if sketch.is_some_and(|s| pick_moved(native, feature, s)) {
                native.run(
                    "timeline.redefine",
                    &json!({"feature": feature, "command": "solid.extrude", "params": r.params}),
                )?;
                ran += 1;
            }
        }
        if native.session.doc.marker.is_some() {
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
        // Looks assigned to other bodies (one added or removed): assigned
        // as the build assigns them.
        for step in again {
            let Some(r) = now.ran.get(step) else {
                continue;
            };
            native.run(&r.command, &r.params)?;
            ran += 1;
        }
        // The design is left as a build leaves it, with the root active.
        if live {
            native.run("component.activate", &json!({"component": "root"}))?;
            ran += 1;
        }
        Ok((now, ran))
    }
}
