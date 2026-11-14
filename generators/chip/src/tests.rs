//! Native tests: the golden hashes, hand-derived geometry and refusals.
use super::*;
use sparksmith_generator_sdk::golden;

fn request(scope: Scope, overrides: &[(String, ParamValue)]) -> Request {
    let mut params = v1::Params::new();
    for param in description().params {
        params.insert(param.key, param.default);
    }
    for (key, value) in overrides {
        params.insert(key.clone(), value.clone());
    }
    Request {
        abi: v1::ABI,
        scope,
        params,
    }
}

fn preset(key: &str) -> Vec<(String, ParamValue)> {
    let description = description();
    let found = description.presets.iter().find(|p| p.key == key);
    found
        .map(|p| p.params.clone())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn kv(key: &str, value: ParamValue) -> (String, ParamValue) {
    (key.into(), value)
}

fn pads(g: &Generation) -> Vec<([i64; 2], PadShape)> {
    g.footprint
        .pads
        .iter()
        .map(|p| (p.at, p.shape.clone()))
        .collect()
}

#[test]
fn golden_hashes_match_natively() -> Result<(), String> {
    let checked = golden::check::<Chip>(include_str!("../golden.json"))?;
    assert!(checked >= 3);
    Ok(())
}

#[test]
fn presets_are_the_starter_rows_and_all_generate() -> Result<(), Vec<Problem>> {
    let description = description();
    assert_eq!(description.presets.len(), 24);
    for p in &description.presets {
        let overrides = preset(&p.key);
        for scope in [Scope::Component, Scope::Package] {
            Chip::generate(&request(scope, &overrides))?;
        }
    }
    Ok(())
}

#[test]
fn resistor_0603_lands_courtyard_and_part() -> Result<(), Vec<Problem>> {
    // 1.6 × 0.8 mm body, 0.3 mm terminations: leads at ±(1.6 − 0.3)/2 =
    // ±0.65 mm; lands 0.3 + 0.6 = 0.9 mm by 0.8 + 0.2 = 1.0 mm. Inner edges
    // at ±0.2 mm already keep the 0.2 mm least gap, so nothing is trimmed.
    let g = Chip::generate(&request(Scope::Component, &preset("resistor-0603")))?;
    let size = PadShape::Rect {
        size: [900_000, 1_000_000],
    };
    assert_eq!(
        pads(&g),
        vec![([-650_000, 0], size.clone()), ([650_000, 0], size)]
    );
    // Lands reach ±1.1 × ±0.5 mm; + 0.25 mm, on the 0.05 mm grid.
    assert_eq!(
        g.footprint.courtyard,
        vec![
            [-1_350_000, -750_000],
            [1_350_000, -750_000],
            [1_350_000, 750_000],
            [-1_350_000, 750_000]
        ]
    );
    assert_eq!(
        g.package.geometry.envelope_nm,
        [1_600_000, 800_000, 450_000]
    );
    assert_eq!(g.package.geometry.leads[1].contact, [650_000, 0, 0]);
    let component = g
        .component
        .as_ref()
        .map(|c| (c.prefix.as_str(), c.name.as_str()));
    assert_eq!(component, Some(("R", "Chip resistor 1.60 × 0.80 mm")));
    assert_eq!(g.binding.len(), 2);
    let pins: Vec<([i64; 2], i64)> = g
        .symbol
        .iter()
        .flat_map(|s| &s.units)
        .flat_map(|u| &u.pins)
        .map(|p| (p.at, p.length_nm))
        .collect();
    assert_eq!(
        pins,
        vec![([-5_000_000, 0], 2_500_000), ([5_000_000, 0], 2_500_000)]
    );
    Ok(())
}

#[test]
fn small_chip_lands_keep_the_least_gap() -> Result<(), Vec<Problem>> {
    // 0201: 0.6 mm body, 0.15 mm terminations, 0.3 mm wide. Leads at
    // ±0.225 mm, lands 0.75 × 0.5 mm reach ±0.6 mm outside but would meet
    // inside; the heel is trimmed to ±0.1 mm (half the 0.2 mm gap): lands
    // 0.5 mm long centred at ±0.35 mm.
    let g = Chip::generate(&request(Scope::Component, &preset("resistor-0201")))?;
    let size = PadShape::Rect {
        size: [500_000, 500_000],
    };
    assert_eq!(
        pads(&g),
        vec![([-350_000, 0], size.clone()), ([350_000, 0], size)]
    );
    // The project's gap rule is a frozen parameter: 0.3 mm trims further.
    let mut wider = preset("resistor-0201");
    wider.push(kv("land_gap_min", ParamValue::Int(300_000)));
    let g = Chip::generate(&request(Scope::Component, &wider))?;
    assert_eq!(pads(&g).first().map(|p| p.0), Some([-375_000, 0]));
    Ok(())
}

#[test]
fn led_uses_the_recommended_lands_and_names_its_pins() -> Result<(), Vec<Problem>> {
    // Kingbright APT1608: 0.8 × 0.8 mm lands, 1.65 mm centre to centre.
    let g = Chip::generate(&request(Scope::Component, &preset("led-0603")))?;
    let size = PadShape::Rect {
        size: [800_000, 800_000],
    };
    assert_eq!(
        pads(&g),
        vec![([-825_000, 0], size.clone()), ([825_000, 0], size)]
    );
    let names: Vec<&str> = g
        .component
        .iter()
        .flat_map(|c| &c.terminals)
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, vec!["K", "A"]);
    assert!(g.notes.is_empty());
    Ok(())
}

