//! Test profiles, built only for tests (`test-fixtures`). Each is a
//! profile file whose source says it is a fixture: none is shipped, offered
//! or a fabricator's capability.
use super::{ProfileFile, ProjectRules, RuleProfile};
use crate::diagnostics::{Error, Result};

/// The values Pcbcraft's checks used before profiles were files (0.2 mm
/// copper clearance, 0.3/0.6 mm vias, tented), kept so the tests that
/// measure against them still do. Not a fabricator's numbers.
pub const PROTOTYPE_JSON: &str = include_str!("fixtures/prototype.json");

/// A second, finer test profile for switching tests.
pub const FINE_JSON: &str = include_str!("fixtures/fine.json");

/// The profile of a profile file's text.
pub fn parse(json: &str) -> Result<RuleProfile> {
    serde_json::from_str::<ProfileFile>(json)
        .map(|file| file.profile)
        .map_err(|e| Error::Invalid(format!("a fixture profile does not parse: {e}")))
}

/// [`PROTOTYPE_JSON`]'s profile.
pub fn prototype() -> Result<RuleProfile> {
    parse(PROTOTYPE_JSON)
}

/// [`FINE_JSON`]'s profile.
pub fn fine() -> Result<RuleProfile> {
    parse(FINE_JSON)
}

/// The sourced JLCPCB profile shipped as a file in `examples/profiles/`.
pub const JLCPCB_EXAMPLE_JSON: &str =
    include_str!("../../../../examples/profiles/jlcpcb-2-layer-1oz.json");

/// [`JLCPCB_EXAMPLE_JSON`]'s profile.
pub fn jlcpcb_example() -> Result<RuleProfile> {
    parse(JLCPCB_EXAMPLE_JSON)
}

/// Project rules on [`prototype`].
pub fn rules() -> Result<ProjectRules> {
    Ok(ProjectRules {
        profile: Some(prototype()?),
        ..ProjectRules::default()
    })
}
