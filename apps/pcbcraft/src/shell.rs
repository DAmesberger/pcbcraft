//! The shared shell: workspace switcher, ribbon tabs and grouped tools, and
//! floating cards. Editors describe their ribbon as data ([`Ribbon`]); the
//! shell draws it and reports what was clicked.
//!
//! Adapted from Solidcraft 4f3ac07a: toolbar.rs (workspace switcher, tab row,
//! button sizing, section captions), shell.rs (floating cards) and
//! ui_kit/panel.rs (card frames). MIT option; see ATTRIBUTION.md. The
//! upstream fonts, icons, tokens and Container are used directly.
use egui::{Align2, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use solidcraft_ui_egui::{
    fonts, icons,
    theme::Tokens,
    toolbar::{RIBBON_HEIGHT, TAB_ROW_HEIGHT},
    ui_kit::{Container, Edge, Surface},
};

/// Upstream `toolbar::WORKSPACE_WIDTH` (crate-private there).
const WORKSPACE_WIDTH: f32 = 88.0;
/// Upstream ribbon button metrics (crate-private there).
const BUTTON_HEIGHT: f32 = 60.0;
const BUTTON_GAP: f32 = 3.0;
const BUTTON_MIN_WIDTH: f32 = 54.0;
const BUTTON_MAX_WIDTH: f32 = 76.0;
const BUTTON_LABEL_PADDING: f32 = 10.0;
const SECTION_HEIGHT: f32 = 20.0;
const TAB_INSET: f32 = 4.0;
const GROUP_PADDING: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    Part,
    Schematic,
    Pcb,
}

impl Workspace {
    pub const ALL: [Self; 3] = [Self::Part, Self::Schematic, Self::Pcb];
    pub fn label(self) -> &'static str {
        match self {
            Self::Part => "Component",
            Self::Schematic => "Schematic",
            Self::Pcb => "PCB",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Part => "component",
            Self::Schematic => "sketch",
            Self::Pcb => "plane",
        }
    }
    pub fn summary(self) -> &'static str {
        match self {
            Self::Part => "Symbols, footprints, pin mapping and 3D",
            Self::Schematic => "Place components and connect nets",
            Self::Pcb => "Place, route and check the board",
        }
    }
}

/// One ribbon button.
pub struct Tool {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub enabled: bool,
    pub selected: bool,
    /// Tooltip; why a disabled tool is unavailable.
    pub tip: &'static str,
}

