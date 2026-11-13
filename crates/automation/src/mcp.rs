//! Model Context Protocol server (JSON-RPC 2.0 over newline-delimited stdio).
//!
//! Each [`Request`] variant is one tool, named by its command ID with `.`
//! replaced by `_`. Its input schema is generated from the parameter schema
//! the JSON-lines transport accepts, shortened where it is large
//! ([`discovery::tool_schema`]); `project_schema` serves the rest on demand.
//! Replies are compact unless a call passes `verbose: true`
//! ([`Session::respond`]) and are sent once, as text. Conventions (bounded
//! lines, version negotiation, notifications without replies) follow
//! Solidcraft's MCP server.
//!
//! Every tool call passes the gate under the server's [`Policy`]
//! ([`Session::respond`] as [`Caller::Model`]): MCP has no way to ask, so
//! whatever would need a person is refused, and the Never class always is.
//!
//! A live server ([`Server::live`]) answers the same tools from the open
//! desktop window instead of a session of its own ([`crate::live`]), whose
//! gate decides and asks.
use crate::gate::{Caller, Policy};
use crate::lines::{self, Line};
use crate::live;
use crate::session::{CommandInfo, Request, Session, discovery};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];
/// Large enough for a complete component draft, small enough to bound memory.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// Claude Code cuts server instructions beyond this many characters.
pub const MAX_INSTRUCTIONS: usize = 4096;
/// The instructions' target in bytes (`docs/mcp-surface.md` §3.1): the
/// draft there is 1,030 characters and 238 Qwen3.8 tokens (4.3 bytes per
/// token), so this holds them near 255 tokens.
pub const INSTRUCTIONS_TARGET: usize = 1100;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub(crate) const INSTRUCTIONS: &str = concat!(
    sparksmith_core::product_name!(),
    " PCB design software. Each tool's description and project_schema name a permission class: read and edit run (unless the permission mode asks for edits too), ask needs a person's Allow, never is always refused; a server with no way to ask refuses what needs asking, and every refusal says what to do instead. project_inspect gives counts, drafts and open findings (at: a JSON pointer such as /project/vias reads one part of the document). Every mutation goes through project_apply with the revision from the last result; project_undo, project_redo, project_new and project_open take it too, and every one is refused unchanged when the document moved since (for example after the user's own edit). A reply is compact: revision, outcome, changes (added, changed and removed stable ids by collection), findings (new ones, resolved counts per code, the open total) and the changed drafts' findings; project_query project.issues lists all findings, and verbose: true on any tool returns the whole document snapshot, which is large. project_apply and project_query list their operation and query names only: project_schema {name: \"layout.add_via\"} returns one's exact schema and how to send it, with large definitions read by project_schema {name, def}; every tool's name works there too. Library definitions are authored as drafts (library.new_draft, library.edit; library.edit_graphics adds, replaces, moves and removes symbol graphics and pins by stable ID in one atomic batch, with rounded corners as rect radius or per-vertex polyline/polygon radii and multi-line literal text) and; parametric footprints are edited through library.edit_footprint (sketch edits, parameters, template pads, arrays and roles, solved by the host evaluator in one atomic step; preview with project_query library.footprint_preview) and land package terminals only through library.assign_terminals (confirm: false previews); drafts are published as exact revisions (library.publish); instances pin one exact component revision (component.place; omit designator and the core numbers it under the component's designator prefix, e.g. R3) and take typed field values through instance.set_properties, whose text is parsed in core (for example 10k or 10 kΩ for a resistance field), and choose terminal functions by ID through instance.select_functions, one atomic batch checked once against resource capacity, exclusions and the chosen implementation. project_query answers component.inspect (instance completeness), component.check (one draft or revision's findings) and component.check_drafts without changing anything; project_query project.issues with scope kind all (or an area, or a selection of objects) lists every issue with its severity, group and one-line title, the same list the desktop Issues surface shows. To find a part, project_query library.search with a query such as `10k 0603` or `R>=10k tol<=1%` (field keys come from the project's field names and units), library.facets for the filters the results offer and library.inspect for one lineage; then place the result's component revision with component.place. A manufacturer Part (definition kind part: manufacturer, complete MPN, the pinned component and binding revisions, rated specs with their meaning, references and distributor offers, no price or stock) is drafted and published like any definition, in the same library.publish set as the component it implements. To buy the design, project_query library.resolve_groups lists groups of instances without an agreeing Part; search each group's query, then pin one Part on the whole group with one instance.assign_part (atomic and one undo step; a Part that disagrees with any instance's component revision, binding or requirements is refused and nothing changes; part null clears it). component.inspect then reports purchasing.state no_part, unresolved or resolved with its findings, and project_query project.bom lists the BOM with unresolved lines marked. A package's 3D model is set by library.edit_package (typed form, model source, placement and one-click quick actions such as rotate, stand_up, seat, centre_on_pads and pin1_to_pad1, in one atomic batch); library.fit_leads fits the typed lead contacts to the footprint's pads (confirm: false only reports the residuals, confirm: true stores the placement). Library folders (shared as plain files or git) are sources: library_sources lists them with their state or the reason one is disabled; adding, creating, removing or turning one on changes the person's own settings, so only they do it, in ",
    sparksmith_core::product_name!(),
    "; library.search then also returns folder entries (source library, with source_lineage). Place one with library_place_from_folder (import and place in one undo step; another project revision of the same lineage is refused with library.other_revision unless other_revision is keep_both); project_query library.updates_available lists copies whose folder has a newer or modified revision; library_import_from_folder brings the newer revision in and project_preview_update with a ComponentRevision change moves chosen instances to it. library_publish_to_folder promotes a published project revision to a writable folder and records its origin. The project never resolves anything through a folder, so it reopens offline. Definition, instance, unit, net and track IDs are stable; names and designators are labels. Changes that affect wiring, placements or routed copper go through project_preview_update, then project_apply_update. A change that joins two named nets (a GND port on a VBUS wire) is refused; its preview lists them in merges_named, and project_apply_update merges them only with merge_named_nets: true. ",
    "Datasheet pinouts (MCUs and other multi-function parts): the datasheet_to_component prompt (prompts/get) is the whole reviewed workflow from datasheets to drafts; follow it, and end with library_review_check, which checks your citations against the drafts. Read the datasheet yourself; ",
    sparksmith_core::product_name!(),
    " never parses free text and never infers electrical compatibility from matching names. Call library_pinout_schema and fill one pinout document (format 1): the device and its reference (title, document revision, URL); one variant per package with its terminal count, exposed pads included; each peripheral instance with its kind (uart, spi, i2c, timer, adc: grouping text only) and roles (label, datasheet signal, capacity, required_when_used); and one row per pin: a stable row key, the number in every variant, name, class, the GPIO port and index only where the datasheet states them, the settable options (pull, speed), each function with direction, signal and drive and its peripheral, role, mux code and (where the datasheet numbers several mux tables from the same codes) mux_table as separate explicit fields (never split a name such as USART1_TX by guesswork), the default function, the source page and table, and uncertain: true with a note on every row you could not read with confidence. Encode the datasheet's restrictions (mutually exclusive functions, pin sets that go together, pins whose reuse disables debugging) as rules: each a short sandboxed Rhai script with a unique label and its source page that evaluates to ok(), forbid(message, [pins]), require(message, [pins]) or warn(message, [pins]) and reads the assignment only through active(\"PIN\", \"function\"), used(\"PERIPHERAL\"), role(\"PERIPHERAL\", \"ROLE\"), option(\"PIN\", \"pull\"), excludes and requires (rule_api in library_pinout_schema has the full contract; the preview evaluates every rule with each function it names active and reports failures as blocking findings); forbid rejects the assignment, require is reported until met and warn is a note. Then project_query library.pinout_preview with target {target: new, name} (or {target: draft, id} to re-import onto a draft, which keeps terminals by number) and the document: show the user the plan's review items, findings and change counts, and apply only after they have seen them, with project_apply library.pinout_apply passing the plan's base and accept all (or except the rows they rejected); a stale base is refused, so preview again. The result is a draft whose imported rows are all unreviewed; a row flagged uncertain blocks library.publish (pinout.uncertain) until a corrected re-import no longer flags it or the person clears it in the draft's review (you can technically clear it through library.edit, so do that only after the person has confirmed that row in the conversation; the gate is advisory against agents), and never publish on the user's behalf before they have seen the unreviewed count and confirmed the rows. For the footprint, read the package drawing and find a package row with project_query library.templates (for example qfn pins:48 pitch:0.5 body:7x7). The import's package and binding drafts have no footprint: preview project_query library.package_plan {request: {draft: the package, row: {source: starter, name} for a built-in row or {source: project, row} for a project row}} and apply library.package_from_template with the same request, which in one undo step sets the package's form and generated model, creates the footprint draft (the plan says footprint_created, and the outcome plan names the new footprint ID), lands one pad per terminal in lead order and lists the footprint in the package's binding draft (plan footprint_bound); when there is no binding draft or several, the finding binding.footprint_unbound names them and you list the footprint with library.edit. The package's terminals carry over by number (request terminals: by_number, the default): a terminal keeps its identity, and so the binding's mappings, when the new form gives its number; any new or dropped number is named in package.terminals_unmatched (and binding.mapping_missing or binding.mapping_unknown), never filled by list order. When the datasheet numbers a pad differently from the form (an exposed pad numbered 49 where the form says EP), pass renumber [{from: \"49\", to: \"EP\"}] once the datasheet shows they are the same pad. terminals: by_position reuses terminals in list order and renumbers them; use it only when the user asks, and check each mapping (binding.mapping_retained). The summary in the plan's terminals says which strategy ran and what it kept. Check the model with project_query library.models {query, fits: the package}, and publish component, symbol, package, footprint and binding in one library.publish. A new part without a datasheet import starts from library.new_from_template instead. Move pads to other terminals only through library.assign_terminals by number. In a design: project_query functions.suggest {instance, request: {enable: [{peripheral}], keep_existing, avoid, limit}} returns ranked assignments that place every role with a candidate (complete ones first; optional lists the terminals of optional roles and unplaced the optional roles left out), or the blockers with the rule that fails, the existing selection that blocks (culprit) and an alternative that moves it where one exists, and applies nothing; project_apply instance.assign_peripheral {instance, peripheral, roles: [{role, terminal or null for the solver}], enable} applies one as one undo step (a role given an explicit terminal moves there from its earlier pin) and is refused whole when a rule forbids it; instance.set_pin_config sets pull, speed, initial level, Schmitt input and the firmware name per terminal, checked against the pin's declared options; it never changes function ERC directly, but pin rules may read it through option(…), so a rule can refuse the edit or report a requirement unmet. project_query project.pin_report {instance} lists per pin the pads, GPIO, active function with peripheral, role, signal and mux code, the pin configuration, the net and every other endpoint on that net with its active function, component and MPN: use it to describe the MCU's connections to its peripherals as facts, not as a compatibility claim. project_query project.pin_config {instance} is the firmware pin-configuration IR (format 1; firmware names come from the override, else a named net's name, else the pin name, and clashes are listed in name_conflicts, never renamed); firmware_export_preview {instance, emitter: {emitter: json, c_header, zephyr_overlay or rust_consts}, or {emitter: user, template: its SHA-256} with template_file naming a Rhai template over the IR} returns the text and a suggested file name, and firmware_export with path writes it to a new file and refuses an existing one. The output is a pin-configuration starting point: it never configures clocks, registers or a HAL. ",
    "Generic parts (headers, chip resistors, capacitors, inductors, LEDs, diodes): never fill a form for these. Query library.generic_parts with plain text such as \"header 1x4\", \"10k 0603\", \"100n 0402 16V\" or \"led 0603 red\"; each candidate says how the text was read, whether the part exists and what would be created. Rows come first in \"5x2\" (5 rows of 2) and nothing is reordered; size codes are imperial unless spelled 1608M. library.find_or_create with the same text returns that component, building it only if the project lacks it, with the text's values as properties for component.place. ICs need a datasheet: use the pinout flow. ",
    "Modules (radio or MCU modules: a small board with a shield can, edge or LGA pads, often an antenna end): there is no package row per module; use the module family's custom entry. Preview project_query library.package_plan {request: {draft: the package, row: {source: custom, family: module}, values: {family: module, ...}}} and apply library.package_from_template with the same request. The values are the datasheet's mechanical drawing and recommended land pattern, in mm, seen from above with the antenna end at +y and the origin at the outline centre: width, length, height, pcb_thickness, antenna_length (0 without an on-board antenna), can_size and can_centre, pitch, pad_size [across the edge, along it], rows (in numbering order, each {side: left, bottom, right or top, count, first: the lowest-numbered pad's centre}; a row runs counter-clockwise from above, so left rows number downwards, bottom rows towards +x, right rows upwards and top rows towards -x; pads number from 1 through the rows), castellation (half-hole diameter where a pad meets the outline, 0 for LGA), land_width and land_extend [beyond the pad's outer end, inside its inner end] for the host land, ground (one ground-pad array as one terminal: rows, cols, pitch, size, centre, rotation_deg, number) and singles (each {number, centre, size}: corner ground pads and other lone pads as their own terminals, after the ground terminal), and keepout {beyond: [-x, +x, +y growth]} for the host-board antenna keep-out. The package's terminal count must equal the rows' pads, plus one for the ground array, plus the singles; a pinout import's variant terminal_count says the same. The footprint gets the lands, courtyard, silkscreen, the pin-1 mark and a copper keep-out over the antenna on every copper layer (forbidding fills, tracks and vias on the host board); the generated 3D model is a green soldermasked board, a nickel-silver can, gold pads with plated castellations, and the printed antenna under the mask. library_set_marking puts text (for example {mpn}) or SVG artwork given as text on the can's top as a display-only marking.",
);

