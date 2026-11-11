use super::*;
use crate::diagnostics::{Error, Result};
use crate::library::graphics::{TextMetric, on_grid};

fn terminals(n: usize) -> Vec<TerminalId> {
    (1..=n as u32).map(TerminalId).collect()
}

fn drawn() -> impl Iterator<Item = SymbolKind> {
    SymbolKind::ALL
        .into_iter()
        .filter(|k| k.terminal_count().is_some())
}

fn build(kind: SymbolKind, conventions: SymbolConventions) -> Result<SymbolDefinition> {
    let n = kind
        .terminal_count()
        .ok_or_else(|| Error::Invalid("a box kind".into()))?;
    generate(kind, conventions, DefinitionId(7), &terminals(n))
}

fn unit(symbol: &SymbolDefinition) -> Result<&Unit> {
    symbol
        .units
        .first()
        .ok_or_else(|| Error::Invalid("no unit".into()))
}

fn pin(symbol: &SymbolDefinition, terminal: u32) -> Result<&Pin> {
    unit(symbol)?
        .pin(TerminalId(terminal))
        .ok_or_else(|| Error::Invalid(format!("no pin {terminal}")))
}

fn conventions() -> [SymbolConventions; 2] {
    [
        SymbolConventions::default(),
        SymbolConventions {
            resistor: ResistorStyle::Us,
        },
    ]
}

/// Every drawn kind is a valid one-unit symbol: one pin per terminal on
/// the 2.5 mm module, no blocking graphic finding, bounded and finite
/// geometry, sequential IDs, designator text and sane bounds.
#[test]
fn every_kind_is_a_valid_symbol_on_the_grid() -> Result<()> {
    for conventions in conventions() {
        for kind in drawn() {
            let symbol = build(kind, conventions)?;
            let unit = unit(&symbol)?;
            let count = kind.terminal_count().unwrap_or_default();
            assert_eq!(unit.pins.len(), count, "{kind:?} pins");
            for k in 1..=count as u32 {
                let pin = pin(&symbol, k)?;
                assert!(
                    on_grid(pin.position_mm(), graphics::SYMBOL_MODULE_MM),
                    "{kind:?} pin {k} at {:?} is off the module",
                    pin.position
                );
                assert!(pin.length_nm.get() > 0, "{kind:?} pin {k} has no stub");
                assert!(!pin.name.visible, "{kind:?} shows pin names");
                assert_eq!(
                    pin.number.visible,
                    count == 3,
                    "{kind:?} pin numbers show only on transistors"
                );
            }
            assert!(symbol.within_bounds(), "{kind:?} bounds");
            assert!(symbol.geometry_finite(), "{kind:?} geometry");
            let mut findings = Vec::new();
            for graphic in &unit.graphics {
                graphics::graphic_findings(DefinitionId(9), graphic, &mut findings);
            }
            assert!(findings.is_empty(), "{kind:?}: {findings:?}");
            let ids: Vec<u32> = unit.graphics.iter().map(|g| g.id.0).collect();
            let expected: Vec<u32> = (2..symbol.next_local).collect();
            assert_eq!(ids, expected, "{kind:?} graphic IDs");
            assert!(
                unit.graphics.iter().any(|g| matches!(
                    &g.item,
                    Item::Text(t) if t.content == TextContent::Designator
                )),
                "{kind:?} has no designator text"
            );
            let bounds = unit
                .bounds(TextMetric::default())
                .ok_or_else(|| Error::Invalid("empty".into()))?;
            let size = bounds.size();
            assert!(
                size[0] >= 2.0 && size[1] >= 2.0 && size[0] <= 20.0 && size[1] <= 20.0,
                "{kind:?} bounds {bounds:?}"
            );
            // Pins reach out of the drawing: each connection end is on the
            // bounds' edge.
            for pin in &unit.pins {
                let p = pin.position_mm();
                let edge = [bounds.min[0], bounds.max[0]].contains(&p[0])
                    || [bounds.min[1], bounds.max[1]].contains(&p[1]);
                assert!(edge, "{kind:?} pin end {p:?} is inside {bounds:?}");
            }
        }
    }
    Ok(())
}

#[test]
fn two_terminal_kinds_share_one_pin_span() -> Result<()> {
    for kind in drawn().filter(|k| k.terminal_count() == Some(2)) {
        let symbol = build(kind, SymbolConventions::default())?;
        let mut ends: Vec<Point> = unit(&symbol)?
            .pins
            .iter()
            .map(|p| p.position_mm())
            .collect();
        ends.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert_eq!(ends, vec![[-5.0, 0.0], [5.0, 0.0]], "{kind:?}");
    }
    Ok(())
}

#[test]
fn the_box_kinds_and_wrong_counts_are_refused() {
    for kind in [SymbolKind::IcBox, SymbolKind::Connector] {
        assert!(
            generate(
                kind,
                SymbolConventions::default(),
                DefinitionId(7),
                &terminals(4)
            )
            .is_err()
        );
    }
    let refused = generate(
        SymbolKind::Capacitor,
        SymbolConventions::default(),
        DefinitionId(7),
        &terminals(3),
    );
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.starts_with("template.symbol")),
        "{refused:?}"
    );
}

fn shapes(symbol: &SymbolDefinition) -> Vec<(Shape, Fill)> {
    symbol
        .graphics()
        .filter_map(|g| match &g.item {
            Item::Shape { shape, fill, .. } => Some((shape.to_mm(), *fill)),
            Item::Text(_) => None,
        })
        .collect()
}

