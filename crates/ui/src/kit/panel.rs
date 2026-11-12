//! Panel elements for dense cards; see `docs/ui-kit.md` for tokens and usage.
use super::{actions::icon_button_at, metrics::*, text::paint_text};
use crate::kit::BackingTooltipExt as _;
use crate::tokens::Tokens;
use egui::{Ui, vec2};

const WINDOW_PAGES_NARROW_WIDTH: f32 = 150.0;
const WINDOW_PAGES_NARROW_AT: f32 = 700.0;

/// Shared active-tab hover tint for the browser and collapsible panel chrome.
pub fn panel_header_hover(painter: &egui::Painter, rect: egui::Rect) {
    let t = Tokens::get();
    painter.rect_filled(rect, t.radius_lg.saturating_sub(t.space_4 as u8), crate::tokens::TabAccent::current(painter.ctx()).soft);
}

fn header_impl(
    ui: &mut Ui,
    icon: (&str, Option<egui::Color32>),
    title: &str,
    collapsed: Option<&mut bool>,
    closeable: bool,
    on_close: impl FnOnce(),
) {
    header_actions(ui, icon, title, collapsed, closeable, on_close, None);
}

fn header_actions(
    ui: &mut Ui,
    icon: (&str, Option<egui::Color32>),
    title: &str,
    collapsed: Option<&mut bool>,
    closeable: bool,
    on_close: impl FnOnce(),
    actions: Option<(f32, &mut dyn FnMut(&mut Ui))>,
) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), header_height()), egui::Sense::hover());
    // Shared card chrome: 12 pt left inset, 19 pt icon, 8 pt gap and 26 pt close target.
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + t.space_12 + t.form_icon_size / 2.0, rect.center().y),
        vec2(t.form_icon_size, t.form_icon_size),
    );
    if icon.0.is_empty() {
        ui.painter().circle_filled(icon_rect.center(), t.space_3, crate::tokens::TabAccent::current(ui.ctx()).ink);
    } else if let Some(ink) = icon.1 {
        crate::kit::icons::paint_ink(ui.painter(), icon_rect, icon.0, ink);
    } else {
        crate::kit::icons::paint_icon(ui.painter(), icon_rect, icon.0, 1.0);
    }
    let close_rect = egui::Rect::from_center_size(
        egui::pos2((rect.right() - t.space_8 - t.form_close_size / 2.0).max(rect.left()), rect.center().y),
        vec2(t.form_close_size, t.form_close_size),
    );
    if closeable {
        let close = icon_button_at(ui, close_rect, "close", "Close");
        solidcraft_ui_registry::publish_handle("dialog:close", close.rect.center());
        if close.clicked() {
            on_close();
        }
    }
    let right = if let Some((width, actions)) = actions {
        let end = if closeable { close_rect.left() - t.space_8 } else { rect.right() - t.space_8 };
        let width = if closeable { width.min((end - icon_rect.right() - t.space_24 * 4.0).max(0.0)) } else { width };
        let action_rect = egui::Rect::from_center_size(
            egui::pos2(end - width / 2.0, rect.center().y),
            vec2(width, if closeable { t.form_close_size } else { t.control_height }),
        );
        let builder = egui::UiBuilder::new().max_rect(action_rect);
        let builder = if closeable { builder.layout(egui::Layout::left_to_right(egui::Align::Center)) } else { builder };
        let mut action_ui = ui.new_child(builder);
        if closeable {
            action_ui.set_clip_rect(ui.clip_rect().intersect(action_rect));
        }
        actions(&mut action_ui);
        solidcraft_ui_registry::publish_rect("dialog:header:accessory", action_rect);
        action_rect.left() - t.space_8
    } else if let Some(collapsed) = collapsed {
        let fold = egui::Rect::from_min_size(egui::pos2(close_rect.left() - t.form_close_size - t.space_4, close_rect.top()), close_rect.size());
        let response =
            super::disclosure_button_at(ui, fold, !*collapsed, solidcraft_ui_text::tr_label(if *collapsed { "Expand" } else { "Collapse" }));
        solidcraft_ui_registry::publish_handle("dialog:collapse", response.rect.center());
        if response.clicked() {
            *collapsed = !*collapsed;
        }
        fold.left()
    } else if closeable {
        close_rect.left()
    } else {
        rect.right() - t.space_8
    };
    let left = (icon_rect.right() + t.space_8).min(right);
    let title_rect = egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(right.max(left), rect.bottom()));
    paint_text(
        ui,
        title_rect,
        title,
        crate::kit::type_face::semibold(if icon.0.is_empty() { t.font_label } else { t.font_heading }),
        t.form_ink,
        false,
    );
    let title_response = ui.interact(title_rect, ui.make_persistent_id("title_text"), egui::Sense::hover());
    title_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), title));
    solidcraft_ui_registry::publish_rect("dialog:header", rect);
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
}

