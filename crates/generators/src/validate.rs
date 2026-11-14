//! Structural checks of a module's output before anyone uses it: every key
//! unique, every reference resolving, every value in range, the parts the
//! scope requires present and the others absent. The library's publish
//! rules then apply as to any authored part (slice GP1).
use sparksmith_generator_abi::Failure;
use sparksmith_generator_abi::v1::{
    Description, Effect, Footprint, GENERATE_PART, Generation, Graphic, HoleShape, Model, PadShape,
    ParamType, ParamValue, Point, Primitive, Scope, Symbol, limits,
};
use std::collections::BTreeSet;

fn invalid(message: String) -> Failure {
    Failure::one(
        "generator.invalid_output",
        format!("{message} Nothing was changed."),
    )
}

fn text(what: &str, s: &str) -> Result<(), Failure> {
    if s.chars().count() > limits::TEXT_CHARS {
        return Err(invalid(format!(
            "The generator returned a {what} longer than {} characters.",
            limits::TEXT_CHARS
        )));
    }
    Ok(())
}

fn key(what: &str, s: &str) -> Result<(), Failure> {
    if s.is_empty() {
        return Err(invalid(format!(
            "The generator returned an empty {what} key."
        )));
    }
    text(what, s)
}

fn unique<'a>(
    what: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<BTreeSet<&'a str>, Failure> {
    let mut seen = BTreeSet::new();
    for k in keys {
        key(what, k)?;
        if !seen.insert(k) {
            return Err(invalid(format!(
                "The generator returned {what} \"{k}\" twice."
            )));
        }
    }
    Ok(seen)
}

fn count(what: &str, n: usize, max: usize) -> Result<(), Failure> {
    if n > max {
        return Err(invalid(format!(
            "The generator returned {n} {what}, more than the {max} allowed."
        )));
    }
    Ok(())
}

fn coordinate(v: i64) -> Result<(), Failure> {
    if v.checked_abs().is_none_or(|a| a > limits::COORDINATE_NM) {
        return Err(invalid(format!(
            "The generator returned a coordinate of {v} nm, beyond the 1 m bound."
        )));
    }
    Ok(())
}

fn point(p: &Point) -> Result<(), Failure> {
    coordinate(p[0])?;
    coordinate(p[1])
}

fn positive(what: &str, v: i64) -> Result<(), Failure> {
    if v <= 0 || v > limits::COORDINATE_NM {
        return Err(invalid(format!(
            "The generator returned a {what} of {v} nm; it must be positive and at most 1 m."
        )));
    }
    Ok(())
}

fn graphic(g: &Graphic) -> Result<(), Failure> {
    if g.stroke_nm < 0 || g.stroke_nm > limits::COORDINATE_NM {
        return Err(invalid(format!(
            "The generator returned a stroke of {} nm.",
            g.stroke_nm
        )));
    }
    match &g.shape {
        Primitive::Line { a, b } | Primitive::Rect { min: a, max: b } => {
            point(a)?;
            point(b)
        }
        Primitive::Polyline { points } | Primitive::Polygon { points } => {
            count("points in one shape", points.len(), limits::POINTS)?;
            points.iter().try_for_each(point)
        }
        Primitive::Circle { center, radius_nm } => {
            point(center)?;
            positive("circle radius", *radius_nm)
        }
        Primitive::Arc {
            center, start, end, ..
        } => {
            point(center)?;
            point(start)?;
            point(end)?;
            // Squared distances in i128: coordinates are within 1 m, so
            // nothing overflows.
            let squared = |p: &[i64; 2]| {
                let dx = i128::from(p[0]) - i128::from(center[0]);
                let dy = i128::from(p[1]) - i128::from(center[1]);
                dx * dx + dy * dy
            };
            let (a, b) = (squared(start), squared(end));
            if a == 0 {
                return Err(invalid(
                    "The generator returned an arc that starts at its centre.".into(),
                ));
            }
            // |ra - rb| <= t  <=>  |a - b| <= t (ra + rb), and
            // ra + rb <= 2 * max(ra, rb) + 1 with integer square roots.
            let tolerance = i128::from(limits::ARC_RADIUS_NM);
            let reach = 2 * a.max(b).isqrt() + 2;
            if (a - b).abs() > tolerance * reach {
                return Err(invalid(
                    "The generator returned an arc whose ends are not on one circle about its centre.".into(),
                ));
            }
            Ok(())
        }
        Primitive::Text {
            at,
            height_nm,
            content,
            ..
        } => {
            point(at)?;
            positive("text height", *height_nm)?;
            if let sparksmith_generator_abi::v1::TextContent::Literal(s) = content {
                text("text", s)?;
            }
            Ok(())
        }
    }
}

