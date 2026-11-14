//! Schemas an LLM client can afford: a short input schema per tool, and
//! `project.schema` for the exact schema of one operation, query or command
//! on demand. Everything is derived from the generated catalog, so a command
//! added to the engine is listed and served without changing this module.
use super::{CommandInfo, Request, SchemaParams};
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use sparksmith_core::permission::EntryDecl;
use sparksmith_core::{Class, Operation, Query};
use std::collections::{BTreeSet, VecDeque};
use std::sync::OnceLock;

/// A tool's input schema is listed whole up to this many bytes.
const WHOLE_SCHEMA: usize = 1500;
/// One property of a shortened schema stays whole up to this many bytes,
/// definitions included.
const WHOLE_PROPERTY: usize = 400;
/// Definitions one `project.schema` entry carries; the rest are named in
/// `more_defs` and read with `def`.
const MAX_DEFS_BYTES: usize = 24 * 1024;
/// Where an index row cuts a summary; the metadata tests keep every summary
/// within it ([`super::metadata::MAX_SUMMARY`]).
const MAX_SUMMARY: usize = super::metadata::MAX_SUMMARY;

/// One addressable schema: a command of the request catalog, or a variant of
/// `project.apply`'s operation or `project.query`'s query.
pub(super) struct Entry {
    /// `command`, `operation` or `query`: where in the catalog it is.
    pub(super) kind: &'static str,
    pub(super) name: String,
    /// Its title from the class listing beside its enum.
    pub(super) title: &'static str,
    /// The operation's editor group, for an operation.
    editor: Option<String>,
    /// The catalog schema whose `$defs` the variant refers to.
    pub(super) root: &'static Value,
    pub(super) schema: Value,
    /// How to send it, with `<params>` where the schema's value goes.
    pub(super) send: Option<Value>,
    /// Its permission class. A missing one reads as Never and fails the
    /// gate's tests.
    pub(super) class: Class,
    /// A command that can name a path, which makes it at least Ask.
    pub(super) takes_path: bool,
    /// Its declaration beside its enum.
    pub(super) decl: Option<&'static EntryDecl>,
}

/// The declaration of `name` in `entries`; none fails the metadata tests
/// and reads as Never.
fn declared(
    entries: impl IntoIterator<Item = &'static EntryDecl>,
    name: &str,
) -> Option<&'static EntryDecl> {
    sparksmith_core::permission::find(entries, name)
}

pub(super) fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(build_entries)
}

fn build_entries() -> Vec<Entry> {
    let mut entries = Vec::new();
    for info in Request::catalog_shared() {
        let mut schema = info.params.clone();
        strip_root(&mut schema);
        if let Some(object) = schema.as_object_mut() {
            object.remove("$defs");
        }
        entries.push(Entry {
            kind: "command",
            name: info.id.to_owned(),
            title: info.title,
            editor: None,
            root: &info.params,
            schema,
            send: None,
            class: info.class,
            takes_path: info.takes_path,
            decl: info.decl,
        });
        match info.id {
            "project.apply" => operation_entries(&info.params, &mut entries),
            "project.query" => query_entries(&info.params, &mut entries),
            _ => {}
        }
    }
    entries
}

fn operation_entries(root: &'static Value, entries: &mut Vec<Entry>) {
    let operation = resolve(&root["properties"]["operation"], root);
    let Some((_, editors)) = union_variants(operation) else {
        return;
    };
    for (editor, variant) in editors {
        let content = resolve(&variant["properties"]["operation"], root);
        match union_variants(content) {
            Some((_, commands)) => {
                for (command, schema) in commands {
                    let decl = declared(Operation::entries(), command);
                    entries.push(Entry {
                        kind: "operation",
                        send: Some(json!({
                            "expected_revision": "<revision>",
                            "operation": {"editor": editor, "operation": {"command": command, "params": "<params>"}},
                        })),
                        name: command.to_owned(),
                        title: decl.map_or("", |d| d.title),
                        editor: Some(editor.to_owned()),
                        root,
                        schema: schema.clone(),
                        class: decl.map_or(Class::Never, |d| d.class),
                        takes_path: false,
                        decl,
                    });
                }
            }
            // A variant that is not itself a command family, such as
            // `Update`: addressed by its editor's name in lower case.
            None => {
                let decl = declared(Operation::entries(), &editor.to_lowercase());
                entries.push(Entry {
                    kind: "operation",
                    send: Some(json!({
                        "expected_revision": "<revision>",
                        "operation": {"editor": editor, "operation": "<params>"},
                    })),
                    name: editor.to_lowercase(),
                    title: decl.map_or("", |d| d.title),
                    editor: Some(editor.to_owned()),
                    root,
                    schema: content.clone(),
                    class: decl.map_or(Class::Never, |d| d.class),
                    takes_path: false,
                    decl,
                });
            }
        }
    }
}

