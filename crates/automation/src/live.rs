//! Live MCP: AI assistants edit the project open in the desktop window, on
//! the window's own engine and undo history.
//!
//! The channel is Solidcraft's control channel (`solidcraft-control`,
//! docs/control-protocol.md there): a loopback-only listener, a per-session
//! token kept in a file only the user can read, one JSON line per request
//! and per reply. Each connection's thread hands its requests to the window
//! ([`Endpoint::next`]), which answers them on the UI thread between frames
//! with [`admit`] and [`run`]: the same [`Session::respond`] the headless
//! transports use, over the window's engine ([`Session::lend`]).
//!
//! Two methods are served: [`EXECUTE`] (`{command, params}`, one
//! automation request) and [`REVISION`]. A request that changes the project
//! names the revision it was prepared against in its own parameters, which
//! [`Session::execute`] checks as for every transport. Any request but a
//! read is refused while the person is typing, dragging or has a menu,
//! dialog or review open. The
//! permission gate decides as for any LLM request; Ask waits for the
//! person's Allow in the window. The CLI's `mcp --live` bridges an MCP
//! client to this channel ([`Client`]).
use crate::gate::{Caller, Decision, Policy, Question, Refusal};
use crate::session::{Request, Session, envelope, take_verbose};
use serde_json::{Value, json};
use solidcraft_control::Effect;
use solidcraft_control::auth;
pub use solidcraft_control::auth::{App, ClientToken, Listen};
use solidcraft_control::server::{Hub, Protocol, Summary, Waits};
use sparksmith_core::Class;
use std::ffi::OsString;
use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub use solidcraft_control::ControlRequest;

/// Run one automation request: `{command, params}`.
pub const EXECUTE: &str = "engine.execute";
/// The project's revision now, and whether it can undo and redo.
pub const REVISION: &str = "project.revision";
/// The first port tried; the next free one of [`auth::PORT_TRIES`] when it
/// is taken. Below the ephemeral ranges, apart from Solidcraft's 27878.
pub const DEFAULT_PORT: u16 = 27979;
/// Requests that belong to the window's own File workflow, which asks about
/// unsaved work and owns the file: refused on the live channel.
pub const WINDOW_ONLY: &[&str] = &["project.new", "project.open", "project.save"];
/// Connections served at once.
const MAX_CONNECTIONS: usize = 8;
/// The longest request line, as the MCP server's.
const MAX_LINE: usize = crate::mcp::MAX_LINE;
/// The longest reply a bridge reads (a `verbose` snapshot of a large board).
const MAX_REPLY: usize = 256 * 1024 * 1024;
/// How long an ordinary request may wait for the window; one that needs a
/// person's Allow may wait [`ASK_WAIT`].
const WAIT: Duration = Duration::from_secs(120);
const ASK_WAIT: Duration = Duration::from_secs(600);
const ACCEPT_POLL: Duration = Duration::from_millis(25);
/// Where a refused client reads how to connect.
const DOCS: &str = concat!(
    "the ",
    sparksmith_core::product_name!(),
    " README, section Live MCP"
);

/// Sparksmith's control channel: token files in `<config>/control`
/// (`SPARKSMITH_CONFIG_DIR` moves the config folder, `SPARKSMITH_CONTROL_DIR`
/// the token folder) and `SPARKSMITH_CONTROL_TOKEN` for a harness.
pub const APP: App = App {
    name: sparksmith_core::product::NAME,
    data_dir: config_dir,
    env: control_env,
    utc: seconds,
    default_port: DEFAULT_PORT,
};

fn config_dir() -> Option<PathBuf> {
    sparksmith_io::settings::UserDirs::from_env()
        .ok()
        .map(|dirs| dirs.config)
}

fn control_env(name: &str) -> Option<OsString> {
    std::env::var_os(format!("SPARKSMITH_{name}"))
}

fn seconds(secs: u64) -> String {
    format!("at {secs} s after the Unix epoch")
}

