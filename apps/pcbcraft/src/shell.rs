//! Adapted from Solidcraft 4f3ac07a: toolbar.rs (ribbon geometry), shell.rs
//! (floating cards), ui_kit/panel.rs (headers), ui_kit/layout.rs (forms).
//! MIT option; see ATTRIBUTION.md. The real upstream fonts, icons, tokens and
//! Container remain the implementation of their respective UI primitives.
use egui::{Align2, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use solidcraft_ui_egui::{
    fonts, icons,
    theme::Tokens,
    ui_kit::{Container, Edge, Surface},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Part,
    Schematic,
    Pcb,
    Copper,
}
impl Workspace {
    pub fn label(self) -> &'static str {
        match self {
            Self::Part => "Component",
            Self::Schematic => "Schematic",
            Self::Pcb => "PCB",
            Self::Copper => "Copper sample",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Part => "component",
            Self::Schematic => "sketch",
            Self::Pcb => "sketch",
            Self::Copper => "box",
        }
    }
}

/// Same icon/label positions and 60 pt action height as Solidcraft's ribbon.
pub fn tool(ui: &mut Ui, icon: &str, label: &str, enabled: bool, selected: bool) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(72.0, 60.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if selected {
        ui.painter().rect(
            rect,
            t.radius_sm,
            t.selected_bg,
            egui::Stroke::new(1.0, t.accent),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() && enabled {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    icons::paint_icon(
        ui.painter(),
        Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0), vec2(26.0, 26.0)),
        icon,
        if enabled { 1.0 } else { 0.35 },
    );
    let ink = if enabled { t.text } else { t.text_dim };
    ui.painter().text(
        pos2(rect.center().x, rect.top() + 40.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(t.font_small),
        ink,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if enabled {
        response.on_hover_text(label)
    } else {
        response.on_hover_text(format!("{label} is unavailable in the current state"))
    }
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

pub fn ribbon(ui: &mut Ui, workspace: &mut Workspace, tools: impl FnOnce(&mut Ui)) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get();
    let host = Container::new(&ctx, Surface::Chrome);
    let gap = host.gap();
    let height = solidcraft_ui_egui::toolbar::RIBBON_HEIGHT;
    host.edge("workspace-ribbon", Edge::Top)
        .exact_size(height + gap * 2.0)
        .show_separator_line(false)
        .frame(host.edge_frame(egui::Color32::TRANSPARENT))
        .show(ui, |ui| {
            let available = ui.max_rect().shrink(gap);
            let workspace_rect = Rect::from_min_size(available.min, vec2(88.0, height));
            let ribbon = Rect::from_min_max(
                pos2(workspace_rect.right() + gap, available.top()),
                available.max,
            );
            for rect in [workspace_rect, ribbon] {
                ui.painter().add(
                    host.frame(&ctx, t.toolbar, solidcraft_ui_egui::frame::Side::Left)
                        .paint(rect),
                );
            }
            ui.painter().rect_filled(
                Rect::from_min_size(
                    workspace_rect.min + vec2(t.space_8, 0.0),
                    vec2(72.0, t.space_3),
                ),
                t.radius_sm,
                t.accent,
            );
            icons::paint_icon(
                ui.painter(),
                Rect::from_center_size(
                    workspace_rect.center() - vec2(0.0, t.space_16),
                    vec2(30.0, 30.0),
                ),
                workspace.icon(),
                1.0,
            );
            ui.painter().text(
                workspace_rect.center() + vec2(0.0, t.space_12),
                Align2::CENTER_CENTER,
                workspace.label(),
                fonts::semibold(t.font_button),
                t.text,
            );
            ui.painter().text(
                workspace_rect.center() + vec2(0.0, t.space_12 + (t.space_16 + t.space_4)),
                Align2::CENTER_CENTER,
                "Electronics",
                FontId::proportional(t.font_small),
                t.text_dim,
            );
            let tabs = Rect::from_min_size(
                ribbon.min + vec2(t.space_6, 0.0),
                vec2(ribbon.width() - t.space_12, 30.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(tabs), |ui| {
                ui.horizontal(|ui| {
                    for item in [Workspace::Part, Workspace::Schematic, Workspace::Pcb] {
                        let r = solidcraft_ui_egui::ui_kit::button(
                            ui,
                            item.label(),
                            if *workspace == item {
                                solidcraft_ui_egui::ui_kit::Button::SmallSelected
                            } else {
                                solidcraft_ui_egui::ui_kit::Button::Ghost
                            },
                        );
                        if r.clicked() {
                            *workspace = item;
                        }
                    }
                });
            });
            t.secondary_hline(ui.painter(), ribbon.x_range(), tabs.bottom(), t.border);
            let content = Rect::from_min_max(
                pos2(ribbon.left() + t.space_8, tabs.bottom() + t.space_8),
                ribbon.max - vec2(t.space_8, t.space_8),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("ribbon-actions")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| tools(ui));
                    });
            });
        });
}

/// Tokenized form scope adopted from upstream ui_kit/layout.rs::scope.
pub fn form<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let t = Tokens::get();
        ui.spacing_mut().item_spacing = vec2(t.space_4, t.form_gap_y);
        ui.spacing_mut().interact_size.y = t.control_height;
        ui.spacing_mut().button_padding = vec2(t.space_8, t.space_4);
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(t.font_label));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(t.font_label));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        add(ui)
    })
    .inner
}

pub fn card(
    ctx: &egui::Context,
    id: &str,
    rect: Rect,
    title: &str,
    icon: &str,
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
                .inner_margin(0)
                .show(ui, |ui| {
                    ui.set_width(rect.width());
                    ui.set_max_height(rect.height());
                    let (header, _) = ui.allocate_exact_size(
                        vec2(rect.width(), t.form_header_height),
                        Sense::hover(),
                    );
                    icons::paint_icon(
                        ui.painter(),
                        Rect::from_center_size(
                            pos2(
                                header.left() + t.space_12 + t.form_icon_size / 2.0,
                                header.center().y,
                            ),
                            vec2(t.form_icon_size, t.form_icon_size),
                        ),
                        icon,
                        1.0,
                    );
                    ui.painter().text(
                        pos2(
                            header.left() + t.space_12 + t.form_icon_size + t.space_8,
                            header.center().y,
                        ),
                        Align2::LEFT_CENTER,
                        title,
                        fonts::semibold(t.font_heading),
                        t.text,
                    );
                    t.secondary_hline(
                        ui.painter(),
                        header.x_range(),
                        header.bottom(),
                        t.dialog_rule,
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(Rect::from_min_max(
                            pos2(rect.left() + t.space_12, header.bottom() + t.space_8),
                            rect.max - vec2(t.space_12, t.space_12),
                        )),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt(id)
                                .max_height(
                                    (rect.height() - t.form_header_height - t.space_24).max(40.0),
                                )
                                .show(ui, |ui| form(ui, add));
                        },
                    );
                });
        });
}
