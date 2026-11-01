//! Slice C3 (contract C0): handlers of `library.pinout_preview` (query)
//! and `library.pinout_apply`. Apply is atomic and one undo step, refused
//! when the target draft changed since the plan's `base` (the stale-plan
//! rule of updates).
use super::Outcome;
use crate::diagnostics::Result;
use crate::library::pinout::{self, PinoutApply, PinoutDocument, PinoutPlan, PinoutTarget};
use crate::model::Document;

/// `library.pinout_preview`.
pub(crate) fn preview(
    _document: &Document,
    _target: &PinoutTarget,
    _pinout: &PinoutDocument,
) -> Result<PinoutPlan> {
    Err(pinout::unsupported())
}

/// `library.pinout_apply` (`Outcome::PinoutApplied`).
pub(crate) fn apply(_document: &mut Document, _request: PinoutApply) -> Result<Outcome> {
    Err(pinout::unsupported())
}