/// Where a feature panel's OK, Cancel and Apply sit (Preferences › Interface › Dialog buttons,
/// #6708). The app publishes the preference each frame, so every `Panel::feature` follows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogButtons {
    /// In a footer row under the inputs (how it always was). Also what a value this version
    /// doesn't know reads as (`prefs::lenient`).
    #[default]
    Bottom,
    /// In the header row, right of the title, where they stay while sections open and close.
    Top,
}

impl DialogButtons {}

/// The header row of a feature panel whose actions sit at the top. It is laid out before the
/// body, and its icon, title and buttons are drawn after it, once the compute status and whether
/// OK is enabled are known. Same tokens as `header`, plus `HeaderActions`'.
pub struct TopHeader<'a> {
    icon: egui::Rect,
    title: egui::Rect,
    actions: super::HeaderActions<'a>,
}

impl<'a> TopHeader<'a> {
    pub fn new(ui: &mut Ui, ok: &'a str, apply: Option<&'a str>, cancel: &'a str, collapsed: Option<&mut bool>, close: &mut bool) -> Self {
        let t = Tokens::get();
        let (band, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), header_height()), egui::Sense::hover());
        let icon = egui::Rect::from_center_size(
            egui::pos2(band.left() + t.space_12 + t.form_icon_size / 2.0, band.center().y),
            vec2(t.form_icon_size, t.form_icon_size),
        );
        // The buttons end where the fields below do.
        let end = (band.right() - t.space_12).max(band.left());
        let actions = super::HeaderActions::new(ui, band, end, ok, apply, cancel);
        let buttons = actions.left(end);
        // Nothing placed yet beside the buttons: the next control keeps clear of them.
        let beside_buttons = |right: f32| buttons < end && right >= buttons;
        // An icon target to the left of what is already placed, `space_4` clear of the buttons.
        let target = |right: f32| {
            let right = if beside_buttons(right) { right - t.space_4 } else { right };
            egui::Rect::from_center_size(
                egui::pos2((right - t.form_close_size / 2.0).max(band.left()), band.center().y),
                vec2(t.form_close_size, t.form_close_size),
            )
        };
        let mut right = buttons;
        // The header's close does what Cancel does in every panel, so with Cancel up here it is
        // left out: one control for one action, and its width goes to the title. Its handle
        // moves to Cancel, so automation that closes a panel keeps working.
        if let Some(cancel_rect) = actions.rect(super::Action::Cancel) {
            solidcraft_ui_registry::publish_handle("dialog:close", cancel_rect.center());
        } else {
            let close_rect = target(right);
            let response = icon_button_at(ui, close_rect, "close", "Close");
            solidcraft_ui_registry::publish_handle("dialog:close", response.rect.center());
            if response.clicked() {
                *close = true;
            }
            right = close_rect.left();
        }
        if let Some(collapsed) = collapsed {
            let fold = target(right);
            let response =
                super::disclosure_button_at(ui, fold, !*collapsed, solidcraft_ui_text::tr_label(if *collapsed { "Expand" } else { "Collapse" }));
            solidcraft_ui_registry::publish_handle("dialog:collapse", response.rect.center());
            if response.clicked() {
                *collapsed = !*collapsed;
            }
            right = fold.left();
        }
        // Text next to a text button keeps a gap; an icon target carries its own padding.
        if beside_buttons(right) {
            right = (right - t.space_6).max(band.left());
        }
        let left = (icon.right() + t.space_8).min(right);
        let title = egui::Rect::from_min_max(egui::pos2(left, band.top()), egui::pos2(right.max(left), band.bottom()));
        // Dragging the panel by its title must not start on a button.
        solidcraft_ui_registry::publish_handle("dialog:title", title.center());
        solidcraft_ui_registry::publish_rect("dialog:header", band);
        t.secondary_hline(ui.painter(), band.x_range(), band.bottom(), t.dialog_rule);
        Self { icon, title, actions }
    }

    /// The icon (the compute spinner while busy), the title (the compute status while there is
    /// one: the footer that showed it is gone) and the buttons. Returns the action asked for.
    pub fn finish(&self, ui: &mut Ui, icon: &str, title: &str, state: &ComputeStatus, ok_enabled: bool) -> super::Action {
        let t = Tokens::get();
        if state.busy {
            egui::Spinner::new().size(t.space_14).paint_at(ui, self.icon);
            let status = state.text.as_deref().unwrap_or_else(|| solidcraft_ui_text::tr_label("Updating the preview"));
            ui.interact(self.icon, ui.scope_id().with("compute_status"), egui::Sense::hover()).backing_on_hover_text(status);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            crate::kit::icons::paint_icon(ui.painter(), self.icon, icon, 1.0);
        }
        let heading = crate::kit::type_face::semibold(t.font_heading);
        match state.text.as_deref() {
            Some(status) => paint_text(ui, self.title, status, crate::kit::type_face::medium(t.font_label), t.form_dim, false),
            None => paint_text(ui, self.title, title, heading.clone(), t.form_ink, false),
        }
        let title_response = ui.interact(self.title, ui.make_persistent_id("title_text"), egui::Sense::hover());
        title_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), title));
        // The buttons take the row's width first; a title that no longer fits says itself on hover.
        if ui.painter().layout_no_wrap(title.to_owned(), heading, t.form_ink).size().x > self.title.width() {
            title_response.backing_on_hover_text(title);
        }
        self.actions.show(ui, ok_enabled)
    }
}

