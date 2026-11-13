//! Compact replies for the MCP and JSON-lines transports: what a request did,
//! not the whole document. A reply names objects by their stable ids; with
//! `verbose: true` the same request returns the snapshot instead.
//!
//! The ids are found by diffing the serialized document, so a collection the
//! document gains later is reported without changing this module.
use super::{InspectParams, Request, Session, envelope};
use crate::gate::{Caller, Decision};
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use sparksmith_core::{Class, Document, Finding, Project, Query, QueryResult, erc, layout};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Ids listed per reply; beyond this `changes.omitted` counts the rest.
const MAX_IDS: usize = 200;
/// New findings listed per reply; beyond this `findings.omitted` counts the
/// rest and `project.issues` lists them all.
const MAX_FINDINGS: usize = 20;
/// Findings listed per changed draft.
const MAX_DRAFT_FINDINGS: usize = 40;

/// The last project a compact reply described, kept so the next mutation
/// serializes and checks only its result. Holding the `Arc` keeps the
/// pointer comparison sound.
pub(super) struct Baseline {
    project: Arc<Project>,
    value: Value,
    findings: Vec<Finding>,
}

impl Baseline {
    fn of(project: &Arc<Project>) -> Result<Self> {
        Ok(Self {
            project: project.clone(),
            value: serde_json::to_value(project.as_ref())?,
            findings: design_findings(project),
        })
    }
}

fn design_findings(project: &Project) -> Vec<Finding> {
    let mut findings = erc::checks(project);
    findings.extend(layout::checks(project));
    findings
}

impl Session {
    /// The transports' entry point: gate one JSON request for `caller`
    /// ([`crate::gate`]), run it and answer in the shared envelope with a
    /// compact result. A refused request changes nothing and answers with
    /// the refusal. `verbose: true`, beside `command` or inside `params`,
    /// answers as [`Session::dispatch`] does.
    pub fn respond(&mut self, caller: Caller<'_>, mut request: Value) -> Value {
        if let Err(refusal) = self.admit(caller, &request) {
            return refusal.reply();
        }
        if take_verbose(&mut request) {
            return self.dispatch(request);
        }
        let result = serde_json::from_value::<Request>(request)
            .map_err(Error::from)
            .and_then(|request| self.execute_compact(request));
        envelope(result)
    }

    /// A dry run of one document edit for `caller`: what [`Session::respond`]
    /// would answer (outcome, changes, findings, drafts), run on a fork of
    /// the engine, so document, revision, history and path stay as they are.
    /// The gate decides as for the edit itself, except that what would ask
    /// a person runs, since nothing is kept. Only requests that change the
    /// engine and nothing else can be tried.
    pub fn rehearse(&mut self, caller: Caller<'_>, mut request: Value) -> Value {
        let policy = match caller {
            Caller::Operator => None,
            Caller::Model(policy) | Caller::Allowed(policy) => Some(policy),
        };
        if let Some(policy) = policy
            && let Decision::Refuse(refusal) = self.check_request(policy, &request)
        {
            return refusal.reply();
        }
        take_verbose(&mut request);
        let result = serde_json::from_value::<Request>(request)
            .map_err(Error::from)
            .and_then(|request| {
                // Only what changes the engine alone: a folder import, for
                // one, also notes a recent in the person's settings.
                let pure = matches!(
                    request,
                    Request::Apply(_)
                        | Request::ApplyAll(_)
                        | Request::Undo(_)
                        | Request::Redo(_)
                        | Request::ApplyUpdate(_)
                );
                if !pure || request.effective_class() != Class::Edit {
                    return Err(sparksmith_core::Error::Invalid(format!(
                        "{} has no dry run: only document operations, undo, redo and updates do",
                        request.subject()
                    ))
                    .into());
                }
                let mut fork = self.engine.fork();
                std::mem::swap(&mut self.engine, &mut fork);
                let path = self.path.clone();
                let baseline = self.baseline.take();
                let tried = self.execute_compact(request);
                std::mem::swap(&mut self.engine, &mut fork);
                self.path = path;
                self.baseline = baseline;
                let mut reply = tried?;
                reply["dry_run"] = json!(true);
                // The document is where it was: the next edit names this.
                reply["revision"] = json!(self.engine.revision());
                reply["can_undo"] = json!(self.engine.can_undo());
                reply["can_redo"] = json!(self.engine.can_redo());
                Ok(reply)
            });
        envelope(result)
    }

