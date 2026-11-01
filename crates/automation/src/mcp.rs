//! Model Context Protocol server (JSON-RPC 2.0 over newline-delimited stdio).
//!
//! Each [`Request`] variant is one tool, named by its command ID with `.`
//! replaced by `_` and described by the same generated parameter schema the
//! JSON-lines transport accepts. Conventions (bounded lines, version
//! negotiation, notifications without replies) follow Solidcraft's MCP server.
use crate::lines::{self, Line};
use crate::session::{Request, Session};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];
/// Large enough for a complete component draft, small enough to bound memory.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub(crate) const INSTRUCTIONS: &str = concat!(
    "Pcbcraft electronics editor. Inspect with project_inspect; every mutation goes through project_apply with the revision from the last result and returns its outcome. Library definitions are authored as drafts (library.new_draft, library.edit; library.edit_graphics adds, replaces, moves and removes symbol graphics and pins by stable ID in one atomic batch, with rounded corners as rect radius or per-vertex polyline/polygon radii and multi-line literal text) and; parametric footprints are edited through library.edit_footprint (sketch edits, parameters, template pads, arrays and roles, solved by the host evaluator in one atomic step; preview with project_query library.footprint_preview) and land package terminals only through library.assign_terminals (confirm: false previews); drafts are published as exact revisions (library.publish); instances pin one exact component revision (component.place; omit designator and the core numbers it under the component's designator prefix, e.g. R3) and take typed field values through instance.set_properties, whose text is parsed in core (for example 10k or 10 kΩ for a resistance field), and choose terminal functions by ID through instance.select_functions, one atomic batch checked once against resource capacity, exclusions and the chosen implementation. project_query answers component.inspect (instance completeness), component.check (one draft or revision's findings) and component.check_drafts without changing anything; project_query project.issues with scope kind all (or an area, or a selection of objects) lists every issue with its severity, group and one-line title, the same list the desktop Issues surface shows. To find a part, project_query library.search with a query such as `10k 0603` or `R>=10k tol<=1%` (field keys come from the project's field names and units), library.facets for the filters the results offer and library.inspect for one lineage; then place the result's component revision with component.place. A manufacturer Part (definition kind part: manufacturer, complete MPN, the pinned component and binding revisions, rated specs with their meaning, references and distributor offers, no price or stock) is drafted and published like any definition, in the same library.publish set as the component it implements. To buy the design, project_query library.resolve_groups lists groups of instances without an agreeing Part; search each group's query, then pin one Part on the whole group with one instance.assign_part (atomic and one undo step; a Part that disagrees with any instance's component revision, binding or requirements is refused and nothing changes; part null clears it). component.inspect then reports purchasing.state no_part, unresolved or resolved with its findings, and project_query project.bom lists the BOM with unresolved lines marked. A package's 3D model is set by library.edit_package (typed form, model source, placement and one-click quick actions such as rotate, stand_up, seat, centre_on_pads and pin1_to_pad1, in one atomic batch); library.fit_leads fits the typed lead contacts to the footprint's pads (confirm: false only reports the residuals, confirm: true stores the placement). Library folders (shared as plain files or git) are sources: library_sources lists them with their state or the reason one is disabled, library_source_add installs one; library.search then also returns folder entries (source library, with source_lineage). Place one with library_place_from_folder (import and place in one undo step; another project revision of the same lineage is refused with library.other_revision unless other_revision is keep_both); project_query library.updates_available lists copies whose folder has a newer or modified revision; library_import_from_folder brings the newer revision in and project_preview_update with a ComponentRevision change moves chosen instances to it. library_publish_to_folder promotes a published project revision to a writable folder and records its origin. The project never resolves anything through a folder, so it reopens offline. Definition, instance, unit, net and track IDs are stable; names and designators are labels. Changes that affect wiring, placements or routed copper go through project_preview_update, then project_apply_update. ",
    "Datasheet pinouts (MCUs and other multi-function parts): read the datasheet yourself; Pcbcraft never parses free text and never infers electrical compatibility from matching names. Call library_pinout_schema and fill one pinout document (format 1): the device and its reference (title, document revision, URL); one variant per package with its terminal count, exposed pads included; each peripheral instance with its kind (uart, spi, i2c, timer, adc: grouping text only) and roles (label, datasheet signal, capacity, required_when_used); and one row per pin: a stable row key, the number in every variant, name, class, the GPIO port and index only where the datasheet states them, the settable options (pull, speed), each function with direction, signal and drive and its peripheral, role and mux code as separate explicit fields (never split a name such as USART1_TX by guesswork), the default function, the source page and table, and uncertain: true with a note on every row you could not read with confidence. Encode the datasheet's restrictions (mutually exclusive functions, pin sets that go together, pins whose reuse disables debugging) as rules: each a short sandboxed Rhai script with a unique label and its source page that evaluates to ok(), forbid(message, [pins]), require(message, [pins]) or warn(message, [pins]) and reads the assignment only through active(\"PIN\", \"function\"), used(\"PERIPHERAL\"), role(\"PERIPHERAL\", \"ROLE\"), option(\"PIN\", \"pull\"), excludes and requires; forbid rejects the assignment, require is reported until met and warn is a note. Then project_query library.pinout_preview with target {target: new, name} (or {target: draft, id} to re-import onto a draft, which keeps terminals by number) and the document: show the user the plan's review items, findings and change counts, and apply only after they have seen them, with project_apply library.pinout_apply passing the plan's base and accept all (or except the rows they rejected); a stale base is refused, so preview again. The result is a draft whose imported rows are all unreviewed; a row flagged uncertain blocks library.publish (pinout.uncertain) until a corrected re-import no longer flags it or the person clears it in the draft's review (you can technically clear it through library.edit, so do that only after the person has confirmed that row in the conversation; the gate is advisory against agents), and never publish on the user's behalf before they have seen the unreviewed count and confirmed the rows. For the footprint, read the package drawing and find a package row with project_query library.templates (for example qfn pins:48 pitch:0.5 body:7x7). The import's package and binding drafts have no footprint: draft one with library.new_draft (kind footprint, package set to the binding's package reference, next_local 1), preview project_query library.package_plan {request: {draft: the package, row: {source: starter, name} for a built-in row or {source: project, row} for a project row}} and apply library.package_from_template, which sets the package's form and generated model and lands one pad per terminal in lead order as one undo step, keeping terminal identities and so the binding's mappings; list the footprint in the binding's footprints with library.edit, check the model with project_query library.models {query, fits: the package}, and publish component, symbol, package, footprint and binding in one library.publish. A new part without a datasheet import starts from library.new_from_template instead. Move pads to other terminals only through library.assign_terminals by number. In a design: project_query functions.suggest {instance, request: {enable: [{peripheral}], keep_existing, avoid, limit}} returns ranked assignments that place every role with a candidate (complete ones first; optional lists the terminals of optional roles and unplaced the optional roles left out), or the blockers with the rule that fails, the existing selection that blocks (culprit) and an alternative that moves it where one exists, and applies nothing; project_apply instance.assign_peripheral {instance, peripheral, roles: [{role, terminal or null for the solver}], enable} applies one as one undo step (a role given an explicit terminal moves there from its earlier pin) and is refused whole when a rule forbids it; instance.set_pin_config sets pull, speed, initial level, Schmitt input and the firmware name per terminal, checked against the pin's declared options; it never changes function ERC directly, but pin rules may read it through option(…), so a rule can refuse the edit or report a requirement unmet. project_query project.pin_report {instance} lists per pin the pads, GPIO, active function with peripheral, role, signal and mux code, the pin configuration, the net and every other endpoint on that net with its active function, component and MPN: use it to describe the MCU's connections to its peripherals as facts, not as a compatibility claim. project_query project.pin_config {instance} is the firmware pin-configuration IR (format 1; firmware names come from the override, else a named net's name, else the pin name, and clashes are listed in name_conflicts, never renamed); firmware_export_preview {instance, emitter: {emitter: json, c_header, zephyr_overlay or rust_consts}, or {emitter: user, template: its SHA-256} with template_file naming a Rhai template over the IR} returns the text and a suggested file name, and firmware_export with path writes it to a new file and refuses an existing one. The output is a pin-configuration starting point: it never configures clocks, registers or a HAL.",
);

