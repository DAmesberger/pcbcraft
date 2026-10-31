//! The Issues surface's app side: it only presents what `project.issues`
//! returns, adding Fix launchers and locate (docs/sidebar-ia.md).
//!
//! Contract from slice S0; slice A1 owns it. The app never computes,
//! merges or rewords findings itself: severity, groups and titles come from
//! `pcbcraft_core::issues`.
#![allow(dead_code, reason = "S0 contract; slice A1 and the W slices use it")]

use pcbcraft_core::issues::IssueRow;
use pcbcraft_core::{Engine, ObjectRef, Operation};

/// Selecting and framing the objects an issue refers to. Each editor
/// implements it for the object kinds its workspace shows, switching tabs
/// as needed; the app routes across workspaces. Locate changes selection
/// and camera only, never the document or its geometry.
pub trait Locate {
    /// Select and frame every object of `refs` this editor shows. Returns
    /// whether it showed at least one.
    fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool;
}

/// A dialog an issue's Fix opens. Each issues its own typed operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FixDialog {
    AssignPads,
    ChoosePart,
    ResolveParts,
    UpdatePcb,
    UpdateInstances,
    Mapping,
}

/// What an issue's Fix button does. Only issues that a typed command, or a
/// dialog that issues one, resolves get a Fix.
#[derive(Clone, Debug)]
pub enum Fix {
    /// Apply this operation through the one command engine.
    Apply {
        label: &'static str,
        operation: Box<Operation>,
    },
    /// Open a dialog whose primary issues the operation.
    Open {
        label: &'static str,
        dialog: FixDialog,
    },
}

/// The Fix for `row`, if it has one. Slice A1 maps rule codes to fixes.
pub fn fix(row: &IssueRow) -> Option<Fix> {
    let _ = row;
    None
}