#[test]
fn package_scope_has_no_component_symbol_or_binding() -> Result<(), Vec<Problem>> {
    let g = Chip::generate(&request(Scope::Package, &[]))?;
    assert!(g.component.is_none() && g.symbol.is_none() && g.binding.is_empty());
    assert_eq!(g.footprint.pads.len(), 2);
    Ok(())
}

#[test]
fn refusals_name_the_field_in_plain_words() {
    let codes = |r: Result<Generation, Vec<Problem>>| -> Vec<(String, Option<String>)> {
        r.err()
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.code, p.field))
            .collect()
    };
    let long = Chip::generate(&request(
        Scope::Component,
        &[kv("termination", ParamValue::Int(800_000))],
    ));
    assert_eq!(
        long.as_ref()
            .err()
            .and_then(|e| e.first())
            .map(|p| p.message.as_str()),
        Some("The two terminations (2 × 0.80 mm) are not shorter than the body (1.60 mm).")
    );
    assert_eq!(
        codes(Chip::generate(&request(
            Scope::Component,
            &[kv("length", ParamValue::Int(9 * MM))]
        ))),
        vec![("param.out_of_range".into(), Some("length".into()))]
    );
    let mut unknown = request(Scope::Component, &[]);
    unknown.params.insert("pitch".into(), ParamValue::Int(1));
    unknown.params.remove("height");
    assert_eq!(
        codes(Chip::generate(&unknown)),
        vec![
            ("param.unknown".into(), Some("pitch".into())),
            ("param.missing".into(), Some("height".into()))
        ]
    );
    assert_eq!(
        codes(Chip::generate(&request(
            Scope::Component,
            &[kv("part", ParamValue::Text("diode".into()))]
        ))),
        vec![("param.out_of_range".into(), Some("part".into()))]
    );
}

/// The part file and values of a generation's model.
fn part_file(g: &Generation) -> Result<(&str, &v1::Params), String> {
    match &g.model {
        Some(v1::Model::PartFile { asset, values }) => Ok((asset, values)),
        None => Err("no model".into()),
    }
}

#[test]
fn every_preset_names_its_part_file_with_every_value() -> Result<(), String> {
    for p in &description().presets {
        for scope in [Scope::Component, Scope::Package] {
            let g =
                Chip::generate(&request(scope, &preset(&p.key))).map_err(|e| format!("{e:?}"))?;
            let (asset, values) = part_file(&g)?;
            let (file, extra): (&str, &[&str]) = match p.params.get("part") {
                Some(ParamValue::Text(t)) if t == "resistor" => {
                    (parts::CHIP_RESISTOR, &["overcoat"])
                }
                Some(ParamValue::Text(t)) if t == "capacitor" => (parts::CHIP_CAPACITOR, &[]),
                Some(ParamValue::Text(t)) if t == "led" => {
                    (parts::CHIP_LED, &["substrate", "lens_round"])
                }
                _ => (parts::CHIP_FERRITE, &[]),
            };
            assert_eq!(asset, file, "{}", p.key);
            let mut names: Vec<&str> = values.keys().map(String::as_str).collect();
            names.sort_unstable();
            let mut expected = vec![
                "body_round",
                "end_round",
                "height",
                "length",
                "termination",
                "width",
            ];
            expected.extend(extra);
            expected.sort_unstable();
            assert_eq!(names, expected, "{}", p.key);
            assert!(
                values
                    .values()
                    .all(|v| matches!(v, ParamValue::Int(n) if *n > 0)),
                "{}",
                p.key
            );
        }
    }
    Ok(())
}

#[test]
fn the_part_file_hashes_are_the_bundles_files() -> Result<(), String> {
    for (stem, hash) in [
        ("chip-resistor", parts::CHIP_RESISTOR),
        ("chip-capacitor", parts::CHIP_CAPACITOR),
        ("chip-ferrite", parts::CHIP_FERRITE),
        ("chip-led", parts::CHIP_LED),
    ] {
        let path = format!("{}/parts/{stem}.solidcraft", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        assert_eq!(
            sparksmith_generator_sdk::abi::hex(&sparksmith_generator_sdk::abi::sha256(&bytes)),
            hash
        );
    }
    Ok(())
}
