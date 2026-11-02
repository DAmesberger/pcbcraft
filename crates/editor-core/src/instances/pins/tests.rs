//! C2 behaviour through public commands and queries: pin configuration,
//! peripheral assignment, `functions.suggest`, `project.pin_report` and
//! the carry of pin configuration through updates. The fixture is an
//! original synthetic part ("EXQ-5"); every expected value is derived by
//! hand from it.
use crate::functions::pin_config::{PinConfigEdit, declaration_problems};
use crate::functions::suggest::{PeripheralRequest, SuggestReport, SuggestRequest};
use crate::functions::{
    Direction, Drive, Function, FunctionConstraint, GpioRef, Level, MuxCode, PinConfig, PinOption,
    Pull, Resolution, Resource, ResourceRole, Role, SignalKind, TerminalFunctions,
};
use crate::instances::pins::{AssignPeripheral, RolePlacement};
use crate::library::TerminalClass;
use crate::pins::PinReport;
use crate::tests::support::*;
use crate::updates::{Change, Resolutions};
use crate::{
    ComponentRef, Engine, Error, FunctionId, FunctionKey, Id, InstanceCommand, Operation, Outcome,
    Query, QueryResult, ResourceId, Result, RoleId, TerminalId,
};

const PA0: u32 = 1;
const PA1: u32 = 2;
const PB0: u32 = 3;
const PB1: u32 = 4;
const VDD: u32 = 5;

const UART1: u32 = 50;
const TX: u32 = 51;
const RX: u32 = 52;
const I2C1: u32 = 53;
const SCL: u32 = 54;
const SDA: u32 = 55;

// Function IDs.
const PA0_IN: u32 = 100;
const PA0_OUT: u32 = 101;
const PA0_TX: u32 = 102;
const PA1_IN: u32 = 103;
const PA1_RX: u32 = 104;
const PB0_IN: u32 = 105;
const PB0_TX: u32 = 106;
const PB0_SCL: u32 = 107;
const PB1_IN: u32 = 108;
const PB1_RX: u32 = 109;
const PB1_SDA: u32 = 110;

fn f(id: u32, label: &str, direction: Direction, role: Option<(u32, u32)>, mux: &str) -> Function {
    let drive = if matches!(direction, Direction::Output | Direction::Bidirectional) {
        if label.starts_with("i2c") {
            Drive::OpenDrain
        } else {
            Drive::PushPull
        }
    } else {
        Drive::None
    };
    Function {
        id: FunctionId(id),
        label: label.into(),
        direction,
        signal: SignalKind::Digital,
        drive,
        resource_role: role.map(|(resource, role)| ResourceRole {
            resource: ResourceId(resource),
            role: RoleId(role),
        }),
        mux: (!mux.is_empty()).then(|| MuxCode(mux.into())),
        mux_table: None,
        source: None,
    }
}

fn io(
    functions: Vec<Function>,
    default: u32,
    port: &str,
    index: u16,
    options: Vec<PinOption>,
) -> TerminalClass {
    TerminalClass::Electrical(TerminalFunctions {
        functions,
        design_default: Some(FunctionId(default)),
        gpio: Some(GpioRef {
            port: port.into(),
            index,
        }),
        options,
    })
}

fn role(id: u32, label: &str, signal: &str) -> Role {
    Role {
        id: RoleId(id),
        label: label.into(),
        capacity: 1,
        required_when_used: true,
        signal: Some(signal.into()),
    }
}

