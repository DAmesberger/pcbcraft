use super::*;

fn at(time: f64) -> Sample {
    Sample {
        time,
        command_only: true,
        ..Sample::default()
    }
}

/// ⌘ alone shows the badges once held for the delay, not before; a key
/// (a ⌘ chord), a click or a scroll while held hides them until ⌘ is let
/// go, and the next hold starts the timer again.
#[test]
fn the_hold_threshold_and_what_cancels_it() {
    let delay = f64::from(pcbcraft_io::settings::HOLD_MS_DEFAULT) / 1000.0;
    let mut hold = Hold::default();
    assert!(!hold.update(at(10.0), delay));
    assert_eq!(hold.wake_in(10.0, delay), Some(delay));
    assert!(!hold.update(at(10.0 + delay - 0.01), delay));
    assert!(hold.update(at(10.0 + delay), delay), "shown at the delay");
    let chord = Sample {
        interrupted: true,
        ..at(10.6)
    };
    assert!(!hold.update(chord, delay), "⌘S hides it");
    assert!(!hold.update(at(12.0), delay), "still held: stays hidden");
    assert_eq!(hold.wake_in(12.0, delay), None);
    assert!(!hold.update(Sample::default(), delay), "let go");
    assert!(!hold.update(at(20.0), delay));
    // A ⌘-click before the delay never shows them.
    let click = Sample {
        interrupted: true,
        ..at(20.1)
    };
    assert!(!hold.update(click, delay));
    assert!(!hold.update(at(25.0), delay));
    // Another modifier with ⌘ is a chord in the making, not a hold.
    let mut hold = Hold::default();
    let shifted = Sample {
        command_only: false,
        ..at(30.0)
    };
    assert!(!hold.update(shifted, delay));
    assert!(!hold.update(shifted, delay));
}

/// ⌥ pressed and let go alone is a tap; ⌥ with a click, a key, a scroll or
/// another modifier (⌥-drag, Shift+⌥ navigation) is not.
#[test]
fn only_a_bare_alt_tap_counts() {
    let alt = |interrupted, alt_only| Sample {
        alt: true,
        alt_only,
        interrupted,
        ..Sample::default()
    };
    let up = Sample::default();
    let mut tap = AltTap::default();
    assert!(!tap.update(alt(false, true)));
    assert!(tap.update(up), "a tap");
    assert!(!tap.update(alt(false, true)));
    assert!(!tap.update(alt(true, true)), "⌥-click");
    assert!(!tap.update(up));
    assert!(!tap.update(alt(false, true)));
    assert!(!tap.update(alt(false, false)), "Shift joined");
    assert!(!tap.update(up));
    assert!(!tap.update(alt(false, true)));
    assert!(tap.update(up), "the next bare tap counts again");
}

/// The registry of control rects keeps its allocation: the same controls
/// drawn every frame never grow it.
#[test]
fn the_rect_registry_does_not_grow_per_frame() {
    let ctx = egui::Context::default();
    let mut copy = Vec::new();
    let mut capacities = Vec::new();
    for _ in 0..200 {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            begin_frame(ui.ctx());
            for (i, id) in ["route", "via", "tune", "fill"].into_iter().enumerate() {
                let rect = Rect::from_min_size(pos2(i as f32 * 60.0, 0.0), vec2(56.0, 60.0));
                register(ui.ctx(), Target::Tool(id), rect);
            }
            register(ui.ctx(), Target::App(AppCommand::Save), Rect::ZERO);
            copy_targets(ui.ctx(), &mut copy);
            let capacity = ui
                .ctx()
                .data_mut(|d| d.get_temp_mut_or_default::<Targets>(slot()).capacity());
            capacities.push((capacity, copy.capacity()));
        })
        .textures_delta
        .clear();
        assert_eq!(copy.len(), 5);
    }
    assert!(
        capacities.windows(2).all(|w| w[0] == w[1]),
        "{capacities:?}"
    );
}

fn tips(entries: &[(&str, Option<char>)]) -> Vec<String> {
    let entries: Vec<TipEntry<'_>> = entries
        .iter()
        .map(|(title, key)| TipEntry {
            title,
            key: *key,
            late: false,
        })
        .collect();
    assign_tips(&entries)
}

/// A single-letter shortcut is the tip; otherwise the first letter while
/// free; otherwise two letters of the title, never starting with a
/// one-letter tip, never a digit.
#[test]
fn tips_follow_shortcuts_then_titles() {
    let got = tips(&[
        ("Layout", None),
        ("3D", None),
        ("Update PCB", None),
        ("Select", None),
        ("Route", Some('R')),
        ("Via", Some('V')),
        ("Tune", Some('T')),
        ("Shape", Some('B')),
        ("Fill", Some('P')),
        ("Refill", None),
        ("Production", None),
    ]);
    assert_eq!(
        got,
        ["L", "D", "U", "S", "R", "V", "T", "B", "P", "F", "C"]
            .map(str::to_owned)
            .to_vec()
    );
    // Same input, same tips.
    assert_eq!(
        got,
        tips(&[
            ("Layout", None),
            ("3D", None),
            ("Update PCB", None),
            ("Select", None),
            ("Route", Some('R')),
            ("Via", Some('V')),
            ("Tune", Some('T')),
            ("Shape", Some('B')),
            ("Fill", Some('P')),
            ("Refill", None),
            ("Production", None),
        ])
    );
    // A later word's initial before the first word's consonants.
    assert_eq!(
        tips(&[("Net label", None), ("New sheet", None)]),
        ["N", "S"].map(str::to_owned).to_vec()
    );
    // Two letters once every letter of the title is taken, never starting
    // with a one-letter tip.
    assert_eq!(
        tips(&[("Box", Some('B')), ("Bob", None)]),
        ["B", "OB"].map(str::to_owned).to_vec()
    );
}

#[test]
fn a_letter_key_is_a_tip_and_others_are_not() {
    use crate::commands::Chord;
    use egui::Key as K;
    assert_eq!(single_letter(Chord::key(K::R)), Some('R'));
    assert_eq!(single_letter(Chord::key(K::R).with_shift()), None);
    assert_eq!(single_letter(Chord::cmd(K::R)), None);
    assert_eq!(single_letter(Chord::key(K::Num1)), None);
    assert_eq!(single_letter(Chord::key(K::Slash)), None);
}
