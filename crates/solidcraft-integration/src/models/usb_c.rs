//! USB-C receptacles: a nickel-plated obround shell tube with the mating
//! opening at −y, a black LCP insulator closing its rear and carrying the
//! tongue, gold solder tails on the contact lands, four shell stakes going
//! through the board and two LCP locating pegs.
//!
//! The stakes hang in the plane of the shell's side walls, up to where its
//! rounded underside meets them. Bodies stay disjoint: they only touch.
//! All four stakes carry the shell terminal.
//!
//! Looks: [`MaterialId::Nickel`] shell and stakes, [`MaterialId::Lcp`]
//! insulator, tongue and pegs, [`MaterialId::Gold`] tails
//! (docs/materials.md).
use super::{Build, check_positive, require};
use crate::Result;
use crate::native::{Plane, ProfileBody, mm};
use crate::pad_outline::Contour;
use sparksmith_core::library::package_form::usb_c::UsbCForm;
use sparksmith_core::library::{BodyRole, Outline, Segment};

/// The builder revision (`PackageModel::Generated { family_rev }`).
pub(super) const FAMILY_REV: Option<u16> = Some(1);

/// Share of the opening's width and height the tongue fills (est.: a
/// Type-C tongue is about 6.7 × 0.7 mm in an 8.34 × 2.56 mm opening).
const TONGUE_WIDTH_SHARE: f64 = 0.8;
const TONGUE_THICKNESS_SHARE: f64 = 0.27;
/// How far the tongue's tip stays behind the mating face (est.).
const TONGUE_RECESS_MM: f64 = 0.6;
/// Depth of the insulator closing the shell's rear (est.).
const INSULATOR_DEPTH_MM: f64 = 2.5;
/// Solder tail thickness and how far it runs under the shell's rear (est.).
const TAIL_THICKNESS_MM: f64 = 0.15;
const TAIL_UNDER_MM: f64 = 0.6;
/// A tail's width as a share of its land: the drawing's 0.20 mm on the
/// 0.30 mm signal lands and 0.45 mm on the 0.60 mm shared lands.
const SIGNAL_TAIL_SHARE: f64 = 2.0 / 3.0;
const POWER_TAIL_SHARE: f64 = 0.75;
/// How much narrower a stake is than its slot along y (est.: the drawing
/// gives 1.10 mm for the front stake in its 1.40 mm slot).
const STAKE_SLOT_CLEARANCE_MM: f64 = 0.3;
/// How far a locating peg reaches below the seating plane (est.).
const PEG_BELOW_MM: f64 = 0.5;

pub(super) fn check(f: &UsbCForm) -> Result<()> {
    check_positive(&[
        ("receptacle width", f.width()),
        ("receptacle depth", f.depth()),
        ("shell thickness", f.shell_thickness()),
        ("stake length", f.stake_length()),
    ])?;
    require(
        INSULATOR_DEPTH_MM + TONGUE_RECESS_MM < f.depth(),
        "the depth must leave room for the insulator and the tongue",
    )?;
    require(
        f.tabs
            .iter()
            .all(|t| t.slot()[1] > STAKE_SLOT_CLEARANCE_MM && f.shell_thickness() < t.slot()[0]),
        "each stake must fit its slot",
    )
}

/// An obround `half[0]` × `half[1]` about (`u`, `v`), in sketch coordinates.
fn obround(u: f64, v: f64, half: [f64; 2]) -> Contour {
    let radius = half[1];
    let flat = (half[0] - radius).max(0.0);
    Contour::Outline(Outline {
        start: [u - flat, v - radius],
        segments: vec![
            Segment::Line {
                to: [u + flat, v - radius],
            },
            Segment::Arc {
                to: [u + flat, v + radius],
                bulge: 1.0,
            },
            Segment::Line {
                to: [u - flat, v + radius],
            },
            Segment::Arc {
                to: [u - flat, v - radius],
                bulge: 1.0,
            },
        ],
    })
}

