//! The two file layouts (docs/file-format.md, "Layouts"), pure and
//! bounded:
//!
//! - **Layout A**, collections (`.jsonl`): a header line, then one record
//!   per line, keys in the declared order, records sorted by the
//!   collection's key.
//! - **Layout B**, documents (`.json`): one object, `format` and `version`
//!   first, then one member per line in the declared order; an array the
//!   layout marks as a table has one element per line.
//!
//! Inside a record or member, objects are written with their keys sorted
//! by UTF-8 bytes and no spaces, as the canonical codec writes them. No
//! value is `null`, every string is in Unicode NFC, and floats appear only
//! in files that still allow them ([`Numbers`]).
//!
//! A reader accepts exactly the bytes the writer would write for the value
//! it reads, so opening and saving changes nothing. Anything else is
//! refused with the file, the line and what is wrong.
use super::files::{MAX_COLLECTION_BYTES, MAX_DOCUMENT_BYTES, MAX_LINE_BYTES};
use super::records::{Collection, DocumentLayout, Numbers, RecordOrder};
use crate::diagnostics::{Error, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Deepest nesting a value may have, as in the canonical codec.
pub const MAX_DEPTH: usize = sparksmith_generator_abi::codec::MAX_DEPTH;

/// One part of a record's sort key: integers sort before strings, integers
/// by value and strings by their UTF-8 bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum KeyPart {
    Int(i128),
    Text(String),
}

impl std::fmt::Display for KeyPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Text(text) => write!(f, "\"{text}\""),
        }
    }
}

type SortKey = Vec<KeyPart>;

