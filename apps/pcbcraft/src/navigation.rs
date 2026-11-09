//! Wheel, trackpad and pinch input shared by every canvas: the 3D view and
//! the flat schematic, PCB, symbol and footprint canvases read a frame's
//! scrolling through [`Trackpad`], so a trackpad and a mouse move every view
//! the same way.
//!
//! This follows Solidcraft's navigation handler with its default Fusion
//! preset and two-finger panning on (upstream `crates/ui-egui/src/navigation.rs`
//! `immediate_wheel`, `TrackpadCoast`, `route_wheel_vertical` and `handle`,
//! MIT, revision 4f3ac07; see ATTRIBUTION.md). Upstream keeps it private to
//! its app, so the input routing is reproduced here:
//!
//! - Trackpad scrolls arrive bracketed (`Start` … `Move` … `End`) in points;
//!   two fingers pan, and in 3D Shift with two fingers orbits.
//! - Bare `Move` events are a mouse wheel: vertical zooms toward the pointer,
//!   horizontal (a thumb wheel) pans sideways.
//! - A pinch (and Cmd/Ctrl with the wheel) zooms toward the pointer.
//! - The inertia macOS sends once the fingers lift coasts in the mode the
//!   fingers had, and ends when the modifiers change.

use egui::{InputOptions, InputState, Modifiers, MouseWheelUnit, TouchPhase, Vec2, vec2};

/// The camera distance (or the flat canvas's scale) changes by `e` to the
/// power of this per point of wheel scrolling.
pub const ZOOM_PER_POINT: f32 = 0.002;

/// The largest scroll accepted from one frame on either axis (points).
const MAX_SCROLL: f32 = 1e4;

/// What one frame of wheel, trackpad and pinch input asks of a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheel {
    /// Mouse-wheel zoom in points: positive (scrolling up) zooms in.
    pub zoom: f32,
    /// How far the content follows the fingers (two-finger scroll) or a
    /// thumb wheel, in points.
    pub pan: Vec2,
    /// Shift with two fingers, in points: the 3D view orbits by it.
    pub orbit: Vec2,
    /// Pinch (or Cmd/Ctrl+wheel) zoom factor: above 1 zooms in.
    pub pinch: f32,
}

impl Wheel {
    pub const NONE: Wheel = Wheel {
        zoom: 0.0,
        pan: Vec2::ZERO,
        orbit: Vec2::ZERO,
        pinch: 1.0,
    };

    /// The flat canvas's scale factor for this frame: the wheel and the
    /// pinch together (above 1 zooms in).
    pub fn scale_factor(&self) -> f32 {
        let factor = (self.zoom * ZOOM_PER_POINT).exp() * self.pinch;
        if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        }
    }

    /// Whether this frame moves the view at all.
    pub fn moves(&self) -> bool {
        *self != Wheel::NONE
    }
}

/// The inertia macOS sends once the fingers lift: its own scroll bracket,
/// starting as the fingers' bracket ends. It coasts in the mode the fingers
/// had; when the modifiers change it ends, so the rest never orbits a pan or
/// pans an orbit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Coast {
    /// Whether the current bracket's last move orbited (Shift).
    last_orbit: Option<bool>,
    /// The current bracket is a coast, keeping this mode.
    keeps: Option<bool>,
    /// The coast's mode changed: the rest of its moves are dropped.
    stopped: bool,
    /// When the fingers' last swipe ended, and whether it orbited.
    lifted: Option<(f64, bool)>,
}

impl Coast {
    /// A coast starts at most this long (seconds) after the fingers lift.
    const GAP: f64 = 0.1;

    fn start(&mut self, now: f64) {
        let lifted = self
            .lifted
            .take()
            .filter(|(at, _)| now >= *at && now - *at <= Self::GAP);
        *self = Coast {
            keeps: lifted.map(|(_, orbit)| orbit),
            ..Default::default()
        };
    }

    fn end(&mut self, now: f64) {
        let lifted = if self.keeps.is_none() {
            self.last_orbit.map(|orbit| (now, orbit))
        } else {
            None
        };
        *self = Coast {
            lifted,
            ..Default::default()
        };
    }

    /// Whether a move in this mode still counts.
    fn moves(&mut self, orbit: bool) -> bool {
        self.last_orbit = Some(orbit);
        self.stopped |= self.keeps.is_some_and(|keeps| keeps != orbit);
        !self.stopped
    }
}

/// One canvas's trackpad state across frames.
#[derive(Clone, Copy, Debug, Default)]
pub struct Trackpad {
    /// A bracketed trackpad scroll is in progress.
    gesture: bool,
    coast: Coast,
    /// The trackpad ran this scroll: egui's smoothed wheel tail is ignored
    /// until scrolling stops, so two-finger input is never replayed as zoom.
    until_idle: bool,
}

