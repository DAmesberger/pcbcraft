//! Emitter tests over an original fixture: an invented EXM-48 controller
//! with a USART, a plain LED output and a button input.
use super::*;
use pcbcraft_core::firmware::{
    FirmwareDevice, FirmwarePeripheral, FirmwarePin, NameConflict, PinConfigReport,
};
use pcbcraft_core::functions::{GpioRef, Level, Pull};
use pcbcraft_core::ids::{ComponentRef, DefinitionId, ResourceId, Revision, TerminalId};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn pin(terminal: u32, name: &str, function: &str, firmware_name: &str, index: u16) -> FirmwarePin {
    FirmwarePin {
        terminal: TerminalId(terminal),
        name: name.into(),
        gpio: Some(GpioRef {
            port: "A".into(),
            index,
        }),
        function: function.into(),
        peripheral: None,
        peripheral_kind: None,
        role: None,
        signal: None,
        mux: None,
        pull: None,
        speed: None,
        initial: None,
        firmware_name: firmware_name.into(),
        assigned: true,
        endpoints: vec![],
    }
}

fn uart(terminal: u32, name: &str, role: &str, firmware_name: &str, index: u16) -> FirmwarePin {
    FirmwarePin {
        peripheral: Some("USART1".into()),
        peripheral_kind: Some("uart".into()),
        role: Some(role.into()),
        signal: Some(role.to_uppercase()),
        mux: Some("AF7".into()),
        ..pin(
            terminal,
            name,
            &format!("usart1_{role}"),
            firmware_name,
            index,
        )
    }
}

fn report() -> PinConfigReport {
    let mut led = pin(5, "PA5", "gpio_out", "LED_STATUS", 5);
    led.initial = Some(Level::Low);
    led.speed = Some("low".into());
    let mut button = pin(6, "PA6", "gpio_in", "BUTTON", 6);
    button.pull = Some(Pull::Up);
    let mut rx = uart(10, "PA10", "rx", "CONSOLE_RX", 10);
    rx.pull = Some(Pull::Up);
    PinConfigReport {
        format: PIN_CONFIG_FORMAT,
        device: FirmwareDevice {
            instance: 12,
            designator: "U1".into(),
            component: ComponentRef {
                id: DefinitionId(3),
                revision: Revision(1),
            },
            component_name: "EXM-48 controller */ \"quoted\"".into(),
            part: None,
            mpn: Some("EXM48-Q".into()),
        },
        peripherals: vec![FirmwarePeripheral {
            peripheral: ResourceId(7),
            label: "USART1".into(),
            kind: Some("uart".into()),
            roles: vec!["tx".into(), "rx".into()],
        }],
        pins: vec![led, button, uart(9, "PA9", "tx", "CONSOLE_TX", 9), rx],
        name_conflicts: vec![],
    }
}

#[test]
fn json_is_the_ir_itself() -> Fallible<()> {
    let ir = report();
    let emitted = emit(&ir, &Emitter::Json, None)?;
    assert_eq!(emitted.file_name, "pins.json");
    assert_eq!(serde_json::from_str::<PinConfigReport>(&emitted.text)?, ir);
    Ok(())
}

#[test]
fn the_c_header_defines_ports_pins_and_mux_codes() -> Fallible<()> {
    let emitted = emit(&report(), &Emitter::CHeader, None)?;
    let text = &emitted.text;
    assert_eq!(emitted.file_name, "pins.h");
    for line in [
        "#ifndef PCBCRAFT_PINS_U1_H",
        "#define LED_STATUS_PORT \"A\"",
        "#define LED_STATUS_PIN 5",
        "#define LED_STATUS_INITIAL PCBCRAFT_LEVEL_LOW",
        "#define BUTTON_PULL PCBCRAFT_PULL_UP",
        "#define CONSOLE_TX_AF \"AF7\"",
        "#define U1_PIN_COUNT 4",
        "static const struct pcbcraft_pin u1_pins[U1_PIN_COUNT] = {",
        "    { \"CONSOLE_RX\", \"PA10\", \"A\", 10, \"usart1_rx\", \"USART1\", \"rx\", \"AF7\", PCBCRAFT_PULL_UP, NULL, PCBCRAFT_LEVEL_UNSET },",
        "#endif /* PCBCRAFT_PINS_U1_H */",
        " * A starting point: pin configuration only; it configures no clocks or registers.",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in\n{text}"
        );
    }
    // A name never ends the comment early; LED has no mux code.
    assert_eq!(
        text.matches("*/").count(),
        text.matches("/*").count(),
        "{text}"
    );
    assert!(!text.contains("LED_STATUS_AF"));
    // Every macro is defined once.
    let mut defines: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("#define "))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    let all = defines.len();
    defines.sort_unstable();
    defines.dedup();
    assert_eq!(defines.len(), all);
    Ok(())
}

