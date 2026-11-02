//! Behaviour of the pin-rule sandbox, verdicts in the resolution and the
//! publish findings. The fixture is a synthetic, original MCU fragment.
use super::*;
use crate::functions::{
    Direction, Drive, Function, FunctionSelections, Resource, ResourceRole, Role, SignalKind,
    TerminalFunctions, resolve_with_rules,
};
use crate::ids::{FunctionId, RoleId};

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

fn functions(list: Vec<Function>, default: u32) -> TerminalFunctions {
    TerminalFunctions {
        functions: list,
        design_default: Some(FunctionId(default)),
        gpio: None,
        options: Vec::new(),
    }
}

struct Mcu {
    terminals: Vec<(TerminalId, String, Option<TerminalFunctions>)>,
    resources: Vec<Resource>,
}

impl Mcu {
    fn new() -> Self {
        let passive = TerminalFunctions::fixed(FunctionId(110), Direction::Passive);
        let passive2 = TerminalFunctions::fixed(FunctionId(111), Direction::Passive);
        Self {
            terminals: vec![
                (
                    TerminalId(1),
                    "PA9".into(),
                    Some(functions(
                        vec![
                            function(100, "gpio", None),
                            function(101, "uart_tx", Some((50, 51))),
                        ],
                        100,
                    )),
                ),
                (
                    TerminalId(2),
                    "PA10".into(),
                    Some(functions(
                        vec![
                            function(102, "gpio", None),
                            function(103, "uart_rx", Some((50, 52))),
                        ],
                        102,
                    )),
                ),
                (
                    TerminalId(3),
                    "P1.2".into(),
                    Some(functions(
                        vec![
                            function(104, "gpio", None),
                            function(105, "spi_sck", Some((60, 61))),
                        ],
                        104,
                    )),
                ),
                (TerminalId(4), "GND".into(), Some(passive)),
                (TerminalId(5), "GND".into(), Some(passive2)),
            ],
            resources: vec![
                Resource {
                    id: ResourceId(50),
                    label: "USART1".into(),
                    kind: Some("uart".into()),
                    roles: vec![role(51, "tx"), role(52, "rx")],
                },
                Resource {
                    id: ResourceId(60),
                    label: "SPI1".into(),
                    kind: Some("spi".into()),
                    roles: vec![role(61, "sck")],
                },
            ],
        }
    }

    fn declarations(&self) -> Vec<TerminalDeclaration<'_>> {
        self.terminals
            .iter()
            .map(|(terminal, name, functions)| TerminalDeclaration {
                terminal: *terminal,
                name,
                functions: functions.as_ref(),
            })
            .collect()
    }

    fn resolve(
        &self,
        selections: &[(u32, u32)],
        rules: &[PinRule],
        config: &PinConfigs,
    ) -> Result<Resolution> {
        let selections: FunctionSelections = selections
            .iter()
            .map(|(t, f)| (TerminalId(*t), FunctionId(*f)))
            .collect();
        resolve_with_rules(
            &self.declarations(),
            &self.resources,
            &[],
            None,
            &selections,
            rules,
            config,
        )
    }

    fn view(&self, selections: &[(u32, u32)]) -> Result<RuleView> {
        let resolution = self.resolve(selections, &[], &PinConfigs::new())?;
        Ok(view(
            &self.declarations(),
            &self.resources,
            &resolution,
            &PinConfigs::new(),
        ))
    }
}

fn role(id: u32, label: &str) -> Role {
    Role {
        id: RoleId(id),
        label: label.into(),
        capacity: 1,
        required_when_used: false,
        signal: None,
    }
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

/// Evaluate one script over a view with the default budget.
fn run(view: &RuleView, source: &str) -> Result<RuleEvaluation> {
    let compiled = compile(&[rule(900, "probe", source)])?;
    evaluate(&compiled, view, RuleBudget::default())
}

fn verdict_of(view: &RuleView, source: &str) -> Result<RuleVerdict> {
    let evaluation = run(view, source)?;
    match (evaluation.outcomes.first(), evaluation.failures.first()) {
        (Some(o), None) => Ok(o.verdict.clone()),
        other => Err(Error::Invalid(format!("no verdict: {other:?}"))),
    }
}

fn failure_of(view: &RuleView, source: &str) -> Result<RuleFailure> {
    run(view, source)?
        .failures
        .first()
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("no failure for {source}")))
}