fn key_text(collection: &Collection, key: &SortKey) -> String {
    collection
        .sort
        .iter()
        .zip(key)
        .map(|(field, part)| format!("{field} {part}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A layout A file read back: the header's own members (beyond `format`
/// and `version`) and the records in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionFile<T> {
    pub header: Map<String, Value>,
    pub records: Vec<T>,
}

/// Write a collection: the header with `header`'s values for the
/// collection's header members, then `records` sorted by the collection's
/// key, one per line.
pub fn write_collection<T: Serialize>(
    collection: &Collection,
    version: u32,
    header: &Map<String, Value>,
    records: &[T],
) -> Result<Vec<u8>> {
    let refuse = |what: String| Error::Invalid(format!("cannot write {}: {what}", collection.path));
    let mut out = header_line(collection, version, header).map_err(refuse)?;
    let mut lines = Vec::with_capacity(records.len());
    for record in records {
        let value = stored_value(record)?;
        lines.push(encode_record(collection, &value).map_err(refuse)?);
    }
    lines.sort_by(|a, b| a.key.cmp(&b.key));
    check_unique(collection, lines.iter().map(|l| (&l.key, l.id))).map_err(refuse)?;
    for line in &lines {
        if line.bytes.len() > MAX_COLLECTION_BYTES.saturating_sub(out.len()) {
            return Err(Error::Limit(format!(
                "{} would exceed {MAX_COLLECTION_BYTES} bytes",
                collection.path
            )));
        }
        out.extend_from_slice(&line.bytes);
    }
    Ok(out)
}

/// Read a collection written by [`write_collection`] for `version`,
/// refusing anything else with `file`, the line number and what is wrong.
pub fn read_collection<T: DeserializeOwned + Serialize>(
    file: &str,
    bytes: &[u8],
    collection: &Collection,
    version: u32,
) -> Result<CollectionFile<T>> {
    if bytes.len() > MAX_COLLECTION_BYTES {
        return Err(Error::Limit(format!(
            "{file} is {} bytes, more than the {MAX_COLLECTION_BYTES} allowed",
            bytes.len()
        )));
    }
    let at = |line: usize, what: String| Error::Invalid(format!("{file}, line {line}: {what}"));
    let mut lines = split_lines(bytes, MAX_LINE_BYTES).map_err(|(line, what)| at(line, what))?;
    let Some((_, first)) = lines.next() else {
        return Err(Error::Invalid(format!(
            "{file} is empty; line 1 must be the header {{\"format\":\"{}\",\"version\":{version}}}",
            collection.format()
        )));
    };
    let header = read_header(collection, version, first).map_err(|what| at(1, what))?;
    let mut records = Vec::new();
    let mut previous: Option<(SortKey, usize)> = None;
    let mut ids: BTreeMap<i128, usize> = BTreeMap::new();
    for (number, line) in lines {
        let value: Value = serde_json::from_slice(line)
            .map_err(|e| at(number, format!("not valid JSON ({e})")))?;
        let encoded = encode_record(collection, &value).map_err(|what| at(number, what))?;
        check_key_order(collection, line, &value).map_err(|what| at(number, what))?;
        if let Some((key, earlier)) = &previous {
            if encoded.key == *key {
                return Err(at(
                    number,
                    format!(
                        "repeats the key ({}) of line {earlier}",
                        key_text(collection, key)
                    ),
                ));
            }
            if encoded.key < *key {
                return Err(at(
                    number,
                    format!(
                        "is out of order: records are sorted by {}, and this one belongs before line {earlier}",
                        collection.sort.join(", then ")
                    ),
                ));
            }
        }
        if let Some(id) = encoded.id
            && let Some(earlier) = ids.insert(id, number)
        {
            return Err(at(number, format!("repeats id {id} of line {earlier}")));
        }
        let typed: T = serde_json::from_value(value)
            .map_err(|e| at(number, format!("does not describe a record ({e})")))?;
        let again =
            encode_record(collection, &stored_value(&typed)?).map_err(|what| at(number, what))?;
        if again.bytes.strip_suffix(b"\n") != Some(line) {
            return Err(at(
                number,
                "is not in the form this build writes (spacing, number spelling, or a default value written out)"
                    .into(),
            ));
        }
        records.push(typed);
        previous = Some((encoded.key, number));
    }
    Ok(CollectionFile { header, records })
}

/// Write a document: `format`, `version`, then `value`'s members in the
/// layout's order, tables one element per line.
pub fn write_document<T: Serialize>(
    layout: &DocumentLayout,
    version: u32,
    value: &T,
) -> Result<Vec<u8>> {
    let value = stored_value(value)?;
    encode_document(layout, version, &value)
        .map_err(|what| Error::Invalid(format!("cannot write {}: {what}", layout.path)))
        .and_then(|bytes| {
            if bytes.len() > MAX_DOCUMENT_BYTES {
                Err(Error::Limit(format!(
                    "{} would exceed {MAX_DOCUMENT_BYTES} bytes",
                    layout.path
                )))
            } else {
                Ok(bytes)
            }
        })
}

/// Read a document written by [`write_document`] for `version`.
pub fn read_document<T: DeserializeOwned + Serialize>(
    file: &str,
    bytes: &[u8],
    layout: &DocumentLayout,
    version: u32,
) -> Result<T> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(Error::Limit(format!(
            "{file} is {} bytes, more than the {MAX_DOCUMENT_BYTES} allowed",
            bytes.len()
        )));
    }
    let at = |line: usize, what: String| Error::Invalid(format!("{file}, line {line}: {what}"));
    // Line ends first; the lines themselves are checked by comparing with
    // what the writer writes.
    split_lines(bytes, MAX_DOCUMENT_BYTES)
        .map_err(|(line, what)| at(line, what))?
        .for_each(drop);
    let mut value: Value =
        serde_json::from_slice(bytes).map_err(|e| at(e.line(), format!("not valid JSON ({e})")))?;
    let Some(object) = value.as_object_mut() else {
        return Err(at(1, "the file must hold one JSON object".into()));
    };
    check_format(object, &layout.format(), version).map_err(|what| at(2, what))?;
    if let Some(key) = object.keys().find(|k| !layout.fields.contains(&k.as_str())) {
        return Err(at(
            line_of_key(bytes, key),
            format!(
                "unknown member \"{key}\"; {} has {}",
                layout.format(),
                layout.fields.join(", ")
            ),
        ));
    }
    let typed: T = serde_json::from_value(value)
        .map_err(|e| at(1, format!("does not describe this document ({e})")))?;
    let again = write_document(layout, version, &typed)?;
    if again != bytes {
        let same = again.iter().zip(bytes).take_while(|(a, b)| a == b).count();
        let line = bytes[..same].iter().filter(|b| **b == b'\n').count() + 1;
        return Err(at(
            line,
            "is not in the form this build writes (member order, spacing, number spelling, or a default value written out)"
                .into(),
        ));
    }
    Ok(typed)
}

/// `value` as JSON with every `null` member of an object left out: an
/// absent value is never written, and reads back as absent. A `null`
/// inside an array has no such meaning and stays, to be refused.
fn stored_value<T: Serialize>(value: &T) -> Result<Value> {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.retain(|_, item| !item.is_null());
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value)?;
    strip(&mut value);
    Ok(value)
}

/// One encoded record: its sort key, its id when it has one, and its line
/// with the final LF.
struct Line {
    key: SortKey,
    id: Option<i128>,
    bytes: Vec<u8>,
}

