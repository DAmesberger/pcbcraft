//! Slice C7: the footprint step of the EXM-48 scenario over JSON. After
//! importing the pinout, the agent reads the package drawing and looks for
//! a package row with `library.templates` (B1). Staged as `pinout_tests`
//! explains: the B1 stub's explicit refusal (nothing changed) ends the step
//! until slice I sets `REQUIRE_COMPLETE`.
use super::pinout_tests::{Fallible, Scenario, finish};
use super::*;

#[test]
fn the_exm48_package_is_found_among_the_no_lead_templates() -> Fallible<()> {
    finish((|| -> Fallible<()> {
        let mut s = Scenario::demo()?;
        let report = s.query(
            "templates",
            "library.templates",
            json!({"query": "qfn pins:48", "limit": 8}),
        )?;
        let hits = report["hits"].as_array().ok_or("hits")?;
        assert!(!hits.is_empty(), "{report}");
        assert!(hits.len() <= 8, "the limit holds: {report}");
        assert!(report["more"].is_u64(), "{report}");
        assert!(
            hits.iter().all(|h| h["family"] == "no_lead"),
            "a QFN is a no-lead package: {report}"
        );
        Ok(())
    })())
}
