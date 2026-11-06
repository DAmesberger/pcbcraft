//! Settings, Keyboard shortcuts and Messages are system windows over the
//! canvas, shown one at a time as Solidcraft shows its system windows:
//! opening one closes the one before, and Esc closes the one shown.

/// Which system windows stay open this frame. `was` is last frame's state
/// and `now` this frame's after every opener ran: a window opened since
/// closes the others, and `escape` closes them all.
pub(crate) fn one_window<const N: usize>(
    was: [bool; N],
    now: [bool; N],
    escape: bool,
) -> [bool; N] {
    if escape {
        return [false; N];
    }
    match (0..N).find(|&i| now[i] && !was[i]) {
        Some(fresh) => std::array::from_fn(|i| i == fresh),
        None => now,
    }
}

#[cfg(test)]
mod tests {
    use super::one_window;

    #[test]
    fn opening_a_window_closes_the_other_and_esc_closes_it() {
        // Settings open, then Keyboard shortcuts opens: Settings closes.
        assert_eq!(
            one_window([false, false, true], [false, true, true], false),
            [false, true, false]
        );
        // Then Messages: only Messages stays.
        assert_eq!(
            one_window([false, true, false], [true, true, false], false),
            [true, false, false]
        );
        // Nothing new: the state stands, a window closed by its × too.
        assert_eq!(
            one_window([true, false, false], [true, false, false], false),
            [true, false, false]
        );
        assert_eq!(
            one_window([true, false, false], [false, false, false], false),
            [false, false, false]
        );
        // Esc closes the window shown.
        assert_eq!(
            one_window([false, true, false], [false, true, false], true),
            [false, false, false]
        );
    }
}
