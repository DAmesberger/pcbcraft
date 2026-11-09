//! The shared shell: workspace switcher, ribbon tabs and grouped tools, and
//! floating cards. Editors describe their ribbon as data ([`Ribbon`]); the
//! shell draws it and reports what was clicked.
//!
//! Adapted from Solidcraft 4f3ac07a: toolbar.rs (workspace switcher, tab row,
//! button sizing, section captions), shell.rs (floating cards) and
//! ui_kit/panel.rs (card frames). MIT option; see ATTRIBUTION.md. The
//! upstream fonts, icons, tokens and Container are used directly.
use egui::{Align2, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::{
    fonts, icons,
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
/// Width of the ribbon's » overflow button.
const OVERFLOW_WIDTH: f32 = 30.0;
/// Width of the » overflow menu's rows.
const OVERFLOW_MENU_WIDTH: f32 = 200.0;
/// Height of one tool row in the » overflow menu.
const OVERFLOW_ROW_HEIGHT: f32 = 30.0;
const WORKSPACE_MENU_WIDTH: f32 = 280.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// One ribbon button. With `menu` it is a split button, as Solidcraft's
/// sketch ribbon draws a tool family (Line ▾, Rectangle ▾): the icon runs
/// the shown variant (`id`), the label with ▾ opens the family's variants.
pub struct Tool {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub enabled: bool,
    pub selected: bool,
    /// Tooltip; why a disabled tool is unavailable.
    pub tip: &'static str,
    /// The family's variants, shown variant included; empty for a plain
    /// button.
    pub menu: Vec<Tool>,
    /// The key that runs it, as shown; the app fills it from the command
    /// registry (`crate::commands`) before the ribbon is drawn.
    pub shortcut: Option<String>,
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
            menu: Vec::new(),
            shortcut: None,
        }
    }
    /// A split button showing `shown` with the variants `menu`; selected
    /// while any variant is.
    pub fn family(shown: Tool, menu: Vec<Tool>) -> Self {
        Self {
            selected: menu.iter().any(|t| t.selected),
            menu,
            ..shown
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
    /// The Issues toggle: expand or collapse the Issues card.
    Issues,
}

/// The ribbon's Issues toggle at its right end, in every workspace and
/// state (docs/sidebar-ia.md, D2: it replaces Check). It shows the active
/// workspace's counts and is lit while the Issues card is expanded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IssuesToggle {
    /// `None` while the issues cannot be counted.
    pub counts: Option<pcbcraft_core::issues::IssueCounts>,
    pub expanded: bool,
}

/// The Issues toggle's width at the ribbon's right end.
const ISSUES_TOGGLE_WIDTH: f32 = 112.0;

fn button_width(ui: &Ui, label: &str) -> f32 {
    let t = Tokens::get();
    let width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(t.font_body), t.text)
        .size()
        .x;
    (width + BUTTON_LABEL_PADDING).clamp(BUTTON_MIN_WIDTH, BUTTON_MAX_WIDTH)
}

/// Width of the ▾ beside a split button's label.
const SPLIT_ARROW: f32 = 12.0;
/// Height of a split button's upper (run) part, as Solidcraft's.
const SPLIT_TOP: f32 = 32.0;

/// A ribbon button's width. A split button fits its label and ▾; one whose
/// label is the chosen variant's ("Line") fits its widest variant, so the
/// ribbon does not move when the choice changes.
fn tool_width(ui: &Ui, tool: &Tool) -> f32 {
    if tool.menu.is_empty() {
        return button_width(ui, tool.label);
    }
    // The ▾ sits in the label's padding where it fits.
    let t = Tokens::get();
    let split = |label: &str| {
        let text = ui
            .painter()
            .layout_no_wrap(label.to_owned(), FontId::proportional(t.font_body), t.text)
            .size()
            .x;
        (text + SPLIT_ARROW + BUTTON_LABEL_PADDING).max(BUTTON_MIN_WIDTH)
    };
    if tool.menu.iter().all(|v| v.label != tool.label) {
        return split(tool.label);
    }
    tool.menu.iter().map(|v| split(v.label)).fold(0.0, f32::max)
}

