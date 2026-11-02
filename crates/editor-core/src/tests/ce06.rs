//! CE-06: multi-function IO, shared resources, per-instance selections and
//! function-aware ERC, driven through public commands and queries only.
//! Expected values are hand-derived from the synthetic fixture below.
use super::support::*;
use crate::functions::{
    ActiveOrigin, Direction, Drive, Function, FunctionConstraint, FunctionEdit, Resolution,
    Resource, ResourceRole, Role, SignalKind, TerminalFunctions,
};
use crate::instances::NewInstance;
use crate::layout::LayoutCommand;
use crate::library::{
    BindingDefinition, ComponentDefinition, Definition, DraftEdit, FootprintDefinition,
    PackageDefinition, PackageTerminal, Pad, Pin, PinDirection, SymbolDefinition, Terminal,
    TerminalClass, TerminalMap, Unit,
};
use crate::updates::{Change, FunctionResolution, InstanceChange, Resolutions};
use crate::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, Engine, Error, FootprintRef,
    FunctionId, FunctionKey, Id, InstanceCommand, ObjectRef, Operation, Outcome, PackageRef,
    PackageTerminalId, PadId, PhysicalChoice, ResourceId, Result, Revision, RoleId, Stage,
    SymbolRef, TerminalId, UnitId, erc, format,
};
use serde_json::json;

// Function IDs of the fixture.
const GPIO_IN_1: u32 = 40;
const GPIO_OUT_1: u32 = 41;
const UART_TX: u32 = 42;
const ADC_IN: u32 = 43;
const UART_TX_ALT: u32 = 46;
const UART_RX: u32 = 49;
const BUS_OD: u32 = 50;

const UART0: u32 = 30;
const TX: u32 = 31;
const RX: u32 = 32;
const ADC0: u32 = 33;
const CH0: u32 = 34;
const BUS0: u32 = 35;
const SDA: u32 = 36;

const DIGITAL: [u32; 8] = [1, 2, 3, 5, 6, 7, 9, 10];
const ANALOG: [u32; 8] = [11, 13, 14, 15, 17, 18, 19, 20];
const POWER: [u32; 5] = [4, 8, 12, 16, 21];

fn function(
    id: u32,
    label: &str,
    direction: Direction,
    signal: SignalKind,
    drive: Drive,
    role: Option<(u32, u32)>,
) -> Function {
    Function {
        id: FunctionId(id),
        label: label.into(),
        direction,
        signal,
        drive,
        resource_role: role.map(|(resource, role)| ResourceRole {
            resource: ResourceId(resource),
            role: RoleId(role),
        }),
        mux: None,
        mux_table: None,
        source: None,
    }
}

fn gpio_in(id: u32) -> Function {
    function(
        id,
        "gpio-in",
        Direction::Input,
        SignalKind::Digital,
        Drive::None,
        None,
    )
}
fn gpio_out(id: u32) -> Function {
    function(
        id,
        "gpio-out",
        Direction::Output,
        SignalKind::Digital,
        Drive::PushPull,
        None,
    )
}

fn electrical(functions: Vec<Function>, default: Option<u32>) -> TerminalClass {
    TerminalClass::Electrical(TerminalFunctions {
        functions,
        design_default: default.map(FunctionId),
        gpio: None,
        options: Vec::new(),
    })
}

fn role(id: u32, label: &str, required_when_used: bool) -> Role {
    Role {
        id: RoleId(id),
        label: label.into(),
        capacity: 1,
        required_when_used,
        signal: None,
    }
}

fn key(terminal: u32, function: u32) -> FunctionKey {
    FunctionKey {
        terminal: TerminalId(terminal),
        function: FunctionId(function),
    }
}

/// The fixture component: terminals 1–21 in a digital, an analog and a power
/// unit; multi-function terminals 1–3; uart0, adc0 and bus0 resources; and
/// one exclusion between adc-in on 1 and bus-od on 3.
fn mcu_component(lineages: &Mcu) -> ComponentDefinition {
    let mut terminals = vec![
        (
            1,
            electrical(
                vec![
                    gpio_in(GPIO_IN_1),
                    gpio_out(GPIO_OUT_1),
                    function(
                        UART_TX,
                        "uart-tx",
                        Direction::Output,
                        SignalKind::Digital,
                        Drive::PushPull,
                        Some((UART0, TX)),
                    ),
                    function(
                        ADC_IN,
                        "adc-in",
                        Direction::Input,
                        SignalKind::Analog,
                        Drive::None,
                        Some((ADC0, CH0)),
                    ),
                ],
                None,
            ),
        ),
        (
            2,
            electrical(
                vec![
                    gpio_in(44),
                    gpio_out(45),
                    function(
                        UART_TX_ALT,
                        "uart-tx-alt",
                        Direction::Output,
                        SignalKind::Digital,
                        Drive::PushPull,
                        Some((UART0, TX)),
                    ),
                ],
                None,
            ),
        ),
        (
            3,
            electrical(
                vec![
                    gpio_in(47),
                    gpio_out(48),
                    function(
                        UART_RX,
                        "uart-rx",
                        Direction::Input,
                        SignalKind::Digital,
                        Drive::None,
                        Some((UART0, RX)),
                    ),
                    function(
                        BUS_OD,
                        "bus-od",
                        Direction::Bidirectional,
                        SignalKind::Digital,
                        Drive::OpenDrain,
                        Some((BUS0, SDA)),
                    ),
                ],
                None,
            ),
        ),
    ];
    for (i, t) in [5u32, 6, 7, 9, 10].into_iter().enumerate() {
        let base = 51 + 2 * i as u32;
        terminals.push((
            t,
            electrical(vec![gpio_in(base), gpio_out(base + 1)], Some(base)),
        ));
    }
    for (j, t) in ANALOG.into_iter().enumerate() {
        terminals.push((
            t,
            electrical(
                vec![function(
                    61 + j as u32,
                    "analog-in",
                    Direction::Input,
                    SignalKind::Analog,
                    Drive::None,
                    None,
                )],
                None,
            ),
        ));
    }
    for (k, t) in POWER.into_iter().enumerate() {
        let label = if matches!(t, 8 | 16) {
            "supply"
        } else {
            "ground"
        };
        terminals.push((
            t,
            electrical(
                vec![function(
                    69 + k as u32,
                    label,
                    Direction::PowerInput,
                    SignalKind::Power,
                    Drive::None,
                    None,
                )],
                None,
            ),
        ));
    }
    terminals.sort_by_key(|(t, _)| *t);
    ComponentDefinition {
        next_local: 74,
        designator_prefix: "U".into(),
        terminals: terminals
            .into_iter()
            .map(|(id, class)| Terminal {
                id: TerminalId(id),
                name: format!("P{id}"),
                class,
            })
            .collect(),
        resources: vec![
            Resource {
                id: ResourceId(UART0),
                label: "uart0".into(),
                kind: None,
                roles: vec![role(TX, "tx", true), role(RX, "rx", false)],
            },
            Resource {
                id: ResourceId(ADC0),
                label: "adc0".into(),
                kind: None,
                roles: vec![role(CH0, "ch0", false)],
            },
            Resource {
                id: ResourceId(BUS0),
                label: "bus0".into(),
                kind: None,
                roles: vec![role(SDA, "sda", false)],
            },
        ],
        constraints: vec![FunctionConstraint::Excludes {
            a: key(1, ADC_IN),
            b: key(3, BUS_OD),
        }],
        representations: vec![SymbolRef::new(lineages.symbol, Revision::FIRST)],
        implementations: vec![
            BindingRef::new(lineages.b1, Revision::FIRST),
            BindingRef::new(lineages.b2, Revision::FIRST),
        ],
        fields: vec![],
        pin_rules: Vec::new(),
        marking: None,
        review: None,
    }
}

