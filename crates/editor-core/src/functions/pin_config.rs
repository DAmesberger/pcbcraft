//! Slice C2: per-terminal data a pin carries beyond its
//! electrical functions (mux codes, GPIO identity, settable non-electrical
//! options) and the per-instance pin configuration that selects among
//! those options.
//!
//! Nothing here changes electrical behaviour: anything that does stays a
//! [`Function`](super::Function), so ERC never reads a [`PinConfig`].
//! Every value is explicit data; nothing is parsed out of names such as
//! `PA5` or `USART1_TX`.
use crate::diagnostics::{Error, Result};
use crate::ids::TerminalId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bytes of a [`MuxCode`].
pub const MAX_MUX_CODE: usize = 16;
/// Bytes of a function's mux table name (`Function::mux_table`).
pub const MAX_MUX_TABLE: usize = 32;
/// Bytes of a GPIO port name.
pub const MAX_GPIO_PORT: usize = 8;
/// Options one terminal declares.
pub const MAX_PIN_OPTIONS: usize = 8;
/// Allowed values of one enumerated option (speeds).
pub const MAX_OPTION_VALUES: usize = 16;
/// Bytes of a firmware name override.
pub const MAX_FIRMWARE_NAME: usize = 64;
/// Terminals one `instance.set_pin_config` edits.
pub const MAX_PIN_CONFIG_EDITS: usize = 256;

/// The vendor's alternate-function selector for one function, as opaque
/// data (`AF7`, `5`, `ALT2`): at most [`MAX_MUX_CODE`] bytes. Never
/// interpreted by the core.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct MuxCode(pub String);

/// Where an imported row came from in its reference document.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    /// Page of the reference document, 1-based; `None` when unknown.
    #[serde(default)]
    pub page: Option<u16>,
    /// Table or section label as printed ("Table 9").
    #[serde(default)]
    pub table: Option<String>,
}

/// A terminal's GPIO identity, stated explicitly: never parsed from a
/// terminal name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GpioRef {
    /// Port name, 1–[`MAX_GPIO_PORT`] bytes ("A", "P0", "GPIO1").
    pub port: String,
    pub index: u16,
}

/// Internal pull resistor setting.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Pull {
    None,
    Up,
    Down,
}

/// Initial output level.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Low,
    High,
}

/// A settable non-electrical option of a terminal and its allowed values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "option", content = "allowed", rename_all = "snake_case")]
pub enum PinOption {
    Pull(Vec<Pull>),
    /// Vendor speed grades as data ("low", "high", "very_high").
    Speed(Vec<String>),
    InitialLevel,
    Schmitt,
}

/// One instance's configuration of one terminal. Validated against the
/// terminal's [`PinOption`]s by `instance.set_pin_config`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<Pull>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schmitt: Option<bool>,
    /// Firmware identifier override; a named net's name is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// An instance's pin configuration by terminal.
pub type PinConfigs = BTreeMap<TerminalId, PinConfig>;

/// One `instance.set_pin_config` edit: `None` clears the terminal's
/// configuration.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinConfigEdit {
    pub terminal: TerminalId,
    pub config: Option<PinConfig>,
}

/// Persistence bounds of a terminal's C2 data (called by
/// `TerminalFunctions::within_bounds`).
pub fn terminal_within_bounds(gpio: Option<&GpioRef>, options: &[PinOption]) -> bool {
    gpio.is_none_or(|g| g.port.len() <= MAX_GPIO_PORT)
        && options.len() <= MAX_PIN_OPTIONS
        && options.iter().all(|o| match o {
            PinOption::Pull(values) => values.len() <= MAX_OPTION_VALUES,
            PinOption::Speed(values) => {
                values.len() <= MAX_OPTION_VALUES
                    && values.iter().all(|v| v.len() <= crate::ids::MAX_LABEL)
            }
            PinOption::InitialLevel | PinOption::Schmitt => true,
        })
}

/// Persistence bounds of a function's C2/C3 data (called by
/// `TerminalFunctions::within_bounds`).
pub fn function_within_bounds(
    mux: Option<&MuxCode>,
    table: Option<&str>,
    source: Option<&SourceSpan>,
) -> bool {
    mux.is_none_or(|m| m.0.len() <= MAX_MUX_CODE)
        && table.is_none_or(|t| t.len() <= MAX_MUX_TABLE)
        && source.is_none_or(|s| {
            s.table
                .as_ref()
                .is_none_or(|t| t.len() <= crate::ids::MAX_LABEL)
        })
}