#[cfg(test)]
mod tool_width_tests {
    use super::*;

    /// N10: a family named apart from its variants (Label ▾) is as wide as
    /// its name, so the gaps either side of it match its neighbours'; one
    /// showing the chosen variant (Line ▾) keeps room for its widest.
    #[test]
    fn a_split_button_fits_its_own_name() {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let variant = |id, label| Tool::new(id, "line", label);
        let mut widths = (0.0, 0.0, 0.0);
        ctx.run_ui(Default::default(), |ui| {
            let long = "Hierarchical sheet label";
            let named = Tool::family(
                Tool::new("label", "line", "Label"),
                vec![variant("net", "Net label"), variant("sheet", long)],
            );
            let shown = Tool::family(
                Tool::new("short", "line", "Line"),
                vec![variant("short", "Line"), variant("long", long)],
            );
            widths = (
                tool_width(ui, &named),
                tool_width(ui, &shown),
                tool_width(ui, &Tool::new("plain", "line", long)),
            );
        })
        .textures_delta
        .clear();
        let (named, shown, plain) = widths;
        assert_eq!(
            named, BUTTON_MIN_WIDTH,
            "Label ▾ takes no more than a button"
        );
        assert!(shown > BUTTON_MAX_WIDTH, "Line ▾ fits its widest: {shown}");
        assert_eq!(plain, BUTTON_MAX_WIDTH);
    }
}

fn group_width(ui: &Ui, group: &Group) -> f32 {
    let t = Tokens::get();
    let buttons: f32 = group
        .tools
        .iter()
        .map(|tool| tool_width(ui, tool))
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
    crate::reveal::register(ui.ctx(), crate::reveal::Target::Tool(tool.id), rect);
    let response = ui.interact(
        rect,
        ui.scope_id().with(("tool", tool.id)),
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
        egui::WidgetInfo::selected(egui::Role::Button, tool.enabled, tool.selected, tool.label)
    });
    tool_tooltip(response, tool)
}

/// What a ribbon tool's tooltip says: the title with its shortcut
/// ("Route (R)"), the key cap on its own, and the help line (or why a
/// disabled tool is unavailable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolTip {
    pub title: String,
    pub key: Option<String>,
    pub body: String,
}

pub fn tool_tip(tool: &Tool) -> ToolTip {
    let title = match &tool.shortcut {
        Some(key) => format!("{} ({key})", tool.label),
        None => tool.label.to_owned(),
    };
    let body = if tool.tip.is_empty() && !tool.enabled {
        format!("{} is unavailable now", tool.label)
    } else {
        tool.tip.to_owned()
    };
    ToolTip {
        title,
        key: tool.shortcut.clone(),
        body,
    }
}

/// Solidcraft's ribbon tooltip (toolbar.rs `ribbon_tooltip`): the name in
/// the heading font with its key cap at the right, the help below.
pub fn tool_tooltip(response: Response, tool: &Tool) -> Response {
    let tip = tool_tip(tool);
    let label = tool.label;
    response.on_hover_ui(move |ui| shortcut_tooltip(ui, label, tip.key.as_deref(), &tip.body))
}

/// The body of a tooltip naming a command and its key.
pub fn shortcut_tooltip(ui: &mut Ui, name: &str, key: Option<&str>, body: &str) {
    let t = Tokens::get();
    let name_width = ui
        .painter()
        .layout_no_wrap(name.to_owned(), fonts::semibold(t.font_heading), t.text)
        .size()
        .x;
    let key_width = key.map_or(0.0, |key| {
        ui.painter()
            .layout_no_wrap(key.to_owned(), FontId::monospace(t.font_body), t.text_dim)
            .size()
            .x
            + t.space_24
    });
    ui.set_width(f32::max(240.0, name_width + key_width + t.space_12));
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(name)
                .font(fonts::semibold(t.font_heading))
                .color(t.text),
        );
        if let Some(key) = key {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                key_cap(ui, key);
            });
        }
    });
    if !body.is_empty() {
        ui.label(egui::RichText::new(body).color(t.text_dim));
    }
}

