//! `cargo xtask format-lock [--update]`: a stored type does not change
//! without a format version or feature change.
//!
//! [`LOCK`] records the project file's format version, its named features
//! and a fingerprint of the saved document's schema: the SHA-256 of the
//! JSON Schema the core derives from its stored types
//! (`sparksmith_core::format::stored_schema`), in a canonical form. The
//! canonical form keeps what decides how a file reads (property names,
//! types, enum tags, defaults, which fields are required) and drops what
//! does not: doc comments (`description`), titles and the Rust names of
//! types, which are renumbered in the order they are first referenced. So a
//! renamed struct or an edited doc comment keeps the fingerprint, and an
//! added, removed, renamed or retyped field changes it.
//!
//! The check fails when the fingerprint differs from the lock while
//! `FILE_VERSION` and `FEATURES` are as locked: that change needs a version
//! raise and a converter step (`sparksmith_io::convert::STEPS`), or a named
//! feature. `--update` records a new lock only after one of those changed.
use crate::{Result, root};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sparksmith_core::format::{FEATURES, FILE_VERSION, stored_schema};
use std::collections::HashMap;

/// The committed lock, from the workspace root.
pub const LOCK: &str = "crates/editor-core/format.lock";

/// Schema keywords that do not change how a file reads.
const IGNORED: &[&str] = &["description", "title", "$comment", "examples"];

const DEFS: &str = "#/$defs/";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lock {
    version: u32,
    features: Vec<String>,
    schema_sha256: String,
}

impl Lock {
    fn current() -> Result<Self> {
        Ok(Self {
            version: FILE_VERSION,
            features: FEATURES.iter().map(|f| (*f).to_owned()).collect(),
            schema_sha256: fingerprint(&stored_schema()?)?,
        })
    }

    fn read() -> Result<Option<Self>> {
        let path = root()?.join(LOCK);
        if !path.exists() {
            return Ok(None);
        }
        let value: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let version = value["version"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| format!("{LOCK} has no version"))?;
        let features = serde_json::from_value(value["features"].clone())
            .map_err(|_| format!("{LOCK} has no feature list"))?;
        let schema_sha256 = value["schema_sha256"]
            .as_str()
            .ok_or_else(|| format!("{LOCK} has no schema_sha256"))?
            .to_owned();
        Ok(Some(Self {
            version,
            features,
            schema_sha256,
        }))
    }

    fn text(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(&json!({
            "version": self.version,
            "features": self.features,
            "schema_sha256": self.schema_sha256,
        }))?;
        text.push('\n');
        Ok(text)
    }
}

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [] => check(),
        [flag] if flag == "--update" => update(),
        _ => Err("usage: cargo xtask format-lock [--update]".into()),
    }
}

pub fn check() -> Result<()> {
    let current = Lock::current()?;
    let Some(locked) = Lock::read()? else {
        return Err(format!("{LOCK} is missing; run `cargo xtask format-lock --update`").into());
    };
    verdict(&locked, &current)
}

fn update() -> Result<()> {
    let current = Lock::current()?;
    if let Some(locked) = Lock::read()? {
        if locked == current {
            println!("{LOCK}: current");
            return Ok(());
        }
        if let Err(error) = verdict(&locked, &current)
            && same_format(&locked, &current)
        {
            return Err(error);
        }
    }
    std::fs::write(root()?.join(LOCK), current.text()?)?;
    println!(
        "{LOCK}: recorded version {} with features {:?}",
        current.version, current.features
    );
    Ok(())
}

fn same_format(locked: &Lock, current: &Lock) -> bool {
    locked.version == current.version && locked.features == current.features
}

/// Whether `current` may stand against the committed `locked`.
fn verdict(locked: &Lock, current: &Lock) -> Result<()> {
    if locked == current {
        return Ok(());
    }
    if current.version < locked.version {
        return Err(format!(
            "FILE_VERSION went down from {} to {}; versions only rise",
            locked.version, current.version
        )
        .into());
    }
    if same_format(locked, current) {
        return Err(format!(
            "a stored type changed (schema fingerprint {}, locked {}) but FILE_VERSION is still {} and \
             the features are unchanged. An incompatible change raises FILE_VERSION and adds a converter \
             step (sparksmith_io::convert::STEPS) and golden files; an optional capability becomes a named \
             feature (sparksmith_core::format::FEATURES). Then run `cargo xtask format-lock --update`.",
            current.schema_sha256, locked.schema_sha256, current.version
        )
        .into());
    }
    Err(format!(
        "the format changed (version {} → {}, features {:?} → {:?}); record it with `cargo xtask format-lock --update`",
        locked.version, current.version, locked.features, current.features
    )
    .into())
}

