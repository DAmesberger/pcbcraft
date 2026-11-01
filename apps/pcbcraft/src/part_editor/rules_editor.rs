//! Slice C5 (contract C0): the Rules sub-view: one Rhai pin-rule script per
//! row with a live syntax check and an explain/test panel that runs
//! `functions::resolve` on a scratch selection (founder decision F2).
/// The editor's buffers and the explain panel's scratch selection.
#[derive(Default)]
pub struct RulesEditor {}
