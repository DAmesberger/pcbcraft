//! Shared chord keycaps and the Preferences title-bar search field.

/// A stored chord as this platform names it: on macOS Ctrl is the primary modifier, shown as
/// Cmd, and Alt is Option; elsewhere the chord is unchanged. A recorded macOS Control+Command
/// chord (`Ctrl+Cmd+…`) names both keys explicitly, so its Ctrl stays.
pub fn chord_label(key: &str) -> String {
    chord_label_for_platform(key, cfg!(target_os = "macos"))
}

pub fn chord_label_for_platform(key: &str, mac: bool) -> String {
    let both = key.starts_with("Ctrl+Cmd+");
    key.split('+')
        .map(|part| match (mac, part) {
            (true, "Ctrl") if !both => "Cmd",
            (true, "Alt") => "Option",
            _ => part,
        })
        .collect::<Vec<_>>()
        .join("+")
}
