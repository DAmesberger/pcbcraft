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

/// Pcbcraft's prototype defaults ([`DesignRules::default`]): the values the
/// checks used before rules were configurable, with copper now kept 0.3 mm
/// from the board edge, the same as the coarsest class.
pub fn default_profile() -> RuleProfile {
    estimated(
        DEFAULT_PROFILE_ID,
        "Pcbcraft default",
        "Estimated prototype defaults; not a fabricator qualification.",
        DesignRules::default(),
    )
}

/// The numbers that tell one built-in class from another; everything
/// else stays at [`DesignRules::default`].
struct Class {
    /// Track and space, mil.
    track_mil: f64,
    /// What the Route tool starts with, mm.
    track_default: f64,
    /// Least drill, mm.
    drill: f64,
    /// Least annular ring (vias and plated holes), mm.
    ring: f64,
    /// The default via's drill, mm (its pad is drill plus two rings).
    via_drill: f64,
    /// Hole wall to copper and hole to hole, mm.
    hole_gap: f64,
    /// Copper to board edge and cutouts, mm.
    edge: f64,
    /// Board thickness over drill, at most.
    aspect: f64,
    /// Extra fill-to-other-net gap, mm.
    fill_clearance: f64,
    mask_expansion: f64,
    mask_web: f64,
    silk_width: f64,
    silk_pad_clearance: f64,
}

/// A rule set from one class's numbers.
fn class(c: &Class) -> DesignRules {
    let mut r = DesignRules::default();
    let track = c.track_mil * MIL;
    r.copper.track_width_min = track;
    r.copper.track_width_default = c.track_default;
    r.copper.clearance = ClearanceMatrix::uniform(track, c.hole_gap, c.hole_gap, c.edge);
    r.holes.drill_min = c.drill;
    r.holes.via_annular_ring_min = c.ring;
    r.holes.pth_annular_ring_min = c.ring;
    r.holes.aspect_ratio_max = Some(c.aspect);
    r.holes.via_drill_default = c.via_drill;
    r.holes.via_diameter_default = c.via_drill + 2.0 * c.ring;
    r.mask.expansion = c.mask_expansion;
    r.mask.web_min = c.mask_web;
    r.silkscreen.line_width_min = c.silk_width;
    r.silkscreen.pad_clearance = c.silk_pad_clearance;
    r.fill.clearance = c.fill_clearance;
    r
}

pub fn standard() -> RuleProfile {
    estimated(
        "std-6-6",
        "Standard 6/6",
        "Estimated generic 6 mil track/space class for 1.6 mm boards; enter your fabricator's numbers as a user profile.",
        class(&Class {
            track_mil: 6.0,
            track_default: 0.25,
            drill: 0.3,
            ring: 0.15,
            via_drill: 0.3,
            hole_gap: 0.25,
            edge: 0.3,
            aspect: 10.0,
            fill_clearance: 0.3,
            mask_expansion: 0.05,
            mask_web: 0.1,
            silk_width: 0.15,
            silk_pad_clearance: 0.15,
        }),
    )
}

pub fn fine() -> RuleProfile {
    estimated(
        "fine-4-4",
        "Fine 4/4",
        "Estimated generic 4 mil track/space class; enter your fabricator's numbers as a user profile.",
        class(&Class {
            track_mil: 4.0,
            track_default: 0.2,
            drill: 0.2,
            ring: 0.125,
            via_drill: 0.2,
            hole_gap: 0.2,
            edge: 0.25,
            aspect: 10.0,
            fill_clearance: 0.2,
            mask_expansion: 0.05,
            mask_web: 0.1,
            silk_width: 0.1,
            silk_pad_clearance: 0.1,
        }),
    )
}

pub fn hdi() -> RuleProfile {
    estimated(
        "hdi-3-3",
        "HDI 3/3",
        "Estimated generic 3 mil track/space class with small mechanically drilled through vias; enter your fabricator's numbers as a user profile.",
        class(&Class {
            track_mil: 3.0,
            track_default: 0.15,
            drill: 0.1,
            ring: 0.1,
            via_drill: 0.15,
            hole_gap: 0.15,
            edge: 0.25,
            aspect: 12.0,
            fill_clearance: 0.15,
            mask_expansion: 0.035,
            mask_web: 0.075,
            silk_width: 0.075,
            silk_pad_clearance: 0.075,
        }),
    )
}

/// Every built-in profile, the default first.
pub fn all() -> Vec<RuleProfile> {
    vec![default_profile(), standard(), fine(), hdi()]
}

/// The built-in profile with this id.
pub fn by_id(id: &str) -> Option<RuleProfile> {
    all().into_iter().find(|p| p.id == id)
}
