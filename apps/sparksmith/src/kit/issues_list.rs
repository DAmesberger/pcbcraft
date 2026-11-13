//! The Issues list on the list system (list-system spec, direction A
//! "Ledger"): group headers that stick to the top of the card's one
//! scroll, one row per issue with its severity glyph and count at the end,
//! hover actions (Locate, Fix…, Waive…) that reserve no space, the
//! disclosed cause, fix and object chips, the inline waive reason, and the
//! Waived group with Restore.
//!
//! Every line is one list row high, so the list stays virtualised however
//! many issues it holds: a disclosed detail (or a waiver's reason, or a
//! refusal under the reason field) is laid out once at the list's width
//! and takes as many lines as it needs, each line painting its own slice
//! of that one text; object chips are packed into chip lines. Only open
//! rows are laid out ahead of the pass, and only the lines in view are
//! built. Keys go through the list's one reducer: ↑↓ move, ←→ fold a group
//! or disclose a row, Enter locates.
use super::list::{
    self, Action, Columns, Effect, Fold, Line, LineCtx, ListTokens, ListView, Row, RowKey,
    RowResponse,
};
use super::severity::{self, Level};
use egui::text::LayoutJob;
use egui::{FontId, Galley, Pos2, Rect, Response, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
use sparksmith_core::issues::IssueDetail;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// Bytes a waive reason may hold: core's bound on waiver text.
const MAX_REASON: usize = sparksmith_core::model::production::MAX_WAIVER_TEXT;

/// One group of the list: its header and how many rows it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueGroupView {
    /// The group's identity across frames (its fold, the sticky header).
    pub key: RowKey,
    pub level: Level,
    pub title: String,
    pub rows: usize,
    pub open: bool,
    /// A group-wide Fix, such as "Update PCB…" for "Not placed"; a line of
    /// its own under the open header.
    pub fix: Option<String>,
}

/// One issue row as drawn, borrowed from what the list shows.
#[derive(Clone, Copy, Debug)]
pub struct IssueRowView<'a> {
    pub level: Level,
    /// At most 80 characters (core's `MAX_TITLE`); cut to the row with the
    /// full text on hover.
    pub title: &'a str,
    /// Findings merged into the row; above one it shows after the glyph.
    pub count: usize,
    /// The Fix action's label, only when a typed command or a dialog that
    /// issues one resolves the issue.
    pub fix: Option<&'a str>,
    /// Whether the row offers Waive….
    pub waive: bool,
    pub detail: &'a IssueDetail,
    /// The user names of the objects the issue refers to: chips while the
    /// row is disclosed.
    pub objects: &'a [String],
}

/// What the list shows. Titles, keys and folds are asked of every row each
/// frame; the full view only of rows drawn or disclosed.
pub trait IssueSource {
    fn groups(&self) -> &[IssueGroupView];
    fn key(&self, group: usize, row: usize) -> RowKey;
    fn title(&self, group: usize, row: usize) -> &str;
    /// Whether the row's details are disclosed, or `None` when it has
    /// nothing to disclose.
    fn disclosed(&self, group: usize, row: usize) -> Option<bool>;
    fn row(&self, group: usize, row: usize) -> IssueRowView<'_>;
}

/// One waived finding: what it names and why it was accepted.
#[derive(Clone, Debug, PartialEq)]
pub struct WaivedView {
    pub key: RowKey,
    /// The objects it names, by their user names.
    pub name: String,
    pub reason: String,
}

/// The Waived group at the list's end, shown while it holds a finding.
#[derive(Clone, Copy, Debug)]
pub struct Waived<'a> {
    pub rows: &'a [WaivedView],
    pub open: bool,
}

/// The waive reason being typed under the row `(group, row)`.
#[derive(Debug)]
pub struct Reason<'a> {
    pub group: usize,
    pub row: usize,
    pub text: &'a mut String,
    /// The engine's refusal of the last Enter, shown under the field.
    pub error: Option<&'a str>,
    pub required: bool,
    /// Give the field the keyboard (the frame it opens or is refused).
    pub start: bool,
}

/// The id of the reason field: a refusal of the waive it sends comes back
/// to it (`refusal::Origin::Field`).
pub fn reason_id() -> egui::Id {
    egui::Id::unique("issues-waive-reason")
}

