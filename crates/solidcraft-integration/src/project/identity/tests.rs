use super::*;

fn source(pick_id: u64, component: u64) -> Source {
    Source {
        pick_id,
        instance: None,
        pad: None,
        track: None,
        via: None,
        net: None,
        layer: None,
        label: String::new(),
        component,
        occurrence: None,
    }
}

/// The largest board the document allows, built as the board builder
/// records it: 2,000 parts (an envelope or a shared model each), 4,096
/// pads (through-hole, so with an opposite land and a barrel), 2,000
/// tracks and 16,384 vias of two sizes, nets changing at every via, and a
/// body name for each.
fn largest_board() -> (Vec<Source>, BTreeMap<String, BodyKind>) {
    let mut sources = Vec::new();
    let mut bodies = BTreeMap::new();
    let mut component = 40u64;
    let next = |sources: &Vec<Source>| u64::try_from(sources.len()).unwrap_or(u64::MAX) + 1;
    for k in 0..2_000u64 {
        let instance = 1_000 + 3 * k;
        let mut part = source(next(&sources), component);
        part.instance = Some(instance);
        part.layer = Some(if k % 3 == 0 {
            CopperLayer::Bottom
        } else {
            CopperLayer::Top
        });
        if k % 2 == 0 {
            part.label = format!("R{k} model");
            part.occurrence = Some(component + 1);
        } else {
            part.label = format!("U{k} envelope");
            bodies.insert(format!("instance_{instance}_body"), BodyKind::PartBody);
        }
        component += 2;
        sources.push(part);
    }
    for k in 0..4_096u64 {
        let instance = 1_000 + 3 * (k / 4);
        let pad = u32::try_from(k % 4 + 1).unwrap_or(1);
        let mut land = source(next(&sources), component);
        land.instance = Some(instance);
        land.pad = Some(PadId(pad));
        land.net = (k % 7 != 0).then_some(50_000 + k % 300);
        land.layer = Some(CopperLayer::Top);
        land.label = format!("J{} pad {}", k / 4, k % 4 + 1);
        for suffix in ["", "_opposite", "_barrel_0"] {
            bodies.insert(format!("pad_{instance}_{pad}{suffix}"), BodyKind::PadCopper);
        }
        component += 2;
        sources.push(land);
    }
    for k in 0..2_000u64 {
        let id = 100_000 + 5 * k + k % 3;
        let mut trace = source(next(&sources), component);
        trace.track = Some(id);
        trace.net = Some(50_000 + (k * 37) % 300);
        trace.layer = Some(match k % 4 {
            0 => CopperLayer::Top,
            1 => CopperLayer::Inner(1),
            2 => CopperLayer::Inner(2),
            _ => CopperLayer::Bottom,
        });
        trace.label = format!("Trace {id}");
        bodies.insert(format!("trace_{id}"), BodyKind::CopperTrack);
        component += 2;
        sources.push(trace);
    }
    let definitions = [component, component + 7];
    for (index, definition) in definitions.iter().enumerate() {
        for layer in 0..5 {
            bodies.insert(
                format!("via_0p{}_ring_{layer}", index + 3),
                BodyKind::ViaCopper,
            );
        }
        bodies.insert(format!("via_0p{}_barrel", index + 3), BodyKind::ViaCopper);
        let _ = definition;
    }
    let mut occurrence = component + 20;
    for k in 0..16_384u64 {
        let id = 200_000 + 2 * k;
        let mut via = source(next(&sources), definitions[usize::from(k % 50 == 0)]);
        via.via = Some(id);
        via.net = Some(50_000 + k % 2);
        via.occurrence = Some(occurrence);
        via.label = format!("Via {id}");
        occurrence += 2;
        sources.push(via);
    }
    for slab in 0..5 {
        bodies.insert(format!("laminate_{slab}"), BodyKind::Substrate);
    }
    (sources, bodies)
}

fn same(a: &[Source], b: &[Source]) -> Result<()> {
    assert_eq!(a.len(), b.len());
    assert_eq!(serde_json::to_value(a)?, serde_json::to_value(b)?);
    Ok(())
}