/// Selection editor chrome, shared by floating Inspectors. Returns an action for registry dispatch.
#[derive(Clone, Copy)]
pub enum InspectorAction {
    Close,
    Remove,
}

/// The host's names for an inspector header's controls: the handle its close button publishes,
/// and the id and label of the "…" menu's remove entry.
#[derive(Clone, Copy)]
pub struct InspectorNames<'a> {
    pub close: &'a str,
    pub remove: &'a str,
    pub remove_label: &'a str,
}

/// A bounded docked or floating card. Uses form_card_width and the caller's tokenised frame.
/// Window IDs and content scopes are retained so moving a card does not reset field focus.
pub struct Panel<'a> {
    pub title: &'a str,
    pub id: egui::Id,
    pub anchor: egui::Pos2,
    pub viewport: egui::Rect,
    pub side: crate::kit::frame::Side,
    pub frame: egui::Frame,
    pub width: f32,
    pub body_rows: Option<f32>,
    pub body_height: Option<f32>,
    pub ok: &'a str,
    pub cancel: &'a str,
    pub reset_scroll: bool,
    pub joined: Option<super::DockEdges>,
    pub grow_key: Option<egui::Id>,
}

pub struct WorkspaceOptions<'a> {
    pub id: &'static str,
    pub title: &'a str,
    pub icon: &'a str,
    pub bounds: egui::Rect,
    pub fill_height: bool,
    /// Use the minimum dock width without overwriting its remembered width.
    pub compact: bool,
    pub editor: bool,
    pub enabled: bool,
    pub header_actions: Option<&'a mut dyn FnMut(&mut Ui)>,
    /// The host's command id of the grip menu's entry that moves the dock to the other side.
    pub move_command: &'a str,
}

pub struct FeatureOptions<'a> {
    pub icon: &'a str,
    pub title: &'a str,
    /// The dock offset the title band drags; empty for a card that stays in its place.
    pub drag_key: &'a str,
    pub moved: egui::Vec2,
    pub docked: egui::Pos2,
    pub apply_label: Option<&'a str>,
}

