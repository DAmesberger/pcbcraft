#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;

fn nm(v: i64) -> Nm {
    Nm::new(v).unwrap()
}

fn p(x: i64, y: i64) -> PointNm {
    PointNm::new(nm(x), nm(y))
}

fn mm(text: &str) -> i64 {
    parse_length(text, LengthUnit::Mm).unwrap().get()
}

#[test]
fn bounds_are_two_metres_either_sign() {
    assert!(Nm::new(MAX_COORD_NM).is_ok());
    assert!(Nm::new(-MAX_COORD_NM).is_ok());
    assert!(Nm::new(MAX_COORD_NM + 1).is_err());
    assert!(Nm::new(-MAX_COORD_NM - 1).is_err());
    assert_eq!(Nm::try_new(i64::MIN), None);
    assert!(nm(MAX_COORD_NM).checked_add(nm(1)).is_err());
    assert!(nm(-MAX_COORD_NM).checked_sub(nm(1)).is_err());
    assert_eq!(nm(5).checked_sub(nm(7)).unwrap(), nm(-2));
}

#[test]
fn the_mm_boundary_rounds_half_away_from_zero_and_refuses_bad_input() {
    assert_eq!(Nm::from_mm_f64_rounded(0.25).unwrap(), nm(250_000));
    assert_eq!(Nm::from_mm_f64_rounded(-0.25).unwrap(), nm(-250_000));
    assert_eq!(Nm::from_mm_f64_rounded(0.000_000_5).unwrap(), nm(1));
    assert_eq!(Nm::from_mm_f64_rounded(-0.000_000_5).unwrap(), nm(-1));
    assert_eq!(Nm::from_mm_f64_rounded(0.000_000_4).unwrap(), nm(0));
    assert_eq!(Nm::from_mm_f64_rounded(2000.0).unwrap(), nm(MAX_COORD_NM));
    assert!(Nm::from_mm_f64_rounded(2_000.000_001).is_err());
    assert!(Nm::from_mm_f64_rounded(f64::NAN).is_err());
    assert!(Nm::from_mm_f64_rounded(f64::INFINITY).is_err());
    assert!(Nm::from_mm_f64_rounded(-1e300).is_err());
}

/// Every value with at most six decimals crosses both ways exactly, and the
/// f64 prints as the exact decimal (what `Length` replies rely on).
#[test]
fn mm_round_trips_are_exact_for_six_decimals() {
    let mut v: i64 = 1;
    while v <= MAX_COORD_NM {
        for n in [v, -v, v - 1, MAX_COORD_NM - v, v / 3 * 2 + 1] {
            let n = nm(n);
            let f = n.to_mm_f64();
            assert_eq!(Nm::from_mm_f64_rounded(f).unwrap(), n);
            assert_eq!(f.to_string().parse::<f64>().unwrap(), f);
            assert_eq!(mm(&f.to_string()), n.get(), "{f}");
            assert_eq!(f.to_string(), format_number(n, LengthUnit::Mm));
        }
        v = v * 3 + 1;
    }
}

#[test]
fn the_parser_reads_every_unit_exactly() {
    assert_eq!(mm("0.25"), 250_000);
    assert_eq!(mm("0.25mm"), 250_000);
    assert_eq!(mm(" 0.25 MM "), 250_000);
    assert_eq!(mm("250um"), 250_000);
    assert_eq!(mm("250 µm"), 250_000);
    assert_eq!(mm("250\u{3bc}m"), 250_000);
    assert_eq!(mm("1mil"), 25_400);
    assert_eq!(mm("10 mil"), 254_000);
    assert_eq!(mm("10mils"), 254_000);
    assert_eq!(mm("10thou"), 254_000);
    assert_eq!(mm("1in"), 25_400_000);
    assert_eq!(mm("0.05in"), 1_270_000);
    assert_eq!(mm("1\""), 25_400_000);
    assert_eq!(mm("-1.5mm"), -1_500_000);
    assert_eq!(mm("+.5"), 500_000);
    assert_eq!(mm("1e-3"), 1_000);
    assert_eq!(mm("2.5E2um"), 250_000);
    assert_eq!(mm("1e-7"), 0);
    assert_eq!(mm("5e-7"), 1);
    assert_eq!(mm("-5e-7"), -1);
    assert_eq!(mm("0.0000014"), 1);
    assert_eq!(mm("2000"), MAX_COORD_NM);
    // A bare number takes the field's default unit.
    assert_eq!(parse_length("10", LengthUnit::Mil).unwrap(), nm(254_000));
    assert_eq!(
        parse_length("10mm", LengthUnit::Mil).unwrap(),
        nm(10_000_000)
    );
    assert_eq!(parse_length("3", LengthUnit::Um).unwrap(), nm(3_000));
}

