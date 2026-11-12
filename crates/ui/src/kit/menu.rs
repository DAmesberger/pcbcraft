//! Menus share one popup surface and one row painter. Command dispatch stays with callers.
use crate::kit::icons;
use crate::kit::{Container, Surface};
use crate::tokens::Tokens;
use egui::{Color32, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use serde::Serialize;
use serde_json::Value;

/// One menu entry: a command id (run with `params` when given, else started like a toolbar
/// click) or a `ui.*` action. `-` is a separator, `#…` a section heading.
#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub shortcut: String,
    pub enabled: bool,
    pub icon: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Item>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swatch: Option<[u8; 3]>,
    /// Why the entry is off: shown on hover, and in the control channel's report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip: Option<String>,
}

impl Item {
    /// A view action (`ui.*`).
    pub fn action(id: &str, label: &str, icon: &str) -> Item {
        Item {
            id: id.into(),
            label: label.into(),
            shortcut: String::new(),
            enabled: true,
            icon: icon.into(),
            params: None,
            children: Vec::new(),
            swatch: None,
            tip: None,
        }
    }
    pub fn sep() -> Item {
        Item {
            id: "-".into(),
            label: String::new(),
            shortcut: String::new(),
            enabled: false,
            icon: String::new(),
            params: None,
            children: Vec::new(),
            swatch: None,
            tip: None,
        }
    }
    pub fn heading(label: &str) -> Item {
        Item {
            id: format!("#{label}"),
            label: label.into(),
            shortcut: String::new(),
            enabled: false,
            icon: String::new(),
            params: None,
            children: Vec::new(),
            swatch: None,
            tip: None,
        }
    }
    pub fn is_sep(&self) -> bool {
        self.id == "-"
    }
    pub fn is_heading(&self) -> bool {
        self.id.starts_with('#')
    }
    pub fn with(mut self, params: Value) -> Item {
        self.params = Some(params);
        self
    }
    pub fn on_if(self, enabled: bool) -> Item {
        self.on(enabled)
    }
    pub fn on(mut self, enabled: bool) -> Item {
        self.enabled &= enabled;
        self
    }
    /// Off, saying why, when there is a reason.
    pub fn off_because(mut self, why: Option<String>) -> Item {
        if let Some(why) = why {
            self.enabled = false;
            self.tip = Some(why);
        }
        self
    }
    pub fn key(mut self, k: &str) -> Item {
        self.shortcut = k.into();
        self
    }
}

/// Width of list menus.
pub const LIST_W: f32 = 250.0;

pub struct Menu;

/// Seconds the pointer rests on another row before an open flyout closes: a path to the flyout
/// that crosses a neighbouring row keeps it open.
pub const SUBMENU_CLOSE_DELAY: f64 = 0.3;

impl Menu {
    pub fn context<R>(response: &Response, add: impl FnOnce(&mut Ui) -> R) -> Option<egui::InnerResponse<R>> {
        Container::new(&response.ctx, Surface::Popup).context_menu(response).frame(Self::frame()).show(add)
    }

