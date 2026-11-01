//! Firmware export (C4) over the JSON request surface. C7 adds the
//! scenario (import EXM-48 → review → apply → assign → report → export)
//! once `project.pin_report` (C2) is in; these tests cover the export
//! itself over an original fixture IR.
use super::firmware::{FirmwareParams, export};
use super::*;
use pcbcraft_core::firmware::PinConfigReport;
use pcbcraft_firmware::{Emitter, template_hash};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// An invented EXM-48 controller with one USART pin and one LED.
fn report() -> Fallible<PinConfigReport> {
    let pin = |terminal: u32, name: &str, function: &str, firmware: &str| {
        json!({"terminal": terminal, "name": name, "gpio": {"port": "B", "index": terminal},
               "function": function, "firmware_name": firmware, "endpoints": []})
    };
    let mut tx = pin(6, "PB6", "usart1_tx", "CONSOLE_TX");
    tx["peripheral"] = json!("USART1");
    tx["role"] = json!("tx");
    tx["mux"] = json!("AF7");
    Ok(serde_json::from_value(json!({
        "format": 1,
        "device": {"instance": 4, "designator": "U1",
                   "component": {"id": 2, "revision": 1}, "component_name": "EXM-48"},
        "peripherals": [{"peripheral": 1, "label": "USART1", "kind": "uart", "roles": ["tx"]}],
        "pins": [pin(3, "PB3", "gpio_out", "LED"), tx],
        "name_conflicts": []
    }))?)
}

fn params(emitter: Emitter, path: Option<String>, template_file: Option<String>) -> FirmwareParams {
    FirmwareParams {
        instance: 4,
        emitter,
        path,
        template_file,
    }
}

#[test]
fn the_preview_returns_the_emitted_file() -> Fallible<()> {
    let reply = export(&report()?, &params(Emitter::CHeader, None, None), false)?;
    assert_eq!(reply["file_name"], "pins.h");
    let text = reply["text"].as_str().ok_or("text")?;
    assert!(text.contains("#define CONSOLE_TX_AF \"AF7\""), "{text}");
    assert!(text.contains("#define LED_PIN 3"), "{text}");
    Ok(())
}

#[test]
fn export_creates_a_new_file_and_refuses_an_existing_one() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.overlay");
    let shown = path.to_string_lossy().into_owned();
    let request = params(Emitter::ZephyrOverlay, Some(shown.clone()), None);
    let reply = export(&report()?, &request, true)?;
    assert_eq!(reply["path"], shown.as_str());
    let written = std::fs::read_to_string(&path)?;
    assert_eq!(reply["bytes"], written.len());
    assert!(written.contains("&usart1 {"), "{written}");

    let again = export(&report()?, &request, true);
    assert!(matches!(&again, Err(e) if e.to_string().contains("output exists")));
    assert_eq!(std::fs::read_to_string(&path)?, written);

    let no_path = export(&report()?, &params(Emitter::Json, None, None), true);
    assert!(matches!(&no_path, Err(e) if e.to_string().contains("needs path")));
    Ok(())
}

#[test]
fn a_user_template_file_must_match_the_emitter_hash() -> Fallible<()> {
    let dir = tempfile::tempdir()?;
    let source = "`${report.device.designator}: ${report.pins.len()} pins`";
    let file = dir.path().join("pins.rhai");
    std::fs::write(&file, source)?;
    let file = Some(file.to_string_lossy().into_owned());
    let user = Emitter::User {
        template: template_hash(source),
    };
    let reply = export(&report()?, &params(user.clone(), None, file.clone()), false)?;
    assert_eq!(reply["text"], "U1: 2 pins");

    let other = Emitter::User {
        template: template_hash("\"other\""),
    };
    assert!(export(&report()?, &params(other, None, file.clone()), false).is_err());
    assert!(export(&report()?, &params(user, None, None), false).is_err());
    assert!(export(&report()?, &params(Emitter::Json, None, file), false).is_err());
    Ok(())
}

#[test]
fn export_requests_never_change_the_document() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let instance = session
        .engine()
        .project()
        .instances
        .first()
        .map(|i| i.id)
        .ok_or("instance")?;
    let revision = session.engine().revision();
    let before = session.engine().to_file();
    for command in ["firmware.export_preview", "firmware.export"] {
        let reply = session.dispatch(json!({"command": command,
            "params": {"instance": instance, "emitter": {"emitter": "c_header"}}}));
        assert!(reply["ok"].is_boolean(), "{reply}");
    }
    let unknown = session.dispatch(json!({"command": "firmware.export_preview",
        "params": {"instance": instance, "emitter": {"emitter": "verilog"}}}));
    assert_eq!(unknown["error"]["code"], "malformed_request", "{unknown}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().to_file(), before);
    Ok(())
}
