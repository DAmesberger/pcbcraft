//! Built-in rule profiles. Every number here is a generic, original
//! estimate in the spirit of common "6/6", "4/4" and "3/3" mil track/space
//! classes, not any fabricator's published capability; each profile says
//! so in its provenance. Fabricator numbers enter only as user profiles.
use super::{
    ClearanceMatrix, DesignRules, ProfileBasis, ProfileOrigin, ProfileProvenance, RuleProfile,
};

/// The id of the profile a new project uses.
pub const DEFAULT_PROFILE_ID: &str = "pcbcraft-default";
/// One mil in mm.
const MIL: f64 = 0.0254;

fn estimated(id: &str, name: &str, notes: &str, rules: DesignRules) -> RuleProfile {
    RuleProfile {
        id: id.into(),
        name: name.into(),
        provenance: ProfileProvenance {
            origin: ProfileOrigin::BuiltIn,
            basis: ProfileBasis::Estimated,
            source: String::new(),
            url: String::new(),
            date: String::new(),
            notes: notes.into(),
        },
        rules,
    }
}

/// Pcbcraft's prototype defaults: exactly the values the checks used
/// before rules were configurable ([`DesignRules::default`]).
pub fn default_profile() -> RuleProfile {
    estimated(
        DEFAULT_PROFILE_ID,
        "Pcbcraft default",
        "Estimated prototype defaults; not a fabricator qualification.",
        DesignRules::default(),
    )
}

/// A profile of `track` mil track and space with the hole sizes given.
fn class(track_mil: f64, drill: f64, ring: f64, hole_gap: f64) -> DesignRules {
    let mut r = DesignRules::default();
    let track = track_mil * MIL;
    r.copper.track_width_min = track;
    r.copper.track_width_default = (2.0 * track).max(0.2);
    r.copper.clearance = ClearanceMatrix::uniform(track, hole_gap, hole_gap, 0.25);
    r.holes.drill_min = drill;
    r.holes.via_annular_ring_min = ring;
    r.holes.pth_annular_ring_min = ring;
    r.holes.aspect_ratio_max = Some(10.0);
    r.holes.via_drill_default = drill;
    r.holes.via_diameter_default = drill + 2.0 * ring;
    r.fill.clearance = (2.0 * track).max(0.2);
    r
}

pub fn standard() -> RuleProfile {
    estimated(
        "std-6-6",
        "Standard 6/6",
        "Estimated generic 6 mil track/space class; enter your fabricator's numbers as a user profile.",
        class(6.0, 0.3, 0.15, 0.25),
    )
}

pub fn fine() -> RuleProfile {
    estimated(
        "fine-4-4",
        "Fine 4/4",
        "Estimated generic 4 mil track/space class.",
        {
            let mut r = class(4.0, 0.2, 0.125, 0.2);
            r.mask.web_min = 0.1;
            r
        },
    )
}

pub fn hdi() -> RuleProfile {
    estimated(
        "hdi-3-3",
        "HDI 3/3",
        "Estimated generic 3 mil track/space class with laser-drilled vias.",
        {
            let mut r = class(3.0, 0.1, 0.1, 0.15);
            r.mask.web_min = 0.075;
            r.silkscreen.line_width_min = 0.075;
            r
        },
    )
}

/// Every built-in profile, the default first.
pub fn all() -> Vec<RuleProfile> {
    vec![default_profile(), standard(), fine(), hdi()]
}