impl Tool {
    pub fn new(id: &'static str, icon: &'static str, label: &'static str) -> Self {
        Self {
            id,
            icon,
            label,
            enabled: true,
            selected: false,
            tip: "",
        }
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn tip(mut self, tip: &'static str) -> Self {
        self.tip = tip;
        self
    }
}

/// A captioned group of tools.
pub struct Group {
    pub label: &'static str,
    pub tools: Vec<Tool>,
}

/// A ribbon tab: one view of the workspace's document.
pub struct Tab {
    pub id: &'static str,
    pub label: &'static str,
    pub active: bool,
}

/// What an editor shows in the ribbon.
#[derive(Default)]
pub struct Ribbon {
    pub tabs: Vec<Tab>,
    pub groups: Vec<Group>,
    /// Replaces the groups with a message, e.g. while a review is open.
    pub notice: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RibbonEvent {
    Workspace(Workspace),
    Tab(&'static str),
    Tool(&'static str),
}

fn button_width(ui: &Ui, label: &str) -> f32 {
    let t = Tokens::get();
    let width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(t.font_body), t.text)
        .size()
        .x;
    (width + BUTTON_LABEL_PADDING).clamp(BUTTON_MIN_WIDTH, BUTTON_MAX_WIDTH)
}

fn group_width(ui: &Ui, group: &Group) -> f32 {
    let t = Tokens::get();
    let buttons: f32 = group
        .tools
        .iter()
        .map(|tool| button_width(ui, tool.label))
        .sum::<f32>()
        + group.tools.len().saturating_sub(1) as f32 * BUTTON_GAP;
    let caption = ui
        .painter()
        .layout_no_wrap(
            group.label.to_owned(),
            FontId::proportional(t.font_caption),
            t.text_dim,
        )
        .size()
        .x;
    buttons.max(caption + t.space_16) + 2.0 * GROUP_PADDING
}

/// One ribbon button: icon over label, Solidcraft's 60 pt action height.
fn tool_button(ui: &mut Ui, rect: Rect, tool: &Tool) -> Response {
    let t = Tokens::get();
    let response = ui.interact(
        rect,
        ui.id().with(("tool", tool.id)),
        if tool.enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if tool.selected {
        ui.painter().rect_filled(rect, t.radius_sm, t.selected_bg);
    } else if response.hovered() && tool.enabled {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    icons::paint_icon(
        ui.painter(),
        Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0), vec2(26.0, 26.0)),
        tool.icon,
        if tool.enabled { 1.0 } else { 0.35 },
    );
    let ink = if tool.selected {
        t.selected_ink
    } else if tool.enabled {
        t.text
    } else {
        t.text_dim
    };
    let font = if tool.selected {
        fonts::semibold(t.font_body)
    } else {
        FontId::proportional(t.font_body)
    };
    let mut job =
        egui::text::LayoutJob::simple(tool.label.to_owned(), font, ink, rect.width() - 4.0);
    job.wrap.max_rows = 2;
    job.halign = egui::Align::Center;
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .galley(pos2(rect.center().x, rect.top() + 32.0), galley, ink);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            tool.enabled,
            tool.selected,
            tool.label,
        )
    });
    match (tool.enabled, tool.tip.is_empty()) {
        (true, true) => response.on_hover_text(tool.label),
        (true, false) => response.on_hover_text(format!("{}\n{}", tool.label, tool.tip)),
        (false, true) => response.on_hover_text(format!("{} is unavailable now", tool.label)),
        (false, false) => response.on_hover_text(format!("{}\n{}", tool.label, tool.tip)),
    }
}

/// The workspace switcher card and the ribbon card.
pub fn ribbon(ui: &mut Ui, workspace: Workspace, ribbon: &Ribbon) -> Option<RibbonEvent> {
    let ctx = ui.ctx().clone();
    let t = Tokens::get();
    let host = Container::new(&ctx, Surface::Chrome);
    let gap = host.gap();
    let mut event = None;
    host.edge("workspace-ribbon", Edge::Top)
        .exact_size(RIBBON_HEIGHT + gap * 2.0)
        .show_separator_line(false)
        .frame(host.edge_frame(egui::Color32::TRANSPARENT))
        .show(ui, |ui| {
            let available = ui.max_rect().shrink(gap);
            let tile = Rect::from_min_size(available.min, vec2(WORKSPACE_WIDTH, RIBBON_HEIGHT));
            let card = Rect::from_min_max(pos2(tile.right() + gap, available.top()), available.max);
            for rect in [tile, card] {
                ui.painter().add(
                    host.frame(&ctx, t.toolbar, solidcraft_ui_egui::frame::Side::Left)
                        .paint(rect),
                );
            }
            if let Some(chosen) = workspace_switcher(ui, tile, workspace) {
                event = Some(RibbonEvent::Workspace(chosen));
            }
            let tabs = Rect::from_min_size(card.min, vec2(card.width(), TAB_ROW_HEIGHT));
            if let Some(tab) = tab_row(ui, tabs, ribbon) {
                event = Some(RibbonEvent::Tab(tab));
            }
            let groups = Rect::from_min_max(pos2(card.left(), tabs.bottom()), card.max);
            if let Some(tool) = group_row(ui, groups, ribbon) {
                event = Some(RibbonEvent::Tool(tool));
            }
        });
    event
}