/// State read after rendering the fields, just as in the original feature card.
pub struct ComputeStatus {
    pub text: Option<String>,
    pub busy: bool,
    pub applying: bool,
}

pub enum Part {
    Header,
    Body,
    Footer,
}

impl<'a> Panel<'a> {
    /// Shared floating card surface for panels, windows and modal confirmations.
    pub(super) fn card_frame(ctx: &egui::Context) -> egui::Frame {
        let t = Tokens::get();
        super::Container::new(ctx, super::Surface::Window).frame(ctx, t.dialog_body, crate::kit::frame::Side::Right).inner_margin(0.0)
    }

    pub fn new(title: &'a str, id: egui::Id, anchor: egui::Pos2, viewport: egui::Rect, side: crate::kit::frame::Side, frame: egui::Frame) -> Self {
        Self {
            title,
            id,
            anchor,
            viewport,
            side,
            frame,
            width: Tokens::get().form_card_width,
            body_rows: None,
            body_height: None,
            ok: "OK",
            cancel: "Cancel",
            reset_scroll: false,
            joined: None,
            grow_key: None,
        }
    }

    /// The footer's primary and closing actions when they are not OK and Cancel. An empty `ok`
    /// leaves the primary action out (a card that only closes, e.g. Measure).
    pub fn actions(mut self, ok: &'a str, cancel: &'a str) -> Self {
        (self.ok, self.cancel) = (ok, cancel);
        self
    }
}

/// System window built on Panel's shared floating card: 40 pt header, bounded content,
/// optional page list, and 48 pt actions with a left slot. Existing IDs and editor policies
/// survive the migration. All geometry and colours derive from Tokens.
pub struct Window<'a> {
    id: egui::Id,
    icon: &'a str,
    title: &'a str,
    anchor: Option<(egui::Align2, egui::Vec2)>,
    placed: Option<(egui::Align2, egui::Pos2)>,
    ok: &'a str,
    cancel: &'a str,
    apply: Option<&'a str>,
    nothing_to_apply: bool,
    enter_submits: bool,
    escape_closes: bool,
    closeable: bool,
    width: f32,
    body_padding: bool,
    page_rows: f32,
    page_padding: egui::Margin,
    page_chrome: bool,
    left_slot: bool,
    page_header_in_titlebar: bool,
}

impl<'a> Window<'a> {
    /// A card with OK and Cancel, centred and movable.
    pub fn new(id: egui::Id, icon: &'a str, title: &'a str) -> Self {
        Self {
            id,
            icon,
            title,
            anchor: None,
            placed: None,
            ok: "OK",
            cancel: "Cancel",
            apply: None,
            nothing_to_apply: false,
            enter_submits: true,
            escape_closes: true,
            closeable: true,
            width: Tokens::get().form_card_width * 1.5,
            body_padding: true,
            page_rows: 16.0,
            page_padding: egui::Margin::same(Tokens::get().space_16 as i8),
            page_chrome: false,
            left_slot: false,
            page_header_in_titlebar: false,
        }
    }

    /// Pin the card at `align` of the screen, `offset` away (it then no longer moves).
    pub fn anchor(mut self, align: egui::Align2, offset: egui::Vec2) -> Self {
        self.anchor = Some((align, offset));
        self
    }

    /// Start with `pivot` of the card at `pos`; the card can then be moved.
    pub fn placed(mut self, pivot: egui::Align2, pos: egui::Pos2) -> Self {
        self.placed = Some((pivot, pos));
        self
    }

    /// The primary and closing actions, and an optional third one between them. An empty `ok`
    /// leaves the primary action out.
    pub fn actions(mut self, ok: &'a str, cancel: &'a str, apply: Option<&'a str>) -> Self {
        (self.ok, self.cancel, self.apply) = (ok, cancel, apply);
        self
    }

    /// Whether the third action has anything to do: for an Apply that keeps the window open,
    /// whether anything changed since it opened or since the last Apply (#8561). Without it
    /// the action is disabled for the pointer, the keyboard and accessibility.
    pub fn apply_enabled(mut self, enabled: bool) -> Self {
        self.nothing_to_apply = !enabled;
        self
    }