#[test]
fn the_parser_refuses_malformed_and_out_of_range_text() {
    for text in [
        "", "mm", "abc", "1 2", "1 ft", "1m", "1nm", "1e", "1..2", "--1", "1mm mm", "0x10", "1,5",
        "NaN", "inf",
    ] {
        assert!(parse_length(text, LengthUnit::Mm).is_err(), "{text:?}");
    }
    assert!(parse_length("2000.000001", LengthUnit::Mm).is_err());
    assert!(parse_length("79in", LengthUnit::Mm).is_err());
    assert!(parse_length("1e400", LengthUnit::Mm).is_err());
    assert!(parse_length("1e30", LengthUnit::Mm).is_err());
    assert!(parse_length(&"1".repeat(65), LengthUnit::Mm).is_err());
}

#[test]
fn mixed_unit_text_agrees_exactly() {
    // 1 in = 25.4 mm = 25,400 µm = 1000 mil, with no float in between.
    let values = ["1in", "25.4mm", "25400um", "1000mil", "0.0254e3mm"];
    for v in values {
        assert_eq!(mm(v), NM_PER_INCH, "{v}");
    }
    // Sums of mixed-unit lengths stay exact.
    let sum = parse_length("10mil", LengthUnit::Mm)
        .unwrap()
        .checked_add(parse_length("0.1mm", LengthUnit::Mm).unwrap())
        .unwrap();
    assert_eq!(sum, nm(354_000));
    assert_eq!(format_length(sum, LengthUnit::Mm), "0.354 mm");
    assert_eq!(format_length(sum, LengthUnit::Mil), "13.93701 mil");
}

#[test]
fn formatting_is_exact_and_reads_back_in_every_unit() {
    assert_eq!(format_length(nm(250_000), LengthUnit::Mm), "0.25 mm");
    assert_eq!(format_length(nm(-1), LengthUnit::Mm), "-0.000001 mm");
    assert_eq!(format_length(nm(0), LengthUnit::Mm), "0 mm");
    assert_eq!(format_length(nm(1_500), LengthUnit::Um), "1.5 µm");
    assert_eq!(format_length(nm(254_000), LengthUnit::Mil), "10 mil");
    assert_eq!(format_length(nm(1_270_000), LengthUnit::Inch), "0.05 in");
    assert_eq!(format_length(nm(-MAX_COORD_NM), LengthUnit::Mm), "-2000 mm");
    let mut v: i64 = 1;
    while v <= MAX_COORD_NM {
        for n in [v, -v, v + 1, MAX_COORD_NM - v] {
            for unit in LengthUnit::ALL {
                let text = format_length(nm(n), unit);
                assert_eq!(
                    parse_length(&text, LengthUnit::Mm).unwrap(),
                    nm(n),
                    "{text}"
                );
            }
        }
        v = v * 7 + 3;
    }
}

#[test]
fn products_go_through_i128_without_overflow() {
    let far = p(MAX_COORD_NM, MAX_COORD_NM);
    let near = p(-MAX_COORD_NM, -MAX_COORD_NM);
    assert_eq!(dist2(near, far), 2 * (4 * i128::from(MAX_COORD_NM).pow(2)));
    assert_eq!(dot(p(0, 0), p(3, 4), p(3, 4)), 25);
    assert_eq!(cross(p(0, 0), p(1, 0), p(0, 1)), 1);
    assert_eq!(
        orient(p(0, 0), p(1, 0), p(0, 1)),
        Orientation::CounterClockwise
    );
    assert_eq!(orient(p(0, 0), p(0, 1), p(1, 0)), Orientation::Clockwise);
    assert_eq!(orient(near, p(0, 0), far), Orientation::Collinear);
    assert_eq!(
        orient(near, far, p(MAX_COORD_NM - 1, MAX_COORD_NM)),
        Orientation::CounterClockwise
    );
}

#[test]
fn angles_are_millidegrees_within_one_turn() {
    assert!(Angle::new(0).is_ok());
    assert!(Angle::new(359_999).is_ok());
    assert!(Angle::new(360_000).is_err());
    assert!(Angle::new(-1).is_err());
    assert_eq!(Angle::normalized(-90_000).millidegrees(), 270_000);
    assert_eq!(Angle::normalized(720_001).millidegrees(), 1);
    assert_eq!(
        Angle::from_degrees_f64_rounded(45.0005)
            .unwrap()
            .millidegrees(),
        45_001
    );
    assert_eq!(
        Angle::from_degrees_f64_rounded(-0.0004)
            .unwrap()
            .millidegrees(),
        0
    );
    assert_eq!(
        Angle::from_degrees_f64_rounded(-90.0)
            .unwrap()
            .millidegrees(),
        270_000
    );
    assert!(Angle::from_degrees_f64_rounded(f64::NAN).is_err());
    let a = Angle::new(350_000).unwrap();
    assert_eq!(
        a.rotated_by(Angle::new(20_000).unwrap()).millidegrees(),
        10_000
    );
    assert_eq!(Angle::new(12_500).unwrap().to_degrees_f64(), 12.5);
}

