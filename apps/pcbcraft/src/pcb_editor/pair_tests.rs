//! Pair routing and length tuning driven through the Layout canvas, as
//! the app does: a click on a pad of a differential pair starts both
//! halves, each click is one coupled step at the pair gap, Backspace
//! unwinds a step (giving back the tracks it extended), a layer change
//! places a via pair, a click on a pad of the pair fans in and finishes,
//! P routes a single net instead, and the Tune tool meanders a segment
//! to its pair partner's length.
use super::route_tests::{Canvas, TestResult, close, edits};
use super::*;
use pcbcraft_core::layout::length;
use pcbcraft_core::rules::signal::{DiffPairSpec, SignalRules};
use pcbcraft_core::rules::{NetClassSpec, RulesCommand};
use pcbcraft_core::{InstanceCommand, Outcome, Stackup, UnitId, schematic::SchematicCommand};

struct Usb {
    engine: Engine,
    nets: [Id; 2],
    /// `[positive, negative]` pad centres of R3 and R4.
    start: [Point; 2],
    end: [Point; 2],
}

fn pad(engine: &Engine, instance: Id, terminal: u32) -> Result<Point, Box<dyn std::error::Error>> {
    layout::pads(engine.project())
        .into_iter()
        .find(|p| p.terminal.instance == instance && p.terminal.terminal.0 == terminal)
        .map(|p| p.position)
        .ok_or_else(|| "pad".into())
}

fn apply(engine: &mut Engine, operation: Operation) -> Result<Outcome, Box<dyn std::error::Error>> {
    Ok(engine.apply(engine.revision(), operation)?)
}

/// The demo with R3 and R4 joined pin for pin as USB_D+ and USB_D-, a
/// 90 Ω pair with a 0.1 mm skew limit, turned so each part's two pads
/// sit side by side across the route, on the 4-layer preset.
fn usb() -> Result<Usb, Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let r1 = engine
        .project()
        .instance_by_designator("R1")
        .cloned()
        .ok_or("R1")?;
    let mut parts = Vec::new();
    for (designator, x) in [("R3", 0.0), ("R4", 30.0)] {
        apply(
            &mut engine,
            Operation::Instance(InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    sheet: None,
                    component: r1.component,
                    designator: Some(designator.into()),
                    properties: vec![],
                    representation: r1.representation,
                    unit: UnitId(1),
                    position: [x, 40.0],
                    physical: r1.physical,
                },
            )),
        )?;
        parts.push(
            engine
                .project()
                .instance_by_designator(designator)
                .map(|i| i.id)
                .ok_or("part")?,
        );
    }
    let [r3, r4] = parts[..] else {
        return Err("parts".into());
    };
    let ep = |instance, terminal| pcbcraft_core::Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    };
    let mut nets = Vec::new();
    for (terminal, name) in [(1, "USB_D+"), (2, "USB_D-")] {
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::Connect {
                from: ep(r3, terminal).into(),
                to: ep(r4, terminal).into(),
            }),
        )?;
        let net = engine
            .project()
            .net_of(ep(r3, terminal))
            .map(|n| n.id)
            .ok_or("net")?;
        apply(
            &mut engine,
            Operation::Schematic(SchematicCommand::RenameNet {
                net,
                name: name.into(),
            }),
        )?;
        nets.push(net);
    }
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetBoardSize { size: [60.0, 40.0] }),
    )?;
    apply(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    for (instance, x) in [(r3, 10.0), (r4, 40.0)] {
        apply(
            &mut engine,
            Operation::Layout(LayoutCommand::Move {
                instance,
                position: [x, 25.0],
                rotation_deg: 90.0,
                layer: Layer::Top,
            }),
        )?;
    }
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetStackup(Stackup::preset(4)?)),
    )?;
    let Outcome::NetClassAdded { class } = apply(
        &mut engine,
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "USB".into(),
                signal: SignalRules {
                    diff_impedance: Some(90.0),
                    pair_max_skew: Some(0.1),
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
    )?
    else {
        return Err("class".into());
    };
    apply(
        &mut engine,
        Operation::Rules(RulesCommand::AssignNets {
            nets: nets.clone(),
            class: Some(class),
        }),
    )?;
    apply(
        &mut engine,
        Operation::Rules(RulesCommand::AddDiffPair {
            pair: DiffPairSpec {
                name: "USB_D".into(),
                positive: nets[0],
                negative: nets[1],
            },
        }),
    )?;
    let start = [pad(&engine, r3, 1)?, pad(&engine, r3, 2)?];
    let end = [pad(&engine, r4, 1)?, pad(&engine, r4, 2)?];
    Ok(Usb {
        engine,
        nets: [nets[0], nets[1]],
        start,
        end,
    })
}