/// What a row asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowAction {
    /// The row, its Locate or Enter: select and frame every object.
    Locate,
    Fix,
    Waive,
    /// Disclose or hide the details.
    Toggle,
    /// An object chip: select and frame only that object.
    Object(usize),
    /// A right-click: where, for the row's menu.
    Menu(Pos2),
}

/// What happened in the list this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum IssueListEvent {
    /// A group header folded or opened.
    Group(usize),
    GroupFix(usize),
    Row {
        group: usize,
        row: usize,
        action: RowAction,
    },
    /// The Waived header folded or opened.
    WaivedFold,
    /// Restore the waived finding at this index.
    Restore(usize),
    /// Enter in the reason field: send the waiver.
    Commit,
    /// Esc in the reason field.
    Cancel,
}

#[derive(Clone, Debug, Default)]
pub struct IssueListOutput {
    pub events: Vec<IssueListEvent>,
    /// Lines built this frame (virtualisation evidence).
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "read by the virtualisation tests")
    )]
    pub built: usize,
    /// The list's whole height, for the card's wanted height.
    pub height: f32,
}

/// One line of the list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Item {
    Group(usize),
    GroupFix(usize),
    Row(usize, usize),
    /// A slice of a disclosed row's cause and fix.
    Detail(usize, usize, usize),
    /// A line of a disclosed row's object chips.
    Chips(usize, usize, usize),
    Reason(usize, usize),
    /// A slice of the refusal under the reason field.
    Refused(usize, usize, usize),
    WaivedGroup,
    Waived(usize),
    /// A slice of a waived finding's reason.
    WaivedReason(usize, usize),
}

/// Text laid out ahead of the pass, shown as `lines` slices.
struct Block {
    galleys: Vec<(f32, Arc<Galley>)>,
    lines: usize,
}

impl Block {
    fn new(ui: &Ui, l: &ListTokens, texts: &[(&str, egui::Color32)], width: f32) -> Option<Self> {
        let mut galleys = Vec::new();
        let mut y = 0.0;
        for (text, ink) in texts.iter().filter(|(t, _)| !t.is_empty()) {
            let mut job = LayoutJob::simple(
                (*text).to_owned(),
                FontId::proportional(l.fs_s),
                *ink,
                width,
            );
            job.wrap.max_rows = 12;
            job.wrap.overflow_character = Some(super::text::ELLIPSIS);
            let g = ui.painter().layout_job(job);
            let h = g.size().y;
            galleys.push((y, g));
            y += h + 2.0;
        }
        if galleys.is_empty() {
            return None;
        }
        let lines = ((y + 4.0) / l.row_h).ceil().max(1.0) as usize;
        Some(Self { galleys, lines })
    }

    /// Paint slice `slice` of the block into `rect` from `left`.
    fn paint(&self, ui: &Ui, l: &ListTokens, rect: Rect, left: f32, slice: usize) {
        let painter = ui.painter_at(rect);
        let height: f32 = self.galleys.last().map_or(0.0, |(y, g)| y + g.size().y);
        let top = rect.top() - slice as f32 * l.row_h
            + ((self.lines as f32 * l.row_h - height) / 2.0).max(0.0);
        for (y, g) in &self.galleys {
            let at = Rect::from_min_size(pos2(left, top + y), g.size());
            if at.intersects(rect) {
                painter.galley(at.min, g.clone(), l.fg);
            }
        }
    }

