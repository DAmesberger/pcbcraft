//! Two-terminal rectangular chips: a ceramic (or ferrite) body between two
//! wrapped end terminations, and on a thick-film resistor a black overcoat
//! raised on the alumina's top. Bodies are disjoint boxes.
use super::{Build, check_positive, mm, require};
use crate::Result;
use pcbcraft_core::library::{BodyRole, ChipForm, ChipKind};
use pcbcraft_core::materials::MaterialId;

/// The overcoat's thickness (est.: thick-film overcoats are 10–40 µm), at
/// most a tenth of the chip height.
const OVERCOAT_MM: f64 = 0.03;
/// The overcoat stops this far in from the long edges (docs/materials.md).
const OVERCOAT_INSET_MM: f64 = 0.04;

pub(super) fn check(c: &ChipForm) -> Result<()> {
    check_positive(&[
        ("chip length", c.length),
        ("chip width", c.width),
        ("chip height", c.height),
        ("termination length", c.termination),
    ])?;
    require(
        2.0 * c.termination < c.length,
        "the two terminations must leave part of the body between them",
    )?;
    Ok(())
}

pub(super) fn build(b: &mut Build<'_>, c: &ChipForm) -> Result<()> {
    let l = b.length("length", c.length)?;
    let w = b.length("width", c.width)?;
    let h = b.length("height", c.height)?;
    let t = b.length("termination", c.termination)?;
    let resistor = c.kind == ChipKind::Resistor;
    let overcoat = (c.height / 10.0).min(OVERCOAT_MM);
    let inset = OVERCOAT_INSET_MM.min(c.width / 4.0);
    let body_height = if resistor {
        let oc = b.length("overcoat", overcoat)?;
        format!("{h} - {oc}")
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
            [t.clone(), w.clone(), h.clone()],
        )?;
        ends.push(name);
    }
    b.number(&ends);
    Ok(())
}