/// What the live channel's requests are, for the control server's
/// messages and waits.
struct LiveProtocol;

impl Protocol for LiveProtocol {
    fn summary(&self, method: &str, params: &Value) -> Summary {
        let command = params.get("command").and_then(Value::as_str);
        let info = command.and_then(|id| Request::catalog_shared().iter().find(|c| c.id == id));
        // What the request does is its effective class, as the gate decides
        // it: `project.apply` of `library.publish` is Ask, a read naming a
        // path is too. An unreadable one is refused at once.
        let typed = info.and_then(|_| {
            let mut request = json!({
                "command": command,
                "params": params.get("params").cloned().unwrap_or_else(|| json!({})),
            });
            take_verbose(&mut request);
            serde_json::from_value::<Request>(request).ok()
        });
        let class = typed.as_ref().map(Request::effective_class);
        let names_path = typed
            .as_ref()
            .is_some_and(|typed| !typed.paths().is_empty());
        let effect = match (method, class) {
            (REVISION, _) | (_, Some(Class::Read)) => Effect::ReadsOnly,
            (_, Some(Class::Ask)) if names_path => Effect::WritesFile,
            _ => Effect::Changes,
        };
        Summary {
            what: match info {
                Some(info) => format!("`{method}` ({})", info.id),
                None => format!("`{}`", method.chars().take(64).collect::<String>()),
            },
            effect,
            command: info.map(|info| info.id),
            steps: None,
            // Anything but a read may wait for a person: Ask always, an edit
            // under ask-every-time. Timing it out first would leave it held,
            // still to run on Allow, after the agent was told it failed.
            long: method != REVISION && class.is_some_and(|class| class != Class::Read),
        }
    }
    fn document(&self) -> &'static str {
        "project"
    }
    fn change_event(&self) -> &'static str {
        "project.changed"
    }
    fn revision_method(&self) -> &'static str {
        REVISION
    }
    fn docs(&self) -> &'static str {
        DOCS
    }
    fn capabilities(&self) -> Value {
        json!({"methods": [EXECUTE, REVISION]})
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The open connections, kept so turning the channel off closes them.
#[derive(Default)]
struct Open {
    next: u64,
    streams: Vec<(u64, TcpStream)>,
}

/// The window's end of the live channel, from turning it on until it is
/// dropped: the listener, the token and its files, and the requests the
/// connections hand over. Dropping it stops listening, closes every
/// connection and removes the token and discovery files.
pub struct Endpoint {
    port: u16,
    token: String,
    app: App,
    requests: Receiver<ControlRequest>,
    stop: Arc<AtomicBool>,
    open: Arc<Mutex<Open>>,
    worker: std::thread::Thread,
    token_file: Option<PathBuf>,
    discovery: bool,
}

impl std::fmt::Debug for Endpoint {
    /// Never the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Endpoint {
    /// Listen on loopback as `listen` says, with a new token (or the app's
    /// `CONTROL_TOKEN`), written to `app`'s token file for the port, which
    /// also becomes the discovery file's port. `wake` makes the window look
    /// at its requests now.
    pub fn start(
        app: App,
        listen: Listen,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let listener = match listen {
            Listen::Exact(port) => TcpListener::bind(("127.0.0.1", port))?,
            Listen::Default => auth::bind_with_fallback(app.default_port, auth::PORT_TRIES)?,
        };
        let port = listener.local_addr()?.port();
        let token = app
            .env_token()
            .or_else(solidcraft_control::server::new_token)
            .ok_or_else(|| std::io::Error::other("the system gave no random token"))?;
        let token_file = app.token_path(port);
        if let Some(path) = &token_file {
            app.write_token_file(path, &token)?;
        }
        let discovery = token_file.is_some() && app.write_current_port(port).is_ok();
        listener.set_nonblocking(true)?;
        let (send, requests) = mpsc::channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let hub_wake = Arc::clone(&wake);
        let hub = Arc::new(
            Hub::new(LiveProtocol, move || hub_wake(), MAX_LINE).with_waits(Waits {
                short: WAIT,
                long: ASK_WAIT,
                token: Duration::from_secs(5),
            }),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let open = Arc::new(Mutex::new(Open::default()));
        let (worker_stop, worker_open, worker_token) = (
            Arc::clone(&stop),
            Arc::clone(&open),
            Arc::new(token.clone()),
        );
        let worker = std::thread::Builder::new()
            .name("sparksmith-live-mcp".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    let stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(_) => {
                            std::thread::park_timeout(ACCEPT_POLL);
                            continue;
                        }
                    };
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let id = {
                        let mut open = lock(&worker_open);
                        if open.streams.len() >= MAX_CONNECTIONS {
                            drop(open);
                            let mut refused = stream;
                            let _ = refused
                                .write_all(b"{\"ok\":false,\"error\":\"too many connections\"}\n");
                            continue;
                        }
                        let Ok(clone) = stream.try_clone() else {
                            continue;
                        };
                        open.next = open.next.wrapping_add(1);
                        let id = open.next;
                        open.streams.push((id, clone));
                        id
                    };
                    wake();
                    let (hub, send, token, open, wake) = (
                        Arc::clone(&hub),
                        send.clone(),
                        Arc::clone(&worker_token),
                        Arc::clone(&worker_open),
                        Arc::clone(&wake),
                    );
                    let served = std::thread::Builder::new()
                        .name("sparksmith-live-peer".into())
                        .spawn(move || {
                            hub.serve(stream, &token, send, false);
                            lock(&open).streams.retain(|(i, _)| *i != id);
                            wake();
                        });
                    if served.is_err() {
                        lock(&worker_open).streams.retain(|(i, _)| *i != id);
                    }
                }
                for (_, stream) in lock(&worker_open).streams.drain(..) {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            })?;
        Ok(Self {
            port,
            token,
            app,
            requests,
            stop,
            open,
            worker: worker.thread().clone(),
            token_file,
            discovery,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// This session's token, for a client the host configures itself and
    /// for tests. Never log it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Connections open now (one that has not shown the token yet closes
    /// within five seconds).
    pub fn connections(&self) -> usize {
        lock(&self.open).streams.len()
    }

    /// The next request a connection handed over, if any.
    pub fn next(&self) -> Option<ControlRequest> {
        self.requests.try_recv().ok()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.unpark();
        for (_, stream) in lock(&self.open).streams.drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Some(path) = &self.token_file {
            auth::remove_token_file(path, &self.token);
        }
        if self.discovery {
            self.app.remove_current_port(self.port, &self.token);
        }
    }
}

/// An MCP client's configuration for the CLI at `cli`: it finds the window
/// through the discovery file and reads the token from its file, so the
/// configuration holds no secret and outlives this session.
pub fn client_config(cli: &str) -> String {
    let config = json!({
        "mcpServers": {sparksmith_core::product::SHORT_NAME: {"command": cli, "args": ["mcp", "--live"]}}
    });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// A request the window may run, as [`Session::respond`] takes it, with
/// the policy it was admitted under.
#[derive(Clone, Debug)]
pub struct Admitted {
    request: Value,
    subject: &'static str,
    class: Class,
    policy: Policy,
    /// Held for the person ([`Verdict::Ask`]): the host runs it only after
    /// their Allow.
    asked: bool,
}

impl Admitted {
    /// The operation, query or request it names (`layout.add_via`).
    pub fn subject(&self) -> &'static str {
        self.subject
    }
    pub fn class(&self) -> Class {
        self.class
    }
}

/// What the window does with one live request.
#[derive(Debug)]
pub enum Verdict {
    /// Answer at once with this reply.
    Reply(Value),
    /// Run it on the window's engine ([`run`]).
    Run(Admitted),
    /// Hold it until the person allows ([`run`]) or denies it.
    Ask(Admitted, Question),
}

fn refusal(code: &'static str, command: &str, why: String, instead: &str) -> Value {
    Refusal {
        code,
        command: command.to_owned(),
        why,
        instead: instead.to_owned(),
    }
    .reply()
}

/// Decide one request on the window's session. `busy` says why the person
/// holds the window now (typing, dragging, a dialog): changes are refused
/// then, reads still run.
pub fn admit(
    session: &Session,
    policy: &Policy,
    busy: Option<&str>,
    method: &str,
    params: &Value,
) -> Verdict {
    let engine = session.engine();
    match method {
        REVISION => {
            return Verdict::Reply(json!({"ok": true, "result": {
                "revision": engine.revision(),
                "can_undo": engine.can_undo(),
                "can_redo": engine.can_redo(),
            }}));
        }
        EXECUTE => {}
        other => {
            let shown: String = other.chars().take(64).collect();
            return Verdict::Reply(json!({"ok": false, "error": {
                "code": "live.unknown_method",
                "message": format!("unknown method {shown}: use {EXECUTE} or {REVISION}"),
            }}));
        }
    }
    let Some(command) = params.get("command").and_then(Value::as_str) else {
        return Verdict::Reply(refusal(
            "malformed_request",
            EXECUTE,
            "it names no command.".into(),
            "Send {command, params}.",
        ));
    };
    if WINDOW_ONLY.contains(&command) {
        return Verdict::Reply(refusal(
            "live.window_only",
            command,
            "the open window owns its file, and its File menu asks the person about unsaved work. Nothing was changed.".into(),
            concat!("Ask the person to open, save or start a project in ", sparksmith_core::product_name!(), ", then carry on."),
        ));
    }
    let mut request = json!({
        "command": command,
        "params": params.get("params").cloned().unwrap_or_else(|| json!({})),
    });
    if params.get("verbose") == Some(&Value::Bool(true)) {
        request["verbose"] = Value::Bool(true);
    }
    let mut typed = request.clone();
    take_verbose(&mut typed);
    let typed = match serde_json::from_value::<Request>(typed) {
        Ok(typed) => typed,
        Err(_) if lacks_revision(command, &request["params"]) => {
            return Verdict::Reply(refusal(
                "live.revision_required",
                command,
                "a change to the open window's project names the revision it was prepared against. Nothing was changed.".into(),
                "Send expected_revision in its parameters: the revision of the last reply, or project_inspect's.",
            ));
        }
        Err(error) => return Verdict::Reply(envelope(Err(error.into()))),
    };
    let class = typed.effective_class();
    let mut admitted = Admitted {
        request,
        subject: typed.subject(),
        class,
        policy: policy.clone(),
        asked: false,
    };
    if class != Class::Read
        && let Some(reason) = busy
    {
        return Verdict::Reply(refusal(
            "live.busy",
            admitted.subject,
            format!("{reason}. Nothing was changed."),
            "Wait a moment and send it again; check the revision first, as the person may have changed the project.",
        ));
    }
    // A stale change is refused now rather than after the person's Allow;
    // Session::execute checks it again when it runs.
    if let Some(expected) = typed.expected_revision()
        && let Err(stale) = engine.check_revision(expected)
    {
        return Verdict::Reply(envelope(Err(stale.into())));
    }
    match session.check_request(policy, &admitted.request) {
        Decision::Run => Verdict::Run(admitted),
        Decision::Ask(question) => {
            admitted.asked = true;
            Verdict::Ask(admitted, question)
        }
        Decision::Refuse(refused) => Verdict::Reply(refused.reply()),
    }
}

/// Whether `command` takes `expected_revision` in its parameters, which
/// `params` lacks: the central revision rule's requirement, named clearly
/// instead of as a parse error.
fn lacks_revision(command: &str, params: &Value) -> bool {
    let requires = Request::catalog_shared()
        .iter()
        .find(|info| info.id == command)
        .and_then(|info| info.params.get("required"))
        .and_then(Value::as_array)
        .is_some_and(|required| required.iter().any(|name| name == "expected_revision"));
    requires && params.get("expected_revision").is_none()
}

/// Run an admitted request on the window's session and answer with the
/// compact reply. [`Session::respond`] gates it again under the policy it
/// was admitted with (a held one as allowed: the host runs it only on the
/// person's Allow), and [`Session::execute`] checks the revision again: the
/// person may have edited while it waited.
pub fn run(session: &mut Session, admitted: &Admitted) -> Value {
    let caller = if admitted.asked {
        Caller::Allowed(&admitted.policy)
    } else {
        Caller::Model(&admitted.policy)
    };
    session.respond(caller, admitted.request.clone())
}

/// A bridge's connection to the window: one authenticated connection, kept
/// open and reopened after a failure. A request whose reply was lost is
/// never sent again.
pub struct Client {
    port: u16,
    token: ClientToken,
    next_id: u64,
    connection: Option<(BufReader<TcpStream>, TcpStream)>,
}

impl Client {
    /// The window on `port`, or the one the discovery file names, with
    /// `token`, else the app's `CONTROL_TOKEN`, else its token file.
    pub fn find(app: &App, port: Option<u16>, token: Option<&str>) -> Result<Self, String> {
        let port = port.or_else(|| app.read_current_port()).ok_or(concat!(
            "no ",
            sparksmith_core::product_name!(),
            " window has live MCP on: in ",
            sparksmith_core::product_name!(),
            ", choose Help › Connect AI assistants… and turn it on"
        ))?;
        let token = app.client_token(port, token).ok_or_else(|| {
            format!(
                "no token for the {} window on port {port}: turn live MCP on in that window, or give SPARKSMITH_CONTROL_TOKEN",
                sparksmith_core::product::NAME
            )
        })?;
        Ok(Self::with_token(port, token))
    }

    pub fn with_token(port: u16, token: ClientToken) -> Self {
        Self {
            port,
            token,
            next_id: 0,
            connection: None,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Send one request and wait for its reply line (`{ok, result | error}`).
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let unreachable = |error: std::io::Error| {
            format!(
                "could not reach the {} window on port {}: {error}. Is live MCP still on?",
                sparksmith_core::product::NAME,
                self.port
            )
        };
        self.next_id = self.next_id.wrapping_add(1);
        let mut line = json!({"id": self.next_id, "method": method, "params": params});
        let (mut reader, mut writer) = match self.connection.take() {
            Some(connection) => connection,
            None => {
                let stream = TcpStream::connect(("127.0.0.1", self.port)).map_err(unreachable)?;
                let reader = BufReader::new(stream.try_clone().map_err(unreachable)?);
                line["token"] = Value::String(self.token.token.clone());
                (reader, stream)
            }
        };
        let mut bytes = serde_json::to_vec(&line).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        writer.write_all(&bytes).map_err(unreachable)?;
        let lost = |why: &str| {
            format!(
                "no reply from the {} window ({why}); inspect the project before sending a change again",
                sparksmith_core::product::NAME
            )
        };
        let reply = match crate::lines::next(&mut reader, MAX_REPLY) {
            Ok(Some(crate::lines::Line::Text(text))) => text,
            Ok(Some(_)) => return Err(lost("the reply was not readable")),
            Ok(None) => return Err(lost("the connection closed")),
            Err(error) => return Err(lost(&error.to_string())),
        };
        let mut reply: Value = serde_json::from_str(&reply).map_err(|e| lost(&e.to_string()))?;
        if let Some(text) = reply.get("error").and_then(Value::as_str)
            && text.starts_with("unauthorized")
        {
            return Err(self.token.explain_error(text));
        }
        if let Some(object) = reply.as_object_mut() {
            object.remove("id");
        }
        self.connection = Some((reader, writer));
        Ok(reply)
    }
}

#[cfg(test)]
mod tests;