/// Wheel points on the vertical axis, following egui's scroll modifiers
/// (Shift makes a mouse wheel scroll sideways, which does not zoom).
fn vertical(points: Vec2, modifiers: Modifiers, options: &InputOptions) -> f32 {
    let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
    let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
    if horizontal && !vertical {
        0.0
    } else if vertical && !horizontal {
        points.x + points.y
    } else {
        points.y
    }
}

fn bounded(v: Vec2) -> Vec2 {
    if v.is_finite() {
        v.clamp(vec2(-MAX_SCROLL, -MAX_SCROLL), vec2(MAX_SCROLL, MAX_SCROLL))
    } else {
        Vec2::ZERO
    }
}

impl Trackpad {
    /// Read this frame's wheel, trackpad and pinch input. With
    /// `shift_orbits` (the 3D view), Shift with two fingers orbits; on a flat
    /// canvas it pans like two fingers alone. Call it every frame the canvas
    /// is shown, hovered or not, so a gesture's bracket is followed; apply
    /// the result only while the pointer is over the canvas.
    pub fn read(&mut self, ui: &egui::Ui, shift_orbits: bool) -> Wheel {
        let options = ui.ctx().options(|o| o.input_options);
        ui.input(|input| self.read_input(input, &options, shift_orbits))
    }

    fn read_input(
        &mut self,
        input: &InputState,
        options: &InputOptions,
        shift_orbits: bool,
    ) -> Wheel {
        let mut zoom = 0.0;
        let mut pan = Vec2::ZERO;
        let mut shift_pan = Vec2::ZERO;
        let mut mouse_pan = Vec2::ZERO;
        let zoom_modifier = options.zoom_modifier;
        let page = input.viewport_rect().height();
        for event in &input.events {
            let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                phase,
                ..
            } = event
            else {
                continue;
            };
            // Cmd/Ctrl+wheel is egui's zoom: it arrives in `zoom_delta`.
            if modifiers.matches_any(zoom_modifier) {
                continue;
            }
            let point = *unit == MouseWheelUnit::Point;
            match phase {
                TouchPhase::Start => {
                    if point {
                        if !self.gesture {
                            self.coast.start(input.time);
                        }
                        self.gesture = true;
                    }
                    continue;
                }
                TouchPhase::End | TouchPhase::Cancel => {
                    if point {
                        if self.gesture {
                            self.coast.end(input.time);
                        }
                        self.gesture = false;
                    }
                    continue;
                }
                TouchPhase::Move => {}
            }
            let points = bounded(match unit {
                MouseWheelUnit::Point => *delta,
                MouseWheelUnit::Line => *delta * options.line_scroll_speed,
                MouseWheelUnit::Page => *delta * page,
            });
            let shift = modifiers.shift && !modifiers.ctrl && !modifiers.command && !modifiers.alt;
            if point && self.gesture {
                if !self.coast.moves(shift) && shift_orbits {
                    continue;
                }
                if shift {
                    shift_pan += points;
                } else {
                    pan += points;
                }
            } else if point {
                // Bare moves are a mouse: vertical zooms, horizontal pans.
                // A wheel needs the opposite sign of fingers for the same
                // motion.
                zoom += vertical(points, *modifiers, options);
                mouse_pan += vec2(-points.x, 0.0);
            } else {
                zoom += vertical(points, *modifiers, options);
            }
        }
        let fingers = pan != Vec2::ZERO || shift_pan != Vec2::ZERO;
        if fingers {
            self.until_idle = true;
        } else if !input.is_scrolling() {
            self.until_idle = false;
            // With no scroll activity no gesture can be in flight (its End
            // was lost); a stuck flag would pan later mouse input.
            self.gesture = false;
        }
        // egui's smoothed wheel for a mouse; the raw wheel while fingers
        // drive the scroll, so their tail never zooms.
        let zoom = if self.until_idle {
            zoom
        } else {
            input.smooth_scroll_delta.y
        };
        let pinch = input.zoom_delta();
        let (pan, orbit) = if shift_orbits {
            (pan + mouse_pan, shift_pan)
        } else {
            (pan + shift_pan + mouse_pan, Vec2::ZERO)
        };
        Wheel {
            zoom: if zoom.is_finite() { zoom } else { 0.0 },
            pan: bounded(pan),
            orbit: bounded(orbit),
            pinch: if pinch.is_finite() && pinch > 0.0 {
                pinch
            } else {
                1.0
            },
        }
    }

    /// A two-finger orbit is in progress (its pivot is kept until it ends).
    pub fn in_gesture(&self) -> bool {
        self.gesture
    }
}

/// The band at a canvas's foot that the floating navigation bar (Fit
/// view) covers: its height and its gap above the canvas bottom. Toasts
/// sit above it, and a flat canvas's fit keeps the page out of it.
pub fn bar_band() -> f32 {
    let t = solidcraft_theme::Tokens::get();
    t.space_16 + t.control_height + 2.0 * t.space_4
}