pub(super) fn build(b: &mut Build<'_>, f: &UsbCForm) -> Result<()> {
    let depth = b.length("depth", f.depth())?;
    let stake_length = b.length("stake_length", f.stake_length())?;
    let [ow, oh] = f.opening();
    let t = f.shell_thickness();
    let half_inner = [ow / 2.0, oh / 2.0];
    let half_outer = [ow / 2.0 + t, oh / 2.0 + t];
    let front = -f.depth() / 2.0;
    let rear = f.depth() / 2.0;
    // The XZ plane maps (u, v) to (u, offset, −v) and extrudes along +y.
    let centre_v = -f.opening_z();

    let shell_material = b.material(BodyRole::Other);
    let shell = b.name("shell");
    let contours = [
        obround(0.0, centre_v, half_outer),
        obround(0.0, centre_v, half_inner),
    ];
    b.native.plane_body(
        Plane::Xz,
        ProfileBody {
            name: &shell,
            offset: format!("- {depth} / 2").into(),
            contours: &contours,
            height: depth.clone().into(),
        },
    )?;
    b.push(shell, BodyRole::Other, shell_material);

    let plastic = b.material(BodyRole::Body);
    let insulator = b.name("insulator");
    let insulator_depth = INSULATOR_DEPTH_MM;
    b.native.plane_body(
        Plane::Xz,
        ProfileBody {
            name: &insulator,
            offset: format!("{depth} / 2 - ({})", mm(insulator_depth)).into(),
            contours: &[obround(0.0, centre_v, half_inner)],
            height: mm(insulator_depth).into(),
        },
    )?;
    b.push(insulator, BodyRole::Body, plastic);

    let tongue_width = ow * TONGUE_WIDTH_SHARE;
    let tongue_thickness = oh * TONGUE_THICKNESS_SHARE;
    let tip = front + TONGUE_RECESS_MM;
    let tongue_end = rear - INSULATOR_DEPTH_MM;
    let tongue = b.name("tongue");
    b.solid(
        &tongue,
        BodyRole::Body,
        plastic,
        [
            mm(-tongue_width / 2.0),
            mm(tip),
            mm(f.opening_z() - tongue_thickness / 2.0),
        ],
        [mm(tongue_width), mm(tongue_end - tip), mm(tongue_thickness)],
    )?;

    // The solder tails, in lead order, each from under the shell's rear to
    // short of its land's outer end.
    let tail_material = b.material(BodyRole::Lead);
    let tail_start = rear - TAIL_UNDER_MM;
    let tail_end = f.land_y() + f.land_length() / 2.0 - f.land_length() / 10.0;
    let mut tails = Vec::with_capacity(12);
    for (k, (x, land)) in f.contact_lands().into_iter().enumerate() {
        let share = if (land - f.power_land_width()).abs() < 1e-9 {
            POWER_TAIL_SHARE
        } else {
            SIGNAL_TAIL_SHARE
        };
        let width = land * share;
        let tail = b.name(&format!("tail {}", k + 1));
        b.solid(
            &tail,
            BodyRole::Lead,
            tail_material,
            [mm(x - width / 2.0), mm(tail_start), "0 mm".into()],
            [mm(width), mm(tail_end - tail_start), mm(TAIL_THICKNESS_MM)],
        )?;
        tails.push(tail);
    }
    b.number(&tails);

    // The stakes: at the outline, one sheet thick, from below the board up
    // to where the shell's rounded underside reaches their inner face.
    let inner_x = f.width() / 2.0 - t;
    let radius = half_outer[1];
    let beyond = (inner_x - (half_outer[0] - radius)).max(0.0).min(radius);
    let stake_top = f.opening_z() - (radius * radius - beyond * beyond).sqrt();
    let shell_terminal = u32::try_from(tails.len() + 1).ok();
    let tabs = [f.tabs[0], f.tabs[0], f.tabs[1], f.tabs[1]];
    for (k, (centre, tab)) in f.stake_centres().into_iter().zip(tabs).enumerate() {
        let along = tab.slot()[1] - STAKE_SLOT_CLEARANCE_MM;
        let x = if centre[0] < 0.0 {
            -f.width() / 2.0
        } else {
            inner_x
        };
        let stake = b.name(&format!("stake {}", k + 1));
        b.solid(
            &stake,
            BodyRole::Lead,
            shell_material,
            [
                mm(x),
                mm(centre[1] - along / 2.0),
                format!("- {stake_length}"),
            ],
            [
                mm(t),
                mm(along),
                format!("{stake_length} + {}", mm(stake_top)),
            ],
        )?;
        if let Some(body) = b.bodies.iter_mut().find(|body| body.name == stake) {
            body.terminal = shell_terminal;
        }
    }

    // The locating pegs, up to the shell's underside.
    let shell_bottom = f.opening_z() - half_outer[1];
    for (k, x) in [-f.peg_x(), f.peg_x()].into_iter().enumerate() {
        let peg = b.name(&format!("peg {}", k + 1));
        b.native.profile_body(ProfileBody {
            name: &peg,
            offset: mm(-PEG_BELOW_MM).into(),
            contours: &[Contour::Circle {
                center: [x, f.peg_y()],
                radius: f.peg_diameter() / 2.0,
            }],
            height: mm(PEG_BELOW_MM + shell_bottom).into(),
        })?;
        b.push(peg, BodyRole::Body, plastic);
    }
    Ok(())
}