fn key(
    canvas: &mut Canvas,
    editor: &mut PcbEditor,
    engine: &Engine,
    at: Point,
    key: egui::Key,
) -> Option<Operation> {
    canvas.key(editor, engine, at, key, egui::Modifiers::NONE)
}

/// The edge gap of every parallel pair of positive and negative tracks
/// on one layer that overlap along their run.
fn coupled_gaps(engine: &Engine, nets: [Id; 2]) -> Vec<f64> {
    let project = engine.project();
    let Some(pair) = project.rules.pair_of(nets[0]).cloned() else {
        return vec![];
    };
    length::coupling(project, &pair)
        .coupled_runs
        .iter()
        .map(|(_, _, _, _, gap)| *gap)
        .collect()
}

#[test]
fn a_pad_of_a_pair_routes_both_halves_coupled_to_the_far_pads() -> TestResult {
    let Usb {
        mut engine,
        nets,
        start,
        end,
    } = usb()?;
    let (_, gap) = layout::impedance::pair_width_gap(engine.project(), nets[0], CopperLayer::Top);
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    editor.on_tool("route", &engine);
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    // Either half's pad starts the pair: here the negative one.
    assert!(canvas.click(&mut editor, &engine, start[1]).is_none());
    let pair = editor.route.pair.clone().ok_or("no pair route")?;
    assert_eq!(pair.nets, nets);
    assert!(close(pair.head.ends[0], start[0]) && close(pair.head.ends[1], start[1]));
    let centre = [
        0.5 * (start[0][0] + start[1][0]),
        0.5 * (start[0][1] + start[1][1]),
    ];
    // Step one: straight on, both halves at the gap.
    let first = canvas.click(&mut editor, &engine, [centre[0] + 10.0, centre[1]]);
    let added = &edits(&first)?.add;
    assert!(added.iter().any(|r| r.net == nets[0]) && added.iter().any(|r| r.net == nets[1]));
    canvas.apply(&mut editor, &mut engine, first)?;
    assert_eq!(editor.route.steps.len(), 1);
    let gaps = coupled_gaps(&engine, nets);
    assert!(
        !gaps.is_empty() && gaps.iter().all(|g| (g - gap).abs() < 1e-6),
        "{gaps:?} {gap}"
    );
    let after_first = engine.project().tracks.clone();
    // Step two turns 45°: it extends step one's last tracks to the mitre.
    let second = canvas.click(&mut editor, &engine, [centre[0] + 13.0, centre[1] + 3.0]);
    assert_eq!(edits(&second)?.replace.len(), 2, "{:?}", edits(&second)?);
    canvas.apply(&mut editor, &mut engine, second)?;
    assert!(
        coupled_gaps(&engine, nets)
            .iter()
            .all(|g| (g - gap).abs() < 1e-6)
    );
    // Backspace takes step two back, extended tracks included.
    let unwind = key(
        &mut canvas,
        &mut editor,
        &engine,
        [30.0, 30.0],
        egui::Key::Backspace,
    );
    assert_eq!(edits(&unwind)?.replace.len(), 2);
    canvas.apply(&mut editor, &mut engine, unwind)?;
    assert_eq!(engine.project().tracks, after_first);
    assert_eq!(editor.route.steps.len(), 1);
    // V and a click: a via pair, then on along Bottom.
    let via_at = [centre[0] + 14.0, centre[1]];
    key(&mut canvas, &mut editor, &engine, via_at, egui::Key::V);
    assert_eq!(editor.layer, CopperLayer::Inner(1));
    key(&mut canvas, &mut editor, &engine, via_at, egui::Key::Num4);
    assert_eq!(editor.layer, CopperLayer::Bottom);
    let through = canvas.click(&mut editor, &engine, [centre[0] + 20.0, centre[1]]);
    let vias = &edits(&through)?.vias;
    assert_eq!(vias.len(), 2);
    assert_eq!((vias[0].net, vias[1].net), (nets[0], nets[1]));
    canvas.apply(&mut editor, &mut engine, through)?;
    assert_eq!(engine.project().vias.len(), 2);
    // Back to Top through another via pair, then onto R4's pads.
    let back_at = [centre[0] + 24.0, centre[1]];
    key(&mut canvas, &mut editor, &engine, back_at, egui::Key::Num1);
    let up = canvas.click(&mut editor, &engine, [centre[0] + 26.0, centre[1]]);
    canvas.apply(&mut editor, &mut engine, up)?;
    let finish = canvas.click(&mut editor, &engine, end[0]);
    let ends: Vec<Point> = edits(&finish)?
        .add
        .iter()
        .filter_map(|r| {
            [r.end, r.start]
                .into_iter()
                .find(|p| close(*p, end[0]) || close(*p, end[1]))
        })
        .collect();
    assert!(ends.len() >= 2, "fans into both pads: {ends:?}");
    canvas.apply(&mut editor, &mut engine, finish)?;
    assert!(
        editor.route_start.is_none() && editor.route.pair.is_none(),
        "finished"
    );
    let open: Vec<_> = layout::airwires(engine.project())
        .into_iter()
        .filter(|(a, _)| nets.contains(&a.net))
        .collect();
    assert!(open.is_empty(), "both halves join R3 to R4: {open:?}");
    // No clearance or pair gap finding on the routed pair.
    let codes: Vec<String> = layout::checks(engine.project())
        .into_iter()
        .filter(|f| matches!(f.code.as_str(), "board.clearance" | "pair.gap"))
        .map(|f| f.message)
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
    Ok(())
}