fn encode_record(collection: &Collection, value: &Value) -> std::result::Result<Line, String> {
    let Some(object) = value.as_object() else {
        return Err("a record must be a JSON object".into());
    };
    let order = record_order(collection, object)?;
    if let Some(key) = object.keys().find(|k| !order.fields.contains(&k.as_str())) {
        return Err(format!(
            "unknown field \"{key}\"; {} records have {}",
            order.kind.unwrap_or(collection.name),
            order.fields.join(", ")
        ));
    }
    let mut key = Vec::with_capacity(collection.sort.len());
    for field in collection.sort {
        key.push(match object.get(*field) {
            Some(Value::Number(n)) => match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => KeyPart::Int(i128::from(i)),
                (None, Some(u)) => KeyPart::Int(i128::from(u)),
                _ => return Err(format!("the sort field \"{field}\" must be a whole number")),
            },
            Some(Value::String(text)) => KeyPart::Text(text.clone()),
            Some(_) => {
                return Err(format!(
                    "the sort field \"{field}\" must be a number or a text"
                ));
            }
            None => return Err(format!("the sort field \"{field}\" is missing")),
        });
    }
    let id = match object.get("id") {
        Some(Value::Number(n)) => n.as_i64().map(i128::from).or(n.as_u64().map(i128::from)),
        _ => None,
    };
    let mut bytes = Vec::new();
    bytes.push(b'{');
    let mut first = true;
    for field in order.fields {
        let Some(item) = object.get(*field) else {
            continue;
        };
        if !first {
            bytes.push(b',');
        }
        first = false;
        check_value(item, collection.numbers, &format!("/{field}"), 0)?;
        write_member(field, item, &mut bytes)?;
    }
    bytes.extend_from_slice(b"}\n");
    if bytes.len() > MAX_LINE_BYTES {
        return Err(format!(
            "the record is {} bytes on its line, more than the {MAX_LINE_BYTES} allowed",
            bytes.len()
        ));
    }
    Ok(Line { key, id, bytes })
}

fn record_order(
    collection: &Collection,
    object: &Map<String, Value>,
) -> std::result::Result<&'static RecordOrder, String> {
    let kinds = || {
        collection
            .records
            .iter()
            .filter_map(|r| r.kind)
            .collect::<Vec<_>>()
            .join(", ")
    };
    match collection.type_field {
        None => collection
            .record(None)
            .ok_or_else(|| format!("{} declares no record", collection.name)),
        Some(field) => match object.get(field) {
            Some(Value::String(kind)) => collection.record(Some(kind)).ok_or_else(|| {
                format!(
                    "unknown {field} \"{kind}\"; {} records are {}",
                    collection.name,
                    kinds()
                )
            }),
            _ => Err(format!(
                "the record has no \"{field}\"; it is one of {}",
                kinds()
            )),
        },
    }
}

/// Whether the keys of a record's line come once each and in the
/// declared order. A parsed value keeps neither the order nor a repeated
/// key, so this reads the line's keys themselves.
fn check_key_order(
    collection: &Collection,
    line: &[u8],
    value: &Value,
) -> std::result::Result<(), String> {
    struct Keys(Vec<String>);
    impl<'de> serde::Deserialize<'de> for Keys {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            struct Visit;
            impl<'de> serde::de::Visitor<'de> for Visit {
                type Value = Keys;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("a JSON object")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> std::result::Result<Keys, A::Error> {
                    let mut keys = Vec::new();
                    while let Some(key) = map.next_key::<String>()? {
                        map.next_value::<serde::de::IgnoredAny>()?;
                        keys.push(key);
                    }
                    Ok(Keys(keys))
                }
            }
            d.deserialize_map(Visit)
        }
    }
    let Keys(keys) =
        serde_json::from_slice(line).map_err(|e| format!("not a JSON object ({e})"))?;
    let Some(object) = value.as_object() else {
        return Err("a record must be a JSON object".into());
    };
    let order = record_order(collection, object)?;
    let mut last: Option<(usize, &str)> = None;
    for key in &keys {
        let Some(place) = order.fields.iter().position(|f| f == key) else {
            continue;
        };
        if let Some((before, earlier)) = last {
            if place == before {
                return Err(format!("the field \"{key}\" appears twice"));
            }
            if place < before {
                return Err(format!(
                    "the field \"{key}\" comes after \"{earlier}\"; the declared order is {}",
                    order.fields.join(", ")
                ));
            }
        }
        last = Some((place, key));
    }
    Ok(())
}

