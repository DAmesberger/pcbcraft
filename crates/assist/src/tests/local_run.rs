//! The first live run on a local model (`examples/tlv75733-local`: 29
//! replies of `qwen3.8:27b` through Ollama's OpenAI-compatible endpoint),
//! replayed offline: the same drafts come out of the same tools, and the
//! review counts what the recording says. The datasheet is a stand-in with
//! text and two pages, so the recorded views of pages 34 and 35 now answer
//! with errors; nothing the run built depended on them.
use super::*;
use crate::model::record::Replay;
use std::path::PathBuf;

#[test]
fn the_local_run_replays_to_the_same_ldo() -> Fallible {
    let dir = tempfile::tempdir()?;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/tlv75733-local/tlv757.recording.jsonl");
    let mut backend = Replay::from_file(&path)?;
    let config = local_config(262_144)?;
    let mut session = Session::default();
    let (outcome, log) = run_with(&config, &mut backend, text_datasheet(&dir)?, &mut session)?;
    assert_eq!(outcome.stop, Stop::Submitted, "{log:?}");
    let f = &outcome.facts;
    assert_eq!(f.requests, 29);
    assert_eq!(f.output_tokens, 38_861);
    // Pin 1 IN, 2 GND, 3 EN, 4 NC, 5 OUT (TLV757P datasheet Table 4-1).
    let pins: Vec<(String, String)> = f
        .pins
        .iter()
        .map(|p| (p.number.clone(), p.name.clone()))
        .collect();
    let expected: Vec<(String, String)> = [
        ("1", "IN"),
        ("2", "GND"),
        ("3", "EN"),
        ("4", "NC"),
        ("5", "OUT"),
    ]
    .iter()
    .map(|(n, s)| ((*n).to_owned(), (*s).to_owned()))
    .collect();
    assert_eq!(pins, expected);
    // The applied pinout came after one refusal (request 6: the base).
    assert_eq!(f.request_log[5].tools, vec!["library_pinout_apply!"]);
    assert_eq!(f.request_log[6].tools, vec!["library_pinout_apply"]);
    let footprint = session
        .engine()
        .drafts()
        .items
        .iter()
        .find_map(|d| match &d.definition {
            sparksmith_core::library::Definition::Footprint(f) => Some(f),
            _ => None,
        })
        .ok_or("a footprint draft")?;
    assert_eq!(footprint.pads.len(), 5);
    // It kept the starter row's pads, not the datasheet's 1.1 × 0.6 mm land
    // (page 35), and said so in an open question: a known shortfall.
    let report = outcome.report.ok_or("a report")?;
    assert!(
        report
            .open_questions
            .iter()
            .any(|q| q.contains("1.1 x 0.6")),
        "{:?}",
        report.open_questions
    );
    Ok(())
}
