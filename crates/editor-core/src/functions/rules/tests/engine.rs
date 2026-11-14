//! Pin rules through the public engine: publishing, `instance.select_functions`,
//! completeness, ERC and the Issues report.
use crate::functions::{
    Direction, Drive, Function, FunctionEdit, PinRule, PinRuleScript, Resolution, Resource,
    ResourceRole, Role, SignalKind, TerminalFunctions,
};
use crate::ids::{FunctionId, ResourceId, RoleId, RuleId, TerminalId};
use crate::issues::{IssueScope, Severity};
use crate::library::TerminalClass;
use crate::tests::support::*;
use crate::{Engine, Error, Id, InstanceCommand, ObjectRef, Operation, Outcome, Result, erc};

fn function(id: u32, label: &str, role: Option<(u32, u32)>) -> Function {
    Function {
        id: FunctionId(id),
        label: label.into(),
        direction: Direction::Bidirectional,
        signal: SignalKind::Digital,
        drive: Drive::PushPull,
        resource_role: role.map(|(resource, role)| ResourceRole {
            resource: ResourceId(resource),
            role: RoleId(role),
        }),
        mux: None,
        mux_table: None,
        source: None,
    }
}

fn electrical(functions: Vec<Function>, default: u32) -> TerminalClass {
    TerminalClass::Electrical(TerminalFunctions {
        functions,
        design_default: Some(FunctionId(default)),
        gpio: None,
        options: Vec::new(),
    })
}

fn rule(id: u32, label: &str, source: &str) -> PinRule {
    PinRule {
        id: RuleId(id),
        label: label.into(),
        script: PinRuleScript {
            source: source.into(),
        },
        source: None,
    }
}