#[test]
fn arcs_check_their_radii_exactly() {
    let arc = |end: PointNm, centre: PointNm| ArcNm {
        end,
        centre,
        direction: ArcDirection::CounterClockwise,
    };
    let start = p(1_000_000, 0);
    assert!(arc(p(0, 1_000_000), p(0, 0)).check(start).is_ok());
    // Full turn.
    assert!(arc(start, p(0, 0)).check(start).is_ok());
    // Radii 1,000,000 and 1,000,002: within 2 nm; 1,000,003 is not.
    assert!(arc(p(0, 1_000_002), p(0, 0)).check(start).is_ok());
    assert!(arc(p(0, 1_000_003), p(0, 0)).check(start).is_err());
    assert!(arc(p(0, 999_998), p(0, 0)).check(start).is_ok());
    assert!(arc(p(0, 999_997), p(0, 0)).check(start).is_err());
    // Rounded ends of a 45° arc: r = 707,107·√2 ≈ 1,000,000.4.
    assert!(arc(p(707_107, 707_107), p(0, 0)).check(start).is_ok());
    // Zero radius.
    assert!(arc(p(0, 0), p(0, 0)).check(start).is_err());
    // Extreme coordinates neither overflow nor pass.
    assert!(
        arc(
            p(MAX_COORD_NM, MAX_COORD_NM),
            p(-MAX_COORD_NM, -MAX_COORD_NM)
        )
        .check(p(-MAX_COORD_NM, -MAX_COORD_NM + 1))
        .is_err()
    );
    let far = p(-MAX_COORD_NM, -MAX_COORD_NM);
    assert!(
        arc(p(MAX_COORD_NM, -MAX_COORD_NM), far)
            .check(p(-MAX_COORD_NM, MAX_COORD_NM))
            .is_ok()
    );
}

