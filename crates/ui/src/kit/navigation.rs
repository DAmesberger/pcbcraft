//! Navigation chrome and measurements shared by the viewport and Preferences editor.
use serde::{Deserialize, Serialize};

pub const NAV_BORDER: f32 = 1.0;
pub const NAV_POPUP_PADDING: i8 = 5;
pub const NAV_ROW_GAP: f32 = 9.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationKind {
    Action,
    Tool,
    Toggle,
    Menu,
    Separator,
}

/// The edge of the 3D view the bar sits on (#8183): the bottom, or the top, under the ribbon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationEdge {
    #[default]
    Bottom,
    Top,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationSize {
    Compact,
    #[default]
    Regular,
}

impl NavigationSize {
    // Logical points: egui's UI zoom applies the preference's scale to these dimensions.
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationIdle {
    Always,
    #[default]
    Fade,
    Hide,
}

#[derive(Clone, Copy, Debug)]
pub struct NavigationMetrics {
    pub button: f32,
    pub icon: f32,
    pub radius: u8,
    pub button_radius: u8,
}
impl NavigationMetrics {}

pub struct NavigationButton<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub icon: &'a str,
    pub selected: bool,
    pub kind: NavigationKind,
    pub labels: bool,
    pub menu: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationFill {
    None,
    Hover,
    Press,
    Active,
}
