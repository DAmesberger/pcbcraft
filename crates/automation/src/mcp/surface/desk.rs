//! The session thread. A [`Session`] is synchronous and does long work
//! (fills, checks, exports), so the projects live on a thread of their own,
//! the [`Desk`]; the async handlers send it work over a bounded channel
//! through a [`Handle`] and await the answer, and the runtime stays free to
//! read the next request, or a cancellation.
//!
//! Work runs one job at a time, in the order it reached the channel. A job
//! sees only the desk, whose ways to run a request are [`Desk::respond`],
//! [`Desk::rehearse`] and [`Desk::open`]: [`Session::respond`] (or its dry
//! run, [`Session::rehearse`]) as [`Caller::Model`] under the server's
//! [`Policy`], so every request passes the gate unchanged. After each job
//! the desk publishes every project's revision, for whatever watches them.
//!
//! **Projects have handles** (`docs/mcp-surface.md` §3.2): opaque
//! `prj_<UUID v4>` strings, minted for the session the server starts with
//! and for each `project.new` or `project.open`, which open another
//! project rather than replacing one, so clients that share this process
//! across conversations keep their work apart. A handle lasts until the
//! process exits. A call may leave the handle out only while one project is
//! open; with several, it must name one.
use crate::gate::{Caller, Decision, Policy};
use crate::session::Session;
use pcbcraft_core::Diagnostic;
use rmcp::ErrorData;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::{mpsc, oneshot, watch};

/// Jobs that may wait for the desk before a sender waits too.
pub const QUEUE: usize = 64;
/// Projects one process holds open at once.
pub const MAX_PROJECTS: usize = 16;
/// What a handle starts with.
pub const HANDLE_PREFIX: &str = "prj_";
/// The longest handle a request may name, in bytes: the prefix and a UUID.
const MAX_HANDLE: usize = 64;

/// Every open project's revision, by handle.
pub type Revisions = BTreeMap<String, u64>;

type Job = Box<dyn FnOnce(&mut Desk) + Send>;
/// Makes the session a new project starts from (with the host's library
/// sources, say).
pub type NewSession = Box<dyn Fn() -> Session + Send>;

/// What a job may do with the projects.
pub struct Desk {
    projects: BTreeMap<String, Session>,
    policy: Policy,
    new_session: NewSession,
}

impl Desk {
    /// Starts the thread that owns `session`, the first project, and the
    /// ones opened later from `new_session`. It ends when the last
    /// [`Handle`] is dropped.
    pub fn start(
        session: Session,
        policy: Policy,
        new_session: NewSession,
    ) -> Result<Handle, String> {
        let first = mint()?;
        let mut desk = Self {
            projects: BTreeMap::from([(first, session)]),
            policy,
            new_session,
        };
        let (jobs, mut inbox) = mpsc::channel::<Job>(QUEUE);
        let (published, revisions) = watch::channel(desk.revisions());
        std::thread::Builder::new()
            .name("pcbcraft-mcp-session".into())
            .spawn(move || {
                while let Some(job) = inbox.blocking_recv() {
                    job(&mut desk);
                    let now = desk.revisions();
                    published.send_if_modified(|seen| {
                        let changed = *seen != now;
                        *seen = now;
                        changed
                    });
                }
            })
            .map_err(|e| format!("cannot start the session thread: {e}"))?;
        Ok(Handle { jobs, revisions })
    }

    /// The handle of the project `project` names, or of the only one open
    /// when it names none. The failure is the session envelope of a
    /// diagnostic, as [`super::outcome`] reads it.
    pub fn project(&self, project: Option<&str>) -> Result<String, Value> {
        match project {
            Some(handle) if self.projects.contains_key(handle) => Ok(handle.to_owned()),
            Some(handle) => Err(failure(
                "project.unknown",
                format!(
                    "no open project has the handle {}: a handle lasts until the server process exits; act project.open {{path}} opens the file again",
                    shown(handle)
                ),
            )),
            None => match self.projects.keys().next() {
                Some(only) if self.projects.len() == 1 => Ok(only.clone()),
                Some(_) => Err(failure(
                    "project.ambiguous",
                    "several projects are open: pass project, the handle from your last reply"
                        .into(),
                )),
                None => Err(failure(
                    "project.none",
                    "no project is open: act project.new or project.open {path} opens one".into(),
                )),
            },
        }
    }

    /// Runs one JSON request (`{"command", "params"}`) on `project` through
    /// the gate as the model under this server's policy, and answers in the
    /// session's envelope, with the project's handle in the result. What
    /// needs a person is refused here; P5 asks them.
    pub fn respond(&mut self, project: Option<&str>, request: Value) -> Value {
        self.run_on(project, |session, policy| {
            session.respond(Caller::Model(policy), request)
        })
    }

    /// Runs a request about the catalog, which no project changes
    /// (`project.schema`), through the gate as [`Self::respond`] does, on
    /// any open project.
    pub fn respond_catalog(&mut self, request: Value) -> Value {
        match self.projects.values_mut().next() {
            Some(session) => session.respond(Caller::Model(&self.policy), request),
            None => failure(
                "project.none",
                "no project is open: act project.new or project.open {path} opens one".into(),
            ),
        }
    }

