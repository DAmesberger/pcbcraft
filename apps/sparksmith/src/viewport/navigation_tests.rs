//! The 3D view's camera under synthetic egui input: a trackpad (pinch, two
//! fingers, Shift+two fingers), a mouse (wheel, middle and right drags) and
//! the keyboard, as Solidcraft's default bindings move it.
use super::*;
use crate::navigation::tests::{notch, swipe};
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use sparksmith_solidcraft::material::BodyKind;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Nav {
    ctx: egui::Context,
    viewport: Viewport,
    time: f64,
    caps: NavCaps,
}

impl Nav {
    fn new(caps: NavCaps) -> Result<Self> {
        let project = crate::model_scene::tests::demo_project()?;
        let model = sparksmith_solidcraft::project::build(&project)?;
        let mut viewport = Viewport::detached();
        viewport.set_model(crate::model_scene::build(model.geometry, &model.stack)?);
        let mut nav = Self {
            ctx: egui::Context::default(),
            viewport,
            time: 0.0,
            caps,
        };
        nav.frame(vec![], Modifiers::NONE);
        nav.viewport.fit();
        nav.frame(vec![], Modifiers::NONE);
        Ok(nav)
    }

    /// One 800 × 600 frame, a 60th of a second after the last; the pick a
    /// click made.
    fn frame(&mut self, events: Vec<Event>, modifiers: Modifiers) -> Option<Option<u64>> {
        self.time += 1.0 / 60.0;
        let (viewport, caps) = (&mut self.viewport, self.caps);
        let mut pick = None;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                time: Some(self.time),
                events: [vec![Event::ModifiersChanged(modifiers)], events].concat(),
                ..Default::default()
            },
            |ui| pick = viewport.show(ui, caps),
        );
        output.textures_delta.clear();
        pick
    }

    fn rect(&self) -> Result<Rect> {
        Ok(self.viewport.rect().ok_or("the view was not drawn")?)
    }

    fn button(at: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers,
        }
    }

    /// Press `button` at `from`, move to `to` in `steps` frames and let go;
    /// the picks any frame made.
    fn drag(
        &mut self,
        button: PointerButton,
        from: Pos2,
        to: Pos2,
        modifiers: Modifiers,
        steps: usize,
    ) -> Vec<Option<u64>> {
        let mut picks = vec![];
        picks.extend(self.frame(vec![Event::PointerMoved(from)], modifiers));
        picks.extend(self.frame(vec![Self::button(from, button, true, modifiers)], modifiers));
        for k in 1..=steps {
            let at = from + (to - from) * (k as f32 / steps as f32);
            picks.extend(self.frame(vec![Event::PointerMoved(at)], modifiers));
        }
        picks.extend(self.frame(vec![Self::button(to, button, false, modifiers)], modifiers));
        picks.extend(self.frame(vec![], modifiers));
        picks
    }

    /// A click of `button` at `at`; the pick it made.
    fn click(&mut self, button: PointerButton, at: Pos2, modifiers: Modifiers) -> Vec<Option<u64>> {
        let mut picks = vec![];
        picks.extend(self.frame(vec![Event::PointerMoved(at)], modifiers));
        picks.extend(self.frame(vec![Self::button(at, button, true, modifiers)], modifiers));
        picks.extend(self.frame(vec![Self::button(at, button, false, modifiers)], modifiers));
        picks.extend(self.frame(vec![], modifiers));
        picks
    }

    /// Where `p` is drawn on screen.
    fn screen(&self, p: Vec3) -> Result<Pos2> {
        let rect = self.rect()?;
        let (x, y, _) = self
            .viewport
            .camera
            .to_screen(
                p,
                f64::from(rect.width()),
                f64::from(rect.height()),
                self.viewport.bounds.diagonal(),
            )
            .ok_or("behind the eye")?;
        Ok(rect.min + vec2(x as f32, y as f32))
    }

    /// The top of a package: a pickable item and its point on screen.
    fn package(&self) -> Result<(u64, Pos2)> {
        let model = self.viewport.model.as_ref().ok_or("no model")?;
        let body = model
            .geometry
            .iter()
            .find(|g| g.kind.is_envelope() && g.min[2] > 1.0 && g.item.is_some())
            .ok_or("no top package")?;
        let top = Vec3::new(
            (body.min[0] + body.max[0]) / 2.0,
            (body.min[1] + body.max[1]) / 2.0,
            body.max[2],
        );
        Ok((body.item.ok_or("no item")?, self.screen(top)?))
    }

    /// A point of the view where nothing is drawn.
    fn empty(&self) -> Result<Pos2> {
        let p = self.rect()?.min + vec2(6.0, 6.0);
        let local = p - self.rect()?.min;
        let rect = self.rect()?;
        let hit = self.viewport.hit(
            f64::from(local.x),
            f64::from(local.y),
            f64::from(rect.width()),
            f64::from(rect.height()),
        );
        if hit.is_some() {
            return Err("the corner is not empty".into());
        }
        Ok(p)
    }
}