#[test]
fn p_routes_one_net_of_a_pair() -> TestResult {
    let Usb {
        engine,
        nets,
        start,
        ..
    } = usb()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    editor.on_tool("route", &engine);
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    key(&mut canvas, &mut editor, &engine, start[0], egui::Key::P);
    assert!(editor.route.single);
    assert!(canvas.click(&mut editor, &engine, start[0]).is_none());
    assert!(editor.route.pair.is_none());
    assert_eq!(editor.net, Some(nets[0]));
    Ok(())
}

/// The Tune tool previews and commits the meander that brings the
/// shorter half to its partner's length.
#[test]
fn the_tune_tool_meanders_to_the_partner_length() -> TestResult {
    let Usb {
        mut engine,
        nets,
        start,
        end,
    } = usb()?;
    let centre = |p: [Point; 2]| [0.5 * (p[0][0] + p[1][0]), 0.5 * (p[0][1] + p[1][1])];
    let pair = engine
        .project()
        .rules
        .pair_of(nets[0])
        .map(|p| p.id)
        .ok_or("pair")?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::RoutePair {
            request: layout::pair::PairRouteRequest {
                pair,
                start,
                legs: vec![layout::pair::PairLeg {
                    layer: CopperLayer::Top,
                    points: vec![centre(start), centre(end)],
                }],
                end: Some(end),
                width: None,
                gap: None,
            },
        }),
    )?;
    // Lengthen the negative half by 0.6 mm: now the positive is short.
    let negative = engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == nets[1])
        .max_by(|a, b| {
            let l = |t: &pcbcraft_core::Track| pcbcraft_core::geometry::distance(t.start, t.end);
            l(a).total_cmp(&l(b))
        })
        .map(|t| t.id)
        .ok_or("track")?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::TuneLength {
            request: layout::meander::TuneRequest {
                track: negative,
                amount: layout::meander::TuneAmount::Add { mm: 0.6 },
                style: Default::default(),
                amplitude: Some(1.0),
                spacing: None,
                side: None,
            },
        }),
    )?;
    let positive = engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == nets[0])
        .max_by(|a, b| {
            let l = |t: &pcbcraft_core::Track| pcbcraft_core::geometry::distance(t.start, t.end);
            l(a).total_cmp(&l(b))
        })
        .cloned()
        .ok_or("track")?;
    let middle = [
        0.5 * (positive.start[0] + positive.end[0]),
        0.5 * (positive.start[1] + positive.end[1]),
    ];
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    editor.on_tool("tune", &engine);
    assert!(editor.tool == Tool::Tune);
    assert_eq!(editor.shortcut_context(), crate::commands::Context::PcbTune);
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    let tune = canvas.click(&mut editor, &engine, middle);
    let Some(Operation::Layout(LayoutCommand::TuneLength { request })) = &tune else {
        return Err(format!("expected layout.tune_length, got {tune:?}").into());
    };
    assert_eq!(request.track, positive.id);
    canvas.apply(&mut editor, &mut engine, tune)?;
    let lengths = length::net_lengths(engine.project());
    let (p, n) = (
        lengths.get(&nets[0]).map_or(0.0, |l| l.total),
        lengths.get(&nets[1]).map_or(0.0, |l| l.total),
    );
    assert!((p - n).abs() < 1e-5, "{p} {n}");
    assert!(
        !layout::checks(engine.project())
            .iter()
            .any(|f| f.code == "pair.skew" || f.code == "board.clearance")
    );
    Ok(())
}

