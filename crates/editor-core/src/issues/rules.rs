//! What core says about each finding code: its severity, its group title,
//! its one-line title and the fix it suggests. Every code a rule owner
//! emits is classified here; [`KNOWN_CODES`] lists them and a test keeps it
//! complete against the sources.
use super::names::Names;
use super::{MAX_TITLE, Severity};
use crate::diagnostics::{Finding, ObjectRef};
use std::collections::BTreeSet;

/// Synthetic codes `project.issues` adds for states no rule owner reports
/// as a finding.
pub(super) const NO_PART: &str = "purchasing.no_part";
pub(super) const CHECK_FAILED: &str = "instance.check_failed";

/// How severe a finding is. Anything that blocks a stage is an error, so a
/// publish blocker is always red and always counted.
pub(super) fn severity(finding: &Finding) -> Severity {
    if finding.blocks.is_some() {
        return Severity::Error;
    }
    match finding.code.as_str() {
        "board.clearance"
        | "board.courtyard_overlap"
        | "board.courtyard_clearance"
        | "board.edge_clearance"
        | "board.hole_clearance"
        | "board.outside_outline"
        | "board.via_annular_ring"
        | "board.via_aspect_ratio"
        | "board.via_drill_clearance"
        | "board.via_drill_size"
        | "board.via_hole_spacing"
        | "erc.driver_conflict"
        | "erc.open_drain_contention"
        | "fill.clearance"
        | "keepout.track"
        | "keepout.via"
        | "keepout.pad"
        // Geometry past the limit is unchecked, so errors may be uncounted.
        | "board.check_limit"
        | "board.shape_unsolved"
        | "board.shape_open"
        | "board.shape_invalid"
        | "board.reference_lost"
        | "board.reference_inexact"
        | CHECK_FAILED => Severity::Error,
        "board.unrouted" | "board.unplaced" | NO_PART => Severity::ToDo,
        "erc.unchecked_terminal"
        | "fill.stale"
        | "library.update_available"
        | "marking.unrepresentable"
        | "marking.does_not_fit" => Severity::Info,
        _ => Severity::Warning,
    }
}

