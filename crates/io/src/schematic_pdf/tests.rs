use super::*;
use pcbcraft_core::geometry::Bounds;
use pcbcraft_core::library::TextMetric;
use pcbcraft_core::model::{LabelKind, PortShape, PowerStyle, SheetItem};
use pcbcraft_core::schematic::SchematicCommand as C;
use pcbcraft_core::{Engine, Operation, Outcome};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// A sheet point a test writes in mm; test points lie within the range.
fn pt(p: [f64; 2]) -> pcbcraft_core::units::PointNm {
    pcbcraft_core::units::PointNm::from_mm_f64_rounded(p).unwrap_or_default()
}

fn nm(mm: f64) -> pcbcraft_core::units::Nm {
    pcbcraft_core::units::Nm::from_mm_f64_rounded(mm).unwrap_or_default()
}

fn apply(
    engine: &mut Engine,
    command: C,
) -> std::result::Result<Outcome, Box<dyn std::error::Error>> {
    Ok(engine.apply(engine.revision(), Operation::Schematic(command))?)
}

/// The demo with a second sheet used by a sheet symbol on the first, a
/// global label and a power port on it, and a note on the first.
fn two_sheets() -> std::result::Result<(Engine, SheetId), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let Outcome::SheetSymbolPlaced { child, .. } = apply(
        &mut engine,
        C::PlaceSheetSymbol {
            sheet: None,
            child: None,
            name: Some("Power supply".into()),
            position: pt([150.0, -40.0]),
            size: pt([40.0, 30.0]),
        },
    )?
    else {
        return Err("no sheet symbol".into());
    };
    apply(
        &mut engine,
        C::PlaceLabel {
            sheet: Some(child),
            name: "VBUS".into(),
            kind: LabelKind::Global {
                shape: PortShape::Input,
            },
            position: pt([60.0, -60.0]),
            rotation: Default::default(),
            attach: None,
        },
    )?;
    apply(
        &mut engine,
        C::PlaceLabel {
            sheet: Some(child),
            name: "GND".into(),
            kind: LabelKind::Power {
                style: PowerStyle::Ground,
            },
            position: pt([80.0, -80.0]),
            rotation: Default::default(),
            attach: None,
        },
    )?;
    apply(
        &mut engine,
        C::AddDrawing {
            sheet: None,
            item: SheetItem::Text {
                at: pt([20.0, -150.0]),
                text: "Printed note".into(),
                height_nm: nm(2.5),
                rotation: Default::default(),
            },
        },
    )?;
    Ok((engine, child))
}

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

fn pages(pdf: &[u8]) -> usize {
    find(pdf, b"/Type /Page") - find(pdf, b"/Type /Pages")
}

#[test]
fn every_sheet_prints_one_page_with_its_title_block_and_text() -> TestResult {
    let (engine, _) = two_sheets()?;
    let pdf = schematic_pdf(engine.project(), &[])?;
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(pdf.trim_ascii_end().ends_with(b"%%EOF"));
    assert_eq!(find(&pdf, b"/Type /Pages"), 1);
    assert_eq!(find(&pdf, b"/MediaBox"), 2, "one page per sheet");
    assert_eq!(pages(&pdf), 2);
    for text in [
        "1 of 2",
        "2 of 2",
        "Power supply",
        "Printed note",
        "VBUS",
        "GND",
        engine.project().name.as_str(),
    ] {
        let literal = format!("({text})");
        assert!(
            find(&pdf, literal.as_bytes()) > 0,
            "{literal} is drawn as text"
        );
    }
    // A4 landscape: 297 × 210 mm in points.
    assert!(find(&pdf, b"/MediaBox [0 0 841.8898 595.2756]") == 2);
    Ok(())
}

#[test]
fn a_sheet_symbol_links_to_its_child_page() -> TestResult {
    let (engine, child) = two_sheets()?;
    let pdf = schematic_pdf(engine.project(), &[])?;
    assert_eq!(find(&pdf, b"/Subtype /Link"), 1);
    assert_eq!(find(&pdf, b"/S /GoTo"), 1);
    // Printing the parent alone leaves nothing to link to.
    let first = engine.project().sheets.first();
    assert_ne!(first, child);
    let alone = schematic_pdf(engine.project(), &[first])?;
    assert_eq!(find(&alone, b"/Subtype /Link"), 0);
    Ok(())
}