/// A shortcut key cap (Solidcraft's ribbon tooltip cap).
pub fn key_cap(ui: &mut Ui, key: &str) -> Response {
    let t = Tokens::get();
    egui::Frame::NONE
        .fill(t.panel_header)
        .stroke(t.tertiary_border(t.border))
        .corner_radius(t.radius_sm.saturating_sub(1))
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(key).monospace().color(t.text_dim));
        })
        .response
}

/// A split ribbon button, adapted from Solidcraft's sketch families
/// (toolbar.rs): the upper part shows the variant's icon and runs it; the
/// label with ▾ below opens the variants. Returns the tool clicked.
fn split_button(ui: &mut Ui, rect: Rect, tool: &Tool) -> Option<&'static str> {
    let t = Tokens::get();
    let sense = if tool.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    crate::reveal::register(ui.ctx(), crate::reveal::Target::Tool(tool.id), rect);
    let upper = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + SPLIT_TOP));
    let lower = Rect::from_min_max(pos2(rect.left(), rect.top() + SPLIT_TOP - 2.0), rect.max);
    let run = ui.interact(upper, ui.scope_id().with(("tool", tool.id)), sense);
    let arrow = ui.interact(
        lower,
        ui.scope_id()
            .with(("family", tool.menu.first().map(|v| v.id))),
        sense,
    );
    if tool.selected {
        ui.painter().rect_filled(rect, t.radius_sm, t.selected_bg);
    } else if tool.enabled && (run.hovered() || arrow.hovered()) {
        ui.painter()
            .rect_filled(rect, t.radius_sm, t.hover.gamma_multiply(0.5));
    }
    if tool.enabled && !tool.selected {
        if run.hovered() {
            ui.painter().rect_filled(upper, t.radius_sm, t.hover);
        } else if arrow.hovered() {
            ui.painter().rect_filled(lower, t.radius_sm, t.hover);
        }
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
    let galley = ui
        .painter()
        .layout_no_wrap(tool.label.to_owned(), font, ink);
    let width = galley.size().x + SPLIT_ARROW;
    let left = rect.center().x - width / 2.0;
    let y = rect.top() + 32.0;
    let label_size = galley.size();
    ui.painter().galley(pos2(left, y), galley, ink);
    crate::kit::chevron(
        ui.painter(),
        Rect::from_center_size(
            pos2(
                left + label_size.x + SPLIT_ARROW / 2.0 + 1.0,
                y + label_size.y / 2.0,
            ),
            vec2(10.0, 10.0),
        ),
        true,
        t.text_dim,
    );
    // As Solidcraft names a family's parts: the upper part by the variant
    // it runs ("Net label"), the label with ▾ by the family ("Label"),
    // which opens the variants.
    let label = tool.label;
    let shown = tool
        .menu
        .iter()
        .find(|v| v.id == tool.id)
        .map_or(label, |v| v.label);
    run.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, tool.enabled, tool.selected, shown)
    });
    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, tool.enabled, label));
    let mut clicked = None;
    let run = tool_tooltip(run, tool);
    if run.clicked() && tool.enabled {
        clicked = Some(tool.id);
    }
    let arrow = arrow.on_hover_text(format!("More {} tools", label.to_lowercase()));
    Container::new(ui.ctx(), Surface::Popup)
        .menu(&arrow)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            crate::kit::form(ui, |ui| {
                ui.set_width(OVERFLOW_MENU_WIDTH);
                for variant in &tool.menu {
                    if overflow_item(ui, variant).clicked() && variant.enabled {
                        clicked = Some(variant.id);
                    }
                }
            });
        });
    clicked
}

/// The workspace switcher card and the ribbon card.
pub fn ribbon(
    ui: &mut Ui,
    workspace: Workspace,
    ribbon: &Ribbon,
    issues: &IssuesToggle,
) -> Option<RibbonEvent> {
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
            let (groups, toggle) = issues_slot(groups);
            if let Some(tool) = group_row(ui, groups, ribbon) {
                event = Some(RibbonEvent::Tool(tool));
            }
            if issues_toggle(ui, toggle, issues) {
                event = Some(RibbonEvent::Issues);
            }
        });
    event
}