fn turned(a: &Camera, b: &Camera) -> bool {
    (a.yaw - b.yaw).abs() > 1e-6 || (a.pitch - b.pitch).abs() > 1e-6
}

fn moved(a: &Camera, b: &Camera) -> bool {
    (a.target - b.target).len() > 1e-9
}

/// A pinch zooms toward the point under the pointer, which stays there.
#[test]
fn a_pinch_zooms_toward_the_point_under_the_pointer() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let rect = nav.rect()?;
    let at = rect.min + vec2(520.0, 240.0);
    let anchor = nav.viewport.surface_at(rect, at).ok_or("no anchor")?;
    let before = nav.viewport.camera;
    nav.frame(
        vec![Event::PointerMoved(at), Event::Zoom(1.5)],
        Modifiers::NONE,
    );
    let after = nav.viewport.camera;
    assert!(
        (after.distance - before.distance / 1.5).abs() < 1e-6 * before.distance,
        "{} → {}",
        before.distance,
        after.distance
    );
    assert!(!turned(&before, &after));
    let still = nav.screen(anchor)?;
    assert!((still - at).length() < 0.05, "{at:?} → {still:?}");
    // Pinching together zooms back out.
    nav.frame(vec![Event::Zoom(1.0 / 1.5)], Modifiers::NONE);
    assert!((nav.viewport.camera.distance - before.distance).abs() < 1e-6 * before.distance);
    Ok(())
}

/// Two fingers pan with the fingers and never zoom or turn the view; Shift
/// with two fingers orbits it.
#[test]
fn two_fingers_pan_and_shift_two_fingers_orbit() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let at = nav.rect()?.center();
    nav.frame(vec![Event::PointerMoved(at)], Modifiers::NONE);
    let rect = nav.rect()?;
    let anchor = nav.viewport.surface_at(rect, at).ok_or("no anchor")?;
    let before = nav.viewport.camera;
    nav.frame(swipe(vec2(10.0, 0.0), Modifiers::NONE, 3), Modifiers::NONE);
    for _ in 0..10 {
        nav.frame(vec![], Modifiers::NONE);
    }
    let panned = nav.viewport.camera;
    assert!(moved(&before, &panned), "two fingers must pan");
    assert!(!turned(&before, &panned), "two fingers must not orbit");
    assert_eq!(
        panned.distance, before.distance,
        "two fingers must not zoom"
    );
    // The model followed the fingers: 30 points to the right.
    let followed = nav.screen(anchor)?;
    assert!(
        (followed - (at + vec2(30.0, 0.0))).length() < 0.5,
        "{followed:?}"
    );

    nav.frame(
        swipe(vec2(12.0, 6.0), Modifiers::SHIFT, 2),
        Modifiers::SHIFT,
    );
    let orbited = nav.viewport.camera;
    assert!(turned(&panned, &orbited), "Shift + two fingers must orbit");
    assert!((orbited.distance - panned.distance).abs() < 1e-9);
    Ok(())
}