/// Whether `name` is a usable firmware identifier override: 1–
/// [`MAX_FIRMWARE_NAME`] bytes of ASCII letters, digits and `_`, not
/// starting with a digit. The exporter never renames it.
pub fn firmware_name_valid(name: &str) -> bool {
    name.len() <= MAX_FIRMWARE_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.bytes().next().is_some_and(|b| !b.is_ascii_digit())
}

/// Whether a configuration sets nothing (it is stored as no entry).
pub fn config_is_empty(config: &PinConfig) -> bool {
    *config == PinConfig::default()
}

fn pull_label(pull: Pull) -> &'static str {
    match pull {
        Pull::None => "none",
        Pull::Up => "up",
        Pull::Down => "down",
    }
}

/// Check `config` against a terminal's declared options: each set value
/// needs its option declared and, for pull and speed, a value among the
/// allowed ones; a firmware name must be an identifier
/// ([`firmware_name_valid`]). Errors are `invalid_input` and name the
/// field; the caller adds the terminal.
pub fn check_config(options: &[PinOption], config: &PinConfig) -> Result<()> {
    if let Some(pull) = config.pull {
        let allowed = options.iter().find_map(|o| match o {
            PinOption::Pull(values) => Some(values),
            _ => None,
        });
        match allowed {
            None => {
                return Err(Error::Invalid(
                    "it declares no pull option, so a pull cannot be set".into(),
                ));
            }
            Some(values) if !values.contains(&pull) => {
                let shown: Vec<&str> = values.iter().map(|p| pull_label(*p)).collect();
                return Err(Error::Invalid(format!(
                    "pull ‘{}’ is not offered (allowed: {})",
                    pull_label(pull),
                    if shown.is_empty() {
                        "none listed".into()
                    } else {
                        shown.join(", ")
                    }
                )));
            }
            Some(_) => {}
        }
    }
    if let Some(speed) = &config.speed {
        let allowed = options.iter().find_map(|o| match o {
            PinOption::Speed(values) => Some(values),
            _ => None,
        });
        match allowed {
            None => {
                return Err(Error::Invalid(
                    "it declares no speed option, so a speed cannot be set".into(),
                ));
            }
            Some(values) if !values.iter().any(|v| v == speed) => {
                let shown =
                    if speed.len() <= MAX_FIRMWARE_NAME && !speed.chars().any(char::is_control) {
                        format!("speed ‘{speed}’")
                    } else {
                        "the speed".into()
                    };
                return Err(Error::Invalid(format!(
                    "{shown} is not offered (allowed: {})",
                    if values.is_empty() {
                        "none listed".into()
                    } else {
                        values.join(", ")
                    }
                )));
            }
            Some(_) => {}
        }
    }
    if config.initial.is_some() && !options.contains(&PinOption::InitialLevel) {
        return Err(Error::Invalid(
            "it declares no initial-level option, so an initial level cannot be set".into(),
        ));
    }
    if config.schmitt.is_some() && !options.contains(&PinOption::Schmitt) {
        return Err(Error::Invalid(
            "it declares no Schmitt-trigger option, so it cannot be set".into(),
        ));
    }
    if let Some(name) = &config.name
        && !firmware_name_valid(name)
    {
        return Err(Error::Invalid(format!(
            "the firmware name must be 1–{MAX_FIRMWARE_NAME} ASCII letters, digits or ‘_’ and not start with a digit"
        )));
    }
    Ok(())
}

/// Persistence bounds of one stored configuration (document validation).
pub fn config_within_bounds(config: &PinConfig) -> bool {
    config
        .speed
        .as_ref()
        .is_none_or(|s| s.len() <= crate::ids::MAX_LABEL)
        && config
            .name
            .as_ref()
            .is_none_or(|n| n.len() <= MAX_FIRMWARE_NAME)
}

/// What is wrong with a component's pin data. Publication reports each as
/// a blocking finding (`function.mux`, `function.gpio`,
/// `function.pin_option`, `resource.kind`, `resource.signal`) on the
/// terminal, function or resource local ID it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinDataProblem {
    /// The finding code publication would use.
    pub code: &'static str,
    /// The local ID (terminal, function or resource) concerned.
    pub local: u32,
    pub message: String,
}

