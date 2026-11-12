//! What a list is made of, as the pure modules see it: stable row keys and
//! the shown rows (groups and items) in display order. Feature code builds
//! [`Line`]s from its own data each frame; nothing here owns that data.
use std::hash::{Hash, Hasher};

/// A row's identity across frames, folds, sorts and filters: a hash of the
/// object it shows, never its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RowKey(pub u64);

impl RowKey {
    /// The key of anything hashable (an id, a name, a tuple of them).
    pub fn of(value: impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// Whether a line folds, and how it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    /// Does not fold.
    Leaf,
    Open,
    Folded,
}

impl Fold {
    pub fn folds(self) -> bool {
        self != Self::Leaf
    }
}

/// One shown line of a list, in display order: a group header or an item.
/// Items under a folded group are not shown and so are not lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Line<'a> {
    pub key: RowKey,
    /// The line this one sits under (its group header or tree parent).
    pub parent: Option<RowKey>,
    pub fold: Fold,
    /// Group headers take no part in the selection.
    pub header: bool,
    /// What type-ahead matches: the row's name as shown.
    pub name: &'a str,
}

impl<'a> Line<'a> {
    /// An item that does not fold.
    pub fn item(key: RowKey, name: &'a str) -> Self {
        Self {
            key,
            parent: None,
            fold: Fold::Leaf,
            header: false,
            name,
        }
    }

    /// A group header, open or folded.
    pub fn header(key: RowKey, name: &'a str, folded: bool) -> Self {
        Self {
            key,
            parent: None,
            fold: if folded { Fold::Folded } else { Fold::Open },
            header: true,
            name,
        }
    }

    pub fn under(mut self, parent: RowKey) -> Self {
        self.parent = Some(parent);
        self
    }
}

/// Natural order of designators and names: digit runs compare as numbers,
/// so R2 sorts before R10; letters compare without case.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    natural(a, b, true).then_with(|| natural(a, b, false))
}

/// [`natural_cmp`]'s one pass: letters without case when `fold`.
fn natural(a: &str, b: &str, fold: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (da, db) = (take(&mut a), take(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let order = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| da.len().cmp(&db.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = if fold {
                    x.to_lowercase().cmp(y.to_lowercase())
                } else {
                    x.cmp(&y)
                };
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn designators_sort_naturally() {
        let mut refs = vec!["R10", "C2", "R2", "r1", "R100", "U1", "C10", "R02"];
        refs.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            refs,
            vec!["C2", "C10", "r1", "R2", "R02", "R10", "R100", "U1"]
        );
    }

    #[test]
    fn keys_follow_identity_not_position() {
        assert_eq!(RowKey::of(("dim", 7_u64)), RowKey::of(("dim", 7_u64)));
        assert_ne!(RowKey::of(("dim", 7_u64)), RowKey::of(("dim", 8_u64)));
    }
}
