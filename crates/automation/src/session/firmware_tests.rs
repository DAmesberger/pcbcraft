//! Slice C7: the EXM-48 scenario continued on the placed instance, JSON
//! only: suggest → assign peripherals (explicit and solver-placed) → a
//! rule-forbidden assignment refused → pin configuration → wire a net →
//! the MCU-to-peripheral pin report → the firmware pin-configuration IR
//! (C4) and its emitters → export to a file. Staged as `pinout_tests`
//! explains: a C0 stub ends the reachable scenario until slice I sets
//! `REQUIRE_COMPLETE`.
use super::pinout_tests::{
    Fallible, Placed, Scenario, Stub, finish, import_reviewed, instance_op, publish_and_place,
    schematic_op,
};
use super::*;

fn assign_op(instance: &Value, peripheral: u64, roles: &[(u64, Option<u64>)]) -> Value {
    let roles: Vec<Value> = roles
        .iter()
        .map(|(role, terminal)| json!({"role": role, "terminal": terminal}))
        .collect();
    instance_op(
        "instance.assign_peripheral",
        json!({"instance": instance, "peripheral": peripheral, "roles": roles, "enable": true}),
    )
}

fn pin_config_op(instance: &Value, edits: Value) -> Value {
    instance_op(
        "instance.set_pin_config",
        json!({"instance": instance, "edits": edits}),
    )
}

fn suggest_params(instance: &Value, peripheral: u64, keep_existing: bool) -> Value {
    json!({"instance": instance, "request": {
        "enable": [{"peripheral": peripheral}], "keep_existing": keep_existing,
        "avoid": [], "limit": 2}})
}

fn firmware(command: &str, instance: &Value, emitter: &str, path: Option<&str>) -> Value {
    json!({"command": command, "params": {
        "instance": instance, "emitter": {"emitter": emitter}, "path": path}})
}

/// Every request shape the scenario and the MCP instructions use parses
/// as the contract's types, so a drift in either fails here even while
/// the handlers are stubs.
#[test]
fn every_scenario_request_is_a_contract_request() -> Fallible<()> {
    let instance = json!(7);
    for operation in [
        assign_op(&instance, 1, &[(1, Some(26)), (2, None)]),
        pin_config_op(
            &instance,
            json!([{"terminal": 27, "config": {"pull": "up", "name": "UART_RX"}},
                   {"terminal": 7, "config": null}]),
        ),
        schematic_op(
            "schematic.connect",
            json!({"from": {"instance": 7, "terminal": 26}, "to": {"instance": 1, "terminal": 1}}),
        ),
    ] {
        serde_json::from_value::<Operation>(operation.clone())
            .map_err(|e| format!("{operation}: {e}"))?;
    }
    for (name, params) in [
        ("functions.suggest", suggest_params(&instance, 3, true)),
        ("project.pin_report", json!({"instance": instance})),
        ("project.pin_config", json!({"instance": instance})),
    ] {
        serde_json::from_value::<Query>(json!({"query": name, "params": params}))
            .map_err(|e| format!("{name}: {e}"))?;
    }
    for emitter in ["json", "c_header", "zephyr_overlay", "rust_consts"] {
        let request = firmware("firmware.export", &instance, emitter, Some("/tmp/x"));
        serde_json::from_value::<Request>(request.clone())
            .map_err(|e| format!("{request}: {e}"))?;
    }
    Ok(())
}

/// Firmware export of an instance with no peripherals: either the C4
/// stub's explicit refusal (nothing changes) or a pin-configuration
/// starting point with a suggested file name.
#[test]
fn firmware_export_of_a_plain_instance() -> Fallible<()> {
    finish((|| -> Fallible<()> {
        let mut s = Scenario::demo()?;
        let instance = json!(
            s.session
                .engine()
                .project()
                .instances
                .first()
                .map(|i| i.id)
                .ok_or("instance")?
        );
        let emitted = s.send(
            "export_preview",
            firmware("firmware.export_preview", &instance, "c_header", None),
        )?;
        assert!(
            emitted["file_name"]
                .as_str()
                .is_some_and(|n| n.ends_with(".h")),
            "{emitted}"
        );
        Ok(())
    })())
}

fn pin<'a>(report: &'a Value, name: &str) -> Fallible<&'a Value> {
    report["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == name)
        .ok_or_else(|| format!("pin {name} in {report}").into())
}

