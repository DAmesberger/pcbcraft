//! The tuned `qwen3.8-flash-next` profile's runs (a user profile, in
//! `examples/qwen3.8-flash-next/assist-profiles/`) on Qwen3.8 Flash Next
//! (`examples/qwen3.8-flash-next`), replayed offline and graded against TI's
//! land patterns: the pinout, the tabbed package form, every pad's size, the
//! rows' land span and pitch, and the report accepted on the first try. The
//! datasheet is a stand-in, so the recorded page views now answer with
//! errors; nothing the runs built depended on them.
use super::*;
use crate::model::record::Replay;
use std::path::PathBuf;

struct Reference {
    recording: &'static str,
    requests: u32,
    pins: &'static [(&'static str, &'static str)],
    /// Land `[across the lead, along y]`, rows' land centre to land centre.
    land: [f64; 2],
    span: f64,
}

fn replay(reference: &Reference) -> Fallible {
    let dir = tempfile::tempdir()?;
    let example =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/qwen3.8-flash-next");
    let path = example.join(reference.recording);
    let mut backend = Replay::from_file(&path)?;
    let mut config = local_config(262_144);
    config.run_profile = crate::profile::select(
        None,
        "RadixArk/Qwen3.8-Flash-Next-NVFP4",
        Some(&example.join("assist-profiles")),
    )?;
    assert_eq!(config.run_profile.name, "qwen3.8-flash-next");
    let mut session = Session::default();
    let (outcome, log) = run_with(&config, &mut backend, text_datasheet(&dir)?, &mut session)?;
    assert_eq!(outcome.stop, Stop::Submitted, "{log:?}");
    let f = &outcome.facts;
    assert_eq!(f.requests, reference.requests);
    // The report was accepted the first time it was sent.
    let reports: Vec<&String> = f
        .request_log
        .iter()
        .flat_map(|r| &r.tools)
        .filter(|t| t.starts_with("submit_review"))
        .collect();
    assert_eq!(reports, vec!["submit_review"]);
    let pins: Vec<(&str, &str)> = f
        .pins
        .iter()
        .map(|p| (p.number.as_str(), p.name.as_str()))
        .collect();
    assert_eq!(pins, reference.pins);

    let drafts = &session.engine().drafts().items;
    let family = drafts.iter().find_map(|d| match &d.definition {
        sparksmith_core::library::Definition::Package(p) => serde_json::to_value(p)
            .ok()
            .map(|v| v["form"]["family"].clone()),
        _ => None,
    });
    assert_eq!(family, Some(json!("tabbed")));
    let footprint = drafts
        .iter()
        .find_map(|d| match &d.definition {
            sparksmith_core::library::Definition::Footprint(f) => Some(f),
            _ => None,
        })
        .ok_or("a footprint draft")?;
    let pads = &footprint.pads;
    assert_eq!(pads.len(), reference.pins.len());
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let mut ys = Vec::new();
    let mut xs = Vec::new();
    for pad in pads {
        let sparksmith_core::library::PadShape::Rect { size } = &pad.shape else {
            return Err(format!("a rectangular land: {pad:?}").into());
        };
        let [w, h] = size.to_mm_f64();
        assert!(
            near(w, reference.land[0]) && near(h, reference.land[1]),
            "{pad:?}"
        );
        let [x, y] = pad.position();
        xs.push(x);
        ys.push(y);
    }
    let spread = |v: &[f64]| {
        v.iter().copied().fold(f64::MIN, f64::max) - v.iter().copied().fold(f64::MAX, f64::min)
    };
    assert!(near(spread(&ys), reference.span), "{ys:?}");
    // Neighbours in a row 0.95 mm apart.
    let mut row: Vec<f64> = xs
        .iter()
        .zip(&ys)
        .filter(|(_, y)| near(**y, ys[0]))
        .map(|(x, _)| *x)
        .collect();
    row.sort_by(f64::total_cmp);
    assert!(
        row.windows(2)
            .all(|w| near(w[1] - w[0], 0.95) || near(w[1] - w[0], 1.9))
    );
    Ok(())
}

#[test]
fn flash_next_builds_the_ldo_on_ti_s_land_pattern() -> Fallible {
    // TLV757P (SBVS322C) Table 4-1; DBV0005A land: 5× 0.6 × 1.1 mm, rows 2.6 apart.
    replay(&Reference {
        recording: "tlv757.recording.jsonl",
        requests: REQUESTS_TLV,
        pins: &[
            ("1", "IN"),
            ("2", "GND"),
            ("3", "EN"),
            ("4", "NC"),
            ("5", "OUT"),
        ],
        land: [0.6, 1.1],
        span: 2.6,
    })
}

#[test]
fn flash_next_builds_the_esd_diode_on_ti_s_land_pattern() -> Fallible {
    // TPD2E009 (SLVS953B) Pin Functions; DBZ0003A land: 3× 0.6 × 1.3 mm, rows 2.1 apart.
    replay(&Reference {
        recording: "tpd2e009.recording.jsonl",
        requests: REQUESTS_TPD,
        pins: &[("1", "D+"), ("2", "D-"), ("3", "GND")],
        land: [0.6, 1.3],
        span: 2.1,
    })
}

/// Requests of the recorded runs (Opus 5.5 took 13 on the TLV75733).
const REQUESTS_TLV: u32 = 16;
const REQUESTS_TPD: u32 = 11;
