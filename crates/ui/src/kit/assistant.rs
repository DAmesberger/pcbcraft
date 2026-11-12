//! Assistant presentation primitives; protocol state and action dispatch stay with callers.

#[derive(Clone, Copy)]
pub enum AssistantTone {
    Neutral,
    Working,
    Success,
    Warning,
    Error,
}

impl AssistantTone {}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
pub enum AssistantToolState {
    Pending,
    Running,
    Done,
    Failed,
}
