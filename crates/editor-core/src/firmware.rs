//! Slice C4 (contract C0): the firmware pin-configuration IR (founder
//! decision F1: pin configuration only; no clock trees, register init or
//! HAL projects). `project.pin_config { instance }` produces a versioned,
//! JSON-schema'd [`PinConfigReport`]; emitters (C header, Zephyr overlay,
//! JSON, Rust consts, user templates) live in the `pcbcraft-firmware`
//! crate. Generated text is never imported back.
//!
//! Firmware names are the `PinConfig::name` or the net name, sanitised to
//! `[A-Z0-9_]`; a collision is reported in `name_conflicts`, never renamed
//! silently.
//!
//! The IR is built from `project.pin_report` (slice C2), so it reports the
//! same facts: it never claims compatibility, and matching labels never
//! count.
use crate::diagnostics::{Error, Result};
use crate::functions::{Level, Pull};
use crate::ids::{ComponentRef, Id, PartRef, ResourceId, RoleId, TerminalId};
use crate::model::Project;
use crate::pins::{PinReport, ReportEndpoint, ReportPin};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The only IR format this build writes.
pub const PIN_CONFIG_FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwareDevice {
    pub instance: Id,
    pub designator: String,
    pub component: ComponentRef,
    pub component_name: String,
    #[serde(default)]
    pub part: Option<PartRef>,
    #[serde(default)]
    pub mpn: Option<String>,
}

/// One enabled peripheral and its active roles.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwarePeripheral {
    pub peripheral: ResourceId,
    pub label: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub roles: Vec<String>,
}

/// One assigned pin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwarePin {
    pub terminal: TerminalId,
    pub name: String,
    #[serde(default)]
    pub gpio: Option<crate::functions::GpioRef>,
    pub function: String,
    #[serde(default)]
    pub peripheral: Option<String>,
    #[serde(default)]
    pub peripheral_kind: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub mux: Option<String>,
    #[serde(default)]
    pub pull: Option<Pull>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub initial: Option<Level>,
    /// Sanitised `[A-Z0-9_]` identifier.
    pub firmware_name: String,
    pub endpoints: Vec<ReportEndpoint>,
}

/// Two or more pins that sanitise to one firmware name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NameConflict {
    pub name: String,
    pub terminals: Vec<TerminalId>,
}

/// The IR every emitter reads.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinConfigReport {
    /// [`PIN_CONFIG_FORMAT`].
    pub format: u32,
    pub device: FirmwareDevice,
    pub peripherals: Vec<FirmwarePeripheral>,
    pub pins: Vec<FirmwarePin>,
    pub name_conflicts: Vec<NameConflict>,
}

/// Bytes of a sanitised firmware name ([`crate::functions::pin_config::MAX_FIRMWARE_NAME`]).
pub const MAX_FIRMWARE_NAME: usize = crate::functions::pin_config::MAX_FIRMWARE_NAME;

/// `project.pin_config`: the firmware IR of one instance, built from
/// `project.pin_report` (the one place nets, endpoints and active functions
/// are joined) plus the instance's device identity.
pub fn pin_config(project: &Project, instance: Id) -> Result<PinConfigReport> {
    let report = crate::pins::pin_report(project, instance)?;
    from_pin_report(project, &report)
}

/// Whether a reported pin belongs in the firmware IR: it has an active
/// function and something firmware configures (a GPIO identity, a
/// peripheral role, a mux code or a pin configuration). Fixed power and
/// passive terminals carry none of these and are left out.
fn configurable(pin: &ReportPin) -> bool {
    pin.function.as_ref().is_some_and(|f| {
        pin.gpio.is_some() || f.peripheral.is_some() || f.mux.is_some() || pin.config.is_some()
    })
}