    fn text(&self) -> String {
        self.galleys
            .iter()
            .map(|(_, g)| g.text().to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A disclosed row's layout: its detail text and its chip lines.
struct Disclosed {
    detail: Option<Block>,
    /// The chips of each chip line, by index into the row's objects.
    chips: Vec<Range<usize>>,
}

fn chip_font(l: &ListTokens) -> FontId {
    FontId::proportional(l.fs_s)
}

fn chip_width(ui: &Ui, l: &ListTokens, name: &str, room: f32) -> f32 {
    let w = ui
        .painter()
        .layout_no_wrap(name.to_owned(), chip_font(l), l.fg)
        .size()
        .x;
    (w + 2.0 * CHIP_PAD).min(room).max(1.0)
}

/// Text inset inside a chip.
const CHIP_PAD: f32 = 7.0;

/// The chips that fit each line of `room`, in order.
fn pack(ui: &Ui, l: &ListTokens, names: &[String], room: f32) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut x = 0.0;
    for (i, name) in names.iter().enumerate() {
        let w = chip_width(ui, l, name, room);
        if i > start && x + w > room {
            out.push(start..i);
            start = i;
            x = 0.0;
        }
        x += w + l.gap * 0.5;
    }
    if start < names.len() {
        out.push(start..names.len());
    }
    out
}

/// A non-row line's response: hover only, so a click never selects it.
fn quiet(ui: &Ui, id: egui::Id, rect: Rect) -> RowResponse {
    let response = ui.interact(rect, id, Sense::hover());
    plain(response, rect)
}

fn plain(response: Response, rect: Rect) -> RowResponse {
    RowResponse {
        clicked: response.clicked(),
        double_clicked: false,
        hovered: response.hovered(),
        fold: false,
        action: None,
        grip: None,
        value_rect: rect,
        response,
    }
}

/// The keyboard cursor on a line that has no band.
fn ring(ui: &Ui, l: &ListTokens, rect: Rect, cursor: bool) {
    if cursor {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            2.0,
            Stroke::new(1.0, l.focus),
            egui::StrokeKind::Inside,
        );
    }
}

/// The grouped Issues list in a scroll `height` high: every group and
/// every row, never capped, then the Waived group.
pub fn issues_list(
    ui: &mut Ui,
    id: egui::Id,
    height: f32,
    src: &impl IssueSource,
    waived: Waived<'_>,
    mut reason: Option<Reason<'_>>,
) -> IssueListOutput {
    let l = ListTokens::get(ui.ctx());
    let width = ui.available_width();
    // Where row text starts: past the row's indent and fold slot.
    let text_x = l.pad_x + l.indent + l.fold_w + l.gap;
    let scroll = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin;
    let room = (width - text_x - l.pad_x - scroll).max(l.fs * 4.0);
    let groups = src.groups();
    let mut items: Vec<Item> = Vec::new();
    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut disclosed: HashMap<(usize, usize), Disclosed> = HashMap::new();
    let mut refused: Option<Block> = None;
    let mut reasons: HashMap<usize, Block> = HashMap::new();
    for (g, group) in groups.iter().enumerate() {
        items.push(Item::Group(g));
        lines.push(Line::header(group.key, &group.title, !group.open));
        if !group.open {
            continue;
        }
        if let Some(fix) = &group.fix {
            items.push(Item::GroupFix(g));
            lines.push(Line::item(RowKey::of((group.key, "fix")), fix).under(group.key));
        }
        for r in 0..group.rows {
            let key = src.key(g, r);
            let open = src.disclosed(g, r);
            items.push(Item::Row(g, r));
            lines.push(Line {
                fold: match open {
                    Some(true) => Fold::Open,
                    Some(false) => Fold::Folded,
                    None => Fold::Leaf,
                },
                ..Line::item(key, src.title(g, r)).under(group.key)
            });
            if let Some(reason) = reason.as_ref().filter(|x| x.group == g && x.row == r) {
                items.push(Item::Reason(g, r));
                lines.push(Line::item(RowKey::of((key, "reason")), "").under(key));
                let ink = severity::ink(Level::Error);
                refused = reason
                    .error
                    .and_then(|e| Block::new(ui, &l, &[(e, ink)], room));
                for i in 0..refused.as_ref().map_or(0, |b| b.lines) {
                    items.push(Item::Refused(g, r, i));
                    lines.push(Line::item(RowKey::of((key, "refused", i)), "").under(key));
                }
            }
            if open != Some(true) {
                continue;
            }
            let view = src.row(g, r);
            let detail = Block::new(
                ui,
                &l,
                &[
                    (view.detail.cause.as_deref().unwrap_or(""), l.fg_dim),
                    (view.detail.fix.as_deref().unwrap_or(""), l.fg),
                ],
                room,
            );
            let chips = pack(ui, &l, view.objects, room);
            for i in 0..detail.as_ref().map_or(0, |b| b.lines) {
                items.push(Item::Detail(g, r, i));
                lines.push(Line::item(RowKey::of((key, "detail", i)), "").under(key));
            }
            for i in 0..chips.len() {
                items.push(Item::Chips(g, r, i));
                lines.push(Line::item(RowKey::of((key, "chips", i)), "").under(key));
            }
            disclosed.insert((g, r), Disclosed { detail, chips });
        }
    }
    let waived_key = RowKey::of("issues-waived");
    if !waived.rows.is_empty() {
        items.push(Item::WaivedGroup);
        lines.push(Line::header(waived_key, "Waived", !waived.open));
        if waived.open {
            for (i, w) in waived.rows.iter().enumerate() {
                items.push(Item::Waived(i));
                lines.push(Line::item(w.key, &w.name).under(waived_key));
                let text = if w.reason.is_empty() {
                    "No reason given".to_owned()
                } else {
                    w.reason.clone()
                };
                if let Some(block) = Block::new(ui, &l, &[(&text, l.fg_dim)], room) {
                    for s in 0..block.lines {
                        items.push(Item::WaivedReason(i, s));
                        lines.push(Line::item(RowKey::of((w.key, "reason", s)), "").under(w.key));
                    }
                    reasons.insert(i, block);
                }
            }
        }
    }
    let index: HashMap<RowKey, usize> = lines.iter().enumerate().map(|(i, x)| (x.key, i)).collect();
    let view = ListView {
        role: egui::Role::Tree,
        multiple: false,
        editing: reason.is_some(),
        ..ListView::new(id, "Issues", list::Layout::Scrolled { height })
    };
    // A field that just opened (or was refused) shows whole, with the
    // refusal under it.
    if reason.as_ref().is_some_and(|r| r.start)
        && let Some(last) = items
            .iter()
            .rposition(|i| matches!(i, Item::Reason(..) | Item::Refused(..)))
    {
        view.reveal(ui.ctx(), last);
    }
    let mut events = Vec::new();
    let cols = Columns::default();
    let text_left = |rect: Rect| rect.left() + text_x;
    let shown = view.show(ui, &lines, |ui, rect, line, cx: LineCtx| {
        let item = index.get(&line.key).and_then(|i| items.get(*i)).copied();
        let line_id = cx.id.with(line.key);
        match item {
            Some(Item::Group(g)) => {
                let Some(group) = groups.get(g) else {
                    return quiet(ui, line_id, rect);
                };
                let response = list::row::header(
                    ui,
                    cx.id,
                    rect,
                    line.key,
                    &group.title,
                    group.rows,
                    !group.open,
                    cx.cursor,
                );
                plain(response, rect)
            }
            Some(Item::WaivedGroup) => {
                let response = list::row::header(
                    ui,
                    cx.id,
                    rect,
                    line.key,
                    "Waived",
                    waived.rows.len(),
                    !waived.open,
                    cx.cursor,
                );
                plain(response, rect)
            }
            Some(Item::GroupFix(g)) => {
                let label = groups.get(g).and_then(|x| x.fix.as_deref()).unwrap_or("");
                let name = [label];
                let row = Row {
                    depth: 1,
                    fold_slot: true,
                    icon: Some("update"),
                    selected: cx.selected,
                    cursor: cx.cursor,
                    role: egui::Role::Button,
                    ..Row::new(line.key, &name)
                };
                let r = list::row::paint(ui, cx.id, rect, &row, &cols);
                if r.clicked {
                    events.push(IssueListEvent::GroupFix(g));
                }
                r
            }
            Some(Item::Row(g, r)) => {
                let view = src.row(g, r);
                let name = [view.title];
                let locate = format!("Locate: {}", view.title);
                let fix = view.fix.map(|f| format!("{f}: {}", view.title));
                let waive = format!("Waive: {}", view.title);
                let mut actions = vec![Action::icon("fit", &locate)];
                let mut kinds = vec![RowAction::Locate];
                if let (Some(text), Some(label)) = (view.fix, fix.as_deref()) {
                    actions.push(Action::text(text, label));
                    kinds.push(RowAction::Fix);
                }
                if view.waive {
                    actions.push(Action::text("Waive\u{2026}", &waive));
                    kinds.push(RowAction::Waive);
                }
                let row = Row {
                    depth: 1,
                    fold_slot: true,
                    fold: line.fold,
                    name: &name,
                    trailing: Some((view.level, view.count)),
                    actions: &actions,
                    selected: cx.selected,
                    cursor: cx.cursor,
                    role: egui::Role::TreeItem,
                    ..Row::new(line.key, &name)
                };
                let response = list::row::paint(ui, cx.id, rect, &row, &cols);
                let action = if let Some(a) = response.action {
                    kinds.get(a).copied()
                } else if response.response.secondary_clicked() {
                    response
                        .response
                        .interact_pointer_pos()
                        .map(RowAction::Menu)
                } else if response.clicked && !response.fold {
                    Some(RowAction::Locate)
                } else {
                    None
                };
                if let Some(action) = action {
                    events.push(IssueListEvent::Row {
                        group: g,
                        row: r,
                        action,
                    });
                }
                response
            }
            Some(Item::Detail(g, r, slice)) => {
                let out = quiet(ui, line_id, rect);
                if let Some(block) = disclosed.get(&(g, r)).and_then(|d| d.detail.as_ref()) {
                    block.paint(ui, &l, rect, text_left(rect), slice);
                    if slice == 0 {
                        let text = block.text();
                        out.response
                            .widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, &text));
                    }
                }
                ring(ui, &l, rect, cx.cursor);
                out
            }
            Some(Item::Chips(g, r, at)) => {
                let out = quiet(ui, line_id, rect);
                let view = src.row(g, r);
                let range = disclosed
                    .get(&(g, r))
                    .and_then(|d| d.chips.get(at).cloned())
                    .unwrap_or(0..0);
                let mut x = text_left(rect);
                let right = rect.right() - l.pad_x;
                for i in range {
                    let Some(name) = view.objects.get(i) else {
                        continue;
                    };
                    let w = chip_width(ui, &l, name, (right - x).max(1.0));
                    if chip(ui, &l, line_id.with(i), rect, x, w, name) {
                        events.push(IssueListEvent::Row {
                            group: g,
                            row: r,
                            action: RowAction::Object(i),
                        });
                    }
                    x += w + l.gap * 0.5;
                }
                ring(ui, &l, rect, cx.cursor);
                out
            }
            Some(Item::Reason(g, r)) => {
                let out = quiet(ui, line_id, rect);
                if let Some(reason) = reason.as_mut() {
                    let title = src.title(g, r);
                    match field(ui, &l, rect, text_left(rect), reason, title) {
                        Some(true) => events.push(IssueListEvent::Commit),
                        Some(false) => events.push(IssueListEvent::Cancel),
                        None => {}
                    }
                }
                out
            }
            Some(Item::Refused(_, _, slice)) => {
                let out = quiet(ui, line_id, rect);
                if let Some(block) = &refused {
                    block.paint(ui, &l, rect, text_left(rect), slice);
                    if slice == 0 {
                        let text = block.text();
                        out.response
                            .widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, &text));
                    }
                }
                out
            }
            Some(Item::Waived(i)) => {
                let name = [line.name];
                let label = format!("Restore: {}", line.name);
                let actions = [Action::text("Restore", &label)];
                let row = Row {
                    depth: 1,
                    fold_slot: true,
                    actions: &actions,
                    selected: cx.selected,
                    cursor: cx.cursor,
                    role: egui::Role::TreeItem,
                    ..Row::new(line.key, &name)
                };
                let r = list::row::paint(ui, cx.id, rect, &row, &cols);
                if r.action == Some(0) {
                    events.push(IssueListEvent::Restore(i));
                }
                r
            }
            Some(Item::WaivedReason(i, slice)) => {
                let out = quiet(ui, line_id, rect);
                if let Some(block) = reasons.get(&i) {
                    block.paint(ui, &l, rect, text_left(rect), slice);
                    if slice == 0 {
                        let text = block.text();
                        out.response
                            .widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, &text));
                    }
                }
                ring(ui, &l, rect, cx.cursor);
                out
            }
            None => quiet(ui, line_id, rect),
        }
    });
    let at = |key: &RowKey| index.get(key).and_then(|i| items.get(*i)).copied();
    for effect in &shown.effects {
        let event = match effect {
            Effect::Fold(key, _) => match at(key) {
                Some(Item::Group(g)) => Some(IssueListEvent::Group(g)),
                Some(Item::Row(g, r)) => Some(IssueListEvent::Row {
                    group: g,
                    row: r,
                    action: RowAction::Toggle,
                }),
                Some(Item::WaivedGroup) => Some(IssueListEvent::WaivedFold),
                _ => None,
            },
            Effect::Activate(key) => match at(key) {
                Some(
                    Item::Row(g, r)
                    | Item::Detail(g, r, _)
                    | Item::Chips(g, r, _)
                    | Item::Reason(g, r)
                    | Item::Refused(g, r, _),
                ) => Some(IssueListEvent::Row {
                    group: g,
                    row: r,
                    action: RowAction::Locate,
                }),
                Some(Item::GroupFix(g)) => Some(IssueListEvent::GroupFix(g)),
                _ => None,
            },
            Effect::CancelEdit => Some(IssueListEvent::Cancel),
            _ => None,
        };
        events.extend(event);
    }
    IssueListOutput {
        events,
        built: shown.built,
        height: lines.len() as f32 * l.row_h,
    }
}