/// The fixture's six lineages.
struct Mcu {
    component: DefinitionId,
    symbol: DefinitionId,
    package: DefinitionId,
    footprint: DefinitionId,
    b1: DefinitionId,
    b2: DefinitionId,
}

impl Mcu {
    fn choice(&self, binding: DefinitionId) -> PhysicalChoice {
        PhysicalChoice {
            binding,
            footprint: self.footprint,
        }
    }
    fn all(&self) -> Vec<DefinitionId> {
        vec![
            self.component,
            self.symbol,
            self.package,
            self.footprint,
            self.b1,
            self.b2,
        ]
    }
}

fn mcu_bodies(m: &Mcu) -> Vec<DraftEdit> {
    let first = Revision::FIRST;
    let unit = |id: u32, label: &str, terminals: &[u32]| Unit {
        id: UnitId(id),
        label: label.into(),
        graphics: vec![],
        pins: terminals
            .iter()
            .enumerate()
            .map(|(row, t)| {
                Pin::new(
                    TerminalId(*t),
                    [-7.5, 10.0 - 2.5 * row as f64],
                    PinDirection::Right,
                    2.5,
                )
            })
            .collect(),
    };
    let symbol = SymbolDefinition {
        component: m.component,
        next_local: 4,
        pin_text_height: 1.25,
        units: vec![
            unit(1, "D", &DIGITAL),
            unit(2, "A", &ANALOG),
            unit(3, "P", &POWER),
        ],
    };
    let package = PackageDefinition {
        next_local: 22,
        terminals: (1..=21)
            .map(|i| PackageTerminal {
                id: PackageTerminalId(i),
                designation: i.to_string(),
            })
            .collect(),
        envelope: [5.0, 5.0, 0.9],
        ..PackageDefinition::default()
    };
    // Perimeter lands 1–20; package terminal 21 (the exposed pad) has three
    // lands, pads 21–23.
    let mut pads: Vec<Pad> = (1..=20u32)
        .map(|i| {
            Pad::smd_rect(
                PadId(i),
                PackageTerminalId(i),
                i.to_string(),
                [
                    -4.5 + f64::from((i - 1) % 10),
                    if i <= 10 { -3.0 } else { 3.0 },
                ],
                [0.4, 0.6],
            )
        })
        .collect();
    for (n, y) in [(21u32, -1.0), (22, 0.0), (23, 1.0)] {
        pads.push(Pad::smd_rect(
            PadId(n),
            PackageTerminalId(21),
            "21",
            [0.0, y],
            [0.8, 0.6],
        ));
    }
    let footprint = FootprintDefinition {
        package: PackageRef::new(m.package, first),
        next_local: 24,
        pads,
        ..FootprintDefinition::default()
    };
    let binding = |unavailable: Vec<FunctionKey>| BindingDefinition {
        component: m.component,
        package: PackageRef::new(m.package, first),
        mappings: (1..=21)
            .map(|i| TerminalMap {
                package_terminal: PackageTerminalId(i),
                terminal: TerminalId(i),
            })
            .collect(),
        footprints: vec![FootprintRef::new(m.footprint, first)],
        unavailable,
    };
    let edit = |id: DefinitionId, name: &str, definition: Definition| DraftEdit {
        id,
        meta: meta(name),
        definition,
    };
    vec![
        edit(m.component, "MCU", Definition::Component(mcu_component(m))),
        edit(m.symbol, "MCU symbol", Definition::Symbol(symbol)),
        edit(m.package, "MCU package", Definition::Package(package)),
        edit(
            m.footprint,
            "MCU footprint",
            Definition::Footprint(footprint),
        ),
        edit(m.b1, "MCU B1", Definition::Binding(binding(vec![]))),
        edit(
            m.b2,
            "MCU B2",
            Definition::Binding(binding(vec![key(3, BUS_OD)])),
        ),
    ]
}