/// Every text a frame of the subject slot paints.
fn card_texts(editor: &mut PcbEditor, engine: &Engine, ctx: &egui::Context) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    // Two frames: sections lay out on the first.
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 2400.0))),
                ..Default::default()
            },
            |ui| {
                editor.subject_slot(ui, engine);
            },
        );
        // No renderer applies the font atlas; egui rejects dropping it.
        output.textures_delta.clear();
        out.clear();
        for shape in &output.shapes {
            walk(&shape.shape, &mut out);
        }
    }
    out
}

/// The Net classes card shows the class's derived pair width, gap and
/// impedance per layer; the High speed card lists the pair and the
/// lengths; the Route card shows the pair and its impedance.
#[test]
fn the_cards_show_derived_impedance_pairs_and_lengths() -> TestResult {
    let Usb { engine, start, .. } = usb()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let class = engine.project().rules.net_classes.first().map(|c| c.id);
    editor.open_net_classes(class);
    let texts = card_texts(&mut editor, &engine, &ctx);
    assert!(
        texts.iter().any(|t| t.eq_ignore_ascii_case("High speed")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("gap 0.2 mm → 90.0 Ω")),
        "{texts:?}"
    );
    editor.open_signals();
    let texts = card_texts(&mut editor, &engine, &ctx);
    for label in ["Differential pairs", "USB_D", "Lengths"] {
        assert!(
            texts.iter().any(|t| t.eq_ignore_ascii_case(label)),
            "{label}: {texts:?}"
        );
    }
    let mut editor = PcbEditor::default();
    let mut canvas = Canvas::new();
    editor.on_tool("route", &engine);
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    canvas.click(&mut editor, &engine, start[0]);
    let texts = card_texts(&mut editor, &engine, &ctx);
    assert!(texts.iter().any(|t| t.contains("pair 90.0 Ω")), "{texts:?}");
    Ok(())
}

/// The High speed card shows what a selected pair's lengths measure: its
/// path's pads and each stub on a line of its own.
#[test]
fn the_high_speed_card_lists_the_path_and_the_stubs() -> TestResult {
    let Usb {
        mut engine,
        nets,
        start,
        end,
    } = usb()?;
    let pair = engine.project().rules.diff_pairs.first().ok_or("pair")?.id;
    let middle = [
        0.25 * (start[0][0] + start[1][0] + end[0][0] + end[1][0]),
        0.25 * (start[0][1] + start[1][1] + end[0][1] + end[1][1]),
    ];
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::RoutePair {
            request: layout::pair::PairRouteRequest {
                pair,
                start,
                legs: vec![layout::pair::PairLeg {
                    layer: CopperLayer::Top,
                    points: vec![
                        [0.5 * (start[0][0] + start[1][0]), middle[1]],
                        [0.5 * (end[0][0] + end[1][0]), middle[1]],
                    ],
                }],
                end: Some(end),
                width: None,
                gap: None,
            },
        }),
    )?;
    let track = engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == nets[0])
        .max_by(|a, b| {
            pcbcraft_core::geometry::distance(a.start, a.end)
                .total_cmp(&pcbcraft_core::geometry::distance(b.start, b.end))
        })
        .cloned()
        .ok_or("track")?;
    let at = [
        0.5 * (track.start[0] + track.end[0]),
        0.5 * (track.start[1] + track.end[1]),
    ];
    let side = if at[1] > middle[1] { 3.0 } else { -3.0 };
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::Route {
            request: layout::RouteRequest {
                net: nets[0],
                start: at,
                end: [at[0], at[1] + side],
                width: 0.2,
                layer: CopperLayer::Top,
            },
        }),
    )?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.open_signals();
    if let Some(card) = &mut editor.signals {
        card.select_pair(pair);
    }
    let texts = card_texts(&mut editor, &engine, &ctx);
    for wanted in ["Between the farthest shared parts", "Set endpoints"] {
        assert!(texts.iter().any(|t| t == wanted), "{wanted}: {texts:?}");
    }
    assert!(
        texts.iter().any(|t| t.contains("R3.1 to R4.1")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("USB_D+ 3 mm to open")),
        "{texts:?}"
    );
    Ok(())
}
