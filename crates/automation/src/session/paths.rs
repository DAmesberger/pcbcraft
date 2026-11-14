//! The paths a request names, found from its schema: a parameter typed as
//! a [`FilePath`] carries `"x-kind": "path"`, and the gate walks a request's
//! value with the request schema to collect every one. Nothing lists which
//! requests take a path; the parameter types say so.
//!
//! [`FilePath`]: sparksmith_core::permission::FilePath
use super::Request;
use serde_json::Value;
use sparksmith_core::permission::PATH_KIND;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// How deep a schema or value is followed; deeper parts are not searched
/// (the request decoder refuses far shallower input).
const MAX_DEPTH: usize = 64;

/// The schema of [`Request`], with its marks, generated once.
fn request_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA
        .get_or_init(|| serde_json::to_value(schemars::schema_for!(Request)).unwrap_or(Value::Null))
}

/// Every path in `request`, a request's JSON value.
pub fn in_request(request: &Value) -> Vec<String> {
    let root = request_schema();
    let mut out = Vec::new();
    collect(request, root, root, &mut out, 0);
    out
}

fn is_path(schema: &Value) -> bool {
    schema.get(PATH_KIND).and_then(Value::as_str) == Some("path")
}

fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    let mut schema = schema;
    for _ in 0..MAX_DEPTH {
        let Some(name) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
        else {
            break;
        };
        match root["$defs"].get(name) {
            Some(target) => schema = target,
            None => break,
        }
    }
    schema
}

