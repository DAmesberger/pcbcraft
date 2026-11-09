//! The five tools of `docs/mcp-surface.md` §3.1: `find` and `describe` over
//! the catalog, and `query`, `apply` and `act`, one verb per permission
//! class, so each tool's annotations are exact. The catalog's entries are
//! data behind them ([`super::catalog`]); a verb reaches an entry only
//! through its own kind, and every call becomes one request of the JSON
//! contract that runs on the desk through the gate ([`super::desk`]).
//!
//! Input schemas are generated from the argument types here, and `apply`'s
//! operation names from the catalog (the union outline of §3.1, Q3).
//! Failures of the call itself (bad arguments, a name of another kind, a
//! batch that cannot be one) are tool results with `isError`, like a
//! refusal of the gate, so the model reads their code.
use super::catalog::{self, Unreachable};
use super::desk::{self, Desk};
use super::outcome;
use crate::session::Request;
use crate::session::discovery::{self, Call};
use crate::session::metadata::{self, EntryMeta, Kind};
use pcbcraft_core::{Class, MAX_BATCH};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::sync::{Arc, OnceLock};

/// The longest `find` text, in characters.
pub const MAX_TEXT: usize = 200;
/// The longest catalog name or definition a call may give, in bytes.
pub const MAX_NAME: usize = 128;
/// `find`'s hits by default, and at most.
pub const FIND_LIMIT: usize = 10;
pub const MAX_FIND_LIMIT: usize = 50;

pub const FIND: &str = "find";
pub const DESCRIBE: &str = "describe";
pub const QUERY: &str = "query";
pub const APPLY: &str = "apply";
pub const ACT: &str = "act";

/// The requests that open a project of their own rather than act on one.
const OPENS: &[&str] = &["project.new", "project.open"];

/// `find` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindArgs {
    /// What it should do, in plain words.
    pub text: String,
    #[serde(default)]
    pub kind: Option<Kind>,
    /// 1 to 50; 10 by default.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `describe` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeArgs {
    /// A catalog name.
    pub name: String,
    /// A definition named in more_defs.
    #[serde(default)]
    pub def: Option<String>,
}

/// `query` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryArgs {
    /// The handle from the last reply.
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

/// `act` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunArgs {
    /// The handle from the last reply.
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    /// For an action that changes the document.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

impl From<QueryArgs> for RunArgs {
    fn from(args: QueryArgs) -> Self {
        Self {
            project: args.project,
            name: args.name,
            params: args.params,
            expected_revision: None,
        }
    }
}

/// `apply` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyArgs {
    /// The handle from the last reply.
    #[serde(default)]
    pub project: Option<String>,
    pub expected_revision: u64,
    /// 1 to 256, in order.
    pub operations: Vec<Step>,
    /// Report the change; keep nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// One step of `apply`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub op: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

/// The five tools, in their fixed order.
pub fn tools() -> Vec<Tool> {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(build).clone()
}

fn build() -> Vec<Tool> {
    let read = metadata::annotations(Class::Read, false);
    vec![
        tool(
            FIND,
            "Find a catalog entry",
            "Searches the catalog by what an entry does. A hit's kind names its tool: query, operation (apply) or action (act).",
            schema::<FindArgs>(),
            read,
        ),
        tool(
            DESCRIBE,
            "Describe a catalog entry",
            "One entry's exact params schema and the call that sends it.",
            schema::<DescribeArgs>(),
            read,
        ),
        tool(
            QUERY,
            "Read a project",
            "Runs a read: project.inspect, project.issues, library.search, project.bom and the rest.",
            schema::<QueryArgs>(),
            read,
        ),
        tool(
            APPLY,
            "Edit a project",
            "Applies operations as one undo step, all or nothing: a failure or a stale expected_revision changes nothing. Undo, redo, updates and folder imports go alone.",
            apply_schema(),
            metadata::annotations(Class::Edit, false),
        ),
        tool(
            ACT,
            "Files, exports and publishing",
            "Runs an action (files, exports, publishing); a person approves it. project.new and project.open {path} return a new project's handle, valid until the server exits.",
            schema::<RunArgs>(),
            metadata::annotations(Class::Ask, true),
        ),
    ]
}

fn tool(
    name: &'static str,
    title: &str,
    description: &'static str,
    schema: JsonObject,
    hints: Option<metadata::Annotations>,
) -> Tool {
    let mut tool = Tool::new(name, description, Arc::new(schema));
    tool.title = Some(title.to_owned());
    tool.annotations = hints.map(|hints| {
        let mut annotations = ToolAnnotations::new()
            .read_only(hints.read_only)
            .open_world(hints.open_world);
        // False is the protocol's default.
        if hints.idempotent {
            annotations = annotations.idempotent(true);
        }
        if let Some(destructive) = hints.destructive {
            annotations = annotations.destructive(destructive);
        }
        annotations
    });
    tool
}