    /// A card of editable fields: Enter belongs to the fields and never submits (confirmations
    /// let Enter give the primary action when nothing has focus).
    pub fn fields(mut self) -> Self {
        self.enter_submits = false;
        self
    }

    /// Esc does not close the card (the header's close and the closing action still do): for a
    /// card holding a long text the user is editing, where Esc must not discard the work.
    pub fn keep_open_on_escape(mut self) -> Self {
        self.escape_closes = false;
        self
    }

    /// `cards` times the card width instead of one and a half, for wide footers.
    pub fn cards(mut self, cards: f32) -> Self {
        if cards.is_finite() && cards > 0.0 {
            self.width = Tokens::get().form_card_width * cards.min(16.0);
        }
        self
    }

    /// Maximum page-body height in control rows, still bounded by the viewport.
    pub fn page_rows(mut self, rows: f32) -> Self {
        if rows.is_finite() {
            self.page_rows = rows.clamp(1.0, 32.0);
        }
        self
    }

    /// Content inset for a page-based window, independent of its navigation rail.
    pub fn page_padding(mut self, padding: egui::Margin) -> Self {
        self.page_padding =
            egui::Margin { left: padding.left.max(0), right: padding.right.max(0), top: padding.top.max(0), bottom: padding.bottom.max(0) };
        self
    }

    /// Place the existing page-header callback beside the title; other windows retain their body header.
    pub fn page_header_in_titlebar(mut self) -> Self {
        self.page_header_in_titlebar = true;
        self
    }

    /// A required choice, such as web recovery, has no dismiss action. Existing data remains
    /// pending until the body explicitly handles a choice.
    pub fn required(mut self) -> Self {
        self.closeable = false;
        self.escape_closes = false;
        self
    }

    /// A standard closable window for platform content and reports. Uses the same header,
    /// bounded scrolling body and fixed outer chrome as every other system window.
    /// Returns whether the shared header's close action was clicked.
    pub fn show_body(self, ctx: &egui::Context, body: impl FnOnce(&mut Ui)) -> bool {
        self.fields().actions("", "", None).show(ctx, true, body).0
    }

    /// Show a temporary status window with no header dismissal or dialog shortcuts.
    pub fn show_passive_body(mut self, ctx: &egui::Context, body: impl FnOnce(&mut Ui)) {
        self.closeable = false;
        // No footer handles either key, so leave both in the shared input queue.
        self.enter_submits = true;
        self.escape_closes = true;
        self.actions("", "", None).show(ctx, true, body);
    }

    /// Draw shared header, content and optional footer, retaining the caller's IDs.
    pub fn show(self, ctx: &egui::Context, valid: bool, body: impl FnOnce(&mut Ui)) -> (bool, super::Action) {
        self.surface(ctx, false, |ui| self.content(ui, valid, body, |_| {}))
    }

    /// A pinned left action slot, independent of the right-aligned footer buttons.
    pub fn show_with_left(
        mut self,
        ctx: &egui::Context,
        valid: bool,
        body: impl FnOnce(&mut Ui),
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        self.left_slot = true;
        self.surface(ctx, false, |ui| self.content(ui, valid, body, left))
    }