fn exq_terminals() -> Vec<(u32, &'static str, TerminalClass)> {
    use Direction::{Bidirectional as Bi, Input, Output};
    vec![
        (
            PA0,
            "PA0",
            io(
                vec![
                    f(PA0_IN, "gpio_in", Input, None, ""),
                    f(PA0_OUT, "gpio_out", Output, None, ""),
                    f(PA0_TX, "usart1_tx", Output, Some((UART1, TX)), "AF7"),
                ],
                PA0_IN,
                "A",
                0,
                vec![
                    PinOption::Pull(vec![Pull::None, Pull::Up, Pull::Down]),
                    PinOption::Speed(vec!["low".into(), "high".into()]),
                    PinOption::InitialLevel,
                ],
            ),
        ),
        (
            PA1,
            "PA1",
            io(
                vec![
                    f(PA1_IN, "gpio_in", Input, None, ""),
                    f(PA1_RX, "usart1_rx", Input, Some((UART1, RX)), "AF7"),
                ],
                PA1_IN,
                "A",
                1,
                vec![],
            ),
        ),
        (
            PB0,
            "PB0",
            io(
                vec![
                    f(PB0_IN, "gpio_in", Input, None, ""),
                    f(PB0_TX, "usart1_tx", Output, Some((UART1, TX)), "AF2"),
                    f(PB0_SCL, "i2c1_scl", Bi, Some((I2C1, SCL)), "AF4"),
                ],
                PB0_IN,
                "B",
                0,
                vec![PinOption::Pull(vec![Pull::Up])],
            ),
        ),
        (
            PB1,
            "PB1",
            io(
                vec![
                    f(PB1_IN, "gpio_in", Input, None, ""),
                    f(PB1_RX, "usart1_rx", Input, Some((UART1, RX)), "AF2"),
                    f(PB1_SDA, "i2c1_sda", Bi, Some((I2C1, SDA)), "AF4"),
                ],
                PB1_IN,
                "B",
                1,
                vec![PinOption::Pull(vec![Pull::Up])],
            ),
        ),
        (VDD, "VDD", power_in()),
    ]
}

/// The EXQ-5 drafts: resources, optional constraints. Not published.
fn exq_draft(engine: &mut Engine, constraints: Vec<FunctionConstraint>) -> Result<Fixture> {
    let fixture = draft(
        engine,
        &Spec {
            name: "EXQ-5",
            terminals: exq_terminals(),
            units: vec![(
                1,
                "A",
                (1..=5u32)
                    .map(|t| (t, [-7.5, 5.0 - 2.5 * f64::from(t)]))
                    .collect(),
            )],
            package: vec![(1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5")],
            mapping: vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)],
            physical: true,
        },
    )?;
    with(engine, fixture.component, component_body, |c| {
        c.next_local = c.next_local.max(200);
        c.resources = vec![
            Resource {
                id: ResourceId(UART1),
                label: "USART1".into(),
                kind: Some("uart".into()),
                roles: vec![role(TX, "tx", "TX"), role(RX, "rx", "RX")],
            },
            Resource {
                id: ResourceId(I2C1),
                label: "I2C1".into(),
                kind: Some("i2c".into()),
                roles: vec![role(SCL, "scl", "SCL"), role(SDA, "sda", "SDA")],
            },
        ];
        c.constraints = constraints;
    })?;
    Ok(fixture)
}

fn exq(engine: &mut Engine, constraints: Vec<FunctionConstraint>) -> Result<(Fixture, Id)> {
    let fixture = exq_draft(engine, constraints)?;
    publish(engine, fixture.all())?;
    let (u1, _) = place(engine, &fixture, "U1", 1, [0.0, 0.0])?;
    Ok((fixture, u1))
}

fn key(terminal: u32, function: u32) -> FunctionKey {
    FunctionKey {
        terminal: TerminalId(terminal),
        function: FunctionId(function),
    }
}

fn uart(roles: Option<Vec<u32>>) -> PeripheralRequest {
    PeripheralRequest {
        peripheral: ResourceId(UART1),
        roles: roles.map(|r| r.into_iter().map(RoleId).collect()),
    }
}

fn suggest(engine: &Engine, instance: Id, request: SuggestRequest) -> Result<SuggestReport> {
    match engine.query(Query::SuggestPins { instance, request })? {
        QueryResult::SuggestPins(report) => Ok(report),
        other => Err(Error::Invalid(format!("unexpected result {other:?}"))),
    }
}

fn request(enable: Vec<PeripheralRequest>) -> SuggestRequest {
    SuggestRequest {
        enable,
        keep_existing: false,
        avoid: vec![],
        limit: None,
    }
}

/// Each suggestion as its (terminal, function) edits.
fn pairs(report: &SuggestReport) -> Vec<Vec<(u32, u32)>> {
    report
        .suggestions
        .iter()
        .map(|s| {
            s.edits
                .iter()
                .map(|e| (e.terminal.0, e.function.map_or(0, |f| f.0)))
                .collect()
        })
        .collect()
}