/// A mouse wheel notch zooms toward the pointer, without panning or turning.
#[test]
fn a_mouse_wheel_zooms_the_3d_view() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let at = nav.rect()?.min + vec2(300.0, 350.0);
    let anchor = nav
        .viewport
        .surface_at(nav.rect()?, at)
        .ok_or("no anchor")?;
    nav.frame(vec![Event::PointerMoved(at)], Modifiers::NONE);
    let before = nav.viewport.camera;
    nav.frame(vec![notch(2.0)], Modifiers::NONE);
    for _ in 0..30 {
        nav.frame(vec![], Modifiers::NONE);
    }
    let after = nav.viewport.camera;
    assert!(
        after.distance < before.distance * 0.95,
        "{} → {}",
        before.distance,
        after.distance
    );
    assert!(!turned(&before, &after));
    let still = nav.screen(anchor)?;
    assert!((still - at).length() < 0.05, "{still:?}");
    Ok(())
}

/// Right-drag orbits and selects nothing; a right click that does not move
/// leaves the view alone.
#[test]
fn right_drag_orbits_and_a_right_click_does_not() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let at = nav.rect()?.center();
    let before = nav.viewport.camera;
    let picks = nav.click(PointerButton::Secondary, at, Modifiers::NONE);
    assert!(picks.is_empty());
    assert_eq!(nav.viewport.camera, before);
    let picks = nav.drag(
        PointerButton::Secondary,
        at,
        at + vec2(80.0, -30.0),
        Modifiers::NONE,
        8,
    );
    assert!(picks.is_empty(), "a right-drag selects nothing");
    let after = nav.viewport.camera;
    assert!(turned(&before, &after));
    // The whole drag counts, the way from the press included.
    assert!(
        ((after.yaw - before.yaw).abs() - 80.0 * 0.008).abs() < 1e-6,
        "{} → {}",
        before.yaw,
        after.yaw
    );
    Ok(())
}

/// Middle-drag pans; Shift+middle-drag orbits.
#[test]
fn middle_drag_pans_and_shift_middle_orbits() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let at = nav.rect()?.center();
    let before = nav.viewport.camera;
    nav.drag(
        PointerButton::Middle,
        at,
        at + vec2(-40.0, 25.0),
        Modifiers::NONE,
        5,
    );
    let panned = nav.viewport.camera;
    assert!(moved(&before, &panned) && !turned(&before, &panned));
    nav.drag(
        PointerButton::Middle,
        at,
        at + vec2(50.0, 0.0),
        Modifiers::SHIFT,
        5,
    );
    assert!(turned(&panned, &nav.viewport.camera));
    Ok(())
}

/// A click still selects; Cmd/Ctrl+drag from empty space orbits, and from a
/// part it does not.
#[test]
fn command_drag_on_empty_space_orbits_and_clicks_still_select() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let (item, on_part) = nav.package()?;
    assert_eq!(
        nav.click(PointerButton::Primary, on_part, Modifiers::NONE),
        vec![Some(item)]
    );
    let empty = nav.empty()?;
    let before = nav.viewport.camera;
    let picks = nav.drag(
        PointerButton::Primary,
        empty,
        empty + vec2(60.0, 40.0),
        Modifiers::COMMAND,
        6,
    );
    assert!(picks.is_empty(), "the orbit selects nothing: {picks:?}");
    let orbited = nav.viewport.camera;
    assert!(
        turned(&before, &orbited),
        "Cmd+drag on empty space must orbit"
    );
    let (_, on_part) = nav.package()?;
    nav.drag(
        PointerButton::Primary,
        on_part,
        on_part + vec2(60.0, 0.0),
        Modifiers::COMMAND,
        6,
    );
    assert_eq!(
        nav.viewport.camera, orbited,
        "Cmd+drag on a part does not orbit"
    );
    // Plain left drags never move the camera (the Component 3D triad owns them).
    nav.drag(
        PointerButton::Primary,
        empty,
        empty + vec2(60.0, 0.0),
        Modifiers::NONE,
        6,
    );
    assert_eq!(nav.viewport.camera, orbited);
    Ok(())
}