    pub fn content(&self, ui: &mut Ui, valid: bool, body: impl FnOnce(&mut Ui), left: impl FnOnce(&mut Ui)) -> (bool, super::Action) {
        super::scope(ui, |ui| {
            let t = Tokens::get();
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut close = false;
            if !self.page_header_in_titlebar {
                header_impl(ui, (self.icon, self.page_chrome.then_some(t.form_primary)), self.title, None, self.closeable, || close = true);
            }
            egui::Frame::NONE.inner_margin(if self.body_padding { t.space_12 } else { 0.0 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = t.form_gap_y;
                if self.body_padding {
                    let height =
                        (dimension(ui.ctx().content_rect().height()) - t.form_header_height - t.form_footer_height - t.space_24 * 2.0).max(0.0);
                    // Offer the body all the room the screen leaves, not just the height the card
                    // had last frame: a card first laid out on a tiny page (a browser canvas not
                    // sized yet) would otherwise keep a body of no height, its buttons hidden (#9472).
                    egui::ScrollArea::vertical()
                        .id_salt("window_body")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, true])
                        .show(ui, body);
                } else {
                    body(ui);
                }
            });
            if !self.enter_submits {
                ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            }
            if !self.escape_closes {
                ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            }
            let action = if self.ok.is_empty() && self.cancel.is_empty() && self.apply.is_none() && !self.left_slot {
                super::Action::None
            } else {
                super::actions::footer_with_left(
                    ui,
                    self.ok,
                    self.cancel,
                    valid,
                    super::Footer { apply: self.apply, nothing_to_apply: self.nothing_to_apply, ..Default::default() },
                    left,
                )
            };
            (close, action)
        })
    }

    /// Page navigation with a strip above the page body (Preferences search), a hook for a page
    /// tab's click and an optional block under the list.
    pub fn show_pages_with_header<S>(
        self,
        ctx: &egui::Context,
        selected: &mut usize,
        pages: &[WindowPage<'_, S>],
        state: &mut S,
        hooks: &PageListHooks<'_, S>,
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        self.show_pages_inner(ctx, selected, pages, state, Some(hooks), left)
    }

    fn show_pages_inner<S>(
        mut self,
        ctx: &egui::Context,
        selected: &mut usize,
        pages: &[WindowPage<'_, S>],
        state: &mut S,
        hooks: Option<&PageListHooks<'_, S>>,
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        self.body_padding = false;
        self.page_chrome = true;
        let rows = self.page_rows;
        let padding = self.page_padding;
        let in_titlebar = self.page_header_in_titlebar;
        let (icon, title, closeable) = (self.icon, self.title, self.closeable);
        let header = hooks.map(|hooks| hooks.header);
        let header_closed = std::cell::Cell::new(false);
        let result = self.show_with_left(
            ctx,
            true,
            |ui| {
                if in_titlebar {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut accessory = |ui: &mut Ui| {
                        if let Some(header) = header {
                            header(ui, state);
                        }
                    };
                    header_actions(
                        ui,
                        (icon, Some(Tokens::get().form_primary)),
                        title,
                        None,
                        closeable,
                        || header_closed.set(true),
                        Some((Tokens::get().space_24 * 9.0 + Tokens::get().space_14, &mut accessory)),
                    );
                }
                page_list_sized(ui, selected, pages, state, rows, padding, if in_titlebar { None } else { header }, hooks, header.is_some());
            },
            left,
        );
        (result.0 || header_closed.get(), result.1)
    }

    /// The same bounded card frame, either movable or an input-blocking modal.
    pub(super) fn surface(&self, ctx: &egui::Context, modal: bool, add: impl FnOnce(&mut Ui) -> (bool, super::Action)) -> (bool, super::Action) {
        let frame = if self.page_chrome {
            // A system window covers what is behind it, like every other window (#9078).
            super::navigation_popup_frame().inner_margin(0).fill(Tokens::get().dialog_body)
        } else {
            Panel::card_frame(ctx)
        };
        let inner_width = (self.width.min(dimension(ctx.content_rect().width())) - frame.total_margin().sum().x).max(0.0);
        if modal {
            let shown = super::Container::new(ctx, super::Surface::Window)
                .modal(self.id)
                .backdrop_color(Tokens::get().tab_on_fill_dark.gamma_multiply(0.35))
                .frame(frame)
                .show(ctx, |ui| {
                    ui.set_width(inner_width);
                    let result = add(ui);
                    super::Container::finish_content(ui, frame);
                    result
                });
            solidcraft_ui_registry::publish_rect("dialog:window", shown.response.rect);
            return shown.inner;
        }
        let window = super::Container::new(ctx, super::Surface::Window)
            .window(self.title)
            .id(self.id)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .min_width(inner_width)
            .max_width(inner_width)
            .default_width(inner_width)
            .scroll(false)
            .frame(frame)
            .fade_in(false)
            .fade_out(false);
        let window = match (self.anchor, self.placed) {
            (Some((align, offset)), _) => window.anchor(align, offset),
            (None, Some((pivot, pos))) => window.pivot(pivot).default_pos(pos),
            (None, None) => window.pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center() - vec2(0.0, row_height() * 2.5)),
        };
        let shown = super::container::FeatureWindow::new(window, self.id, frame).show(ctx, |ui| {
            let result = add(ui);
            super::Container::finish_content(ui, frame);
            result
        });
        if let Some(shown) = &shown {
            solidcraft_ui_registry::publish_rect("dialog:window", shown.response.rect);
        }
        shown.and_then(|r| r.inner).unwrap_or((false, super::Action::None))
    }
}

