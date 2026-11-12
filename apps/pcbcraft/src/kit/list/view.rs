//! The list widget: it takes the keyboard focus, reads the list keys and
//! clicks through the pure reducer ([`super::nav`]), lays the lines out
//! either bounded inside a card ("4 of 7 · Show 3 more") or virtualised in
//! a scrolling panel ([`super::virt`]: only the shown lines plus overscan
//! are built, with a sticky group header), reorders by the grip, moves
//! rows with the FLIP motion and shows the type-ahead bubble. What a row
//! looks like is the caller's [`super::row::paint`]; what an effect does
//! (select objects, edit, delete, reorder through a command) is the
//! caller's too, from [`ListOutput::effects`].
use super::model::{Line, RowKey};
use super::nav::{self, Effect, Input};
use super::row::RowResponse;
use super::state::ListState;
use super::tokens::ListTokens;
use super::virt;
use egui::{FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// How a list sits in its container.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    /// In a card: the first `rows` lines, then "Show n more"; no inner
    /// scroll.
    Bounded { rows: usize },
    /// A panel: a virtualised scroll area `height` high.
    Scrolled { height: f32 },
}

/// A list's rules and its place.
#[derive(Clone, Copy, Debug)]
pub struct ListView<'a> {
    pub id: egui::Id,
    /// The accessible name ("Dimensions").
    pub label: &'a str,
    /// ListBox, Tree or Grid.
    pub role: egui::Role,
    pub layout: Layout,
    pub multiple: bool,
    pub reorder: bool,
    pub delete: bool,
    /// The filter has text: Esc clears it before the selection.
    pub filtered: bool,
    /// A value of the list is being edited: Esc cancels it first.
    pub editing: bool,
    /// The object hovered elsewhere, for the edge pill when its row is
    /// scrolled out of view.
    pub highlighted: Option<RowKey>,
}

impl<'a> ListView<'a> {
    pub fn new(id: egui::Id, label: &'a str, layout: Layout) -> Self {
        Self {
            id,
            label,
            role: egui::Role::ListBox,
            layout,
            multiple: true,
            reorder: false,
            delete: false,
            filtered: false,
            editing: false,
            highlighted: None,
        }
    }
}

/// How a line should be drawn this frame.
#[derive(Clone, Copy, Debug)]
pub struct LineCtx {
    /// The id rows and their parts derive from.
    pub id: egui::Id,
    pub selected: bool,
    /// Show the focus ring (keyboard navigation only).
    pub cursor: bool,
    /// The sticky copy of a group header.
    pub sticky: bool,
}

/// What the list did this frame.
#[derive(Clone, Debug, Default)]
pub struct ListOutput {
    pub effects: Vec<Effect>,
    /// The row under the pointer: the cross-highlight this list writes.
    pub hovered: Option<RowKey>,
    /// Hover actions clicked: (row, action index).
    pub actions: Vec<(RowKey, usize)>,
    pub state: ListState,
    pub focused: bool,
    /// Lines built this frame (virtualisation evidence).
    pub built: usize,
    /// Where each built row's value is drawn, for an inline editor.
    pub value_rects: Vec<(RowKey, Rect)>,
}

/// What the rows did while being drawn, applied after the pass.
#[derive(Default)]
struct Pass {
    clicks: Vec<Input>,
    folds: Vec<RowKey>,
    /// (row, index, started (else dropped), grab offset or drop y).
    grips: Vec<(RowKey, usize, bool, f32)>,
}

/// A grip drag in progress: the row and where it was picked up.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    key: RowKey,
    from: usize,
    grab: f32,
}