/// The two resistor conventions: a body-filled rectangle (graphic 2, the
/// body the editor grows) or an open zigzag.
#[test]
fn resistor_style_follows_the_conventions() -> Result<()> {
    let iec = build(SymbolKind::Resistor, SymbolConventions::default())?;
    let first = unit(&iec)?.graphic(GraphicId(2)).map(|g| &g.item);
    assert!(matches!(
        first,
        Some(Item::Shape {
            shape: crate::library::ShapeNm::Rect { .. },
            fill: Fill::Body,
            ..
        })
    ));
    let us = build(
        SymbolKind::Resistor,
        SymbolConventions {
            resistor: ResistorStyle::Us,
        },
    )?;
    let zigzag = shapes(&us);
    assert!(
        matches!(zigzag.first(), Some((Shape::Polyline { points, .. }, Fill::None)) if points.len() == 8)
    );
    assert!(!zigzag.iter().any(|(s, _)| matches!(s, Shape::Rect { .. })));
    Ok(())
}

/// A capacitor is two plates, never a rectangle; a polarised one curves
/// its − plate and marks + on terminal 1's side.
#[test]
fn capacitors_draw_plates_and_polarity() -> Result<()> {
    let plain = build(SymbolKind::Capacitor, SymbolConventions::default())?;
    let plates = shapes(&plain);
    assert_eq!(plates.len(), 2);
    assert!(
        plates
            .iter()
            .all(|(s, _)| matches!(s, Shape::Line { a, b } if a[0] == b[0]))
    );
    let polar = build(SymbolKind::PolarisedCapacitor, SymbolConventions::default())?;
    assert!(
        shapes(&polar)
            .iter()
            .any(|(s, _)| matches!(s, Shape::Arc { center, .. } if center[0] > 0.0))
    );
    assert!(
        pin(&polar, 1)?.position_mm()[0] < 0.0,
        "+ is terminal 1, left"
    );
    // The + mark's vertical stroke sits on the + side.
    assert!(shapes(&polar).iter().any(|(s, _)| matches!(
        s,
        Shape::Line { a, b } if a[0] == b[0] && a[0] < -1.0 && a[1] > 0.5
    )));
    Ok(())
}

/// Diodes put the cathode (terminal 1) at the bar, the triangle's apex.
#[test]
fn diodes_put_the_cathode_at_the_bar() -> Result<()> {
    for kind in [
        SymbolKind::Diode,
        SymbolKind::Zener,
        SymbolKind::Schottky,
        SymbolKind::Led,
    ] {
        let symbol = build(kind, SymbolConventions::default())?;
        let apex = shapes(&symbol)
            .iter()
            .find_map(|(s, _)| match s {
                Shape::Polygon { points, .. } if points.len() == 3 => {
                    points.iter().map(|p| p[0]).reduce(f64::max)
                }
                _ => None,
            })
            .ok_or_else(|| Error::Invalid("no triangle".into()))?;
        assert!(pin(&symbol, 1)?.position_mm()[0] > apex, "{kind:?}");
        assert!(pin(&symbol, 2)?.position_mm()[0] < 0.0, "{kind:?}");
    }
    let led = build(SymbolKind::Led, SymbolConventions::default())?;
    let heads = shapes(&led)
        .iter()
        .filter(|(s, f)| *f == Fill::Solid && matches!(s, Shape::Polygon { .. }))
        .count();
    assert_eq!(heads, 2, "two solid arrow heads");
    Ok(())
}

/// Transistor arrows: NPN out of the base towards the emitter, PNP in;
/// N-channel towards the channel, P-channel away.
#[test]
fn transistor_arrows_point_the_right_way() -> Result<()> {
    let tip = |kind| -> Result<Point> {
        let symbol = build(kind, SymbolConventions::default())?;
        shapes(&symbol)
            .iter()
            .find_map(|(s, f)| match s {
                Shape::Polygon { points, .. } if *f == Fill::Solid => points.first().copied(),
                _ => None,
            })
            .ok_or_else(|| Error::Invalid("no arrow".into()))
    };
    let (npn, pnp) = (tip(SymbolKind::NpnBjt)?, tip(SymbolKind::PnpBjt)?);
    assert!(
        npn[0] > pnp[0],
        "NPN tip nearer the emitter: {npn:?} {pnp:?}"
    );
    let (n, p) = (tip(SymbolKind::NMosfet)?, tip(SymbolKind::PMosfet)?);
    assert!(n[0] < p[0], "N-channel tip at the channel: {n:?} {p:?}");
    Ok(())
}

#[test]
fn prefixes_name_their_kinds() {
    assert_eq!(
        SymbolKind::for_prefix("c", false),
        Some(SymbolKind::Capacitor)
    );
    assert_eq!(
        SymbolKind::for_prefix("C", true),
        Some(SymbolKind::PolarisedCapacitor)
    );
    assert_eq!(
        SymbolKind::for_prefix("FB", false),
        Some(SymbolKind::FerriteBead)
    );
    assert_eq!(SymbolKind::for_prefix("U", false), None);
    for kind in drawn().filter(|k| k.terminal_count() == Some(2)) {
        let back = SymbolKind::for_prefix(kind.prefix(), kind.polarised());
        assert!(back.is_some(), "{kind:?}");
    }
}
