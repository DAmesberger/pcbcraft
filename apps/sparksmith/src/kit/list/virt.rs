//! Virtualisation without egui: every line has the same height, so where a
//! line is, which lines the viewport shows, where to scroll to reveal one
//! and where the sticky group header sits are all O(1) or one search over
//! the header positions. Only the shown lines plus [`OVERSCAN`] on each
//! side are ever built, so a list of 10 000 vias costs what a screenful
//! does.
use std::ops::Range;

/// Lines built beyond each edge of the viewport.
pub const OVERSCAN: usize = 4;

/// The lines of `len`, each `row` high, that a viewport `height` high at
/// scroll offset `top` shows, with [`OVERSCAN`] more on each side.
pub fn visible(len: usize, row: f32, top: f32, height: f32) -> Range<usize> {
    if len == 0 || row <= 0.0 || !row.is_finite() {
        return 0..0;
    }
    let top = top.max(0.0);
    let first = ((top / row).floor() as usize).min(len);
    let last = (((top + height.max(0.0)) / row).ceil() as usize).min(len);
    first.saturating_sub(OVERSCAN)..last.saturating_add(OVERSCAN).min(len)
}

/// The height of `len` lines.
pub fn content(len: usize, row: f32) -> f32 {
    len as f32 * row
}

/// The scroll offset that shows line `index` whole, moving as little as
/// possible from `top` (nothing when it already shows).
pub fn reveal(index: usize, row: f32, top: f32, height: f32) -> f32 {
    let y = index as f32 * row;
    if y < top {
        y
    } else if y + row > top + height {
        (y + row - height).max(0.0)
    } else {
        top
    }
}

/// Where a line is relative to the viewport, for the cross-highlight's
/// edge pill ("R14 ↓").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Above,
    Shown,
    Below,
}

pub fn place(index: usize, row: f32, top: f32, height: f32) -> Place {
    let y = index as f32 * row;
    if y + row <= top {
        Place::Above
    } else if y >= top + height {
        Place::Below
    } else {
        Place::Shown
    }
}

/// The group header pinned at the viewport's top: the header line and its
/// offset from the top (0, or negative while the next header pushes it up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sticky {
    pub header: usize,
    pub offset: f32,
}

/// The sticky header for the sorted line indices `headers` of group
/// headers. None while the first group's own header still shows in place.
pub fn sticky(headers: &[usize], row: f32, top: f32) -> Option<Sticky> {
    if row <= 0.0 || top <= 0.0 {
        return None;
    }
    // The last header that starts at or above the viewport's top.
    let at = headers.partition_point(|&h| h as f32 * row <= top);
    let current = *headers.get(at.checked_sub(1)?)?;
    let offset = headers
        .get(at)
        .map_or(0.0, |&next| (next as f32 * row - top - row).min(0.0));
    Some(Sticky {
        header: current,
        offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_viewport_and_overscan_are_built() {
        // 10 000 vias at 26 pt in a 400 pt panel scrolled to line 5000.
        let r = visible(10_000, 26.0, 5000.0 * 26.0, 400.0);
        assert_eq!(r, 4996..5020);
        assert!(r.len() <= 400 / 26 + 2 + 2 * OVERSCAN);
        assert_eq!(visible(10, 26.0, 0.0, 400.0), 0..10);
        assert_eq!(visible(0, 26.0, 0.0, 400.0), 0..0);
        assert_eq!(visible(5, 0.0, 0.0, 400.0), 0..0, "no height: nothing");
        assert_eq!(visible(100, 26.0, 1e9, 400.0), 96..100, "past the end");
    }

    #[test]
    fn reveal_scrolls_the_least() {
        assert_eq!(reveal(2, 26.0, 0.0, 260.0), 0.0, "already shown");
        assert_eq!(reveal(20, 26.0, 0.0, 260.0), 21.0 * 26.0 - 260.0);
        assert_eq!(reveal(1, 26.0, 200.0, 260.0), 26.0, "above: to the top");
        assert_eq!(place(1, 26.0, 200.0, 260.0), Place::Above);
        assert_eq!(place(8, 26.0, 200.0, 260.0), Place::Shown);
        assert_eq!(place(18, 26.0, 200.0, 260.0), Place::Below);
    }

    #[test]
    fn the_next_header_pushes_the_sticky_one_up() {
        let headers = [0, 5, 9];
        assert_eq!(sticky(&headers, 20.0, 0.0), None, "in place at the top");
        assert_eq!(
            sticky(&headers, 20.0, 30.0),
            Some(Sticky {
                header: 0,
                offset: 0.0
            })
        );
        // Header 5 is at y = 100; at top 90 it is 10 below the top, so the
        // pinned header (20 high) is pushed up by 10.
        assert_eq!(
            sticky(&headers, 20.0, 90.0),
            Some(Sticky {
                header: 0,
                offset: -10.0
            })
        );
        assert_eq!(
            sticky(&headers, 20.0, 100.0),
            Some(Sticky {
                header: 5,
                offset: 0.0
            })
        );
        assert_eq!(
            sticky(&headers, 20.0, 500.0),
            Some(Sticky {
                header: 9,
                offset: 0.0
            })
        );
        assert_eq!(sticky(&[3], 20.0, 30.0), None, "no header above the top");
    }
}