fn active(resolution: &Resolution, terminal: u32) -> Option<u32> {
    resolution
        .active(TerminalId(terminal))
        .map(|a| a.function.0)
}

fn set_config(engine: &mut Engine, instance: Id, edits: Vec<PinConfigEdit>) -> Result<Outcome> {
    apply(
        engine,
        Operation::Instance(InstanceCommand::SetPinConfig { instance, edits }),
    )
}

fn assign(engine: &mut Engine, request: AssignPeripheral) -> Result<Resolution> {
    match apply(
        engine,
        Operation::Instance(InstanceCommand::AssignPeripheral(request)),
    )? {
        Outcome::FunctionsSelected { resolution } => Ok(resolution),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

fn edit(terminal: u32, config: Option<PinConfig>) -> PinConfigEdit {
    PinConfigEdit {
        terminal: TerminalId(terminal),
        config,
    }
}

#[test]
fn suggest_ranks_complete_assignments_in_declared_order() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    let mut all = request(vec![uart(None)]);
    all.limit = Some(8);
    let report = suggest(&engine, u1, all)?;
    // Both required roles; every TX × RX pair on distinct terminals, all
    // of equal cost, in declaration order.
    assert_eq!(
        pairs(&report),
        [
            vec![(PA0, PA0_TX), (PA1, PA1_RX)],
            vec![(PA0, PA0_TX), (PB1, PB1_RX)],
            vec![(PA1, PA1_RX), (PB0, PB0_TX)],
            vec![(PB0, PB0_TX), (PB1, PB1_RX)],
        ]
    );
    assert!(report.blockers.is_empty() && !report.exhausted);
    assert_eq!(
        report.suggestions[0].changed,
        [TerminalId(PA0), TerminalId(PA1)]
    );
    // Deterministic: the same query gives the same report.
    let mut again = request(vec![uart(None)]);
    again.limit = Some(8);
    assert_eq!(suggest(&engine, u1, again)?, report);

    // With I2C1 too, only PA0/PA1 remain for the UART.
    let both = suggest(
        &engine,
        u1,
        request(vec![
            uart(None),
            PeripheralRequest {
                peripheral: ResourceId(I2C1),
                roles: None,
            },
        ]),
    )?;
    assert_eq!(
        pairs(&both),
        [vec![
            (PA0, PA0_TX),
            (PA1, PA1_RX),
            (PB0, PB0_SCL),
            (PB1, PB1_SDA)
        ]]
    );

    // Avoiding PA0 moves TX to PB0; a subset of roles places only those.
    let mut avoid = request(vec![uart(None)]);
    avoid.avoid = vec![TerminalId(PA0)];
    assert_eq!(
        pairs(&suggest(&engine, u1, avoid)?),
        [vec![(PA1, PA1_RX), (PB0, PB0_TX)]]
    );
    assert_eq!(
        pairs(&suggest(&engine, u1, request(vec![uart(Some(vec![RX]))]))?),
        [vec![(PA1, PA1_RX)]]
    );
    // The query changes nothing.
    assert!(engine.project().instances[0].functions.is_empty());
    Ok(())
}

#[test]
fn suggest_keeps_existing_selections_and_prefers_unconnected_terminals() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    // PA0 is explicitly a GPIO output.
    apply(
        &mut engine,
        Operation::Instance(InstanceCommand::SelectFunctions {
            instance: u1,
            edits: vec![crate::functions::FunctionEdit {
                terminal: TerminalId(PA0),
                function: Some(FunctionId(PA0_OUT)),
            }],
        }),
    )?;
    let mut keep = request(vec![uart(None)]);
    keep.keep_existing = true;
    assert_eq!(
        pairs(&suggest(&engine, u1, keep)?),
        [vec![(PA1, PA1_RX), (PB0, PB0_TX)]]
    );
    // Without keep_existing, overriding PA0's explicit selection is
    // possible but ranked last.
    let mut free = request(vec![uart(Some(vec![TX]))]);
    free.limit = Some(8);
    assert_eq!(
        pairs(&suggest(&engine, u1, free)?),
        [vec![(PB0, PB0_TX)], vec![(PA0, PA0_TX)]]
    );

    // A terminal on a net with another endpoint is ranked after a free one.
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    let probe_fixture = component(&mut engine, &probe("Probe", input()))?;
    let (p1, _) = place(&mut engine, &probe_fixture, "P1", 1, [30.0, 0.0])?;
    connect(&mut engine, ep(u1, PA1), ep(p1, 1))?;
    let mut rx = request(vec![uart(Some(vec![RX]))]);
    rx.limit = Some(8);
    assert_eq!(
        pairs(&suggest(&engine, u1, rx)?),
        [vec![(PB1, PB1_RX)], vec![(PA1, PA1_RX)]]
    );
    Ok(())
}