impl ListView<'_> {
    fn focus_id(&self) -> egui::Id {
        self.id.with("focus")
    }

    /// Give this list the keyboard.
    pub fn request_focus(&self, ctx: &egui::Context) {
        let mut state = ListState::load(ctx, self.id);
        state.keyboard_owner = true;
        state.store(ctx, self.id);
        ctx.memory_mut(|m| m.request_focus(self.focus_id()));
    }

    /// Scroll a virtualised list so line `index` shows whole on the next
    /// frame, moving as little as possible (a selection made elsewhere,
    /// such as on the canvas, brings its row into view).
    pub fn reveal(&self, ctx: &egui::Context, index: usize) {
        let Layout::Scrolled { height } = self.layout else {
            return;
        };
        let row_h = ListTokens::get(ctx).row_h;
        let (top, shown) = ctx
            .data(|d| d.get_temp::<(f32, f32)>(self.id.with("scroll")))
            .unwrap_or((0.0, height));
        let to = virt::reveal(index, row_h, top, shown);
        if (to - top).abs() > 0.5 {
            ctx.data_mut(|d| d.insert_temp(self.id.with("reveal"), to));
        }
    }

    /// Show the list over `lines`; `paint` draws one line into its rect
    /// and returns what it did.
    pub fn show(
        &self,
        ui: &mut Ui,
        lines: &[Line<'_>],
        mut paint: impl FnMut(&mut Ui, Rect, &Line<'_>, LineCtx) -> RowResponse,
    ) -> ListOutput {
        let l = ListTokens::get(ui.ctx());
        let mut state = ListState::load(ui.ctx(), self.id);
        let mut out = ListOutput::default();
        let focus_id = self.focus_id();
        // The list holds the keyboard from a click on it until a click
        // elsewhere or another widget takes the focus. egui's own focus is
        // requested too, so other key readers treat it as typing; the
        // list's own flag keeps it when a card widget drops egui's.
        let owner = ui.memory(|m| m.focused());
        if owner.is_some_and(|o| o != focus_id) {
            state.keyboard_owner = false;
        }
        let focused = ui.memory(|m| m.has_focus(focus_id)) || state.keyboard_owner;
        out.focused = focused;
        let now = ui.input(|i| i.time);
        let page = match self.layout {
            Layout::Bounded { rows } => rows,
            Layout::Scrolled { height } => ((height / l.row_h).floor() as usize).max(1),
        };
        let cx = nav::Context {
            page,
            reorder: self.reorder,
            delete: self.delete,
            multiple: self.multiple,
            editing: self.editing,
            filtered: self.filtered,
        };
        if focused && !self.editing {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    focus_id,
                    egui::EventFilter {
                        tab: false,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            for input in super::keys::inputs(ui.ctx(), now) {
                let effect = nav::reduce(&mut state, lines, input, cx);
                push(&mut out.effects, effect);
            }
        }
        let shown_cursor = focused && state.keyboard;
        let total = lines.len();
        let (count, more) = match self.layout {
            Layout::Bounded { rows } if !state.show_all && total > rows => {
                // The cursor's row stays shown.
                let at = state
                    .cursor
                    .and_then(|c| lines.iter().position(|x| x.key == c))
                    .map_or(0, |i| i + 1);
                let n = rows.max(at);
                (n.min(total), n < total)
            }
            _ => (total, false),
        };
        let width = ui.available_width();
        let drag = ui.data(|d| d.get_temp::<Drag>(self.id.with("drag")));
        let mut pass = Pass::default();
        let row_h = l.row_h;
        let selection = state.selection.clone();
        let cursor = state.cursor;
        let draw =
            |ui: &mut Ui,
             index: usize,
             top: f32,
             out: &mut ListOutput,
             pass: &mut Pass,
             paint: &mut dyn FnMut(&mut Ui, Rect, &Line<'_>, LineCtx) -> RowResponse| {
                let Some(line) = lines.get(index) else {
                    return;
                };
                let target = top + index as f32 * row_h;
                // FLIP animates the row's place in the list, not on the
                // screen: scrolling or moving the list never animates rows.
                let y = top
                    + ui.ctx().animate_value_with_time(
                        self.id.with(("y", line.key)),
                        index as f32 * row_h,
                        l.motion_flip,
                    );
                let mut y = y;
                if let Some(d) = drag
                    && d.key == line.key
                    && let Some(p) = ui.ctx().pointer_latest_pos()
                {
                    y = p.y - d.grab;
                }
                let rect = Rect::from_min_size(pos2(ui.max_rect().left(), y), vec2(width, row_h));
                let lc = LineCtx {
                    id: self.id,
                    selected: selection.contains(&line.key),
                    cursor: shown_cursor && cursor == Some(line.key),
                    sticky: false,
                };
                let r = paint(ui, rect, line, lc);
                out.built += 1;
                out.value_rects.push((line.key, r.value_rect));
                if r.hovered {
                    out.hovered = Some(line.key);
                }
                if let Some(a) = r.action {
                    out.actions.push((line.key, a));
                }
                if r.fold {
                    pass.folds.push(line.key);
                } else if r.double_clicked {
                    pass.clicks.push(Input::DoubleClick { key: line.key });
                } else if r.clicked {
                    let m = ui.input(|i| i.modifiers);
                    pass.clicks.push(Input::Click {
                        key: line.key,
                        extend: m.shift,
                        toggle: m.command,
                    });
                }
                if let Some(grip) = &r.grip {
                    if grip.drag_started() {
                        let grab = ui
                            .ctx()
                            .pointer_interact_pos()
                            .map_or(row_h / 2.0, |p| p.y - y);
                        pass.grips.push((line.key, index, true, grab));
                    } else if grip.drag_stopped() {
                        let at = ui.ctx().pointer_latest_pos().map_or(target, |p| p.y - top);
                        pass.grips.push((line.key, index, false, at));
                    }
                }
            };
        let list_rect;
        let mut top_seen = 0.0;
        let mut viewport_h = 0.0;
        match self.layout {
            Layout::Bounded { .. } => {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(width, row_h * count as f32), Sense::hover());
                list_rect = rect;
                let bg = ui.interact(rect, focus_id, Sense::click());
                container_info(&bg, self, total);
                if bg.clicked() {
                    bg.request_focus();
                    state.keyboard_owner = true;
                }
                for index in 0..count {
                    draw(ui, index, rect.top(), &mut out, &mut pass, &mut paint);
                }
                if more && super::row::more(ui, count, total) {
                    state.show_all = true;
                }
            }
            Layout::Scrolled { height } => {
                let slot = self.id.with("scroll");
                let (last_top, last_h) = ui
                    .data(|d| d.get_temp::<(f32, f32)>(slot))
                    .unwrap_or((0.0, height));
                let mut area = egui::ScrollArea::vertical()
                    .id_salt(self.id.with("area"))
                    .max_height(height)
                    .min_scrolled_height(height.min(virt::content(total, row_h)))
                    .auto_shrink([false, true]);
                // Keep the keyboard's cursor in view.
                if out
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::Selected | Effect::Moved))
                    && let Some(at) = state
                        .cursor
                        .and_then(|c| lines.iter().position(|x| x.key == c))
                {
                    let to = virt::reveal(at, row_h, last_top, last_h);
                    if (to - last_top).abs() > 0.5 {
                        area = area.vertical_scroll_offset(to);
                    }
                }
                let reveal = ui.data(|d| d.get_temp::<f32>(self.id.with("reveal")));
                if let Some(to) = reveal {
                    area = area.vertical_scroll_offset(to);
                    ui.data_mut(|d| d.remove::<f32>(self.id.with("reveal")));
                }
                let shown = area.show_viewport(ui, |ui, viewport| {
                    ui.set_height(virt::content(total, row_h));
                    let origin = ui.max_rect().top();
                    let bg = ui.interact(ui.max_rect(), focus_id, Sense::click());
                    container_info(&bg, self, total);
                    if bg.clicked() {
                        bg.request_focus();
                        state.keyboard_owner = true;
                    }
                    for index in virt::visible(total, row_h, viewport.top(), viewport.height()) {
                        draw(ui, index, origin, &mut out, &mut pass, &mut paint);
                    }
                    // The sticky group header, pushed up by the next one.
                    let headers: Vec<usize> = lines
                        .iter()
                        .enumerate()
                        .filter(|(_, x)| x.header)
                        .map(|(i, _)| i)
                        .collect();
                    if let Some(s) = virt::sticky(&headers, row_h, viewport.top())
                        && let Some(line) = lines.get(s.header)
                    {
                        let y = origin + viewport.top() + s.offset;
                        let rect =
                            Rect::from_min_size(pos2(ui.max_rect().left(), y), vec2(width, row_h));
                        let lc = LineCtx {
                            id: self.id.with("sticky"),
                            selected: false,
                            cursor: false,
                            sticky: true,
                        };
                        let mut layer =
                            ui.new_child(egui::UiBuilder::new().max_rect(rect).layer_id(
                                egui::LayerId::new(
                                    egui::Order::Middle,
                                    self.id.with("sticky-layer"),
                                ),
                            ));
                        layer.set_clip_rect(ui.clip_rect());
                        let r = paint(&mut layer, rect, line, lc);
                        if r.fold || r.clicked {
                            pass.folds.push(line.key);
                        }
                    }
                    (viewport.top(), viewport.height())
                });
                top_seen = shown.inner.0;
                viewport_h = shown.inner.1;
                ui.data_mut(|d| d.insert_temp(slot, (top_seen, viewport_h)));
                list_rect = shown.inner_rect;
            }
        }
        for key in pass.folds {
            state.cursor = Some(key);
            let effect = nav::reduce(&mut state, lines, Input::Space, cx);
            push(&mut out.effects, effect);
        }
        // A press elsewhere ends the list's hold on the keyboard.
        if ui.input(|i| i.pointer.any_pressed())
            && ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| !list_rect.contains(p))
        {
            state.keyboard_owner = false;
        }
        for input in pass.clicks {
            state.keyboard_owner = true;
            ui.memory_mut(|m| m.request_focus(focus_id));
            let effect = nav::reduce(&mut state, lines, input, cx);
            push(&mut out.effects, effect);
        }
        // Grip drags: picked up, then dropped at a new place.
        for (key, from, start, value) in pass.grips {
            if start {
                ui.data_mut(|d| {
                    d.insert_temp(
                        self.id.with("drag"),
                        Drag {
                            key,
                            from,
                            grab: value,
                        },
                    );
                });
            } else {
                ui.data_mut(|d| d.remove::<Drag>(self.id.with("drag")));
                let to = ((value / row_h).floor().max(0.0) as usize).min(total.saturating_sub(1));
                let by = to as i64 - from as i64;
                if by != 0 && self.reorder {
                    push(&mut out.effects, Effect::Reorder(key, by as i32));
                }
            }
        }
        if drag.is_some() && ui.input(|i| !i.pointer.any_down()) {
            ui.data_mut(|data| data.remove::<Drag>(self.id.with("drag")));
        }
        // The edge pill: the row hovered elsewhere is out of view.
        if let (Layout::Scrolled { .. }, Some(key)) = (self.layout, self.highlighted)
            && let Some(at) = lines.iter().position(|x| x.key == key)
        {
            let place = virt::place(at, row_h, top_seen, viewport_h);
            if place != virt::Place::Shown
                && let Some(line) = lines.get(at)
                && edge_pill(ui, &l, list_rect, line.name, place)
            {
                let to = virt::reveal(at, row_h, top_seen, viewport_h);
                ui.data_mut(|d| d.insert_temp(self.id.with("reveal"), to));
            }
        }
        if let Some(typed) = state.typing(now).filter(|_| focused) {
            bubble(ui, &l, list_rect, typed);
            ui.ctx().request_repaint();
        }
        out.state = state.clone();
        state.store(ui.ctx(), self.id);
        out
    }
}