fn codes(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.code.as_str()).collect()
}

#[test]
fn verdict_helpers_build_typed_verdicts_with_terminals() -> Result<()> {
    let view = Mcu::new().view(&[])?;
    assert_eq!(verdict_of(&view, "ok()")?, RuleVerdict::Ok);
    assert_eq!(
        verdict_of(&view, r##"forbid("No.", ["PA9", "#2", "PA9"])"##)?,
        RuleVerdict::Forbid {
            message: "No.".into(),
            terminals: vec![TerminalId(1), TerminalId(2)],
        }
    );
    assert_eq!(
        verdict_of(&view, r#"require("Needs it.")"#)?,
        RuleVerdict::Require {
            message: "Needs it.".into(),
            terminals: vec![],
        }
    );
    assert_eq!(
        verdict_of(&view, r#"warn("Careful.", ["P1.2"])"#)?,
        RuleVerdict::Warn {
            message: "Careful.".into(),
            terminals: vec![TerminalId(3)],
        }
    );
    Ok(())
}

#[test]
fn view_helpers_read_the_assignment() -> Result<()> {
    let mcu = Mcu::new();
    let idle = mcu.view(&[])?;
    let uart = mcu.view(&[(1, 101), (3, 105)])?;
    let truth = |view: &RuleView, expr: &str| -> Result<bool> {
        let source = format!("if {expr} {{ forbid(\"yes\") }} else {{ ok() }}");
        Ok(matches!(
            verdict_of(view, &source)?,
            RuleVerdict::Forbid { .. }
        ))
    };
    for (expr, on_idle, on_uart) in [
        (r#"active("PA9", "uart_tx")"#, false, true),
        (r#"active("PA9", "UART_TX")"#, false, true),
        (r#"active("PA9.uart_tx")"#, false, true),
        (r#"active("PA9.gpio")"#, true, false),
        (r#"active("P1.2.spi_sck")"#, false, true),
        (r##"active("#3.spi_sck")"##, false, true),
        (r#"used("USART1")"#, false, true),
        (r#"used("usart1")"#, false, true),
        (r#"role("USART1", "tx")"#, false, true),
        (r#"role("USART1", "rx")"#, false, false),
        (r#"kind("SPI1") == "spi""#, true, true),
        (r#"function("PA9") == "uart_tx""#, false, true),
        (r#"function("PA10") == "gpio""#, true, true),
        (
            r#"active_count(["PA9.uart_tx", "P1.2.spi_sck", "PA10.uart_rx"]) == 2"#,
            false,
            true,
        ),
        (r#"option("PA9", "pull") == """#, true, true),
    ] {
        assert_eq!(truth(&idle, expr)?, on_idle, "{expr} idle");
        assert_eq!(truth(&uart, expr)?, on_uart, "{expr} uart");
    }
    Ok(())
}

#[test]
fn pair_helpers_match_the_typed_constraints() -> Result<()> {
    let mcu = Mcu::new();
    let both = mcu.view(&[(1, 101), (3, 105)])?;
    assert_eq!(
        verdict_of(&both, r#"excludes("PA9.uart_tx", "P1.2.spi_sck")"#)?,
        RuleVerdict::Forbid {
            message: "‘uart_tx’ on terminal ‘PA9’ and ‘spi_sck’ on terminal ‘P1.2’ cannot be active together.".into(),
            terminals: vec![TerminalId(1), TerminalId(3)],
        }
    );
    assert_eq!(
        verdict_of(&both, r#"requires("PA9.uart_tx", "PA10.uart_rx")"#)?,
        RuleVerdict::Require {
            message: "‘uart_tx’ on terminal ‘PA9’ requires ‘uart_rx’ on terminal ‘PA10’.".into(),
            terminals: vec![TerminalId(1), TerminalId(2)],
        }
    );
    let idle = mcu.view(&[])?;
    assert_eq!(
        verdict_of(&idle, r#"excludes("PA9.uart_tx", "P1.2.spi_sck")"#)?,
        RuleVerdict::Ok
    );
    assert_eq!(
        verdict_of(&idle, r#"requires("PA9.uart_tx", "PA10.uart_rx")"#)?,
        RuleVerdict::Ok
    );
    Ok(())
}

#[test]
fn unknown_and_ambiguous_names_are_runtime_failures() -> Result<()> {
    let view = Mcu::new().view(&[])?;
    for (source, text) in [
        (
            r#"if active("PA11", "gpio") { ok() } else { ok() }"#,
            "No terminal is named ‘PA11’.",
        ),
        (
            r#"if active("PA9.spi") { ok() } else { ok() }"#,
            "Terminal ‘PA9’ has no function ‘spi’.",
        ),
        (
            r#"if active("GND.passive") { ok() } else { ok() }"#,
            "Several terminals are named ‘GND’",
        ),
        (
            r#"if used("I2C1") { ok() } else { ok() }"#,
            "No peripheral is named ‘I2C1’.",
        ),
        (
            r#"if role("USART1", "cts") { ok() } else { ok() }"#,
            "Peripheral ‘USART1’ has no role ‘cts’.",
        ),
        (
            r#"if option("PA9", "drive") == "" { ok() } else { ok() }"#,
            "is not a pin option",
        ),
        (r#"forbid("x", ["PZ"])"#, "No terminal is named ‘PZ’."),
    ] {
        let failure = failure_of(&view, source)?;
        assert_eq!(failure.kind, RuleFailureKind::Runtime, "{source}");
        assert!(
            failure.message.contains(text),
            "{source}: {}",
            failure.message
        );
    }
    Ok(())
}

#[test]
fn a_rule_must_end_with_a_bounded_verdict() -> Result<()> {
    let view = Mcu::new().view(&[])?;
    let failure = failure_of(&view, "1 + 1")?;
    assert_eq!(failure.kind, RuleFailureKind::Runtime);
    assert!(failure.message.contains("must end with ok()"));
    let failure = failure_of(&view, r#"forbid("")"#)?;
    assert!(failure.message.contains("needs a message"));
    let long = format!(r#"forbid("{}")"#, "m".repeat(MAX_VERDICT_MESSAGE + 1));
    let failure = failure_of(&view, &long)?;
    assert_eq!(failure.kind, RuleFailureKind::Runtime);
    assert!(failure.message.contains("exceeds 512 bytes"));
    let failure = failure_of(&view, "")?;
    assert_eq!(failure.kind, RuleFailureKind::Compile);
    Ok(())
}

#[test]
fn the_sandbox_refuses_eval_output_undefined_names_and_clocks() -> Result<()> {
    let view = Mcu::new().view(&[])?;
    for source in [
        r#"eval("ok()")"#,
        r#"print("x"); ok()"#,
        r#"debug("x"); ok()"#,
        "undefined_variable",
        r#"import "x" as y; ok()"#,
        "let f = |x| x; ok()",
    ] {
        assert_eq!(
            failure_of(&view, source)?.kind,
            RuleFailureKind::Compile,
            "{source}"
        );
        assert!(syntax_error(source).is_some(), "{source}");
    }
    // Functions that need the clock or randomness do not exist.
    for source in ["let t = timestamp(); ok()", "let r = rand(); ok()"] {
        assert_eq!(
            failure_of(&view, source)?.kind,
            RuleFailureKind::Runtime,
            "{source}"
        );
    }
    assert_eq!(syntax_error("if true { ok() } else { warn(\"x\") }"), None);
    let failure = syntax_error("if (").unwrap_or_default();
    assert!(failure.contains("line 1"), "{failure}");
    Ok(())
}

#[test]
fn operation_size_and_depth_limits_are_budget_failures() -> Result<()> {
    let view = Mcu::new().view(&[])?;
    let looping = run(&view, "loop { } ok()")?;
    assert_eq!(
        looping.failures.first().map(|f| f.kind),
        Some(RuleFailureKind::Budget)
    );
    assert!(looping.operations <= MAX_RULE_OPERATIONS);
    assert!(looping.operations > MAX_RULE_OPERATIONS / 2);
    for source in [
        // A string past 1024 bytes.
        r#"let s = "x"; for i in 0..11 { s += s; } ok()"#,
        // An array past 1024 items.
        "let a = []; for i in 0..1100 { a.push(i); } ok()",
        // A map past 256 entries (an index assignment is checked when the
        // map is next used, inside the operation budget).
        "let m = #{}; for i in 0..300 { m[`k${i}`] = i; } m.len(); ok()",
        "let m = #{}; for i in 0..300 { let x = #{}; x[`k${i}`] = i; m += x; } ok()",
        // Recursion past 16 calls.
        "fn deep(n) { deep(n + 1) } deep(0)",
    ] {
        assert_eq!(
            failure_of(&view, source)?.kind,
            RuleFailureKind::Budget,
            "{source}"
        );
    }
    // Within the limits the same constructs are fine.
    assert_eq!(
        verdict_of(
            &view,
            "let a = []; for i in 0..1000 { a.push(i); } if a.len() == 1000 { ok() } else { forbid(\"x\") }"
        )?,
        RuleVerdict::Ok
    );
    Ok(())
}

#[test]
fn the_total_budget_bounds_all_rules_of_one_evaluation() -> Result<()> {
    let view = Mcu::new().view(&[])?;
    let rules = [
        rule(1, "spin", "let n = 0; while n < 1000 { n += 1; } ok()"),
        rule(
            2,
            "spin again",
            "let n = 0; while n < 1000 { n += 1; } ok()",
        ),
        rule(3, "cheap", "ok()"),
    ];
    let compiled = compile(&rules)?;
    let full = evaluate(&compiled, &view, RuleBudget::default())?;
    assert_eq!(full.outcomes.len(), 3);
    let per_rule = full.operations / 2;
    // A total that admits the first rule only.
    let tight = evaluate(
        &compiled,
        &view,
        RuleBudget {
            per_rule: MAX_RULE_OPERATIONS,
            total: per_rule + 10,
        },
    )?;
    assert_eq!(tight.outcomes.first().map(|o| o.rule), Some(RuleId(1)));
    let kinds: Vec<_> = tight.failures.iter().map(|f| (f.rule, f.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            (RuleId(2), RuleFailureKind::Budget),
            (RuleId(3), RuleFailureKind::Budget)
        ]
    );
    assert!(tight.operations <= per_rule + 10);
    Ok(())
}

#[test]
fn evaluation_is_deterministic_and_compiles_once_per_revision() -> Result<()> {
    let view = Mcu::new().view(&[(1, 101)])?;
    let rules = [
        rule(1, "map", "let m = #{b: 1, a: 2}; warn(`${m}`)"),
        rule(
            2,
            "uart",
            r#"if used("USART1") { require("Needs RX.", ["PA10"]) } else { ok() }"#,
        ),
        rule(3, "broken", "if ("),
    ];
    assert!(!is_cached(&rules));
    let first = evaluate(&compile(&rules)?, &view, RuleBudget::default())?;
    assert!(is_cached(&rules));
    let again = evaluate(&compile(&rules)?, &view, RuleBudget::default())?;
    assert_eq!(first, again);
    assert_eq!(first.outcomes.len(), 2);
    assert_eq!(
        first
            .failures
            .iter()
            .map(|f| (f.rule, f.kind))
            .collect::<Vec<_>>(),
        vec![(RuleId(3), RuleFailureKind::Compile)]
    );
    // A changed source is a new revision of the rules.
    let mut edited = rules.clone();
    edited[1].script.source.push(' ');
    assert!(!is_cached(&edited));
    Ok(())
}

#[test]
fn rules_out_of_bounds_are_refused() {
    let big = rule(1, "big", &"x".repeat(MAX_RULE_SOURCE + 1));
    assert!(!within_bounds(std::slice::from_ref(&big)));
    assert!(matches!(compile(&[big]), Err(Error::Limit(m)) if m.starts_with("rule.bounds")));
    let many: Vec<PinRule> = (0..=MAX_PIN_RULES as u32)
        .map(|i| rule(i + 1, &format!("r{i}"), "ok()"))
        .collect();
    assert!(!within_bounds(&many));
    let total: Vec<PinRule> = (0..65)
        .map(|i| rule(i + 1, &format!("r{i}"), &"x".repeat(MAX_RULE_SOURCE)))
        .collect();
    assert!(!within_bounds(&total));
    assert!(syntax_error(&"x".repeat(MAX_RULE_SOURCE + 1)).is_some());
    let findings = declaration_findings(DefinitionId(7), &total);
    assert_eq!(codes(&findings), vec!["rule.bounds"]);
}

#[test]
fn verdicts_land_in_the_resolution_buckets() -> Result<()> {
    let mcu = Mcu::new();
    let rules = [
        rule(
            1,
            "uart_or_spi",
            r#"excludes("PA9.uart_tx", "P1.2.spi_sck")"#,
        ),
        rule(2, "uart_rx", r#"requires("PA9.uart_tx", "PA10.uart_rx")"#),
        rule(
            3,
            "spi_note",
            r#"if used("SPI1") { warn("SPI1 shares its clock with the debug port.", ["P1.2"]) } else { ok() }"#,
        ),
        rule(4, "spin", "loop {} ok()"),
    ];
    let config = PinConfigs::new();
    let idle = mcu.resolve(&[], &rules, &config)?;
    assert!(idle.conflicts.is_empty() && idle.incomplete.is_empty() && idle.notes.is_empty());
    assert_eq!(idle.rule_failures.len(), 1);
    assert_eq!(idle.rule_failures[0].kind, RuleFailureKind::Budget);
    assert!(
        idle.rule_failures[0]
            .message
            .starts_with("Pin rule ‘spin’: ")
    );

    let both = mcu.resolve(&[(1, 101), (3, 105)], &rules, &config)?;
    assert_eq!(
        both.conflicts,
        vec![Conflict::Rule {
            rule: RuleId(1),
            terminals: vec![TerminalId(1), TerminalId(3)],
            message: "Pin rule ‘uart_or_spi’: ‘uart_tx’ on terminal ‘PA9’ and ‘spi_sck’ on terminal ‘P1.2’ cannot be active together.".into(),
        }]
    );
    assert_eq!(
        both.incomplete,
        vec![Requirement::Rule {
            rule: RuleId(2),
            terminals: vec![TerminalId(1), TerminalId(2)],
            message: "Pin rule ‘uart_rx’: ‘uart_tx’ on terminal ‘PA9’ requires ‘uart_rx’ on terminal ‘PA10’.".into(),
        }]
    );
    assert_eq!(
        both.notes,
        vec![RuleNote {
            rule: RuleId(3),
            message: "Pin rule ‘spi_note’: SPI1 shares its clock with the debug port.".into(),
            terminals: vec![TerminalId(3)],
        }]
    );
    Ok(())
}

#[test]
fn rules_read_the_instance_pin_configuration() -> Result<()> {
    let mcu = Mcu::new();
    let rules = [rule(
        1,
        "pull",
        r#"if active("PA9.uart_tx") && option("PA9", "pull") != "up" { warn("TX idles high; enable the pull-up.", ["PA9"]) } else { ok() }"#,
    )];
    let mut config = PinConfigs::new();
    assert_eq!(mcu.resolve(&[(1, 101)], &rules, &config)?.notes.len(), 1);
    config.insert(
        TerminalId(1),
        PinConfig {
            pull: Some(crate::functions::Pull::Up),
            ..PinConfig::default()
        },
    );
    assert!(mcu.resolve(&[(1, 101)], &rules, &config)?.notes.is_empty());
    Ok(())
}

#[test]
fn explain_reports_each_rule() -> Result<()> {
    let mcu = Mcu::new();
    let rules = [
        rule(1, "fine", "ok()"),
        rule(
            2,
            "uart_or_spi",
            r#"excludes("PA9.uart_tx", "P1.2.spi_sck")"#,
        ),
        rule(
            3,
            "typo",
            r#"if active("PA99.gpio") { ok() } else { ok() }"#,
        ),
    ];
    let selections: FunctionSelections = [
        (TerminalId(1), FunctionId(101)),
        (TerminalId(3), FunctionId(105)),
    ]
    .into_iter()
    .collect();
    let (resolution, explained) = crate::functions::explain_rules(
        &mcu.declarations(),
        &mcu.resources,
        &[],
        &selections,
        &rules,
        &PinConfigs::new(),
    )?;
    assert_eq!(resolution.conflicts.len(), 1);
    let summary: Vec<_> = explained
        .rules
        .iter()
        .map(|r| {
            (
                r.label.as_str(),
                r.verdict
                    .as_ref()
                    .map(|v| matches!(v, RuleVerdict::Forbid { .. })),
                r.failure.as_ref().map(|f| f.kind),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("fine", Some(false), None),
            ("uart_or_spi", Some(true), None),
            ("typo", None, Some(RuleFailureKind::Runtime)),
        ]
    );
    Ok(())
}

#[test]
fn publish_findings_check_scripts_references_and_the_default() -> Result<()> {
    let mcu = Mcu::new();
    let declarations = mcu.declarations();
    let component = DefinitionId(7);
    let check = |rules: &[PinRule]| {
        publish_findings(component, Some((&declarations, &mcu.resources)), rules)
    };
    assert!(
        check(&[rule(
            1,
            "fine",
            r#"requires("PA9.uart_tx", "PA10.uart_rx")"#
        )])
        .is_empty()
    );
    // A typo on a branch the default never takes is still found.
    let typo = check(&[rule(
        1,
        "typo",
        r#"if used("USART1") { if active("PA10.uart_rxd") { ok() } else { ok() } } else { ok() }"#,
    )]);
    assert_eq!(codes(&typo), vec!["rule.reference"]);
    assert!(
        typo[0].message.contains("‘uart_rxd’"),
        "{}",
        typo[0].message
    );
    assert_eq!(
        typo[0].objects,
        vec![
            ObjectRef::Definition { id: component },
            ObjectRef::LibraryObject {
                definition: component,
                local: 1
            }
        ]
    );
    // A computed name is found when the default evaluates it.
    let computed = check(&[rule(
        1,
        "computed",
        r#"let t = "PA" + "11"; if active(t, "gpio") { ok() } else { ok() }"#,
    )]);
    assert_eq!(codes(&computed), vec!["rule.reference"]);
    assert_eq!(
        codes(&check(&[rule(1, "bad", "if (")])),
        vec!["rule.script"]
    );
    assert_eq!(
        codes(&check(&[rule(1, "loop", "loop {} ok()")])),
        vec!["rule.budget"]
    );
    assert_eq!(
        codes(&check(&[rule(1, "throw", r#"throw "boom""#)])),
        vec!["rule.runtime"]
    );
    let default = check(&[rule(
        1,
        "no_gpio",
        r#"if active("PA9.gpio") { forbid("PA9 must not idle as GPIO.") } else { ok() }"#,
    )]);
    assert_eq!(codes(&default), vec!["rule.default_conflict"]);
    assert!(default[0].message.ends_with("PA9 must not idle as GPIO."));
    // A require or warn verdict at the default is allowed.
    assert!(
        check(&[
            rule(1, "w", r#"warn("note")"#),
            rule(2, "r", r#"require("later")"#)
        ])
        .is_empty()
    );
    let labels = check(&[
        rule(1, "Same", "ok()"),
        rule(2, "same", "ok()"),
        rule(3, " ", "ok()"),
    ]);
    assert_eq!(codes(&labels), vec!["rule.reference", "rule.reference"]);
    // Without the declarations only the script itself is checked.
    assert!(declaration_findings(component, &[rule(1, "typo", r#"active("PZ.gpio")"#)]).is_empty());
    assert_eq!(
        codes(&declaration_findings(component, &[rule(1, "bad", "if (")])),
        vec!["rule.script"]
    );
    Ok(())
}

#[test]
fn the_scanner_reads_literal_helper_calls_only() {
    let calls = scan::literal_calls(
        r#"
        // active("COMMENT.x")
        /* used("NOPE") /* nested */ */
        let a = active("PA9", "gpio") && x.active("METHOD.y");
        let b = active(name, "gpio") || used("USART1") || active_count(["A.f", "B.g"]);
        fn role(x) { x }
        role("SPI1", "sck")
        "#,
    );
    let got: Vec<(String, Vec<String>)> = calls.into_iter().map(|c| (c.helper, c.args)).collect();
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        got,
        vec![
            ("active".into(), s(&["PA9", "gpio"])),
            ("used".into(), s(&["USART1"])),
            ("active_count".into(), s(&["A.f", "B.g"])),
            ("role".into(), s(&["SPI1", "sck"])),
        ]
    );
}

mod engine;