/// One object chip at `x`, `w` wide, in the line `rect`; returns a click.
fn chip(ui: &Ui, l: &ListTokens, id: egui::Id, rect: Rect, x: f32, w: f32, name: &str) -> bool {
    let h = (l.row_h - 8.0).max(l.chip_h);
    let at = Rect::from_min_size(pos2(x, rect.center().y - h / 2.0), vec2(w, h));
    let response = ui
        .interact(at, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut job = LayoutJob::simple_singleline(name.to_owned(), chip_font(l), l.fg);
    job.wrap.max_width = (w - 2.0 * CHIP_PAD).max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some(super::text::ELLIPSIS);
    let g = ui.painter().layout_job(job);
    let cut = g.elided;
    let fill = if response.hovered() {
        l.hover
    } else {
        l.surface_2
    };
    ui.painter().rect(
        at,
        h / 2.0,
        fill,
        Stroke::new(1.0, l.line_strong),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(at.center() - g.size() / 2.0, g, l.fg);
    if response.has_focus() {
        ui.painter().rect_stroke(
            at,
            h / 2.0,
            Stroke::new(1.0, l.focus),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, name));
    let response = super::text::hover(response, name, "", cut);
    response.clicked()
}

/// The reason field in the line `rect` from `left`: `Some(true)` on
/// Enter, `Some(false)` on Esc.
fn field(
    ui: &mut Ui,
    l: &ListTokens,
    rect: Rect,
    left: f32,
    reason: &mut Reason<'_>,
    title: &str,
) -> Option<bool> {
    let id = reason_id();
    let at = Rect::from_min_max(
        pos2(left, rect.top() + 3.0),
        pos2(
            (rect.right() - l.pad_x).max(left + 1.0),
            rect.bottom() - 3.0,
        ),
    );
    let hint = if reason.required {
        "Why it is acceptable (required)"
    } else {
        "Why it is acceptable"
    };
    let response = ui.put(
        at,
        egui::TextEdit::singleline(reason.text)
            .id(id)
            .hint_text(hint)
            .font(FontId::proportional(l.fs_s))
            .margin(vec2(6.0, 1.0))
            .char_limit(MAX_REASON)
            .desired_width(at.width()),
    );
    let label = format!("Reason to waive {title}");
    let text = reason.text.clone();
    response.widget_info(|| {
        let mut info = WidgetInfo::text_edit(true, "", &text, hint);
        info.label = Some(label.clone());
        info
    });
    let ink = if reason.error.is_some() {
        severity::ink(Level::Error)
    } else {
        l.focus
    };
    ui.painter()
        .rect_stroke(at, 5.0, Stroke::new(1.0, ink), egui::StrokeKind::Outside);
    if reason.start {
        response.request_focus();
        return None;
    }
    if !response.lost_focus() {
        return None;
    }
    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        Some(false)
    } else if enter {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "issues_list_tests.rs"]
pub(crate) mod tests;