/// The largest board's identity fits a small part of Solidcraft's
/// metadata, in values below its per-value limit, and reads back exactly.
#[test]
fn the_largest_board_is_recorded_compactly_and_read_back_exactly() -> Result<()> {
    let (sources, bodies) = largest_board();
    let values = values(&sources, &bodies)?;
    let mut total = 0;
    for (key, value) in &values {
        let bytes = serde_json::to_string(value)?.len();
        assert!(bytes <= CHUNK_BYTES, "{key}: {bytes} bytes");
        total += key.len() + bytes + 4;
    }
    eprintln!(
        "{} sources and {} bodies in {} values, {total} bytes",
        sources.len(),
        bodies.len(),
        values.len()
    );
    // A quarter of Solidcraft's 256 KiB, leaving room for the models'
    // records (35 KiB measured).
    assert!(total < 64 * 1024, "{total} bytes");
    let (read, kinds) = read_values(&values)?;
    same(&read, &sources)?;
    assert_eq!(kinds, bodies);
    Ok(())
}

/// A saved native file holds that identity through Solidcraft's own
/// metadata limits, and reopening it reads it back.
#[test]
fn the_largest_board_identity_survives_a_native_save_and_reopen() -> Result<()> {
    let (sources, bodies) = largest_board();
    let mut native = Native::new("identity")?;
    write(&mut native, &sources, &bodies)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("identity.solidcraft")
        .to_string_lossy()
        .into_owned();
    native.save_new(&path)?;
    let reopened = Native::open(&path)?;
    let (read, kinds) = read(&reopened)?;
    same(&read, &sources)?;
    assert_eq!(kinds, bodies);
    Ok(())
}

/// Sources in any order are recorded in pick order, and labels a via's or
/// track's id does not give are kept as written.
#[test]
fn unusual_sources_and_labels_read_back_as_written() -> Result<()> {
    let mut sources = Vec::new();
    for (k, label) in [
        "Via 9",
        "Via 10",
        "Trace 3",
        "",
        "  ",
        "R1 pad  7",
        "ü pad ß",
        "Board",
    ]
    .into_iter()
    .enumerate()
    {
        let k = k as u64;
        let mut s = source(20 - k, u64::MAX - k);
        s.via = (k < 2).then_some(9);
        s.track = (k == 2).then_some(3);
        s.instance = (k == 5).then_some(u64::MAX);
        s.pad = (k == 5).then_some(PadId(u32::MAX));
        s.occurrence = (k == 6).then_some(0);
        s.net = (k == 7).then_some(0);
        s.layer = Some(CopperLayer::Inner(u8::try_from(k).unwrap_or(1)));
        s.label = label.into();
        sources.push(s);
    }
    let bodies = BTreeMap::from([("b".to_owned(), BodyKind::Substrate)]);
    let (read, _) = read_values(&values(&sources, &bodies)?)?;
    sources.sort_by_key(|s| s.pick_id);
    same(&read, &sources)
}

/// Body names keep every character through their templates: literal
/// braces, leading zeros, long digit runs, many numbers and non-ASCII text.
#[test]
fn body_name_templates_are_lossless() -> Result<()> {
    let names = [
        "{}",
        "a{b}c{{",
        "007",
        "0",
        "x12y0z00",
        "via_0p3_0p6_ring_4",
        "ü1ß22",
        "12345678901234567890123",
        "1_2_3_4_5_6_7_8_9_10_11_12_13_14_15_16_17_18_19_20",
        "",
        "{0}",
    ];
    for name in names {
        let (template, numbers) = template(name);
        assert!(numbers.len() <= MAX_NAME_NUMBERS);
        assert_eq!(fill(&template, &numbers)?, name, "{template}");
    }
    let bodies: BTreeMap<String, BodyKind> = names
        .iter()
        .enumerate()
        .map(|(k, n)| ((*n).to_owned(), BodyKind::ALL[k % BodyKind::ALL.len()]))
        .collect();
    let (_, kinds) = read_values(&values(&[source(1, 1)], &bodies)?)?;
    assert_eq!(kinds, bodies);
    Ok(())
}

/// Escaped names are measured as written: quotes and backslashes take two
/// bytes, and a name too long for one value is refused. The names have no
/// numbers, so each is a template of its own.
#[test]
fn records_measure_escaped_names_and_refuse_one_too_large() -> Result<()> {
    let mut bodies = BTreeMap::new();
    for k in 0..3_000 {
        let kind = BodyKind::ALL[k % BodyKind::ALL.len()];
        let letters: String = format!("{k:04}")
            .bytes()
            .map(|d| char::from(b'a' + (d - b'0')))
            .collect();
        bodies.insert(format!("n{letters} \\\"{}", "\\".repeat(20)), kind);
    }
    let values = values(&[source(1, 1)], &bodies)?;
    assert!(values.len() > 2);
    for value in values.values() {
        assert!(serde_json::to_string(value)?.len() <= CHUNK_BYTES);
    }
    assert_eq!(read_values(&values)?.1, bodies);
    bodies.insert("x".repeat(CHUNK_BYTES), BodyKind::Substrate);
    assert!(matches!(
        values_of(&bodies),
        Err(Error::Invalid(message)) if message.contains("too large")
    ));
    Ok(())
}