fn check_unique<'a>(
    collection: &Collection,
    lines: impl Iterator<Item = (&'a SortKey, Option<i128>)>,
) -> std::result::Result<(), String> {
    let mut previous: Option<&SortKey> = None;
    let mut ids = std::collections::BTreeSet::new();
    for (key, id) in lines {
        if previous == Some(key) {
            return Err(format!(
                "two records have the key ({})",
                key_text(collection, key)
            ));
        }
        if let Some(id) = id
            && !ids.insert(id)
        {
            return Err(format!("two records have id {id}"));
        }
        previous = Some(key);
    }
    Ok(())
}

fn header_line(
    collection: &Collection,
    version: u32,
    header: &Map<String, Value>,
) -> std::result::Result<Vec<u8>, String> {
    if let Some(key) = header
        .keys()
        .find(|k| !collection.header.contains(&k.as_str()))
    {
        return Err(format!("the header has no member \"{key}\""));
    }
    let mut out = Vec::new();
    out.push(b'{');
    write_member("format", &Value::String(collection.format()), &mut out)?;
    out.push(b',');
    write_member("version", &Value::from(version), &mut out)?;
    for field in collection.header {
        let Some(item) = header.get(*field) else {
            return Err(format!("the header needs \"{field}\""));
        };
        check_value(item, Numbers::IntegersOnly, &format!("/{field}"), 0)?;
        out.push(b',');
        write_member(field, item, &mut out)?;
    }
    out.extend_from_slice(b"}\n");
    Ok(out)
}

fn read_header(
    collection: &Collection,
    version: u32,
    line: &[u8],
) -> std::result::Result<Map<String, Value>, String> {
    let expected = || {
        format!(
            "the header must be {{\"format\":\"{}\",\"version\":{version}{}}}",
            collection.format(),
            collection
                .header
                .iter()
                .map(|h| format!(",\"{h}\":…"))
                .collect::<String>()
        )
    };
    let mut value: Value = serde_json::from_slice(line).map_err(|_| expected())?;
    let Some(object) = value.as_object_mut() else {
        return Err(expected());
    };
    check_format(object, &collection.format(), version)?;
    let header = std::mem::take(object);
    if header_line(collection, version, &header)?.strip_suffix(b"\n") != Some(line) {
        return Err(expected());
    }
    Ok(header)
}

/// Remove and check `format` and `version`. A version other than the
/// tree's is refused: the file was saved by another version, most often
/// merged in from a branch.
fn check_format(
    object: &mut Map<String, Value>,
    format: &str,
    version: u32,
) -> std::result::Result<(), String> {
    match object.remove("format") {
        Some(Value::String(found)) if found == format => {}
        Some(Value::String(found)) => {
            return Err(format!(
                "this is a \"{found}\" file where \"{format}\" belongs"
            ));
        }
        _ => return Err(format!("\"format\" must be \"{format}\"")),
    }
    match object.remove("version").as_ref().and_then(Value::as_u64) {
        Some(found) if found == u64::from(version) => Ok(()),
        Some(found) => Err(format!(
            "this file is format version {found} but the project is version {version}; a file saved by another version, often merged from another branch, must be converted with the rest of the project"
        )),
        None => Err("\"version\" must be a whole number".into()),
    }
}

fn encode_document(
    layout: &DocumentLayout,
    version: u32,
    value: &Value,
) -> std::result::Result<Vec<u8>, String> {
    let Some(object) = value.as_object() else {
        return Err("a document must be a JSON object".into());
    };
    if let Some(key) = object.keys().find(|k| !layout.fields.contains(&k.as_str())) {
        return Err(format!(
            "unknown member \"{key}\"; {} has {}",
            layout.format(),
            layout.fields.join(", ")
        ));
    }
    let mut members: Vec<Vec<u8>> = Vec::new();
    let mut head = Vec::new();
    write_member("format", &Value::String(layout.format()), &mut head)?;
    members.push(head);
    let mut head = Vec::new();
    write_member("version", &Value::from(version), &mut head)?;
    members.push(head);
    for field in layout.fields {
        let Some(item) = object.get(*field) else {
            continue;
        };
        check_value(item, layout.numbers, &format!("/{field}"), 0)?;
        let mut member = Vec::new();
        match item {
            Value::Array(rows) if layout.tables.contains(field) && !rows.is_empty() => {
                write_string(field, &mut member)?;
                member.extend_from_slice(b":[\n");
                for (k, row) in rows.iter().enumerate() {
                    member.extend_from_slice(b"    ");
                    write_value(row, &mut member)?;
                    if k + 1 < rows.len() {
                        member.push(b',');
                    }
                    member.push(b'\n');
                }
                member.extend_from_slice(b"  ]");
            }
            _ => write_member(field, item, &mut member)?,
        }
        members.push(member);
    }
    let mut out = b"{\n".to_vec();
    let count = members.len();
    for (k, member) in members.into_iter().enumerate() {
        out.extend_from_slice(b"  ");
        out.extend_from_slice(&member);
        if k + 1 < count {
            out.push(b',');
        }
        out.push(b'\n');
    }
    out.extend_from_slice(b"}\n");
    Ok(out)
}