#[test]
fn stored_forms_are_integers_and_bounded_on_read() {
    let point = p(250_000, -1);
    let json = serde_json::to_string(&point).unwrap();
    assert_eq!(json, r#"{"x_nm":250000,"y_nm":-1}"#);
    assert_eq!(serde_json::from_str::<PointNm>(&json).unwrap(), point);
    assert!(serde_json::from_str::<PointNm>(r#"{"x_nm":2000000001,"y_nm":0}"#).is_err());
    assert!(serde_json::from_str::<PointNm>(r#"{"x_nm":0.5,"y_nm":0}"#).is_err());
    assert!(serde_json::from_str::<PointNm>(r#"{"x_nm":0,"y_nm":0,"z_nm":0}"#).is_err());
    assert_eq!(
        serde_json::to_string(&Angle::new(90_000).unwrap()).unwrap(),
        "90000"
    );
    assert!(serde_json::from_str::<Angle>("360000").is_err());
    let arc = ArcNm {
        end: p(1, 2),
        centre: p(0, 0),
        direction: ArcDirection::Clockwise,
    };
    let json = serde_json::to_string(&arc).unwrap();
    assert_eq!(
        json,
        r#"{"end":{"x_nm":1,"y_nm":2},"centre":{"x_nm":0,"y_nm":0},"direction":"clockwise"}"#
    );
    assert_eq!(serde_json::from_str::<ArcNm>(&json).unwrap(), arc);
}

#[test]
fn the_wire_length_takes_mm_numbers_or_unit_text_and_replies_in_mm() {
    let read = |json: &str| serde_json::from_str::<Length>(json).map(|l| l.0.get());
    assert_eq!(read("0.25").unwrap(), 250_000);
    assert_eq!(read("2").unwrap(), 2_000_000);
    assert_eq!(read("-2").unwrap(), -2_000_000);
    assert_eq!(read(r#""0.25mm""#).unwrap(), 250_000);
    assert_eq!(read(r#""10mil""#).unwrap(), 254_000);
    assert_eq!(read(r#""0.25""#).unwrap(), 250_000);
    assert_eq!(read("0.1234565").unwrap(), 123_457);
    assert!(read("2001").is_err());
    assert!(read("18446744073709551615").is_err());
    assert!(read(r#""10 ft""#).is_err());
    assert!(read("null").is_err());
    assert!(read("[1]").is_err());
    for (n, text) in [
        (250_000, "0.25"),
        (254_000, "0.254"),
        (1, "1e-6"),
        (-123_456_789, "-123.456789"),
        (MAX_COORD_NM, "2000.0"),
        (0, "0.0"),
    ] {
        assert_eq!(serde_json::to_string(&Length(nm(n))).unwrap(), text);
    }
}

#[test]
fn the_wire_schema_offers_a_number_or_unit_text() {
    let schema = serde_json::to_value(schemars::schema_for!(Length)).unwrap();
    let options = schema["oneOf"].as_array().unwrap();
    assert_eq!(options[0]["type"], "number");
    assert_eq!(options[1]["type"], "string");
    assert!(schema["description"].as_str().unwrap().contains("mil"));
    let nm_schema = serde_json::to_value(schemars::schema_for!(Nm)).unwrap();
    assert_eq!(nm_schema["type"], "integer");
    assert_eq!(nm_schema["maximum"], MAX_COORD_NM);
}

mod params {
    use super::*;
    use serde::{Deserialize, Serialize};

    fn three() -> Nm {
        Nm::new(3_000_000).unwrap()
    }

    /// A stored record with every kind of field the derive maps.
    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, Params)]
    #[serde(deny_unknown_fields)]
    struct Pad {
        name: String,
        width_nm: Nm,
        at: PointNm,
        rotation_mdeg: Angle,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drill_nm: Option<Nm>,
        #[serde(default = "three")]
        gap_nm: Nm,
        outline: Vec<PointNm>,
        corners_nm: [Nm; 2],
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Params)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Row {
        One {
            #[params]
            pad: Box<Pad>,
        },
        Many {
            #[params]
            pads: Vec<Pad>,
            pitch_nm: Nm,
        },
        Arc(ArcNm),
        Empty,
    }

    #[test]
    fn stored_records_carry_millimetres_on_the_wire() {
        let pad = Pad {
            name: "1".into(),
            width_nm: nm(250_000),
            at: p(1_270_000, -2_540_000),
            rotation_mdeg: Angle::new(90_000).unwrap(),
            drill_nm: Some(nm(300_000)),
            gap_nm: nm(1),
            outline: vec![p(1, 2)],
            corners_nm: [nm(1_000), nm(2_000)],
        };
        let stored = serde_json::to_value(&pad).unwrap();
        assert_eq!(stored["width_nm"], 250_000);
        assert_eq!(
            stored["at"],
            serde_json::json!({"x_nm": 1_270_000, "y_nm": -2_540_000})
        );
        let wire = serde_json::to_value(PadParams::from(pad.clone())).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "name": "1", "width": 0.25, "at": [1.27, -2.54], "rotation": 90.0,
                "drill": 0.3, "gap": 0.000001, "outline": [[0.000001, 0.000002]],
                "corners": [0.001, 0.002]
            })
        );
        let back: PadParams = serde_json::from_value(wire).unwrap();
        assert_eq!(Pad::from(back), pad);

        // Text with units, defaults through the stored default function.
        let typed: PadParams = serde_json::from_value(serde_json::json!({
            "name": "2", "width": "10mil", "at": [0, "1in"], "rotation": -90,
            "outline": [], "corners": [0, 0]
        }))
        .unwrap();
        let typed = Pad::from(typed);
        assert_eq!(typed.width_nm, nm(254_000));
        assert_eq!(typed.at, p(0, 25_400_000));
        assert_eq!(typed.rotation_mdeg.millidegrees(), 270_000);
        assert_eq!(typed.gap_nm, three());
        assert_eq!(typed.drill_nm, None);
        assert!(serde_json::from_value::<PadParams>(serde_json::json!({"width_nm": 1})).is_err());
    }

    #[test]
    fn nested_records_and_enums_convert_both_ways() {
        let rows = vec![
            Row::One {
                pad: Box::default(),
            },
            Row::Many {
                pads: vec![Pad::default()],
                pitch_nm: nm(500_000),
            },
            Row::Arc(ArcNm {
                end: p(1_000_000, 0),
                centre: p(0, 0),
                direction: ArcDirection::Clockwise,
            }),
            Row::Empty,
        ];
        for row in rows {
            let wire = RowParams::from(row.clone());
            let text = serde_json::to_value(&wire).unwrap();
            let back: RowParams = serde_json::from_value(text).unwrap();
            assert_eq!(Row::from(back), row);
        }
        let many = serde_json::to_value(RowParams::from(Row::Many {
            pads: vec![],
            pitch_nm: nm(500_000),
        }))
        .unwrap();
        assert_eq!(
            many,
            serde_json::json!({"kind": "many", "pads": [], "pitch": 0.5})
        );
        let schema = serde_json::to_string(&schemars::schema_for!(RowParams)).unwrap();
        assert!(schema.contains("\"pitch\"") && !schema.contains("pitch_nm"));
    }
}