/// The firmware pins of the IR by terminal name.
fn fw_pin<'a>(ir: &'a Value, placed: &Placed, name: &str) -> Fallible<&'a Value> {
    let terminal = placed.imported.terminal(name)?;
    ir["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["terminal"] == terminal)
        .ok_or_else(|| format!("firmware pin {name} in {ir}").into())
}

/// Steps 9–19, after `pinout_tests::import_reviewed` and placement.
fn assign_report_export(s: &mut Scenario, placed: &Placed) -> Fallible<()> {
    let u1 = &placed.instance;
    let p = &placed.imported;
    let t = |name: &str| p.terminal(name);

    // 9. Suggest SPI1 on a fresh instance: ranked complete assignments on
    // the SPI1 pins, nothing applied.
    let report = s.query(
        "suggest SPI1",
        "functions.suggest",
        suggest_params(u1, p.peripheral("SPI1")?, true),
    )?;
    let suggestions = report["suggestions"].as_array().ok_or("suggestions")?;
    assert!(!suggestions.is_empty(), "{report}");
    let spi_pins = [t("PA4")?, t("PA5")?, t("PA6")?, t("PA7")?];
    for suggestion in suggestions {
        let changed = suggestion["changed"].as_array().ok_or("changed")?;
        assert!(
            changed.contains(&json!(t("PA5")?)),
            "SCK is required: {suggestion}"
        );
        assert!(
            changed.iter().all(|c| spi_pins.iter().any(|p| c == p)),
            "{suggestion}"
        );
    }

    // 10. USART1 on PA9/PA10, placed explicitly.
    let usart1 = p.peripheral("USART1")?;
    let assigned = s.apply(
        "assign USART1",
        assign_op(
            u1,
            usart1,
            &[
                (p.role("USART1", "tx")?, Some(t("PA9")?)),
                (p.role("USART1", "rx")?, Some(t("PA10")?)),
            ],
        ),
    )?;
    assert_eq!(assigned["outcome"]["outcome"], "functions_selected");
    // 11. ADC1 IN0 on PA0, and I2C1 wherever the solver puts it.
    s.apply(
        "assign ADC1",
        assign_op(
            u1,
            p.peripheral("ADC1")?,
            &[(p.role("ADC1", "in0")?, Some(t("PA0")?))],
        ),
    )?;
    s.apply("assign I2C1", assign_op(u1, p.peripheral("I2C1")?, &[]))?;

    // 12. The datasheet rule now forbids SPI1: the suggestion names it and
    // the assignment is refused whole, nothing changed.
    let report = s.query(
        "suggest SPI1 blocked",
        "functions.suggest",
        suggest_params(u1, p.peripheral("SPI1")?, true),
    )?;
    assert!(
        report["suggestions"].as_array().is_some_and(Vec::is_empty),
        "{report}"
    );
    let blockers = report["blockers"].as_array().ok_or("blockers")?;
    assert!(
        blockers.iter().any(|b| b["message"]
            .as_str()
            .is_some_and(|m| m.contains("sampling switch"))),
        "the forbidding rule is named: {report}"
    );
    let refused = s.refuse_apply(
        "assign SPI1 forbidden",
        assign_op(u1, p.peripheral("SPI1")?, &[]),
        "conflict",
    )?;
    assert!(refused.to_string().contains("SPI1"), "{refused}");

    // 13. Pin configuration, checked against the declared options.
    s.apply(
        "pin config",
        pin_config_op(
            u1,
            json!([{"terminal": t("PA10")?, "config": {"pull": "up", "name": "UART_RX"}}]),
        ),
    )?;
    s.refuse_apply(
        "undeclared speed",
        pin_config_op(
            u1,
            json!([{"terminal": t("PA10")?, "config": {"speed": "ludicrous"}}]),
        ),
        "invalid_input",
    )?;
    s.refuse_apply(
        "option on a pin without options",
        pin_config_op(
            u1,
            json!([{"terminal": t("NRST")?, "config": {"pull": "up"}}]),
        ),
        "invalid_input",
    )?;

    // 14. Wire USART1 TX to R1 and name the net.
    let r1 = json!(
        s.session
            .engine()
            .project()
            .instances
            .iter()
            .find(|i| i.designator == "R1")
            .map(|i| i.id)
            .ok_or("R1")?
    );
    let wired = s.apply(
        "connect",
        schematic_op(
            "schematic.connect",
            json!({"from": {"instance": u1, "terminal": t("PA9")?},
                   "to": {"instance": r1, "terminal": 1}}),
        ),
    )?;
    let net = wired["project"]["nets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| {
            n["terminals"].as_array().is_some_and(|ts| {
                ts.contains(&json!({"instance": u1, "terminal": t("PA9").unwrap_or(0)}))
            })
        })
        .map(|n| n["id"].clone())
        .ok_or("the PA9 net")?;
    s.apply(
        "net name",
        schematic_op(
            "schematic.net_rename",
            json!({"net": net, "name": "UART_TX"}),
        ),
    )?;

    // 15. The MCU-to-peripheral report: facts per pin, and every other
    // endpoint on the net with its component.
    let report = s.query("pin report", "project.pin_report", json!({"instance": u1}))?;
    assert_eq!(report["designator"], placed.designator);
    assert_eq!(report["pins"].as_array().map(Vec::len), Some(49));
    let pa9 = pin(&report, "PA9")?;
    assert_eq!(pa9["gpio"], json!({"port": "A", "index": 9}));
    let function = &pa9["function"];
    assert_eq!(function["label"], "usart1_tx");
    assert_eq!(function["peripheral"], usart1);
    assert_eq!(function["peripheral_label"], "USART1");
    assert_eq!(function["peripheral_kind"], "uart");
    assert_eq!(function["role_label"], "tx");
    assert_eq!(function["signal"], "TX");
    assert_eq!(function["mux"], "AF7");
    assert_eq!(pa9["net"], net);
    assert_eq!(pa9["net_name"], "UART_TX");
    let endpoints = pa9["endpoints"].as_array().ok_or("endpoints")?;
    assert_eq!(endpoints.len(), 1, "{pa9}");
    assert_eq!(endpoints[0]["designator"], "R1");
    assert_eq!(endpoints[0]["terminal"], 1);
    assert_eq!(
        pin(&report, "PA10")?["config"],
        json!({"pull": "up", "name": "UART_RX"})
    );
    assert_eq!(pin(&report, "PA0")?["function"]["peripheral_label"], "ADC1");
    let scl = pin(&report, "PB6")?["function"]["label"] == "i2c1_scl";
    let sda = pin(&report, "PB7")?["function"]["label"] == "i2c1_sda";
    let alt_scl = pin(&report, "PB8")?["function"]["label"] == "i2c1_scl";
    let alt_sda = pin(&report, "PB9")?["function"]["label"] == "i2c1_sda";
    assert!(
        (scl && sda && !alt_scl && !alt_sda) || (alt_scl && alt_sda && !scl && !sda),
        "I2C1 on one complete pin pair: {report}"
    );
    assert!(pin(&report, "PA12")?["net"].is_null());

    // 16. The firmware IR: format 1, the enabled peripherals, names from
    // the net or the override.
    let ir = s.query(
        "pin config IR",
        "project.pin_config",
        json!({"instance": u1}),
    )?;
    assert_eq!(ir["format"], 1);
    assert_eq!(ir["device"]["designator"], placed.designator);
    let enabled: Vec<&str> = ir["peripherals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["label"].as_str())
        .collect();
    for label in ["USART1", "ADC1", "I2C1"] {
        assert!(enabled.contains(&label), "{label} enabled: {ir}");
    }
    for label in ["SPI1", "USART2", "I2C2", "TIM1"] {
        assert!(!enabled.contains(&label), "{label} not enabled: {ir}");
    }
    let tx = fw_pin(&ir, placed, "PA9")?;
    assert_eq!(tx["firmware_name"], "UART_TX");
    assert_eq!(tx["mux"], "AF7");
    assert_eq!(tx["peripheral"], "USART1");
    assert_eq!(tx["peripheral_kind"], "uart");
    assert_eq!(tx["role"], "tx");
    assert_eq!(tx["gpio"], json!({"port": "A", "index": 9}));
    let rx = fw_pin(&ir, placed, "PA10")?;
    assert_eq!(rx["firmware_name"], "UART_RX");
    assert_eq!(rx["pull"], "up");
    assert!(
        ir["name_conflicts"].as_array().is_some_and(Vec::is_empty),
        "{ir}"
    );

    // 17. A clashing name is reported, never renamed silently.
    s.apply(
        "clashing name",
        pin_config_op(
            u1,
            json!([{"terminal": t("PA0")?, "config": {"name": "UART_TX"}}]),
        ),
    )?;
    let clash = s.query(
        "pin config clash",
        "project.pin_config",
        json!({"instance": u1}),
    )?;
    let conflicts = clash["name_conflicts"].as_array().ok_or("name_conflicts")?;
    assert_eq!(conflicts.len(), 1, "{clash}");
    assert_eq!(conflicts[0]["name"], "UART_TX");
    let mut terminals: Vec<u64> = conflicts[0]["terminals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    terminals.sort_unstable();
    let mut expected = vec![t("PA0")?, t("PA9")?];
    expected.sort_unstable();
    assert_eq!(terminals, expected);
    assert_eq!(fw_pin(&clash, placed, "PA0")?["firmware_name"], "UART_TX");
    assert_eq!(fw_pin(&clash, placed, "PA9")?["firmware_name"], "UART_TX");
    s.apply(
        "clear clash",
        pin_config_op(u1, json!([{"terminal": t("PA0")?, "config": null}])),
    )?;

    // 18. The emitters over the same IR. Previews change nothing.
    let revision = s.revision();
    let json_text = s.send(
        "emit json",
        firmware("firmware.export_preview", u1, "json", None),
    )?;
    let reparsed: Value = serde_json::from_str(json_text["text"].as_str().ok_or("text")?)?;
    assert_eq!(reparsed, ir, "the json emitter is the IR itself");
    let header = s.send(
        "emit c header",
        firmware("firmware.export_preview", u1, "c_header", None),
    )?;
    assert!(
        header["file_name"]
            .as_str()
            .is_some_and(|n| n.ends_with(".h"))
    );
    let text = header["text"].as_str().ok_or("text")?;
    for needle in ["UART_TX_PORT", "UART_TX_PIN", "UART_TX_AF", "UART_RX_PIN"] {
        assert!(text.contains(needle), "{needle} in {text}");
    }
    let overlay = s.send(
        "emit zephyr",
        firmware("firmware.export_preview", u1, "zephyr_overlay", None),
    )?;
    assert!(
        overlay["file_name"]
            .as_str()
            .is_some_and(|n| n.ends_with(".overlay"))
    );
    let text = overlay["text"].as_str().ok_or("text")?;
    assert!(text.contains("okay"), "{text}");
    assert!(text.to_lowercase().contains("usart1"), "{text}");
    assert!(text.to_lowercase().contains("starting point"), "{text}");
    let consts = s.send(
        "emit rust",
        firmware("firmware.export_preview", u1, "rust_consts", None),
    )?;
    assert!(
        consts["text"]
            .as_str()
            .is_some_and(|t| t.contains("UART_TX"))
    );
    assert_eq!(s.revision(), revision, "previews are not document changes");

    // 19. Export writes a new file; an existing file is refused untouched.
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("pins.h");
    let path_text = path.to_str().ok_or("utf-8 path")?;
    s.send(
        "export",
        firmware("firmware.export", u1, "c_header", Some(path_text)),
    )?;
    assert_eq!(
        std::fs::read_to_string(&path)?,
        header["text"].as_str().unwrap_or_default()
    );
    std::fs::write(&path, "kept")?;
    let again = s.send(
        "export again",
        firmware("firmware.export", u1, "c_header", Some(path_text)),
    );
    match again {
        Ok(result) => return Err(format!("overwrote an existing file: {result}").into()),
        Err(error) if error.is::<Stub>() => return Err(error),
        Err(_) => {}
    }
    assert_eq!(std::fs::read_to_string(&path)?, "kept");
    assert_eq!(s.revision(), revision, "exports are not document changes");
    Ok(())
}

/// The whole datasheet-to-firmware scenario through JSON only.
#[test]
fn exm48_from_datasheet_to_firmware_pins() -> Fallible<()> {
    finish((|| -> Fallible<()> {
        let mut s = Scenario::demo()?;
        let imported = import_reviewed(&mut s)?;
        let placed = publish_and_place(&mut s, imported)?;
        assign_report_export(&mut s, &placed)
    })())
}
