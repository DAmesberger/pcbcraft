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

const INSTRUCTIONS: &str = "Pcbcraft electronics editor. Inspect with project_inspect; every mutation goes through project_apply with the revision from the last result and returns its outcome. Library definitions are authored as drafts (library.new_draft, library.edit; library.edit_graphics adds, replaces, moves and removes symbol graphics and pins by stable ID in one atomic batch, with rounded corners as rect radius or per-vertex polyline/polygon radii and multi-line literal text) and; parametric footprints are edited through library.edit_footprint (sketch edits, parameters, template pads, arrays and roles, solved by the host evaluator in one atomic step; preview with project_query library.footprint_preview) and land package terminals only through library.assign_terminals (confirm: false previews); drafts are published as exact revisions (library.publish); instances pin one exact component revision (component.place; omit designator and the core numbers it under the component's designator prefix, e.g. R3) and take typed field values through instance.set_properties, whose text is parsed in core (for example 10k or 10 kΩ for a resistance field), and choose terminal functions by ID through instance.select_functions, one atomic batch checked once against resource capacity, exclusions and the chosen implementation. project_query answers component.inspect (instance completeness), component.check (one draft or revision's findings) and component.check_drafts without changing anything. To find a part, project_query library.search with a query such as `10k 0603` or `R>=10k tol<=1%` (field keys come from the project's field names and units), library.facets for the filters the results offer and library.inspect for one lineage; then place the result's component revision with component.place. A manufacturer Part (definition kind part: manufacturer, complete MPN, the pinned component and binding revisions, rated specs with their meaning, references and distributor offers, no price or stock) is drafted and published like any definition, in the same library.publish set as the component it implements. To buy the design, project_query library.resolve_groups lists groups of instances without an agreeing Part; search each group's query, then pin one Part on the whole group with one instance.assign_part (atomic and one undo step; a Part that disagrees with any instance's component revision, binding or requirements is refused and nothing changes; part null clears it). component.inspect then reports purchasing.state no_part, unresolved or resolved with its findings, and project_query project.bom lists the BOM with unresolved lines marked. Definition, instance, unit, net and track IDs are stable; names and designators are labels. Changes that affect wiring, placements or routed copper go through project_preview_update, then project_apply_update.";

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
mod tests;