/// The input schema of `T`, without what costs tokens and tells a model
/// nothing: the generator's root keys, `default` and `format`, `null` in an
/// optional field's type, and references to a definition, which is inlined.
fn schema<T: JsonSchema>() -> JsonObject {
    let mut value = serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null);
    let defs = value
        .as_object_mut()
        .and_then(|object| object.remove("$defs"))
        .unwrap_or(Value::Null);
    compact(&mut value, &defs, 0);
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
        object.remove("description");
    }
    match value {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

/// How deep [`compact`] inlines; the argument types nest two levels.
const MAX_DEPTH: usize = 8;

fn compact(value: &mut Value, defs: &Value, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Value::Array(items) = value {
        for item in items {
            compact(item, defs, depth.saturating_add(1));
        }
        return;
    }
    let Value::Object(object) = value else {
        return;
    };
    for key in ["default", "format", "minimum"] {
        object.remove(key);
    }
    // Any property is allowed unless a schema says otherwise.
    if object.get("additionalProperties") == Some(&Value::Bool(true)) {
        object.remove("additionalProperties");
    }
    // An optional field: what it is when given.
    if let Some(Value::Array(options)) = object.get("anyOf") {
        let given: Vec<&Value> = options.iter().filter(|o| o["type"] != "null").collect();
        if let [only] = given.as_slice() {
            let only = (*only).clone();
            object.remove("anyOf");
            if let Value::Object(inner) = only {
                for (key, inner) in inner {
                    object.entry(key).or_insert(inner);
                }
            }
        }
    }
    if let Some(Value::Array(types)) = object.get("type") {
        let given: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
        if let [only] = given.as_slice() {
            object.insert("type".into(), only.clone());
        }
    }
    if let Some(name) = object.remove("$ref").and_then(|r| {
        r.as_str()
            .and_then(|r| r.strip_prefix("#/$defs/"))
            .map(str::to_owned)
    }) && let Some(Value::Object(def)) = defs.get(&name)
    {
        for (key, inner) in def {
            if key != "description" {
                object.entry(key.clone()).or_insert(inner.clone());
            }
        }
    }
    for inner in object.values_mut() {
        compact(inner, defs, depth.saturating_add(1));
    }
}

/// `apply`'s schema, with every operation name a step's `op` takes.
fn apply_schema() -> JsonObject {
    let mut schema = schema::<ApplyArgs>();
    let names: Vec<&str> = catalog::reachable()
        .filter(|e| e.kind == Some(Kind::Operation))
        .map(|e| e.name.as_str())
        .collect();
    let step = schema
        .get_mut("properties")
        .and_then(|p| p.get_mut("operations"))
        .and_then(|o| o.get_mut("items"))
        .and_then(Value::as_object_mut);
    if let Some(step) = step {
        step.remove("description");
        if let Some(op) = step.get_mut("properties").and_then(|p| p.get_mut("op")) {
            *op = json!({"type": "string", "enum": names});
        }
    }
    schema
}

/// Runs the tool `name` with `arguments` on the desk. `Err` only for an
/// unknown tool or a stopped session thread.
pub async fn call(
    handle: &desk::Handle,
    name: &str,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, ErrorData> {
    let arguments = Value::Object(arguments.unwrap_or_default());
    let reply = match name {
        FIND => match parse::<FindArgs>(arguments) {
            Ok(args) => find(args),
            Err(failure) => failure,
        },
        DESCRIBE => match parse::<DescribeArgs>(arguments) {
            Ok(args) => handle.run(move |d| describe(d, args)).await?,
            Err(failure) => failure,
        },
        QUERY => match parse::<QueryArgs>(arguments) {
            Ok(args) => {
                handle
                    .run(move |d| run(d, Kind::Query, args.into()))
                    .await?
            }
            Err(failure) => failure,
        },
        APPLY => match parse::<ApplyArgs>(arguments) {
            Ok(args) => handle.run(move |d| apply(d, args)).await?,
            Err(failure) => failure,
        },
        ACT => match parse::<RunArgs>(arguments) {
            Ok(args) => handle.run(move |d| run(d, Kind::Action, args)).await?,
            Err(failure) => failure,
        },
        other => {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {other}: the tools are find, describe, query, apply and act"),
                None,
            ));
        }
    };
    Ok(outcome(&reply))
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, Value> {
    serde_json::from_value(arguments).map_err(|error| {
        desk::failure(
            "arguments.invalid",
            format!("{error}: the tool's input schema lists its arguments"),
        )
    })
}

fn too_long(what: &str, text: &str, max: usize) -> Option<Value> {
    (text.len() > max).then(|| {
        desk::failure(
            "arguments.invalid",
            format!("{what} is {} bytes; at most {max}", text.len()),
        )
    })
}

/// `find`: the reachable entries whose name, title or summary match the
/// most words of `text`, best first and then in catalog order.
fn find(args: FindArgs) -> Value {
    if args.text.chars().count() > MAX_TEXT {
        return desk::failure(
            "arguments.invalid",
            format!("text is longer than {MAX_TEXT} characters"),
        );
    }
    let limit = args.limit.unwrap_or(FIND_LIMIT);
    if !(1..=MAX_FIND_LIMIT).contains(&limit) {
        return desk::failure(
            "arguments.invalid",
            format!("limit is {limit}; give 1 to {MAX_FIND_LIMIT}"),
        );
    }
    let wanted = words(&args.text);
    let mut scored: Vec<(usize, &EntryMeta)> = catalog::reachable()
        .filter(|e| args.kind.is_none_or(|kind| e.kind == Some(kind)))
        .filter_map(|e| {
            let score = score(&wanted, &args.text, e);
            (score > 0).then_some((score, e))
        })
        .collect();
    // Stable: equal scores keep catalog order.
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let more = scored.len().saturating_sub(limit);
    let hits: Vec<Value> = scored
        .into_iter()
        .take(limit)
        .map(|(_, e)| {
            json!({
                "name": e.name,
                "kind": e.kind,
                "class": e.class,
                "title": e.title,
                "summary": e.summary,
            })
        })
        .collect();
    let mut result = json!({"hits": hits});
    if more > 0 {
        result["more"] = json!(more);
    }
    if hits_empty(&result) {
        result["hint"] = json!("nothing matched: try other words, or fewer");
    }
    json!({"ok": true, "result": result})
}

fn hits_empty(result: &Value) -> bool {
    result["hits"].as_array().is_none_or(Vec::is_empty)
}

/// Lower-case words of letters and digits.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether two words are the same but for an ending ("via", "vias").
fn alike(a: &str, b: &str) -> bool {
    a == b || (a.len().min(b.len()) >= 3 && (a.starts_with(b) || b.starts_with(a)))
}

fn score(wanted: &[String], text: &str, entry: &EntryMeta) -> usize {
    if entry.name == text.trim() {
        return 1000;
    }
    let name = words(&entry.name);
    let title = words(entry.title);
    let summary = words(&entry.summary);
    wanted
        .iter()
        .map(|w| {
            let has = |list: &[String]| list.iter().any(|x| alike(x, w));
            usize::from(has(&name)) * 3 + usize::from(has(&title)) * 2 + usize::from(has(&summary))
        })
        .sum()
}

/// `describe`: `project.schema` of one reachable entry, through the gate,
/// with the call that sends it on this surface instead of today's tools.
fn describe(desk: &mut Desk, args: DescribeArgs) -> Value {
    if let Some(failure) = too_long("name", &args.name, MAX_NAME) {
        return failure;
    }
    if let Some(failure) = args
        .def
        .as_deref()
        .and_then(|d| too_long("def", d, MAX_NAME))
    {
        return failure;
    }
    if let Some(topic) = args.name.strip_prefix("guide:") {
        return desk::failure(
            "guide.unknown",
            format!("there is no guide {topic}: this server serves no guides yet"),
        );
    }
    let Some(entry) = catalog::entry(&args.name) else {
        return Unreachable::Unknown(args.name).reply();
    };
    let request =
        json!({"command": "project.schema", "params": {"name": args.name, "def": args.def}});
    let mut reply = desk.respond_catalog(request);
    if reply["ok"] != true || args.def.is_some() {
        return reply;
    }
    let Some(found) = reply["result"]["matches"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["name"] == entry.name.as_str()))
        .cloned()
    else {
        return Unreachable::Unknown(entry.name.clone()).reply();
    };
    let mut described = found;
    if let Some(object) = described.as_object_mut() {
        object.remove("send");
        object.remove("asks_with_path");
        object.remove("kind");
        object.insert("kind".into(), json!(entry.kind));
        object.insert("title".into(), json!(entry.title));
        object.insert("summary".into(), json!(entry.summary));
        object.insert("call".into(), example(entry));
        if entry.person.is_some() {
            object.insert("person".into(), json!(entry.person));
        }
    }
    reply["result"] = described;
    reply
}