pub struct Server {
    session: Session,
    initialized: bool,
    /// Every tool call passes [`crate::gate`] under it, and Ask is refused:
    /// this transport has no way to ask.
    policy: Policy,
    /// When set, tool calls go to the open window, whose gate decides.
    live: Option<live::Client>,
}

/// What a live server's agent is told before the rest.
pub(crate) const LIVE_INSTRUCTIONS: &str = concat!(
    "This server edits the project open in the person's ",
    sparksmith_core::product_name!(),
    " window: they see each change, and it joins their undo history. Every tool that changes the project takes expected_revision (from the last reply's revision, or project_inspect's); a stale one is refused, so inspect again, since the person may have edited meanwhile. Changes are refused while the person is typing, dragging or has a menu, dialog or review open: wait and retry. A tool whose class is ask waits for the person's Allow in the window. Opening, saving and starting projects is the person's, in the window. "
);

impl Server {
    /// A server whose tool calls all pass the gate under `policy`.
    pub fn new(session: Session, policy: Policy) -> Self {
        Self {
            session,
            initialized: false,
            policy,
            live: None,
        }
    }
    /// A server whose tool calls run in the open window `client` reaches,
    /// whose own gate decides.
    pub fn live(client: live::Client) -> Self {
        Self {
            live: Some(client),
            ..Self::new(Session::default(), Policy::default())
        }
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Serve until `input` closes. Only protocol messages go to `output`.
    pub fn serve(
        &mut self,
        mut input: impl BufRead,
        mut output: impl Write,
    ) -> std::io::Result<()> {
        while let Some(line) = lines::next(&mut input, MAX_LINE)? {
            let reply = match line {
                Line::Text(text) => self.handle_line(&text),
                Line::TooLarge => Some(error(Value::Null, INVALID_REQUEST, "request too large")),
                Line::NotUtf8 => Some(error(Value::Null, PARSE_ERROR, "input is not UTF-8")),
            };
            if let Some(reply) = reply {
                output.write_all(reply.to_string().as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// One message in, at most one message out.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(message) => self.handle(message),
            Err(e) => Some(error(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            )),
        }
    }

    pub fn handle(&mut self, message: Value) -> Option<Value> {
        let Value::Object(object) = &message else {
            return Some(error(
                Value::Null,
                INVALID_REQUEST,
                "message must be an object",
            ));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            if object.contains_key("result") || object.contains_key("error") {
                return None;
            }
            return Some(error(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "missing method",
            ));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            if method == "notifications/initialized" {
                self.initialized = true;
            }
            return None;
        };
        Some(match self.request(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, text)) => error(id, code, &text),
        })
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION);
                let version = if SUPPORTED_VERSIONS.contains(&asked) {
                    asked
                } else {
                    PROTOCOL_VERSION
                };
                let instructions = if self.live.is_some() {
                    format!("{LIVE_INSTRUCTIONS}{INSTRUCTIONS}")
                } else {
                    INSTRUCTIONS.to_owned()
                };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "prompts": {"listChanged": false}},
                    "serverInfo": {"name": sparksmith_core::product::SHORT_NAME, "title": sparksmith_core::product::NAME, "version": env!("CARGO_PKG_VERSION")},
                    "instructions": instructions,
                }))
            }
            "ping" => Ok(json!({})),
            "prompts/list" => Ok(prompts::list()),
            "prompts/get" => prompts::get(params).map_err(|why| (INVALID_PARAMS, why)),
            "tools/list" => {
                Ok(json!({"tools": if self.live.is_some() { live_tools() } else { tools() }}))
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "missing tool name".to_string()))?;
                let command = Request::catalog_shared()
                    .iter()
                    .filter(|c| listed(c))
                    .find(|c| tool_name(c.id) == name)
                    .ok_or((INVALID_PARAMS, format!("unknown tool {name}")))?;
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => json!({}),
                    Some(arguments) => arguments.clone(),
                };
                Ok(self.call(command, arguments))
            }
            _ => Err((METHOD_NOT_FOUND, format!("method {method} not found"))),
        }
    }

    /// Tool failures are results with `isError`, not protocol errors, so the
    /// agent sees the diagnostic code and message.
    fn call(&mut self, command: &CommandInfo, arguments: Value) -> Value {
        let reply = match &mut self.live {
            Some(client) => call_live(client, command, arguments),
            None => {
                let request = json!({"command": command.id, "params": arguments});
                self.session.respond(Caller::Model(&self.policy), request)
            }
        };
        let failed = reply["ok"] != true;
        let payload = if failed {
            reply["error"].clone()
        } else {
            reply["result"].clone()
        };
        json!({
            "content": [{"type": "text", "text": payload.to_string()}],
            "isError": failed,
        })
    }
}