fn graphics(what: &str, list: &[Graphic]) -> Result<(), Failure> {
    count(what, list.len(), limits::GRAPHICS)?;
    list.iter().try_for_each(graphic)
}

fn symbol(symbol: &Symbol, terminals: &BTreeSet<&str>) -> Result<(), Failure> {
    count("symbol units", symbol.units.len(), limits::UNITS)?;
    if symbol.units.is_empty() {
        return Err(invalid(
            "The generator returned a symbol with no units.".into(),
        ));
    }
    unique("symbol unit", symbol.units.iter().map(|u| u.key.as_str()))?;
    let mut drawn = BTreeSet::new();
    for unit in &symbol.units {
        count("symbol pins", unit.pins.len(), limits::TERMINALS)?;
        graphics("symbol graphics", &unit.graphics)?;
        unique(
            "symbol drawing",
            unit.graphics.iter().map(|g| g.key.as_str()),
        )?;
        for pin in &unit.pins {
            if !terminals.contains(pin.terminal.as_str()) {
                return Err(invalid(format!(
                    "The generator drew a pin for terminal \"{}\", which the component does not have.",
                    pin.terminal
                )));
            }
            point(&pin.at)?;
            if pin.length_nm < 0 || pin.length_nm > limits::COORDINATE_NM {
                return Err(invalid(format!(
                    "The generator returned a pin length of {} nm.",
                    pin.length_nm
                )));
            }
            drawn.insert(pin.terminal.as_str());
        }
    }
    if let Some(missing) = terminals.iter().find(|t| !drawn.contains(**t)) {
        return Err(invalid(format!(
            "The generator's symbol has no pin for terminal \"{missing}\"."
        )));
    }
    Ok(())
}

fn footprint(footprint: &Footprint, package_terminals: &BTreeSet<&str>) -> Result<(), Failure> {
    count("pads", footprint.pads.len(), limits::PADS)?;
    unique("pad", footprint.pads.iter().map(|p| p.key.as_str()))?;
    for pad in &footprint.pads {
        if !package_terminals.contains(pad.terminal.as_str()) {
            return Err(invalid(format!(
                "The generator's pad {} lands terminal \"{}\", which the package does not have.",
                pad.key, pad.terminal
            )));
        }
        point(&pad.at)?;
        match &pad.shape {
            PadShape::Rect { size } | PadShape::Oval { size } => {
                positive("pad size", size[0])?;
                positive("pad size", size[1])?;
            }
            PadShape::RoundRect { size, radius_nm } => {
                positive("pad size", size[0])?;
                positive("pad size", size[1])?;
                if *radius_nm < 0 || 2 * *radius_nm > size[0].min(size[1]) {
                    return Err(invalid(format!(
                        "The generator's pad {} has a corner radius larger than half its side.",
                        pad.key
                    )));
                }
            }
            PadShape::Circle { diameter_nm } => positive("pad diameter", *diameter_nm)?,
            PadShape::Polygon { points } => {
                if points.len() < 3 {
                    return Err(invalid(format!(
                        "The generator's pad {} is a polygon of fewer than three points.",
                        pad.key
                    )));
                }
                count("points in one pad", points.len(), limits::POINTS)?;
                points.iter().try_for_each(point)?;
            }
        }
        if let Some(hole) = &pad.hole {
            match hole.shape {
                HoleShape::Round { diameter_nm } => positive("hole diameter", diameter_nm)?,
                HoleShape::Slot { size } => {
                    positive("slot size", size[0])?;
                    positive("slot size", size[1])?;
                }
            }
        }
    }
    let terminals_with_pads: BTreeSet<&str> =
        footprint.pads.iter().map(|p| p.terminal.as_str()).collect();
    if let Some(missing) = package_terminals
        .iter()
        .find(|t| !terminals_with_pads.contains(**t))
    {
        return Err(invalid(format!(
            "The generator's footprint has no pad for package terminal \"{missing}\"."
        )));
    }
    if footprint.courtyard.len() < 3 {
        return Err(invalid(
            "The generator returned no courtyard outline.".into(),
        ));
    }
    count(
        "courtyard points",
        footprint.courtyard.len(),
        limits::POINTS,
    )?;
    footprint.courtyard.iter().try_for_each(point)?;
    graphics("silkscreen graphics", &footprint.silk)?;
    graphics("assembly graphics", &footprint.assembly)?;
    unique(
        "footprint drawing",
        footprint
            .silk
            .iter()
            .chain(&footprint.assembly)
            .map(|g| g.key.as_str()),
    )?;
    Ok(())
}

