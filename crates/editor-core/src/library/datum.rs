//! Datum vocabulary shared by package datums (slice B2) and the board
//! interface (slices B1 and B3): what geometry a datum has and how a datum
//! is named inside its owner. Data and syntax only; the interface's rules
//! live in `crate::interface` (docs/architecture/board-interface.md).
//!
//! Package datums (slice B2) are stored here: explicit geometry in a
//! package's model frame (z = 0 the seating plane), lengths in whole
//! nanometres. `crate::interface::package` places them on the board.
use crate::units::{self, Nm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Longest datum name, in bytes.
pub const MAX_DATUM_NAME: usize = 32;

/// Names a package may not give a datum: the interface already uses them
/// for every placement (`i80.frame`, `i80.envelope`), so a package datum of
/// that name would make two datums with one token.
pub const RESERVED_DATUM_NAMES: [&str; 2] = ["frame", "envelope"];

/// The part-file name of a construction plane (or a sketch, for a free
/// profile) that declares an opening: `opening` or `opening-<suffix>`. It
/// becomes the package datum `opening` or `opening_<suffix>` (slice B2).
pub const PART_FILE_OPENING: &str = "opening";
/// The part-file name of a construction axis that declares an axis datum:
/// `axis` or `axis-<suffix>`, giving `axis` or `axis_<suffix>` (slice B2).
pub const PART_FILE_AXIS: &str = "axis";

/// The geometry a datum carries (docs/architecture/board-interface.md,
/// "Datum kinds").
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum DatumKind {
    /// An origin and two axes: the board origin, a part's placement frame.
    Frame,
    /// An origin and a normal: board top and bottom, a mating plane.
    Plane,
    /// A point and a direction, with an optional diameter: a mounting
    /// hole, an LED's light axis.
    Axis,
    /// Closed loops of lines and arcs on a plane: the board outline, a
    /// cutout, a connector opening's outline.
    Profile,
    /// A profile and a height along its normal: a keepout, a part's
    /// envelope, the space a plug needs.
    Volume,
    /// A name, a value and a unit: board thickness, a board parameter.
    Parameter,
}

/// A datum's name inside its owner (`opening`, `axis_led`, `top`): lower
/// case `[a-z][a-z0-9_]*`, at most [`MAX_DATUM_NAME`] bytes. It is part of
/// the datum's key and token, so it is identity, unlike the display name.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct DatumName(pub String);

impl DatumName {
    /// The name's syntax: `[a-z][a-z0-9_]*`, at most [`MAX_DATUM_NAME`]
    /// bytes.
    pub fn valid(&self) -> bool {
        let mut chars = self.0.chars();
        self.0.len() <= MAX_DATUM_NAME
            && chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    /// Whether a package may declare a datum of this name: valid and not
    /// one of [`RESERVED_DATUM_NAMES`].
    pub fn valid_for_package(&self) -> bool {
        self.valid() && !RESERVED_DATUM_NAMES.contains(&self.0.as_str())
    }

    /// The datum name a part-file construction feature declares:
    /// `opening` → `opening`, `opening-usb` → `opening_usb` for `prefix`
    /// [`PART_FILE_OPENING`]; `None` for any other name, or a suffix that
    /// does not make a valid name.
    pub fn from_part_file(feature: &str, prefix: &str) -> Option<Self> {
        let rest = feature.strip_prefix(prefix)?;
        let name = match rest.strip_prefix('-') {
            None if rest.is_empty() => prefix.to_owned(),
            Some(suffix) if !suffix.is_empty() => format!("{prefix}_{}", suffix.replace('-', "_")),
            _ => return None,
        };
        let name = Self(name);
        name.valid_for_package().then_some(name)
    }
}

/// Most datums one package declares.
pub const MAX_PACKAGE_DATUMS: usize = 32;
/// A direction's length lies this close to 1, and a frame's two axes are
/// this close to perpendicular.
pub const DIRECTION_TOLERANCE: f64 = 1e-6;

/// Where a package datum came from. A datum the user authored or edited is
/// never replaced by a part file or form; the others follow their source
/// when it changes (`PackageDefinition::sync_datums`).
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum PackageDatumOrigin {
    /// Written by a person or an agent (`library.edit_package`).
    #[default]
    Authored,
    /// Declared in the package's part file (`opening-*` planes, `axis-*`
    /// axes) and copied when the part file is used.
    PartFile,
    /// Emitted by the package's typed form from its datasheet values (a
    /// USB-C receptacle's opening).
    Form,
}

/// A frame in the model: an origin and two perpendicular unit axes; the
/// third is `x × y`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct DatumFrame {
    pub origin_nm: [Nm; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
}

impl DatumFrame {
    /// The origin, mm.
    pub fn origin(&self) -> [f64; 3] {
        self.origin_nm.map(Nm::to_mm_f64)
    }

    /// `x × y`.
    pub fn z(&self) -> [f64; 3] {
        cross(self.x, self.y)
    }

    fn valid(&self) -> bool {
        unit(self.x) && unit(self.y) && dot(self.x, self.y).abs() <= DIRECTION_TOLERANCE
    }
}

/// A closed profile in a frame's x–y plane, centred on its origin.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(rename_all = "snake_case", tag = "shape", deny_unknown_fields)]
pub enum DatumProfile {
    /// A rectangle `size` along x and y, its corners rounded by
    /// `corner_radius`; half the smaller side makes a stadium, as a USB-C
    /// opening.
    Rect {
        size_nm: [Nm; 2],
        #[serde(default)]
        corner_radius_nm: Nm,
    },
}

impl DatumProfile {
    /// The size along x and y and the corner radius, mm.
    pub fn rect_mm(&self) -> ([f64; 2], f64) {
        match self {
            Self::Rect {
                size_nm,
                corner_radius_nm,
            } => (size_nm.map(Nm::to_mm_f64), corner_radius_nm.to_mm_f64()),
        }
    }

    fn problem(&self) -> Option<String> {
        match self {
            Self::Rect {
                size_nm,
                corner_radius_nm,
            } => {
                let smaller = size_nm[0].min(size_nm[1]);
                if size_nm.iter().any(|s| *s <= Nm::ZERO) {
                    Some("a profile needs a positive size".into())
                } else if *corner_radius_nm < Nm::ZERO
                    || corner_radius_nm.get().saturating_mul(2) > smaller.get()
                {
                    Some("a corner radius lies between 0 and half the smaller side".into())
                } else {
                    None
                }
            }
        }
    }
}

/// A package datum's geometry in the model frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum PackageDatumGeometry {
    Frame {
        #[params]
        frame: DatumFrame,
    },
    Plane {
        origin_nm: [Nm; 3],
        normal: [f64; 3],
    },
    Axis {
        point_nm: [Nm; 3],
        direction: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diameter_nm: Option<Nm>,
    },
    /// A profile in the frame's x–y plane.
    Profile {
        #[params]
        frame: DatumFrame,
        #[params]
        profile: DatumProfile,
    },
    /// A profile swept `height` along the frame's z. A connector's opening
    /// is one: its frame on the mating face with z pointing out of the
    /// connector along the plug's insertion path, the profile the aperture
    /// the plug needs, the height how far that space reaches.
    Volume {
        #[params]
        frame: DatumFrame,
        #[params]
        profile: DatumProfile,
        height_nm: Nm,
    },
}

impl PackageDatumGeometry {
    pub fn kind(&self) -> DatumKind {
        match self {
            Self::Frame { .. } => DatumKind::Frame,
            Self::Plane { .. } => DatumKind::Plane,
            Self::Axis { .. } => DatumKind::Axis,
            Self::Profile { .. } => DatumKind::Profile,
            Self::Volume { .. } => DatumKind::Volume,
        }
    }

    fn problem(&self) -> Option<String> {
        let frame = |f: &DatumFrame| {
            (!f.valid()).then(|| "a frame needs two perpendicular unit axes".to_owned())
        };
        match self {
            Self::Frame { frame: f } => frame(f),
            Self::Plane { normal, .. } => {
                (!unit(*normal)).then(|| "a plane needs a unit normal".into())
            }
            Self::Axis {
                direction,
                diameter_nm,
                ..
            } => {
                if !unit(*direction) {
                    Some("an axis needs a unit direction".into())
                } else if diameter_nm.is_some_and(|d| d <= Nm::ZERO) {
                    Some("an axis diameter must be positive".into())
                } else {
                    None
                }
            }
            Self::Profile { frame: f, profile } => frame(f).or_else(|| profile.problem()),
            Self::Volume {
                frame: f,
                profile,
                height_nm,
            } => frame(f).or_else(|| profile.problem()).or_else(|| {
                (*height_nm <= Nm::ZERO).then(|| "a volume needs a positive height".into())
            }),
        }
    }
}

/// A datum a package declares: explicit geometry in the model frame,
/// revisioned with the package and the same for every model kind
/// (docs/architecture/board-interface.md, "How a connector's opening is
/// defined").
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct PackageDatum {
    /// Identity inside the package: a placement's token is
    /// `i<instance>.<name>`.
    pub name: DatumName,
    #[params]
    pub geometry: PackageDatumGeometry,
    /// Room an enclosure should leave round the datum (a cutout's
    /// clearance), when the package recommends one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_nm: Option<Nm>,
    /// Meant for the enclosure: the `external_datums` rule exports it for
    /// every placement.
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub origin: PackageDatumOrigin,
}

impl PackageDatum {
    pub fn kind(&self) -> DatumKind {
        self.geometry.kind()
    }

    /// What is wrong with this datum on its own, if anything: its name,
    /// axes, sizes or clearance.
    pub fn problem(&self) -> Option<String> {
        if !self.name.valid_for_package() {
            return Some(format!(
                "‘{}’ is not a package datum name (lower case [a-z][a-z0-9_]*, at most \
                 {MAX_DATUM_NAME} bytes, not frame or envelope)",
                self.name.0.chars().take(40).collect::<String>()
            ));
        }
        if self.clearance_nm.is_some_and(|c| c < Nm::ZERO) {
            return Some(format!("{}: a clearance cannot be negative", self.name.0));
        }
        self.geometry
            .problem()
            .map(|p| format!("{}: {p}", self.name.0))
    }
}

/// Problems with a package's datums: each datum's own, the count, and
/// repeated names.
pub fn datum_problems(datums: &[PackageDatum]) -> Vec<String> {
    let mut out: Vec<String> = datums.iter().filter_map(PackageDatum::problem).collect();
    if datums.len() > MAX_PACKAGE_DATUMS {
        out.push(format!(
            "a package declares at most {MAX_PACKAGE_DATUMS} datums"
        ));
    }
    let mut names = BTreeSet::new();
    for d in datums {
        if !names.insert(&d.name) {
            out.push(format!("two datums are named {}", d.name.0));
        }
    }
    out
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> bool {
    v.iter().all(|c| c.is_finite()) && (dot(v, v).sqrt() - 1.0).abs() <= DIRECTION_TOLERANCE
}