fn text_ok(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

/// Check the C2 data of a component's terminals and resources: mux codes
/// and GPIO ports are non-blank printable data within their bounds, one
/// GPIO identity belongs to one terminal, each option kind is declared
/// once with a non-empty set of distinct values, and peripheral kinds and
/// role signals are printable. Nothing is parsed out of names.
pub fn declaration_problems(
    terminals: &[super::TerminalDeclaration<'_>],
    resources: &[super::Resource],
) -> Vec<PinDataProblem> {
    let mut out = Vec::new();
    let mut gpios: BTreeMap<(&str, u16), u32> = BTreeMap::new();
    for declaration in terminals {
        let Some(declared) = declaration.functions else {
            continue;
        };
        let local = declaration.terminal.0;
        let name = declaration.name.trim();
        for function in &declared.functions {
            if let Some(mux) = &function.mux
                && (!text_ok(&mux.0, MAX_MUX_CODE) || mux.0.chars().any(char::is_whitespace))
            {
                out.push(PinDataProblem {
                    code: "function.mux",
                    local: function.id.0,
                    message: format!(
                        "Function ‘{}’ of terminal ‘{name}’ needs a mux code of 1–{MAX_MUX_CODE} bytes without spaces.",
                        function.label
                    ),
                });
            }
            if let Some(table) = &function.mux_table
                && !text_ok(table, MAX_MUX_TABLE)
            {
                out.push(PinDataProblem {
                    code: "function.mux_table",
                    local: function.id.0,
                    message: format!(
                        "Function ‘{}’ of terminal ‘{name}’ needs a mux table name of 1–{MAX_MUX_TABLE} bytes.",
                        function.label
                    ),
                });
            }
        }
        if let Some(gpio) = &declared.gpio {
            if !text_ok(&gpio.port, MAX_GPIO_PORT) || gpio.port.chars().any(char::is_whitespace) {
                out.push(PinDataProblem {
                    code: "function.gpio",
                    local,
                    message: format!(
                        "Terminal ‘{name}’ needs a GPIO port of 1–{MAX_GPIO_PORT} bytes without spaces."
                    ),
                });
            } else if let Some(first) = gpios.insert((gpio.port.as_str(), gpio.index), local) {
                out.push(PinDataProblem {
                    code: "function.gpio",
                    local,
                    message: format!(
                        "Terminal ‘{name}’ repeats GPIO {}·{} of terminal {first}.",
                        gpio.port, gpio.index
                    ),
                });
            }
        }
        let mut kinds = std::collections::BTreeSet::new();
        for option in &declared.options {
            let (kind, ok) = match option {
                PinOption::Pull(values) => {
                    let mut seen = Vec::new();
                    let distinct = values.iter().all(|v| {
                        let fresh = !seen.contains(v);
                        seen.push(*v);
                        fresh
                    });
                    ("pull", !values.is_empty() && distinct)
                }
                PinOption::Speed(values) => {
                    let mut seen = std::collections::BTreeSet::new();
                    let ok = !values.is_empty()
                        && values
                            .iter()
                            .all(|v| text_ok(v, crate::ids::MAX_LABEL) && seen.insert(v.as_str()));
                    ("speed", ok)
                }
                PinOption::InitialLevel => ("initial level", true),
                PinOption::Schmitt => ("Schmitt trigger", true),
            };
            if !kinds.insert(kind) {
                out.push(PinDataProblem {
                    code: "function.pin_option",
                    local,
                    message: format!("Terminal ‘{name}’ declares its {kind} option twice."),
                });
            } else if !ok {
                out.push(PinDataProblem {
                    code: "function.pin_option",
                    local,
                    message: format!(
                        "The {kind} option of terminal ‘{name}’ needs at least one value and no repeats."
                    ),
                });
            }
        }
    }
    for resource in resources {
        if let Some(kind) = &resource.kind
            && !text_ok(kind, super::MAX_KIND)
        {
            out.push(PinDataProblem {
                code: "resource.kind",
                local: resource.id.0,
                message: format!(
                    "Peripheral ‘{}’ needs a kind of 1–{} printable bytes, or none.",
                    resource.label,
                    super::MAX_KIND
                ),
            });
        }
        for role in &resource.roles {
            if let Some(signal) = &role.signal
                && !text_ok(signal, super::MAX_KIND)
            {
                out.push(PinDataProblem {
                    code: "resource.signal",
                    local: role.id.0,
                    message: format!(
                        "Role ‘{}’ of peripheral ‘{}’ needs a signal name of 1–{} printable bytes, or none.",
                        role.label,
                        resource.label,
                        super::MAX_KIND
                    ),
                });
            }
        }
    }
    out
}