/// Space+drag pans, from a part too, and selects nothing.
#[test]
fn space_drag_pans() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let (_, on_part) = nav.package()?;
    let space = |pressed| Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    nav.frame(
        vec![Event::PointerMoved(on_part), space(true)],
        Modifiers::NONE,
    );
    let before = nav.viewport.camera;
    let picks = nav.drag(
        PointerButton::Primary,
        on_part,
        on_part + vec2(30.0, 30.0),
        Modifiers::NONE,
        5,
    );
    nav.frame(vec![space(false)], Modifiers::NONE);
    assert!(picks.is_empty(), "{picks:?}");
    let after = nav.viewport.camera;
    assert!(moved(&before, &after) && !turned(&before, &after));
    Ok(())
}

/// With Space held, a click (also one pressed right after the pointer
/// jumped there, or moved 2 pt before the release) still selects and never
/// moves the camera: the pan starts only once the press is a drag.
#[test]
fn a_click_with_space_held_selects_and_does_not_pan() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let (item, on_part) = nav.package()?;
    let space = Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let far = on_part + vec2(-80.0, -60.0);
    nav.frame(vec![Event::PointerMoved(far), space], Modifiers::NONE);
    let before = nav.viewport.camera;
    let press = |at| Nav::button(at, PointerButton::Primary, true, Modifiers::NONE);
    let release = |at| Nav::button(at, PointerButton::Primary, false, Modifiers::NONE);
    let mut picks = vec![];
    picks.extend(nav.frame(
        vec![Event::PointerMoved(on_part), press(on_part)],
        Modifiers::NONE,
    ));
    picks.extend(nav.frame(vec![release(on_part)], Modifiers::NONE));
    assert_eq!(picks, vec![Some(item)]);
    assert_eq!(nav.viewport.camera, before, "the jump and click panned");
    let near = on_part + vec2(2.0, 1.0);
    let mut picks = vec![];
    picks.extend(nav.frame(vec![press(on_part)], Modifiers::NONE));
    picks.extend(nav.frame(vec![Event::PointerMoved(near)], Modifiers::NONE));
    picks.extend(nav.frame(vec![release(near)], Modifiers::NONE));
    assert_eq!(picks, vec![Some(item)]);
    assert_eq!(nav.viewport.camera, before, "a 2 pt click panned");
    Ok(())
}

/// Component 3D keeps the left button for its triad: Space and Cmd/Ctrl
/// drags do not navigate there, while the trackpad and right-drag do.
#[test]
fn the_placement_view_leaves_the_left_button_to_the_triad() -> Result<()> {
    let mut nav = Nav::new(NavCaps::PLACEMENT)?;
    let empty = nav.empty()?;
    let before = nav.viewport.camera;
    nav.drag(
        PointerButton::Primary,
        empty,
        empty + vec2(60.0, 40.0),
        Modifiers::COMMAND,
        6,
    );
    assert_eq!(nav.viewport.camera, before);
    nav.frame(
        vec![Event::PointerMoved(empty), Event::Zoom(1.2)],
        Modifiers::NONE,
    );
    assert!(nav.viewport.camera.distance < before.distance);
    let zoomed = nav.viewport.camera;
    nav.drag(
        PointerButton::Secondary,
        empty,
        empty + vec2(60.0, 0.0),
        Modifiers::NONE,
        6,
    );
    assert!(turned(&zoomed, &nav.viewport.camera));
    Ok(())
}