#[test]
fn suggest_reports_what_blocks_a_peripheral() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(
        &mut engine,
        vec![FunctionConstraint::Excludes {
            a: key(PA0, PA0_TX),
            b: key(PA1, PA1_RX),
        }],
    )?;
    let mut blocked = request(vec![uart(None)]);
    blocked.avoid = vec![TerminalId(PB0), TerminalId(PB1)];
    let report = suggest(&engine, u1, blocked)?;
    assert!(report.suggestions.is_empty());
    assert_eq!(report.blockers.len(), 1);
    let blocker = &report.blockers[0];
    assert_eq!(blocker.peripheral, ResourceId(UART1));
    assert_eq!(blocker.role, Some(RoleId(RX)));
    assert_eq!(blocker.rule, None);
    assert!(
        blocker.message.starts_with(
            "‘USART1’ cannot be enabled: every candidate for role ‘rx’ is rejected: ‘usart1_tx’ on terminal ‘PA0’ and ‘usart1_rx’ on terminal ‘PA1’ cannot be active together."
        ),
        "{}",
        blocker.message
    );

    // No terminal left for TX at all.
    let mut none = request(vec![uart(None)]);
    none.avoid = vec![TerminalId(PA0), TerminalId(PB0)];
    let report = suggest(&engine, u1, none)?;
    assert!(report.suggestions.is_empty());
    assert_eq!(report.blockers.len(), 1);
    assert_eq!(report.blockers[0].role, Some(RoleId(TX)));
    assert!(
        report.blockers[0]
            .message
            .contains("no terminal can take role ‘tx’")
    );

    // Malformed requests are invalid input.
    for bad in [
        request(vec![]),
        request(vec![uart(None), uart(None)]),
        request(vec![uart(Some(vec![]))]),
        request(vec![uart(Some(vec![SCL]))]),
        SuggestRequest {
            limit: Some(9),
            ..request(vec![uart(None)])
        },
        request(vec![PeripheralRequest {
            peripheral: ResourceId(999),
            roles: None,
        }]),
    ] {
        assert!(suggest(&engine, u1, bad).is_err());
    }
    Ok(())
}

#[test]
fn assign_peripheral_is_one_atomic_undo_step() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    let before = engine.to_file();
    // TX pinned to PB0 by the caller; RX placed by the solver.
    let resolution = assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![RolePlacement {
                role: RoleId(TX),
                terminal: Some(TerminalId(PB0)),
            }],
            enable: true,
        },
    )?;
    assert_eq!(active(&resolution, PB0), Some(PB0_TX));
    assert_eq!(active(&resolution, PA1), Some(PA1_RX));
    assert_eq!(active(&resolution, PA0), Some(PA0_IN));
    assert!(resolution.incomplete.is_empty() && resolution.conflicts.is_empty());
    let enabled = engine.to_file();
    engine.undo()?;
    assert_eq!(engine.to_file(), before, "one undo step");
    engine.redo()?;
    assert_eq!(engine.to_file(), enabled);

    // A terminal that cannot take the role fails and changes nothing.
    let revision = engine.revision();
    let refused = assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![RolePlacement {
                role: RoleId(RX),
                terminal: Some(TerminalId(PA0)),
            }],
            enable: true,
        },
    );
    assert!(refused.is_err());
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.to_file(), enabled);

    // Disabling clears every selection of the peripheral.
    let cleared = assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![],
            enable: false,
        },
    )?;
    assert_eq!(active(&cleared, PB0), Some(PB0_IN));
    assert_eq!(active(&cleared, PA1), Some(PA1_IN));
    assert!(engine.project().instances[0].functions.is_empty());
    // Nothing left to clear.
    let revision = engine.revision();
    assert!(
        assign(
            &mut engine,
            AssignPeripheral {
                instance: u1,
                peripheral: ResourceId(UART1),
                roles: vec![],
                enable: false,
            },
        )
        .is_err()
    );
    assert_eq!(engine.revision(), revision);

    // With I2C1 on PB0/PB1 and kept, the solver places the UART on PA0/PA1.
    assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(I2C1),
            roles: vec![],
            enable: true,
        },
    )?;
    let uart = assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![],
            enable: true,
        },
    )?;
    for (terminal, function) in [(PA0, PA0_TX), (PA1, PA1_RX), (PB0, PB0_SCL), (PB1, PB1_SDA)] {
        assert_eq!(active(&uart, terminal), Some(function));
    }
    Ok(())
}

