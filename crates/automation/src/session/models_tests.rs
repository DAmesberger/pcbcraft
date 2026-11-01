//! Slice C7: the 3D model library (D1) over JSON. Staged as
//! `mcp::scenario` explains: the D1 stub's explicit refusal (nothing
//! changed) ends the step until D1 is merged and `REQUIRE_COMPLETE` is set.
use super::pinout_tests::{Fallible, Scenario, finish};
use super::*;

#[test]
fn the_model_query_answers_a_bounded_report() -> Fallible<()> {
    finish((|| -> Fallible<()> {
        let mut s = Scenario::demo()?;
        let report = s.query(
            "models",
            "library.models",
            json!({"query": "kind:model pins:8", "limit": 4}),
        )?;
        let hits = report["hits"].as_array().ok_or("hits")?;
        assert!(hits.len() <= 4, "the limit holds: {report}");
        assert!(report["more"].is_u64(), "{report}");
        Ok(())
    })())
}