    fn execute_compact(&mut self, request: Request) -> Result<Value> {
        let replaced = matches!(request, Request::Open(_) | Request::New(_));
        let before = self.engine.document().clone();
        self.compact = true;
        let result = self.execute(request);
        self.compact = false;
        let mut reply = result?;
        // A command that took a full snapshot itself still answers compactly.
        if let Some(object) = reply.as_object_mut() {
            for key in ["project", "drafts", "erc", "board_checks"] {
                object.remove(key);
            }
        }
        if replaced {
            reply["summary"] = self.summary()?;
        } else {
            self.add_changes(&before, &mut reply)?;
        }
        Ok(reply)
    }

    /// The document state a mutation answers with, before its own fields.
    pub(super) fn document_reply(&self) -> Value {
        if self.compact {
            json!({
                "revision": self.engine.revision(),
                "can_undo": self.engine.can_undo(),
                "can_redo": self.engine.can_redo(),
            })
        } else {
            self.snapshot()
        }
    }

    /// `project.inspect`: one part of the snapshot, the snapshot, or in a
    /// compact reply a summary.
    pub(super) fn inspect(&self, params: InspectParams) -> Result<Value> {
        if let Some(at) = params.at {
            let snapshot = self.snapshot();
            let value = snapshot.pointer(&at).ok_or_else(|| {
                sparksmith_core::Error::NotFound(format!("nothing at {at} in the snapshot"))
            })?;
            return Ok(json!({"revision": self.engine.revision(), "at": at, "value": value}));
        }
        if self.compact {
            let mut reply = self.document_reply();
            reply["summary"] = self.summary()?;
            return Ok(reply);
        }
        Ok(self.snapshot())
    }

    /// Project name, object counts, drafts and open findings per code.
    fn summary(&self) -> Result<Value> {
        let project = self.engine.project();
        let value = serde_json::to_value(project)?;
        let mut counts = Map::new();
        count_collections(&value, "", &mut counts);
        let findings = design_findings(project);
        let mut by_code: BTreeMap<&str, u64> = BTreeMap::new();
        for finding in &findings {
            *by_code.entry(&finding.code).or_default() += 1;
        }
        let drafts = self.draft_reports(None, 0)?;
        Ok(json!({
            "name": project.name,
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "counts": counts,
            "drafts": drafts,
            "findings": {"open": findings.len(), "by_code": by_code},
        }))
    }

    /// Add `changes`, `findings` and `drafts` for what differs from `before`.
    fn add_changes(&mut self, before: &Document, reply: &mut Value) -> Result<()> {
        let after = self.engine.document().clone();
        let mut changes = Changes::default();
        let mut design = None;
        if !Arc::ptr_eq(&before.project, &after.project) {
            let old = match self.baseline.take() {
                Some(baseline) if Arc::ptr_eq(&baseline.project, &before.project) => baseline,
                _ => Baseline::of(&before.project)?,
            };
            let new = Baseline::of(&after.project)?;
            changes.diff("", &old.value, &new.value);
            design = Some(findings_delta(&old.findings, &new.findings));
            self.baseline = Some(new);
        }
        let mut drafts = None;
        if !Arc::ptr_eq(&before.drafts, &after.drafts) {
            let old = serde_json::to_value(before.drafts.as_ref())?;
            let new = serde_json::to_value(after.drafts.as_ref())?;
            let mut delta = Changes::default();
            delta.diff("drafts", &old, &new);
            let ids: Vec<Value> = delta
                .added
                .get("drafts.items")
                .into_iter()
                .chain(delta.changed.get("drafts.items"))
                .flatten()
                .cloned()
                .collect();
            if !ids.is_empty() {
                drafts = Some(self.draft_reports(Some(&ids), MAX_DRAFT_FINDINGS)?);
            }
            changes.merge(delta);
        }
        if let Some(changes) = changes.into_value() {
            reply["changes"] = changes;
        }
        if let Some(design) = design.filter(|d| d["new"] != json!([]) || d["resolved"] != json!({}))
        {
            reply["findings"] = design;
        }
        if let Some(drafts) = drafts {
            reply["drafts"] = Value::Array(drafts);
        }
        Ok(())
    }