#[test]
fn a_selection_prints_in_page_order_and_refuses_unknown_sheets() -> TestResult {
    let (engine, child) = two_sheets()?;
    let one = schematic_pdf(engine.project(), &[child, child])?;
    assert_eq!(pages(&one), 1);
    assert!(find(&one, b"(2 of 2)") > 0);
    assert_eq!(find(&one, b"(1 of 2)"), 0);
    let error = schematic_pdf(engine.project(), &[SheetId(99)]);
    assert!(matches!(error, Err(Error::Invalid(_))));
    Ok(())
}

#[test]
fn export_writes_the_file_atomically_and_counts_pages() -> TestResult {
    let (engine, _) = two_sheets()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("schematic.pdf");
    let count = export_schematic_pdf(engine.project(), &[], &path)?;
    assert_eq!(count, 2);
    let bytes = std::fs::read(&path)?;
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(
        export_schematic_pdf(engine.project(), &[SheetId(42)], &dir.path().join("x.pdf")).is_err()
    );
    assert!(!dir.path().join("x.pdf").exists());
    Ok(())
}

#[test]
fn ohm_is_spelled_out_for_the_winansi_font() {
    assert_eq!(printable("10 kΩ ±5%"), "10 kOhm ±5%");
    assert_eq!(printable("a–b"), "a–b");
}

/// Whether two boxes overlap by more than touching.
fn overlap(a: &Bounds, b: &Bounds) -> bool {
    a.min[0] < b.max[0] && b.min[0] < a.max[0] && a.min[1] < b.max[1] && b.min[1] < a.max[1]
}

fn ink(d: &Drawing, m: TextMetric) -> Vec<Bounds> {
    d.strokes
        .iter()
        .map(|s| s.shape.bounds(s.width))
        .chain(d.texts.iter().map(|t| t.bounds(m)))
        .collect()
}

/// The gap a label keeps from pin names, numbers and no-connect crosses
/// in this check, mm: about a printed stroke.
const MARK_GAP_MM: f64 = 0.1;

