//! Status heading and constraint meter; Tokens: font_label, text, text_dim, accent, dialog_rule, sketch_accent, spacing.

/// Status content for a named card. `key` is the scenario namespace, `fraction` is clamped.
pub struct StatusHeader<'a> {
    pub icon: &'a str,
    pub text: &'a str,
    pub complete: bool,
    pub fraction: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warning,
    Error,
}

impl CheckStatus {}
