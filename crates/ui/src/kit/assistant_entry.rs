//! Assistant entry point and its status mark, shared by application chrome and previews.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistantEntryState {
    Quiet,
    Ready,
    Busy,
    NeedsAuth,
    Stopped,
    Waiting(usize),
}