/// Keep one Selected per frame; keep every other effect in order.
fn push(effects: &mut Vec<Effect>, effect: Effect) {
    match effect {
        Effect::None => {}
        Effect::Selected if effects.contains(&Effect::Selected) => {}
        other => effects.push(other),
    }
}

fn container_info(response: &egui::Response, view: &ListView<'_>, total: usize) {
    let label = format!("{}, {total} rows", view.label);
    response.widget_info(|| egui::WidgetInfo::labeled(view.role, true, &label));
}

/// "R14 ↓": the cross-highlighted row is above or below the viewport.
fn edge_pill(ui: &mut Ui, l: &ListTokens, rect: Rect, name: &str, place: virt::Place) -> bool {
    let arrow = if place == virt::Place::Above {
        "\u{2191}"
    } else {
        "\u{2193}"
    };
    let text = format!("{name} {arrow}");
    let g = ui
        .painter()
        .layout_no_wrap(text.clone(), FontId::proportional(l.fs_s), l.fg);
    let size = g.size() + vec2(16.0, 4.0);
    let y = if place == virt::Place::Above {
        rect.top() + 4.0
    } else {
        rect.bottom() - size.y - 4.0
    };
    let pill = Rect::from_min_size(pos2(rect.right() - size.x - 14.0, y), size);
    let response = ui.interact(
        pill,
        ui.scope_id().with(("edge-pill", name)),
        Sense::click(),
    );
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        ui.scope_id().with("edge-pill-layer"),
    ));
    painter.rect_filled(pill, size.y / 2.0, l.surface);
    painter.rect_stroke(
        pill,
        size.y / 2.0,
        Stroke::new(1.0, l.xh_line),
        egui::StrokeKind::Inside,
    );
    painter.galley(pill.center() - g.size() / 2.0, g, l.fg);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
    response.clicked()
}