/// Whether `value` agrees with every `const` property of `variant`, so a
/// tagged union is walked only through the variant the value names.
fn matches(value: &Value, variant: &Value) -> bool {
    variant["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, property)| property.get("const").map(|c| (key, c)))
        .all(|(key, constant)| value.get(key) == Some(constant))
}

fn collect(value: &Value, schema: &Value, root: &Value, out: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let schema = resolve(schema, root);
    if is_path(schema) {
        if let Some(path) = value.as_str() {
            out.push(path.to_owned());
        }
        return;
    }
    let next = depth.saturating_add(1);
    for sub in schema["allOf"].as_array().into_iter().flatten() {
        collect(value, sub, root, out, next);
    }
    for key in ["oneOf", "anyOf"] {
        let Some(variants) = schema[key].as_array() else {
            continue;
        };
        for variant in variants {
            if matches(value, resolve(variant, root)) {
                collect(value, variant, root, out, next);
            }
        }
    }
    match value {
        Value::Object(object) => {
            let properties = schema["properties"].as_object();
            for (key, inner) in object {
                match properties.and_then(|p| p.get(key)) {
                    Some(property) => collect(inner, property, root, out, next),
                    None => {
                        if let Some(extra) = schema.get("additionalProperties")
                            && extra.is_object()
                        {
                            collect(inner, extra, root, out, next);
                        }
                    }
                }
            }
        }
        Value::Array(items) => {
            let prefix = schema["prefixItems"].as_array();
            for (index, item) in items.iter().enumerate() {
                match prefix.and_then(|p| p.get(index)) {
                    Some(each) => collect(item, each, root, out, next),
                    None => {
                        if let Some(each) = schema.get("items")
                            && each.is_object()
                        {
                            collect(item, each, root, out, next);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Whether anything `schema` (a root schema with its `$defs`) reaches is a
/// path: whether a request of this type can name one.
pub fn marks_path(schema: &Value) -> bool {
    let mut seen = BTreeSet::new();
    reaches_path(schema, schema, &mut seen, 0)
}

fn reaches_path(schema: &Value, root: &Value, seen: &mut BTreeSet<String>, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match schema {
        Value::Object(object) => {
            if is_path(schema) {
                return true;
            }
            if let Some(name) = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
                && seen.insert(name.to_owned())
                && let Some(target) = root["$defs"].get(name)
                && reaches_path(target, root, seen, depth.saturating_add(1))
            {
                return true;
            }
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "$defs")
                .any(|(_, inner)| reaches_path(inner, root, seen, depth.saturating_add(1)))
        }
        Value::Array(items) => items
            .iter()
            .any(|inner| reaches_path(inner, root, seen, depth.saturating_add(1))),
        _ => false,
    }
}

/// `schema` without its path marks: what a caller is shown, unchanged from
/// before paths were typed.
pub fn unmark(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            object.retain(|key, value| !(key == PATH_KIND && value == "path"));
            object.values_mut().for_each(unmark);
        }
        Value::Array(items) => items.iter_mut().for_each(unmark),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

    fn request(value: Value) -> std::result::Result<Request, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn a_request_names_the_paths_its_types_mark() -> Fallible {
        let open = request(json!({"command": "project.open",
            "params": {"expected_revision": 0, "path": "/a/b.sparksmith"}}))?;
        assert_eq!(open.paths()?, ["/a/b.sparksmith"]);
        let save = request(json!({"command": "project.save", "params": {}}))?;
        assert!(save.paths()?.is_empty());
        let follow = request(json!({"command": "assembly.preview",
            "params": {"path": "/p", "design": "/d"}}))?;
        let mut both = follow.paths()?;
        both.sort();
        assert_eq!(both, ["/d", "/p"]);
        let firmware = request(json!({"command": "firmware.export",
            "params": {"instance": 1, "emitter": {"emitter": "json"}, "path": "/o",
                "template_file": "/t"}}))?;
        let mut both = firmware.paths()?;
        both.sort();
        assert_eq!(both, ["/o", "/t"]);
        let place = request(json!({"command": "layout.place_part_file",
            "params": {"path": "/f", "position": [0.0, 0.0], "expected_revision": 0}}))?;
        assert_eq!(place.paths()?, ["/f"]);
        Ok(())
    }

    /// A text field that is no path is never read as one, even beside a
    /// variant of the same union that has a path of that name.
    #[test]
    fn other_strings_are_not_paths() -> Fallible {
        let rename = request(json!({"command": "project.apply", "params": {
            "expected_revision": 0,
            "operation": {"editor": "Schematic",
                "operation": {"command": "project.rename", "params": {"name": "/x"}}}}}))?;
        assert!(rename.paths()?.is_empty());
        let favourite = request(json!({"command": "library.favourite",
            "params": {"key": {"source": "s", "lineage": "l"}, "favourite": true}}));
        if let Ok(favourite) = favourite {
            assert!(favourite.paths()?.is_empty());
        }
        Ok(())
    }

    /// What a caller is shown is the schema a plain string gave, so typing a
    /// path changes no tool list and no `project.schema` reply.
    #[test]
    fn an_unmarked_path_schema_is_a_string_schema() -> Fallible {
        use sparksmith_core::permission::FilePath;
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Typed {
            /// A file.
            path: FilePath,
            /// Maybe a file.
            maybe: Option<FilePath>,
            #[schemars(default)]
            plain: Option<FilePath>,
        }
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Plain {
            /// A file.
            path: String,
            /// Maybe a file.
            maybe: Option<String>,
            #[schemars(default)]
            plain: Option<String>,
        }
        let mut typed = serde_json::to_value(schemars::schema_for!(Typed))?;
        assert!(marks_path(&typed));
        unmark(&mut typed);
        let mut plain = serde_json::to_value(schemars::schema_for!(Plain))?;
        typed["title"] = Value::Null;
        plain["title"] = Value::Null;
        assert_eq!(typed, plain);
        Ok(())
    }

    #[test]
    fn marks_are_found_and_removed() {
        let mut schema = json!({"properties": {"a": {"$ref": "#/$defs/P"}},
            "$defs": {"P": {"type": "string", "x-kind": "path"}}});
        assert!(marks_path(&schema));
        unmark(&mut schema);
        assert!(!marks_path(&schema));
        assert_eq!(schema["$defs"]["P"], json!({"type": "string"}));
    }
}
