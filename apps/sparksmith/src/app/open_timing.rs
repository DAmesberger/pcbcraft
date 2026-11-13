//! Time to the first frame of the PCB 3D view: from the frame the view was
//! opened in to the frame after the first one that drew the board (that
//! frame was then painted and presented). The board's model is prepared in
//! the background while it is edited in 2D, so opening shows it at once;
//! this measures that. `SPARKSMITH_TIMINGS=1` prints each opening to stderr.
use crate::scene::Display;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct OpenTiming {
    /// When the view was opened, while its first frame is still to come.
    opened: Option<Instant>,
    /// How the first frame with the board in it showed it.
    drawn: Option<Display>,
    was_open: bool,
}

impl OpenTiming {
    /// At the start of each frame, whether the 3D view is open in it.
    /// Returns a finished measurement: the time from opening to the end
    /// of the first frame with the board in it, and how it showed it.
    pub fn frame(&mut self, open: bool, now: Instant) -> Option<(Duration, Display)> {
        let done = match (self.opened, self.drawn) {
            (Some(opened), Some(how)) => {
                self.opened = None;
                self.drawn = None;
                Some((now.saturating_duration_since(opened), how))
            }
            _ => None,
        };
        if open && !self.was_open {
            self.opened = Some(now);
            self.drawn = None;
        } else if !open {
            self.opened = None;
            self.drawn = None;
        }
        self.was_open = open;
        done
    }

    /// The view drew the board this frame, as `how` says.
    pub fn drew(&mut self, how: Display) {
        if self.opened.is_some() && self.drawn.is_none() && how != Display::Blank {
            self.drawn = Some(how);
        }
    }

    /// Whether a measurement waits for a frame.
    pub fn pending(&self) -> bool {
        self.opened.is_some()
    }
}

/// Print a finished measurement when `SPARKSMITH_TIMINGS` is set.
pub fn report(measured: Option<(Duration, Display)>) {
    let Some((took, how)) = measured else {
        return;
    };
    if std::env::var_os("SPARKSMITH_TIMINGS").is_some() {
        let shown = match how {
            Display::Current => "the current board",
            Display::Stale => "the last board, updating",
            Display::Blank => "nothing",
        };
        eprintln!(
            "sparksmith: PCB 3D first frame {:.1} ms after opening ({shown})",
            took.as_secs_f64() * 1e3
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_with_the_board_ends_the_measurement() {
        let mut timing = OpenTiming::default();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        assert_eq!(timing.frame(false, at(0)), None);
        // Opened: frames without the board do not end it.
        assert_eq!(timing.frame(true, at(10)), None);
        timing.drew(Display::Blank);
        assert!(timing.pending());
        assert_eq!(timing.frame(true, at(20)), None);
        timing.drew(Display::Current);
        // The next frame starts once that one is presented.
        assert_eq!(
            timing.frame(true, at(35)),
            Some((Duration::from_millis(25), Display::Current))
        );
        assert!(!timing.pending());
        timing.drew(Display::Current);
        assert_eq!(timing.frame(true, at(50)), None);
        // Closed before the board showed: no measurement; reopened: a new one.
        assert_eq!(timing.frame(false, at(60)), None);
        assert_eq!(timing.frame(true, at(70)), None);
        assert_eq!(timing.frame(false, at(80)), None);
        timing.drew(Display::Stale);
        assert_eq!(timing.frame(true, at(90)), None);
        timing.drew(Display::Stale);
        assert_eq!(
            timing.frame(true, at(95)),
            Some((Duration::from_millis(5), Display::Stale))
        );
    }
}