/// The group header of a code: "Clearance", "Unrouted connections".
pub fn group_title(code: &str) -> String {
    let known = match code {
        "board.clearance" => "Clearance",
        "board.acute_copper" => "Acute copper angles",
        "board.copper_sliver" => "Unfilled slivers",
        "board.courtyard_overlap" => "Courtyards overlap",
        "board.outside_outline" => "Outside the board outline",
        "board.unrouted" => "Unrouted connections",
        "board.unplaced" => "Not placed",
        "board.check_limit" => "Board checks incomplete",
        "board.via_annular_ring" => "Thin via annular rings",
        "board.via_drill_clearance" => "Via drills too close to copper",
        "board.via_hole_spacing" => "Via drills too close together",
        "board.via_dangling" => "Vias joining nothing",
        "board.track_dangling" => "Track ends joining nothing",
        "board.via_drill_size" => "Via drills too small",
        "board.via_aspect_ratio" => "Via aspect ratio too high",
        "board.edge_clearance" => "Too close to the board edge",
        "board.hole_clearance" => "Too close to a mounting hole",
        "board.courtyard_clearance" => "Courtyards too close",
        "fill.clearance" => "Fill clearance",
        "fill.empty" => "Empty fills",
        "fill.island_unconnected" => "Unconnected fill islands",
        "fill.net_missing" => "Fills on missing nets",
        "fill.stale" => "Fills to refill",
        "keepout.track" => "Tracks in keepouts",
        "keepout.via" => "Vias in keepouts",
        "keepout.pad" => "Pads in keepouts",
        "pair.skew" => "Pair skew",
        "pair.uncoupled" => "Pairs running uncoupled",
        "pair.gap" => "Pair gap",
        "length.mismatch" => "Length mismatch",
        "impedance.mismatch" => "Impedance off target",
        "board.constraint_unresolved" => "Edge constraints unresolved",
        "board.shape_unsolved" => "Board shape does not solve",
        "board.shape_open" => "Board outline not closed",
        "board.shape_invalid" => "Board outline not one board",
        "board.reference_lost" => "Dimension references lost",
        "board.reference_inexact" => "Dimension references to check",
        "instance.unit_unplaced" => "Units not placed",
        "instance.terminal_unaccounted" => "Unconnected terminals",
        "instance.required_terminal_no_connect" => "Power inputs marked no-connect",
        "instance.no_connect_connected" => "No-connect pins connected",
        "instance.field_required" => "Required values missing",
        "instance.no_physical_choice" => "No footprint chosen",
        "config.function_unresolved" => "Pin functions not chosen",
        "config.requirement_unmet" => "Pin function requirements",
        "config.rule_unmet" => "Pin rules not met",
        "config.rule_note" => "Pin rule notes",
        "config.pin_option" => "Invalid pin configuration",
        "marking.unrepresentable" => "Markings that cannot be drawn",
        "marking.does_not_fit" => "Markings that do not fit",
        "pinout.uncertain" => "Uncertain imported pins",
        "pinout.unreviewed" => "Unreviewed imported pins",
        "erc.driver_conflict" => "Drivers in conflict",
        "erc.open_drain_contention" => "Open-drain contention",
        "erc.input_only_net" => "Inputs without a driver",
        "erc.undriven_power_input" => "Power inputs without a supply",
        "erc.unchecked_terminal" => "Terminals not checked",
        "erc.label_unconnected" => "Labels without a wire",
        "erc.wire_end_free" => "Wire ends not connected",
        "erc.global_label_single" => "Global labels used once",
        "erc.hierarchy_mismatch" => "Sheet pins and labels that do not match",
        "erc.label_conflict" => "Nets with conflicting labels",
        "erc.power_port_undriven" => "Power nets without a source",
        NO_PART => "No Part chosen",
        CHECK_FAILED => "Instances that cannot be checked",
        "part.stale_component" => "Part for an older revision",
        "part.newer_component" => "Part for a newer revision",
        "part.component_mismatch" => "Part for another component",
        "part.binding_mismatch" => "Part for another footprint",
        "part.spec_mismatch" => "Rating does not meet the value",
        "part.spec_unrated" => "Value the Part does not rate",
        "part.spec_uncomparable" => "Rating cannot be compared",
        "part.spec_field_mismatch" => "Rating of another field",
        "reference.missing" => "Missing from the project library",
        "reference.owner_mismatch" => "Owned by another component",
        "footprint.terminal_required" => "Pads without a package terminal",
        "footprint.terminal_without_pad" => "Package terminals without a pad",
        "footprint.terminal_forbidden" => "Pads that cannot land a terminal",
        "footprint.pad_terminal_unknown" => "Pads landing unknown terminals",
        "component.mapping_missing" => "Unmapped terminals",
        "component.pin_missing" => "Terminals without a pin",
        "binding.mapping_missing" => "Unmapped package terminals",
        "binding.mapping_retained" => "Mappings kept by position",
        "binding.footprint_unbound" => "Footprint not listed in a binding",
        "footprint.courtyard_missing" => "No courtyard",
        "footprint.polarity_missing" => "No pin-1 marking",
        "footprint.mask_web" => "Thin solder-mask webs",
        "footprint.silk_width" => "Thin silkscreen lines",
        "footprint.text_height" => "Small silkscreen text",
        "footprint.silk_over_pad" => "Silkscreen over pads",
        "footprint.pad_outside_courtyard" => "Pads outside the courtyard",
        "footprint.param_expr" => "Parameter expressions",
        "footprint.solve_failed" => "Sketch does not solve",
        "footprint.over_constrained" => "Sketch over-constrained",
        "symbol.pin_off_grid" => "Pins off the grid",
        "symbol.grid_sheet" => "Symbol grid",
        "symbol.field_missing" => "No designator text",
        "definition.name" => "Names missing",
        "definition.stale_base" => "Drafts of an older revision",
        "definition.bounds" => "Drafts too large",
        "part.identity" => "Part identity",
        "part_file.model" => "Part file is not a SolidCraft design",
        "part_file.values" => "Part file values",
        "package.lead_off_land" => "Leads off their lands",
        "package.lead_unseated" => "Leads off the seating plane",
        "package.body_below_board" => "Body below the board",
        "package.terminals_unmatched" => "Package terminals not matched by number",
        "component.no_implementation" => "No physical implementation",
        "asset.missing" => "Attached copies not packed",
        "library.update_available" => "Library updates",
        "library.source_modified" => "Library sources changed",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    humanised(code)
}

/// "footprint.pad_size" → "Pad size": the rule part of a code in words.
fn humanised(code: &str) -> String {
    let rule = code.rsplit('.').next().unwrap_or(code).replace('_', " ");
    crate::library::names::capitalised(rule.trim())
}

/// What resolves a code, as an imperative.
pub(super) fn fix(code: &str) -> Option<&'static str> {
    Some(match code {
        "board.clearance" => "Move the parts apart or reroute the copper",
        "board.acute_copper" => "Reroute the copper to meet at a wider angle",
        "board.copper_sliver" => "Move the copper apart or together, or widen the hatch",
        "board.courtyard_overlap" => "Move the parts apart",
        "board.outside_outline" => "Move it inside the board",
        "board.unrouted" => "Route the connection",
        "board.unplaced" => "Update the PCB from the schematic",
        "board.via_annular_ring" => "Enlarge the via pad or shrink its drill",
        "board.via_drill_clearance" | "board.via_hole_spacing" => "Move the via",
        "board.via_dangling" => "Route copper to the via or delete it",
        "board.track_dangling" => "Route the end to copper of its net or delete the track",
        "board.via_drill_size" | "board.via_aspect_ratio" => "Enlarge the via drill",
        "board.edge_clearance" => "Move it away from the edge",
        "board.hole_clearance" => "Move the copper away from the hole",
        "board.courtyard_clearance" => "Move the parts apart",
        "fill.clearance" | "fill.stale" => "Refill the fill",
        "fill.empty" => "Enlarge the fill outline or lower its priority",
        "fill.island_unconnected" => "Remove unconnected islands or connect them",
        "fill.net_missing" => "Choose the fill's net",
        "keepout.track" | "keepout.via" | "keepout.pad" => "Move it out of the keepout",
        "pair.skew" | "length.mismatch" => "Tune the shorter net with the Tune tool",
        "pair.uncoupled" => "Route the halves together with pair routing",
        "pair.gap" => "Reroute the pair at its gap",
        "impedance.mismatch" => "Route at the width the Route card shows for the layer",
        "board.constraint_unresolved" => "Edit or delete the edge constraint",
        "board.shape_unsolved" => "Fix the board sketch's dimensions",
        "board.shape_open" => "Join the open ends in Board shape",
        "board.shape_invalid" => "Fix the crossing or move the hole inside the board",
        "board.reference_lost" => "Pick the reference again or delete the dimension",
        "board.reference_inexact" => "Check the reference and acknowledge it, or pick it again",
        "instance.unit_unplaced" => "Place the unit or mark it unused",
        "instance.terminal_unaccounted" => "Connect the terminal or mark it no-connect",
        "instance.required_terminal_no_connect" => "Connect the power input",
        "instance.no_connect_connected" => "Remove the wire or clear the no-connect mark",
        "instance.field_required" => "Enter the value",
        "instance.no_physical_choice" => "Choose a footprint",
        "config.function_unresolved" => "Choose a pin function",
        NO_PART => "Choose a Part with Resolve parts",
        "part.stale_component" | "part.newer_component" | "part.component_mismatch" => {
            "Choose a Part for this revision"
        }
        "footprint.terminal_required" => "Assign the pad a package terminal, or make it mechanical",
        "footprint.terminal_without_pad" => "Assign a pad to the package terminal",
        "component.mapping_missing" | "binding.mapping_missing" => {
            "Map the terminal on the Mapping tab"
        }
        "component.pin_missing" => "Add a pin for the terminal on the Symbol tab",
        "erc.label_unconnected" => "Wire the label or remove it",
        "erc.wire_end_free" => "Connect the end or remove the wire",
        "erc.global_label_single" => "Use the name elsewhere or make it a local label",
        "erc.hierarchy_mismatch" => "Add the missing pin or hierarchical label",
        "erc.label_conflict" => "Rename the labels to one name",
        "erc.power_port_undriven" => "Connect a supply to the net",
        _ => return None,
    })
}

