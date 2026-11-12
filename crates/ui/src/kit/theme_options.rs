//! Standard behaviours are registered here; file declarations alone do not offer a control.
use crate::kit::theme_files::OptionValue;
use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// Message-catalog id, translated at the presentation boundary.
    Unavailable(&'static str),
    /// A standard option this app provides no behaviour for; the host says why.
    Unsupported,
}

/// A standard option's behaviour. Its name and help are the host's text for its `id`.
pub struct Provider {
    pub id: &'static str,
    pub availability: fn() -> Availability,
    pub apply: fn(&mut Tokens, &OptionValue),
}

// Consumed by tab_settings or the blur-intent boundary; never store runtime state in tokens.