    /// [`Self::respond`] as a dry run ([`Session::rehearse`]): the answer
    /// the edit would give, with nothing kept.
    pub fn rehearse(&mut self, project: Option<&str>, request: Value) -> Value {
        self.run_on(project, |session, policy| {
            session.rehearse(Caller::Model(policy), request)
        })
    }

    /// Runs `project.new` or `project.open` (`request`) on a new session,
    /// through the gate as [`Self::respond`] does; when it succeeds, the
    /// session becomes a project with a handle of its own, which the result
    /// carries. Nothing open is replaced, and a failure opens nothing.
    pub fn open(&mut self, mut request: Value) -> Value {
        if self.projects.len() >= MAX_PROJECTS {
            return failure(
                "project.limit",
                format!("{MAX_PROJECTS} projects are open, which is as many as this server holds"),
            );
        }
        let mut session = (self.new_session)();
        // A new project replaces nothing, so it expects the new session's
        // own revision.
        if let Some(params) = request.get_mut("params").and_then(Value::as_object_mut) {
            params.insert(
                "expected_revision".into(),
                json!(session.engine().revision()),
            );
        }
        let mut reply = session.respond(Caller::Model(&self.policy), request);
        if reply["ok"] != true {
            return reply;
        }
        let handle = match mint() {
            Ok(handle) => handle,
            Err(why) => return failure("project.handle", why),
        };
        stamp(&mut reply, &handle);
        self.projects.insert(handle, session);
        reply
    }

    /// What the gate would decide for `request` on `project`, without
    /// running it.
    pub fn decide(&self, project: Option<&str>, request: &Value) -> Result<Decision, Value> {
        let handle = self.project(project)?;
        match self.projects.get(&handle) {
            Some(session) => Ok(session.check_request(&self.policy, request)),
            None => Err(failure("project.unknown", "the project closed".into())),
        }
    }

    /// The revision of `project`, when it is open.
    pub fn revision(&self, project: &str) -> Option<u64> {
        self.projects.get(project).map(|s| s.engine().revision())
    }

    /// Every open project's revision.
    pub fn revisions(&self) -> Revisions {
        self.projects
            .iter()
            .map(|(handle, session)| (handle.clone(), session.engine().revision()))
            .collect()
    }

    /// The session behind `project`, to read.
    pub fn session(&self, project: &str) -> Option<&Session> {
        self.projects.get(project)
    }

    fn run_on(
        &mut self,
        project: Option<&str>,
        run: impl FnOnce(&mut Session, &Policy) -> Value,
    ) -> Value {
        let handle = match self.project(project) {
            Ok(handle) => handle,
            Err(failure) => return failure,
        };
        let Some(session) = self.projects.get_mut(&handle) else {
            return failure("project.unknown", "the project closed".into());
        };
        let mut reply = run(session, &self.policy);
        stamp(&mut reply, &handle);
        reply
    }
}

/// The handlers' way to the desk; cheap to clone.
#[derive(Clone)]
pub struct Handle {
    jobs: mpsc::Sender<Job>,
    revisions: watch::Receiver<Revisions>,
}

impl Handle {
    /// Runs `work` on the desk and returns its answer. Fails, as an internal
    /// error, only when the session thread has stopped.
    pub async fn run<R, F>(&self, work: F) -> Result<R, ErrorData>
    where
        R: Send + 'static,
        F: FnOnce(&mut Desk) -> R + Send + 'static,
    {
        let (reply, answer) = oneshot::channel();
        let job: Job = Box::new(move |desk| {
            // A caller that went away no longer needs the answer.
            drop(reply.send(work(desk)));
        });
        self.jobs.send(job).await.map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())
    }

    /// Every project's revision after the last job, and every later change.
    pub fn revisions(&self) -> watch::Receiver<Revisions> {
        self.revisions.clone()
    }
}

/// A new handle: the prefix and a random UUID v4.
fn mint() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot make a project handle: {e}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let part = |range: std::ops::Range<usize>| hex.get(range).unwrap_or_default();
    Ok(format!(
        "{HANDLE_PREFIX}{}-{}-{}-{}-{}",
        part(0..8),
        part(8..12),
        part(12..16),
        part(16..20),
        part(20..32)
    ))
}

/// Puts the project's handle into a successful reply's result.
fn stamp(reply: &mut Value, handle: &str) {
    if reply["ok"] == true
        && let Some(result) = reply.get_mut("result").and_then(Value::as_object_mut)
    {
        result.insert("project".into(), json!(handle));
    }
}

/// A handle as a message repeats it: cut to its bound.
fn shown(handle: &str) -> String {
    handle.chars().take(MAX_HANDLE).collect()
}

/// The session envelope of a failure with `code`.
pub fn failure(code: &str, message: String) -> Value {
    let diagnostic = Diagnostic {
        code: code.into(),
        rule: None,
        message,
        expected_revision: None,
        actual_revision: None,
    };
    json!({"ok": false, "error": diagnostic})
}

fn stopped() -> ErrorData {
    ErrorData::internal_error("the session thread has stopped", None)
}
