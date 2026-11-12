//! The list's keyboard and pointer model, without egui: one input and the
//! shown lines in, the new state and what the list should do out
//! ([`reduce`]). Every list (Dimensions, Issues, the parts table) reads
//! its keys and clicks through this one reducer, so they behave alike.
//!
//! The keys are the spec's keyboard map (docs: list-system spec §4); which
//! physical key means which [`Input`] is the keymap registry's
//! (`kit::list::keys`), so it stays configurable.
use super::model::{Fold, Line, RowKey};
use super::state::{ListState, TYPEAHEAD_MAX, TYPEAHEAD_SECONDS};

/// One thing a person did to a focused list.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// ↑ / ↓; with `extend` (⇧) the range grows from the anchor; with
    /// `cursor_only` (⌘) only the cursor moves.
    Up {
        extend: bool,
        cursor_only: bool,
    },
    Down {
        extend: bool,
        cursor_only: bool,
    },
    Home {
        extend: bool,
    },
    End {
        extend: bool,
    },
    PageUp {
        extend: bool,
    },
    PageDown {
        extend: bool,
    },
    /// ← folds, or goes to the parent.
    Left,
    /// → unfolds, or goes to the first child.
    Right,
    /// Toggle the cursor row; on a group header, fold.
    Space,
    /// Enter / F2 / double-click: the primary action.
    Activate,
    Escape,
    /// ⌘A: every shown, selectable row.
    SelectAll,
    Delete,
    /// ⌥↑ / ⌥↓.
    MoveUp,
    MoveDown,
    /// `/` or ⌘F.
    FocusFilter,
    /// Letters and digits typed at time `now` (seconds).
    Type {
        text: String,
        now: f64,
    },
    /// A click on the row `key`: plain selects it, `extend` (⇧) the range
    /// to it, `toggle` (⌘) adds or removes it.
    Click {
        key: RowKey,
        extend: bool,
        toggle: bool,
    },
    DoubleClick {
        key: RowKey,
    },
}

/// What the list allows and what is going on around it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Context {
    /// Rows a page key moves.
    pub page: usize,
    /// Order is document data here (Dimensions).
    pub reorder: bool,
    pub delete: bool,
    /// More than one row may be selected.
    pub multiple: bool,
    /// A value is being edited in the list.
    pub editing: bool,
    /// The filter has text.
    pub filtered: bool,
}

/// What the list should do after an input.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    None,
    /// The selection changed (it is in the state).
    Selected,
    /// The cursor moved without changing the selection.
    Moved,
    /// The primary action on this row: edit, or locate.
    Activate(RowKey),
    /// The group or tree row folded (`true`) or opened.
    Fold(RowKey, bool),
    /// Delete these rows (the selection), in display order.
    Delete(Vec<RowKey>),
    /// Move this row by one place, up (`-1`) or down (`1`).
    Reorder(RowKey, i32),
    FocusFilter,
    CancelEdit,
    ClearFilter,
}