fn workspace_switcher(ui: &mut Ui, tile: Rect, workspace: Workspace) -> Option<Workspace> {
    let ctx = ui.ctx().clone();
    let t = Tokens::get();
    let response = ui
        .interact(tile, ui.id().with("workspace-switcher"), Sense::click())
        .on_hover_text("Change workspace");
    if response.hovered() {
        ui.painter()
            .rect_filled(tile.shrink(TAB_INSET), t.radius_md, t.hover);
    }
    ui.painter().rect_filled(
        Rect::from_min_size(
            tile.min + vec2(t.space_8, 0.0),
            vec2(tile.width() - 2.0 * t.space_8, t.space_3),
        ),
        t.radius_sm,
        t.accent,
    );
    icons::paint_icon(
        ui.painter(),
        Rect::from_center_size(pos2(tile.center().x, tile.top() + 39.0), vec2(30.0, 30.0)),
        workspace.icon(),
        1.0,
    );
    ui.painter().text(
        pos2(tile.center().x, tile.top() + 69.0),
        Align2::CENTER_CENTER,
        workspace.label(),
        fonts::semibold(t.font_button),
        t.text,
    );
    let caption = ui.painter().layout_no_wrap(
        "Workspace".to_owned(),
        FontId::proportional(t.font_small),
        t.text_dim,
    );
    let caption_width = caption.size().x;
    let caption_center = pos2(tile.center().x - 6.0, tile.top() + 89.0);
    ui.painter()
        .galley(caption_center - caption.size() / 2.0, caption, t.text_dim);
    crate::kit::chevron(
        ui.painter(),
        Rect::from_center_size(
            pos2(
                caption_center.x + caption_width / 2.0 + 7.0,
                caption_center.y + 0.5,
            ),
            vec2(10.0, 10.0),
        ),
        true,
        t.text_dim,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Change workspace")
    });
    let mut chosen = None;
    Container::new(&ctx, Surface::Popup)
        .menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            crate::kit::form(ui, |ui| {
                ui.set_min_width(260.0);
                for item in Workspace::ALL {
                    if workspace_item(ui, item, item == workspace).clicked() {
                        chosen = Some(item);
                    }
                }
            });
        });
    chosen.filter(|c| *c != workspace)
}

fn workspace_item(ui: &mut Ui, workspace: Workspace, current: bool) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    if current {
        ui.painter().rect_filled(rect, t.radius_sm, t.selected_bg);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    icons::paint_icon(
        ui.painter(),
        Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(24.0, 24.0)),
        workspace.icon(),
        1.0,
    );
    let ink = if current { t.selected_ink } else { t.text };
    ui.painter().text(
        pos2(rect.left() + 44.0, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        workspace.label(),
        fonts::semibold(t.font_label),
        ink,
    );
    ui.painter().text(
        pos2(rect.left() + 44.0, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        workspace.summary(),
        FontId::proportional(t.font_small),
        t.text_dim,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            current,
            workspace.label(),
        )
    });
    response
}

fn tab_row(ui: &mut Ui, row: Rect, ribbon: &Ribbon) -> Option<&'static str> {
    let t = Tokens::get();
    let corner = t
        .radius_lg
        .saturating_sub(TAB_INSET as u8)
        .max(t.radius_sm / 2);
    let mut x = row.left() + TAB_INSET;
    let mut clicked = None;
    for tab in &ribbon.tabs {
        let font = fonts::semibold(t.font_button);
        let width = ui
            .painter()
            .layout_no_wrap(tab.label.to_owned(), font.clone(), t.text)
            .size()
            .x
            + 24.0;
        let rect = Rect::from_min_size(
            pos2(x, row.top() + TAB_INSET),
            vec2(width, TAB_ROW_HEIGHT - 2.0 * TAB_INSET),
        );
        if rect.right() > row.right() {
            break;
        }
        let response = ui.interact(rect, ui.id().with(("tab", tab.id)), Sense::click());
        let (fill, ink) = if tab.active {
            (t.accent_soft, t.selected_ink)
        } else if response.hovered() {
            (t.hover, t.text)
        } else {
            (egui::Color32::TRANSPARENT, t.text)
        };
        ui.painter().rect_filled(rect, corner, fill);
        let font = if tab.active {
            font
        } else {
            FontId::proportional(t.font_button)
        };
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, tab.label, font, ink);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, tab.active, tab.label)
        });
        if response.clicked() && !tab.active {
            clicked = Some(tab.id);
        }
        x += width + 2.0;
    }
    t.secondary_hline(
        ui.painter(),
        row.x_range(),
        row.bottom() - 0.5,
        t.toolbar.lerp_to_gamma(t.border, 0.5),
    );
    clicked
}

