//! The catalog as modules, and `surface.lock` (`docs/extension-surface.md`).
//!
//! Every entry belongs to the module its declaration names. A module's
//! digest is the SHA-256 of its canonical description: its entries sorted
//! by name, each with what it declares and the canonical schema of its
//! parameters. The lock records per module its version and digest, and per
//! entry its class, a schema digest, a digest per parameter and the
//! required parameters, so [`ratchet`] can tell a compatible change (a new
//! entry, a new optional parameter: a minor bump) from a breaking one (a
//! removed entry or parameter, a retyped parameter, a new required one, a
//! stricter class: a major bump). `cargo xtask surface --check` runs
//! [`tests::the_lock_holds`].
use super::discovery::{self, Entry};
use super::metadata::{self, EntryMeta};
use serde_json::{Map, Value, json};
use sparksmith_core::permission::{MODULES, ModuleDecl};
use sparksmith_generator_abi::surface::{
    LOCK_FORMAT, Lock, LockedEntry, LockedModule, parse_version, version_text,
};
use sparksmith_generator_abi::{hex, sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The lock's place in the repository, beside this crate's manifest.
pub const LOCK_FILE: &str = "surface.lock";

/// Schema keywords that change no call: doc comments and type names.
const IGNORED: &[&str] = &["description", "title", "$comment", "examples", "$schema"];
const DEFS: &str = "#/$defs/";
/// Hex digits of a short digest.
const SHORT: usize = 16;

/// The parameters' schema of `entry`: the request's own, or the variant's
/// `params` for an operation or query.
fn input(entry: &Entry) -> &Value {
    let schema = &entry.schema;
    let params = &schema["properties"]["params"];
    if entry.kind != "command" && params.is_object() {
        resolve(params, entry.root)
    } else {
        schema
    }
}

fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix(DEFS))
        .and_then(|name| root["$defs"].get(name))
        .unwrap_or(schema)
}

/// `schema` with the definitions it reaches, renumbered in the order they
/// are first referenced and without the keywords that change no call, so a
/// renamed type or an edited doc comment keeps the form.
fn canonical(schema: &Value, root: &Value) -> Value {
    let mut order: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let main = rewrite(schema, &mut order, &mut index);
    let mut defs = Vec::new();
    let mut next = 0;
    while let Some(name) = order.get(next).cloned() {
        let def = root["$defs"].get(&name).cloned().unwrap_or(Value::Null);
        defs.push(rewrite(&def, &mut order, &mut index));
        next += 1;
    }
    json!({"schema": main, "defs": defs})
}

fn rewrite(value: &Value, order: &mut Vec<String>, index: &mut HashMap<String, usize>) -> Value {
    match value {
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, inner) in object {
                if IGNORED.contains(&key.as_str()) || key == "$defs" {
                    continue;
                }
                if key == "$ref"
                    && let Some(name) = inner.as_str().and_then(|r| r.strip_prefix(DEFS))
                {
                    let number = *index.entry(name.to_owned()).or_insert_with(|| {
                        order.push(name.to_owned());
                        order.len() - 1
                    });
                    out.insert(key.clone(), json!(format!("{DEFS}{number}")));
                    continue;
                }
                out.insert(key.clone(), rewrite(inner, order, index));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| rewrite(i, order, index)).collect())
        }
        other => other.clone(),
    }
}

fn digest(value: &Value) -> String {
    hex(&sha256(value.to_string().as_bytes()))
}

fn short(value: &Value) -> String {
    let mut text = digest(value);
    text.truncate(SHORT);
    text
}

/// One entry as the lock and the module digest see it.
struct Described {
    locked: LockedEntry,
    /// What the module digest covers for this entry.
    contract: Value,
}

fn describe_entry(entry: &Entry, meta: &EntryMeta) -> Described {
    let schema = input(entry);
    let root = entry.root;
    let whole = canonical(schema, root);
    let mut fields: Vec<(String, String)> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, property)| (name.clone(), short(&canonical(property, root))))
        .collect();
    fields.sort();
    let mut required: Vec<String> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    required.sort();
    let contract = json!({
        "name": meta.name,
        "kind": meta.kind,
        "class": meta.class,
        "effect": meta.effect,
        "reach": meta.reach.iter().map(|r| r.name()).collect::<Vec<_>>(),
        "needs": meta.needs.iter().map(|f| f.name()).collect::<Vec<_>>(),
        "person": meta.person,
        "long": meta.long,
        "result": meta.result,
        "since": version_text(meta.since),
        "input": digest(&whole),
    });
    Described {
        locked: LockedEntry {
            name: meta.name.clone(),
            class: meta.class,
            schema: short(&whole),
            fields,
            required,
        },
        contract,
    }
}