/// Apply `input` to `state` over the shown `lines`.
pub fn reduce(state: &mut ListState, lines: &[Line<'_>], input: Input, cx: Context) -> Effect {
    let at = |key: Option<RowKey>| key.and_then(|k| lines.iter().position(|l| l.key == k));
    let cursor = at(state.cursor);
    // A cursor that left the shown lines (filtered, folded away) restarts.
    if cursor.is_none() {
        state.cursor = None;
    }
    match input {
        Input::Up {
            extend,
            cursor_only,
        } => {
            let to = match cursor {
                Some(i) => i.saturating_sub(1),
                None => lines.len().saturating_sub(1),
            };
            go(state, lines, to, extend && cx.multiple, cursor_only)
        }
        Input::Down {
            extend,
            cursor_only,
        } => {
            let to = cursor.map_or(0, |i| (i + 1).min(lines.len().saturating_sub(1)));
            go(state, lines, to, extend && cx.multiple, cursor_only)
        }
        Input::Home { extend } => go(state, lines, 0, extend && cx.multiple, false),
        Input::End { extend } => go(
            state,
            lines,
            lines.len().saturating_sub(1),
            extend && cx.multiple,
            false,
        ),
        Input::PageUp { extend } => {
            let to = cursor.unwrap_or(0).saturating_sub(cx.page.max(1));
            go(state, lines, to, extend && cx.multiple, false)
        }
        Input::PageDown { extend } => {
            let to = cursor
                .unwrap_or(0)
                .saturating_add(cx.page.max(1))
                .min(lines.len().saturating_sub(1));
            go(state, lines, to, extend && cx.multiple, false)
        }
        Input::Left => {
            let Some(line) = cursor.and_then(|i| lines.get(i)) else {
                return Effect::None;
            };
            if line.fold == Fold::Open {
                state.folded.insert(line.key);
                return Effect::Fold(line.key, true);
            }
            match at(line.parent) {
                Some(parent) => go(state, lines, parent, false, false),
                None => Effect::None,
            }
        }
        Input::Right => {
            let Some((i, line)) = cursor.and_then(|i| lines.get(i).map(|l| (i, l))) else {
                return Effect::None;
            };
            match line.fold {
                Fold::Folded => {
                    state.folded.remove(&line.key);
                    Effect::Fold(line.key, false)
                }
                Fold::Open if lines.get(i + 1).is_some_and(|n| n.parent == Some(line.key)) => {
                    go(state, lines, i + 1, false, false)
                }
                _ => Effect::None,
            }
        }
        Input::Space => {
            let Some(line) = cursor.and_then(|i| lines.get(i)) else {
                return Effect::None;
            };
            if line.header {
                return toggle_fold(state, line);
            }
            if !state.selection.remove(&line.key) {
                if !cx.multiple {
                    state.selection.clear();
                }
                state.selection.insert(line.key);
            }
            state.anchor = Some(line.key);
            Effect::Selected
        }
        Input::Activate => match cursor.and_then(|i| lines.get(i)) {
            Some(line) if line.header => toggle_fold(state, line),
            Some(line) => Effect::Activate(line.key),
            None => Effect::None,
        },
        Input::Escape => {
            if cx.editing {
                Effect::CancelEdit
            } else if cx.filtered {
                Effect::ClearFilter
            } else if !state.selection.is_empty() {
                state.selection.clear();
                state.anchor = None;
                Effect::Selected
            } else {
                Effect::None
            }
        }
        Input::SelectAll => {
            if !cx.multiple {
                return Effect::None;
            }
            state.selection = lines.iter().filter(|l| !l.header).map(|l| l.key).collect();
            Effect::Selected
        }
        Input::Delete => {
            if !cx.delete || state.selection.is_empty() {
                return Effect::None;
            }
            Effect::Delete(
                lines
                    .iter()
                    .filter(|l| state.selection.contains(&l.key))
                    .map(|l| l.key)
                    .collect(),
            )
        }
        Input::MoveUp | Input::MoveDown => {
            let Some(line) = cursor.and_then(|i| lines.get(i)) else {
                return Effect::None;
            };
            if !cx.reorder || line.header {
                return Effect::None;
            }
            Effect::Reorder(line.key, if input == Input::MoveUp { -1 } else { 1 })
        }
        Input::FocusFilter => Effect::FocusFilter,
        Input::Type { text, now } => type_ahead(state, lines, &text, now),
        Input::Click {
            key,
            extend,
            toggle,
        } => {
            let Some(i) = at(Some(key)) else {
                return Effect::None;
            };
            state.keyboard = false;
            if lines.get(i).is_some_and(|l| l.header) {
                state.cursor = Some(key);
                return lines.get(i).map_or(Effect::None, |l| toggle_fold(state, l));
            }
            if toggle && cx.multiple {
                if !state.selection.remove(&key) {
                    state.selection.insert(key);
                }
                state.cursor = Some(key);
                state.anchor = Some(key);
                Effect::Selected
            } else if extend && cx.multiple {
                state.cursor = None;
                go_pointer(state, lines, i)
            } else {
                state.select_one(key);
                Effect::Selected
            }
        }
        Input::DoubleClick { key } => {
            if at(Some(key)).is_none() {
                return Effect::None;
            }
            state.keyboard = false;
            state.select_one(key);
            Effect::Activate(key)
        }
    }
}

fn toggle_fold(state: &mut ListState, line: &Line<'_>) -> Effect {
    let fold = line.fold != Fold::Folded;
    if fold {
        state.folded.insert(line.key);
    } else {
        state.folded.remove(&line.key);
    }
    Effect::Fold(line.key, fold)
}

/// A ⇧-click: the range from the anchor to row `to`.
fn go_pointer(state: &mut ListState, lines: &[Line<'_>], to: usize) -> Effect {
    let effect = go(state, lines, to, true, false);
    state.keyboard = false;
    effect
}

/// Move the cursor to line `to`: select it, extend the range from the
/// anchor to it, or only move.
fn go(
    state: &mut ListState,
    lines: &[Line<'_>],
    to: usize,
    extend: bool,
    cursor_only: bool,
) -> Effect {
    let Some(line) = lines.get(to) else {
        return Effect::None;
    };
    state.keyboard = true;
    state.cursor = Some(line.key);
    if cursor_only || line.header {
        return Effect::Moved;
    }
    if extend {
        let anchor = state
            .anchor
            .and_then(|a| lines.iter().position(|l| l.key == a))
            .unwrap_or(to);
        let (a, b) = (anchor.min(to), anchor.max(to));
        state.selection = lines
            .get(a..=b)
            .unwrap_or_default()
            .iter()
            .filter(|l| !l.header)
            .map(|l| l.key)
            .collect();
        state.anchor = lines.get(anchor).map(|l| l.key);
    } else {
        state.selection.clear();
        state.selection.insert(line.key);
        state.anchor = Some(line.key);
    }
    Effect::Selected
}

/// Letters jump to the first row, from the cursor on, whose name starts
/// with what was typed within [`TYPEAHEAD_SECONDS`]. A repeated single
/// letter steps through the rows that start with it.
fn type_ahead(state: &mut ListState, lines: &[Line<'_>], text: &str, now: f64) -> Effect {
    if text.chars().all(char::is_whitespace) {
        return Effect::None;
    }
    if now - state.typed_at > TYPEAHEAD_SECONDS {
        state.typed.clear();
    }
    if state.typed.len() + text.len() <= TYPEAHEAD_MAX {
        state.typed.push_str(text);
    }
    state.typed_at = now;
    let typed = state.typed.to_lowercase();
    let first = typed.chars().next();
    let repeated = typed.chars().count() > 1 && typed.chars().all(|c| Some(c) == first);
    let (needle, skip_current) = if repeated {
        (first.map(String::from).unwrap_or_default(), true)
    } else {
        (typed.clone(), typed.chars().count() == 1)
    };
    let start = state
        .cursor
        .and_then(|c| lines.iter().position(|l| l.key == c))
        .map_or(0, |i| if skip_current { i + 1 } else { i });
    let n = lines.len();
    let found = (0..n).map(|k| (start + k) % n.max(1)).find(|&i| {
        lines
            .get(i)
            .is_some_and(|l| !l.header && l.name.to_lowercase().starts_with(&needle))
    });
    match found {
        Some(i) => go(state, lines, i, false, false),
        None => Effect::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 6] = ["C1", "C2", "R1", "R2", "R10", "U1"];

    fn lines() -> Vec<Line<'static>> {
        NAMES
            .iter()
            .enumerate()
            .map(|(i, n)| Line::item(RowKey(i as u64), n))
            .collect()
    }

    fn cx() -> Context {
        Context {
            page: 3,
            multiple: true,
            delete: true,
            reorder: true,
            ..Default::default()
        }
    }

    fn keys(state: &ListState) -> Vec<u64> {
        state.selection.iter().map(|k| k.0).collect()
    }

    fn down() -> Input {
        Input::Down {
            extend: false,
            cursor_only: false,
        }
    }

    #[test]
    fn arrows_move_and_select() {
        let (mut s, l) = (ListState::default(), lines());
        assert_eq!(reduce(&mut s, &l, down(), cx()), Effect::Selected);
        assert_eq!((s.cursor, keys(&s)), (Some(RowKey(0)), vec![0]));
        reduce(&mut s, &l, down(), cx());
        assert_eq!(keys(&s), vec![1]);
        let up = Input::Up {
            extend: false,
            cursor_only: false,
        };
        reduce(&mut s, &l, up.clone(), cx());
        reduce(&mut s, &l, up, cx());
        assert_eq!(keys(&s), vec![0], "the top row stays the top row");
        assert!(s.keyboard, "the focus ring shows after keys");
    }

    #[test]
    fn shift_extends_from_the_anchor_and_cmd_moves_only_the_cursor() {
        let (mut s, l) = (ListState::default(), lines());
        reduce(
            &mut s,
            &l,
            Input::Click {
                key: RowKey(2),
                extend: false,
                toggle: false,
            },
            cx(),
        );
        let ext = Input::Down {
            extend: true,
            cursor_only: false,
        };
        reduce(&mut s, &l, ext.clone(), cx());
        reduce(&mut s, &l, ext, cx());
        assert_eq!(keys(&s), vec![2, 3, 4]);
        // Back up past the anchor: the range flips side.
        for _ in 0..3 {
            reduce(
                &mut s,
                &l,
                Input::Up {
                    extend: true,
                    cursor_only: false,
                },
                cx(),
            );
        }
        assert_eq!(keys(&s), vec![1, 2]);
        let effect = reduce(
            &mut s,
            &l,
            Input::Down {
                extend: false,
                cursor_only: true,
            },
            cx(),
        );
        assert_eq!(effect, Effect::Moved);
        assert_eq!((s.cursor, keys(&s)), (Some(RowKey(2)), vec![1, 2]));
        // Space toggles the cursor row out, then back in.
        reduce(&mut s, &l, Input::Space, cx());
        assert_eq!(keys(&s), vec![1]);
        reduce(&mut s, &l, Input::Space, cx());
        assert_eq!(keys(&s), vec![1, 2]);
    }

    #[test]
    fn clicks_select_range_and_toggle() {
        let (mut s, l) = (ListState::default(), lines());
        let click = |key, extend, toggle| Input::Click {
            key: RowKey(key),
            extend,
            toggle,
        };
        reduce(&mut s, &l, click(1, false, false), cx());
        reduce(&mut s, &l, click(4, true, false), cx());
        assert_eq!(keys(&s), vec![1, 2, 3, 4]);
        reduce(&mut s, &l, click(2, false, true), cx());
        assert_eq!(keys(&s), vec![1, 3, 4]);
        reduce(&mut s, &l, click(5, false, false), cx());
        assert_eq!(keys(&s), vec![5]);
        assert!(!s.keyboard, "a click hides the focus ring");
        // A single-selection list ignores ⇧ and ⌘.
        let single = Context {
            multiple: false,
            ..cx()
        };
        reduce(&mut s, &l, click(0, true, false), single);
        assert_eq!(keys(&s), vec![0]);
    }

    #[test]
    fn home_end_and_pages_jump() {
        let (mut s, l) = (ListState::default(), lines());
        reduce(&mut s, &l, Input::End { extend: false }, cx());
        assert_eq!(keys(&s), vec![5]);
        reduce(&mut s, &l, Input::PageUp { extend: false }, cx());
        assert_eq!(keys(&s), vec![2]);
        reduce(&mut s, &l, Input::PageDown { extend: true }, cx());
        assert_eq!(keys(&s), vec![2, 3, 4, 5]);
        reduce(&mut s, &l, Input::Home { extend: false }, cx());
        assert_eq!(keys(&s), vec![0]);
    }

    #[test]
    fn select_all_skips_headers_and_escape_clears_in_order() {
        let g = RowKey(100);
        let mut l = vec![Line::header(g, "Resistors", false)];
        l.extend(lines().into_iter().map(|x| x.under(g)));
        let mut s = ListState::default();
        assert_eq!(reduce(&mut s, &l, Input::SelectAll, cx()), Effect::Selected);
        assert_eq!(keys(&s), vec![0, 1, 2, 3, 4, 5]);
        let editing = Context {
            editing: true,
            filtered: true,
            ..cx()
        };
        assert_eq!(
            reduce(&mut s, &l, Input::Escape, editing),
            Effect::CancelEdit
        );
        let filtered = Context {
            filtered: true,
            ..cx()
        };
        assert_eq!(
            reduce(&mut s, &l, Input::Escape, filtered),
            Effect::ClearFilter
        );
        assert_eq!(reduce(&mut s, &l, Input::Escape, cx()), Effect::Selected);
        assert!(s.selection.is_empty());
        assert_eq!(reduce(&mut s, &l, Input::Escape, cx()), Effect::None);
    }

    #[test]
    fn left_and_right_fold_and_walk_the_tree() {
        let g = RowKey(100);
        let mut l = vec![Line::header(g, "Resistors", false)];
        l.extend(lines().into_iter().map(|x| x.under(g)));
        let mut s = ListState::default();
        reduce(
            &mut s,
            &l,
            Input::Down {
                extend: false,
                cursor_only: false,
            },
            cx(),
        );
        assert_eq!(s.cursor, Some(g), "a header takes the cursor");
        assert!(s.selection.is_empty(), "but not the selection");
        assert_eq!(reduce(&mut s, &l, Input::Right, cx()), Effect::Selected);
        assert_eq!(s.cursor, Some(RowKey(0)), "→ on an open group: first child");
        assert_eq!(reduce(&mut s, &l, Input::Left, cx()), Effect::Moved);
        assert_eq!(s.cursor, Some(g), "← on a leaf: its parent");
        assert_eq!(reduce(&mut s, &l, Input::Left, cx()), Effect::Fold(g, true));
        assert!(s.folded.contains(&g));
        let folded = [Line::header(g, "Resistors", true)];
        assert_eq!(
            reduce(&mut s, &folded, Input::Right, cx()),
            Effect::Fold(g, false)
        );
        assert_eq!(
            reduce(&mut s, &folded, Input::Space, cx()),
            Effect::Fold(g, false),
            "Space on a folded header unfolds it"
        );
    }

    #[test]
    fn delete_reorder_and_activate_follow_the_list_rules() {
        let (mut s, l) = (ListState::default(), lines());
        assert_eq!(reduce(&mut s, &l, Input::Delete, cx()), Effect::None);
        reduce(
            &mut s,
            &l,
            Input::Click {
                key: RowKey(3),
                extend: false,
                toggle: false,
            },
            cx(),
        );
        reduce(
            &mut s,
            &l,
            Input::Click {
                key: RowKey(1),
                extend: false,
                toggle: true,
            },
            cx(),
        );
        assert_eq!(
            reduce(&mut s, &l, Input::Delete, cx()),
            Effect::Delete(vec![RowKey(1), RowKey(3)]),
            "display order"
        );
        let fixed = Context {
            delete: false,
            reorder: false,
            ..cx()
        };
        assert_eq!(reduce(&mut s, &l, Input::Delete, fixed), Effect::None);
        assert_eq!(reduce(&mut s, &l, Input::MoveUp, fixed), Effect::None);
        assert_eq!(
            reduce(&mut s, &l, Input::MoveUp, cx()),
            Effect::Reorder(RowKey(1), -1)
        );
        assert_eq!(
            reduce(&mut s, &l, Input::MoveDown, cx()),
            Effect::Reorder(RowKey(1), 1)
        );
        assert_eq!(
            reduce(&mut s, &l, Input::Activate, cx()),
            Effect::Activate(RowKey(1))
        );
        assert_eq!(
            reduce(&mut s, &l, Input::DoubleClick { key: RowKey(4) }, cx()),
            Effect::Activate(RowKey(4))
        );
        assert_eq!(keys(&s), vec![4]);
    }

    #[test]
    fn type_ahead_jumps_within_the_buffer_time() {
        let (mut s, l) = (ListState::default(), lines());
        let typed = |text: &str, now: f64| Input::Type {
            text: text.into(),
            now,
        };
        reduce(&mut s, &l, typed("r", 1.0), cx());
        assert_eq!(s.cursor, Some(RowKey(2)), "r → R1");
        reduce(&mut s, &l, typed("1", 1.2), cx());
        assert_eq!(s.cursor, Some(RowKey(2)), "r1 stays on R1");
        reduce(&mut s, &l, typed("0", 1.4), cx());
        assert_eq!(s.cursor, Some(RowKey(4)), "r10 → R10");
        assert_eq!(s.typing(1.5), Some("r10"));
        assert_eq!(s.typing(3.0), None, "the bubble goes after 700 ms");
        // After the pause, a new buffer; a repeated letter steps.
        reduce(&mut s, &l, typed("c", 3.0), cx());
        assert_eq!(s.cursor, Some(RowKey(0)));
        reduce(&mut s, &l, typed("c", 3.1), cx());
        assert_eq!(s.cursor, Some(RowKey(1)), "cc steps to the next C");
        reduce(&mut s, &l, typed("c", 3.2), cx());
        assert_eq!(s.cursor, Some(RowKey(0)), "and wraps");
        assert_eq!(reduce(&mut s, &l, typed("z", 5.0), cx()), Effect::None);
    }

    #[test]
    fn a_cursor_filtered_away_restarts_at_the_top() {
        let (mut s, l) = (ListState::default(), lines());
        s.cursor = Some(RowKey(99));
        reduce(&mut s, &l, down(), cx());
        assert_eq!(s.cursor, Some(RowKey(0)));
    }
}