/// The SHA-256 of `schema`'s canonical form ([`canonical`]).
fn fingerprint(schema: &Value) -> Result<String> {
    let digest = Sha256::digest(canonical(schema)?.as_bytes());
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// `schema` as text with sorted keys, without [`IGNORED`] keywords, and
/// with each definition named by the order it is first referenced in
/// (the root first, then each definition in that order), so the Rust
/// names of types do not count.
fn canonical(schema: &Value) -> Result<String> {
    let empty = Map::new();
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut names = Names::default();
    let mut out = String::new();
    let mut root = schema.clone();
    if let Some(object) = root.as_object_mut() {
        object.remove("$defs");
    }
    write(&root, &mut names, &mut out)?;
    let mut next = 0;
    while let Some(name) = names.order.get(next).cloned() {
        let def = defs
            .get(&name)
            .ok_or_else(|| format!("the schema refers to {name}, which it does not define"))?;
        out.push('\n');
        write(def, &mut names, &mut out)?;
        next += 1;
    }
    Ok(out)
}

#[derive(Default)]
struct Names {
    order: Vec<String>,
    index: HashMap<String, usize>,
}

impl Names {
    fn number(&mut self, name: &str) -> usize {
        if let Some(n) = self.index.get(name) {
            return *n;
        }
        let n = self.order.len();
        self.order.push(name.to_owned());
        self.index.insert(name.to_owned(), n);
        n
    }
}

fn write(value: &Value, names: &mut Names, out: &mut String) -> Result<()> {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map
                .keys()
                .filter(|k| !IGNORED.contains(&k.as_str()))
                .collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                let item = &map[key.as_str()];
                match (key.as_str(), item.as_str()) {
                    ("$ref", Some(target)) => {
                        let name = target
                            .strip_prefix(DEFS)
                            .ok_or_else(|| format!("unexpected schema reference {target}"))?;
                        let n = names.number(name);
                        out.push_str(&serde_json::to_string(&format!("{DEFS}{n}"))?);
                    }
                    _ => write(item, names, out)?,
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, names, out)?;
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other)?),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(def: &str, field: &str, doc: &str) -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "EditorFile",
            "type": "object",
            "properties": {"project": {"$ref": format!("#/$defs/{def}")}},
            "$defs": {def: {
                "description": doc,
                "type": "object",
                "properties": {field: {"type": "integer"}},
                "required": [field]
            }}
        })
    }

    /// Doc comments and type names do not count; field names do.
    #[test]
    fn the_fingerprint_follows_the_stored_form_only() -> Result<()> {
        let base = fingerprint(&schema("Project", "next_id", "A project."))?;
        assert_eq!(
            base,
            fingerprint(&schema("Design", "next_id", "Renamed, redocumented."))?
        );
        assert_ne!(
            base,
            fingerprint(&schema("Project", "next_object", "A project."))?
        );
        Ok(())
    }

    /// A stored type changed without a version or feature change fails;
    /// with a version raise it asks for the lock to be recorded.
    #[test]
    fn a_changed_type_needs_a_version_or_feature() -> Result<()> {
        let locked = Lock {
            version: 2,
            features: vec![],
            schema_sha256: "a".into(),
        };
        let changed = Lock {
            schema_sha256: "b".into(),
            ..locked.clone()
        };
        let message = verdict(&locked, &changed)
            .err()
            .ok_or("refused")?
            .to_string();
        assert!(message.contains("FILE_VERSION is still 2"), "{message}");
        let raised = Lock {
            version: 3,
            ..changed.clone()
        };
        let message = verdict(&locked, &raised)
            .err()
            .ok_or("refused")?
            .to_string();
        assert!(message.contains("--update"), "{message}");
        let featured = Lock {
            features: vec!["arcs".into()],
            ..changed
        };
        assert!(verdict(&locked, &featured).is_err());
        assert!(verdict(&locked, &locked).is_ok());
        let lower = Lock {
            version: 1,
            ..locked.clone()
        };
        assert!(verdict(&locked, &lower).is_err());
        Ok(())
    }
}