fn draft_mcu(engine: &mut Engine) -> Result<Mcu> {
    let m = Mcu {
        component: new_draft(engine, DefinitionKind::Component)?,
        symbol: new_draft(engine, DefinitionKind::Symbol)?,
        package: new_draft(engine, DefinitionKind::Package)?,
        footprint: new_draft(engine, DefinitionKind::Footprint)?,
        b1: new_draft(engine, DefinitionKind::Binding)?,
        b2: new_draft(engine, DefinitionKind::Binding)?,
    };
    edit(engine, mcu_bodies(&m))?;
    Ok(m)
}

fn place_mcu(engine: &mut Engine, m: &Mcu, designator: &str, y: f64) -> Result<Id> {
    match apply(
        engine,
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: ComponentRef::new(m.component, Revision::FIRST),
            designator: Some(designator.into()),
            properties: vec![],
            representation: m.symbol,
            unit: UnitId(1),
            position: [0.0, y],
            physical: Some(m.choice(m.b1)),
        })),
    )? {
        Outcome::InstancePlaced { instance, .. } => Ok(instance),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

/// `instance.select_functions` with (terminal, function) pairs.
fn choose(engine: &mut Engine, instance: Id, edits: &[(u32, Option<u32>)]) -> Result<Resolution> {
    let edits = edits
        .iter()
        .map(|(terminal, function)| FunctionEdit {
            terminal: TerminalId(*terminal),
            function: function.map(FunctionId),
        })
        .collect();
    match apply(
        engine,
        Operation::Instance(InstanceCommand::SelectFunctions { instance, edits }),
    )? {
        Outcome::FunctionsSelected { resolution } => Ok(resolution),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

fn active(resolution: &Resolution, terminal: u32) -> Option<(u32, ActiveOrigin)> {
    resolution
        .active(TerminalId(terminal))
        .map(|a| (a.function.0, a.origin))
}

/// ERC codes of findings on one net, sorted.
fn erc_on(engine: &Engine, net: Id) -> Vec<String> {
    let mut codes: Vec<String> = erc::checks(engine.project())
        .into_iter()
        .filter(|f| f.objects.contains(&ObjectRef::Net { id: net }))
        .map(|f| f.code)
        .collect();
    codes.sort();
    codes
}

fn terminal(instance: Id, t: u32) -> ObjectRef {
    ObjectRef::Terminal {
        endpoint: ep(instance, t),
    }
}

fn has_finding(engine: &Engine, instance: Id, code: &str, objects: &[ObjectRef]) -> Result<bool> {
    Ok(inspect(engine, instance)?
        .findings
        .iter()
        .any(|f| f.code == code && f.objects == objects))
}

fn open_drain() -> TerminalClass {
    electrical(
        vec![function(
            0,
            "od",
            Direction::Output,
            SignalKind::Digital,
            Drive::OpenDrain,
            None,
        )],
        None,
    )
}

#[test]
fn ce06_functions_are_selected_per_instance_and_checked_by_drive() -> Result<()> {
    let mut engine = Engine::default();
    let m = draft_mcu(&mut engine)?;
    publish(&mut engine, m.all())?;
    let input_probe = component(&mut engine, &probe("Input probe", input()))?;
    let push_pull = component(&mut engine, &probe("Push-pull probe", output()))?;
    let od_probe = component(&mut engine, &probe("Open-drain probe", open_drain()))?;
    let passive_probe = component(&mut engine, &probe("Passive probe", passive()))?;

    // U1 and U2 are independent instances on B1.
    let u1 = place_mcu(&mut engine, &m, "U1", 0.0)?;
    let u2 = place_mcu(&mut engine, &m, "U2", 60.0)?;
    let fresh = inspect(&engine, u2)?.functions;
    assert_eq!(
        fresh.unresolved,
        vec![TerminalId(1), TerminalId(2), TerminalId(3)]
    );
    assert_eq!(active(&fresh, 5), Some((51, ActiveOrigin::DesignDefault)));
    assert_eq!(active(&fresh, 11), Some((61, ActiveOrigin::Fixed)));

    // U1: uart-tx on 1, uart-rx on 3, in one batch.
    let resolution = choose(&mut engine, u1, &[(1, Some(UART_TX)), (3, Some(UART_RX))])?;
    assert!(resolution.conflicts.is_empty());
    assert!(resolution.incomplete.is_empty());
    assert_eq!(
        active(&resolution, 1),
        Some((UART_TX, ActiveOrigin::Selected))
    );
    assert_eq!(
        active(&resolution, 3),
        Some((UART_RX, ActiveOrigin::Selected))
    );
    assert_eq!(resolution.unresolved, vec![TerminalId(2)]);
    // Selections on U1 do not appear in U2's report.
    assert_eq!(inspect(&engine, u2)?.functions, fresh);

    // uart-tx (push-pull) driving an input: no ERC finding on that net.
    let (in1, _) = place(&mut engine, &input_probe, "IN1", 1, [30.0, 0.0])?;
    connect(&mut engine, ep(u1, 1), ep(in1, 1))?;
    let net1 = net_of(&engine, ep(u1, 1))?;
    assert_eq!(erc_on(&engine, net1), Vec::<String>::new());

    // gpio-in on 1: inputs only, and uart0 is used (rx on 3) without tx.
    choose(&mut engine, u1, &[(1, Some(GPIO_IN_1))])?;
    assert_eq!(erc_on(&engine, net1), ["erc.input_only_net"]);
    assert!(has_finding(
        &engine,
        u1,
        "config.requirement_unmet",
        &[ObjectRef::Instance { id: u1 }, terminal(u1, 3)],
    )?);
    let report = inspect(&engine, u1)?;
    assert!(
        report
            .findings
            .iter()
            .filter(|f| f.code.starts_with("config."))
            .all(|f| f.blocks_stage(Stage::ElectricallyResolved))
    );

    // gpio-out on 1: one push-pull driver and an input.
    choose(&mut engine, u1, &[(1, Some(GPIO_OUT_1))])?;
    assert_eq!(erc_on(&engine, net1), Vec::<String>::new());
    let (pp1, _) = place(&mut engine, &push_pull, "PP1", 1, [30.0, 10.0])?;
    connect(&mut engine, ep(pp1, 1), ep(u1, 1))?;
    let net1 = net_of(&engine, ep(u1, 1))?;
    assert_eq!(erc_on(&engine, net1), ["erc.driver_conflict"]);

    // bus-od on 3 with an open-drain probe and a passive pull-up: no finding.
    choose(&mut engine, u1, &[(3, Some(BUS_OD))])?;
    let (od1, _) = place(&mut engine, &od_probe, "OD1", 1, [30.0, 20.0])?;
    let (pull, _) = place(&mut engine, &passive_probe, "RP1", 1, [30.0, 30.0])?;
    connect(&mut engine, ep(u1, 3), ep(od1, 1))?;
    connect(&mut engine, ep(u1, 3), ep(pull, 1))?;
    let net3 = net_of(&engine, ep(u1, 3))?;
    assert_eq!(erc_on(&engine, net3), Vec::<String>::new());
    // A push-pull driver on that net is contention, not a driver conflict:
    // ERC uses the open-drain drive, not the direction alone.
    let (pp2, _) = place(&mut engine, &push_pull, "PP2", 1, [30.0, 40.0])?;
    connect(&mut engine, ep(pp2, 1), ep(u1, 3))?;
    let net3 = net_of(&engine, ep(u1, 3))?;
    assert_eq!(erc_on(&engine, net3), ["erc.open_drain_contention"]);
    choose(&mut engine, u1, &[(3, Some(UART_RX))])?;

    // U2 terminal 1 has no selection or default.
    let (p3, _) = place(&mut engine, &passive_probe, "RP2", 1, [30.0, 60.0])?;
    connect(&mut engine, ep(u2, 1), ep(p3, 1))?;
    let net_u2 = net_of(&engine, ep(u2, 1))?;
    assert!(has_finding(
        &engine,
        u2,
        "config.function_unresolved",
        &[terminal(u2, 1)],
    )?);
    let unchecked: Vec<_> = erc::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == "erc.unchecked_terminal")
        .map(|f| f.objects)
        .collect();
    assert_eq!(
        unchecked,
        vec![vec![ObjectRef::Net { id: net_u2 }, terminal(u2, 1)]]
    );

    // Both TX routes on U1 exceed tx capacity 1: rejected, nothing changes.
    let kept = Snapshot::of(&engine);
    let error = choose(
        &mut engine,
        u1,
        &[(1, Some(UART_TX)), (2, Some(UART_TX_ALT))],
    )
    .err();
    assert_eq!(error.as_ref().map(Error::code), Some("conflict"));
    assert!(error.is_some_and(|e| e.to_string().contains("capacity 1")));
    kept.assert_kept(&engine, "double TX");

    // One TX on U2 is fine.
    let resolution = choose(&mut engine, u2, &[(2, Some(UART_TX_ALT))])?;
    assert!(resolution.conflicts.is_empty());

    // Atomic move of U1's TX from 1 to 2.
    choose(&mut engine, u1, &[(1, Some(UART_TX))])?;
    let kept = Snapshot::of(&engine);
    // Edit by edit, the first step would exceed tx capacity.
    let single = choose(&mut engine, u1, &[(2, Some(UART_TX_ALT))]).err();
    assert_eq!(single.map(|e| e.code()), Some("conflict"));
    kept.assert_kept(&engine, "intermediate TX state");
    let before = engine.document().clone();
    let resolution = choose(
        &mut engine,
        u1,
        &[(2, Some(UART_TX_ALT)), (1, Some(GPIO_OUT_1))],
    )?;
    assert_eq!(engine.revision(), kept.revision() + 1);
    assert_eq!(
        active(&resolution, 2),
        Some((UART_TX_ALT, ActiveOrigin::Selected))
    );
    assert_eq!(
        active(&resolution, 1),
        Some((GPIO_OUT_1, ActiveOrigin::Selected))
    );
    let moved = engine.document().clone();
    engine.undo()?;
    assert_eq!(
        engine.document(),
        &before,
        "one undo step restores 1: uart-tx"
    );
    let restored = inspect(&engine, u1)?.functions;
    assert_eq!(
        active(&restored, 1),
        Some((UART_TX, ActiveOrigin::Selected))
    );
    assert!(restored.unresolved.contains(&TerminalId(2)));
    engine.redo()?;
    assert_eq!(engine.document(), &moved);

    // r2 reorders terminal 2's functions and relabels uart-tx-alt.
    with(&mut engine, m.component, component_body, |c| {
        if let Some(TerminalClass::Electrical(f)) = c
            .terminals
            .iter_mut()
            .find(|t| t.id == TerminalId(2))
            .map(|t| &mut t.class)
        {
            f.functions.reverse();
            if let Some(alt) = f
                .functions
                .iter_mut()
                .find(|f| f.id == FunctionId(UART_TX_ALT))
            {
                alt.label = "uart-tx-b".into();
            }
        }
    })?;
    let r2 = publish_component(&mut engine, m.component)?;
    let update = |target: ComponentRef, instances: Vec<Id>, resolutions: Resolutions| {
        Change::ComponentRevision {
            instances,
            target,
            resolutions,
        }
    };
    let plan = engine.preview_update(update(r2, vec![u1, u2], Resolutions::default()));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let function_changes: Vec<&String> = plan
        .changes
        .iter()
        .filter(|c| c.contains("function") || c.contains("selection"))
        .collect();
    assert_eq!(
        function_changes,
        [
            "U1 terminal 2: function 46 ‘uart-tx-alt’ is relabeled ‘uart-tx-b’; selections are kept by ID.",
            "U2 terminal 2: function 46 ‘uart-tx-alt’ is relabeled ‘uart-tx-b’; selections are kept by ID.",
        ]
    );
    engine.apply_update(plan.revision, plan.change, false)?;
    for instance in [u1, u2] {
        let report = inspect(&engine, instance)?;
        assert_eq!(report.component, r2);
        let alt = report.functions.active(TerminalId(2));
        assert_eq!(
            alt.map(|a| (a.function, a.label.as_str())),
            Some((FunctionId(UART_TX_ALT), "uart-tx-b"))
        );
    }

    // r3 removes the selected function: blocked for both until resolved.
    with(&mut engine, m.component, component_body, |c| {
        if let Some(TerminalClass::Electrical(f)) = c
            .terminals
            .iter_mut()
            .find(|t| t.id == TerminalId(2))
            .map(|t| &mut t.class)
        {
            f.functions.retain(|f| f.id != FunctionId(UART_TX_ALT));
        }
    })?;
    let r3 = publish_component(&mut engine, m.component)?;
    for instance in [u1, u2] {
        let plan = engine.preview_update(update(r3, vec![instance], Resolutions::default()));
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.contains("FunctionResolution")),
            "{:?}",
            plan.blockers
        );
    }
    // Cancel: not applying keeps the pins and the selections.
    let kept = Snapshot::of(&engine);
    let blocked = update(r3, vec![u1, u2], Resolutions::default());
    let error = engine.apply_update(engine.revision(), blocked, false).err();
    assert_eq!(error.map(|e| e.code()), Some("conflict"));
    kept.assert_kept(&engine, "blocked function removal");
    let resolved = Resolutions {
        functions: [u1, u2]
            .into_iter()
            .map(|instance| FunctionResolution {
                instance,
                terminal: TerminalId(2),
                function: None,
            })
            .collect(),
        ..Resolutions::default()
    };
    let plan = engine.preview_update(update(r3, vec![u1, u2], resolved));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c == "U1 terminal 2: selection function 46 → no selection.")
    );
    engine.apply_update(plan.revision, plan.change, false)?;
    assert_eq!(inspect(&engine, u1)?.component, r3);

    // bus-od on 3 together with adc-in on 1 is excluded.
    choose(&mut engine, u1, &[(3, Some(BUS_OD))])?;
    let kept = Snapshot::of(&engine);
    let excluded = choose(&mut engine, u1, &[(1, Some(ADC_IN))]).err();
    assert!(excluded.is_some_and(
        |e| e.code() == "conflict" && e.to_string().contains("cannot be active together")
    ));
    kept.assert_kept(&engine, "excluded pair");

    // B2 lacks bus-od: a direct implementation change is rejected...
    let error = apply(
        &mut engine,
        Operation::Instance(InstanceCommand::SetPhysical {
            instance: u1,
            choice: Some(m.choice(m.b2)),
        }),
    )
    .err();
    assert!(error.is_some_and(|e| {
        e.code() == "conflict"
            && e.to_string()
                .contains("not available in the chosen implementation")
    }));
    kept.assert_kept(&engine, "unavailable function");
    // ...and on a placed instance the reviewed plan is blocked.
    apply(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    assert_eq!(inspect(&engine, u1)?.board_placements, 1);
    let plan = engine.preview_update(Change::Instance {
        edit: InstanceChange::SetPhysical {
            instance: u1,
            choice: Some(m.choice(m.b2)),
        },
    });
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.contains("not available in the chosen implementation")),
        "{:?}",
        plan.blockers
    );
    // Availability is an invariant, never a board-placeable finding.
    let report = inspect(&engine, u1)?;
    assert!(report.board_placeable);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.blocks_stage(Stage::BoardPlaceable))
    );

    // Save and reopen keep the arrays, IDs and selections.
    let bytes = format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(reopened.project(), engine.project());
    assert_eq!(
        inspect(&reopened, u1)?.functions,
        inspect(&engine, u1)?.functions
    );
    Ok(())
}