fn write_member(key: &str, value: &Value, out: &mut Vec<u8>) -> std::result::Result<(), String> {
    write_string(key, out)?;
    out.push(b':');
    write_value(value, out)
}

/// A value without spaces, object keys sorted by their UTF-8 bytes
/// (`serde_json`'s map is ordered by key).
fn write_value(value: &Value, out: &mut Vec<u8>) -> std::result::Result<(), String> {
    serde_json::to_writer(out, value).map_err(|e| e.to_string())
}

fn write_string(text: &str, out: &mut Vec<u8>) -> std::result::Result<(), String> {
    serde_json::to_writer(out, text).map_err(|e| e.to_string())
}

/// No `null`, strings in NFC, floats only where allowed, bounded depth.
/// `at` is the JSON pointer of `value` within its record.
fn check_value(
    value: &Value,
    numbers: Numbers,
    at: &str,
    depth: usize,
) -> std::result::Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{at} nests deeper than {MAX_DEPTH} levels"));
    }
    match value {
        Value::Null => Err(format!(
            "{at} is null; an absent value is left out, and a list holds no null"
        )),
        Value::Bool(_) => Ok(()),
        Value::Number(n) => {
            if numbers == Numbers::IntegersOnly && !(n.is_i64() || n.is_u64()) {
                Err(format!(
                    "{at} is {n}, not a whole number; this file holds integers only"
                ))
            } else {
                Ok(())
            }
        }
        Value::String(text) => {
            check_nfc(text).map_err(|()| format!("{at} is not in Unicode normal form C"))
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(k, item)| check_value(item, numbers, &format!("{at}/{k}"), depth + 1)),
        Value::Object(map) => map.iter().try_for_each(|(key, item)| {
            check_nfc(key)
                .map_err(|()| format!("{at}: the key \"{key}\" is not in Unicode normal form C"))?;
            check_value(
                item,
                numbers,
                &format!("{at}/{}", pointer_token(key)),
                depth + 1,
            )
        }),
    }
}

fn check_nfc(text: &str) -> std::result::Result<(), ()> {
    if text.is_ascii() {
        return Ok(());
    }
    match sparksmith_generator_abi::encode(&text) {
        Err(sparksmith_generator_abi::CodecError::NotNfc) => Err(()),
        _ => Ok(()),
    }
}

fn pointer_token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The lines of a file, numbered from 1, without their LF. Refuses a byte
/// order mark, a CR, an empty line, a missing final LF, a line of
/// `max_line` bytes or more with its LF, and text that is not UTF-8.
fn split_lines(
    bytes: &[u8],
    max_line: usize,
) -> std::result::Result<impl Iterator<Item = (usize, &[u8])>, (usize, String)> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err((
            1,
            "starts with a byte order mark; files are UTF-8 without one".into(),
        ));
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        let line = bytes.iter().filter(|b| **b == b'\n').count() + 1;
        return Err((
            line,
            "has no line end; every line, the last included, ends with LF".into(),
        ));
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let lines: Vec<&[u8]> = if bytes.is_empty() {
        Vec::new()
    } else {
        body.split(|b| *b == b'\n').collect()
    };
    for (k, line) in lines.iter().enumerate() {
        let number = k + 1;
        if line.ends_with(b"\r") || line.contains(&b'\r') {
            return Err((
                number,
                "has a CR; files use LF line ends only (check git's core.autocrlf)".into(),
            ));
        }
        if line.is_empty() {
            return Err((number, "is empty".into()));
        }
        if line.len() >= max_line {
            return Err((
                number,
                format!("is longer than the {max_line} bytes allowed"),
            ));
        }
        if std::str::from_utf8(line).is_err() {
            return Err((number, "is not UTF-8 text".into()));
        }
    }
    Ok(lines.into_iter().enumerate().map(|(k, line)| (k + 1, line)))
}

fn line_of_key(bytes: &[u8], key: &str) -> usize {
    let needle = format!("\n  {}:", serde_json::to_string(key).unwrap_or_default());
    match bytes
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())
    {
        Some(at) => bytes[..=at].iter().filter(|b| **b == b'\n').count() + 1,
        None => 1,
    }
}
