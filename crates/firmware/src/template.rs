//! User firmware templates: a Rhai script that builds the output text from
//! the IR. The script sees one constant, `report` (the [`PinConfigReport`]
//! as maps and arrays, exactly its JSON form), and these helpers:
//!
//! - `c_string(s)`, `rust_string(s)`: a quoted, escaped C or Rust literal;
//! - `ident(s)`, `lower_ident(s)`: `s` sanitised to `[A-Z0-9_]` (or lower
//!   case), the rule firmware names follow;
//! - `comment(s)`: `s` safe inside a one-line comment.
//!
//! It evaluates to the text, or to `#{ file_name: "x.h", text: "…" }` to
//! suggest a file name. Back-tick strings interpolate:
//! `` `#define ${p.firmware_name}_PIN ${p.gpio.index}` ``.
//!
//! Sandbox: no modules, I/O, time, randomness, `eval`, `print` or `debug`;
//! every string is at most [`MAX_OUTPUT_BYTES`], arrays and maps are capped
//! and operations are counted ([`MAX_TEMPLATE_OPERATIONS`], a target). A
//! script error or exhausted limit is an error; nothing panics.
use crate::{Emitted, MAX_OUTPUT_BYTES, text};
use pcbcraft_core::firmware::{PinConfigReport, sanitise};
use pcbcraft_core::{Error, Result};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};
use serde_json::Value;

/// Operations one template run may take (target).
pub const MAX_TEMPLATE_OPERATIONS: u64 = 5_000_000;
/// Items of one array, including the IR's pins and endpoints.
pub const MAX_TEMPLATE_ARRAY: usize = 65_536;
/// Entries of one map.
pub const MAX_TEMPLATE_MAP: usize = 256;
/// Bytes of a suggested file name.
pub const MAX_FILE_NAME: usize = 64;
/// The file name used when the template suggests none.
pub const DEFAULT_FILE_NAME: &str = "pins.txt";

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_TEMPLATE_OPERATIONS)
        .set_max_call_levels(32)
        .set_max_expr_depths(64, 32)
        .set_max_variables(256)
        .set_max_functions(64)
        .set_max_string_size(MAX_OUTPUT_BYTES)
        .set_max_array_size(MAX_TEMPLATE_ARRAY)
        .set_max_map_size(MAX_TEMPLATE_MAP)
        .set_strict_variables(true);
    engine
        .disable_symbol("eval")
        .disable_symbol("print")
        .disable_symbol("debug");
    engine.register_fn("c_string", |s: ImmutableString| text::c_string(&s));
    engine.register_fn("rust_string", |s: ImmutableString| text::rust_string(&s));
    engine.register_fn("ident", |s: ImmutableString| sanitise(&s));
    engine.register_fn("lower_ident", |s: ImmutableString| text::lower_ident(&s));
    engine.register_fn("comment", |s: ImmutableString| text::comment(&s));
    engine
}

fn dynamic(value: Value) -> Dynamic {
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => b.into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64().unwrap_or_default().into(),
        },
        Value::String(s) => s.into(),
        Value::Array(items) => items.into_iter().map(dynamic).collect::<Vec<_>>().into(),
        Value::Object(fields) => {
            let mut map = Map::new();
            for (key, value) in fields {
                map.insert(key.into(), dynamic(value));
            }
            map.into()
        }
    }
}

fn script_error(error: &EvalAltResult) -> Error {
    if matches!(
        error.unwrap_inner(),
        EvalAltResult::ErrorTooManyOperations(..)
            | EvalAltResult::ErrorTooManyVariables(..)
            | EvalAltResult::ErrorStackOverflow(..)
            | EvalAltResult::ErrorDataTooLarge(..)
            | EvalAltResult::ErrorTerminated(..)
    ) {
        Error::Limit(format!("template limit: {error}"))
    } else {
        Error::Invalid(format!("template: {error}"))
    }
}

fn file_name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FILE_NAME
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Check that `source` compiles, without running it.
pub fn check(source: &str) -> Result<()> {
    let mut scope = Scope::new();
    scope.push_constant("report", Dynamic::UNIT);
    engine()
        .compile_with_scope(&scope, source)
        .map(|_| ())
        .map_err(|e| Error::Invalid(format!("template: {e}")))
}

/// Run the template `source` over `report`.
pub fn emit(report: &PinConfigReport, source: &str) -> Result<Emitted> {
    let engine = engine();
    let mut scope = Scope::new();
    scope.push_constant("report", dynamic(serde_json::to_value(report)?));
    let ast = engine
        .compile_with_scope(&scope, source)
        .map_err(|e| Error::Invalid(format!("template: {e}")))?;
    let result: Dynamic = engine
        .eval_ast_with_scope(&mut scope, &ast)
        .map_err(|e| script_error(&e))?;
    let shape = || {
        Error::Invalid(
            "template: the script must evaluate to a string or #{ file_name, text }".into(),
        )
    };
    let (file_name, text) = if result.is_string() {
        let text = result.into_immutable_string().map_err(|_| shape())?;
        (DEFAULT_FILE_NAME.to_owned(), text.to_string())
    } else if let Some(map) = result.try_cast::<Map>() {
        let field = |key: &str| {
            map.get(key)
                .and_then(|v| v.clone().into_immutable_string().ok())
                .map(|s| s.to_string())
        };
        if map.keys().any(|k| k != "file_name" && k != "text") {
            return Err(shape());
        }
        let text = field("text").ok_or_else(shape)?;
        let file_name = match map.get("file_name") {
            None => DEFAULT_FILE_NAME.to_owned(),
            Some(_) => field("file_name").ok_or_else(shape)?,
        };
        (file_name, text)
    } else {
        return Err(shape());
    };
    if !file_name_valid(&file_name) {
        return Err(Error::Invalid(format!(
            "template: the file name must be 1–{MAX_FILE_NAME} bytes of letters, digits, '.', '_' or '-', not starting with '.'"
        )));
    }
    Ok(Emitted { file_name, text })
}