#[test]
fn the_zephyr_overlay_enables_peripherals_and_lists_plain_gpios() -> Fallible<()> {
    let emitted = emit(&report(), &Emitter::ZephyrOverlay, None)?;
    let text = &emitted.text;
    assert_eq!(emitted.file_name, "pins.overlay");
    for line in [
        " * Starting point; check node names against your board's HAL.",
        "&usart1 {",
        "    status = \"okay\";",
        "    pinctrl-0 = <&usart1_tx_pa9 &usart1_rx_pa10>;",
        "    pinctrl-names = \"default\";",
        "    zephyr,user {",
        "        led-status-gpios = <&gpioa 5 (GPIO_ACTIVE_HIGH)>;",
        "        button-gpios = <&gpioa 6 (GPIO_ACTIVE_HIGH | GPIO_PULL_UP)>;",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in\n{text}"
        );
    }
    assert_eq!(
        text.matches('{').count(),
        text.matches('}').count(),
        "{text}"
    );
    Ok(())
}

/// The fixture plus PC13 left on its default `gpio_in` and an SWD debug
/// port.
fn with_defaults() -> PinConfigReport {
    let mut ir = report();
    let mut idle = pin(13, "PC13", "gpio_in", "PC13", 13);
    idle.assigned = false;
    if let Some(gpio) = &mut idle.gpio {
        gpio.port = "C".into();
    }
    let mut swdio = pin(14, "PA13", "swd_swdio", "PA13", 13);
    swdio.peripheral = Some("SWD".into());
    swdio.peripheral_kind = Some("debug".into());
    swdio.role = Some("swdio".into());
    swdio.mux = Some("AF0".into());
    ir.peripherals.push(FirmwarePeripheral {
        peripheral: ResourceId(9),
        label: "SWD".into(),
        kind: Some("debug".into()),
        roles: vec!["swdio".into()],
    });
    ir.pins.push(idle);
    ir.pins.push(swdio);
    ir
}

#[test]
fn the_c_header_leaves_default_pins_out_and_gives_numbers() -> Fallible<()> {
    let text = emit(&with_defaults(), &Emitter::CHeader, None)?.text;
    for line in [
        " *   PC13: gpio_in",
        "#define CONSOLE_TX_PORT_INDEX 0",
        "#define CONSOLE_TX_AF_NUM 7",
        "#define PA13_AF_NUM 0",
        "#define U1_PIN_COUNT 5",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in\n{text}"
        );
    }
    assert!(!text.contains("#define PC13_"), "{text}");
    assert!(!text.contains("\"PC13\", \"PC13\""), "{text}");
    Ok(())
}

