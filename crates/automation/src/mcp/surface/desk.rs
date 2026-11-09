//! The session thread. A [`Session`] is synchronous and does long work
//! (fills, checks, exports), so it lives on a thread of its own, the
//! [`Desk`]; the async handlers send it work over a bounded channel through
//! a [`Handle`] and await the answer, and the runtime stays free to read the
//! next request, or a cancellation.
//!
//! Work runs one job at a time, in the order it reached the channel. A job
//! sees only the desk, whose one way to run a request is [`Desk::respond`]:
//! [`Session::respond`] as [`Caller::Model`] under the server's [`Policy`],
//! so every request passes the gate unchanged. After each job the desk
//! publishes the revision, for whatever watches the project.
//!
//! P2 turns the one session into the projects behind explicit handles.
use crate::gate::{Caller, Decision, Policy};
use crate::session::Session;
use rmcp::ErrorData;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

/// Jobs that may wait for the desk before a sender waits too.
pub const QUEUE: usize = 64;

type Job = Box<dyn FnOnce(&mut Desk) + Send>;

/// What a job may do with the session.
pub struct Desk {
    session: Session,
    policy: Policy,
}

impl Desk {
    /// Starts the thread that owns `session`. It ends when the last
    /// [`Handle`] is dropped.
    pub fn start(session: Session, policy: Policy) -> Result<Handle, String> {
        let (jobs, mut inbox) = mpsc::channel::<Job>(QUEUE);
        let (published, revision) = watch::channel(session.engine().revision());
        let mut desk = Self { session, policy };
        std::thread::Builder::new()
            .name("pcbcraft-mcp-session".into())
            .spawn(move || {
                while let Some(job) = inbox.blocking_recv() {
                    job(&mut desk);
                    let now = desk.revision();
                    published.send_if_modified(|seen| std::mem::replace(seen, now) != now);
                }
            })
            .map_err(|e| format!("cannot start the session thread: {e}"))?;
        Ok(Handle { jobs, revision })
    }

    /// Runs one JSON request (`{"command", "params"}`) through the gate as
    /// the model under this server's policy, and answers in the session's
    /// envelope. What needs a person is refused here; P5 asks them.
    pub fn respond(&mut self, request: Value) -> Value {
        self.session.respond(Caller::Model(&self.policy), request)
    }

    /// What the gate would decide for `request`, without running it.
    pub fn decide(&self, request: &Value) -> Decision {
        self.session.check_request(&self.policy, request)
    }

    /// The document's revision.
    pub fn revision(&self) -> u64 {
        self.session.engine().revision()
    }

    /// The session, to read.
    pub fn session(&self) -> &Session {
        &self.session
    }
}

/// The handlers' way to the desk; cheap to clone.
#[derive(Clone)]
pub struct Handle {
    jobs: mpsc::Sender<Job>,
    revision: watch::Receiver<u64>,
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

    /// The revision after the last job, and every later change.
    pub fn revision(&self) -> watch::Receiver<u64> {
        self.revision.clone()
    }
}

fn stopped() -> ErrorData {
    ErrorData::internal_error("the session thread has stopped", None)
}