fn publish_component(engine: &mut Engine, component: DefinitionId) -> Result<ComponentRef> {
    publish(engine, vec![component])?
        .into_iter()
        .next()
        .map(|r| ComponentRef::new(r.id, r.revision))
        .ok_or_else(|| Error::NotFound("component revision".into()))
}

/// A small multi-function component for publication cases: terminal 1 with
/// an input (10) and a resource-taking output (11), terminal 2 with one
/// fixed passive function (12), and resource 20 with role 21 of capacity 1.
fn small(engine: &mut Engine) -> Result<Fixture> {
    let f = draft(
        engine,
        &Spec {
            name: "Small",
            terminals: vec![
                (
                    1,
                    "A",
                    electrical(
                        vec![
                            function(
                                10,
                                "in",
                                Direction::Input,
                                SignalKind::Digital,
                                Drive::None,
                                None,
                            ),
                            function(
                                11,
                                "out",
                                Direction::Output,
                                SignalKind::Digital,
                                Drive::PushPull,
                                Some((20, 21)),
                            ),
                        ],
                        Some(10),
                    ),
                ),
                (
                    2,
                    "B",
                    electrical(
                        vec![function(
                            12,
                            "passive",
                            Direction::Passive,
                            SignalKind::Unspecified,
                            Drive::None,
                            None,
                        )],
                        None,
                    ),
                ),
            ],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )?;
    with(engine, f.component, component_body, |c| {
        c.next_local = 30;
        c.resources = vec![Resource {
            id: ResourceId(20),
            label: "r".into(),
            kind: None,
            roles: vec![role(21, "role", false)],
        }];
    })?;
    Ok(f)
}

fn function_mut(c: &mut ComponentDefinition, id: u32) -> Option<&mut Function> {
    c.terminals
        .iter_mut()
        .filter_map(|t| match &mut t.class {
            TerminalClass::Electrical(f) => Some(f),
            _ => None,
        })
        .flat_map(|f| f.functions.iter_mut())
        .find(|f| f.id == FunctionId(id))
}

fn functions_mut(c: &mut ComponentDefinition, terminal: u32) -> Option<&mut TerminalFunctions> {
    c.terminals
        .iter_mut()
        .find(|t| t.id == TerminalId(terminal))
        .and_then(|t| match &mut t.class {
            TerminalClass::Electrical(f) => Some(f),
            _ => None,
        })
}

#[test]
fn publication_rejects_invalid_function_declarations() -> Result<()> {
    type Mutation = fn(&mut ComponentDefinition);
    let component_cases: Vec<(&str, &str, Mutation)> = vec![
        ("input with push-pull drive", "function.combination", |c| {
            if let Some(f) = function_mut(c, 10) {
                f.drive = Drive::PushPull;
            }
        }),
        ("output without drive", "function.combination", |c| {
            if let Some(f) = function_mut(c, 11) {
                f.drive = Drive::None;
            }
        }),
        ("power signal on an input", "function.combination", |c| {
            if let Some(f) = function_mut(c, 10) {
                f.signal = SignalKind::Power;
            }
        }),
        (
            "duplicate function ID",
            "definition.local_id_invalid",
            |c| {
                if let Some(f) = function_mut(c, 12) {
                    f.id = FunctionId(10);
                }
            },
        ),
        (
            "design default not listed",
            "function.design_default",
            |c| {
                if let Some(f) = functions_mut(c, 1) {
                    f.design_default = Some(FunctionId(12));
                }
            },
        ),
        ("unknown resource", "function.resource_unknown", |c| {
            if let Some(f) = function_mut(c, 11) {
                f.resource_role = Some(ResourceRole {
                    resource: ResourceId(29),
                    role: RoleId(21),
                });
            }
        }),
        ("unknown role", "function.resource_unknown", |c| {
            if let Some(f) = function_mut(c, 11) {
                f.resource_role = Some(ResourceRole {
                    resource: ResourceId(20),
                    role: RoleId(29),
                });
            }
        }),
        (
            "constraint key of another terminal",
            "function.constraint_key",
            |c| {
                c.constraints = vec![FunctionConstraint::Excludes {
                    a: key(1, 12),
                    b: key(2, 12),
                }];
            },
        ),
        (
            "defaults exceed a role's capacity",
            "function.default_conflict",
            |c| {
                if let Some(f) = functions_mut(c, 1) {
                    f.design_default = Some(FunctionId(11));
                }
                if let Some(f) = function_mut(c, 12) {
                    f.resource_role = Some(ResourceRole {
                        resource: ResourceId(20),
                        role: RoleId(21),
                    });
                }
            },
        ),
    ];
    for (name, code, mutate) in component_cases {
        let mut engine = Engine::default();
        let f = small(&mut engine)?;
        with(&mut engine, f.component, component_body, mutate)?;
        let report = check(
            &engine,
            crate::completeness::CheckTarget::Draft(f.component),
        )?;
        assert!(
            report
                .findings
                .iter()
                .any(|x| x.code == code && x.blocks_stage(Stage::Publish)),
            "{name}: {:?}",
            report.findings
        );
        refused(&mut engine, f.all(), name)?;
    }
    let binding_cases: Vec<(&str, &str, Vec<FunctionKey>)> = vec![
        (
            "fixed function unavailable",
            "binding.unavailable_required",
            vec![key(2, 12)],
        ),
        (
            "design default unavailable",
            "binding.unavailable_required",
            vec![key(1, 10)],
        ),
        (
            "function of another terminal",
            "binding.unavailable_unknown",
            vec![key(1, 12)],
        ),
        (
            "duplicate unavailable key",
            "binding.unavailable_duplicate",
            vec![key(1, 11), key(1, 11)],
        ),
    ];
    for (name, code, unavailable) in binding_cases {
        let mut engine = Engine::default();
        let f = small(&mut engine)?;
        with(&mut engine, f.binding, binding_body, |b| {
            b.unavailable = unavailable
        })?;
        let checked = if code == "binding.unavailable_duplicate" {
            f.binding
        } else {
            f.component
        };
        let report = check(&engine, crate::completeness::CheckTarget::Draft(checked))?;
        assert!(
            report
                .findings
                .iter()
                .any(|x| x.code == code && x.blocks_stage(Stage::Publish)),
            "{name}: {:?}",
            report.findings
        );
        refused(&mut engine, f.all(), name)?;
    }
    // The valid fixture publishes, with an unavailable non-default function.
    let mut engine = Engine::default();
    let f = small(&mut engine)?;
    with(&mut engine, f.binding, binding_body, |b| {
        b.unavailable = vec![key(1, 11)]
    })?;
    publish(&mut engine, f.all())?;
    Ok(())
}

/// Publishing `ids` fails with `conflict` and changes nothing.
fn refused(engine: &mut Engine, ids: Vec<DefinitionId>, what: &str) -> Result<()> {
    let kept = Snapshot::of(engine);
    let error = publish(engine, ids).err();
    assert_eq!(error.map(|e| e.code()), Some("conflict"), "{what}");
    kept.assert_kept(engine, what);
    Ok(())
}

#[test]
fn device_nc_and_reserved_terminals_and_no_connect_exclude_wiring() -> Result<()> {
    let mut engine = Engine::default();
    let part = component(
        &mut engine,
        &Spec {
            name: "Classified",
            terminals: vec![
                (1, "A", passive()),
                (2, "NC", TerminalClass::DeviceNc),
                (3, "RSV", TerminalClass::Reserved),
                (4, "D", passive()),
            ],
            units: vec![(
                1,
                "A",
                vec![
                    (1, [-5.0, 0.0]),
                    (2, [5.0, 0.0]),
                    (3, [0.0, 5.0]),
                    (4, [0.0, -5.0]),
                ],
            )],
            package: vec![(1, "1"), (2, "2"), (3, "3"), (4, "4")],
            mapping: vec![(1, 1), (2, 2), (3, 3), (4, 4)],
            physical: true,
        },
    )?;
    let probe = component(&mut engine, &probe("Probe", passive()))?;
    let (x, _) = place(&mut engine, &part, "X1", 1, [0.0, 0.0])?;
    let (p, _) = place(&mut engine, &probe, "P1", 1, [20.0, 0.0])?;
    let (q, _) = place(&mut engine, &probe, "P2", 1, [20.0, 10.0])?;
    let kept = Snapshot::of(&engine);
    for t in [2, 3] {
        let error = connect(&mut engine, ep(x, t), ep(p, 1)).err();
        assert_eq!(error.map(|e| e.code()), Some("conflict"), "wire {t}");
        let nc = apply(
            &mut engine,
            Operation::Instance(InstanceCommand::SetNoConnect {
                instance: x,
                terminals: vec![TerminalId(t)],
                no_connect: true,
            }),
        )
        .err();
        assert_eq!(
            nc.map(|e| e.code()),
            Some("invalid_input"),
            "no-connect {t}"
        );
        kept.assert_kept(&engine, "non-electrical terminal");
    }
    // Device NC and reserved terminals need no decision.
    let report = inspect(&engine, x)?;
    for t in [2, 3] {
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.objects == [terminal(x, t)])
        );
    }
    // A wired terminal cannot become no-connect, and a no-connect terminal
    // cannot be wired: the same diagnostic.
    connect(&mut engine, ep(x, 1), ep(p, 1))?;
    let no_connect = |engine: &mut Engine, t: u32| {
        apply(
            engine,
            Operation::Instance(InstanceCommand::SetNoConnect {
                instance: x,
                terminals: vec![TerminalId(t)],
                no_connect: true,
            }),
        )
    };
    let wired = no_connect(&mut engine, 1).err();
    no_connect(&mut engine, 4)?;
    let marked = connect(&mut engine, ep(x, 4), ep(q, 1)).err();
    let (Some(wired), Some(marked)) = (wired, marked) else {
        return Err(Error::Invalid("both edits must be rejected".into()));
    };
    assert_eq!(wired.code(), marked.code());
    assert_eq!(
        wired.to_string().replace("terminal ‘A’", "terminal N"),
        marked.to_string().replace("terminal ‘D’", "terminal N")
    );
    // A passive-only net has no ERC finding.
    let net = net_of(&engine, ep(x, 1))?;
    assert_eq!(erc_on(&engine, net), Vec::<String>::new());
    Ok(())
}