/// How this surface sends `entry`.
fn example(entry: &EntryMeta) -> Value {
    let name = entry.name.as_str();
    match entry.kind {
        Some(Kind::Operation) => json!({"tool": APPLY, "arguments": {
            "project": "<project>", "expected_revision": "<revision>",
            "operations": [{"op": name, "params": "<params>"}]}}),
        Some(Kind::Action) if OPENS.contains(&name) => {
            json!({"tool": ACT, "arguments": {"name": name, "params": "<params>"}})
        }
        Some(Kind::Action) => json!({"tool": ACT, "arguments": {
            "project": "<project>", "name": name, "params": "<params>"}}),
        _ => json!({"tool": QUERY, "arguments": {
            "project": "<project>", "name": name, "params": "<params>"}}),
    }
}

/// `query` and `act`: one entry of `kind` as one request.
fn run(desk: &mut Desk, kind: Kind, args: RunArgs) -> Value {
    if let Some(failure) = too_long("name", &args.name, MAX_NAME) {
        return failure;
    }
    let entry = match catalog::resolve(kind, &args.name) {
        Ok(entry) => entry,
        Err(unreachable) => return unreachable.reply(),
    };
    if OPENS.contains(&entry.name.as_str()) {
        if args.project.is_some() {
            return desk::failure(
                "project.opens_new",
                format!(
                    "{} opens another project and returns its handle: leave project out",
                    entry.name
                ),
            );
        }
        return desk.open(json!({"command": entry.name, "params": args.params}));
    }
    let request = match request(entry, args.params, args.expected_revision) {
        Ok(request) => request,
        Err(failure) => return failure,
    };
    desk.respond(args.project.as_deref(), request)
}