    pub fn text<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
        Self::button_with(ui, |ui, _| super::text_button(ui, label, true), add)
    }

    pub fn button_with<R>(
        ui: &mut Ui,
        button: impl FnOnce(&mut Ui, bool) -> Response,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> egui::InnerResponse<Option<R>> {
        Self::button_at(ui, button, add, false)
    }

    fn button_at<R>(
        ui: &mut Ui,
        button: impl FnOnce(&mut Ui, bool) -> Response,
        add: impl FnOnce(&mut Ui) -> R,
        beside: bool,
    ) -> egui::InnerResponse<Option<R>> {
        let previous = ui.style().clone();
        let frame = Self::frame();
        let style = ui.style_mut();
        style.spacing.menu_margin = frame.inner_margin;
        style.visuals.menu_corner_radius = frame.corner_radius;
        style.visuals.popup_shadow = frame.shadow;
        style.visuals.window_fill = frame.fill;
        style.visuals.window_stroke = frame.stroke;
        let nested = egui::menu::is_in_menu(ui);
        let response = button(ui, nested);
        let popup_id = if nested { egui::menu::SubMenu::id_from_widget_id(response.id) } else { egui::Popup::default_response_id(&response) };
        let was_open = ui.ctx().read_response(popup_id).is_some();
        let gate_id = popup_id.with("opening_click");
        let first_click = !was_open || ui.ctx().data(|d| d.get_temp::<bool>(gate_id)).unwrap_or(true);
        let (opening_press, released) = ui.input(|i| {
            (
                first_click
                    && (i.pointer.any_down() || i.pointer.any_released())
                    && i.pointer.press_origin().or_else(|| i.pointer.interact_pos()).is_some_and(|p| response.rect.contains(p)),
                i.pointer.any_released(),
            )
        });
        let content = |ui: &mut Ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            // The opening gesture belongs to the parent even when a narrow popup overlaps it.
            ui.ctx().data_mut(|d| d.insert_temp(gate_id, first_click && !released));
            let result = ui.add_enabled_ui(!opening_press, add).inner;
            Container::finish_content(ui, frame);
            result
        };
        let popup = if nested {
            let submenu = Container::new(ui.ctx(), Surface::Popup).submenu();
            let submenu = if opening_press {
                submenu.config(egui::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            } else {
                submenu
            };
            let slot = super::container::BackingSlot::reserve(ui.ctx(), egui::LayerId::new(egui::Order::Foreground, popup_id));
            submenu.show(ui, &response, |ui| match slot {
                Some(slot) => slot.body(ui, content),
                None => content(ui),
            })
        } else if beside {
            let open = Self::flyout_open(ui, &response, popup_id);
            let popup = Container::new(ui.ctx(), Surface::Popup).menu(&response).open(open).frame(frame);
            // Last pass's size; the first pass assumes the flyout fits on the right.
            let size = popup.get_expected_size().unwrap_or(egui::Vec2::ZERO);
            let at = Self::flyout_at(ui.ctx().content_rect(), response.interact_rect, size, false);
            popup.anchor(at).align(egui::RectAlign::BOTTOM_START).align_alternatives(&[]).gap(0.0).show(content)
        } else {
            Container::new(ui.ctx(), Surface::Popup).menu(&response).frame(frame).show(content)
        };
        let response = egui::InnerResponse { response, inner: popup.map(|r| r.inner) };
        ui.set_style(previous);
        response
    }

    /// Hovering a row opens its flyout, like the context menu's. The open flyout stays while the
    /// pointer is on its row or inside it, and closes after resting elsewhere in the parent menu.
    fn flyout_open(ui: &Ui, row: &Response, popup_id: egui::Id) -> bool {
        // (open flyout, when the pointer left it, the pass that last showed it, the pointer has
        // moved since the parent menu opened)
        type State = (Option<egui::Id>, Option<f64>, u64, bool);
        let key = ui.scope_id().with("sc_flyout");
        let ctx = ui.ctx();
        let pass = ctx.cumulative_pass_nr();
        let (mut open, mut leave, seen, mut moved) = ctx.data(|d| d.get_temp::<State>(key)).unwrap_or((None, None, 0, false));
        // A parent menu that closed and opened again starts with no flyout.
        if seen + 1 < pass {
            (open, leave, moved) = (None, None, false);
        }
        // Only a menu that opened under a still pointer waits for a move; after that, any row the
        // pointer rests on shows its flyout, including once another flyout has closed.
        moved |= Self::pointer_moved(ctx);
        if open.is_none() && ((moved && row.hovered()) || row.clicked()) {
            open = Some(popup_id);
        }
        if open == Some(popup_id) {
            let pointer = ctx.pointer_hover_pos();
            let inside = pointer.is_some_and(|p| row.rect.contains(p) || ctx.read_response(popup_id).is_some_and(|r| r.rect.contains(p)));
            let elsewhere = pointer.is_some_and(|p| ui.clip_rect().contains(p)) && !inside;
            if !row.clicked() && elsewhere && (Self::pointer_moved(ctx) || leave.is_some()) {
                if Self::flyout_leaving(ctx, &mut leave) {
                    open = None;
                    // The row under the pointer may come earlier in this pass; it opens on the next.
                    ctx.request_repaint();
                }
            } else {
                leave = None;
            }
        }
        ctx.data_mut(|d| d.insert_temp(key, (open, leave, pass, moved)));
        open == Some(popup_id)
    }

    fn pointer_moved(ctx: &egui::Context) -> bool {
        ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
    }

    /// The pointer rests on another row of the parent menu since `leave`: true once the open
    /// flyout should close after [`SUBMENU_CLOSE_DELAY`] (#6657).
    pub fn flyout_leaving(ctx: &egui::Context, leave: &mut Option<f64>) -> bool {
        let now = ctx.input(|i| i.time);
        let left = SUBMENU_CLOSE_DELAY - (now - *leave.get_or_insert(now));
        if left <= 0.0 || !left.is_finite() {
            *leave = None;
            return true;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        false
    }

    /// Where a flyout of `size` sits beside its `row`: on the right with its first row level with
    /// `row`, on the left only when the right has no room (#8267), and kept on screen. A flyout of
    /// a flyout that opened to the left (`leftward`) opens to the left too while there is room, so
    /// it never covers the menu.
    pub fn flyout_at(screen: Rect, row: Rect, size: egui::Vec2, leftward: bool) -> egui::Pos2 {
        let margin = Self::frame().total_margin();
        let gap = margin.right + 2.0;
        let right = row.right() + gap;
        let left = row.left() - gap - size.x;
        let x = if right + size.x <= screen.right() && !(leftward && left >= screen.left()) { right } else { left.max(screen.left()) };
        let y = (row.top() - margin.top).min(screen.bottom() - size.y).max(screen.top());
        pos2(x, y)
    }

    pub fn frame() -> egui::Frame {
        super::Container::menu_frame()
    }

    pub fn show_frame<R>(ui: &mut Ui, frame: egui::Frame, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
        let bounds = ui.max_rect();
        Container::new(ui.ctx(), Surface::Popup).show_frame(ui, frame, bounds, add)
    }

    pub fn show<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
        Self::show_frame(ui, Self::frame(), |ui| {
            ui.set_width((width - Tokens::get().space_6 * 2.0 - 2.0).max(0.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), add).inner
        })
        .inner
    }

    pub fn separator(ui: &mut Ui) {
        let t = Tokens::get();
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.space_10 + 1.0), Sense::hover());
        ui.painter().hline(r.x_range(), r.center().y, t.tertiary_border(t.dialog_rule));
    }

    pub fn heading(ui: &mut Ui, label: &str) {
        let t = Tokens::get();
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.space_24), Sense::hover());
        let mut job = egui::text::LayoutJob::simple_singleline(
            solidcraft_ui_text::tr_label(label).to_uppercase(),
            crate::kit::type_face::semibold(t.font_small),
            t.form_dim,
        );
        for section in &mut job.sections {
            section.format.extra_letter_spacing = t.font_small * t.font_section_tracking;
        }
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(pos2(r.left() + t.space_8, r.center().y - galley.size().y / 2.0), galley, t.form_dim);
    }
}