#[test]
fn function_declarations_persist_and_are_exposed_in_schemas() -> Result<()> {
    let mut engine = Engine::default();
    let m = draft_mcu(&mut engine)?;
    publish(&mut engine, m.all())?;
    let u1 = place_mcu(&mut engine, &m, "U1", 0.0)?;
    choose(&mut engine, u1, &[(1, Some(UART_TX)), (3, Some(UART_RX))])?;
    let bytes = format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(reopened.document(), engine.document());
    let file = serde_json::to_value(engine.to_file())?;
    assert_eq!(
        file["project"]["instances"][0]["functions"],
        json!({"1": UART_TX, "3": UART_RX})
    );
    // A device-NC terminal with functions is not representable: decode
    // refuses it as malformed rather than dropping data.
    let mut broken = file.clone();
    broken["project"]["library"]["components"][0]["definition"]["terminals"][1]["class"] =
        json!({"device_nc": {"functions": [], "design_default": null}});
    assert!(format::decode(&serde_json::to_vec(&broken)?).is_err());
    let schemas = serde_json::to_string(&crate::schemas())?;
    for name in [
        "instance.select_functions",
        "TerminalFunctions",
        "design_default",
        "resource_role",
        "FunctionConstraint",
        "unavailable",
        "functions_selected",
    ] {
        assert!(schemas.contains(name), "schema exposes {name}");
    }
    Ok(())
}