#[test]
fn an_explicit_placement_moves_a_placed_role() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    let place = |role: u32, terminal: u32| RolePlacement {
        role: RoleId(role),
        terminal: Some(TerminalId(terminal)),
    };
    assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![place(TX, PB0), place(RX, PA1)],
            enable: true,
        },
    )?;
    let placed = engine.to_file();
    // Both roles move in one step; their earlier terminals return to
    // their defaults.
    let moved = assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![place(TX, PA0), place(RX, PB1)],
            enable: true,
        },
    )?;
    for (terminal, function) in [(PA0, PA0_TX), (PB1, PB1_RX), (PB0, PB0_IN), (PA1, PA1_IN)] {
        assert_eq!(active(&moved, terminal), Some(function), "{terminal}");
    }
    assert!(moved.conflicts.is_empty() && moved.incomplete.is_empty());
    engine.undo()?;
    assert_eq!(engine.to_file(), placed, "the move is one undo step");
    // Moving one role leaves the other where it is.
    let one = assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![place(RX, PB1)],
            enable: true,
        },
    )?;
    for (terminal, function) in [(PB0, PB0_TX), (PB1, PB1_RX), (PA1, PA1_IN)] {
        assert_eq!(active(&one, terminal), Some(function), "{terminal}");
    }
    Ok(())
}

#[test]
fn pin_config_is_validated_against_declared_options() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    let led = PinConfig {
        pull: Some(Pull::Up),
        speed: Some("high".into()),
        initial: Some(Level::High),
        schmitt: None,
        name: Some("LED_RED".into()),
    };
    set_config(&mut engine, u1, vec![edit(PA0, Some(led.clone()))])?;
    assert_eq!(
        engine.project().instances[0]
            .pin_config
            .get(&TerminalId(PA0)),
        Some(&led)
    );
    let saved = engine.to_file();
    let text = String::from_utf8(crate::format::encode(&saved)?)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(text.contains("\"pin_config\"") && text.contains("LED_RED"));
    let reopened = Engine::from_file(crate::format::decode(text.as_bytes())?)?;
    assert_eq!(
        reopened.project().instances[0]
            .pin_config
            .get(&TerminalId(PA0)),
        Some(&led)
    );

    let refusals = [
        // PA1 declares no options; a name alone is fine but pull is not.
        vec![edit(
            PA1,
            Some(PinConfig {
                pull: Some(Pull::Up),
                ..PinConfig::default()
            }),
        )],
        // PB0 offers only a pull-up.
        vec![edit(
            PB0,
            Some(PinConfig {
                pull: Some(Pull::Down),
                ..PinConfig::default()
            }),
        )],
        vec![edit(
            PA0,
            Some(PinConfig {
                speed: Some("very_high".into()),
                ..PinConfig::default()
            }),
        )],
        vec![edit(
            PB0,
            Some(PinConfig {
                schmitt: Some(true),
                ..PinConfig::default()
            }),
        )],
        vec![edit(
            PA0,
            Some(PinConfig {
                name: Some("1st led".into()),
                ..PinConfig::default()
            }),
        )],
        vec![edit(PA0, None), edit(PA0, None)],
        vec![edit(99, None)],
        vec![],
    ];
    for edits in refusals {
        let revision = engine.revision();
        assert!(set_config(&mut engine, u1, edits).is_err());
        assert_eq!(engine.revision(), revision);
        assert_eq!(engine.to_file(), saved);
    }
    let error = set_config(
        &mut engine,
        u1,
        vec![edit(
            PB0,
            Some(PinConfig {
                pull: Some(Pull::Down),
                ..PinConfig::default()
            }),
        )],
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(
        error.contains("U1 terminal ‘PB0’: pull ‘down’ is not offered (allowed: up)"),
        "{error}"
    );

    // A name needs no option; an empty config clears like `None`.
    set_config(
        &mut engine,
        u1,
        vec![
            edit(
                PA1,
                Some(PinConfig {
                    name: Some("UART_RX".into()),
                    ..PinConfig::default()
                }),
            ),
            edit(PA0, Some(PinConfig::default())),
        ],
    )?;
    let configs = &engine.project().instances[0].pin_config;
    assert_eq!(
        configs.keys().copied().collect::<Vec<_>>(),
        [TerminalId(PA1)]
    );
    engine.undo()?;
    assert_eq!(engine.to_file(), saved, "one undo step");
    Ok(())
}

