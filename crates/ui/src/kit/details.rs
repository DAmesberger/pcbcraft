//! Description-driven details content. No tool, document or command knowledge lives here.

#[derive(Clone, Debug, PartialEq)]
pub enum DetailValue {
    Number {
        value: f64,
        min: f64,
        max: f64,
        suffix: String,
    },
    /// Shown to `decimals` places, as the design shows lengths.
    Length {
        value: f64,
        units: String,
        decimals: u8,
    },
    Angle(f64),
    Slider {
        value: f64,
        min: f64,
        max: f64,
    },
    Axis {
        value: [f64; 3],
        current: String,
    },
    Check(bool),
    Choice {
        selected: usize,
        options: Vec<String>,
    },
}

pub struct DetailSetting {
    pub id: String,
    pub label: String,
    pub value: DetailValue,
    pub enabled: bool,
}

pub struct DetailKey {
    pub colour: egui::Color32,
    pub pattern: bool,
    pub label: String,
    pub value: String,
    pub numeric: bool,
    pub pattern_colour: Option<egui::Color32>,
}

pub struct DetailFinding {
    pub id: String,
    pub label: String,
    pub value: String,
    pub warning: bool,
    pub target: bool,
}

/// All blocks are optional. Strings and numeric values are current caller-owned data.
#[derive(Default)]
pub struct Details {
    pub caption: Option<String>,
    pub settings: Vec<DetailSetting>,
    pub key: Vec<DetailKey>,
    pub findings_title: String,
    pub findings_count: Option<String>,
    pub findings: Vec<DetailFinding>,
    pub readout: Vec<(String, String)>,
    pub actions: Vec<(String, String)>,
}

#[derive(Debug, PartialEq)]
pub enum DetailEvent {
    Change(String, DetailValue),
    Target(String),
    Action(String),
}