/// Split the groups row: the tools, and the Issues toggle at the right.
/// The toggle keeps its width (the tools overflow into » first) unless the
/// row is narrower than it.
fn issues_slot(groups: Rect) -> (Rect, Rect) {
    let width = ISSUES_TOGGLE_WIDTH.min(groups.width() / 2.0).max(0.0);
    let split = groups.right() - width;
    (
        Rect::from_min_max(groups.min, pos2(split, groups.bottom())),
        Rect::from_min_max(pos2(split, groups.top()), groups.max),
    )
}

/// The Issues toggle: the counts as a badge over the label "Issues", lit
/// while the card is expanded. Returns whether it was clicked.
fn issues_toggle(ui: &mut Ui, slot: Rect, toggle: &IssuesToggle) -> bool {
    let t = Tokens::get();
    let rect = Rect::from_min_size(
        pos2(slot.left() + TAB_INSET, slot.top() + 2.0),
        vec2((slot.width() - 2.0 * TAB_INSET).max(0.0), BUTTON_HEIGHT),
    );
    let separator = t.toolbar.lerp_to_gamma(t.border, 0.6);
    t.secondary_vline(
        ui.painter(),
        slot.left(),
        (rect.top() + 4.0)..=(slot.bottom() - 6.0),
        separator,
    );
    let response = ui.interact(rect, ui.scope_id().with("issues-toggle"), Sense::click());
    if toggle.expanded {
        ui.painter().rect_filled(rect, t.radius_sm, t.selected_bg);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let mut badge_clicked = false;
    let words = match &toggle.counts {
        Some(counts) => {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        pos2(rect.left(), rect.top() + 5.0),
                        pos2(rect.right(), rect.top() + 32.0),
                    ))
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            badge_clicked = crate::kit::count_badge(&mut child, counts, false).clicked();
            crate::kit::issues::counts_words(counts, true, false)
        }
        None => "Not counted".to_owned(),
    };
    let ink = if toggle.expanded {
        t.selected_ink
    } else {
        t.text
    };
    let font = if toggle.expanded {
        fonts::semibold(t.font_body)
    } else {
        FontId::proportional(t.font_body)
    };
    ui.painter().text(
        pos2(rect.center().x, rect.top() + 32.0),
        Align2::CENTER_TOP,
        "Issues",
        font,
        ink,
    );
    let label = format!("Issues: {words}");
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, toggle.expanded, &label)
    });
    let tip = if toggle.expanded {
        "Hide the Issues list"
    } else {
        "Show the Issues list"
    };
    response.on_hover_text(format!("{label}\n{tip}")).clicked() || badge_clicked
}

fn workspace_switcher(ui: &mut Ui, tile: Rect, workspace: Workspace) -> Option<Workspace> {
    let ctx = ui.ctx().clone();
    let t = Tokens::get();
    let response = ui
        .interact(
            tile,
            ui.scope_id().with("workspace-switcher"),
            Sense::click(),
        )
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
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Change workspace"));
    let mut chosen = None;
    Container::new(&ctx, Surface::Popup)
        .menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            crate::kit::form(ui, |ui| {
                ui.set_width(WORKSPACE_MENU_WIDTH);
                for item in Workspace::ALL {
                    if workspace_item(ui, item, item == workspace).clicked() {
                        chosen = Some(item);
                        // Also when the item is pressed without a pointer
                        // click (keyboard, accessibility).
                        ui.close();
                    }
                }
            });
        });
    chosen.filter(|c| *c != workspace)
}

