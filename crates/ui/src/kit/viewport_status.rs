//! Persistent viewport status content. The caller owns items and dispatches returned actions.
use egui::Id;

/// Shape and outline communicate the kind independently of colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Mode,
    Result,
    Attention,
}

impl StatusKind {}

/// Presentation data, already translated and formatted in the design's units.
/// A later engine adapter supplies this without storing session state in the widget.
pub struct StatusItem {
    pub id: String,
    pub kind: StatusKind,
    pub name: String,
    pub setting: String,
    pub has_key: bool,
    pub editable: bool,
}

#[derive(Default)]
pub struct StatusPresentation {
    pub open_key: Option<String>,
    pub focus_field: Option<String>,
    pub selected_record: Option<String>,
    pub live_details: bool,
    return_focus: Option<Id>,
    return_pending: bool,
    pub card_owner: Option<String>,
    pub opened_frame: Option<u64>,
    pub session_uid: Option<u64>,
    focus_ids: Vec<Id>,
    pub summary_popup: Option<Id>,
    pub summary_members: Vec<String>,
    pub summary_suspended: bool,
}

impl StatusPresentation {}

#[derive(Debug, PartialEq, Eq)]
pub enum StatusAction {
    Clear(String),
    ClearAll,
    Edit(String),
}