/// Longest part-file parameter name (as the document bounds it).
const PART_PARAMETER: usize = 64;

/// The 3D model: required, a part file named by its SHA-256 (64 lower-case
/// hex digits), with at most [`limits::MODEL_VALUES`] whole-number values
/// within the 1 m bound under part-file parameter names. Whether the file
/// is in the generator's bundle, and how it evaluates, the host checks
/// when it makes the part.
fn model(g: &Generation) -> Result<(), Failure> {
    let refuse = |message: String| {
        Failure::one(
            "generator.invalid_model",
            format!("{message} Nothing was changed."),
        )
    };
    let Some(Model::PartFile { asset, values }) = &g.model else {
        return Err(refuse(
            "The generator made no 3D model; a part needs one.".into(),
        ));
    };
    let hex = asset.len() == 64
        && asset
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    if !hex {
        return Err(refuse(format!(
            "The generator names its 3D model's part file \"{asset}\", which is not a SHA-256 (64 lower-case hex digits)."
        )));
    }
    if values.len() > limits::MODEL_VALUES {
        return Err(refuse(format!(
            "The generator gives its part file {} values, more than the {} allowed.",
            values.len(),
            limits::MODEL_VALUES
        )));
    }
    for (name, value) in values {
        let mut chars = name.chars();
        let named = name.len() <= PART_PARAMETER
            && chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !named {
            return Err(refuse(format!(
                "The generator gives a value to \"{name}\", which cannot name a part-file parameter (1 to {PART_PARAMETER} ASCII letters, digits and underscores, not starting with a digit)."
            )));
        }
        match value {
            ParamValue::Int(v) if v.checked_abs().is_some_and(|a| a <= limits::COORDINATE_NM) => {}
            ParamValue::Int(v) => {
                return Err(refuse(format!(
                    "The generator sets the part file's \"{name}\" to {v}, beyond the 1 m bound."
                )));
            }
            _ => {
                return Err(refuse(format!(
                    "The generator sets the part file's \"{name}\" to something other than a whole number."
                )));
            }
        }
    }
    Ok(())
}