/// The catalog as `surface.lock` records it, modules in declaration order.
pub fn current() -> Lock {
    let mut by_module: BTreeMap<&str, Vec<Described>> = BTreeMap::new();
    for (entry, meta) in discovery::entries().iter().zip(metadata::entries()) {
        by_module
            .entry(meta.module)
            .or_default()
            .push(describe_entry(entry, meta));
    }
    let modules = MODULES
        .iter()
        .map(|module| {
            let mut entries = by_module.remove(module.id).unwrap_or_default();
            entries.sort_by(|a, b| a.locked.name.cmp(&b.locked.name));
            let description = json!({
                "module": module.id,
                "entries": entries.iter().map(|e| &e.contract).collect::<Vec<_>>(),
            });
            LockedModule {
                id: module.id.to_owned(),
                version: version_text(module.version),
                digest: format!("sha256:{}", digest(&description)),
                entries: entries.into_iter().map(|e| e.locked).collect(),
            }
        })
        .collect();
    Lock {
        format: LOCK_FORMAT,
        modules,
    }
}

/// The lock's text: one line per entry, so a change reads as a line.
pub fn text(lock: &Lock) -> Result<String, serde_json::Error> {
    let mut out = format!("{{\"format\": {}, \"modules\": [\n", lock.format);
    for (m, module) in lock.modules.iter().enumerate() {
        out.push_str(&format!(
            " {{\"id\": {}, \"version\": {}, \"digest\": {}, \"entries\": [\n",
            serde_json::to_string(&module.id)?,
            serde_json::to_string(&module.version)?,
            serde_json::to_string(&module.digest)?,
        ));
        for (e, entry) in module.entries.iter().enumerate() {
            out.push_str("  ");
            out.push_str(&serde_json::to_string(entry)?);
            out.push_str(if e + 1 < module.entries.len() {
                ",\n"
            } else {
                "\n"
            });
        }
        out.push_str(if m + 1 < lock.modules.len() {
            " ]},\n"
        } else {
            " ]}\n"
        });
    }
    out.push_str("]}\n");
    Ok(out)
}

/// What a change from `locked` to `current` breaks without the version it
/// needs, in plain words. Empty when every change is numbered.
pub fn ratchet(locked: &Lock, current: &Lock, declared: &[ModuleDecl]) -> Vec<String> {
    let mut problems = Vec::new();
    let since: HashMap<&str, (u16, u16)> = metadata::entries()
        .iter()
        .map(|e| (e.name.as_str(), e.since))
        .collect();
    let since_of = |name: &str| since.get(name).copied();
    for old in &locked.modules {
        if !current.modules.iter().any(|m| m.id == old.id) {
            problems.push(format!("module {} was removed", old.id));
        }
    }
    for new in &current.modules {
        let Some(old) = locked.modules.iter().find(|m| m.id == new.id) else {
            continue;
        };
        let (Some(was), Some(now)) = (parse_version(&old.version), parse_version(&new.version))
        else {
            problems.push(format!("module {} has an unreadable version", new.id));
            continue;
        };
        if now < was {
            problems.push(format!(
                "module {} went back from {} to {}",
                new.id, old.version, new.version
            ));
            continue;
        }
        if old.digest == new.digest {
            continue;
        }
        if now == was {
            problems.push(format!(
                "module {} changed without a version change: raise it from {} ({})",
                new.id,
                old.version,
                breaking(old, new)
                    .first()
                    .map_or("a minor change", String::as_str)
            ));
            continue;
        }
        // Before 1.0 the minor is the breaking number, as in semver: any
        // raise numbers a break. From 1.0 a break raises the major.
        if now.0 == was.0 && was.0 != 0 {
            for why in breaking(old, new) {
                problems.push(format!(
                    "module {} {} needs a major version, not {}",
                    new.id, why, new.version
                ));
            }
        }
        let known: BTreeSet<&str> = old.entries.iter().map(|e| e.name.as_str()).collect();
        for entry in new
            .entries
            .iter()
            .filter(|e| !known.contains(e.name.as_str()))
        {
            let declared_version = declared.iter().find(|m| m.id == new.id).map(|m| m.version);
            if since_of(&entry.name).is_some() && since_of(&entry.name) != declared_version {
                problems.push(format!(
                    "{} is new in module {} {}: declare it since({}, {})",
                    entry.name, new.id, new.version, now.0, now.1
                ));
            }
        }
    }
    problems
}

/// The changes from `old` to `new` that break a caller.
fn breaking(old: &LockedModule, new: &LockedModule) -> Vec<String> {
    let mut out = Vec::new();
    for was in &old.entries {
        let Some(now) = new.entries.iter().find(|e| e.name == was.name) else {
            out.push(format!("removed {}", was.name));
            continue;
        };
        if now.class > was.class {
            out.push(format!(
                "made {} stricter ({} to {})",
                was.name,
                was.class.name(),
                now.class.name()
            ));
        }
        for (field, digest) in &was.fields {
            match now.fields.iter().find(|(name, _)| name == field) {
                None => out.push(format!("removed {}.{field}", was.name)),
                Some((_, other)) if other != digest => {
                    out.push(format!("retyped {}.{field}", was.name));
                }
                Some(_) => {}
            }
        }
        for field in &now.required {
            if !was.required.contains(field) {
                out.push(format!("made {}.{field} required", was.name));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