#[test]
fn pin_report_lists_functions_nets_and_far_endpoints() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    let probe_fixture = component(&mut engine, &probe("Probe", input()))?;
    let (p1, _) = place(&mut engine, &probe_fixture, "P1", 1, [30.0, 0.0])?;
    connect(&mut engine, ep(u1, PA0), ep(p1, 1))?;
    assign(
        &mut engine,
        AssignPeripheral {
            instance: u1,
            peripheral: ResourceId(UART1),
            roles: vec![RolePlacement {
                role: RoleId(TX),
                terminal: Some(TerminalId(PA0)),
            }],
            enable: true,
        },
    )?;
    set_config(
        &mut engine,
        u1,
        vec![edit(
            PA0,
            Some(PinConfig {
                pull: Some(Pull::Up),
                ..PinConfig::default()
            }),
        )],
    )?;
    let report: PinReport = match engine.query(Query::PinReport { instance: u1 })? {
        QueryResult::PinReport(report) => *report,
        other => return Err(Error::Invalid(format!("unexpected result {other:?}"))),
    };
    assert_eq!(report.designator, "U1");
    let names: Vec<&str> = report.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["PA0", "PA1", "PB0", "PB1", "VDD"]);
    let pa0 = &report.pins[0];
    assert_eq!(pa0.pads, ["1"]);
    assert_eq!(
        pa0.gpio,
        Some(GpioRef {
            port: "A".into(),
            index: 0
        })
    );
    let function = pa0
        .function
        .as_ref()
        .ok_or(Error::Invalid("no function".into()))?;
    assert_eq!(function.label, "usart1_tx");
    assert_eq!(function.peripheral, Some(ResourceId(UART1)));
    assert_eq!(function.peripheral_label.as_deref(), Some("USART1"));
    assert_eq!(function.peripheral_kind.as_deref(), Some("uart"));
    assert_eq!(function.role_label.as_deref(), Some("tx"));
    assert_eq!(function.signal.as_deref(), Some("TX"));
    assert_eq!(function.mux.as_deref(), Some("AF7"));
    assert_eq!(pa0.config.as_ref().and_then(|c| c.pull), Some(Pull::Up));
    assert_eq!(pa0.net, Some(net_of(&engine, ep(u1, PA0))?));
    assert_eq!(pa0.endpoints.len(), 1);
    let far = &pa0.endpoints[0];
    assert_eq!(far.designator, "P1");
    assert_eq!(far.instance, p1);
    assert_eq!(far.terminal_name, "P");
    assert_eq!(far.component, "Probe");
    assert_eq!(far.mpn, None);
    let far_function = far
        .function
        .as_ref()
        .map(|f| (f.label.as_str(), f.peripheral));
    assert_eq!(far_function, Some(("input", None)));
    // PA1 is alone on its net; PB0 keeps its default; VDD has no GPIO.
    assert_eq!(report.pins[1].net, Some(net_of(&engine, ep(u1, PA1))?));
    assert!(report.pins[1].endpoints.is_empty());
    assert_eq!(
        report.pins[2].function.as_ref().map(|f| f.label.as_str()),
        Some("gpio_in")
    );
    assert_eq!(report.pins[4].gpio, None);
    Ok(())
}