    /// Each selected draft (all without `only`) as id, kind, name, whether it
    /// is publishable, its finding count and up to `listed` of its findings.
    fn draft_reports(&self, only: Option<&[Value]>, listed: usize) -> Result<Vec<Value>> {
        let items = &self.engine.drafts().items;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let QueryResult::Drafts(reports) = self.engine.query(Query::CheckDrafts {})? else {
            return Err(sparksmith_core::Error::Invalid(
                "component.check_drafts gave another report".into(),
            )
            .into());
        };
        let mut out = Vec::new();
        for (draft, report) in items.iter().zip(reports) {
            let draft = serde_json::to_value(draft)?;
            if only.is_some_and(|ids| identity(&draft).is_none_or(|id| !ids.contains(&id))) {
                continue;
            }
            let check = serde_json::to_value(QueryResult::Definition(report))?;
            let all = check["result"]["findings"].as_array();
            let findings: Vec<Value> = all
                .into_iter()
                .flatten()
                .take(listed)
                .map(|f| json!({"code": f["code"], "message": f["message"], "blocks": f["blocks"]}))
                .collect();
            let mut row = json!({
                "id": draft["id"],
                "kind": draft["definition"]["kind"],
                "name": draft["meta"]["name"],
                "publishable": check["result"]["publishable"],
                "finding_count": all.map_or(0, Vec::len),
            });
            if listed > 0 {
                row["findings"] = Value::Array(findings);
            }
            out.push(row);
        }
        Ok(out)
    }
}

/// Remove `verbose` from a request and from its parameters; true when either
/// asked for the snapshot.
pub(crate) fn take_verbose(request: &mut Value) -> bool {
    fn take(value: &mut Value) -> bool {
        value
            .as_object_mut()
            .and_then(|object| object.remove("verbose"))
            .is_some_and(|verbose| verbose == true)
    }
    let outer = take(request);
    let inner = request.get_mut("params").is_some_and(take);
    outer || inner
}

/// The identity of a collection element: its `id` (with `revision` for a
/// published library record), or the `instance` a placement belongs to, or
/// an asset's `hash`.
fn identity(element: &Value) -> Option<Value> {
    let object = element.as_object()?;
    if let Some(id) = object.get("id") {
        return Some(match object.get("revision") {
            Some(revision) => json!({"id": id, "revision": revision}),
            None => id.clone(),
        });
    }
    object
        .get("instance")
        .or_else(|| object.get("hash"))
        .cloned()
}

#[derive(Default)]
struct Changes {
    added: BTreeMap<String, Vec<Value>>,
    changed: BTreeMap<String, Vec<Value>>,
    removed: BTreeMap<String, Vec<Value>>,
    fields: Vec<String>,
}

impl Changes {
    fn diff(&mut self, path: &str, before: &Value, after: &Value) {
        if before == after {
            return;
        }
        match (before, after) {
            (Value::Object(old), Value::Object(new)) => {
                let empty = Value::Array(Vec::new());
                for key in old
                    .keys()
                    .chain(new.keys().filter(|k| !old.contains_key(*k)))
                {
                    // Allocators move with every new object and say nothing.
                    if key.starts_with("next") {
                        continue;
                    }
                    let inner = join(path, key);
                    match (old.get(key), new.get(key)) {
                        (Some(a), Some(b)) => self.diff(&inner, a, b),
                        // An empty collection is left out when serialized.
                        (Some(a @ Value::Array(_)), None) => self.diff(&inner, a, &empty),
                        (None, Some(b @ Value::Array(_))) => self.diff(&inner, &empty, b),
                        _ => self.fields.push(inner),
                    }
                }
            }
            (Value::Array(old), Value::Array(new)) => {
                if !self.diff_collection(path, old, new) {
                    self.fields.push(path.to_owned());
                }
            }
            _ => self.fields.push(path.to_owned()),
        }
    }

