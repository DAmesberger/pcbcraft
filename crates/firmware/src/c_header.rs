//! `c-header`: `#define <NAME>_PORT/_PIN/_AF` per pin and a pin table.
use crate::text::{c_string, comment, level_name, lower_ident, preamble, pull_name};
use pcbcraft_core::firmware::{PinConfigReport, sanitise};
use std::fmt::Write;

pub fn emit(report: &PinConfigReport) -> String {
    let device = sanitise(&report.device.designator);
    let guard = format!("PCBCRAFT_PINS_{device}_H");
    let table = lower_ident(&report.device.designator);
    let mut out = String::new();
    out.push_str("/*\n");
    for line in preamble(report) {
        let _ = writeln!(out, " * {line}");
    }
    out.push_str(" */\n");
    let _ = writeln!(out, "#ifndef {guard}\n#define {guard}\n");
    for pin in &report.pins {
        let name = &pin.firmware_name;
        let mut about = format!("{}: {}", comment(&pin.name), comment(&pin.function));
        if let (Some(peripheral), Some(role)) = (&pin.peripheral, &pin.role) {
            let _ = write!(about, " ({} {})", comment(peripheral), comment(role));
        }
        let _ = writeln!(out, "/* {about} */");
        match &pin.gpio {
            Some(gpio) => {
                let _ = writeln!(out, "#define {name}_PORT {}", c_string(&gpio.port));
                let _ = writeln!(out, "#define {name}_PIN {}", gpio.index);
            }
            None => out.push_str("/* no GPIO identity */\n"),
        }
        if let Some(mux) = &pin.mux {
            let _ = writeln!(out, "#define {name}_AF {}", c_string(mux));
        }
        if let Some(pull) = pin.pull {
            let _ = writeln!(out, "#define {name}_PULL PCBCRAFT_PULL_{}", pull_name(pull));
        }
        if let Some(level) = pin.initial {
            let _ = writeln!(
                out,
                "#define {name}_INITIAL PCBCRAFT_LEVEL_{}",
                level_name(level)
            );
        }
        out.push('\n');
    }
    out.push_str(
        "#ifndef PCBCRAFT_PIN_DEFINED
#define PCBCRAFT_PIN_DEFINED
enum pcbcraft_pull { PCBCRAFT_PULL_UNSET = -1, PCBCRAFT_PULL_NONE = 0, PCBCRAFT_PULL_UP = 1, PCBCRAFT_PULL_DOWN = 2 };
enum pcbcraft_level { PCBCRAFT_LEVEL_UNSET = -1, PCBCRAFT_LEVEL_LOW = 0, PCBCRAFT_LEVEL_HIGH = 1 };
/* port and mux are NULL and pin is -1 when the data does not state them. */
struct pcbcraft_pin {
    const char *name;
    const char *terminal;
    const char *port;
    int pin;
    const char *function;
    const char *peripheral;
    const char *role;
    const char *mux;
    enum pcbcraft_pull pull;
    const char *speed;
    enum pcbcraft_level initial;
};
#endif

",
    );
    let optional =
        |value: Option<&String>| value.map_or_else(|| "NULL".to_owned(), |v| c_string(v));
    let _ = writeln!(out, "#define {device}_PIN_COUNT {}", report.pins.len());
    if report.pins.is_empty() {
        let _ = writeln!(
            out,
            "/* {} has no configurable pins. */",
            comment(&report.device.designator)
        );
    } else {
        let _ = writeln!(
            out,
            "static const struct pcbcraft_pin {table}_pins[{device}_PIN_COUNT] = {{"
        );
        for pin in &report.pins {
            let (port, index) = pin.gpio.as_ref().map_or_else(
                || ("NULL".to_owned(), "-1".to_owned()),
                |g| (c_string(&g.port), g.index.to_string()),
            );
            let _ = writeln!(
                out,
                "    {{ {}, {}, {port}, {index}, {}, {}, {}, {}, PCBCRAFT_PULL_{}, {}, PCBCRAFT_LEVEL_{} }},",
                c_string(&pin.firmware_name),
                c_string(&pin.name),
                c_string(&pin.function),
                optional(pin.peripheral.as_ref()),
                optional(pin.role.as_ref()),
                optional(pin.mux.as_ref()),
                pin.pull.map_or("UNSET", pull_name),
                optional(pin.speed.as_ref()),
                pin.initial.map_or("UNSET", level_name),
            );
        }
        out.push_str("};\n");
    }
    let _ = writeln!(out, "\n#endif /* {guard} */");
    out
}