#[test]
fn pin_config_is_carried_through_updates_and_reported_when_withdrawn() -> Result<()> {
    let mut engine = Engine::default();
    let (fixture, u1) = exq(&mut engine, vec![])?;
    set_config(
        &mut engine,
        u1,
        vec![
            edit(
                PA0,
                Some(PinConfig {
                    pull: Some(Pull::Down),
                    name: Some("BUTTON".into()),
                    ..PinConfig::default()
                }),
            ),
            edit(
                PB0,
                Some(PinConfig {
                    pull: Some(Pull::Up),
                    ..PinConfig::default()
                }),
            ),
        ],
    )?;
    assert!(
        inspect(&engine, u1)?
            .findings
            .iter()
            .all(|f| f.code != "config.pin_option")
    );
    // r2 withdraws PA0's pull-down; PB0 is unchanged.
    with(&mut engine, fixture.component, component_body, |c| {
        if let Some(TerminalClass::Electrical(t)) = c
            .terminals
            .iter_mut()
            .find(|t| t.id == TerminalId(PA0))
            .map(|t| &mut t.class)
        {
            t.options = vec![PinOption::Pull(vec![Pull::None, Pull::Up])];
        }
    })?;
    let r2 = publish(&mut engine, vec![fixture.component])?
        .into_iter()
        .next()
        .map(|r| ComponentRef::new(r.id, r.revision))
        .ok_or(Error::NotFound("r2".into()))?;
    let plan = engine.preview_update(Change::ComponentRevision {
        instances: vec![u1],
        target: r2,
        resolutions: Resolutions::default(),
    });
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    engine.apply_update(plan.revision, plan.change, false)?;
    let instance = &engine.project().instances[0];
    assert_eq!(instance.component, r2);
    assert_eq!(instance.pin_config.len(), 2, "kept by terminal ID");
    let findings: Vec<_> = inspect(&engine, u1)?
        .findings
        .into_iter()
        .filter(|f| f.code == "config.pin_option")
        .collect();
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].message,
        "U1 terminal ‘PA0’: its pin configuration is no longer valid: pull ‘down’ is not offered (allowed: none, up)."
    );
    assert_eq!(findings[0].blocks, None);
    assert_eq!(
        findings[0].objects,
        [crate::ObjectRef::Terminal {
            endpoint: ep(u1, PA0)
        }]
    );
    // Fixing it is an ordinary edit.
    set_config(
        &mut engine,
        u1,
        vec![edit(
            PA0,
            Some(PinConfig {
                pull: Some(Pull::Up),
                ..PinConfig::default()
            }),
        )],
    )?;
    assert!(
        inspect(&engine, u1)?
            .findings
            .iter()
            .all(|f| f.code != "config.pin_option")
    );
    Ok(())
}

#[test]
fn an_update_lists_the_pin_configurations_it_drops() -> Result<()> {
    let mut engine = Engine::default();
    let (fixture, u1) = exq(&mut engine, vec![])?;
    set_config(
        &mut engine,
        u1,
        vec![edit(
            PB0,
            Some(PinConfig {
                pull: Some(Pull::Up),
                name: Some("SENSE".into()),
                ..PinConfig::default()
            }),
        )],
    )?;
    // r2 declares PB0 not connected.
    with(&mut engine, fixture.component, component_body, |c| {
        if let Some(t) = c.terminals.iter_mut().find(|t| t.id == TerminalId(PB0)) {
            t.class = TerminalClass::DeviceNc;
        }
    })?;
    let r2 = publish(&mut engine, vec![fixture.component])?
        .into_iter()
        .next()
        .map(|r| ComponentRef::new(r.id, r.revision))
        .ok_or(Error::NotFound("r2".into()))?;
    let plan = engine.preview_update(Change::ComponentRevision {
        instances: vec![u1],
        target: r2,
        resolutions: Resolutions::default(),
    });
    assert!(
        plan.changes.iter().any(|c| c
            == "U1: the pin configuration of terminal 3 (pull up, name SENSE) is dropped; the new revision removes the terminal or declares it not connected."),
        "{:#?}",
        plan.changes
    );
    assert!(
        plan.items
            .iter()
            .any(|i| i.line == "U1: pin configuration of terminal ‘PB0’ is dropped"),
        "{:#?}",
        plan.items
    );
    Ok(())
}