#[test]
fn the_zephyr_overlay_skips_debug_ports_and_default_pins_and_lists_pinctrl() -> Fallible<()> {
    let text = emit(&with_defaults(), &Emitter::ZephyrOverlay, None)?.text;
    assert!(!text.contains("&swd"), "{text}");
    assert!(text.contains("/* SWD (debug): swdio."), "{text}");
    assert!(!text.contains("pc13-gpios"), "{text}");
    for line in [
        " * TODO pinctrl: the labels above follow <peripheral>_<role>_p<port><index>.",
        " *   usart1_tx_pa9: PA9 as usart1_tx, mux AF7",
        " *   usart1_rx_pa10: PA10 as usart1_rx, mux AF7",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in\n{text}"
        );
    }
    // Every label referenced is listed.
    for label in text
        .split('&')
        .skip(1)
        .filter_map(|r| {
            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
        })
        .filter(|l| l.contains("_p"))
    {
        assert!(
            text.contains(&format!(" *   {label}: ")),
            "{label} in\n{text}"
        );
    }
    let rust = emit(&with_defaults(), &Emitter::RustConsts, None)?.text;
    assert!(!rust.contains("pub const PC13"), "{rust}");
    assert!(rust.contains("// Left on their default function, so no consts: PC13 (gpio_in)."));
    Ok(())
}

#[test]
fn rust_consts_declare_one_const_per_pin_and_a_table() -> Fallible<()> {
    let emitted = emit(&report(), &Emitter::RustConsts, None)?;
    let text = &emitted.text;
    assert_eq!(emitted.file_name, "pins.rs");
    for line in [
        "pub const LED_STATUS: Pin = Pin {",
        "    gpio: Some(Gpio { port: \"A\", index: 5 }),",
        "    initial: Some(Level::Low),",
        "    speed: Some(\"low\"),",
        "    mux: Some(\"AF7\"),",
        "    pull: Some(Pull::Up),",
        "pub const PINS: [Pin; 4] = [LED_STATUS, BUTTON, CONSOLE_TX, CONSOLE_RX];",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in\n{text}"
        );
    }
    // The quoted component name is escaped as a literal only, never code.
    assert!(text.starts_with("//! Pin configuration of U1 (EXM-48 controller * / \"quoted\")"));
    Ok(())
}

#[test]
fn literals_escape_user_text() {
    assert_eq!(
        text::c_string("a\"b\\c\n?é"),
        "\"a\\\"b\\\\c\\012\\?\\303\\251\""
    );
    assert_eq!(text::rust_string("a\"b\n"), "\"a\\\"b\\n\"");
    assert_eq!(text::comment("x */ y\nz"), "x * / y z");
}

#[test]
fn code_emitters_refuse_name_conflicts_but_json_carries_them() -> Fallible<()> {
    let mut ir = report();
    for p in &mut ir.pins {
        p.firmware_name = "SAME".into();
    }
    ir.name_conflicts = vec![NameConflict {
        name: "SAME".into(),
        terminals: ir.pins.iter().map(|p| p.terminal).collect(),
    }];
    for emitter in [
        Emitter::CHeader,
        Emitter::ZephyrOverlay,
        Emitter::RustConsts,
    ] {
        let result = emit(&ir, &emitter, None);
        assert!(
            matches!(&result, Err(Error::Conflict(m)) if m.contains("SAME (terminals 5, 6, 9, 10)")),
            "{emitter:?}: {result:?}"
        );
    }
    let json = emit(&ir, &Emitter::Json, None)?;
    assert!(json.text.contains("\"name_conflicts\""));
    Ok(())
}