pub struct Server {
    session: Session,
    initialized: bool,
}

impl Server {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            initialized: false,
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
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "pcbcraft", "title": "Pcbcraft", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "missing tool name".to_string()))?;
                let command = Request::catalog()
                    .into_iter()
                    .find(|c| tool_name(c.id) == name)
                    .ok_or((INVALID_PARAMS, format!("unknown tool {name}")))?;
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => json!({}),
                    Some(arguments) => arguments.clone(),
                };
                Ok(self.call(command.id, arguments))
            }
            _ => Err((METHOD_NOT_FOUND, format!("method {method} not found"))),
        }
    }

    /// Tool failures are results with `isError`, not protocol errors, so the
    /// agent sees the diagnostic code and message.
    fn call(&mut self, command: &str, arguments: Value) -> Value {
        let reply = self
            .session
            .dispatch(json!({"command": command, "params": arguments}));
        let failed = reply["ok"] != true;
        let payload = if failed {
            reply["error"].clone()
        } else {
            reply["result"].clone()
        };
        let mut result = json!({
            "content": [{"type": "text", "text": payload.to_string()}],
            "isError": failed,
        });
        if payload.is_object() {
            result["structuredContent"] = payload;
        }
        result
    }
}

pub fn tool_name(command: &str) -> String {
    command.replace('.', "_")
}

/// Tool definitions generated from the request catalog.
pub fn tools() -> Vec<Value> {
    Request::catalog()
        .into_iter()
        .map(|command| {
            json!({
                "name": tool_name(command.id),
                "title": command.id,
                "description": command.description,
                "inputSchema": command.params,
            })
        })
        .collect()
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
pub(crate) mod scenario;
#[cfg(test)]
mod tests;