fn workspace_item(ui: &mut Ui, workspace: Workspace, current: bool) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(vec2(WORKSPACE_MENU_WIDTH, 44.0), Sense::click());
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
        egui::WidgetInfo::selected(egui::Role::Button, true, current, workspace.label())
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
        crate::reveal::register(ui.ctx(), crate::reveal::Target::Tab(tab.id), rect);
        let response = ui.interact(rect, ui.scope_id().with(("tab", tab.id)), Sense::click());
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
            egui::WidgetInfo::selected(egui::Role::Button, true, tab.active, tab.label)
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
    // Groups that fit in full; the rest go to the » menu, as Solidcraft's
    // toolbar trims panels into its overflow menu, so every tool stays
    // reachable by a click however narrow the window.
    let widths: Vec<f32> = ribbon.groups.iter().map(|g| group_width(ui, g)).collect();
    let total: f32 = widths.iter().sum::<f32>() + widths.len().saturating_sub(1) as f32;
    let room = area.right() - x;
    let shown = if total <= room {
        ribbon.groups.len()
    } else {
        let mut used = 0.0;
        widths
            .iter()
            .take_while(|w| {
                used += *w + 1.0;
                used <= room - OVERFLOW_WIDTH
            })
            .count()
    };
    for (index, group) in ribbon.groups.iter().enumerate().take(shown) {
        let width = widths[index];
        let buttons: f32 = group
            .tools
            .iter()
            .map(|tool| tool_width(ui, tool))
            .sum::<f32>()
            + group.tools.len().saturating_sub(1) as f32 * BUTTON_GAP;
        let mut bx = x + (width - buttons) / 2.0;
        for tool in &group.tools {
            let w = tool_width(ui, tool);
            let rect = Rect::from_min_size(pos2(bx, top), vec2(w, BUTTON_HEIGHT));
            if tool.menu.is_empty() {
                if tool_button(ui, rect, tool).clicked() && tool.enabled {
                    clicked = Some(tool.id);
                }
            } else if let Some(id) = split_button(ui, rect, tool) {
                clicked = Some(id);
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
    if shown < ribbon.groups.len() {
        let rect = Rect::from_min_size(
            pos2(area.right() - OVERFLOW_WIDTH, top),
            vec2(OVERFLOW_WIDTH - TAB_INSET, BUTTON_HEIGHT),
        );
        let response = ui
            .interact(rect, ui.scope_id().with("ribbon-overflow"), Sense::click())
            .on_hover_text("More tools");
        if response.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "»",
            FontId::proportional(t.font_panel),
            t.text,
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "More tools"));
        Container::new(ui.ctx(), Surface::Popup)
            .menu(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                crate::kit::form(ui, |ui| {
                    ui.set_width(OVERFLOW_MENU_WIDTH);
                    for group in &ribbon.groups[shown..] {
                        overflow_caption(ui, group.label);
                        // A family lists its variants in the menu.
                        for tool in group.tools.iter().flat_map(|t| {
                            if t.menu.is_empty() {
                                std::slice::from_ref(t)
                            } else {
                                &t.menu[..]
                            }
                        }) {
                            if overflow_item(ui, tool).clicked() && tool.enabled {
                                clicked = Some(tool.id);
                            }
                        }
                    }
                });
            });
    }
    clicked
}

/// A group caption in the » overflow menu, like the ribbon's captions.
fn overflow_caption(ui: &mut Ui, label: &str) {
    let t = Tokens::get();
    let (rect, _) =
        ui.allocate_exact_size(vec2(OVERFLOW_MENU_WIDTH, SECTION_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(rect.left() + t.space_4, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(t.font_caption),
        t.text_dim,
    );
}

/// One tool in the » overflow menu: its ribbon icon and label, with the
/// ribbon's hover, selected and disabled looks.
fn overflow_item(ui: &mut Ui, tool: &Tool) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(OVERFLOW_MENU_WIDTH, OVERFLOW_ROW_HEIGHT),
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
        Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(20.0, 20.0)),
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
    ui.painter().text(
        pos2(rect.left() + 36.0, rect.center().y),
        Align2::LEFT_CENTER,
        tool.label,
        font,
        ink,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, tool.enabled, tool.selected, tool.label)
    });
    tool_tooltip(response, tool)
}