/// The arrows turn the view 15° (Shift: 90°); Cmd/Ctrl+arrows pan it.
#[test]
fn arrow_keys_turn_and_command_arrows_pan() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let at = nav.rect()?.center();
    nav.frame(vec![Event::PointerMoved(at)], Modifiers::NONE);
    let key = |key, modifiers| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let before = nav.viewport.camera;
    nav.frame(
        vec![key(egui::Key::ArrowRight, Modifiers::NONE)],
        Modifiers::NONE,
    );
    let turned_view = nav.viewport.camera;
    let angle = before
        .back()
        .dot(turned_view.back())
        .clamp(-1.0, 1.0)
        .acos();
    assert!(
        (angle - 15f64.to_radians()).abs() < 1e-6,
        "{}",
        angle.to_degrees()
    );
    assert!(!moved(&before, &turned_view));
    nav.frame(
        vec![key(egui::Key::ArrowLeft, Modifiers::COMMAND)],
        Modifiers::COMMAND,
    );
    let panned = nav.viewport.camera;
    assert!(moved(&turned_view, &panned) && !turned(&turned_view, &panned));
    Ok(())
}

/// A middle double-click fits the view after it was panned away.
#[test]
fn a_middle_double_click_fits() -> Result<()> {
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let fitted = nav.viewport.camera;
    let at = nav.rect()?.center();
    nav.drag(
        PointerButton::Middle,
        at,
        at + vec2(200.0, 0.0),
        Modifiers::NONE,
        5,
    );
    assert!(moved(&fitted, &nav.viewport.camera));
    nav.click(PointerButton::Middle, at, Modifiers::NONE);
    nav.click(PointerButton::Middle, at, Modifiers::NONE);
    nav.frame(vec![], Modifiers::NONE);
    let refit = nav.viewport.camera;
    assert!(
        (refit.target - fitted.target).len() < 1e-6,
        "{:?}",
        refit.target
    );
    Ok(())
}

/// The fixtures above find a part, the board and empty space.
#[test]
fn the_fixture_has_a_part_and_empty_space() -> Result<()> {
    let nav = Nav::new(NavCaps::BOARD)?;
    let model = nav.viewport.model.as_ref().ok_or("no model")?;
    assert!(model.geometry.iter().any(|g| g.kind == BodyKind::Substrate));
    nav.package()?;
    nav.empty()?;
    Ok(())
}

/// The navigation cube: a click on its face looks along that face, a
/// click on the house goes home, a drag on it orbits, and none of them
/// selects what lies under the cube.
#[test]
fn the_cube_turns_the_view_to_a_face_and_drags_orbit() -> Result<()> {
    use sparksmith_solidcraft::viewport::StandardView;
    let mut nav = Nav::new(NavCaps::BOARD)?;
    let rect = nav.rect()?;
    let (cube, home) = nav_cube::layout(rect);
    // The iso view shows TOP above the centre: click there.
    let top = cube.center() - vec2(0.0, 20.0);
    assert_eq!(
        nav_cube::target(&nav.viewport.camera, rect, top),
        Some(nav_cube::Target::View(Vec3::Z))
    );
    let picks = nav.click(PointerButton::Primary, top, Modifiers::NONE);
    assert!(picks.is_empty(), "a click on the cube selects nothing");
    let mut want = nav.viewport.camera;
    want.set_view(StandardView::Top);
    assert!((nav.viewport.camera.back() - want.back()).len() < 1e-4);
    // Press and release in one frame (a fast click) works the same.
    nav.frame(vec![Event::PointerMoved(home.center())], Modifiers::NONE);
    nav.frame(
        vec![
            Nav::button(home.center(), PointerButton::Primary, true, Modifiers::NONE),
            Nav::button(
                home.center(),
                PointerButton::Primary,
                false,
                Modifiers::NONE,
            ),
        ],
        Modifiers::NONE,
    );
    let mut iso = nav.viewport.camera;
    iso.set_view(StandardView::Iso);
    assert!(
        (nav.viewport.camera.back() - iso.back()).len() < 1e-4,
        "home"
    );
    // A drag that starts on the cube orbits, and selects nothing.
    let before = nav.viewport.camera;
    let picks = nav.drag(
        PointerButton::Primary,
        cube.center(),
        cube.center() + vec2(60.0, 0.0),
        Modifiers::NONE,
        6,
    );
    assert!(picks.is_empty());
    assert!(turned(&before, &nav.viewport.camera));
    assert!(
        !moved(&before, &nav.viewport.camera),
        "it turns about the target"
    );
    Ok(())
}