/// `apply`: the steps as one request, `project.apply_all` for document
/// operations, or the one command-level edit (undo, an update) alone.
fn apply(desk: &mut Desk, args: ApplyArgs) -> Value {
    let count = args.operations.len();
    if count == 0 || count > MAX_BATCH {
        return desk::failure(
            "arguments.invalid",
            format!("operations has {count} steps; give 1 to {MAX_BATCH}"),
        );
    }
    let mut operations = Vec::with_capacity(count);
    let mut alone = None;
    for step in args.operations {
        if let Some(failure) = too_long("op", &step.op, MAX_NAME) {
            return failure;
        }
        let entry = match catalog::resolve(Kind::Operation, &step.op) {
            Ok(entry) => entry,
            Err(unreachable) => return unreachable.reply(),
        };
        match discovery::call(&entry.name, Value::Object(step.params.clone())) {
            Some(Call::Operation(operation)) => operations.push(operation),
            _ if count == 1 => alone = Some((entry, step.params)),
            _ => {
                return desk::failure(
                    "apply.alone",
                    format!(
                        "{} is not a document operation, so it cannot share a batch: send it as the only step",
                        entry.name
                    ),
                );
            }
        }
    }
    let request = match alone {
        Some((entry, params)) => match request(entry, params, Some(args.expected_revision)) {
            Ok(request) => request,
            Err(failure) => return failure,
        },
        None => json!({"command": "project.apply_all", "params": {
            "expected_revision": args.expected_revision, "operations": operations}}),
    };
    if args.dry_run {
        desk.rehearse(args.project.as_deref(), request)
    } else {
        desk.respond(args.project.as_deref(), request)
    }
}

/// The request that runs `entry` with `params`: a command as itself (with
/// `expected_revision` where its schema takes one), an operation inside
/// `project.apply_all`, a query inside `project.query`.
fn request(
    entry: &EntryMeta,
    mut params: Map<String, Value>,
    expected: Option<u64>,
) -> Result<Value, Value> {
    let name = entry.name.as_str();
    match discovery::call(name, Value::Object(params.clone())) {
        Some(Call::Operation(operation)) => {
            let Some(expected) = expected else {
                return Err(desk::failure(
                    "arguments.invalid",
                    format!(
                        "{name} changes the document: give expected_revision from the last reply"
                    ),
                ));
            };
            Ok(json!({"command": "project.apply_all", "params": {
                "expected_revision": expected, "operations": [operation]}}))
        }
        Some(Call::Query(query)) => Ok(json!({"command": "project.query", "params": query})),
        Some(Call::Command) | None => {
            if let Some(expected) = expected
                && takes_revision(name)
            {
                params.insert("expected_revision".into(), json!(expected));
            }
            Ok(json!({"command": name, "params": params}))
        }
    }
}

/// Whether the command `name`'s parameters name the revision they expect.
fn takes_revision(name: &str) -> bool {
    Request::catalog_shared()
        .iter()
        .find(|c| c.id == name)
        .is_some_and(|c| c.params["properties"].get("expected_revision").is_some())
}