/// The firmware IR of `report`'s instance. Pure: every value is copied from
/// the report or the pinned library data; nothing is inferred from names.
pub fn from_pin_report(project: &Project, report: &PinReport) -> Result<PinConfigReport> {
    let instance = project
        .instance(report.instance)
        .ok_or_else(|| Error::NotFound(format!("Instance {} does not exist.", report.instance)))?;
    let component = project
        .library
        .component(instance.component)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{} pins component {}, which is not in the project library.",
                instance.designator, instance.component
            ))
        })?;
    let purchasing = crate::completeness::purchasing(project, instance);
    let device = FirmwareDevice {
        instance: instance.id,
        designator: instance.designator.clone(),
        component: instance.component,
        component_name: component.meta.name.clone(),
        part: instance.part,
        mpn: purchasing.mpn,
    };

    let mut pins = Vec::new();
    // Active roles by peripheral, from the pins that take them.
    let mut active: BTreeMap<ResourceId, BTreeSet<RoleId>> = BTreeMap::new();
    for pin in report.pins.iter().filter(|p| configurable(p)) {
        let Some(function) = pin.function.as_ref() else {
            continue;
        };
        if let (Some(resource), Some(role)) = (function.peripheral, function.role) {
            active.entry(resource).or_default().insert(role);
        }
        let config = pin.config.as_ref();
        // A net's name counts only when someone named it: every placed
        // terminal sits on a net, and an unnamed one is called `N<id>`.
        let net_name = pin.net_name.as_deref().filter(|name| {
            pin.net
                .is_none_or(|net| *name != crate::nets::default_name(net))
        });
        let source = config
            .and_then(|c| c.name.as_deref())
            .or(net_name)
            .unwrap_or(&pin.name);
        pins.push(FirmwarePin {
            terminal: pin.terminal,
            name: pin.name.clone(),
            gpio: pin.gpio.clone(),
            function: function.label.clone(),
            peripheral: function.peripheral_label.clone(),
            peripheral_kind: function.peripheral_kind.clone(),
            role: function.role_label.clone(),
            signal: function.signal.clone(),
            mux: function.mux.clone(),
            pull: config.and_then(|c| c.pull),
            speed: config.and_then(|c| c.speed.clone()),
            initial: config.and_then(|c| c.initial),
            firmware_name: sanitise(source),
            endpoints: pin.endpoints.clone(),
        });
    }

    // Peripherals and roles in the component's declaration order.
    let peripherals = component
        .definition
        .resources
        .iter()
        .filter_map(|resource| {
            let roles = active.get(&resource.id)?;
            Some(FirmwarePeripheral {
                peripheral: resource.id,
                label: resource.label.clone(),
                kind: resource.kind.clone(),
                roles: resource
                    .roles
                    .iter()
                    .filter(|r| roles.contains(&r.id))
                    .map(|r| r.label.clone())
                    .collect(),
            })
        })
        .collect();

    let mut by_name: BTreeMap<&str, Vec<TerminalId>> = BTreeMap::new();
    for pin in &pins {
        by_name
            .entry(pin.firmware_name.as_str())
            .or_default()
            .push(pin.terminal);
    }
    let name_conflicts = by_name
        .into_iter()
        .filter(|(_, terminals)| terminals.len() > 1)
        .map(|(name, terminals)| NameConflict {
            name: name.to_owned(),
            terminals,
        })
        .collect();

    Ok(PinConfigReport {
        format: PIN_CONFIG_FORMAT,
        device,
        peripherals,
        pins,
        name_conflicts,
    })
}

/// Sanitise `text` to a firmware identifier of `[A-Z0-9_]`: ASCII letters
/// are upper-cased, every other character becomes `_`, runs of `_` collapse
/// and edge underscores are trimmed. A name that would start with a digit
/// (or is empty) gets the prefix `PIN_`. At most [`MAX_FIRMWARE_NAME`]
/// bytes; two sources that sanitise alike are reported as a
/// [`NameConflict`], never renamed.
pub fn sanitise(text: &str) -> String {
    let mut name = String::new();
    for c in text.chars() {
        let c = if c.is_ascii_alphanumeric() {
            c.to_ascii_uppercase()
        } else {
            '_'
        };
        if c == '_' && (name.is_empty() || name.ends_with('_')) {
            continue;
        }
        name.push(c);
    }
    while name.ends_with('_') {
        name.pop();
    }
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert_str(0, "PIN_");
    }
    name.truncate(MAX_FIRMWARE_NAME);
    while name.ends_with('_') {
        name.pop();
    }
    name
}

