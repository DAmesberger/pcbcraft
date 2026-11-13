//! Generation off the caller's thread, with a wall-time bound and
//! cancellation (slice GP1).
//!
//! Each call runs on a fresh worker thread under the host's fuel and memory
//! limits. The caller waits at most [`Worker::wall`] and checks its cancel
//! flag while it waits; a call that runs out of time or is cancelled is
//! refused at once and its result, if one ever comes, is dropped. The
//! worker thread itself stops when the module returns or its fuel runs out
//! (about 0.5 s natively at the default fuel), so an abandoned call costs
//! bounded work and no state: nothing it computes reaches the document.
use crate::{Limits, WasmiRuntime};
use sparksmith_generator_abi::v1::{Description, Request};
use sparksmith_generator_abi::{Failure, Generated, GeneratorRuntime};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The wall-time target of one call (`generators/README.md`): fuel bounds
/// the work, this bounds how long a caller waits on a slow host.
pub const WALL: Duration = Duration::from_secs(2);

/// How often a waiting caller looks at its cancel flag.
const POLL: Duration = Duration::from_millis(10);

/// A [`GeneratorRuntime`] that runs every call on its own thread.
#[derive(Clone, Debug)]
pub struct Worker {
    limits: Limits,
    wall: Duration,
    cancel: Option<Arc<AtomicBool>>,
}

impl Default for Worker {
    fn default() -> Self {
        Self::new(Limits::DEFAULT, WALL)
    }
}

impl Worker {
    pub fn new(limits: Limits, wall: Duration) -> Self {
        Self {
            limits,
            wall,
            cancel: None,
        }
    }

    /// This worker with a cancel flag: setting it refuses the call in
    /// progress (`generator.cancelled`) and every later one until cleared.
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn wall(&self) -> Duration {
        self.wall
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Run `job` on a worker thread and wait for it within the wall time,
    /// watching the cancel flag.
    fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&WasmiRuntime) -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, Failure> {
        if self.cancelled() {
            return Err(cancelled());
        }
        let (send, receive) = mpsc::sync_channel(1);
        let limits = self.limits;
        std::thread::Builder::new()
            .name("sparksmith-generator".into())
            .spawn(move || {
                let runtime = WasmiRuntime::new(limits);
                // The receiver may have given up; then the result is dropped.
                let _ = send.send(job(&runtime));
            })
            .map_err(|e| {
                Failure::one(
                    "generator.worker",
                    format!("The generator could not be started ({e}). It was not run."),
                )
            })?;
        let deadline = Instant::now() + self.wall;
        loop {
            if self.cancelled() {
                return Err(cancelled());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(timed_out(self.wall));
            }
            match receive.recv_timeout(POLL.min(deadline - now)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Failure::one(
                        "generator.worker",
                        "The generator stopped without an answer. Nothing was changed.".into(),
                    ));
                }
            }
        }
    }
}

fn cancelled() -> Failure {
    Failure::one(
        "generator.cancelled",
        "The generator was cancelled. Nothing was changed.".into(),
    )
}

fn timed_out(wall: Duration) -> Failure {
    Failure::one(
        "generator.timed_out",
        format!(
            "The generator did not finish within {} ms. Nothing was changed.",
            wall.as_millis()
        ),
    )
}

impl GeneratorRuntime for Worker {
    fn describe(&self, module: &[u8]) -> Result<Description, Failure> {
        let module = module.to_vec();
        self.run(move |runtime| runtime.describe(&module))
    }

    fn generate(&self, module: &[u8], request: &Request) -> Result<Generated, Failure> {
        let module = module.to_vec();
        let request = request.clone();
        self.run(move |runtime| GeneratorRuntime::generate(runtime, &module, &request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_cancel_flag_refuses_before_anything_runs() {
        let flag = Arc::new(AtomicBool::new(true));
        let worker = Worker::default().with_cancel(flag);
        let failure = worker.describe(b"not a module").err();
        assert_eq!(
            failure.as_ref().and_then(Failure::code),
            Some("generator.cancelled")
        );
    }

    #[test]
    fn refusals_of_the_runtime_come_back_from_the_worker() {
        let failure = Worker::default().describe(b"not a module").err();
        assert_eq!(
            failure.as_ref().and_then(Failure::code),
            Some("generator.invalid_module")
        );
    }

    #[test]
    fn a_call_past_its_wall_time_is_refused() {
        let worker = Worker::new(Limits::DEFAULT, Duration::ZERO);
        let failure = worker.describe(b"not a module").err();
        assert_eq!(
            failure.as_ref().and_then(Failure::code),
            Some("generator.timed_out")
        );
    }
}
