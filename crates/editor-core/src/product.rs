//! The product's name, in one place. Every name a person sees (window and
//! menu titles, About, messages, the CLI's usage, the MCP server, generated
//! file headers, the per-user folders) comes from here, so a rename is the
//! one line in [`product_name!`] and [`product_short_name!`].
//!
//! The macros exist for text built at compile time (`concat!`, `const`
//! tables); everything else uses the constants. `apps/sparksmith/macos/Info.plist`
//! cannot read Rust, so a test checks its names against these.

/// The product name as people read it: "Sparksmith".
#[macro_export]
macro_rules! product_name {
    () => {
        "Sparksmith"
    };
}

/// The lower-case short name: the CLI binary, the MCP server name and the
/// per-user config, cache and state folders.
#[macro_export]
macro_rules! product_short_name {
    () => {
        "sparksmith"
    };
}

/// The product name: "Sparksmith".
pub const NAME: &str = product_name!();
/// The lower-case short name (CLI binary, MCP server, folders).
pub const SHORT_NAME: &str = product_short_name!();
/// The reverse-DNS prefix of bundle and type identifiers.
pub const BUNDLE_ID_PREFIX: &str = "io.sparksmith";
/// The macOS application bundle identifier.
pub const BUNDLE_ID: &str = "io.sparksmith.app";
/// The product's website.
pub const WEBSITE: &str = "https://sparksmith.io";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_agree() {
        assert_eq!(NAME, "Sparksmith");
        assert_eq!(SHORT_NAME, NAME.to_ascii_lowercase());
        assert!(BUNDLE_ID.starts_with(BUNDLE_ID_PREFIX));
        assert!(WEBSITE.starts_with("https://"));
    }
}