fn query_entries(root: &'static Value, entries: &mut Vec<Entry>) {
    let Some((_, queries)) = union_variants(root) else {
        return;
    };
    for (query, schema) in queries {
        let decl = declared(Query::ENTRIES, query);
        entries.push(Entry {
            kind: "query",
            name: query.to_owned(),
            title: decl.map_or("", |d| d.title),
            editor: None,
            root,
            schema: schema.clone(),
            send: Some(json!({"query": query, "params": "<params>"})),
            class: decl.map_or(Class::Never, |d| d.class),
            takes_path: false,
            decl,
        });
    }
}

/// How catalog entry `name` is sent, filled with `params`, from the same
/// table `project.schema`'s `send` comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// A request of its own: `{"command": name, "params": params}`.
    Command,
    /// One of `project.apply`'s operations, as the operation value.
    Operation(Value),
    /// One of `project.query`'s queries, as the query value.
    Query(Value),
}

/// [`Call`] for the entry named `name`, or `None` when no entry is.
pub fn call(name: &str, params: Value) -> Option<Call> {
    let entry = entries().iter().find(|e| e.name == name)?;
    let fill = |template: &Value| fill_params(template, &params);
    Some(match (entry.kind, &entry.send) {
        ("operation", Some(send)) => Call::Operation(fill(&send["operation"])),
        ("query", Some(send)) => Call::Query(fill(send)),
        _ => Call::Command,
    })
}

