//! Stackup edits through the public command interface: shrinking with
//! copper on a removed layer, renumbering when a middle pair goes, explicit
//! moves, swaps, malformed moves and the editing draft.
use super::*;
use crate::diagnostics::Error;
use crate::layout::fill::FillCommand;
use crate::layout::{LayoutCommand, RouteRequest};
use crate::library::{Outline, Segment};
use crate::model::fill::{FillSettings, FillSpec, KeepoutRules, KeepoutSpec};
use crate::{Engine, Operation, Outcome};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn apply(engine: &mut Engine, operation: Operation) -> crate::diagnostics::Result<Outcome> {
    engine.apply(engine.revision(), operation)
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Outline {
    Outline {
        start: [x0, y0],
        segments: vec![
            Segment::Line { to: [x1, y0] },
            Segment::Line { to: [x1, y1] },
            Segment::Line { to: [x0, y1] },
            Segment::Line { to: [x0, y0] },
        ],
    }
}

/// The demo board on eight layers with a track on Inner 3 and Inner 5, a
/// fill on Inner 6 and a keepout on Inner 5.
fn eight_layers() -> Fallible<Engine> {
    let mut engine = Engine::demo()?;
    let net = engine.project().nets[0].id;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetStackup(Stackup::preset(8)?)),
    )?;
    for (layer, y) in [(CopperLayer::Inner(3), 20.0), (CopperLayer::Inner(5), 22.0)] {
        apply(
            &mut engine,
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start: [20.0, y],
                    end: [30.0, y],
                    width: 0.3,
                    layer,
                },
            }),
        )?;
    }
    apply(
        &mut engine,
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                region: crate::model::fill::FillRegion::Drawn,
                name: String::new(),
                net: Some(net),
                layer: CopperLayer::Inner(6),
                outline: rect(5.0, 5.0, 15.0, 15.0),
                priority: 0,
                settings: FillSettings::default(),
            }),
        }),
    )?;
    apply(
        &mut engine,
        Operation::Fill(FillCommand::AddKeepout {
            keepout: Box::new(KeepoutSpec {
                name: "Keep".into(),
                outline: rect(40.0, 5.0, 45.0, 10.0),
                layers: KeepoutLayers::Only(vec![CopperLayer::Inner(5)]),
                forbid: KeepoutRules::default(),
            }),
        }),
    )?;
    Ok(engine)
}

fn track_layers(engine: &Engine) -> Vec<CopperLayer> {
    engine.project().tracks.iter().map(|t| t.layer).collect()
}

fn edit(engine: &mut Engine, edit: StackupEdit) -> crate::diagnostics::Result<Outcome> {
    apply(engine, Operation::Layout(LayoutCommand::EditStackup(edit)))
}

#[test]
fn shrinking_past_copper_is_refused_naming_the_layer_and_changes_nothing() -> Fallible<()> {
    let mut engine = eight_layers()?;
    let before = engine.project().clone();
    let refused = apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetStackup(Stackup::preset(6)?)),
    );
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m == "Inner 5 still carries 1 track, 1 keepout; move or delete it first"),
        "{refused:?}"
    );
    assert_eq!(engine.project(), &before);
    // The fill alone also refuses: nothing is dropped silently.
    let mut fill_only = eight_layers()?;
    let mut file = fill_only.to_file();
    file.project.tracks.clear();
    file.project.keepouts.clear();
    fill_only = Engine::from_file(file)?;
    let refused = apply(
        &mut fill_only,
        Operation::Layout(LayoutCommand::SetStackup(Stackup::preset(6)?)),
    );
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("Inner 6 still carries 1 fill")),
        "{refused:?}"
    );
    Ok(())
}

#[test]
fn removing_a_middle_pair_renumbers_the_copper_below_it_in_one_undo_step() -> Fallible<()> {
    let mut engine = eight_layers()?;
    let before = engine.project().clone();
    let mut draft = StackupDraft::of(&before.stackup);
    draft.remove_layer(1)?;
    draft.remove_layer(1)?;
    assert_eq!(draft.stackup.copper.len(), 6);
    draft.stackup.check()?;
    assert_eq!(
        draft.removed(),
        vec![CopperLayer::Inner(1), CopperLayer::Inner(2)]
    );
    edit(&mut engine, draft.edit(&BTreeMap::new()))?;
    assert_eq!(
        track_layers(&engine),
        vec![CopperLayer::Inner(1), CopperLayer::Inner(3)]
    );
    let project = engine.project();
    assert_eq!(project.fills[0].layer, CopperLayer::Inner(4));
    assert_eq!(
        project.keepouts[0].layers,
        KeepoutLayers::Only(vec![CopperLayer::Inner(3)])
    );
    assert_eq!(project.stackup.copper.len(), 6);
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn copper_on_a_removed_layer_moves_only_where_the_edit_says() -> Fallible<()> {
    let mut engine = eight_layers()?;
    let before = engine.project().clone();
    let mut draft = StackupDraft::of(&before.stackup);
    // Drop Inner 3 and Inner 4: Inner 3 carries a track.
    draft.remove_layer(3)?;
    draft.remove_layer(3)?;
    let refused = edit(&mut engine, draft.edit(&BTreeMap::new()));
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.starts_with("Inner 3 still carries 1 track")),
        "{refused:?}"
    );
    assert_eq!(engine.project(), &before);
    let dropped = BTreeMap::from([(CopperLayer::Inner(3), CopperLayer::Inner(2))]);
    edit(&mut engine, draft.edit(&dropped))?;
    // Inner 3's track to Inner 2; Inner 5 and 6 renamed Inner 3 and 4.
    assert_eq!(
        track_layers(&engine),
        vec![CopperLayer::Inner(2), CopperLayer::Inner(3)]
    );
    assert_eq!(engine.project().fills[0].layer, CopperLayer::Inner(4));
    Ok(())
}

