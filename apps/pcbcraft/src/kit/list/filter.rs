//! The filter bar of a list: the field (`/` or ⌘F focuses it; ↓ or Enter
//! returns to the rows), one chip per term ("x ≥ 12 mm") whose × removes
//! the term, red error chips that say why a term is not read, and the
//! saved filters as a row of named chips (the star saves the current
//! query). Terms are read by [`super::query`], over the catalog grammar.
//!
//! Saved filters are per-user settings: the app publishes them into the
//! context ([`publish_saved`]) and persists what the bar changed
//! ([`take_changes`]), so a feature panel never touches settings itself.
use super::query::{Fields, Filter};
use super::tokens::ListTokens;
use egui::{FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// A saved filter: the list's stable name and its query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub list: String,
    pub query: String,
}

/// A change to the saved filters the app persists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedChange {
    Add(Saved),
    Remove(Saved),
}

#[derive(Clone, Default)]
struct Published(Vec<Saved>);

#[derive(Clone, Default)]
struct Outbox(Vec<SavedChange>);

fn saved_slot() -> egui::Id {
    egui::Id::unique("pcbcraft-list-saved-filters")
}

fn outbox_slot() -> egui::Id {
    egui::Id::unique("pcbcraft-list-saved-changes")
}

/// Make `saved` the filters every list's bar offers.
pub fn publish_saved(ctx: &egui::Context, saved: Vec<Saved>) {
    ctx.data_mut(|d| d.insert_temp(saved_slot(), Published(saved)));
}

/// What the bars changed since the last call, for the app to persist.
pub fn take_changes(ctx: &egui::Context) -> Vec<SavedChange> {
    ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<Outbox>(outbox_slot()).0))
}

fn saved_of(ctx: &egui::Context, list: &str) -> Vec<String> {
    let mut all = ctx
        .data(|d| d.get_temp::<Published>(saved_slot()))
        .unwrap_or_default()
        .0;
    // What this frame's bar already changed shows at once.
    let pending = ctx.data(|d| d.get_temp::<Outbox>(outbox_slot()).unwrap_or_default().0);
    for change in pending {
        match change {
            SavedChange::Add(s) if !all.contains(&s) => all.push(s),
            SavedChange::Remove(s) => all.retain(|x| *x != s),
            SavedChange::Add(_) => {}
        }
    }
    all.into_iter()
        .filter(|s| s.list == list)
        .map(|s| s.query)
        .collect()
}

fn send(ctx: &egui::Context, change: SavedChange) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Outbox>(outbox_slot()).0.push(change));
}

/// One list's filter bar.
#[derive(Clone, Copy, Debug)]
pub struct FilterBar<'a> {
    pub id: egui::Id,
    /// The list's stable name, for saved filters ("pcb.parts").
    pub list: &'a str,
    /// The field's placeholder ("Filter: x>=10 side:bottom pkg:0603").
    pub hint: &'a str,
    /// Offer saved filters (tables).
    pub saving: bool,
}

/// What the bar did.
#[derive(Clone, Debug, Default)]
pub struct FilterResponse {
    pub filter: Filter,
    pub changed: bool,
    /// ↓ or Enter in the field: give the keyboard back to the rows.
    pub to_rows: bool,
    pub has_focus: bool,
}