/// JSON schema of the IR.
pub fn schema() -> serde_json::Value {
    serde_json::json!(schemars::schema_for!(PinConfigReport))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use crate::functions::{GpioRef, PinConfig, Resource, Role};
    use crate::pins::{ReportFunction, ReportPin};

    type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn names_sanitise_to_upper_identifiers() {
        assert_eq!(sanitise("led_status"), "LED_STATUS");
        assert_eq!(sanitise("/uart tx"), "UART_TX");
        assert_eq!(sanitise("a--b__c."), "A_B_C");
        assert_eq!(sanitise("3V3_EN"), "PIN_3V3_EN");
        assert_eq!(sanitise("µ"), "PIN");
        assert_eq!(sanitise(""), "PIN");
        let long = sanitise(&"x".repeat(200));
        assert_eq!(long.len(), MAX_FIRMWARE_NAME);
        assert!(long.bytes().all(|b| b == b'X'));
    }

    fn function(label: &str) -> ReportFunction {
        ReportFunction {
            label: label.into(),
            peripheral: None,
            peripheral_label: None,
            peripheral_kind: None,
            role: None,
            role_label: None,
            signal: None,
            mux: None,
        }
    }

    fn uart(label: &str, role: u32, role_label: &str, mux: &str) -> ReportFunction {
        ReportFunction {
            peripheral: Some(ResourceId(7)),
            peripheral_label: Some("USART1".into()),
            peripheral_kind: Some("uart".into()),
            role: Some(RoleId(role)),
            role_label: Some(role_label.into()),
            signal: Some(role_label.to_uppercase()),
            mux: Some(mux.into()),
            ..function(label)
        }
    }

    fn pin(terminal: u32, name: &str, function: Option<ReportFunction>) -> ReportPin {
        ReportPin {
            terminal: TerminalId(terminal),
            name: name.into(),
            pads: vec![terminal.to_string()],
            gpio: Some(GpioRef {
                port: "A".into(),
                index: u16::try_from(terminal).unwrap_or_default(),
            }),
            function,
            config: None,
            net: None,
            net_name: None,
            endpoints: vec![],
        }
    }

    /// The demo project with its first component declaring a USART whose
    /// roles are declared rx before tx, so order follows the declaration.
    fn project() -> Fallible<(Project, Id)> {
        let mut project = Engine::demo()?.project().clone();
        let instance = project.instances.first().ok_or("instance")?;
        let (id, component) = (instance.id, instance.component);
        let published = project
            .library
            .components
            .iter_mut()
            .find(|c| c.id == component.id && c.revision == component.revision)
            .ok_or("component")?;
        let role = |id: u32, label: &str| Role {
            id: RoleId(id),
            label: label.into(),
            capacity: 1,
            required_when_used: false,
            signal: None,
        };
        published.definition.resources = vec![
            Resource {
                id: ResourceId(3),
                label: "SPI2".into(),
                kind: Some("spi".into()),
                roles: vec![role(1, "sck")],
            },
            Resource {
                id: ResourceId(7),
                label: "USART1".into(),
                kind: Some("uart".into()),
                roles: vec![role(2, "rx"), role(1, "tx"), role(3, "cts")],
            },
        ];
        Ok((project, id))
    }

    #[test]
    fn the_ir_copies_reported_facts_and_names_pins_explicitly() -> Fallible<()> {
        let (project, instance) = project()?;
        let mut tx = pin(9, "PA9", Some(uart("usart1_tx", 1, "tx", "AF7")));
        tx.net_name = Some("debug tx".into());
        let mut rx = pin(10, "PA10", Some(uart("usart1_rx", 2, "rx", "AF7")));
        rx.config = Some(PinConfig {
            pull: Some(Pull::Up),
            name: Some("console_rx".into()),
            ..PinConfig::default()
        });
        rx.net_name = Some("ignored because the config names it".into());
        let led = pin(5, "PA5", Some(function("gpio_out")));
        let mut power = pin(1, "VDD", Some(function("power_in")));
        power.gpio = None;
        let unresolved = pin(6, "PA6", None);
        let report = PinReport {
            instance,
            designator: "U1".into(),
            pins: vec![power, led, tx, rx, unresolved],
        };
        let ir = from_pin_report(&project, &report)?;
        assert_eq!(ir.format, PIN_CONFIG_FORMAT);
        let first = project.instances.first().ok_or("instance")?;
        assert_eq!(ir.device.designator, first.designator);
        assert_eq!(ir.device.component, first.component);
        assert_eq!(ir.device.mpn, None);

        let names: Vec<_> = ir.pins.iter().map(|p| p.firmware_name.as_str()).collect();
        assert_eq!(names, ["PA5", "DEBUG_TX", "CONSOLE_RX"]);
        let rx = ir.pins.get(2).ok_or("rx")?;
        assert_eq!(rx.pull, Some(Pull::Up));
        assert_eq!(rx.mux.as_deref(), Some("AF7"));
        assert_eq!(rx.peripheral.as_deref(), Some("USART1"));
        assert_eq!(rx.role.as_deref(), Some("rx"));
        assert_eq!(rx.gpio.as_ref().map(|g| g.index), Some(10));

        // Only the used peripheral, its active roles in declaration order.
        assert_eq!(ir.peripherals.len(), 1);
        let usart = ir.peripherals.first().ok_or("usart")?;
        assert_eq!(usart.label, "USART1");
        assert_eq!(usart.kind.as_deref(), Some("uart"));
        assert_eq!(usart.roles, ["rx", "tx"]);
        assert!(ir.name_conflicts.is_empty());
        Ok(())
    }

    #[test]
    fn an_unnamed_net_leaves_the_pin_name() -> Fallible<()> {
        let (project, instance) = project()?;
        let mut alone = pin(4, "PA4", Some(function("gpio_in")));
        alone.net = Some(33);
        alone.net_name = Some(crate::nets::default_name(33));
        let mut named = pin(5, "PA5", Some(function("gpio_out")));
        named.net = Some(34);
        named.net_name = Some("STATUS_LED".into());
        let report = PinReport {
            instance,
            designator: "U1".into(),
            pins: vec![alone, named],
        };
        let ir = from_pin_report(&project, &report)?;
        let names: Vec<_> = ir.pins.iter().map(|p| p.firmware_name.as_str()).collect();
        assert_eq!(names, ["PA4", "STATUS_LED"]);
        Ok(())
    }

    #[test]
    fn colliding_names_are_reported_not_renamed() -> Fallible<()> {
        let (project, instance) = project()?;
        let mut a = pin(2, "PA2", Some(function("gpio_out")));
        a.net_name = Some("led-1".into());
        let mut b = pin(3, "PA3", Some(function("gpio_out")));
        b.net_name = Some("LED_1".into());
        let report = PinReport {
            instance,
            designator: "U1".into(),
            pins: vec![a, b],
        };
        let ir = from_pin_report(&project, &report)?;
        let names: Vec<_> = ir.pins.iter().map(|p| p.firmware_name.as_str()).collect();
        assert_eq!(names, ["LED_1", "LED_1"]);
        assert_eq!(
            ir.name_conflicts,
            [NameConflict {
                name: "LED_1".into(),
                terminals: vec![TerminalId(2), TerminalId(3)],
            }]
        );
        Ok(())
    }

    #[test]
    fn an_unknown_instance_is_not_found() -> Fallible<()> {
        let (project, _) = project()?;
        let report = PinReport {
            instance: u64::MAX,
            ..PinReport::default()
        };
        assert!(matches!(
            from_pin_report(&project, &report),
            Err(Error::NotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn the_ir_round_trips_and_has_a_schema() -> Fallible<()> {
        let (project, instance) = project()?;
        let report = PinReport {
            instance,
            designator: "U1".into(),
            pins: vec![pin(9, "PA9", Some(uart("usart1_tx", 1, "tx", "AF7")))],
        };
        let ir = from_pin_report(&project, &report)?;
        let text = serde_json::to_string(&ir)?;
        assert_eq!(serde_json::from_str::<PinConfigReport>(&text)?, ir);
        let schema = schema();
        assert!(schema["properties"]["format"].is_object(), "{schema}");
        Ok(())
    }
}
