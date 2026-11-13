//! Two-terminal rectangular chips: a ceramic (or ferrite) body between two
//! wrapped end terminations, and on a thick-film resistor a black overcoat
//! raised on the alumina's top. A chip LED is a thin white substrate and
//! terminations under a clear lens. Bodies are disjoint boxes with rounded
//! edges: the body's long edges and each termination's outer end.
use super::{Build, check_positive, mm, require, top_edges};
use crate::Result;
use sparksmith_core::library::{BodyRole, ChipForm, ChipKind};
use sparksmith_core::materials::MaterialId;

/// Edge rounds (est.: tumbled ceramic chips and dipped terminations show
/// 20–80 µm radii), capped by the chip's size below.
const BODY_EDGE_MM: f64 = 0.06;
const END_EDGE_MM: f64 = 0.08;

/// The overcoat's thickness (est.: thick-film overcoats are 10–40 µm), at
/// most a tenth of the chip height.
const OVERCOAT_MM: f64 = 0.03;
/// The overcoat stops this far in from the long edges (docs/materials.md).
const OVERCOAT_INSET_MM: f64 = 0.04;

/// A chip LED's substrate thickness (Kingbright's APT1608 and APT2012
/// drawings both give 0.25 mm), at most half the height.
const LED_SUBSTRATE_MM: f64 = 0.25;
/// Round of the lens's top edges (est.), capped by the lens's size.
const LENS_EDGE_MM: f64 = 0.1;

pub(super) fn check(c: &ChipForm) -> Result<()> {
    check_positive(&[
        ("chip length", c.length()),
        ("chip width", c.width()),
        ("chip height", c.height()),
        ("termination length", c.termination()),
    ])?;
    require(
        2.0 * c.termination() < c.length(),
        "the two terminations must leave part of the body between them",
    )?;
    Ok(())
}

pub(super) fn build(b: &mut Build<'_>, c: &ChipForm) -> Result<()> {
    let l = b.length("length", c.length())?;
    let w = b.length("width", c.width())?;
    let h = b.length("height", c.height())?;
    let t = b.length("termination", c.termination())?;
    let resistor = c.kind == ChipKind::Resistor;
    let overcoat = (c.height() / 10.0).min(OVERCOAT_MM);
    let inset = OVERCOAT_INSET_MM.min(c.width() / 4.0);
    let led = c.kind == ChipKind::Led;
    let substrate = LED_SUBSTRATE_MM.min(c.height() / 2.0);
    let body_height = if resistor {
        let oc = b.length("overcoat", overcoat)?;
        format!("{h} - {oc}")
    } else if led {
        b.length("substrate", substrate)?
    } else {
        h.clone()
    };
    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [
            format!("- {l} / 2 + {t}"),
            format!("- {w} / 2"),
            "0 mm".into(),
        ],
        [format!("{l} - 2 * {t}"), w.clone(), body_height.clone()],
    )?;
    if resistor {
        let oc = b.p("overcoat");
        let name = b.name("overcoat");
        let material = b
            .materials
            .roles
            .iter()
            .find(|r| r.role == BodyRole::Other)
            .map_or(MaterialId::ResistorOvercoat, |r| r.material);
        b.solid(
            &name,
            BodyRole::Other,
            material,
            [
                format!("- {l} / 2 + {t}"),
                format!("- {w} / 2 + {}", mm(inset)),
                format!("{h} - {oc}"),
            ],
            [
                format!("{l} - 2 * {t}"),
                format!("{w} - 2 * {}", mm(inset)),
                oc.clone(),
            ],
        )?;
    }
    if led {
        let name = b.name("lens");
        let material = b.material(BodyRole::Lens);
        b.solid(
            &name,
            BodyRole::Lens,
            material,
            [
                format!("- {l} / 2 + {t}"),
                format!("- {w} / 2"),
                body_height.clone(),
            ],
            [
                format!("{l} - 2 * {t}"),
                w.clone(),
                format!("{h} - {body_height}"),
            ],
        )?;
        let half = (c.length() / 2.0 - c.termination(), c.width() / 2.0);
        let radius = LENS_EDGE_MM
            .min(half.1 / 2.0)
            .min((c.height() - substrate) / 2.0);
        b.round(
            "lens_round",
            &name,
            &top_edges([-half.0, -half.1, substrate], [half.0, half.1, c.height()]),
            radius,
        )?;
    }
    // An LED's terminations wrap only its substrate.
    let end_height = if led { body_height.clone() } else { h.clone() };
    let end_top = if led { substrate } else { c.height() };
    let material = b.material(BodyRole::Termination);
    let mut ends = Vec::new();
    for (part, x) in [
        ("termination 1", format!("- {l} / 2")),
        ("termination 2", format!("{l} / 2 - {t}")),
    ] {
        let name = b.name(part);
        b.solid(
            &name,
            BodyRole::Termination,
            material,
            [x, format!("- {w} / 2"), "0 mm".into()],
            [t.clone(), w.clone(), end_height.clone()],
        )?;
        ends.push(name);
    }
    // The body's four long edges, kept below the overcoat's inset so the
    // overcoat sits on the flat; then the edges round each outer end face.
    let (hl, hw) = (c.length() / 2.0, c.width() / 2.0);
    let small = c.width().min(end_top);
    let top = if resistor {
        c.height() - overcoat
    } else {
        end_top
    };
    let mut body_edge = BODY_EDGE_MM.min(0.1 * small);
    if resistor {
        body_edge = body_edge.min(inset);
    }
    let along = |y: f64, z: f64| [0.0, y, z];
    b.round(
        "body_round",
        &body,
        &[
            along(-hw, 0.0),
            along(hw, 0.0),
            along(hw, top),
            along(-hw, top),
        ],
        body_edge,
    )?;
    let end_edge = END_EDGE_MM.min(0.15 * small).min(c.termination() / 3.0);
    let r = b.length("end_round", end_edge)?;
    for (name, x) in ends.iter().zip([-hl, hl]) {
        let edges = [
            [x, 0.0, 0.0],
            [x, hw, end_top / 2.0],
            [x, 0.0, end_top],
            [x, -hw, end_top / 2.0],
        ];
        let feature = format!("{name} round");
        b.native.fillet(&feature, name, &edges, r.clone().into())?;
    }
    b.number(&ends);
    Ok(())
}