impl FilterBar<'_> {
    fn field_id(&self) -> egui::Id {
        self.id.with("filter-field")
    }

    /// Give the field the keyboard (`/` or ⌘F in the list).
    pub fn focus(&self, ctx: &egui::Context) {
        ctx.memory_mut(|m| m.request_focus(self.field_id()));
    }

    pub fn show(&self, ui: &mut Ui, text: &mut String, fields: &Fields) -> FilterResponse {
        let l = ListTokens::get(ui.ctx());
        let before = text.clone();
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, l.row_h + 2.0), Sense::hover());
        let field = rect.shrink2(vec2(l.pad_x - 2.0, 2.0));
        let focused = ui.memory(|m| m.has_focus(self.field_id()));
        ui.painter().rect_filled(field, 6.0, l.surface);
        ui.painter().rect_stroke(
            field,
            6.0,
            Stroke::new(1.0, if focused { l.focus } else { l.line }),
            egui::StrokeKind::Inside,
        );
        let icon = Rect::from_center_size(pos2(field.left() + 12.0, field.center().y), vec2(13.0, 13.0));
        super::super::icon(ui.painter(), icon.center(), 13.0, "zoom", 0.6);
        let star_w = if self.saving { field.height() } else { 0.0 };
        let edit_rect = Rect::from_min_max(
            pos2(icon.right() + 6.0, field.top() + 1.0),
            pos2(field.right() - star_w - 4.0, field.bottom() - 1.0),
        );
        let edit = ui.put(
            edit_rect,
            egui::TextEdit::singleline(text)
                .id(self.field_id())
                .frame(egui::Frame::NONE)
                .hint_text(self.hint)
                .font(FontId::monospace(l.fs_s))
                .char_limit(pcbcraft_core::catalog::MAX_QUERY)
                .desired_width(edit_rect.width()),
        );
        let label = format!("Filter {}", self.list);
        edit.widget_info(|| {
            egui::WidgetInfo::text_edit(true, before.clone(), text.as_str(), label.as_str())
        });
        let mut filter = Filter::parse(text, fields);
        if self.saving {
            let star = Rect::from_min_size(
                pos2(field.right() - star_w, field.top()),
                vec2(star_w, field.height()),
            );
            let r = ui.interact(star, self.id.with("save-filter"), Sense::click());
            let can = !text.trim().is_empty();
            super::super::icon(
                ui.painter(),
                star.center(),
                13.0,
                "star",
                if r.hovered() && can { 1.0 } else { 0.5 },
            );
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, can, "Save this filter"));
            if r.on_hover_text("Save this filter").clicked() && can {
                send(
                    ui.ctx(),
                    SavedChange::Add(Saved {
                        list: self.list.to_owned(),
                        query: text.trim().to_owned(),
                    }),
                );
            }
        }
        // The chips: one per term; an error chip says why.
        let mut remove = None;
        if !filter.chips.is_empty() {
            chip_rows(ui, &l, |ui| {
                for (i, chip) in filter.chips.iter().enumerate() {
                    if chip_ui(ui, &l, self.id.with(("chip", i)), &chip.label, chip.error.as_deref())
                    {
                        remove = Some(chip.span.clone());
                    }
                }
            });
        }
        if let Some(span) = remove {
            *text = filter.without(&span);
            filter = Filter::parse(text, fields);
        }
        if self.saving {
            let saved = saved_of(ui.ctx(), self.list);
            if !saved.is_empty() {
                let mut apply = None;
                let mut drop = None;
                chip_rows(ui, &l, |ui| {
                    for (i, query) in saved.iter().enumerate() {
                        let on = text.trim() == query;
                        match saved_chip(ui, &l, self.id.with(("saved", i)), query, on) {
                            Some(true) => apply = Some(query.clone()),
                            Some(false) => drop = Some(query.clone()),
                            None => {}
                        }
                    }
                });
                if let Some(query) = apply {
                    *text = query;
                    filter = Filter::parse(text, fields);
                }
                if let Some(query) = drop {
                    send(
                        ui.ctx(),
                        SavedChange::Remove(Saved {
                            list: self.list.to_owned(),
                            query,
                        }),
                    );
                }
            }
        }
        let to_rows = edit.lost_focus() && !text.is_empty();
        FilterResponse {
            changed: *text != before,
            filter,
            to_rows,
            has_focus: edit.has_focus(),
        }
    }
}

fn chip_rows(ui: &mut Ui, l: &ListTokens, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(l.pad_x - 2.0);
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        add(ui);
    });
    ui.add_space(2.0);
}

/// One query chip; returns whether its × was clicked.
fn chip_ui(ui: &mut Ui, l: &ListTokens, id: egui::Id, label: &str, error: Option<&str>) -> bool {
    let font = FontId::proportional(l.fs_s);
    let (ink, fill, border) = match error {
        Some(_) => {
            let (ink, soft) = l.severity(super::super::severity::Level::Error);
            (ink, soft, ink.gamma_multiply(0.35))
        }
        None => (l.fg, l.focus.gamma_multiply(0.12), l.focus.gamma_multiply(0.3)),
    };
    let g = ui.painter().layout_no_wrap(label.to_owned(), font, ink);
    let size = vec2(g.size().x + 8.0 + 18.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 5.0, fill);
    ui.painter()
        .rect_stroke(rect, 5.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter()
        .galley(pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0), g, ink);
    let x = Rect::from_center_size(pos2(rect.right() - 10.0, rect.center().y), vec2(16.0, 16.0));
    let close = ui.interact(x, id.with("remove"), Sense::click());
    if close.hovered() {
        ui.painter().rect_filled(x, 4.0, l.hover);
    }
    super::super::icon(ui.painter(), x.center(), 10.0, "close", 0.7);
    let name = format!("Remove filter {label}");
    close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let described = match error {
        Some(why) => format!("{label}: {why}"),
        None => label.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &described));
    if let Some(why) = error {
        response.on_hover_text(why);
    }
    close.on_hover_text(name).clicked()
}

/// One saved filter: a click applies it (`Some(true)`), its × forgets it
/// (`Some(false)`).
fn saved_chip(ui: &mut Ui, l: &ListTokens, id: egui::Id, query: &str, on: bool) -> Option<bool> {
    let font = FontId::proportional(l.fs_s);
    let ink = if on { l.focus } else { l.fg_dim };
    let g = ui.painter().layout_no_wrap(query.to_owned(), font, ink);
    let size = vec2(g.size().x + 8.0 + 16.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect, 5.0, l.surface);
    ui.painter().rect_stroke(
        rect,
        5.0,
        Stroke::new(1.0, if on { l.focus } else { l.line }),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(pos2(rect.left() + 7.0, rect.center().y - g.size().y / 2.0), g, ink);
    let x = Rect::from_center_size(pos2(rect.right() - 9.0, rect.center().y), vec2(14.0, 14.0));
    let close = ui.interact(x, id.with("forget"), Sense::click());
    if response.hovered() || close.hovered() || close.has_focus() {
        super::super::icon(ui.painter(), x.center(), 9.0, "close", 0.7);
    }
    let name = format!("Saved filter {query}");
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &name));
    let forget = format!("Forget saved filter {query}");
    close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &forget));
    if close.on_hover_text(&forget).clicked() {
        Some(false)
    } else if response.clicked() {
        Some(true)
    } else {
        None
    }
}