/// Two GPIO terminals, "TX" (with `uart_tx` on USART1) and "SCK" (with
/// `spi_sck` on SPI1), carrying `rules`.
fn mcu(engine: &mut Engine, rules: Vec<PinRule>) -> Result<Fixture> {
    let f = draft(
        engine,
        &Spec {
            name: "Rule MCU",
            terminals: vec![
                (
                    1,
                    "TX",
                    electrical(
                        vec![
                            function(10, "gpio", None),
                            function(11, "uart_tx", Some((20, 21))),
                        ],
                        10,
                    ),
                ),
                (
                    2,
                    "SCK",
                    electrical(
                        vec![
                            function(12, "gpio", None),
                            function(13, "spi_sck", Some((22, 23))),
                        ],
                        12,
                    ),
                ),
            ],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )?;
    let role = |id: u32, label: &str| Role {
        id: RoleId(id),
        label: label.into(),
        capacity: 1,
        required_when_used: false,
        signal: None,
    };
    with(engine, f.component, component_body, |c| {
        c.next_local = 100;
        c.resources = vec![
            Resource {
                id: ResourceId(20),
                label: "USART1".into(),
                kind: Some("uart".into()),
                roles: vec![role(21, "tx")],
            },
            Resource {
                id: ResourceId(22),
                label: "SPI1".into(),
                kind: Some("spi".into()),
                roles: vec![role(23, "sck")],
            },
        ];
        c.pin_rules = rules;
    })?;
    Ok(f)
}

fn choose(engine: &mut Engine, instance: Id, edits: &[(u32, u32)]) -> Result<Resolution> {
    let edits = edits
        .iter()
        .map(|(terminal, function)| FunctionEdit {
            terminal: TerminalId(*terminal),
            function: Some(FunctionId(*function)),
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

fn instance_codes(engine: &Engine, instance: Id) -> Result<Vec<String>> {
    Ok(inspect(engine, instance)?
        .findings
        .into_iter()
        .map(|f| f.code)
        .collect())
}

const SHARED_PINS: &str = r#"excludes("TX.uart_tx", "SCK.spi_sck")"#;
const NEEDS_SPI: &str = r#"if used("USART1") && !used("SPI1") { require("USART1 needs the SPI1 clock running.", ["SCK"]) } else { ok() }"#;
const NOTE: &str =
    r#"if active("SCK.spi_sck") { warn("SPI1 disables the debug clock.", ["SCK"]) } else { ok() }"#;

#[test]
fn a_forbid_verdict_rejects_the_selection_and_keeps_the_document() -> Result<()> {
    let mut engine = Engine::default();
    let f = mcu(&mut engine, vec![rule(90, "shared_pins", SHARED_PINS)])?;
    publish(&mut engine, f.all())?;
    let (x, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    assert!(choose(&mut engine, x, &[(1, 11)])?.conflicts.is_empty());
    let kept = Snapshot::of(&engine);
    let error = choose(&mut engine, x, &[(2, 13)]).err();
    let message = error.as_ref().map(ToString::to_string).unwrap_or_default();
    assert_eq!(error.map(|e| e.code()), Some("conflict"));
    assert!(
        message.contains("Pin rule ‘shared_pins’: ‘uart_tx’ on terminal ‘TX’ and ‘spi_sck’ on terminal ‘SCK’ cannot be active together."),
        "{message}"
    );
    kept.assert_kept(&engine, "a forbidden selection");
    // Moving both in one batch is fine.
    let resolution = choose(&mut engine, x, &[(1, 10), (2, 13)])?;
    assert!(resolution.conflicts.is_empty());
    Ok(())
}

#[test]
fn require_verdicts_block_electrical_resolution_and_warn_verdicts_are_erc_notes() -> Result<()> {
    let mut engine = Engine::default();
    let f = mcu(
        &mut engine,
        vec![
            rule(90, "needs_spi", NEEDS_SPI),
            rule(91, "debug_clock", NOTE),
        ],
    )?;
    publish(&mut engine, f.all())?;
    let (x, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    assert!(!instance_codes(&engine, x)?.contains(&"config.rule_unmet".to_owned()));

    choose(&mut engine, x, &[(1, 11)])?;
    let report = inspect(&engine, x)?;
    let unmet: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.code == "config.rule_unmet")
        .collect();
    assert_eq!(unmet.len(), 1);
    assert_eq!(
        unmet[0].message,
        "U1: Pin rule ‘needs_spi’: USART1 needs the SPI1 clock running."
    );
    assert_eq!(
        unmet[0].objects,
        vec![
            ObjectRef::Instance { id: x },
            ObjectRef::Terminal { endpoint: ep(x, 2) }
        ]
    );
    assert!(!report.electrically_resolved);
    let issues = crate::issues::report(
        engine.project(),
        engine.drafts(),
        &IssueScope::All,
        engine.check_settings(),
    )?;
    let row = issues
        .rows
        .iter()
        .find(|r| r.group.code == "config.rule_unmet")
        .ok_or_else(|| Error::NotFound("rule_unmet row".into()))?;
    assert_eq!(row.severity, Severity::Error);
    assert_eq!(row.group.title, "Pin rules not met");

    // Enabling SPI1 meets the requirement and raises the note.
    choose(&mut engine, x, &[(2, 13)])?;
    assert!(!instance_codes(&engine, x)?.contains(&"config.rule_unmet".to_owned()));
    let notes: Vec<_> = erc::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == "config.rule_note")
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].message,
        "U1: Pin rule ‘debug_clock’: SPI1 disables the debug clock."
    );
    assert_eq!(notes[0].blocks, None);
    let issues = crate::issues::report(
        engine.project(),
        engine.drafts(),
        &IssueScope::All,
        engine.check_settings(),
    )?;
    let row = issues
        .rows
        .iter()
        .find(|r| r.group.code == "config.rule_note")
        .ok_or_else(|| Error::NotFound("rule_note row".into()))?;
    assert_eq!(row.severity, Severity::Warning);
    assert_eq!(row.group.title, "Pin rule notes");

    // Undo restores the unmet requirement.
    engine.undo()?;
    assert!(instance_codes(&engine, x)?.contains(&"config.rule_unmet".to_owned()));
    Ok(())
}

#[test]
fn a_rule_failing_on_an_assignment_is_a_blocking_finding_not_a_rejection() -> Result<()> {
    let mut engine = Engine::default();
    // The name is computed, so publishing's samples (which activate the
    // functions a rule names literally) cannot reach the loop; the
    // literal form is refused at publish (next test).
    let f = mcu(
        &mut engine,
        vec![rule(
            90,
            "spin_on_spi",
            r#"let pin = "SC" + "K"; if active(pin, "spi_sck") { loop {} } ok()"#,
        )],
    )?;
    publish(&mut engine, f.all())?;
    let (x, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let resolution = choose(&mut engine, x, &[(2, 13)])?;
    assert_eq!(resolution.rule_failures.len(), 1);
    let report = inspect(&engine, x)?;
    let budget = report
        .findings
        .iter()
        .find(|f| f.code == "rule.budget")
        .ok_or_else(|| Error::NotFound("rule.budget".into()))?;
    assert!(
        budget
            .message
            .starts_with("U1: Pin rule ‘spin_on_spi’: The rule ran out of operations")
    );
    assert!(!report.electrically_resolved);
    Ok(())
}

#[test]
fn publishing_evaluates_each_rule_with_the_functions_it_names_active() -> Result<()> {
    // Each rule is fine without selections and fails only on a branch a
    // named function opens: an endless loop, a helper that does not
    // exist, a branch that ends in a statement (no verdict).
    for (script, code, cause) in [
        (
            r#"if active("SCK.spi_sck") { loop {} } ok()"#,
            "rule.budget",
            "operations",
        ),
        (
            r#"if active("SCK", "spi_sck") { pin_used("SCK") } else { ok() }"#,
            "rule.runtime",
            "is not a rule helper",
        ),
        (
            r#"if active("SCK.spi_sck") { let note = warn("SPI clock"); } else { ok() }"#,
            "rule.runtime",
            "must end with",
        ),
    ] {
        let mut engine = Engine::default();
        let f = mcu(&mut engine, vec![rule(90, "branchy", script)])?;
        let kept = Snapshot::of(&engine);
        assert_eq!(
            publish(&mut engine, f.all()).err().map(|e| e.code()),
            Some("conflict"),
            "{script}"
        );
        kept.assert_kept(&engine, script);
        let findings = check(
            &engine,
            crate::completeness::CheckTarget::Draft(f.component),
        )?
        .findings;
        let finding = findings
            .iter()
            .find(|f| f.code == code)
            .ok_or_else(|| Error::NotFound(format!("{code} for {script}: {findings:?}")))?;
        assert!(
            finding.message.contains("fails with SCK.spi_sck active")
                && finding.message.contains(cause),
            "{}",
            finding.message
        );
    }
    Ok(())
}

#[test]
fn publishing_refuses_rules_that_do_not_compile_or_share_a_label() -> Result<()> {
    for (rules, what) in [
        (vec![rule(90, "broken", "if (")], "a syntax error"),
        (
            vec![rule(90, "twin", "ok()"), rule(91, "Twin", "ok()")],
            "a duplicate label",
        ),
    ] {
        let mut engine = Engine::default();
        let f = mcu(&mut engine, rules)?;
        let kept = Snapshot::of(&engine);
        let error = publish(&mut engine, f.all()).err();
        assert_eq!(error.map(|e| e.code()), Some("conflict"), "{what}");
        kept.assert_kept(&engine, what);
        let findings = check(
            &engine,
            crate::completeness::CheckTarget::Draft(f.component),
        )?
        .findings;
        assert!(
            findings.iter().any(|f| f.code.starts_with("rule.")),
            "{what}: {findings:?}"
        );
    }
    Ok(())
}

#[test]
fn rule_scripts_persist_with_the_revision() -> Result<()> {
    let mut engine = Engine::default();
    let f = mcu(&mut engine, vec![rule(90, "shared_pins", SHARED_PINS)])?;
    let revisions = publish(&mut engine, f.all())?;
    let component = revisions
        .iter()
        .find(|r| r.id == f.component)
        .ok_or_else(|| Error::NotFound("component revision".into()))?;
    let bytes = crate::format::encode(&engine.to_file())?;
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\"pin_rules\""));
    assert!(text.contains("shared_pins"));
    let reopened = Engine::from_file(crate::format::decode(&bytes)?)?;
    let rules = reopened
        .project()
        .library
        .component(crate::ComponentRef::new(component.id, component.revision))
        .map(|c| c.definition.pin_rules.clone())
        .unwrap_or_default();
    assert_eq!(rules, vec![rule(90, "shared_pins", SHARED_PINS)]);
    Ok(())
}

/// `instance.set_pin_config` does not change function ERC, but rules read
/// the configuration: a `forbid` over `option(…)` refuses the edit and a
/// `require` becomes a `config.rule_unmet` finding.
#[test]
fn rules_over_pin_options_refuse_or_flag_a_pin_configuration() -> Result<()> {
    use crate::functions::pin_config::PinConfigEdit;
    use crate::functions::{PinConfig, PinOption, Pull};
    let mut engine = Engine::default();
    let f = mcu(
        &mut engine,
        vec![
            rule(
                90,
                "no_pull_down",
                r#"if option("TX", "pull") == "down" { forbid("TX must not be pulled down.", ["TX"]) } else { ok() }"#,
            ),
            rule(
                91,
                "pull_up_needs_uart",
                r#"if option("TX", "pull") == "up" && !used("USART1") { require("A pulled-up TX needs USART1.", ["TX"]) } else { ok() }"#,
            ),
        ],
    )?;
    with(&mut engine, f.component, component_body, |c| {
        if let Some(TerminalClass::Electrical(declared)) = c
            .terminals
            .iter_mut()
            .find(|t| t.id == TerminalId(1))
            .map(|t| &mut t.class)
        {
            declared.options = vec![PinOption::Pull(vec![Pull::None, Pull::Up, Pull::Down])];
        }
    })?;
    publish(&mut engine, f.all())?;
    let (x, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    let set = |engine: &mut Engine, pull: Pull| {
        apply(
            engine,
            Operation::Instance(InstanceCommand::SetPinConfig {
                instance: x,
                edits: vec![PinConfigEdit {
                    terminal: TerminalId(1),
                    config: Some(PinConfig {
                        pull: Some(pull),
                        ..PinConfig::default()
                    }),
                }],
            }),
        )
    };
    let kept = Snapshot::of(&engine);
    let refused = set(&mut engine, Pull::Down).err();
    let message = refused
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert_eq!(refused.map(|e| e.code()), Some("conflict"), "{message}");
    assert!(message.contains("TX must not be pulled down."), "{message}");
    kept.assert_kept(&engine, "a forbidden pin configuration");

    set(&mut engine, Pull::Up)?;
    let unmet: Vec<_> = inspect(&engine, x)?
        .findings
        .into_iter()
        .filter(|f| f.code == "config.rule_unmet")
        .collect();
    assert_eq!(unmet.len(), 1, "{unmet:?}");
    assert!(unmet[0].message.contains("A pulled-up TX needs USART1."));
    Ok(())
}

#[test]
fn a_rule_note_keeps_its_datasheet_source() -> Result<()> {
    let mut engine = Engine::default();
    let mut note = rule(91, "debug_clock", NOTE);
    note.source = Some(crate::functions::SourceSpan {
        page: Some(16),
        table: Some("Table 5".into()),
    });
    let f = mcu(&mut engine, vec![note])?;
    publish(&mut engine, f.all())?;
    let (x, _) = place(&mut engine, &f, "U1", 1, [0.0, 0.0])?;
    choose(&mut engine, x, &[(2, 13)])?;
    let notes: Vec<_> = erc::checks(engine.project())
        .into_iter()
        .filter(|f| f.code == "config.rule_note")
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].message,
        "U1: Pin rule ‘debug_clock’: SPI1 disables the debug clock. (source: page 16, Table 5)"
    );
    Ok(())
}