/// Check a generation for `scope`.
pub fn generation(g: &Generation, scope: Scope) -> Result<(), Failure> {
    count(
        "package terminals",
        g.package.terminals.len(),
        limits::TERMINALS,
    )?;
    let package_terminals = unique(
        "package terminal",
        g.package.terminals.iter().map(String::as_str),
    )?;
    let geometry = &g.package.geometry;
    for v in geometry.envelope_nm {
        positive("package size", v)?;
    }
    coordinate(geometry.seating_nm)?;
    if !package_terminals.contains(geometry.pin1.as_str()) {
        return Err(invalid(format!(
            "The generator names pin 1 \"{}\", which the package does not have.",
            geometry.pin1
        )));
    }
    unique("lead", geometry.leads.iter().map(|l| l.key.as_str()))?;
    for lead in &geometry.leads {
        if !package_terminals.contains(lead.key.as_str()) {
            return Err(invalid(format!(
                "The generator places a lead for \"{}\", which the package does not have.",
                lead.key
            )));
        }
        lead.contact.iter().try_for_each(|v| coordinate(*v))?;
    }
    footprint(&g.footprint, &package_terminals)?;
    model(g)?;
    for note in &g.notes {
        text("note", note)?;
    }
    for field in &g.fields {
        key("field", &field.field)?;
        text("field value", &field.value)?;
    }
    match scope {
        Scope::Package => {
            if g.component.is_some() || g.symbol.is_some() || !g.binding.is_empty() {
                return Err(invalid(
                    "The generator returned a component, symbol or binding for a package-only request."
                        .into(),
                ));
            }
        }
        Scope::Component => {
            let (Some(component), Some(sym)) = (&g.component, &g.symbol) else {
                return Err(invalid(
                    "The generator returned no component or no symbol for a complete part.".into(),
                ));
            };
            count("terminals", component.terminals.len(), limits::TERMINALS)?;
            let terminals = unique(
                "terminal",
                component.terminals.iter().map(|t| t.key.as_str()),
            )?;
            for t in &component.terminals {
                text("terminal name", &t.name)?;
            }
            key("designator prefix", &component.prefix)?;
            text("component name", &component.name)?;
            symbol(sym, &terminals)?;
            let mut bound = BTreeSet::new();
            for entry in &g.binding {
                if !package_terminals.contains(entry.package_terminal.as_str()) {
                    return Err(invalid(format!(
                        "The generator binds package terminal \"{}\", which the package does not have.",
                        entry.package_terminal
                    )));
                }
                if !terminals.contains(entry.terminal.as_str()) {
                    return Err(invalid(format!(
                        "The generator binds to terminal \"{}\", which the component does not have.",
                        entry.terminal
                    )));
                }
                if !bound.insert(entry.package_terminal.as_str()) {
                    return Err(invalid(format!(
                        "The generator binds package terminal \"{}\" twice.",
                        entry.package_terminal
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Check a description: ABI 1, an id, a version from 1, unique
/// capabilities, a [`GENERATE_PART`] capability (a generator) of effect
/// `none` with at least one scope, unique parameter keys, every default of
/// its parameter's type, presets naming known parameters, a deterministic
/// plugin whenever a capability is of effect `none`, and `needs` only for
/// a plugin with `read` or `edit` capabilities.
pub fn description(d: &Description) -> Result<(), Failure> {
    let bad = |message: String| {
        Failure::one(
            "generator.invalid_description",
            format!("{message} The generator was not used."),
        )
    };
    if d.abi != sparksmith_generator_abi::v1::ABI {
        return Err(bad(format!(
            "The generator describes itself as ABI {}.",
            d.abi
        )));
    }
    key("generator id", &d.id).map_err(|f| bad(f.to_string()))?;
    text("title", &d.title).map_err(|f| bad(f.to_string()))?;
    text("licence", &d.licence).map_err(|f| bad(f.to_string()))?;
    if d.version == 0 {
        return Err(bad(
            "The generator gives version 0; versions count from 1.".into()
        ));
    }
    count("capabilities", d.capabilities.len(), MAX_CAPABILITIES)
        .map_err(|f| bad(f.to_string()))?;
    unique("capability", d.capabilities.iter().map(|c| c.name.as_str()))
        .map_err(|f| bad(f.to_string()))?;
    if d.capabilities.iter().any(|c| c.effect == Effect::None) && !d.deterministic {
        return Err(bad(
            "The plugin offers a capability of effect none, whose results are hashed, but does not declare itself deterministic.".into(),
        ));
    }
    if d.pure() && !d.needs.is_empty() {
        return Err(bad(
            "The plugin's capabilities are all of effect none, so it can need no catalog module."
                .into(),
        ));
    }
    for need in &d.needs {
        key("needed module", &need.module).map_err(|f| bad(f.to_string()))?;
        if need.class == Effect::None {
            return Err(bad(format!(
                "The plugin needs module \"{}\" for effect none; a need is for read or edit.",
                need.module
            )));
        }
    }
    let Some(part) = d.part() else {
        return Err(bad(format!(
            "The plugin offers no \"{GENERATE_PART}\" capability, so it makes no parts."
        )));
    };
    if part.effect != Effect::None {
        return Err(bad(format!(
            "Its \"{GENERATE_PART}\" capability declares effect {:?}; making a part is of effect none.",
            part.effect
        )));
    }
    if d.scopes().is_empty() {
        return Err(bad("The generator offers no scope.".into()));
    }
    let keys = unique("parameter", d.params().iter().map(|p| p.key.as_str()))
        .map_err(|f| bad(f.to_string()))?;
    for param in d.params() {
        if !crate::params::fits(&param.ty, &param.default) {
            return Err(bad(format!(
                "The default of parameter \"{}\" does not fit its type.",
                param.key
            )));
        }
        if let ParamType::Length { min_nm, max_nm, .. } = param.ty
            && min_nm > max_nm
        {
            return Err(bad(format!(
                "Parameter \"{}\" has an empty range.",
                param.key
            )));
        }
    }
    unique("preset", d.presets().iter().map(|p| p.key.as_str())).map_err(|f| bad(f.to_string()))?;
    for preset in d.presets() {
        if let Some(unknown) = preset.params.keys().find(|k| !keys.contains(k.as_str())) {
            return Err(bad(format!(
                "Preset \"{}\" sets \"{unknown}\", which is not a parameter.",
                preset.key
            )));
        }
    }
    Ok(())
}

/// Most capabilities one plugin offers.
const MAX_CAPABILITIES: usize = 64;

#[cfg(test)]
mod tests {
    use super::description;
    use sparksmith_generator_abi::v1::{
        ABI, Capability, Description, Effect, GENERATE_PART, Need, PartOffer, Scope,
    };

    pub fn fixture() -> Description {
        Description {
            abi: ABI,
            id: "fixture".into(),
            version: 1,
            title: "Fixture".into(),
            licence: "MIT".into(),
            deterministic: true,
            capabilities: vec![Capability {
                name: GENERATE_PART.into(),
                effect: Effect::None,
                title: None,
                params: Vec::new(),
                rules: Vec::new(),
                presets: Vec::new(),
                part: Some(PartOffer {
                    families: Vec::new(),
                    scopes: vec![Scope::Package],
                    grammar: Vec::new(),
                    instance_fields: Vec::new(),
                }),
            }],
            needs: Vec::new(),
        }
    }

    fn refusal(d: &Description) -> String {
        description(d)
            .err()
            .map(|f| f.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn a_generator_at_version_0_is_refused() {
        let mut d = fixture();
        assert!(description(&d).is_ok());
        d.version = 0;
        assert!(refusal(&d).contains("version 0; versions count from 1"));
    }

    #[test]
    fn effects_needs_and_determinism_are_checked() {
        let mut d = fixture();
        d.deterministic = false;
        assert!(refusal(&d).contains("does not declare itself deterministic"));
        let mut d = fixture();
        d.needs.push(Need {
            module: "layout".into(),
            major: 1,
            minor: 0,
            class: Effect::Edit,
        });
        assert!(refusal(&d).contains("can need no catalog module"));
        // A plugin that also offers an edit command may need modules.
        d.capabilities.push(Capability {
            name: "vias".into(),
            effect: Effect::Edit,
            title: None,
            params: Vec::new(),
            rules: Vec::new(),
            presets: Vec::new(),
            part: None,
        });
        assert!(description(&d).is_ok());
        let mut d = fixture();
        if let Some(c) = d.capabilities.first_mut() {
            c.effect = Effect::Read;
        }
        assert!(refusal(&d).contains("making a part is of effect none"));
    }
}