/// `template` with its `"<params>"` placeholder replaced by `params`.
fn fill_params(template: &Value, params: &Value) -> Value {
    match template {
        Value::String(text) if text == "<params>" => params.clone(),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, inner)| (key.clone(), fill_params(inner, params)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `project.schema`: the index without a name, the schema of `name`, or one
/// definition of that schema with `def`.
pub(super) fn schema_reply(params: SchemaParams) -> Result<Value> {
    let Some(name) = params.name else {
        if params.def.is_some() {
            return Err(invalid("def needs the name of the schema it belongs to"));
        }
        return Ok(index());
    };
    let found: Vec<&Entry> = entries()
        .iter()
        .filter(|e| {
            e.name == name || (e.kind == "command" && super::super::mcp::tool_name(&e.name) == name)
        })
        .collect();
    if found.is_empty() {
        return Err(not_found(format!(
            "no operation, query or command is named {name}; project.schema without a name lists them"
        )));
    }
    if let Some(def) = params.def {
        let schema = found
            .iter()
            .find_map(|entry| entry.root["$defs"].get(&def).map(|s| (s, entry.root)));
        let Some((schema, root)) = schema else {
            return Err(not_found(format!("{name} has no definition {def}")));
        };
        let mut reply = json!({"name": name, "def": def, "schema": schema});
        add_defs(&mut reply, schema, root);
        return Ok(reply);
    }
    let matches: Vec<Value> = found.iter().map(|entry| describe(entry)).collect();
    Ok(json!({"name": name, "matches": matches}))
}

fn describe(entry: &Entry) -> Value {
    let mut out = json!({
        "kind": entry.kind,
        "name": entry.name,
        "class": entry.class,
        "schema": entry.schema,
    });
    if entry.takes_path {
        out["asks_with_path"] = json!(true);
    }
    if let Some(editor) = &entry.editor {
        out["editor"] = json!(editor);
    }
    if let Some(send) = &entry.send {
        out["send"] = send.clone();
    }
    add_defs(&mut out, &entry.schema, entry.root);
    out
}

/// Add the definitions `schema` reaches, nearest first, up to the budget.
/// What did not fit is named in `more_defs`.
fn add_defs(out: &mut Value, schema: &Value, root: &Value) {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    collect_refs(schema, &mut seen, &mut queue);
    let mut defs = Map::new();
    let mut more = Vec::new();
    let mut used = 0usize;
    while let Some(name) = queue.pop_front() {
        let Some(def) = root["$defs"].get(&name) else {
            continue;
        };
        let size = def.to_string().len();
        if used.saturating_add(size) > MAX_DEFS_BYTES {
            more.push(name);
            continue;
        }
        used = used.saturating_add(size);
        collect_refs(def, &mut seen, &mut queue);
        defs.insert(name, def.clone());
    }
    if !defs.is_empty() {
        out["$defs"] = Value::Object(defs);
    }
    if !more.is_empty() {
        out["more_defs"] = json!(more);
    }
}

fn collect_refs(value: &Value, seen: &mut BTreeSet<String>, queue: &mut VecDeque<String>) {
    match value {
        Value::Object(object) => {
            for (key, inner) in object {
                if key == "$ref" {
                    if let Some(name) = inner.as_str().and_then(def_name)
                        && seen.insert(name.to_owned())
                    {
                        queue.push_back(name.to_owned());
                    }
                } else {
                    collect_refs(inner, seen, queue);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_refs(i, seen, queue)),
        _ => {}
    }
}

fn def_name(reference: &str) -> Option<&str> {
    reference.strip_prefix("#/$defs/")
}

/// Every operation, query and command with one line about it. To stay small
/// a row names its permission class only where `classes` does not cover it.
fn index() -> Value {
    let row = |entry: &Entry| {
        let mut row = json!({"name": entry.name, "summary": summary_of(entry)});
        let usual = match entry.kind {
            "operation" => Some(Class::Edit),
            "query" => Some(Class::Read),
            _ => None,
        };
        if usual != Some(entry.class) {
            row["class"] = json!(entry.class);
        }
        if entry.takes_path {
            row["asks_with_path"] = json!(true);
        }
        row
    };
    let mut operations = Map::new();
    let mut queries = Vec::new();
    let mut commands = Vec::new();
    for entry in entries() {
        match (entry.kind, &entry.editor) {
            ("operation", Some(editor)) => {
                if let Some(list) = operations
                    .entry(editor.clone())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                {
                    list.push(row(entry));
                }
            }
            ("query", _) => queries.push(row(entry)),
            _ => commands.push(row(entry)),
        }
    }
    json!({
        "classes": {"operations": Class::Edit, "queries": Class::Read},
        "operations": operations,
        "queries": queries,
        "commands": commands,
    })
}

/// A command's summary, or the first sentence of an operation's or query's
/// doc comment.
pub(super) fn summary_of(entry: &Entry) -> String {
    let text = if entry.kind == "command" {
        Request::catalog_shared()
            .iter()
            .find(|c| c.id == entry.name)
            .map_or("", |c| c.summary)
    } else {
        entry.schema["description"].as_str().unwrap_or("")
    };
    first_sentence(text)
}

fn first_sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = flat.find(". ").map_or(flat.len(), |i| i.saturating_add(1));
    let line = flat.get(..end).unwrap_or(&flat);
    if line.chars().count() <= MAX_SUMMARY {
        return line.to_owned();
    }
    let cut: String = line.chars().take(MAX_SUMMARY).collect();
    format!("{cut}…")
}

/// The input schema `tools/list` shows for one command: whole when small;
/// otherwise its structure, with large parts replaced by what they accept in
/// outline and where the rest is served.
pub fn tool_schema(info: &CommandInfo) -> Value {
    let mut whole = info.params.clone();
    strip_root(&mut whole);
    let mut out = if whole.to_string().len() <= WHOLE_SCHEMA {
        whole
    } else {
        shorten(info)
    };
    out["properties"]["verbose"] = json!({"type": "boolean"});
    out
}

fn shorten(info: &CommandInfo) -> Value {
    let root = &info.params;
    if let Some(union) = union_outline(root, root, info.id) {
        return union;
    }
    let mut properties = Map::new();
    let mut kept = BTreeSet::new();
    let mut queue = VecDeque::new();
    for (key, property) in root["properties"].as_object().into_iter().flatten() {
        if closure_size(property, root) <= WHOLE_PROPERTY {
            collect_refs(property, &mut kept, &mut queue);
            properties.insert(key.clone(), property.clone());
        } else {
            properties.insert(key.clone(), outline(property, root, info.id));
        }
    }
    let mut defs = Map::new();
    while let Some(name) = queue.pop_front() {
        if let Some(def) = root["$defs"].get(&name) {
            collect_refs(def, &mut kept, &mut queue);
            defs.insert(name, def.clone());
        }
    }
    let mut out = json!({"type": "object", "properties": properties});
    copy(
        root,
        &mut out,
        &["description", "required", "additionalProperties"],
    );
    if !defs.is_empty() {
        out["$defs"] = Value::Object(defs);
    }
    out
}

/// A large property: its outline if it is a tagged union, else its type and
/// description, with where to read the rest.
fn outline(property: &Value, root: &Value, id: &str) -> Value {
    let target = resolve(property, root);
    if let Some(mut union) = union_outline(target, root, id) {
        if let Some(text) = property.get("description") {
            union["description"] = text.clone();
        }
        return union;
    }
    let text = property
        .get("description")
        .or_else(|| target.get("description"))
        .and_then(Value::as_str)
        .map_or_else(String::new, |text| format!("{} ", first_sentence(text)));
    let mut out =
        json!({"description": format!("{text}Schema: project_schema {{name: \"{id}\"}}.")});
    if let Some(kind) = target.get("type") {
        out["type"] = kind.clone();
    }
    out
}

/// A tagged union as its tag's values and an open content field that says
/// which names to pass to `project_schema`.
fn union_outline(schema: &Value, root: &Value, id: &str) -> Option<Value> {
    let (tag, variants) = union_variants(schema)?;
    let values: Vec<&str> = variants.iter().map(|(value, _)| *value).collect();
    let content = content_key(&variants, tag);
    let mut properties = Map::new();
    properties.insert(tag.to_owned(), json!({"type": "string", "enum": values}));
    let mut required = vec![tag.to_owned()];
    if let Some(key) = content {
        let mut nested = Vec::new();
        let mut names = Vec::new();
        for (value, variant) in &variants {
            match union_variants(resolve(&variant["properties"][key], root)) {
                Some((_, inner)) => {
                    let list: Vec<&str> = inner.iter().map(|(name, _)| *name).collect();
                    nested.push(format!("{value}: {}", list.join(", ")));
                }
                None => names.push(*value),
            }
        }
        let text = if nested.is_empty() {
            format!(
                "The parameters of the chosen {tag}. Schema: project_schema {{name: <the {tag}>}}."
            )
        } else {
            let mut groups = nested;
            groups.extend(
                names
                    .iter()
                    .map(|name| format!("{name}: {}", name.to_lowercase())),
            );
            format!(
                "Inner command by {tag}: {}. Schema of one: project_schema {{name: <the command>}}.",
                groups.join("; ")
            )
        };
        properties.insert(key.to_owned(), json!({"description": text}));
        if variants.iter().all(|(_, v)| is_required(v, key)) {
            required.push(key.to_owned());
        }
    } else {
        // Fields sit beside the tag: nothing to list without the schema.
        let text =
            format!("Other fields depend on {tag}. Schema: project_schema {{name: \"{id}\"}}.");
        return Some(json!({
            "type": "object",
            "description": text,
            "properties": properties,
            "required": required,
        }));
    }
    Some(json!({"type": "object", "properties": properties, "required": required}))
}

/// `(tag key, [(tag value, variant)])` when every `oneOf` variant is an object
/// that names itself by one shared `const` property.
fn union_variants(schema: &Value) -> Option<(&str, Vec<(&str, &Value)>)> {
    let variants = schema.get("oneOf")?.as_array()?;
    let first = variants.first()?;
    let tag = first["properties"]
        .as_object()?
        .iter()
        .find(|(_, property)| property.get("const").is_some())
        .map(|(key, _)| key.as_str())?;
    let mut out = Vec::with_capacity(variants.len());
    for variant in variants {
        out.push((variant["properties"][tag]["const"].as_str()?, variant));
    }
    Some((tag, out))
}

/// The one key beside the tag, when every variant has only that one.
fn content_key<'a>(variants: &[(&str, &'a Value)], tag: &str) -> Option<&'a str> {
    let mut key = None;
    for (_, variant) in variants {
        for name in variant["properties"].as_object()?.keys() {
            if name == tag {
                continue;
            }
            match key {
                None => key = Some(name.as_str()),
                Some(existing) if existing == name => {}
                Some(_) => return None,
            }
        }
    }
    key
}