#[test]
fn a_document_with_a_config_on_a_missing_terminal_is_refused() -> Result<()> {
    let mut engine = Engine::default();
    let (_, u1) = exq(&mut engine, vec![])?;
    set_config(
        &mut engine,
        u1,
        vec![edit(
            PA1,
            Some(PinConfig {
                name: Some("RX".into()),
                ..PinConfig::default()
            }),
        )],
    )?;
    // Terminal 2 → 77 in the saved document: no such terminal.
    let bytes = crate::format::encode(&engine.to_file())?;
    assert!(Engine::from_file(crate::format::decode(&bytes)?).is_ok());
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    let config = value
        .pointer_mut("/project/instances/0/pin_config")
        .and_then(|c| c.as_object_mut())
        .ok_or(Error::Invalid("pin_config missing".into()))?;
    let entry = config
        .remove("2")
        .ok_or(Error::Invalid("terminal 2".into()))?;
    config.insert("77".into(), entry);
    let bytes = serde_json::to_vec(&value).map_err(|e| Error::Invalid(e.to_string()))?;
    let reopened = crate::format::decode(&bytes).and_then(Engine::from_file);
    assert!(reopened.is_err());
    Ok(())
}

#[test]
fn declaration_problems_name_malformed_pin_data() {
    let mut terminals = exq_terminals();
    if let TerminalClass::Electrical(t) = &mut terminals[1].2 {
        t.gpio = Some(GpioRef {
            port: "A".into(),
            index: 0,
        });
        t.options = vec![PinOption::Pull(vec![]), PinOption::Pull(vec![Pull::Up])];
        t.functions[1].mux = Some(MuxCode("AF 7".into()));
    }
    let (built, _) = crate::tests::support::terminals(&terminals);
    let component = crate::library::ComponentDefinition {
        terminals: built,
        ..Default::default()
    };
    let resources = vec![Resource {
        id: ResourceId(UART1),
        label: "USART1".into(),
        kind: Some(" ".into()),
        roles: vec![role(TX, "tx", "TX")],
    }];
    let problems: Vec<(&str, u32)> = declaration_problems(&component.declarations(), &resources)
        .iter()
        .map(|p| (p.code, p.local))
        .collect::<Vec<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        problems,
        [
            ("function.mux", PA1_RX),
            ("function.gpio", PA1),
            ("function.pin_option", PA1),
            ("function.pin_option", PA1),
            ("resource.kind", UART1),
        ]
    );
    assert!(
        declaration_problems(
            &crate::library::ComponentDefinition {
                terminals: crate::tests::support::terminals(&exq_terminals()).0,
                ..Default::default()
            }
            .declarations(),
            &[]
        )
        .is_empty()
    );
}

/// Publication refuses malformed pin data with the problem's code, and
/// changes nothing.
#[test]
fn publication_refuses_a_repeated_gpio() -> Result<()> {
    let mut engine = Engine::demo()?;
    let fixture = exq_draft(&mut engine, vec![])?;
    with(&mut engine, fixture.component, component_body, |c| {
        let gpio = c.terminals.iter().find_map(|t| match &t.class {
            TerminalClass::Electrical(f) if t.id.0 == PA0 => f.gpio.clone(),
            _ => None,
        });
        if let Some(TerminalClass::Electrical(f)) = c
            .terminals
            .iter_mut()
            .find(|t| t.id.0 == PA1)
            .map(|t| &mut t.class)
        {
            f.gpio = gpio;
        }
    })?;
    let before = engine.document().clone();
    let refused = publish(&mut engine, fixture.all());
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.contains("repeats GPIO")),
        "{refused:?}"
    );
    assert_eq!(engine.document(), &before);
    Ok(())
}