fn values_of(bodies: &BTreeMap<String, BodyKind>) -> Result<Map<String, Value>> {
    values(&[source(1, 1)], bodies)
}

/// Another version, a record whose columns disagree and a column claiming
/// more rows than any board are refused, not read in part.
#[test]
fn inconsistent_records_are_refused() -> Result<()> {
    let bodies = BTreeMap::from([("b".to_owned(), BodyKind::Substrate)]);
    let good = values(&[source(1, 1), source(2, 1)], &bodies)?;
    let mut newer = good.clone();
    newer.insert(METADATA_KEY.into(), json!({"version": 2}));
    assert!(matches!(read_values(&newer), Err(Error::Verification(_))));
    let tamper = |edit: &dyn Fn(&mut Value)| -> Result<bool> {
        let mut values = good.clone();
        if let Some(value) = values.get_mut(&format!("{RECORDS_KEY}0")) {
            edit(value);
        }
        Ok(read_values(&values).is_err())
    };
    assert!(tamper(
        &|v| v["records"][0]["sources"]["components"] = json!("1")
    )?);
    assert!(tamper(
        &|v| v["records"][0]["sources"]["picks"] = json!("1*99999999999")
    )?);
    assert!(tamper(
        &|v| v["records"][0]["sources"]["labels"] = json!([[null, 2]])
    )?);
    assert!(tamper(
        &|v| v["records"][0]["sources"]["nets"] = json!("0,5")
    )?);
    assert!(tamper(
        &|v| v["records"][1]["bodies"]["template"] = json!("{")
    )?);
    assert!(tamper(
        &|v| v["records"][0]["sources"]["layers"] = json!("0,999")
    )?);
    // A column longer than the record's picks is refused, however long.
    assert!(tamper(
        &|v| v["records"][0]["sources"]["nets"] = json!("0*1000000")
    )?);
    assert!(!tamper(&|_| {})?);
    // An identity of the current version without its records (an older
    // pre-release layout) says so.
    let mut older = Map::new();
    older.insert(METADATA_KEY.into(), json!({"version": VERSION}));
    older.insert("org.pcbcraft.electronics.bodies.0".into(), json!({}));
    assert!(matches!(
        read_values(&older),
        Err(Error::Verification(why)) if why.contains("older pre-release")
    ));
    Ok(())
}

/// Columns read back every sequence exactly, and runs and periodic runs
/// take a few bytes.
#[test]
fn columns_are_lossless_and_short_for_runs_and_periods() -> Result<()> {
    let mut seed = 7u64;
    let mut random = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        i128::from(seed >> 59) - 8
    };
    let noisy: Vec<i128> = (0..5_000).map(|_| random()).collect();
    let periodic: Vec<i128> = (0..4_096).map(|k| k % 4 + 1).collect();
    let mixed: Vec<i128> = (0..3_000)
        .map(|k| if k % 500 < 250 { k % 3 } else { 7 })
        .chain([i128::from(u64::MAX) + 1, -5, 0])
        .collect();
    for values in [&noisy, &periodic, &mixed, &Vec::new(), &vec![3]] {
        assert_eq!(
            &Column::values(values.iter().copied()).read_values(MAX_ROWS)?,
            values
        );
        assert_eq!(
            &Column::deltas(values.iter().copied()).read_deltas(MAX_ROWS)?,
            values
        );
    }
    assert!(Column::deltas(periodic.iter().copied()).0.len() < 32);
    assert!(Column::values(std::iter::repeat_n(5, 100_000)).0.len() < 16);
    for bad in [
        "(1 2",
        "(1 2)*x",
        "(1 2 3 4 5 6 7 8 9)*2",
        "1*0",
        "(1)*0",
        "x",
        "1,,2",
    ] {
        assert!(Column(bad.into()).read_values(MAX_ROWS).is_err(), "{bad}");
    }
    // A column claiming more values than its record has rows is refused
    // before it is expanded.
    assert!(Column("7*3".into()).read_values(3).is_ok());
    assert!(Column("7*4".into()).read_values(3).is_err());
    assert!(Column("(1 2)*2".into()).read_values(3).is_err());
    Ok(())
}
