//! Layouts A and B against the showcase board's real records: round trip,
//! deterministic bytes, declared field order, sort order, one record per
//! line, and refusals that name the line.
use super::*;
use crate::format::FILE_VERSION;
use crate::format::records::{self, COLLECTIONS, DOCUMENTS};
use crate::model::production::Waiver;
use crate::model::{
    ComponentInstance, Fill, Junction, Keepout, Label, Net, PartDimension, Placement,
    ProjectProperties, SheetDrawing, SheetSymbol, SymbolOccurrence, Track, Via, Wire,
};
use crate::{Engine, Project};
use serde_json::json;

type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

fn showcase() -> Result<Project> {
    Ok(Engine::showcase()?.project().clone())
}

fn values<T: Serialize>(items: &[T]) -> Result<Vec<Value>> {
    items
        .iter()
        .map(|item| Ok(serde_json::to_value(item)?))
        .collect()
}

/// Write `records`, read them back, and check that the bytes are the same
/// whatever order the records came in, that every line after the header
/// is one record, and that writing what was read gives the same bytes.
fn round_trip<T>(
    collection: &Collection,
    header: &Map<String, Value>,
    records: &[T],
) -> Result<Vec<u8>>
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug + Clone,
{
    let bytes = write_collection(collection, FILE_VERSION, header, records)?;
    let mut reversed = records.to_vec();
    reversed.reverse();
    assert_eq!(
        write_collection(collection, FILE_VERSION, header, &reversed)?,
        bytes
    );
    let text = String::from_utf8(bytes.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
    assert_eq!(
        text.lines().count(),
        records.len() + 1,
        "one record per line"
    );
    let read: CollectionFile<T> =
        read_collection(collection.path, &bytes, collection, FILE_VERSION)?;
    assert_eq!(&read.header, header);
    assert_eq!(read.records.len(), records.len());
    for record in records {
        assert!(
            read.records.contains(record),
            "{record:?} did not read back"
        );
    }
    assert_eq!(
        write_collection(collection, FILE_VERSION, header, &read.records)?,
        bytes
    );
    Ok(bytes)
}

fn no_header() -> Map<String, Value> {
    Map::new()
}

/// The keys of every line after the header, in file order.
fn line_keys(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    text.lines()
        .skip(1)
        .map(|line| {
            let keys = line
                .trim_start_matches('{')
                .split(',')
                .filter_map(|part| part.strip_prefix('"'))
                .filter_map(|part| part.split_once("\":").map(|(k, _)| k.to_owned()))
                .collect();
            Ok(keys)
        })
        .collect()
}

#[test]
fn the_showcase_board_round_trips_collection_by_collection() -> Fallible {
    let project = showcase()?;
    assert!(!project.tracks.is_empty() && !project.vias.is_empty());
    round_trip(&records::INSTANCES, &no_header(), &project.instances[..])?;
    round_trip(&records::NETS, &no_header(), &project.nets[..])?;
    round_trip(&records::PLACEMENTS, &no_header(), &project.placements[..])?;
    round_trip(&records::TRACKS, &no_header(), &project.tracks[..])?;
    round_trip(&records::VIAS, &no_header(), &project.vias[..])?;
    round_trip(&records::KEEPOUTS, &no_header(), &project.keepouts[..])?;
    round_trip(
        &records::DIMENSIONS,
        &no_header(),
        &project.part_dimensions[..],
    )?;
    let fills: Vec<Fill> = project
        .fills
        .iter()
        .cloned()
        .map(|mut fill| {
            fill.computed = None;
            fill
        })
        .collect();
    assert!(!fills.is_empty());
    round_trip(&records::FILLS, &no_header(), &fills)?;
    let waivers = [
        Waiver {
            key: "b".into(),
            reason: "checked by hand".into(),
        },
        Waiver {
            key: "a".into(),
            reason: "fab confirmed".into(),
        },
    ];
    round_trip(&records::WAIVERS, &no_header(), &waivers)?;
    Ok(())
}

/// A fill that still carries its computed copper cannot be written: the
/// copper is derived data, not a declared field.
#[test]
fn computed_fill_copper_is_not_a_stored_field() -> Fallible {
    let project = showcase()?;
    let fill = project.fills.iter().find(|f| f.computed.is_some());
    let Some(fill) = fill else {
        return Err("the showcase has no filled pour".into());
    };
    let refused = write_collection(&records::FILLS, FILE_VERSION, &no_header(), &[fill]);
    assert!(refused.is_err_and(|e| e.to_string().contains("unknown field \"computed\"")),);
    Ok(())
}

/// The schematic items of one sheet in one file, typed and sorted by type
/// then id, without the sheet each record names today.
#[test]
fn a_sheet_is_one_mixed_collection() -> Fallible {
    let project = Engine::demo()?.project().clone();
    let mut items = Vec::new();
    let mut add = |kind: &str, value: Value| -> Result<()> {
        let mut value = value;
        let Some(object) = value.as_object_mut() else {
            return Err(Error::Invalid("not an object".into()));
        };
        object.remove("sheet");
        object.insert("type".into(), json!(kind));
        items.push(value);
        Ok(())
    };
    for v in values(&project.occurrences)? {
        add("occurrence", v)?;
    }
    for v in values(&project.wires)? {
        add("wire", v)?;
    }
    for v in values(&project.junctions)? {
        add("junction", v)?;
    }
    for v in values(&project.labels)? {
        add("label", v)?;
    }
    assert!(items.len() >= 3, "{items:?}");
    let mut header = Map::new();
    header.insert("sheet".into(), json!(1));
    let bytes = round_trip(&records::SHEET, &header, &items)?;
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let first = text.lines().next().unwrap_or_default();
    assert_eq!(
        first,
        format!("{{\"format\":\"sparksmith/sheet\",\"version\":{FILE_VERSION},\"sheet\":1}}")
    );
    let order: Vec<(String, u64)> = text
        .lines()
        .skip(1)
        .map(|line| {
            let v: Value = serde_json::from_str(line).unwrap_or_default();
            (
                v["type"].as_str().unwrap_or_default().to_owned(),
                v["id"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    for keys in line_keys(text.as_bytes())? {
        assert_eq!(
            keys.get(..2),
            Some(&["id".to_owned(), "type".to_owned()][..])
        );
    }
    Ok(())
}

/// Lines start with the id and keep the declared order; tracks and vias
/// are sorted by net, then id.
#[test]
fn tracks_are_written_id_first_and_sorted_by_net() -> Fallible {
    let track = |id: u64, net: u64, x: f64| -> Result<Track> {
        Ok(serde_json::from_value(json!({
            "id": id, "net": net, "start": [x, 7.355], "end": [x, 8.6],
            "width": 0.3, "layer": "Top", "locked": false
        }))?)
    };
    let tracks = [
        track(396, 6, 20.75)?,
        track(12, 9, 1.0)?,
        track(394, 6, 19.75)?,
    ];
    let bytes = write_collection(&records::TRACKS, FILE_VERSION, &no_header(), &tracks)?;
    assert_eq!(
        String::from_utf8(bytes).map_err(|e| e.to_string())?,
        format!(
            "{{\"format\":\"sparksmith/tracks\",\"version\":{FILE_VERSION}}}\n\
             {{\"id\":394,\"net\":6,\"layer\":\"Top\",\"width\":0.3,\"start\":[19.75,7.355],\"end\":[19.75,8.6],\"locked\":false}}\n\
             {{\"id\":396,\"net\":6,\"layer\":\"Top\",\"width\":0.3,\"start\":[20.75,7.355],\"end\":[20.75,8.6],\"locked\":false}}\n\
             {{\"id\":12,\"net\":9,\"layer\":\"Top\",\"width\":0.3,\"start\":[1.0,7.355],\"end\":[1.0,8.6],\"locked\":false}}\n"
        )
    );
    Ok(())
}

/// The declared order of the main record types, pinned: a change here
/// changes every file of that kind, so it is a format version change.
#[test]
fn the_declared_orders_are_pinned() {
    let order = |c: &Collection, kind: Option<&str>| c.record(kind).map(|r| r.fields.to_vec());
    let pinned: &[(&Collection, Option<&str>, &[&str])] = &[
        (
            &records::TRACKS,
            None,
            &["id", "net", "layer", "width", "start", "end", "locked"],
        ),
        (
            &records::VIAS,
            None,
            &[
                "id", "net", "span", "drill", "diameter", "position", "tented", "locked",
            ],
        ),
        (
            &records::PLACEMENTS,
            None,
            &["instance", "layer", "position", "rotation_deg", "locked"],
        ),
        (&records::NETS, None, &["id", "name", "terminals"]),
        (
            &records::FILLS,
            None,
            &[
                "id", "net", "layer", "name", "region", "priority", "settings", "outline", "locked",
            ],
        ),
        (
            &records::SHEET,
            Some("wire"),
            &["id", "type", "from", "to", "corners"],
        ),
        (
            &records::SHEET,
            Some("occurrence"),
            &[
                "id", "type", "instance", "unit", "position", "rotation", "mirror",
            ],
        ),
        (
            &records::SHEET,
            Some("label"),
            &[
                "id", "type", "name", "kind", "position", "rotation", "manual",
            ],
        ),
    ];
    for (collection, kind, fields) in pinned {
        assert_eq!(
            order(collection, *kind),
            Some(fields.to_vec()),
            "{} {kind:?}",
            collection.name
        );
    }
    let sorts: Vec<(&str, &[&str])> = COLLECTIONS.iter().map(|c| (c.name, c.sort)).collect();
    assert_eq!(
        sorts,
        [
            ("instances", &["id"][..]),
            ("nets", &["id"]),
            ("sheet", &["type", "id"]),
            ("placements", &["instance"]),
            ("tracks", &["net", "id"]),
            ("vias", &["net", "id"]),
            ("fills", &["id"]),
            ("keepouts", &["id"]),
            ("dimensions", &["id"]),
            ("waivers", &["key"]),
        ]
    );
}

fn schema_fields<T: schemars::JsonSchema>() -> Result<Vec<String>> {
    let schema = serde_json::to_value(schemars::schema_for!(T))?;
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Err(Error::Invalid("the schema has no properties".into()));
    };
    let mut fields: Vec<String> = properties.keys().cloned().collect();
    fields.sort();
    Ok(fields)
}

/// Every stored field of each record type is declared, and nothing else,
/// except the fields a declaration leaves out on purpose: the sheet (the
/// file says it), the record type (the file adds it) and computed fill
/// copper (derived data, moving to a cache).
#[test]
fn declarations_cover_every_stored_field() -> Fallible {
    fn check<T: schemars::JsonSchema>(
        collection: &Collection,
        kind: Option<&str>,
        left_out: &[&str],
    ) -> Fallible {
        let Some(order) = collection.record(kind) else {
            return Err(format!("{} {kind:?} is not declared", collection.name).into());
        };
        let mut declared: Vec<String> = order
            .fields
            .iter()
            .filter(|f| Some(**f) != collection.type_field)
            .map(|f| (*f).to_owned())
            .collect();
        declared.extend(left_out.iter().map(|f| (*f).to_owned()));
        declared.sort();
        assert_eq!(
            declared,
            schema_fields::<T>()?,
            "{} {kind:?}",
            collection.name
        );
        assert_eq!(
            order.fields.first().copied(),
            collection.sort.last().copied()
        );
        Ok(())
    }
    check::<ComponentInstance>(&records::INSTANCES, None, &[])?;
    check::<Net>(&records::NETS, None, &[])?;
    check::<Placement>(&records::PLACEMENTS, None, &[])?;
    check::<Track>(&records::TRACKS, None, &[])?;
    check::<Via>(&records::VIAS, None, &[])?;
    check::<Fill>(&records::FILLS, None, &["computed"])?;
    check::<Keepout>(&records::KEEPOUTS, None, &[])?;
    check::<PartDimension>(&records::DIMENSIONS, None, &[])?;
    check::<Waiver>(&records::WAIVERS, None, &[])?;
    check::<SymbolOccurrence>(&records::SHEET, Some("occurrence"), &["sheet"])?;
    check::<Wire>(&records::SHEET, Some("wire"), &["sheet"])?;
    check::<Junction>(&records::SHEET, Some("junction"), &["sheet"])?;
    check::<Label>(&records::SHEET, Some("label"), &["sheet"])?;
    check::<SheetSymbol>(&records::SHEET, Some("sheet_symbol"), &["sheet"])?;
    check::<SheetDrawing>(&records::SHEET, Some("drawing"), &["sheet"])?;
    // Paths and format names are unique, and every path follows the
    // tree's rules.
    let mut paths: Vec<&str> = COLLECTIONS.iter().map(|c| c.path).collect();
    paths.extend(DOCUMENTS.iter().map(|d| d.path));
    let count = paths.len();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(paths.len(), count);
    for path in paths {
        crate::format::files::file_kind(&path.replace("{sheet}", "1"))?;
    }
    Ok(())
}

fn track_file() -> Result<Vec<u8>> {
    Ok(format!(
        "{{\"format\":\"sparksmith/tracks\",\"version\":{FILE_VERSION}}}\n\
         {{\"id\":4,\"net\":6,\"layer\":\"Top\",\"width\":0.3,\"start\":[0.0,0.0],\"end\":[1.0,0.0],\"locked\":false}}\n\
         {{\"id\":5,\"net\":6,\"layer\":\"Top\",\"width\":0.3,\"start\":[1.0,0.0],\"end\":[2.0,0.0],\"locked\":false}}\n"
    )
    .into_bytes())
}

fn refusal(bytes: &[u8]) -> String {
    match read_collection::<Track>("board/tracks.jsonl", bytes, &records::TRACKS, FILE_VERSION) {
        Ok(_) => "accepted".into(),
        Err(error) => error.to_string(),
    }
}

/// Swap `from` for `to` in the canonical track file and read it.
fn edited(from: &str, to: &str) -> Result<String> {
    let text = String::from_utf8(track_file()?).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(text.contains(from), "{from}");
    Ok(refusal(text.replacen(from, to, 1).as_bytes()))
}

#[test]
fn malformed_lines_are_refused_with_their_number() -> Fallible {
    assert_eq!(refusal(&track_file()?), "accepted");
    let cases: &[(&str, &str, &str)] = &[
        (
            "\"locked\":false}\n{\"id\":5",
            "\"locked\":false}\r\n{\"id\":5",
            "line 2: has a CR",
        ),
        (
            "\"locked\":false}\n{\"id\":5",
            "\"locked\":false}\n\n{\"id\":5",
            "line 3: is empty",
        ),
        ("{\"id\":5,", "{\"id\":5,,", "line 3: not valid JSON"),
        (
            "{\"id\":5,\"net\":6,",
            "{\"net\":6,\"id\":5,",
            "line 3: the field \"id\" comes after \"net\"",
        ),
        (
            "\"locked\":false}\n{\"id\":5",
            "\"locked\":false,\"colour\":1}\n{\"id\":5",
            "line 2: unknown field \"colour\"",
        ),
        (
            "{\"id\":5,\"net\":6,",
            "{\"id\":5,\"id\":5,\"net\":6,",
            "line 3: the field \"id\" appears twice",
        ),
        (
            "{\"id\":5,\"net\":6,",
            "{\"id\":3,\"net\":6,",
            "line 3: is out of order",
        ),
        (
            "{\"id\":5,\"net\":6,",
            "{\"id\":4,\"net\":6,",
            "line 3: repeats the key (net 6, id 4) of line 2",
        ),
        (
            "{\"id\":5,\"net\":6,",
            "{\"id\":4,\"net\":7,",
            "line 3: repeats id 4 of line 2",
        ),
        (
            "\"width\":0.3,\"start\":[1.0",
            "\"width\":null,\"start\":[1.0",
            "line 3: /width is null",
        ),
        (
            "\"width\":0.3,\"start\":[1.0",
            "\"width\":0.30,\"start\":[1.0",
            "line 3: is not in the form this build writes",
        ),
        (
            "\"width\":0.3,\"start\":[1.0",
            "\"width\": 0.3,\"start\":[1.0",
            "line 3: is not in the form this build writes",
        ),
        (
            "\"width\":0.3,\"start\":[1.0",
            "\"width\":\"wide\",\"start\":[1.0",
            "line 3: does not describe a record",
        ),
        (
            "}\n{\"id\":4",
            ",\"x\":1}\n{\"id\":4",
            "line 1: the header has no member \"x\"",
        ),
        (
            "\"version\":",
            "\"version\": ",
            "line 1: the header must be",
        ),
        (
            "tracks\",\"version\"",
            "vias\",\"version\"",
            "line 1: this is a \"sparksmith/vias\" file",
        ),
    ];
    for (from, to, expected) in cases {
        let message = edited(from, to)?;
        assert!(
            message.starts_with("board/tracks.jsonl, line "),
            "{message}"
        );
        assert!(message.contains(expected), "{to:?}: {message}");
    }
    let other_version = edited(&format!("\"version\":{FILE_VERSION}"), "\"version\":1")?;
    assert!(
        other_version.contains("line 1: this file is format version 1"),
        "{other_version}"
    );
    assert!(other_version.contains("another branch"), "{other_version}");
    let mut unterminated = track_file()?;
    unterminated.pop();
    assert!(refusal(&unterminated).contains("line 3: has no line end"));
    assert!(refusal(b"").contains("is empty; line 1 must be the header"));
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend(track_file()?);
    assert!(refusal(&bom).contains("line 1: starts with a byte order mark"));
    Ok(())
}

#[test]
fn values_follow_the_value_rules() -> Fallible {
    // Floats are refused where a file holds integers only.
    let net = json!({"id": 1, "name": "GND", "terminals": [], "x": 1});
    assert!(write_collection(&records::NETS, FILE_VERSION, &no_header(), &[net]).is_err());
    let float = json!({"id": 1, "name": "GND", "terminals": [[1.5]]});
    let refused = write_collection(&records::NETS, FILE_VERSION, &no_header(), &[float]);
    assert!(refused.is_err_and(|e| {
        e.to_string()
            .contains("/terminals/0/0 is 1.5, not a whole number")
    }));
    // Text is in NFC: "é" as e and a combining accent is refused.
    let decomposed = json!({"id": 1, "name": "Re\u{301}seau", "terminals": []});
    let refused = write_collection(&records::NETS, FILE_VERSION, &no_header(), &[decomposed]);
    assert!(refused.is_err_and(|e| {
        e.to_string()
            .contains("/name is not in Unicode normal form C")
    }));
    // A null in a list is refused; a null member is left out.
    let listed = json!({"id": 1, "name": "GND", "terminals": [null]});
    let refused = write_collection(&records::NETS, FILE_VERSION, &no_header(), &[listed]);
    assert!(refused.is_err_and(|e| e.to_string().contains("/terminals/0 is null")));
    // Nested objects are written with sorted keys, and absent values are
    // left out.
    let nested =
        json!({"id": 1, "name": "N", "terminals": [{"terminal": 3, "instance": 2, "unit": null}]});
    let bytes = write_collection(&records::NETS, FILE_VERSION, &no_header(), &[nested])?;
    assert!(
        String::from_utf8(bytes)?.ends_with(
            "{\"id\":1,\"name\":\"N\",\"terminals\":[{\"instance\":2,\"terminal\":3}]}\n"
        )
    );
    // Two records with one id are refused, even on different nets.
    let a = json!({"id": 1, "net": 1, "layer": "Top", "width": 0.1, "start": [0, 0], "end": [1, 0], "locked": false});
    let b = json!({"id": 1, "net": 2, "layer": "Top", "width": 0.1, "start": [0, 0], "end": [1, 0], "locked": false});
    let refused = write_collection(&records::TRACKS, FILE_VERSION, &no_header(), &[a, b]);
    assert!(refused.is_err_and(|e| e.to_string().contains("two records have id 1")));
    Ok(())
}

#[test]
fn documents_are_laid_out_one_member_per_line() -> Fallible {
    let properties = ProjectProperties {
        title: "Dev board".into(),
        revision: "B".into(),
        date: String::new(),
        author: "A. Person".into(),
        company: String::new(),
    };
    let bytes = write_document(&records::PROPERTIES, FILE_VERSION, &properties)?;
    let text = String::from_utf8(bytes.clone())?;
    assert_eq!(
        text,
        format!(
            "{{\n  \"format\":\"sparksmith/properties\",\n  \"version\":{FILE_VERSION},\n  \"title\":\"Dev board\",\n  \"revision\":\"B\",\n  \"date\":\"\",\n  \"author\":\"A. Person\",\n  \"company\":\"\"\n}}\n"
        )
    );
    let read: ProjectProperties = read_document(
        "properties.json",
        &bytes,
        &records::PROPERTIES,
        FILE_VERSION,
    )?;
    assert_eq!(read, properties);
    // A table has one element per line.
    let sheets = json!({"sheets": [{"id": 1, "name": "Power"}, {"id": 3, "name": "MCU"}]});
    let bytes = write_document(&records::SHEETS, FILE_VERSION, &sheets)?;
    assert_eq!(
        String::from_utf8(bytes.clone())?,
        format!(
            "{{\n  \"format\":\"sparksmith/sheets\",\n  \"version\":{FILE_VERSION},\n  \"sheets\":[\n    {{\"id\":1,\"name\":\"Power\"}},\n    {{\"id\":3,\"name\":\"MCU\"}}\n  ]\n}}\n"
        )
    );
    let read: Value = read_document(
        "schematic/sheets.json",
        &bytes,
        &records::SHEETS,
        FILE_VERSION,
    )?;
    assert_eq!(read, sheets);
    Ok(())
}

#[test]
fn malformed_documents_are_refused_with_their_line() -> Fallible {
    let properties = ProjectProperties {
        title: "T".into(),
        ..Default::default()
    };
    let bytes = write_document(&records::PROPERTIES, FILE_VERSION, &properties)?;
    let text = String::from_utf8(bytes)?;
    let refusal = |text: &str| match read_document::<ProjectProperties>(
        "properties.json",
        text.as_bytes(),
        &records::PROPERTIES,
        FILE_VERSION,
    ) {
        Ok(_) => "accepted".to_owned(),
        Err(error) => error.to_string(),
    };
    assert_eq!(refusal(&text), "accepted");
    let swapped = text.replace(
        "  \"title\":\"T\",\n  \"revision\":\"\",",
        "  \"revision\":\"\",\n  \"title\":\"T\",",
    );
    assert!(
        refusal(&swapped).contains("properties.json, line 4: is not in the form"),
        "{}",
        refusal(&swapped)
    );
    let unknown = text.replace(
        "  \"company\":\"\"\n",
        "  \"company\":\"\",\n  \"colour\":\"red\"\n",
    );
    assert!(
        refusal(&unknown).contains("line 9: unknown member \"colour\""),
        "{}",
        refusal(&unknown)
    );
    assert!(refusal(&text.replace('\n', "\r\n")).contains("line 1: has a CR"));
    let older = text.replace(&format!("\"version\":{FILE_VERSION}"), "\"version\":1");
    assert!(refusal(&older).contains("format version 1"));
    assert!(refusal("[]\n").contains("line 1: the file must hold one JSON object"));
    Ok(())
}

/// Many generated tracks on a few nets, given in a scrambled order: the
/// bytes do not depend on the order, the file reads back the same records,
/// and the lines come sorted by net, then id.
#[test]
fn generated_tracks_round_trip_in_any_order() -> Fallible {
    let mut state: u64 = 0x5eed;
    let mut next = move |bound: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % bound
    };
    let mut tracks = Vec::new();
    for id in 1..=500_u64 {
        let x = next(100_000) as f64 / 1000.0;
        let layer = if next(2) == 0 { "Top" } else { "Bottom" };
        tracks.push(serde_json::from_value::<Track>(json!({
            "id": id * 7, "net": next(12) + 1, "start": [x, 1.25], "end": [x + 0.5, 1.25],
            "width": 0.15 + next(5) as f64 / 20.0, "layer": layer, "locked": next(3) == 0
        }))?);
    }
    let bytes = round_trip(&records::TRACKS, &no_header(), &tracks)?;
    for _ in 0..5 {
        for k in (1..tracks.len()).rev() {
            let other = usize::try_from(next(k as u64 + 1))?;
            tracks.swap(k, other);
        }
        assert_eq!(
            write_collection(&records::TRACKS, FILE_VERSION, &no_header(), &tracks)?,
            bytes
        );
    }
    let read: CollectionFile<Track> =
        read_collection("board/tracks.jsonl", &bytes, &records::TRACKS, FILE_VERSION)?;
    let keys: Vec<(u64, u64)> = read.records.iter().map(|t| (t.net, t.id)).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
    Ok(())
}