/// A page's icon, translated label key, stable handle and draw callback.
pub struct WindowPage<'a, S> {
    pub icon: super::Icon<'a>,
    pub label: &'a str,
    pub group: Option<&'a str>,
    pub handle: &'a str,
    pub scroll: bool,
    pub draw: &'a dyn Fn(&mut Ui, &mut S),
}

/// What a page list draws besides its pages: a strip above the page body (Preferences search),
/// what runs when a page tab is clicked (with that page's index, before it is drawn), and a block
/// pinned under the list with its height.
pub struct PageListHooks<'a, S> {
    pub header: &'a dyn Fn(&mut Ui, &mut S),
    pub on_select: &'a dyn Fn(&mut S, usize),
    pub sidebar_footer: Option<(f32, &'a dyn Fn(&mut Ui, &mut S))>,
}

fn page_list_sized<S>(
    ui: &mut Ui,
    selected: &mut usize,
    pages: &[WindowPage<'_, S>],
    state: &mut S,
    rows: f32,
    padding: egui::Margin,
    header: Option<&dyn Fn(&mut Ui, &mut S)>,
    hooks: Option<&PageListHooks<'_, S>>,
    search_scroll: bool,
) {
    let on_select = hooks.map(|hooks| hooks.on_select);
    let sidebar_footer = hooks.and_then(|hooks| hooks.sidebar_footer);
    let t = Tokens::get();
    // The search strip, the gap under it, and the following item spacing sit above the page.
    // They must not eat the rows the page already had, or the graphics checkboxes fall out of view.
    let header_band = if header.is_some() { t.control_height + t.form_gap_y + t.space_8 } else { 0.0 };
    let height = (dimension(ui.ctx().content_rect().height()) - t.form_header_height - t.form_footer_height - t.space_24 * 3.0)
        .clamp(0.0, t.control_height * rows + header_band);
    let (body, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), height), egui::Sense::hover());
    let width = if body.width() + 2.0 * super::NAV_BORDER < WINDOW_PAGES_NARROW_AT {
        WINDOW_PAGES_NARROW_WIDTH
    } else {
        t.form_label_width * 2.0 + t.space_24 * 2.0
    }
    .min(body.width());
    let nav = egui::Rect::from_min_size(body.min, vec2(width, body.height()));
    ui.painter().rect_filled(nav, 0.0, t.dialog_footer);
    t.secondary_vline(ui.painter(), nav.right(), nav.y_range(), t.dialog_rule);
    solidcraft_ui_registry::publish_rect("dialog:pages", nav);
    *selected = (*selected).min(pages.len().saturating_sub(1));
    let nav_inner = if sidebar_footer.is_some() {
        egui::Rect::from_min_max(nav.min + vec2(t.space_8, t.space_6), nav.max - vec2(t.space_8, t.space_10))
    } else {
        nav.shrink2(vec2(t.space_8, t.space_6))
    };
    let footer_height = sidebar_footer.map_or(0.0, |(height, _)| dimension(height).min(nav_inner.height()));
    let list = egui::Rect::from_min_max(nav_inner.min, egui::pos2(nav_inner.right(), nav_inner.bottom() - footer_height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(nav));
        ui.spacing_mut().item_spacing.y = t.space_2;
        let selection_id = ui.scope_id().with("page_selection");
        let changed = ui.data_mut(|data| {
            let previous = data.get_temp::<usize>(selection_id);
            data.insert_temp(selection_id, *selected);
            previous != Some(*selected)
        });
        egui::ScrollArea::vertical().id_salt("window_pages").max_height(list.height()).show(ui, |ui| {
            for (index, page) in pages.iter().enumerate() {
                if let Some(group) = page.group {
                    ui.add_space(t.space_8);
                    let (rect, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), t.space_24), egui::Sense::hover());
                    let rect = rect.shrink2(vec2(t.space_10, 0.0));
                    paint_text(
                        ui,
                        rect,
                        &solidcraft_ui_text::tr(group).to_uppercase(),
                        crate::kit::type_face::semibold(t.font_small),
                        t.form_dim,
                        true,
                    );
                }
                let label = solidcraft_ui_text::tr(page.label);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(dimension(ui.available_width()), t.control_height + t.space_4), egui::Sense::click());
                if response.clicked() {
                    *selected = index;
                    if let Some(on_select) = on_select {
                        on_select(state, index);
                    }
                }
                let on = *selected == index;
                if on && changed && sidebar_footer.is_some() && !list.contains_rect(rect) {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if on || response.hovered() || response.has_focus() {
                    ui.painter().rect_filled(rect, t.control_radius, if on { t.selected_bg } else { t.hover });
                }
                let ink = if on { t.form_primary } else { t.form_ink };
                let icon =
                    egui::Rect::from_center_size(egui::pos2(rect.left() + t.space_10 + t.space_8, rect.center().y), vec2(t.space_16, t.space_16));
                match page.icon {
                    super::Icon::Kit(name) => crate::kit::icons::paint_ink(ui.painter(), icon, name, if on { ink } else { t.form_dim }),
                    host => host.paint(ui.painter(), icon, 1.0),
                }
                let text_rect = egui::Rect::from_min_max(egui::pos2(icon.right() + super::NAV_ROW_GAP, rect.top()), rect.right_bottom());
                paint_text(
                    ui,
                    text_rect,
                    label,
                    if on { crate::kit::type_face::semibold(t.font_label) } else { egui::FontId::proportional(t.font_label) },
                    ink,
                    false,
                );
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, label));
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                solidcraft_ui_registry::publish_target(page.handle, rect);
                response.backing_on_hover_text(label);
            }
        });
    });
    if let Some((_, footer)) = sidebar_footer {
        let rect = egui::Rect::from_min_max(egui::pos2(nav_inner.left(), list.bottom()), nav_inner.max);
        ui.scope_builder(egui::UiBuilder::new().id_salt("pages_footer").max_rect(rect), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(rect));
            footer(ui, state);
        });
        solidcraft_ui_registry::publish_rect("dialog:pages_footer", rect);
    }
    let content = egui::Rect::from_min_max(egui::pos2(nav.right(), body.top()), body.max);
    let padding = egui::epaint::MarginF32 {
        left: padding.leftf().min(content.width() / 2.0),
        right: padding.rightf().min(content.width() / 2.0),
        top: padding.topf().min(content.height() / 2.0),
        bottom: padding.bottomf().min(content.height() / 2.0),
    };
    let content = content - padding;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(content)
            .ui_stack_info(egui::UiStackInfo::default().with_tag_value(super::preferences::PAGE_SCROLL_GUTTER, padding.right)),
        |ui| {
            let clip = egui::Rect::from_min_max(content.min, egui::pos2(body.right(), content.bottom()));
            ui.set_clip_rect(ui.clip_rect().intersect(clip));
            ui.spacing_mut().item_spacing.y = t.form_gap_y;
            if let Some(header) = header {
                header(ui, state);
            }
            let force_scroll = search_scroll && super::settings_search::force_scroll(ui.ctx());
            if let Some(page) = pages.get(*selected) {
                if page.scroll || force_scroll {
                    let scroll = super::settings_scroll(ui, ("sc_prefs_page", *selected), |ui| (page.draw)(ui, state));
                    solidcraft_ui_registry::publish_rect("dialog:page_body", scroll.inner_rect);
                } else {
                    (page.draw)(ui, state);
                    solidcraft_ui_registry::publish_rect("dialog:page_body", content);
                }
            }
        },
    );
}