#[test]
fn rust_consts_refuse_a_pin_named_like_the_table() {
    let mut ir = report();
    if let Some(p) = ir.pins.first_mut() {
        p.firmware_name = "PINS".into();
    }
    assert!(matches!(
        emit(&ir, &Emitter::RustConsts, None),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn another_ir_format_is_refused() {
    let mut ir = report();
    ir.format = 2;
    assert!(matches!(
        emit(&ir, &Emitter::Json, None),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn emitters_are_deterministic() -> Fallible<()> {
    let ir = report();
    for emitter in [
        Emitter::Json,
        Emitter::CHeader,
        Emitter::ZephyrOverlay,
        Emitter::RustConsts,
    ] {
        assert_eq!(emit(&ir, &emitter, None)?, emit(&ir, &emitter, None)?);
    }
    Ok(())
}

fn user(source: &str) -> Emitter {
    Emitter::User {
        template: template_hash(source),
    }
}

const TEMPLATE: &str = r#"
let lines = [`// ${comment(report.device.designator)}: ${report.pins.len()} pins`];
for p in report.pins {
    if p.gpio != () {
        lines.push(`#define ${p.firmware_name}_PIN ${p.gpio.index}`);
    }
    if p.mux != () {
        lines.push(`#define ${p.firmware_name}_AF ${c_string(p.mux)}`);
    }
}
#{ file_name: "board_pins.h", text: lines.reduce(|sum, l| sum + l + "\n", "") }
"#;

#[test]
fn a_user_template_builds_text_from_the_ir() -> Fallible<()> {
    let emitted = emit(&report(), &user(TEMPLATE), Some(TEMPLATE))?;
    assert_eq!(emitted.file_name, "board_pins.h");
    assert_eq!(
        emitted.text,
        "// U1: 4 pins\n#define LED_STATUS_PIN 5\n#define BUTTON_PIN 6\n\
         #define CONSOLE_TX_PIN 9\n#define CONSOLE_TX_AF \"AF7\"\n\
         #define CONSOLE_RX_PIN 10\n#define CONSOLE_RX_AF \"AF7\"\n"
    );
    let plain = r#"`${ident("led-1")} ${lower_ident("A b")} ${rust_string("q")}`"#;
    let emitted = emit(&report(), &user(plain), Some(plain))?;
    assert_eq!(emitted.file_name, template::DEFAULT_FILE_NAME);
    assert_eq!(emitted.text, "LED_1 a_b \"q\"");
    Ok(())
}

#[test]
fn a_user_template_must_match_its_hash() {
    let other = user("\"other\"");
    assert!(matches!(
        emit(&report(), &other, Some(TEMPLATE)),
        Err(Error::Invalid(m)) if m.contains("hash")
    ));
    assert!(matches!(
        emit(&report(), &user(TEMPLATE), None),
        Err(Error::Invalid(_))
    ));
}

/// A template source and the error it must end with.
type Case = (&'static str, fn(&Error) -> bool);

#[test]
fn user_templates_are_bounded_and_sandboxed() {
    let cases: [Case; 7] = [
        // Unbounded loop: operation budget.
        ("loop { }", |e| matches!(e, Error::Limit(_))),
        // Doubling a string past the output cap.
        (r#"let s = "x"; loop { s += s; }"#, |e| {
            matches!(e, Error::Limit(_))
        }),
        // Recursion depth.
        ("fn f(n) { f(n + 1) } f(0)", |e| {
            matches!(e, Error::Limit(_))
        }),
        // No eval, print, file or module access.
        (r#"eval("1")"#, |e| matches!(e, Error::Invalid(_))),
        (r#"print("x"); "" "#, |e| matches!(e, Error::Invalid(_))),
        (r#"import "fs" as fs; """#, |e| {
            matches!(e, Error::Invalid(_))
        }),
        // Wrong result shape and a path-like file name.
        (
            r#"#{ file_name: "../x.h", text: "" }"#,
            |e| matches!(e, Error::Invalid(m) if m.contains("file name")),
        ),
    ];
    for (source, expected) in cases {
        let result = emit(&report(), &user(source), Some(source));
        assert!(
            matches!(&result, Err(error) if expected(error)),
            "{source}: {result:?}"
        );
    }
    for source in [
        "42",
        "#{ text: 1 }",
        "#{ text: \"\", extra: 1 }",
        "undefined_var",
    ] {
        assert!(
            matches!(
                emit(&report(), &user(source), Some(source)),
                Err(Error::Invalid(_))
            ),
            "{source}"
        );
    }
    let huge = "x".repeat(MAX_TEMPLATE_BYTES + 1);
    assert!(matches!(
        emit(&report(), &user(&huge), Some(&huge)),
        Err(Error::Limit(_))
    ));
}

#[test]
fn template_syntax_is_checked_without_running() {
    assert!(template::check("let x = 1; x").is_ok());
    assert!(matches!(template::check("let = ;"), Err(Error::Invalid(_))));
}