fn is_required(variant: &Value, key: &str) -> bool {
    variant["required"]
        .as_array()
        .is_some_and(|required| required.iter().any(|name| name == key))
}

/// Follow a `$ref`, or the one reference of an optional (`anyOf` with null).
fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    let reference = schema.get("$ref").or_else(|| {
        schema
            .get("anyOf")?
            .as_array()?
            .iter()
            .find_map(|option| option.get("$ref"))
    });
    reference
        .and_then(Value::as_str)
        .and_then(def_name)
        .and_then(|name| root["$defs"].get(name))
        .unwrap_or(schema)
}

fn closure_size(schema: &Value, root: &Value) -> usize {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    collect_refs(schema, &mut seen, &mut queue);
    let mut size = schema.to_string().len();
    while let Some(name) = queue.pop_front() {
        if let Some(def) = root["$defs"].get(&name) {
            size = size.saturating_add(def.to_string().len());
            collect_refs(def, &mut seen, &mut queue);
        }
    }
    size
}

fn strip_root(schema: &mut Value) {
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
}

fn copy(from: &Value, to: &mut Value, keys: &[&str]) {
    for key in keys {
        if let Some(value) = from.get(*key) {
            to[*key] = value.clone();
        }
    }
}

fn invalid(text: &str) -> Error {
    sparksmith_core::Error::Invalid(text.to_owned()).into()
}

fn not_found(text: String) -> Error {
    sparksmith_core::Error::NotFound(text).into()
}