pub fn icon_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> Response {
    icon_button_help(ui, icon, label, "", enabled)
}

/// An [`icon_button`] whose hover help adds `help` (lines) under its label.
pub fn icon_button_help(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    help: &str,
    enabled: bool,
) -> Response {
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if help.is_empty() {
        response.on_hover_text(label)
    } else {
        response.on_hover_ui(|ui| {
            ui.strong(label);
            ui.label(help);
        })
    }
}

/// How wide a grip a resizable card's right edge offers, counted inwards
/// (upstream `browser::BROWSER_RAIL`).
pub const RESIZE_RAIL: f32 = 6.0;

/// A card's right-edge resize: the remembered width, changed while the
/// edge is dragged and kept inside `range`.
pub struct Resize<'a> {
    pub width: &'a mut f32,
    pub range: std::ops::RangeInclusive<f32>,
}

/// What a resizable card's edge did this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resized {
    /// The width changed.
    pub changed: bool,
    /// A drag of the edge ended: the time to remember the width.
    pub stopped: bool,
}

/// `width + delta` inside `range`; a non-finite delta keeps the width, and
/// a non-finite width becomes the range's start.
pub fn resized_width(width: f32, delta: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    let (min, max) = (*range.start(), range.end().max(*range.start()));
    let width = if width.is_finite() { width } else { min };
    let next = if delta.is_finite() {
        width + delta
    } else {
        width
    };
    next.clamp(min, max)
}

/// A floating card at `rect`: `header` draws its title band, `add` its
/// scrolling body. The card hugs its content up to `rect`'s height. With
/// `resize`, its right edge is a rail that drags the card's width, as
/// upstream's `browser::resize_column`; the caller lays `rect` out from
/// that width.
pub fn card(
    ctx: &egui::Context,
    id: &str,
    rect: Rect,
    resize: Option<Resize<'_>>,
    header: impl FnOnce(&mut Ui),
    add: impl FnOnce(&mut Ui),
) -> Resized {
    let t = Tokens::get();
    let host = Container::new(ctx, Surface::Panel);
    let mut out = Resized::default();
    host.area(egui::Id::unique(id))
        .fixed_pos(rect.min)
        .movable(false)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let card = host
                .frame(ctx, t.panel, solidcraft_ui_egui::frame::Side::Left)
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
                })
                .response
                .rect;
            let Some(resize) = resize else {
                return;
            };
            let rail =
                Rect::from_x_y_ranges(card.right() - RESIZE_RAIL..=card.right(), card.y_range());
            let response = ui
                .interact(rail, egui::Id::unique((id, "resize")), Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, true, "Resize"));
            if response.dragged() {
                let delta = ui.input(|i| i.pointer.delta().x);
                let next = resized_width(*resize.width, delta, &resize.range);
                out.changed = next != *resize.width;
                *resize.width = next;
            }
            out.stopped = response.drag_stopped();
        });
    out
}

/// The inset of a column card's frame, in points.
pub const COLUMN_CARD_INSET: i8 = 4;

/// A card of the right column (`kit::card::column`): it fills `rect`
/// exactly, whatever its content, so the cards stacked in the column keep
/// their places. `add` gets the whole inner rectangle, with no scroll of
/// its own (`kit::card::card` owns the one scroll). `inset` is the frame's
/// vertical inset; the horizontal one is [`COLUMN_CARD_INSET`].
pub fn column_card<R>(
    ctx: &egui::Context,
    id: &str,
    rect: Rect,
    inset: i8,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let t = Tokens::get();
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let host = Container::new(ctx, Surface::Panel);
    let frame = host
        .frame(ctx, t.panel, solidcraft_ui_egui::frame::Side::Left)
        .inner_margin(egui::Margin::symmetric(COLUMN_CARD_INSET, inset));
    let inner = (rect.size() - frame.total_margin().sum()).max(egui::Vec2::ZERO);
    let shown = host
        .area(egui::Id::unique(id))
        .fixed_pos(rect.min)
        .movable(false)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    add(ui)
                })
                .inner
        });
    Some(shown.inner)
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