pub fn tool_name(command: &str) -> String {
    command.replace('.', "_")
}

/// Requests the session serves (CLI `stdio`, the engine) that MCP does not
/// list or call yet: the generator requests wait for their compact tools,
/// which arrive with GP7 (docs/generator-plugins.md).
pub const NOT_TOOLS: &[&str] = &["library.generators", "library.generate_part"];

/// Whether `command` is an MCP tool.
fn listed(command: &CommandInfo) -> bool {
    !NOT_TOOLS.contains(&command.id)
}

/// Tool definitions generated from the request catalog.
pub fn tools() -> Vec<Value> {
    Request::catalog_shared()
        .iter()
        .filter(|command| listed(command))
        .map(tool)
        .collect()
}

/// One tool: its catalog title and, where its class gives them, the
/// annotations ([`crate::session::metadata`]).
fn tool(command: &CommandInfo) -> Value {
    let mut tool = json!({
        "name": tool_name(command.id),
        "title": command.title,
        "description": format!("{} {}", command.description, command.permission_note()),
        "inputSchema": discovery::tool_schema(command),
    });
    if let Some(annotations) = command.annotations() {
        tool["annotations"] = json!(annotations);
    }
    tool
}

/// A live server's tools: the same, less the window's own file requests.
pub fn live_tools() -> Vec<Value> {
    Request::catalog_shared()
        .iter()
        .filter(|command| listed(command) && !live::WINDOW_ONLY.contains(&command.id))
        .map(tool)
        .collect()
}

/// One tool call sent to the window. A lost connection is a failure with
/// the reason, never retried.
fn call_live(client: &mut live::Client, command: &CommandInfo, arguments: Value) -> Value {
    let params = json!({"command": command.id, "params": arguments});
    match client.call(live::EXECUTE, params) {
        Ok(reply) => reply,
        Err(message) => {
            json!({"ok": false, "error": {"code": "live.unreachable", "message": message}})
        }
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub mod prompts;
#[cfg(test)]
pub(crate) mod scenario;
pub mod surface;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod workflow_tests;