/// `text` as one line of at most [`MAX_TITLE`] characters with no final
/// full stop: the first clause, clipped at a word with "…".
pub(super) fn line(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    let clause = first.split("; ").next().unwrap_or(first).trim();
    let clause = clause.trim_end_matches('.').trim_end();
    clip(clause)
}

/// The first line of `text` without a final full stop, unclipped: the
/// cause shown under a row.
pub(super) fn line_full(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    first.trim_end_matches('.').trim_end().to_owned()
}

/// At most [`MAX_TITLE`] characters, cut at a word boundary with "…".
pub(super) fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_TITLE {
        return text.to_owned();
    }
    let keep: String = text.chars().take(MAX_TITLE - 1).collect();
    let cut = keep
        .rfind(' ')
        .filter(|at| *at > MAX_TITLE / 2)
        .unwrap_or(keep.len());
    let head = keep[..cut].trim_end_matches([' ', ',', ';', ':', '.']);
    format!("{head}…")
}

/// Distinct names of the referenced objects of `kind`, in first-seen order.
fn names_of(names: &Names<'_>, findings: &[&Finding], pick: fn(&ObjectRef) -> bool) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for object in findings.iter().flat_map(|f| f.objects.iter()) {
        if pick(object) {
            let name = names.object(object);
            if seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

/// The parties of a clearance or courtyard finding: the instances and
/// tracks it names, by name.
fn parties(names: &Names<'_>, findings: &[&Finding]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for object in findings.iter().flat_map(|f| f.objects.iter()) {
        let name = match *object {
            ObjectRef::Pad { instance, .. } | ObjectRef::Instance { id: instance } => {
                names.designator(instance)
            }
            ObjectRef::Track { .. } => names.object(object),
            _ => continue,
        };
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

fn is_pad(object: &ObjectRef) -> bool {
    matches!(object, ObjectRef::Pad { .. })
}

/// "R1, R2 and R3", shortened to "R1, R2 and 3 more" past four names.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] if items.len() <= 4 => format!("{} and {last}", init.join(", ")),
        _ => format!(
            "{} and {} more",
            items.get(..2).unwrap_or_default().join(", "),
            items.len() - 2
        ),
    }
}

/// The one-line title of a row: its primary finding and every finding
/// merged into it. Titles name objects by their user names.
pub(super) fn title(names: &Names<'_>, findings: &[&Finding]) -> String {
    let Some(first) = findings.first() else {
        return String::new();
    };
    let message = first.message.as_str();
    let title = match first.code.as_str() {
        "board.clearance" => {
            let pads = names_of(names, findings, is_pad);
            match parties(names, findings).as_slice() {
                [a, b] if findings.len() > 1 => {
                    format!("{a} ↔ {b}: {} pads too close", pads.len())
                }
                _ => {
                    let objects = names_of(names, findings, |_| true);
                    format!("{} too close", objects.join(" ↔ "))
                }
            }
        }
        "board.courtyard_overlap" => {
            // The top side is the usual case; only the bottom is named.
            let side = if message.contains("bottom") {
                " (bottom)"
            } else {
                ""
            };
            format!(
                "{} courtyards overlap{side}",
                parties(names, findings).join(" ↔ ")
            )
        }
        "board.outside_outline" => {
            let what = if message.contains("extends into a slot") {
                "extends into a slot"
            } else if message.contains("extends into a cutout") {
                "extends into a cutout"
            } else {
                "extends beyond the board outline"
            };
            format!("{} {what}", list(&names_of(names, findings, |_| true)))
        }
        "board.unrouted" => {
            let nets = names_of(names, findings, |o| matches!(o, ObjectRef::Net { .. }));
            let pads = names_of(names, findings, is_pad);
            match pads.as_slice() {
                [pad] => format!("{}: {pad} is unrouted", list(&nets)),
                _ => format!("{}: {} pads unrouted", list(&nets), pads.len()),
            }
        }
        "board.unplaced" => format!("{} not placed", list(&names_of(names, findings, |_| true))),
        // Short enough for the Issues row: the hole and what it is near.
        "board.hole_clearance" => {
            let near = names_of(names, findings, |o| !matches!(o, ObjectRef::Board));
            let what = if message.contains(" overlaps ") {
                "overlaps"
            } else {
                "too close to"
            };
            match near.as_slice() {
                [] => line(message),
                _ => format!("Hole {what} {}", list(&near)),
            }
        }
        "instance.unit_unplaced" => format!(
            "{} is neither placed nor marked unused",
            names_of(names, findings, |_| true).join(", ")
        ),
        "instance.terminal_unaccounted" => format!(
            "{} unconnected",
            names_of(names, findings, |_| true).join(", ")
        ),
        "config.function_unresolved" => format!(
            "{}: no pin function",
            names_of(names, findings, |_| true).join(", ")
        ),
        "instance.no_physical_choice" => format!(
            "{}: no footprint",
            names_of(names, findings, |_| true).join(", ")
        ),
        NO_PART => format!(
            "{}: no Part",
            names_of(names, findings, |_| true).join(", ")
        ),
        "component.mapping_missing" if findings.len() > 1 => {
            let package = names_of(names, findings, |o| {
                matches!(o, ObjectRef::LibraryObject { .. })
            })
            .into_iter()
            .find(|n| n.starts_with("package terminal"));
            match package {
                Some(p) => format!(
                    "{} has no electrical terminal",
                    crate::library::names::capitalised(&p)
                ),
                None => line(message),
            }
        }
        code if code.starts_with("part.") && findings.len() > 1 => {
            let instances = names_of(names, findings, |o| matches!(o, ObjectRef::Instance { .. }));
            let rest = message.split_once(": ").map_or(message, |(_, rest)| rest);
            format!("{}: {}", list(&instances), line(rest))
        }
        _ => line(message),
    };
    clip(title.trim())
}

/// Every code a rule owner in core emits, and the synthetic ones
/// `project.issues` adds. A test checks this list against the sources, so
/// a new code is classified (severity, group title) when it is added.
pub const KNOWN_CODES: &[&str] = &[
    "asset.missing",
    "binding.duplicate_footprint_lineage",
    "binding.footprint_count",
    "binding.footprint_package",
    "binding.footprint_unbound",
    "binding.mapping_missing",
    "binding.mapping_retained",
    "binding.mapping_unknown",
    "binding.unavailable_duplicate",
    "binding.unavailable_required",
    "binding.unavailable_unknown",
    "board.acute_copper",
    "board.check_limit",
    "board.clearance",
    "board.constraint_unresolved",
    "board.copper_sliver",
    "board.courtyard_clearance",
    "board.courtyard_overlap",
    "board.edge_clearance",
    "board.hole_clearance",
    "board.outside_outline",
    "board.reference_inexact",
    "board.reference_lost",
    "board.shape_invalid",
    "board.shape_open",
    "board.shape_unsolved",
    "board.track_dangling",
    "board.unplaced",
    "board.unrouted",
    "board.via_annular_ring",
    "board.via_aspect_ratio",
    "board.via_dangling",
    "board.via_drill_clearance",
    "board.via_drill_size",
    "board.via_hole_spacing",
    "component.designator_prefix",
    "component.duplicate_binding_lineage",
    "component.duplicate_field_lineage",
    "component.duplicate_symbol_lineage",
    "component.field_default_invalid",
    "component.mapping_missing",
    "component.mapping_unknown",
    "component.no_implementation",
    "component.no_representation",
    "component.pin_missing",
    "component.pin_unknown",
    "component.terminal_count",
    "component.terminal_name",
    "component.text_field_unknown",
    "config.function_unresolved",
    "config.pin_option",
    "config.requirement_unmet",
    "config.rule_note",
    "config.rule_unmet",
    "definition.base_invalid",
    "definition.bounds",
    "definition.local_id_invalid",
    "definition.local_id_reused",
    "definition.local_id_unallocated",
    "definition.name",
    "definition.next_local_decreased",
    "definition.revisions_exhausted",
    "definition.stale_base",
    "erc.driver_conflict",
    "erc.global_label_single",
    "erc.hierarchy_mismatch",
    "erc.input_only_net",
    "erc.label_conflict",
    "erc.label_unconnected",
    "erc.open_drain_contention",
    "erc.power_port_undriven",
    "erc.unchecked_terminal",
    "erc.undriven_power_input",
    "erc.wire_end_free",
    "fill.clearance",
    "fill.empty",
    "fill.island_unconnected",
    "fill.net_missing",
    "fill.stale",
    "field.choice_label",
    "field.default_invalid",
    "field.required_stage",
    "field.value_type",
    "footprint.aperture_duplicate",
    "footprint.courtyard_missing",
    "footprint.courtyard_open",
    "footprint.hole_kind",
    "footprint.hole_outside_land",
    "footprint.hole_shape",
    "footprint.keepout_open",
    "footprint.mask_web",
    "footprint.over_constrained",
    "footprint.override_orphaned",
    "footprint.pad_count",
    "footprint.pad_number",
    "footprint.pad_outside_courtyard",
    "footprint.pad_shape",
    "footprint.pad_side",
    "footprint.pad_size",
    "footprint.pad_terminal_unknown",
    "footprint.param_duplicate",
    "footprint.param_expr",
    "footprint.param_name",
    "footprint.param_reserved",
    "footprint.paste_window_outside_pad",
    "footprint.paste_window_shape",
    "footprint.polarity_marker_unknown",
    "footprint.polarity_missing",
    "footprint.polarity_pad_unknown",
    "footprint.silk_over_pad",
    "footprint.silk_width",
    "footprint.solve_failed",
    "footprint.source_pad_unknown",
    "footprint.terminal_forbidden",
    "footprint.terminal_required",
    "footprint.terminal_without_pad",
    "footprint.text_height",
    "function.combination",
    "function.constraint_key",
    "function.count",
    "function.default_conflict",
    "function.design_default",
    "function.gpio",
    "function.label",
    "function.mux",
    "function.mux_table",
    "function.pin_option",
    "function.resource_unknown",
    "graphic.degenerate",
    "graphic.fill_body",
    "graphic.fill_open",
    "graphic.invisible",
    "graphic.polygon_self_intersecting",
    "graphic.radius_too_large",
    "impedance.mismatch",
    "instance.field_required",
    "instance.no_connect_connected",
    "instance.no_physical_choice",
    "instance.required_terminal_no_connect",
    "instance.terminal_unaccounted",
    "instance.unit_unplaced",
    "keepout.pad",
    "keepout.track",
    "keepout.via",
    "library.source_modified",
    "length.mismatch",
    "library.update_available",
    "marking.artwork",
    "marking.does_not_fit",
    "marking.template",
    "marking.unrepresentable",
    "model.asset",
    "model.body_unmapped",
    "model.bounds",
    "model.leads",
    "model.licence",
    "package.body_below_board",
    "package.designation",
    "package.envelope",
    "package.envelope_derived",
    "package.form_range",
    "package.form_terminals",
    "package.lead_off_land",
    "package.lead_unseated",
    "package.model_form",
    "package.model_materials",
    "package.terminal_count",
    "package.terminals_unmatched",
    "package_row.alias",
    "package_row.family",
    "package_row.provenance",
    "package_row.range",
    "part.binding_mismatch",
    "part.binding_unlisted",
    "part.component_mismatch",
    "part.hash",
    "part.identity",
    "part.link",
    "part.newer_component",
    "part.offer_text",
    "part.provenance_basis",
    "part.reference_text",
    "part.spec_duplicate",
    "part.spec_field",
    "part.spec_field_mismatch",
    "part.spec_meaning",
    "part.spec_mismatch",
    "part.spec_uncomparable",
    "part.spec_unrated",
    "part.spec_value",
    "part.stale_component",
    "part_file.model",
    "part_file.values",
    "pinout.uncertain",
    "pinout.unreviewed",
    "pair.gap",
    "pair.skew",
    "pair.uncoupled",
    "quantity_kind.default_unit",
    "quantity_kind.units",
    "reference.missing",
    "reference.owner_mismatch",
    "resource.kind",
    "resource.label",
    "resource.role_capacity",
    "resource.role_count",
    "resource.role_label",
    "resource.signal",
    "rule.bounds",
    "rule.budget",
    "rule.default_conflict",
    "rule.reference",
    "rule.runtime",
    "rule.script",
    "symbol.field_missing",
    "symbol.grid_sheet",
    "symbol.pin_duplicate",
    "symbol.pin_off_grid",
    "symbol.unit_count",
    "symbol.unit_empty",
    "symbol.unit_label",
    "text.content_kind",
    "text.empty",
    "text.too_long",
    NO_PART,
    CHECK_FAILED,
];