/// The platform's name for the Command key in help text.
pub fn command_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

/// The 3D view's one-line canvas hint (the full list is the Fit view
/// button's hover help, [`help_3d`]).
pub fn hint_3d() -> &'static str {
    "Shift + two fingers to orbit · two fingers to pan · pinch to zoom"
}

/// The 3D view's gestures, listed in the Keyboard shortcuts sheet under
/// PCB › 3D (none is a key, so none takes one): trackpad, mouse and the
/// navigation cube.
pub const NAVIGATION_3D: &[(&str, &str)] = &[
    ("Shift + two fingers", "Orbit (trackpad)"),
    ("Two fingers", "Pan (trackpad)"),
    ("Pinch", "Zoom toward the pointer (trackpad)"),
    ("Right-drag", "Orbit (mouse)"),
    ("Shift + middle-drag", "Orbit (mouse)"),
    ("Middle-drag", "Pan (mouse)"),
    ("Wheel", "Zoom toward the pointer (mouse)"),
    ("Cmd/Ctrl + drag on empty space", "Orbit"),
    ("Space + drag", "Pan"),
    ("Drag the cube", "Orbit"),
    ("Click a cube face or corner", "Look from that side"),
    ("Arrow keys", "Turn the view 15° (with Shift: 90°)"),
    ("Middle double-click", "Fit the board"),
];

/// Hover help for the 3D view: every way to move it.
pub fn help_3d() -> String {
    let cmd = command_key();
    [
        "Trackpad: two fingers pan · Shift+two fingers orbit · pinch zooms".to_owned(),
        "Mouse: middle-drag pans · right-drag or Shift+middle orbits · wheel zooms".to_owned(),
        "Cube: click a face or corner for that view · drag it to orbit".to_owned(),
        format!("Space+drag pans · {cmd}+drag on empty space orbits"),
        format!("Arrows turn the view (Shift: 90°) · {cmd}+arrows pan"),
        "Middle double-click fits".to_owned(),
    ]
    .join("\n")
}

/// Hover help for the Component 3D view, where the left button and the
/// arrows place the model.
pub fn help_3d_placement() -> String {
    [
        "Trackpad: two fingers pan · Shift+two fingers orbit · pinch zooms",
        "Mouse: middle-drag pans · right-drag or Shift+middle orbits · wheel zooms",
        "Middle double-click fits",
    ]
    .join("\n")
}