fn group_row(ui: &mut Ui, area: Rect, ribbon: &Ribbon) -> Option<&'static str> {
    let t = Tokens::get();
    if let Some(notice) = ribbon.notice {
        ui.painter().text(
            pos2(area.left() + t.space_16, area.center().y),
            Align2::LEFT_CENTER,
            notice,
            FontId::proportional(t.font_label),
            t.text_dim,
        );
        return None;
    }
    let mut clicked = None;
    let mut x = area.left() + TAB_INSET;
    let top = area.top() + 2.0;
    let separator = t.toolbar.lerp_to_gamma(t.border, 0.6);
    for (index, group) in ribbon.groups.iter().enumerate() {
        let width = group_width(ui, group);
        if x + width > area.right() {
            break;
        }
        let buttons: f32 = group
            .tools
            .iter()
            .map(|tool| button_width(ui, tool.label))
            .sum::<f32>()
            + group.tools.len().saturating_sub(1) as f32 * BUTTON_GAP;
        let mut bx = x + (width - buttons) / 2.0;
        for tool in &group.tools {
            let w = button_width(ui, tool.label);
            let rect = Rect::from_min_size(pos2(bx, top), vec2(w, BUTTON_HEIGHT));
            if tool_button(ui, rect, tool).clicked() && tool.enabled {
                clicked = Some(tool.id);
            }
            bx += w + BUTTON_GAP;
        }
        let caption = Rect::from_min_size(
            pos2(x, area.bottom() - SECTION_HEIGHT - (TAB_INSET - 1.0)),
            vec2(width, SECTION_HEIGHT),
        );
        ui.painter().text(
            caption.center(),
            Align2::CENTER_CENTER,
            group.label,
            FontId::proportional(t.font_caption),
            t.text_dim,
        );
        x += width;
        if index + 1 < ribbon.groups.len() {
            t.secondary_vline(
                ui.painter(),
                x,
                (top + 4.0)..=(area.bottom() - 6.0),
                separator,
            );
            x += 1.0;
        }
    }
    clicked
}

pub fn icon_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> Response {
    let t = Tokens::get();
    let (r, response) = ui.allocate_exact_size(
        vec2(t.control_height, t.control_height),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if response.hovered() && enabled {
        ui.painter().rect_filled(r, t.radius_sm, t.hover);
    }
    icons::paint_icon(
        ui.painter(),
        r.shrink(t.space_4),
        icon,
        if enabled { 1.0 } else { 0.35 },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

/// A floating card at `rect`: `header` draws its title band, `add` its
/// scrolling body. The card hugs its content up to `rect`'s height.
pub fn card(
    ctx: &egui::Context,
    id: &str,
    rect: Rect,
    header: impl FnOnce(&mut Ui),
    add: impl FnOnce(&mut Ui),
) {
    let t = Tokens::get();
    let host = Container::new(ctx, Surface::Panel);
    host.area(egui::Id::new(id))
        .fixed_pos(rect.min)
        .movable(false)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            host.frame(ctx, t.panel, solidcraft_ui_egui::frame::Side::Left)
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    let inner = rect.width() - 8.0;
                    ui.set_width(inner);
                    ui.set_max_height(rect.height() - 8.0);
                    header(ui);
                    let used = t.form_header_height + t.space_8;
                    egui::ScrollArea::vertical()
                        .id_salt(id)
                        .max_height((rect.height() - used - t.space_16).max(40.0))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add_space(t.space_4);
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::symmetric(t.space_8 as i8, 0))
                                .show(ui, |ui| crate::kit::form(ui, add));
                            ui.add_space(t.space_8);
                        });
                });
        });
}

/// The properties card's title band: what is inspected and its kind.
pub struct Header {
    pub icon: &'static str,
    pub title: String,
    pub kind: &'static str,
}

impl Header {
    pub fn new(icon: &'static str, title: impl Into<String>, kind: &'static str) -> Self {
        Self {
            icon,
            title: title.into(),
            kind,
        }
    }
}
