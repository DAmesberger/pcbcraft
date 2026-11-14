//! The board interface's host requests for the desktop, off the UI thread
//! (docs/architecture/board-interface.md, "Commands"; slice B6).
//!
//! The publish preview, the publish, the linked design preview and reading an
//! linked design update run the automation session's own code, the code the
//! CLI and MCP run, on a session over a fork of the window's engine: the
//! window keeps drawing and navigating while Solidcraft builds or reads a
//! design. What changes the document comes back as the typed command the
//! request would have applied (`interface.record_publish`,
//! `assembly.record_link` or `assembly.record_update`), and the window
//! applies it on its own engine as one undo step.
use sparksmith_automation::{Request, Session};
use sparksmith_core::interface::PublishPreview;
use sparksmith_core::interface::assembly_link::{AssemblyPreview, PreviewParams, UpdateParams};
use sparksmith_core::interface::{PublishParams, PublishPreviewParams};
use sparksmith_core::{Engine, Operation};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

/// A host request for the worker.
#[derive(Clone, Debug)]
pub(super) enum Job {
    PublishPreview(PublishPreviewParams),
    Publish(PublishParams),
    AssemblyPreview(PreviewParams),
    AssemblyUpdate(UpdateParams),
}

/// What a job gave back, in the words the window shows on failure.
#[derive(Debug)]
pub(super) enum Done {
    PublishPreview(Result<PublishPreview, String>),
    /// The record to apply, and the revision the design was published at.
    Published(Result<(Operation, u32), String>),
    AssemblyPreview(Result<AssemblyPreview, String>),
    /// `assembly.record_link` or `assembly.record_update` to apply.
    AssemblyRead(Result<Operation, String>),
}

/// One job in flight.
pub(super) struct Running {
    receiver: Receiver<Done>,
}

impl Running {
    /// A job that has not answered yet, and the sender that would answer
    /// it (tests).
    #[cfg(test)]
    pub(super) fn pending() -> (std::sync::mpsc::Sender<Done>, Self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (sender, Self { receiver })
    }

    /// The job's result, once it is there.
    pub(super) fn poll(&mut self) -> Option<Done> {
        match self.receiver.try_recv() {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Done::PublishPreview(Err(
                "The request stopped unexpectedly".into(),
            ))),
        }
    }
}

/// An automation error in the window's words: the core's refusal wording,
/// or the message without its code.
pub(super) fn words(error: &sparksmith_automation::Error) -> String {
    match error {
        sparksmith_automation::Error::Document(e) => crate::refusal::shown_error(e),
        other => {
            let text = other.to_string();
            let text = match text.split_once(": ") {
                Some((code, rest)) if code.contains('.') && !code.contains(' ') => rest.to_owned(),
                _ => text,
            };
            crate::refusal::clip(&text, crate::refusal::MAX_SHOWN)
        }
    }
}

/// Run `job` on a session over a fork of `engine`, whose file is
/// `project` (relative linked design paths are read against its folder).
/// `None` when no thread could be started.
pub(super) fn start(
    ctx: &egui::Context,
    engine: &Engine,
    project: Option<PathBuf>,
    job: Job,
) -> Option<Running> {
    let mut fork = engine.fork();
    let (sender, receiver) = std::sync::mpsc::channel();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("sparksmith-interface".into())
        .spawn(sparksmith_core::library::standards::carried(move || {
            let mut session = Session::lending();
            let done = session.lend(&mut fork, project.as_deref(), |s| run(s, job));
            let _ = sender.send(done);
            repaint.request_repaint();
        }))
        .ok()?;
    Some(Running { receiver })
}

/// The job, on `session`.
pub(super) fn run(session: &mut Session, job: Job) -> Done {
    match job {
        Job::PublishPreview(params) => Done::PublishPreview(
            session
                .execute(Request::InterfacePreview(params))
                .map_err(|e| words(&e))
                .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string())),
        ),
        Job::Publish(params) => Done::Published(
            session
                .interface_write(&params)
                .map(|(operation, published)| {
                    let revision = published["revision"]
                        .as_u64()
                        .and_then(|r| u32::try_from(r).ok())
                        .unwrap_or_default();
                    (operation, revision)
                })
                .map_err(|e| words(&e)),
        ),
        Job::AssemblyPreview(params) => Done::AssemblyPreview(
            session
                .execute(Request::AssemblyPreview(params))
                .map_err(|e| words(&e))
                .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string())),
        ),
        Job::AssemblyUpdate(params) => Done::AssemblyRead(
            session
                .assembly_update_command(&params)
                .map(Operation::Assembly)
                .map_err(|e| words(&e)),
        ),
    }
}