/// Hover help for a flat canvas; `space` when Space+drag pans there.
pub fn help_2d(space: bool) -> String {
    let mut lines = vec![
        "Trackpad: two fingers pan · pinch zooms".to_owned(),
        "Mouse: middle-drag pans · wheel zooms".to_owned(),
    ];
    if space {
        lines.push("Space+drag pans".to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use egui::{Event, Pos2, RawInput, Rect};

    /// One trackpad scroll move of `delta` points with `modifiers`.
    pub fn scroll(delta: Vec2, modifiers: Modifiers, phase: TouchPhase) -> Event {
        Event::MouseWheel {
            source: egui::MouseWheelSource::Unknown,
            unit: MouseWheelUnit::Point,
            delta,
            modifiers,
            phase,
        }
    }

    /// A complete two-finger swipe of `steps` moves of `delta`.
    pub fn swipe(delta: Vec2, modifiers: Modifiers, steps: usize) -> Vec<Event> {
        let mut events = vec![scroll(Vec2::ZERO, modifiers, TouchPhase::Start)];
        events.extend((0..steps).map(|_| scroll(delta, modifiers, TouchPhase::Move)));
        events.push(scroll(Vec2::ZERO, modifiers, TouchPhase::End));
        events
    }

    /// One mouse-wheel notch (lines, no bracket).
    pub fn notch(lines: f32) -> Event {
        Event::MouseWheel {
            source: egui::MouseWheelSource::Unknown,
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, lines),
            modifiers: Modifiers::NONE,
            phase: TouchPhase::Move,
        }
    }

    fn read(
        ctx: &egui::Context,
        trackpad: &mut Trackpad,
        time: f64,
        events: Vec<Event>,
        shift_orbits: bool,
    ) -> Wheel {
        let mut wheel = Wheel::NONE;
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| wheel = trackpad.read(ui, shift_orbits),
        );
        output.textures_delta.clear();
        wheel
    }

    #[test]
    fn two_fingers_pan_and_never_zoom() {
        let ctx = egui::Context::default();
        let mut trackpad = Trackpad::default();
        let wheel = read(
            &ctx,
            &mut trackpad,
            0.1,
            swipe(vec2(3.0, -4.0), Modifiers::NONE, 2),
            true,
        );
        assert_eq!(wheel.pan, vec2(6.0, -8.0));
        assert_eq!(wheel.orbit, Vec2::ZERO);
        assert_eq!(wheel.zoom, 0.0, "a trackpad swipe must not zoom");
        // egui's smoothing tail of the swipe is ignored too.
        for k in 0..5 {
            let tail = read(
                &ctx,
                &mut trackpad,
                0.2 + f64::from(k) * 0.016,
                vec![],
                true,
            );
            assert_eq!(tail.zoom, 0.0, "frame {k}");
        }
    }

    #[test]
    fn shift_with_two_fingers_orbits_in_3d_and_pans_flat() {
        let ctx = egui::Context::default();
        let mut trackpad = Trackpad::default();
        let wheel = read(
            &ctx,
            &mut trackpad,
            0.1,
            swipe(vec2(5.0, 0.0), Modifiers::SHIFT, 1),
            true,
        );
        assert_eq!((wheel.pan, wheel.orbit), (Vec2::ZERO, vec2(5.0, 0.0)));
        let ctx = egui::Context::default();
        let mut flat = Trackpad::default();
        let wheel = read(
            &ctx,
            &mut flat,
            0.1,
            swipe(vec2(5.0, 0.0), Modifiers::SHIFT, 1),
            false,
        );
        assert_eq!((wheel.pan, wheel.orbit), (vec2(5.0, 0.0), Vec2::ZERO));
    }

    #[test]
    fn a_mouse_wheel_zooms_and_a_thumb_wheel_pans() {
        let ctx = egui::Context::default();
        let mut trackpad = Trackpad::default();
        let mut zoom = 0.0;
        for k in 0..30 {
            let events = if k == 0 { vec![notch(1.0)] } else { vec![] };
            let wheel = read(&ctx, &mut trackpad, f64::from(k) * 0.016, events, true);
            assert_eq!(wheel.pan, Vec2::ZERO);
            zoom += wheel.zoom;
        }
        assert!(zoom > 1.0, "a notch up zooms in: {zoom}");
        let thumb = Event::MouseWheel {
            source: egui::MouseWheelSource::Unknown,
            unit: MouseWheelUnit::Point,
            delta: vec2(7.0, 0.0),
            modifiers: Modifiers::NONE,
            phase: TouchPhase::Move,
        };
        let wheel = read(&ctx, &mut Trackpad::default(), 2.0, vec![thumb], true);
        assert_eq!(wheel.pan, vec2(-7.0, 0.0));
    }

    #[test]
    fn a_pinch_is_a_zoom_factor() {
        let ctx = egui::Context::default();
        let wheel = read(
            &ctx,
            &mut Trackpad::default(),
            0.1,
            vec![Event::Zoom(1.25)],
            true,
        );
        assert!((wheel.pinch - 1.25).abs() < 1e-6, "{}", wheel.pinch);
        assert!((wheel.scale_factor() - 1.25).abs() < 1e-6);
        // A broken factor is no zoom.
        let wheel = read(
            &ctx,
            &mut Trackpad::default(),
            0.2,
            vec![Event::Zoom(f32::NAN)],
            true,
        );
        assert_eq!(wheel.pinch, 1.0);
    }

    /// The inertia after a Shift orbit keeps orbiting, and stops when Shift
    /// is let go mid-coast instead of turning into a pan.
    #[test]
    fn a_coast_keeps_its_mode_and_stops_when_the_modifiers_change() {
        let ctx = egui::Context::default();
        let mut trackpad = Trackpad::default();
        read(
            &ctx,
            &mut trackpad,
            0.1,
            swipe(vec2(4.0, 0.0), Modifiers::SHIFT, 1),
            true,
        );
        let mut coast = vec![scroll(Vec2::ZERO, Modifiers::SHIFT, TouchPhase::Start)];
        coast.push(scroll(vec2(2.0, 0.0), Modifiers::SHIFT, TouchPhase::Move));
        coast.push(scroll(vec2(2.0, 0.0), Modifiers::NONE, TouchPhase::Move));
        coast.push(scroll(vec2(2.0, 0.0), Modifiers::SHIFT, TouchPhase::Move));
        coast.push(scroll(Vec2::ZERO, Modifiers::NONE, TouchPhase::End));
        let wheel = read(&ctx, &mut trackpad, 0.15, coast, true);
        assert_eq!(wheel.orbit, vec2(2.0, 0.0));
        assert_eq!(wheel.pan, Vec2::ZERO, "the coast never pans");
    }

    #[test]
    fn help_lines_stay_short() {
        assert!(hint_3d().chars().count() <= 80);
        for help in [help_3d(), help_3d_placement(), help_2d(true)] {
            for line in help.lines() {
                assert!(line.chars().count() <= 80, "{line}");
            }
        }
    }
}