/// The type-ahead buffer, at the list's bottom right.
fn bubble(ui: &Ui, l: &ListTokens, rect: Rect, typed: &str) {
    let g = ui
        .painter()
        .layout_no_wrap(typed.to_owned(), FontId::monospace(l.fs_s), l.surface);
    let size = g.size() + vec2(12.0, 4.0);
    let at = Rect::from_min_size(
        pos2(rect.right() - size.x - 8.0, rect.bottom() - size.y - 6.0),
        size,
    );
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        ui.scope_id().with("type-ahead"),
    ));
    painter.rect_filled(at, 5.0, l.fg.gamma_multiply(0.88));
    painter.galley(at.center() - g.size() / 2.0, g, l.surface);
}

/// A list's footer: the count, the shown range and the selection.
pub fn footer(ui: &mut Ui, text: &str) {
    let l = ListTokens::get(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), l.row_h - 4.0), Sense::hover());
    ui.painter().line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, l.line),
    );
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::monospace(l.fs_xs), l.fg_faint);
    ui.painter().galley(
        pos2(rect.left() + l.pad_x, rect.center().y - g.size().y / 2.0),
        g,
        l.fg_faint,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
}

/// The empty state: what will appear and how to add the first item.
pub fn empty(ui: &mut Ui, title: &str, how: &str) {
    let l = ListTokens::get(ui.ctx());
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.add_space(l.pad_x);
        ui.label(egui::RichText::new(title).size(l.fs).color(l.fg));
    });
    if !how.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(l.pad_x);
            ui.label(egui::RichText::new(how).size(l.fs_s).color(l.fg_dim));
        });
    }
    ui.add_space(6.0);
}

/// A filter that matches nothing: names the query and offers Clear filter.
pub fn no_match(ui: &mut Ui, query: &str) -> bool {
    let l = ListTokens::get(ui.ctx());
    ui.add_space(6.0);
    let mut clear = false;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(l.pad_x);
        ui.label(
            egui::RichText::new(format!("Nothing matches \u{201c}{query}\u{201d}"))
                .size(l.fs_s)
                .color(l.fg_dim),
        );
        clear = ui
            .link(egui::RichText::new("Clear filter").size(l.fs_s))
            .clicked();
    });
    ui.add_space(6.0);
    clear
}
