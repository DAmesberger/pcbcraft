//! `zephyr-overlay`: a devicetree overlay skeleton. One `&<peripheral>`
//! node per enabled peripheral with `status = "okay"` and a `pinctrl-0`
//! built from the template `<peripheral>_<role>_p<port><index>`, and the
//! assigned plain GPIOs as `<name>-gpios` properties of `/zephyr,user`.
//! Pins on their default function are left out. A peripheral of kind
//! `debug` (SWD, JTAG) gets no node: the SoC configures its debug port, so
//! it is listed in a comment only. Every pinctrl label the overlay uses is
//! listed in a TODO block with its pin and mux code, because SoC pinctrl
//! files define them for some vendors only. Node and pinctrl names differ
//! between SoC vendors; the file says it is a starting point to check
//! against the board's HAL.
use crate::text::{comment, lower_ident, preamble};
use pcbcraft_core::firmware::{FirmwarePin, PinConfigReport};
use pcbcraft_core::functions::Pull;
use std::fmt::Write;

fn pinctrl(peripheral: &str, pin: &FirmwarePin) -> Option<String> {
    let gpio = pin.gpio.as_ref()?;
    let role = pin.role.as_deref().unwrap_or(&pin.function);
    Some(format!(
        "{}_{}_p{}{}",
        lower_ident(peripheral),
        lower_ident(role),
        lower_ident(&gpio.port),
        gpio.index
    ))
}

pub fn emit(report: &PinConfigReport) -> String {
    let mut out = String::from("/*\n");
    for line in preamble(report) {
        let _ = writeln!(out, " * {line}");
    }
    out.push_str(
        " * Starting point; check node names against your board's HAL.\n */\n\n\
         #include <zephyr/dt-bindings/gpio/gpio.h>\n",
    );
    let mut labels: Vec<(String, &FirmwarePin)> = Vec::new();
    for peripheral in &report.peripherals {
        let pins: Vec<&FirmwarePin> = report
            .pins
            .iter()
            .filter(|p| p.peripheral.as_deref() == Some(peripheral.label.as_str()))
            .collect();
        if peripheral
            .kind
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case("debug"))
        {
            let _ = writeln!(
                out,
                "\n/* {} (debug): {}. The SoC configures its debug port; no node emitted. */",
                comment(&peripheral.label),
                comment(&peripheral.roles.join(", "))
            );
            continue;
        }
        let _ = write!(
            out,
            "\n/* {}{}: {} */\n&{} {{\n    status = \"okay\";\n",
            comment(&peripheral.label),
            peripheral
                .kind
                .as_deref()
                .map_or_else(String::new, |k| format!(" ({})", comment(k))),
            comment(&peripheral.roles.join(", ")),
            lower_ident(&peripheral.label)
        );
        let mut refs: Vec<String> = Vec::new();
        for pin in &pins {
            if let Some(label) = pinctrl(&peripheral.label, pin) {
                refs.push(format!("&{label}"));
                labels.push((label, pin));
            }
        }
        if !refs.is_empty() {
            let _ = writeln!(
                out,
                "    pinctrl-0 = <{}>;\n    pinctrl-names = \"default\";",
                refs.join(" ")
            );
        }
        for pin in pins.iter().filter(|p| p.gpio.is_none()) {
            let _ = writeln!(
                out,
                "    /* {} on {}: no GPIO identity, so no pinctrl entry */",
                comment(pin.role.as_deref().unwrap_or(&pin.function)),
                comment(&pin.name)
            );
        }
        out.push_str("};\n");
    }
    if !labels.is_empty() {
        out.push_str(
            "\n/*\n * TODO pinctrl: the labels above follow <peripheral>_<role>_p<port><index>.\n \
             * Some SoC pinctrl files (hal_stm32's, for one) define labels of this form;\n \
             * for any your SoC's files lack, define the group under &pinctrl:\n",
        );
        for (label, pin) in &labels {
            let _ = writeln!(
                out,
                " *   {label}: {} as {}{}",
                comment(&pin.name),
                comment(&pin.function),
                pin.mux
                    .as_deref()
                    .map_or_else(String::new, |m| format!(", mux {}", comment(m)))
            );
        }
        out.push_str(" */\n");
    }
    let plain: Vec<&FirmwarePin> = report
        .pins
        .iter()
        .filter(|p| p.peripheral.is_none() && p.gpio.is_some() && p.assigned)
        .collect();
    if !plain.is_empty() {
        out.push_str("\n/ {\n    zephyr,user {\n");
        for pin in plain {
            let Some(gpio) = &pin.gpio else { continue };
            let flags = match pin.pull {
                Some(Pull::Up) => "GPIO_ACTIVE_HIGH | GPIO_PULL_UP",
                Some(Pull::Down) => "GPIO_ACTIVE_HIGH | GPIO_PULL_DOWN",
                Some(Pull::None) | None => "GPIO_ACTIVE_HIGH",
            };
            let _ = writeln!(
                out,
                "        /* {}: {} */\n        {}-gpios = <&gpio{} {} ({flags})>;",
                comment(&pin.name),
                comment(&pin.function),
                lower_ident(&pin.firmware_name).replace('_', "-"),
                lower_ident(&gpio.port),
                gpio.index
            );
        }
        out.push_str("    };\n};\n");
    }
    out
}
