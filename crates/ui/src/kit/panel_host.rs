//! Bounded universal workspace chrome. Placement and transactions are owned by
//! panel_runtime; every outer surface here uses the common Container policy.

pub struct HostChrome<'a> {
    pub label: &'a str,
    pub icon: Option<&'a str>,
    pub icon_only: bool,
    pub attention: bool,
    pub selected: bool,
    pub enabled: bool,
    pub drag: bool,
    pub title: bool,
    /// A tab group's header target paints only its section grip, retaining its label for accessibility.
    pub grip_only: bool,
    pub rtl: bool,
}
