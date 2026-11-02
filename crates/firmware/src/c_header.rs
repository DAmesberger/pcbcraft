//! `c-header`: `#define <NAME>_PORT/_PIN/_AF` per assigned pin and a pin
//! table. Pins that only carry their default function get no macros; a
//! comment lists them so the reader can tell them from assigned pins.
//!
//! `_PORT` and `_AF` are the library's identifiers as C strings. Where
//! they have a plain numeric reading, `_PORT_INDEX` (a one-letter port,
//! `A` = 0) and `_AF_NUM` (a mux code `AF<n>` or `<n>`) give it as a
//! number for preprocessor arithmetic and token pasting.
use crate::text::{c_string, comment, level_name, lower_ident, preamble, pull_name};
use pcbcraft_core::firmware::{FirmwarePin, PinConfigReport, sanitise};
use std::fmt::Write;

/// A one-letter port's index (`A` = 0 … `Z` = 25).
fn port_index(port: &str) -> Option<u8> {
    match port.as_bytes() {
        [letter] if letter.is_ascii_uppercase() => Some(letter - b'A'),
        _ => None,
    }
}

/// A mux code's number: `AF7` or `7` give 7.
fn af_number(mux: &str) -> Option<u32> {
    let digits = mux.strip_prefix("AF").unwrap_or(mux);
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

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
    let assigned: Vec<&FirmwarePin> = report.pins.iter().filter(|p| p.assigned).collect();
    let defaults: Vec<&FirmwarePin> = report.pins.iter().filter(|p| !p.assigned).collect();
    if !defaults.is_empty() {
        out.push_str("/* Left on their default function, so no macros:\n");
        for pin in &defaults {
            let _ = writeln!(
                out,
                " *   {}: {}",
                comment(&pin.name),
                comment(&pin.function)
            );
        }
        out.push_str(" */\n\n");
    }
    for pin in &assigned {
        let name = &pin.firmware_name;
        let mut about = format!("{}: {}", comment(&pin.name), comment(&pin.function));
        if let (Some(peripheral), Some(role)) = (&pin.peripheral, &pin.role) {
            let _ = write!(about, " ({} {})", comment(peripheral), comment(role));
        }
        let _ = writeln!(out, "/* {about} */");
        match &pin.gpio {
            Some(gpio) => {
                let _ = writeln!(out, "#define {name}_PORT {}", c_string(&gpio.port));
                if let Some(index) = port_index(&gpio.port) {
                    let _ = writeln!(out, "#define {name}_PORT_INDEX {index}");
                }
                let _ = writeln!(out, "#define {name}_PIN {}", gpio.index);
            }
            None => out.push_str("/* no GPIO identity */\n"),
        }
        if let Some(mux) = &pin.mux {
            let _ = writeln!(out, "#define {name}_AF {}", c_string(mux));
            if let Some(number) = af_number(mux) {
                let _ = writeln!(out, "#define {name}_AF_NUM {number}");
            }
        }
        if let Some(table) = &pin.mux_table {
            let _ = writeln!(out, "#define {name}_MUX_TABLE {}", c_string(table));
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
    let _ = writeln!(out, "#define {device}_PIN_COUNT {}", assigned.len());
    if assigned.is_empty() {
        let _ = writeln!(
            out,
            "/* {} has no assigned pins. */",
            comment(&report.device.designator)
        );
    } else {
        let _ = writeln!(
            out,
            "static const struct pcbcraft_pin {table}_pins[{device}_PIN_COUNT] = {{"
        );
        for pin in &assigned {
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
