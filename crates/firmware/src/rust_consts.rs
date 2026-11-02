//! `rust-consts`: a `Pin` type and one `pub const` per assigned pin. Pins
//! on their default function are listed in a comment only.
use crate::text::{comment, preamble, rust_string};
use pcbcraft_core::firmware::PinConfigReport;
use pcbcraft_core::functions::{Level, Pull};
use pcbcraft_core::{Error, Result};
use std::fmt::Write;

/// The table's name; a pin may not take it.
const TABLE: &str = "PINS";

fn optional(value: Option<&String>) -> String {
    value.map_or_else(
        || "None".to_owned(),
        |v| format!("Some({})", rust_string(v)),
    )
}

pub fn emit(report: &PinConfigReport) -> Result<String> {
    if let Some(pin) = report.pins.iter().find(|p| p.firmware_name == TABLE) {
        return Err(Error::Conflict(format!(
            "the firmware name {TABLE} of terminal {} is the Rust pin table's name; set another pin-config name",
            pin.terminal
        )));
    }
    let mut out = String::new();
    for line in preamble(report) {
        let _ = writeln!(out, "//! {line}");
    }
    out.push_str(
        "
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    None,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Low,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gpio {
    pub port: &'static str,
    pub index: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub name: &'static str,
    pub terminal: &'static str,
    pub gpio: Option<Gpio>,
    pub function: &'static str,
    pub peripheral: Option<&'static str>,
    pub role: Option<&'static str>,
    pub mux: Option<&'static str>,
    /// The mux table `mux` belongs to when the part has several.
    pub mux_table: Option<&'static str>,
    pub pull: Option<Pull>,
    pub speed: Option<&'static str>,
    pub initial: Option<Level>,
}
",
    );
    let defaults: Vec<String> = report
        .pins
        .iter()
        .filter(|p| !p.assigned)
        .map(|p| format!("{} ({})", comment(&p.name), comment(&p.function)))
        .collect();
    if !defaults.is_empty() {
        let _ = writeln!(
            out,
            "\n// Left on their default function, so no consts: {}.",
            defaults.join(", ")
        );
    }
    for pin in report.pins.iter().filter(|p| p.assigned) {
        let gpio = pin.gpio.as_ref().map_or_else(
            || "None".to_owned(),
            |g| {
                format!(
                    "Some(Gpio {{ port: {}, index: {} }})",
                    rust_string(&g.port),
                    g.index
                )
            },
        );
        let pull = pin.pull.map_or("None", |p| match p {
            Pull::None => "Some(Pull::None)",
            Pull::Up => "Some(Pull::Up)",
            Pull::Down => "Some(Pull::Down)",
        });
        let initial = pin.initial.map_or("None", |l| match l {
            Level::Low => "Some(Level::Low)",
            Level::High => "Some(Level::High)",
        });
        let _ = write!(
            out,
            "
/// {}: {}.
pub const {}: Pin = Pin {{
    name: {},
    terminal: {},
    gpio: {gpio},
    function: {},
    peripheral: {},
    role: {},
    mux: {},
    mux_table: {},
    pull: {pull},
    speed: {},
    initial: {initial},
}};
",
            comment(&pin.name),
            comment(&pin.function),
            pin.firmware_name,
            rust_string(&pin.firmware_name),
            rust_string(&pin.name),
            rust_string(&pin.function),
            optional(pin.peripheral.as_ref()),
            optional(pin.role.as_ref()),
            optional(pin.mux.as_ref()),
            optional(pin.mux_table.as_ref()),
            optional(pin.speed.as_ref()),
        );
    }
    let names: Vec<&str> = report
        .pins
        .iter()
        .filter(|p| p.assigned)
        .map(|p| p.firmware_name.as_str())
        .collect();
    let _ = writeln!(
        out,
        "\n/// Every configured pin, in terminal order.\npub const {TABLE}: [Pin; {}] = [{}];",
        names.len(),
        names.join(", ")
    );
    Ok(out)
}