#[test]
fn swapping_two_inner_layers_swaps_their_copper() -> Fallible<()> {
    let mut engine = eight_layers()?;
    let mut draft = StackupDraft::of(&engine.project().stackup);
    draft.stackup.copper[3].plane = true;
    draft.move_row(DraftRow::Foil(3), true)?;
    assert!(draft.stackup.copper[2].plane && !draft.stackup.copper[3].plane);
    assert!(draft.changed());
    edit(&mut engine, draft.edit(&BTreeMap::new()))?;
    assert_eq!(
        track_layers(&engine),
        vec![CopperLayer::Inner(2), CopperLayer::Inner(5)]
    );
    // An explicit two-way swap applies at once, not in sequence.
    let stackup = engine.project().stackup.clone();
    edit(
        &mut engine,
        StackupEdit {
            stackup,
            moves: vec![
                LayerMove {
                    from: CopperLayer::Inner(2),
                    to: CopperLayer::Inner(5),
                },
                LayerMove {
                    from: CopperLayer::Inner(5),
                    to: CopperLayer::Inner(2),
                },
            ],
            removed: Vec::new(),
        },
    )?;
    assert_eq!(
        track_layers(&engine),
        vec![CopperLayer::Inner(5), CopperLayer::Inner(2)]
    );
    Ok(())
}

#[test]
fn malformed_moves_are_refused_without_change() -> Fallible<()> {
    let mut engine = eight_layers()?;
    let before = engine.project().clone();
    let stackup = before.stackup.clone();
    let mv = |from, to| LayerMove { from, to };
    for (moves, words) in [
        (
            vec![mv(CopperLayer::Inner(7), CopperLayer::Top)],
            "no Inner 7 layer to move copper from",
        ),
        (
            vec![mv(CopperLayer::Top, CopperLayer::Inner(9))],
            "no Inner 9 layer to move copper to",
        ),
        (
            vec![
                mv(CopperLayer::Top, CopperLayer::Inner(1)),
                mv(CopperLayer::Top, CopperLayer::Inner(2)),
            ],
            "moves Top copper twice",
        ),
    ] {
        let refused = edit(
            &mut engine,
            StackupEdit {
                stackup: stackup.clone(),
                moves,
                removed: Vec::new(),
            },
        );
        assert!(
            matches!(&refused, Err(Error::Invalid(m)) if m.contains(words)),
            "{refused:?}"
        );
        assert_eq!(engine.project(), &before);
    }
    let too_many = vec![mv(CopperLayer::Top, CopperLayer::Top); MAX_COPPER_LAYERS + 1];
    assert!(
        edit(
            &mut engine,
            StackupEdit {
                stackup,
                moves: too_many,
                removed: Vec::new(),
            },
        )
        .is_err()
    );
    // An odd stackup is the rules' refusal.
    let mut odd = before.stackup.clone();
    odd.copper.pop();
    odd.dielectrics.pop();
    let refused = edit(
        &mut engine,
        StackupEdit {
            stackup: odd,
            moves: Vec::new(),
            removed: Vec::new(),
        },
    );
    assert!(matches!(&refused, Err(Error::Invalid(m)) if m.contains("even")));
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn the_draft_adds_layers_in_pairs_and_keeps_its_outer_foils() -> Fallible<()> {
    let mut draft = StackupDraft::of(&Stackup::preset(4)?);
    assert!(!draft.changed());
    draft.add_layer(&StackupPresets::fixture()?, DielectricKind::Prepreg)?;
    // Five foils: the rules say why it cannot be applied yet.
    assert!(draft.stackup.check().is_err());
    draft.add_layer(&StackupPresets::fixture()?, DielectricKind::Core)?;
    draft.stackup.check()?;
    use DielectricKind::{Core, Prepreg};
    let kinds: Vec<_> = draft.stackup.dielectrics.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, vec![Prepreg, Core, Prepreg, Core, Prepreg]);
    // Top, Inner 1 and Inner 2 keep their copper; Bottom stays Bottom.
    assert_eq!(draft.origin(0), Some(CopperLayer::Top));
    assert_eq!(draft.origin(3), None);
    assert_eq!(draft.origin(5), Some(CopperLayer::Bottom));
    assert!(draft.moves(&BTreeMap::new()).is_empty());
    draft.add_pair(&StackupPresets::fixture()?)?;
    assert_eq!(draft.stackup.copper.len(), 8);
    draft.stackup.check()?;
    assert!(draft.remove_layer(0).is_err());
    assert!(draft.remove_layer(7).is_err());
    assert!(draft.move_row(DraftRow::Foil(1), true).is_err());
    assert!(draft.move_row(DraftRow::Dielectric(0), true).is_err());
    draft.move_row(DraftRow::Dielectric(0), false)?;
    assert_eq!(draft.stackup.dielectrics[0].kind, Core);
    let mut full = StackupDraft::of(&Stackup::preset(MAX_COPPER_LAYERS)?);
    assert!(full.add_pair(&StackupPresets::fixture()?).is_err());
    assert_eq!(full.stackup.copper.len(), MAX_COPPER_LAYERS);
    // A preset keeps the outer layers' identity.
    let mut shrink = StackupDraft::of(&Stackup::preset(8)?);
    shrink.use_preset(&StackupPresets::fixture()?, 4)?;
    assert_eq!(
        shrink.removed(),
        vec![
            CopperLayer::Inner(3),
            CopperLayer::Inner(4),
            CopperLayer::Inner(5),
            CopperLayer::Inner(6)
        ]
    );
    assert_eq!(shrink.origin(3), Some(CopperLayer::Bottom));
    Ok(())
}