    /// Compare two arrays element by element when every element has a unique
    /// identity. False when they do not, or when only the order differs.
    fn diff_collection(&mut self, path: &str, old: &[Value], new: &[Value]) -> bool {
        let keyed = |elements: &[Value]| -> Option<HashMap<String, usize>> {
            let mut map = HashMap::with_capacity(elements.len());
            for (index, element) in elements.iter().enumerate() {
                if map.insert(identity(element)?.to_string(), index).is_some() {
                    return None;
                }
            }
            Some(map)
        };
        let (Some(old_keys), Some(new_keys)) = (keyed(old), keyed(new)) else {
            return false;
        };
        let mut any = false;
        let mut record = |list: &mut BTreeMap<String, Vec<Value>>, element: &Value| {
            if let Some(id) = identity(element) {
                list.entry(path.to_owned()).or_default().push(id);
            }
            any = true;
        };
        for element in new {
            let key = identity(element).map(|id| id.to_string());
            match key.and_then(|key| old_keys.get(&key).copied()) {
                None => record(&mut self.added, element),
                Some(index) => {
                    if old.get(index) != Some(element) {
                        record(&mut self.changed, element);
                    }
                }
            }
        }
        for element in old {
            let key = identity(element).map(|id| id.to_string());
            if !key.is_some_and(|key| new_keys.contains_key(&key)) {
                record(&mut self.removed, element);
            }
        }
        any
    }

    fn merge(&mut self, other: Changes) {
        self.added.extend(other.added);
        self.changed.extend(other.changed);
        self.removed.extend(other.removed);
        self.fields.extend(other.fields);
    }

    /// The reply field, or `None` when nothing changed. At most [`MAX_IDS`]
    /// ids are listed; `omitted` counts the rest.
    fn into_value(self) -> Option<Value> {
        let mut budget = MAX_IDS;
        let mut omitted = 0;
        let mut out = Map::new();
        for (name, lists) in [
            ("added", self.added),
            ("changed", self.changed),
            ("removed", self.removed),
        ] {
            if lists.is_empty() {
                continue;
            }
            let mut listed = Map::new();
            for (path, mut ids) in lists {
                let keep = ids.len().min(budget);
                omitted += ids.len() - keep;
                budget -= keep;
                ids.truncate(keep);
                listed.insert(path, Value::Array(ids));
            }
            out.insert(name.to_owned(), Value::Object(listed));
        }
        if !self.fields.is_empty() {
            out.insert("fields".to_owned(), json!(self.fields));
        }
        if omitted > 0 {
            out.insert("omitted".to_owned(), json!(omitted));
        }
        (!out.is_empty()).then_some(Value::Object(out))
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// How many elements each collection of the project holds, by dotted path.
fn count_collections(value: &Value, path: &str, counts: &mut Map<String, Value>) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (key, inner) in object {
        match inner {
            Value::Array(items) if items.iter().all(|i| identity(i).is_some()) => {
                counts.insert(join(path, key), json!(items.len()));
            }
            Value::Object(_) => count_collections(inner, &join(path, key), counts),
            _ => {}
        }
    }
}

/// Which findings appeared and which went: findings compare by code, message
/// and objects, so a repeated finding counts each time it occurs.
fn findings_delta(old: &[Finding], new: &[Finding]) -> Value {
    fn key(finding: &Finding) -> String {
        format!(
            "{}\0{}\0{:?}",
            finding.code, finding.message, finding.objects
        )
    }
    let mut remaining: HashMap<String, (usize, &str)> = HashMap::with_capacity(old.len());
    for finding in old {
        remaining
            .entry(key(finding))
            .or_insert((0, &finding.code))
            .0 += 1;
    }
    let mut appeared = Vec::new();
    for finding in new {
        match remaining.get_mut(&key(finding)) {
            Some((count, _)) if *count > 0 => *count -= 1,
            _ => appeared.push(finding),
        }
    }
    let mut resolved: BTreeMap<&str, usize> = BTreeMap::new();
    for (count, code) in remaining.values() {
        if *count > 0 {
            *resolved.entry(code).or_default() += count;
        }
    }
    let listed: Vec<Value> = appeared
        .iter()
        .take(MAX_FINDINGS)
        .map(|f| {
            let mut row = json!({"code": f.code, "message": f.message, "objects": f.objects});
            if let Some(stage) = f.blocks {
                row["blocks"] = json!(stage);
            }
            row
        })
        .collect();
    let mut out = json!({"new": listed, "resolved": resolved, "open": new.len()});
    if appeared.len() > MAX_FINDINGS {
        out["omitted"] = json!(appeared.len() - MAX_FINDINGS);
    }
    out
}

#[cfg(test)]
pub(super) mod tests_support {
    use super::*;

    pub fn changes_of(before: &Value, after: &Value) -> Value {
        let mut changes = Changes::default();
        changes.diff("", before, after);
        changes.into_value().unwrap_or(Value::Null)
    }

    pub fn delta(old: &[Finding], new: &[Finding]) -> Value {
        findings_delta(old, new)
    }
}
