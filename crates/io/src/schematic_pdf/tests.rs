use super::*;
use pcbcraft_core::model::{LabelKind, PortShape, PowerStyle, SheetItem};
use pcbcraft_core::schematic::SchematicCommand as C;
use pcbcraft_core::{Engine, Operation, Outcome};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

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
            position: [150.0, -40.0],
            size: [40.0, 30.0],
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
            position: [60.0, -60.0],
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
            position: [80.0, -80.0],
            rotation: Default::default(),
            attach: None,
        },
    )?;
    apply(
        &mut engine,
        C::AddDrawing {
            sheet: None,
            item: SheetItem::Text {
                at: [20.0, -150.0],
                text: "Printed note".into(),
                height: 2.5,
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
fn text_is_win_ansi_with_ohm_spelled_out() {
    assert_eq!(win_ansi("10 kΩ ±5%"), b"10 kOhm \xb15%".to_vec());
    assert_eq!(win_ansi("a–b"), b"a\x96b".to_vec());
    assert_eq!(win_ansi("π"), b"?".to_vec());
    assert!((width(b"A", 10.0) - 6.67).abs() < 1e-9);
}