/// The ESP32 dev board's 133 labels and power ports, all attached to pins
/// (gap 22 of docs/e2e-esp32-devboard.md): none overlaps a symbol body or
/// another label on either sheet, nor runs into a pin name or number or a
/// no-connect cross, and the print draws exactly them.
#[test]
fn the_esp32_board_labels_clear_bodies_and_each_other() -> TestResult {
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    ));
    let project = crate::read_project(path)?.project;
    // Label text measured as it prints (capitals in Helvetica).
    let m: TextMetric = scene::LABEL_METRIC;
    assert_eq!(project.sheets.list.len(), 2);
    let mut found = Vec::new();
    let mut count = 0;
    for sheet in project.sheets.list.iter().map(|s| s.id) {
        let printed = scene::sheet_scene(&project, sheet);
        let bodies: Vec<Bounds> = printed
            .units
            .iter()
            .flat_map(|u| &u.shapes)
            .filter(|s| s.fill == Fill::Body)
            .map(|s| s.shape.bounds(0.0))
            .collect();
        // Pin names and numbers, each with the space between it and its
        // pin line (a label's stem must not run under a number), and the
        // no-connect crosses: a label keeps a visible gap from them, across
        // the pin for pin text, all round for a cross. (text, box, gap).
        let marks: Vec<(String, Bounds, Point)> = printed
            .units
            .iter()
            .flat_map(|u| &u.pins)
            .flat_map(|p| {
                let horizontal =
                    (p.end[0] - p.body_end[0]).abs() >= (p.end[1] - p.body_end[1]).abs();
                p.name.iter().chain(&p.number).map(move |t| {
                    let b = t.bounds(m);
                    // Down to the pin line: only a label's leader, which
                    // continues the pin as a wire does, may run under it.
                    let toward = |lo: f64, hi: f64, line: f64| (lo.min(line), hi.max(line));
                    let (region, gap) = if horizontal {
                        let (lo, hi) = toward(b.min[1], b.max[1], p.end[1]);
                        (
                            Bounds {
                                min: [b.min[0], lo],
                                max: [b.max[0], hi],
                            },
                            [0.0, MARK_GAP_MM],
                        )
                    } else {
                        let (lo, hi) = toward(b.min[0], b.max[0], p.end[0]);
                        (
                            Bounds {
                                min: [lo, b.min[1]],
                                max: [hi, b.max[1]],
                            },
                            [MARK_GAP_MM, 0.0],
                        )
                    };
                    (format!("pin text {}", t.text), region, gap)
                })
            })
            .chain(printed.no_connects.iter().map(|p| {
                (
                    "a no-connect cross".to_owned(),
                    Bounds::point(*p).inflated(pcbcraft_core::schematic::NO_CONNECT_MM),
                    [MARK_GAP_MM, MARK_GAP_MM],
                )
            }))
            .collect();
        let drawn = scene::sheet_labels(&project, sheet);
        count += drawn.len();
        let name = |id| {
            project
                .label(id)
                .map(|l| l.name.clone())
                .unwrap_or_default()
        };
        for (i, (id, d)) in drawn.iter().enumerate() {
            let boxes = ink(d, m);
            // The label without its leader (a wire-width line from the pin).
            let symbol: Vec<Bounds> = d
                .strokes
                .iter()
                .filter(|s| s.width != scene::WIRE_MM)
                .map(|s| s.shape.bounds(s.width))
                .chain(d.texts.iter().map(|t| t.bounds(m)))
                .collect();
            if bodies.iter().any(|b| boxes.iter().any(|x| overlap(x, b))) {
                found.push(format!("{} ({id}) over a symbol", name(*id)));
            }
            for (what, mark, [gx, gy]) in &marks {
                let grown = |b: &Bounds| Bounds {
                    min: [b.min[0] - gx, b.min[1] - gy],
                    max: [b.max[0] + gx, b.max[1] + gy],
                };
                if symbol.iter().any(|b| overlap(&grown(b), mark)) {
                    found.push(format!("{} ({id}) against {what}", name(*id)));
                }
            }
            for (other, e) in drawn.iter().skip(i + 1) {
                let theirs = ink(e, m);
                if boxes.iter().any(|b| theirs.iter().any(|t| overlap(b, t))) {
                    found.push(format!(
                        "{} ({id}) over {} ({other})",
                        name(*id),
                        name(*other)
                    ));
                }
            }
            // The print draws this same placement.
            for t in &d.texts {
                assert!(printed.items.texts.contains(t), "{} printed", t.text);
            }
        }
    }
    assert_eq!(count, 133);
    assert!(found.is_empty(), "{} overlaps: {found:#?}", found.len());
    Ok(())
}

/// A rounded wire corner prints as its arc, flattened to the curve
/// tolerance (the PDF writer has no arcs): many more line segments than the
/// sharp corner. Where the arc runs is checked on the scene in core.
#[test]
fn a_rounded_wire_corner_prints_as_its_arc() -> TestResult {
    use pcbcraft_core::model::{Corner, FreeEnd};
    use pcbcraft_core::schematic::WireTarget;
    let mut engine = Engine::default();
    let free = |p: Point| FreeEnd::new(p).map(WireTarget::Free).ok_or("free");
    apply(
        &mut engine,
        C::DrawWire {
            sheet: None,
            from: free([20.0, -20.0])?,
            corners: vec![Corner::sharp(pt([60.0, -20.0]))],
            to: free([60.0, -60.0])?,
        },
    )?;
    let wire = engine.project().wires.first().map(|w| w.id).ok_or("wire")?;
    let segments = |pdf: &[u8]| find(pdf, b" l\n");
    let sharp = schematic_pdf(engine.project(), &[])?;
    apply(
        &mut engine,
        C::SetWireRadius {
            wire,
            index: 0,
            radius: nm(10.0),
        },
    )?;
    let rounded = schematic_pdf(engine.project(), &[])?;
    // A 10 mm quarter arc within 0.02 mm of its chords takes about 13.
    assert!(
        segments(&rounded) >= segments(&sharp) + 10,
        "{} then {}",
        segments(&sharp),
        segments(&rounded)
    );
    Ok(())
}