/// Shared row states. Swatches are document colour data; chrome always uses Tokens.
pub struct MenuItem<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub icon: &'a str,
    pub shortcut: &'a str,
    pub enabled: bool,
    pub highlighted: bool,
    pub checked: Option<bool>,
    pub swatch: Option<Color32>,
    pub danger: bool,
    pub submenu: bool,
}

impl<'a> MenuItem<'a> {
    pub fn new(id: &'a str, label: &'a str, icon: &'a str) -> Self {
        Self { id, label, icon, shortcut: "", enabled: true, highlighted: false, checked: None, swatch: None, danger: false, submenu: false }
    }

    /// The row for a menu entry, `lit` when it is chosen with the keys or its flyout is open. A
    /// `current` shortcut marks the checked entry of a choice.
    pub fn from_item(item: &'a Item, lit: bool) -> Self {
        Self {
            id: &item.id,
            label: &item.label,
            icon: &item.icon,
            shortcut: if item.shortcut == "current" { "" } else { &item.shortcut },
            enabled: item.enabled,
            highlighted: lit,
            danger: item.icon == "delete",
            checked: (item.shortcut == "current").then_some(true),
            swatch: item.swatch.map(|c| Color32::from_rgb(c[0], c[1], c[2])),
            submenu: !item.children.is_empty(),
        }
    }

    pub fn show(&self, ui: &mut Ui) -> Response {
        let t = Tokens::get();
        // Areas first measure intrinsic content. Filling their provisional available
        // width would make a flipped submenu cover its parent and consume the opening click.
        let width = if ui.is_sizing_pass() {
            let label =
                ui.painter().layout_no_wrap(solidcraft_ui_text::tr_label(self.label).to_string(), FontId::proportional(t.font_label), t.form_ink);
            let shortcut = ui.painter().layout_no_wrap(self.shortcut.to_string(), FontId::monospace(t.font_small), t.form_dim);
            let trailing = if self.submenu { t.space_16 } else { 0.0 };
            (t.control_height + t.space_2 + label.size().x + shortcut.size().x + t.space_24 + trailing).min(ui.available_width())
        } else {
            ui.available_width()
        };
        let (r, response) =
            ui.allocate_exact_size(vec2(width, t.control_height + t.space_2), if self.enabled { Sense::click() } else { Sense::hover() });
        self.paint(ui, r, &response, 0.0);
        response
    }

