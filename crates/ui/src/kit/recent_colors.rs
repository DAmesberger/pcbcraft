//! The colours picked last in any colour field, newest first (#9234). The host keeps the list in
//! its saved settings and hands it to the colour pickers every frame through egui memory with
//! [`share`]; the kit's colour popover shows it and adds finished picks.

/// The colours picked last, newest first, shared by every colour picker of the window.
#[derive(Clone, Default)]
struct RecentColors {
    colors: Vec<[u8; 3]>,
    /// Picked since the app last took the list for its settings.
    changed: bool,
}