#[test]
fn a_selection_batch_rejects_structural_errors_atomically() -> Result<()> {
    let mut engine = Engine::default();
    let m = draft_mcu(&mut engine)?;
    publish(&mut engine, m.all())?;
    let u1 = place_mcu(&mut engine, &m, "U1", 0.0)?;
    let kept = Snapshot::of(&engine);
    for (what, edits) in [
        ("function of another terminal", vec![(2, Some(UART_TX))]),
        ("unknown terminal", vec![(99, Some(UART_TX))]),
        ("terminal edited twice", vec![(1, Some(UART_TX)), (1, None)]),
        ("empty batch", vec![]),
    ] {
        let error = choose(&mut engine, u1, &edits).err();
        assert_eq!(error.map(|e| e.code()), Some("invalid_input"), "{what}");
        kept.assert_kept(&engine, what);
    }
    let stale = engine
        .apply(
            engine.revision() + 1,
            Operation::Instance(InstanceCommand::SelectFunctions {
                instance: u1,
                edits: vec![FunctionEdit {
                    terminal: TerminalId(1),
                    function: Some(FunctionId(UART_TX)),
                }],
            }),
        )
        .err();
    assert_eq!(stale.map(|e| e.code()), Some("stale_revision"));
    let malformed = serde_json::from_value::<Operation>(json!({
        "editor": "Instance",
        "operation": {"command": "instance.select_functions", "params": {"instance": u1, "edits": [{"terminal": "one"}]}}
    }));
    assert!(malformed.is_err());
    kept.assert_kept(&engine, "stale and malformed");
    Ok(())
}