    /// Ribbon rows retain their own drag/focus IDs and reserve a trailing pin target.
    pub fn paint(&self, ui: &Ui, r: Rect, response: &Response, trailing: f32) {
        let t = Tokens::get();
        let enabled = self.enabled && ui.is_enabled();
        let hot = enabled && (self.highlighted || response.hovered() || response.has_focus());
        let ink = if self.danger {
            t.form_error
        } else if hot {
            t.form_primary
        } else {
            t.form_ink
        };
        let dim = if self.danger {
            t.form_error
        } else if hot {
            t.form_primary
        } else {
            t.form_dim
        };
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(r));
        let mut painter = painter;
        if !enabled {
            painter.multiply_opacity(0.4);
        }
        if hot {
            painter.rect_filled(r, t.control_radius, t.accent_soft);
        }
        let icon = Rect::from_center_size(pos2(r.left() + t.control_height / 2.0, r.center().y), vec2(t.space_16, t.space_16));
        if let Some(on) = self.checked {
            if on {
                super::checks::paint_check_glyph(&painter, icon, false, t.form_primary);
            }
        } else if let Some(color) = self.swatch {
            painter.rect(icon, t.radius_sm, color, t.control_border_stroke(t.control_border), egui::StrokeKind::Inside);
        } else if !self.icon.is_empty() {
            if self.danger {
                icons::paint_danger(&painter, icon, self.icon, 1.0);
            } else if hot {
                icons::paint_ink(&painter, icon, self.icon, dim);
            } else {
                icons::paint_icon(&painter, icon, self.icon, 1.0);
            }
        }
        let label = solidcraft_ui_text::tr_label(self.label);
        let mut right = r.right() - t.space_8 - trailing;
        if self.submenu {
            let c = pos2(right - t.space_4, r.center().y);
            painter.add(egui::Shape::line(
                vec![c + vec2(-t.space_2, -t.space_4), c + vec2(t.space_2, 0.0), c + vec2(-t.space_2, t.space_4)],
                Stroke::new(1.0, dim),
            ));
            right -= t.space_16;
        }
        if !self.shortcut.is_empty() {
            let key = painter.layout_no_wrap(solidcraft_ui_text::tr_label(self.shortcut).to_string(), FontId::monospace(t.font_small), dim);
            painter.galley(pos2(right - key.size().x, r.center().y - key.size().y / 2.0), key.clone(), dim);
            right -= key.size().x + t.space_8;
        }
        let left = r.left() + t.control_height + t.space_2;
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), FontId::proportional(t.font_label), ink);
        job.wrap.max_width = (right - left).max(0.0);
        job.wrap.max_rows = 1;
        let galley = painter.layout_job(job);
        painter.galley(pos2(left, r.center().y - galley.size().y / 2.0), galley, ink);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
        solidcraft_ui_registry::publish_target(&format!("menu:{}", self.id), r);
        if enabled && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
}

/// A list menu's popup surface, [`LIST_W`] wide, around `add`.
pub fn menu_frame<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Menu::show(ui, LIST_W, add)
}

/// One entry of a list menu: a separator, a heading or a row. Returns the entry when its row was
/// clicked while enabled.
pub fn list_row(ui: &mut Ui, item: &Item) -> Option<Item> {
    list_row_lit(ui, item, false)
}

/// [`list_row`] with the row lit (chosen with the keys, or its flyout open).
pub fn list_row_lit(ui: &mut Ui, item: &Item, lit: bool) -> Option<Item> {
    if item.is_sep() {
        Menu::separator(ui);
        return None;
    }
    if item.is_heading() {
        Menu::heading(ui, &item.label);
        return None;
    }
    let row = MenuItem::from_item(item, lit);
    (row.show(ui).clicked() && item.enabled).then(|| item.clone())
}

/// How the workspace menu shows one of the host's workspaces: the slug of its handle
/// (`workspace:menu:<slug>`), the message ids of its description and of its tab list (`None`: a
/// planned workspace, not yet available) and its accent ramp (`None`: the main accent, tinted).
#[derive(Clone, Copy)]
pub struct WorkspaceStyle {
    pub slug: &'static str,
    pub description: &'static str,
    pub tabs: Option<&'static str>,
    pub ramp: Option<crate::tokens::AccentRamp>,
}

/// The host's workspace menu: its entries, how each is shown (an entry without a style is not
/// listed) and the footer note under them.
pub struct WorkspaceMenu<'a> {
    pub items: &'a [Item],
    pub style: &'a dyn Fn(&Item) -> Option<WorkspaceStyle>,
    pub footer: &'a str,
}
