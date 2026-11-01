//! The message style on the wording examples of docs/sidebar-ia.md, and
//! `project.issues` over fixtures: every title follows the style, every raw
//! message and refusal names objects by their user names, nothing is
//! dropped by merging, and every finding code is classified.
use super::style::{Problem, check, naming};
use super::*;

mod fixtures;

/// The rewritten titles of the design's wording table follow the style.
#[test]
fn rewritten_titles_follow_the_style() {
    for title in [
        "Package terminal 2 has no electrical terminal",
        "2 pads have no package terminal",
        "pad_l: the expression ends with an operator",
        "Circular reference: pad_l → pad_x → gap → pad_l",
        "Not published: 3 issues block publishing",
        "Part needs a name",
        "Enter the manufacturer",
        "Part EXR-10K0-F is for an older revision (r1)",
        "EXR-10K0-F is rated 5 %",
        "Enter a number, such as 10k or 4.7 kΩ",
        "R2 pad 1 moves from N2 to N5",
        "Removes R1, 1 wire and its PCB placement",
        "After this update: 4 new issues, 0 resolved",
        "1 track on N22 would join two nets",
        "R13 must stay inside the board (60 × 40 mm)",
        "R3 ↔ R6: 4 pads too close",
        "N7: 3 of 4 pads unrouted",
        "Too close to R6 pad 1 (N3): needs 0.2 mm",
        "Too close to R6 pad 1 (N3): needs 0.4 mm (net class Power)",
        "Too close to a via on GND: needs 0.15 mm (Fine 4/4)",
        "A via on GND: annular ring 0.050 mm is under the minimum",
        "R1 pad 2 is 0.1 mm from the board edge",
        "Track · N22",
        "No issues",
        "Symbol \"X\" r2 is missing from the project library",
    ] {
        assert_eq!(check(title), Vec::<Problem>::new(), "{title}");
    }
}

/// The audited messages the design replaces are each caught.
#[test]
fn audited_messages_break_the_style() {
    for (text, problem) in [
        (
            "Binding D20 r2 maps package terminal 2 to nothing",
            Problem::DefinitionId,
        ),
        (
            "footprint.param_expr: `gap`: circular reference",
            Problem::CodePrefix,
        ),
        ("Remove instance R1 (ID 8)", Problem::InternalId),
        ("Explicit removal required for tracks [68]", Problem::IdList),
        ("Conflicting tracks [3, 4] would merge", Problem::IdList),
        (
            "R2 pad 1 world geometry moves from net 2",
            Problem::NetNumber,
        ),
        ("Missing reference Id(3)", Problem::DebugText),
        ("`pad_l`: unexpected end of expression", Problem::CodeQuote),
        (
            "Preview the change with project.preview_update first",
            Problem::ApiName,
        ),
        ("Net N3: R1.2, R2.1", Problem::PinDot),
        ("Conflict { tracks: 2 } on N22", Problem::DebugText),
        ("Some(Smd) lands no terminal", Problem::DebugText),
        ("Part needs a name.", Problem::FullStop),
        ("Part needs a name\nand an MPN", Problem::MultiLine),
        ("", Problem::Empty),
        (
            "A Part needs a printable manufacturer and MPN before it can be published to the library",
            Problem::TooLong,
        ),
    ] {
        assert!(check(text).contains(&problem), "{text}: {problem:?}");
    }
}

/// Words that only look like the patterns are not flagged.
#[test]
fn names_that_resemble_ids_pass() {
    for text in [
        "Subnet 3 of N4",
        "Pad D1 r is unrouted",
        "DP2 r3x has no pad",
        "IDC header has 10 pins",
        "Footprint [left] side",
        "R13 · 10 kΩ · R0603",
        "Too close to R6 pad 1 (N3): needs 0.2 mm",
        "R1 pin 2 unconnected",
        "Pitch 0.5 mm, e.g. SOIC",
        "Version 1.2 of the library",
    ] {
        assert_eq!(check(text), Vec::<Problem>::new(), "{text}");
    }
}

/// Severity order is most severe first, so sorting rows puts errors on top.
#[test]
fn severity_sorts_most_severe_first() {
    let mut order = vec![
        Severity::Info,
        Severity::ToDo,
        Severity::Error,
        Severity::Warning,
    ];
    order.sort();
    assert_eq!(
        order,
        [
            Severity::Error,
            Severity::Warning,
            Severity::ToDo,
            Severity::Info
        ]
    );
}

/// A board check that stopped at its limit leaves geometry unchecked, so
/// errors may be uncounted: it is an error itself, counted on the bar, and
/// it names the board so its row locates.
#[test]
fn an_incomplete_board_check_is_a_counted_error_that_locates() {
    let project = crate::model::Project::default();
    let drafts = crate::library::Drafts::default();
    let names = names::Names::new(&project, &drafts);
    for layout in [
        crate::layout::courtyard_limit_finding(),
        crate::layout::copper_limit_finding(),
    ] {
        assert_eq!(rules::severity(&layout), Severity::Error);
        let row = row(&names, IssueArea::Board, vec![layout]);
        assert_eq!(counts(std::iter::once(&row)).error, 1);
        assert_eq!(
            row.objects().copied().collect::<Vec<_>>(),
            [ObjectRef::Board]
        );
        assert_eq!(check(&row.title), Vec::<Problem>::new(), "{}", row.title);
        assert!(
            row.title.starts_with("Board checks incomplete"),
            "{}",
            row.title
        );
    }
}