#[test]
fn a_requires_constraint_is_reported_until_its_prerequisite_is_active() -> Result<()> {
    let mut engine = Engine::default();
    let f = small(&mut engine)?;
    // Terminal 2 gains a second function 13 and no default; selecting 11 on
    // terminal 1 requires 13 on terminal 2.
    with(&mut engine, f.component, component_body, |c| {
        if let Some(t2) = functions_mut(c, 2) {
            t2.functions.push(function(
                13,
                "enable",
                Direction::Output,
                SignalKind::Digital,
                Drive::PushPull,
                None,
            ));
        }
        c.constraints = vec![FunctionConstraint::Requires {
            selected: key(1, 11),
            requires: key(2, 13),
        }];
    })?;
    publish(&mut engine, f.all())?;
    let (x, _) = place(&mut engine, &f, "X1", 1, [0.0, 0.0])?;
    let unmet = |engine: &Engine| {
        has_finding(
            engine,
            x,
            "config.requirement_unmet",
            &[
                ObjectRef::Instance { id: x },
                terminal(x, 1),
                terminal(x, 2),
            ],
        )
    };

    // Selecting 11 alone is a valid stepwise state with an unmet requirement.
    let resolution = choose(&mut engine, x, &[(1, Some(11))])?;
    assert!(resolution.conflicts.is_empty());
    assert_eq!(resolution.incomplete.len(), 1);
    assert_eq!(
        resolution.incomplete[0].message(),
        "‘out’ on terminal ‘A’ requires ‘enable’ on terminal ‘B’."
    );
    assert!(unmet(&engine)?);
    assert!(!inspect(&engine, x)?.electrically_resolved);

    // The passive alternative on 2 does not satisfy it.
    choose(&mut engine, x, &[(2, Some(12))])?;
    assert!(unmet(&engine)?);

    // Activating 13 does.
    let resolution = choose(&mut engine, x, &[(2, Some(13))])?;
    assert!(resolution.incomplete.is_empty());
    assert!(!unmet(&engine)?);

    // Undo restores the unmet state; redo clears it again.
    engine.undo()?;
    assert!(unmet(&engine)?);
    engine.redo()?;
    assert!(!unmet(&engine)?);

    // Without 11 the constraint does not apply.
    choose(&mut engine, x, &[(1, Some(10)), (2, Some(12))])?;
    assert!(!unmet(&engine)?);
    Ok(())
}

#[test]
fn an_unrouted_exposed_pad_land_is_reported_although_its_terminal_is_one_net() -> Result<()> {
    let mut engine = Engine::default();
    let m = draft_mcu(&mut engine)?;
    publish(&mut engine, m.all())?;
    let u1 = place_mcu(&mut engine, &m, "U1", 0.0)?;
    apply(&mut engine, Operation::Layout(LayoutCommand::Sync))?;
    // Terminal 21 is one electrical terminal on one net with three lands,
    // pads 21–23 at y = −1, 0, +1 mm.
    let lands: Vec<_> = crate::layout::pads(engine.project())
        .into_iter()
        .filter(|p| p.instance == u1 && p.terminal.terminal == TerminalId(21))
        .collect();
    assert_eq!(
        lands.iter().map(|p| p.pad.0).collect::<Vec<_>>(),
        [21, 22, 23]
    );
    let net = net_of(&engine, ep(u1, 21))?;
    assert!(lands.iter().all(|p| p.net == net));
    let at = |pad: u32| {
        lands
            .iter()
            .find(|p| p.pad == PadId(pad))
            .map(|p| p.position)
            .ok_or_else(|| Error::NotFound(format!("pad {pad}")))
    };
    let unrouted = |engine: &Engine| -> Vec<u32> {
        let mut pads: Vec<u32> = crate::layout::checks(engine.project())
            .into_iter()
            .filter(|f| {
                f.code == "board.unrouted" && f.objects.contains(&ObjectRef::Net { id: net })
            })
            .filter_map(|f| match f.objects.first() {
                Some(ObjectRef::Pad { instance, pad }) if *instance == u1 => Some(pad.0),
                _ => None,
            })
            .collect();
        pads.sort();
        pads
    };
    let route = |engine: &mut Engine, from: u32, to: u32| -> Result<()> {
        let request = crate::layout::RouteRequest {
            net,
            start: at(from)?,
            end: at(to)?,
            width: 0.2,
            layer: crate::Layer::Top.into(),
        };
        apply(engine, Operation::Layout(LayoutCommand::Route { request }))?;
        Ok(())
    };

    // Three separate copper islands on one net.
    assert_eq!(unrouted(&engine), [21, 22, 23]);
    // Two lands joined: land 23 is still its own island, so the net is not
    // physically connected even though its schematic connection is complete.
    route(&mut engine, 21, 22)?;
    assert_eq!(unrouted(&engine), [21, 22, 23]);
    assert!(
        crate::layout::checks(engine.project())
            .iter()
            .any(|f| f.code == "board.unrouted"
                && f.objects
                    == [
                        ObjectRef::Pad {
                            instance: u1,
                            pad: PadId(23)
                        },
                        ObjectRef::Net { id: net }
                    ])
    );
    route(&mut engine, 22, 23)?;
    assert_eq!(unrouted(&engine), Vec::<u32>::new());
    engine.undo()?;
    assert_eq!(unrouted(&engine), [21, 22, 23]);
    Ok(())
}

#[test]
fn a_power_input_without_a_power_output_on_its_net_is_reported() -> Result<()> {
    let mut engine = Engine::default();
    let sink = component(&mut engine, &probe("Power sink", power_in()))?;
    let source = component(&mut engine, &probe("Power source", power_out()))?;
    let undriven = |engine: &Engine| -> Vec<crate::Endpoint> {
        erc::checks(engine.project())
            .into_iter()
            .filter(|f| f.code == "erc.undriven_power_input")
            .filter_map(|f| match f.objects.as_slice() {
                [ObjectRef::Terminal { endpoint }] => Some(*endpoint),
                _ => None,
            })
            .collect()
    };
    let (a, _) = place(&mut engine, &sink, "A1", 1, [0.0, 0.0])?;
    // A lone power input is already undriven.
    assert_eq!(undriven(&engine), [ep(a, 1)]);
    // Two power inputs do not drive each other.
    let (b, _) = place(&mut engine, &sink, "A2", 1, [20.0, 0.0])?;
    connect(&mut engine, ep(a, 1), ep(b, 1))?;
    assert_eq!(undriven(&engine), [ep(a, 1), ep(b, 1)]);
    // A power output on the net drives both.
    let (s, _) = place(&mut engine, &source, "S1", 1, [40.0, 0.0])?;
    connect(&mut engine, ep(b, 1), ep(s, 1))?;
    assert_eq!(undriven(&engine), Vec::<crate::Endpoint>::new());
    engine.undo()?;
    assert_eq!(undriven(&engine), [ep(a, 1), ep(b, 1)]);
    Ok(())
}
