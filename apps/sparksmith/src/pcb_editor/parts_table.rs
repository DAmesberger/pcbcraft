//! The Parts and Vias tables in the band under the PCB canvas (list-system
//! spec, "Parts and vias"; approved 2026-11-12). One row per instance (Ref,
//! Value, Package, Side, X, Y, Rot; an unplaced one says so) or per via
//! (Net, X, Y, Drill, Span), on the list system's table row.
//!
//! - The rows' text is built once per document revision ([`Model`]), and
//!   the filtered, sorted order once per query or sort ([`Shown`]); a frame
//!   builds only the rows it shows, so 10 000 vias cost a screenful.
//! - The table shares the editor's one selection: a row click selects the
//!   part as a canvas click does, and a canvas or 3D selection selects and
//!   reveals the rows. Enter or a double-click locates: select, reveal on
//!   the canvas without zooming, and ping.
//! - [`Hover`] is the one cross-highlight slot: whichever surface holds the
//!   pointer writes it each frame and the others paint it as an overlay.
//!   Neither selection nor hover rebuilds board geometry.
use super::PcbEditor;
use super::group::{Group, Pick};
use crate::kit::list::{
    Effect, Layout, Line, ListState, ListTokens, ListView, RowKey, Sort,
    filter::FilterBar,
    natural_cmp,
    query::{self, Field, Fields, Filter, Kind},
    table::{self, Cell, ColumnSpec, TableLayout},
    view as list_view,
};
use egui::Ui;
use sparksmith_core::{CopperLayer, Engine, Id, Layer, Point, Project, ViaSpan};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

/// Which table the band shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tab {
    #[default]
    Parts,
    Vias,
}

impl Tab {
    fn index(self) -> usize {
        match self {
            Tab::Parts => 0,
            Tab::Vias => 1,
        }
    }

    /// The list's stable name, for saved filters.
    fn list(self) -> &'static str {
        match self {
            Tab::Parts => "pcb.parts",
            Tab::Vias => "pcb.vias",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Parts => "Parts",
            Tab::Vias => "Vias",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Parts => "Filter: R1 side:top",
            Tab::Vias => "Filter: GND drill<0.3",
        }
    }
}

/// The surface that wrote the hovered object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HoverFrom {
    Table,
    /// The Layout canvas or PCB 3D.
    Board,
}

/// The one cross-highlight slot (spec §3): cleared at each frame's start
/// and written by whichever surface holds the pointer. The surfaces drawn
/// before the writer read the last frame's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Hover {
    now: Option<(Pick, HoverFrom)>,
    last: Option<(Pick, HoverFrom)>,
}

impl Hover {
    /// A new frame. Returns whether the hover changed during the last one,
    /// so the surfaces drawn before its writer paint it again.
    pub fn begin(&mut self) -> bool {
        let changed = self.now != self.last;
        self.last = self.now.take();
        changed
    }

    pub fn set(&mut self, pick: Pick, from: HoverFrom) {
        self.now = Some((pick, from));
    }

    /// What a surface other than `here` hovers.
    pub fn elsewhere(&self, here: HoverFrom) -> Option<Pick> {
        self.now
            .or(self.last)
            .filter(|(_, from)| *from != here)
            .map(|(pick, _)| pick)
    }
}

fn part_key(id: Id) -> RowKey {
    RowKey::of(("part", id))
}

fn via_key(id: Id) -> RowKey {
    RowKey::of(("via", id))
}

/// One instance as the Parts table shows it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PartRow {
    pub id: Id,
    reference: String,
    value: String,
    package: String,
    /// Where it sits: side, position and rotation; `None` when unplaced.
    placed: Option<(Layer, Point, f64)>,
    x: String,
    y: String,
    rot: String,
    search: String,
}

/// One via as the Vias table shows it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ViaRow {
    pub id: Id,
    net_id: Id,
    net: String,
    /// The copper its swatch shows: the top for a through via.
    layer: CopperLayer,
    at: Point,
    drill_mm: f64,
    x: String,
    y: String,
    drill: String,
    span: String,
    search: String,
}

fn mm(v: f64) -> String {
    format!("{v:.3} mm")
}

fn side_name(side: Layer) -> &'static str {
    match side {
        Layer::Top => "Top",
        Layer::Bottom => "Bottom",
    }
}

/// Both tables' rows, built once per document revision.
#[derive(Debug, Default)]
pub(super) struct Model {
    revision: Option<u64>,
    pub parts: Vec<PartRow>,
    pub vias: Vec<ViaRow>,
    /// The row of each key, per tab.
    index: [HashMap<RowKey, usize>; 2],
}

impl Model {
    /// The rows of `project`: every placed instance, and every unplaced one
    /// that has a package, in natural designator order; every via, by net
    /// and position.
    pub fn build(project: &Project, revision: u64) -> Self {
        let placements: HashMap<Id, &sparksmith_core::Placement> =
            project.placements.iter().map(|p| (p.instance, p)).collect();
        let mut parts: Vec<PartRow> = project
            .instances
            .iter()
            .filter_map(|instance| {
                let placement = placements.get(&instance.id);
                let package =
                    sparksmith_core::fabrication::assembly::package_name(project, instance);
                if placement.is_none() && package.is_none() {
                    return None;
                }
                let value = sparksmith_core::instances::value_text(&project.library, instance)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let placed = placement.map(|p| (p.layer, p.position, p.rotation_deg));
                let (x, y, rot) = match placed {
                    Some((_, at, rot)) => (mm(at[0]), mm(at[1]), format!("{rot:.0}\u{b0}")),
                    None => Default::default(),
                };
                let package = package.unwrap_or_default();
                let search = format!("{} {value} {package}", instance.designator);
                Some(PartRow {
                    id: instance.id,
                    reference: instance.designator.clone(),
                    value,
                    package,
                    placed,
                    x,
                    y,
                    rot,
                    search,
                })
            })
            .collect();
        parts.sort_by(|a, b| natural_cmp(&a.reference, &b.reference));
        // Nets in natural order, ranked once rather than compared per via.
        let mut names: Vec<(Id, &str)> = project
            .nets
            .iter()
            .map(|n| (n.id, n.name.as_str()))
            .collect();
        names.sort_by(|a, b| natural_cmp(a.1, b.1));
        let rank: HashMap<Id, (usize, &str)> = names
            .iter()
            .enumerate()
            .map(|(i, (id, name))| (*id, (i, *name)))
            .collect();
        let mut vias: Vec<(usize, ViaRow)> = project
            .vias
            .iter()
            .map(|v| {
                let (order, net) = rank.get(&v.net).copied().unwrap_or((usize::MAX, ""));
                let (layer, span) = match v.span {
                    ViaSpan::Through => (CopperLayer::Top, "Through".to_owned()),
                    ViaSpan::Between { from, to } => {
                        (from, format!("{} \u{2013} {}", from.name(), to.name()))
                    }
                };
                let search = format!("{net} {span}");
                (
                    order,
                    ViaRow {
                        id: v.id,
                        net_id: v.net,
                        net: net.to_owned(),
                        layer,
                        at: v.position,
                        drill_mm: v.drill,
                        x: mm(v.position[0]),
                        y: mm(v.position[1]),
                        drill: mm(v.drill),
                        span,
                        search,
                    },
                )
            })
            .collect();
        vias.sort_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then(a.at[0].total_cmp(&b.at[0]))
                .then(a.at[1].total_cmp(&b.at[1]))
        });
        let vias: Vec<ViaRow> = vias.into_iter().map(|(_, v)| v).collect();
        let index = [
            parts
                .iter()
                .enumerate()
                .map(|(i, p)| (part_key(p.id), i))
                .collect(),
            vias.iter()
                .enumerate()
                .map(|(i, v)| (via_key(v.id), i))
                .collect(),
        ];
        Self {
            revision: Some(revision),
            parts,
            vias,
            index,
        }
    }

    fn len(&self, tab: Tab) -> usize {
        match tab {
            Tab::Parts => self.parts.len(),
            Tab::Vias => self.vias.len(),
        }
    }

    fn key(&self, tab: Tab, row: usize) -> Option<RowKey> {
        match tab {
            Tab::Parts => self.parts.get(row).map(|p| part_key(p.id)),
            Tab::Vias => self.vias.get(row).map(|v| via_key(v.id)),
        }
    }

    fn name(&self, tab: Tab, row: usize) -> &str {
        match tab {
            Tab::Parts => self.parts.get(row).map_or("", |p| p.reference.as_str()),
            Tab::Vias => self.vias.get(row).map_or("", |v| v.net.as_str()),
        }
    }

    fn searchable(&self, tab: Tab, row: usize) -> &str {
        match tab {
            Tab::Parts => self.parts.get(row).map_or("", |p| p.search.as_str()),
            Tab::Vias => self.vias.get(row).map_or("", |v| v.search.as_str()),
        }
    }

    /// Row `row`'s value in query field `field`.
    fn query_cell(&self, tab: Tab, row: usize, field: usize) -> query::Cell {
        let text = |s: &str| query::Cell::Text(s.to_owned());
        match tab {
            Tab::Parts => {
                let Some(p) = self.parts.get(row) else {
                    return query::Cell::None;
                };
                match (field, p.placed) {
                    (0, _) => text(&p.reference),
                    (1, _) => text(&p.value),
                    (2, _) => text(&p.package),
                    (3, Some((side, ..))) => query::Cell::Choice(match side {
                        Layer::Top => 0,
                        Layer::Bottom => 1,
                    }),
                    (4, Some((_, at, _))) => query::Cell::Length(at[0]),
                    (5, Some((_, at, _))) => query::Cell::Length(at[1]),
                    (6, Some((_, _, rot))) => query::Cell::Angle(rot),
                    _ => query::Cell::None,
                }
            }
            Tab::Vias => {
                let Some(v) = self.vias.get(row) else {
                    return query::Cell::None;
                };
                match field {
                    0 => text(&v.net),
                    1 => query::Cell::Length(v.at[0]),
                    2 => query::Cell::Length(v.at[1]),
                    3 => query::Cell::Length(v.drill_mm),
                    4 => text(&v.span),
                    _ => query::Cell::None,
                }
            }
        }
    }

    /// Rows `a` and `b` by column `column`, ascending.
    fn compare(&self, tab: Tab, column: usize, a: usize, b: usize) -> Ordering {
        let num = |x: Option<f64>, y: Option<f64>| match (x, y) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            // Unplaced parts sort last.
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        match tab {
            Tab::Parts => {
                let (Some(p), Some(q)) = (self.parts.get(a), self.parts.get(b)) else {
                    return Ordering::Equal;
                };
                let at = |r: &PartRow, i: usize| r.placed.map(|(_, at, _)| at[i]);
                match column {
                    1 => natural_cmp(&p.value, &q.value),
                    2 => natural_cmp(&p.package, &q.package),
                    3 => p.placed.map(|x| x.0).cmp(&q.placed.map(|x| x.0)),
                    4 => num(at(p, 0), at(q, 0)),
                    5 => num(at(p, 1), at(q, 1)),
                    6 => num(p.placed.map(|x| x.2), q.placed.map(|x| x.2)),
                    _ => Ordering::Equal,
                }
                .then_with(|| natural_cmp(&p.reference, &q.reference))
            }
            Tab::Vias => {
                let (Some(p), Some(q)) = (self.vias.get(a), self.vias.get(b)) else {
                    return Ordering::Equal;
                };
                match column {
                    1 => p.at[0].total_cmp(&q.at[0]),
                    2 => p.at[1].total_cmp(&q.at[1]),
                    3 => p.drill_mm.total_cmp(&q.drill_mm),
                    4 => natural_cmp(&p.span, &q.span),
                    // The model's order is by net already.
                    _ => a.cmp(&b),
                }
                .then(a.cmp(&b))
            }
        }
    }
}

/// The columns of `tab`. Rot, Y, X and then Side leave a narrow row first.
fn specs(tab: Tab) -> Vec<ColumnSpec<'static>> {
    match tab {
        Tab::Parts => vec![
            ColumnSpec::text("Ref", 40.0, 0),
            ColumnSpec::text("Value", 56.0, 1),
            ColumnSpec::text("Package", 64.0, 2).flex(),
            ColumnSpec::text("Side", 56.0, 3),
            ColumnSpec::number("X", 64.0, 4),
            ColumnSpec::number("Y", 64.0, 5),
            ColumnSpec::number("Rot", 40.0, 6),
        ],
        Tab::Vias => vec![
            ColumnSpec::text("Net", 60.0, 0).flex(),
            ColumnSpec::number("X", 64.0, 2),
            ColumnSpec::number("Y", 64.0, 3),
            ColumnSpec::number("Drill", 56.0, 1),
            ColumnSpec::text("Span", 64.0, 4),
        ],
    }
}

/// The query fields of `tab`, one per column in column order.
fn fields(tab: Tab) -> Fields {
    Fields::new(match tab {
        Tab::Parts => vec![
            Field::new("ref", Kind::Text),
            Field::new("value", Kind::Text),
            Field::package("package"),
            Field::new("side", Kind::Choice(vec!["Top".into(), "Bottom".into()])),
            Field::new("x", Kind::Length),
            Field::new("y", Kind::Length),
            Field::new("rot", Kind::Angle),
        ],
        Tab::Vias => vec![
            Field::new("net", Kind::Text),
            Field::new("x", Kind::Length),
            Field::new("y", Kind::Length),
            Field::new("drill", Kind::Length),
            Field::new("span", Kind::Text),
        ],
    })
}

/// A row's cells, borrowing its text.
fn row_cells<'a>(model: &'a Model, tab: Tab, row: usize, l: &ListTokens) -> Vec<Cell<'a>> {
    match tab {
        Tab::Parts => model.parts.get(row).map_or_else(Vec::new, |p| {
            let side = p.placed.map_or(Cell::Empty, |(side, ..)| {
                Cell::Layer(l.layer(side), side_name(side))
            });
            let (x, y, rot) = if p.placed.is_some() {
                (Cell::Number(&p.x), Cell::Number(&p.y), Cell::Number(&p.rot))
            } else {
                (Cell::Dim("not placed"), Cell::Empty, Cell::Empty)
            };
            vec![
                Cell::Strong(&p.reference),
                Cell::Text(&p.value),
                Cell::Text(&p.package),
                side,
                x,
                y,
                rot,
            ]
        }),
        Tab::Vias => model.vias.get(row).map_or_else(Vec::new, |v| {
            let colour = crate::palette::board().copper_layer(v.layer);
            vec![
                Cell::Net(colour, &v.net),
                Cell::Number(&v.x),
                Cell::Number(&v.y),
                Cell::Number(&v.drill),
                Cell::Text(&v.span),
            ]
        }),
    }
}

/// The filtered and sorted rows of one tab, kept while the revision, the
/// query and the sort stay.
#[derive(Debug, Default)]
struct Shown {
    key: Option<(u64, String, Option<Sort>)>,
    order: Vec<usize>,
    /// The display position of each shown key.
    position: HashMap<RowKey, usize>,
}

/// The band's state: the tab, each tab's query, the cached model, shown
/// orders and fitted columns, and the canvas requests a Locate makes.
#[derive(Debug, Default)]
pub(super) struct PartsTable {
    pub tab: Tab,
    queries: [String; 2],
    model: Model,
    shown: [Shown; 2],
    /// The fitted columns per (revision, tab, width).
    fitted: Option<((u64, Tab, i32), TableLayout)>,
    /// The editor selection each tab last showed.
    synced: [Option<BTreeSet<RowKey>>; 2],
    /// A Locate asked the canvas to bring this point into view (no zoom).
    pub reveal: Option<Point>,
    /// Where and when a Locate pinged.
    pub ping: Option<(Point, f64)>,
    /// The last row located, for tests.
    #[cfg(test)]
    pub located: Option<Pick>,
}

/// How long a Locate's ping rings, in seconds (none with reduced motion).
pub(super) const PING_SECONDS: f32 = 0.6;

impl PartsTable {
    /// The rows of `revision`, built when it changed.
    pub fn refresh(&mut self, project: &Project, revision: u64) {
        if self.model.revision != Some(revision) {
            self.model = Model::build(project, revision);
        }
    }

    /// Bring the shown rows of `tab` up to `filter` and `sort`.
    fn update_shown(&mut self, tab: Tab, filter: &Filter, fields: &Fields, sort: Option<Sort>) {
        let revision = self.model.revision.unwrap_or_default();
        let key = Some((revision, filter.text.trim().to_owned(), sort));
        let model = &self.model;
        let shown = &mut self.shown[tab.index()];
        if shown.key != key {
            let mut order: Vec<usize> = (0..model.len(tab))
                .filter(|&row| {
                    filter.is_empty()
                        || filter.matches(fields, model.searchable(tab, row), |field| {
                            model.query_cell(tab, row, field)
                        })
                })
                .collect();
            if let Some(sort) = sort {
                order.sort_by(|&a, &b| {
                    let o = model.compare(tab, sort.column, a, b);
                    if sort.ascending { o } else { o.reverse() }
                });
            }
            shown.position = order
                .iter()
                .enumerate()
                .filter_map(|(at, &row)| Some((model.key(tab, row)?, at)))
                .collect();
            shown.order = order;
            shown.key = key;
        }
    }
}

/// The list's id per tab.
fn list_id(tab: Tab) -> egui::Id {
    egui::Id::unique(("pcb-parts-table", tab))
}

impl PcbEditor {
    /// A new frame for the cross-highlight slot; `true` when the hover
    /// changed during the last frame (paint once more).
    pub fn begin_hover(&mut self) -> bool {
        self.hover.begin()
    }

    /// What the table hovers, for PCB 3D to light (an overlay only).
    pub fn lit(&self) -> Group {
        let mut group = Group::default();
        if let Some(pick) = self.hover.elsewhere(HoverFrom::Board) {
            group.insert(pick);
        }
        group
    }

    /// The tab the band shows.
    #[cfg(test)]
    pub fn table_tab(&mut self, tab: Tab) {
        self.table.tab = tab;
    }

    /// The part the band's last Locate (Enter, double-click) named.
    #[cfg(test)]
    pub fn table_located(&self) -> Option<Id> {
        match self.table.located {
            Some(Pick::Part(id)) => Some(id),
            _ => None,
        }
    }

    /// The Parts and Vias band under the canvas: the tabs, the filter, the
    /// column captions, the virtualised rows and the footer.
    pub fn parts_panel(&mut self, ui: &mut Ui, engine: &Engine) {
        let ctx = ui.ctx().clone();
        let l = ListTokens::get(&ctx);
        let project = engine.project();
        self.table.refresh(project, engine.revision());
        let tab = self.table.tab;
        let fields = fields(tab);
        let specs = specs(tab);
        let bar = FilterBar {
            id: egui::Id::unique(("pcb-parts-filter", tab)),
            list: tab.list(),
            hint: tab.hint(),
            saving: true,
        };
        let mut text = std::mem::take(&mut self.table.queries[tab.index()]);
        let mut response = None;
        let mut picked_tab = None;
        let labels = [Tab::Parts, Tab::Vias]
            .map(|t| (t, format!("{} {}", t.label(), self.table.model.len(t))));
        // The tabs and the filter share a line when the filter keeps room
        // for its hint; a narrow band puts the filter under the tabs.
        let font = egui::FontId::proportional(l.fs_s);
        let spacing = ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
        let tabs_w: f32 = labels
            .iter()
            .map(|(_, label)| {
                ui.painter()
                    .layout_no_wrap(label.clone(), font.clone(), l.fg)
                    .size()
                    .x
                    + spacing
            })
            .sum();
        let hint_w = ui
            .painter()
            .layout_no_wrap(tab.hint().to_owned(), egui::FontId::monospace(l.fs_s), l.fg)
            .size()
            .x;
        // The field's icon, star and insets around the hint.
        let one_line = ui.available_width() - tabs_w >= hint_w + 3.0 * l.row_h;
        let mut tabs = |ui: &mut Ui| {
            for (t, label) in &labels {
                let tab_button =
                    ui.selectable_label(*t == tab, egui::RichText::new(label).size(l.fs_s));
                if tab_button.clicked() {
                    picked_tab = Some(*t);
                }
            }
        };
        if one_line {
            ui.horizontal_top(|ui| {
                tabs(ui);
                ui.vertical(|ui| response = Some(bar.show(ui, &mut text, &fields)));
            });
        } else {
            ui.horizontal(|ui| tabs(ui));
            response = Some(bar.show(ui, &mut text, &fields));
        }
        let response = response.unwrap_or_default();
        let view = ListView {
            role: egui::Role::Grid,
            filtered: !text.trim().is_empty(),
            highlighted: self.hover.elsewhere(HoverFrom::Table).and_then(|pick| {
                match (tab, pick) {
                    (Tab::Parts, Pick::Part(id)) => Some(part_key(id)),
                    (Tab::Vias, Pick::Via(id)) => Some(via_key(id)),
                    _ => None,
                }
            }),
            ..ListView::new(list_id(tab), tab.label(), Layout::Scrolled { height: 0.0 })
        };
        let mut state = ListState::load(&ctx, view.id);
        let width = ui.available_width();
        let revision = engine.revision();
        let fit_key = (revision, tab, width.round() as i32);
        if self.table.fitted.as_ref().map(|(k, _)| *k) != Some(fit_key) {
            let model = &self.table.model;
            let rows: Vec<Vec<Cell<'_>>> = (0..model.len(tab))
                .map(|row| row_cells(model, tab, row, &l))
                .collect();
            let layout = table::layout(ui, &specs, width, rows.iter().map(Vec::as_slice));
            self.table.fitted = Some((fit_key, layout));
        }
        let layout = self
            .table
            .fitted
            .as_ref()
            .map(|(_, layout)| layout.clone())
            .unwrap_or_default();
        if let Some(column) = table::header(ui, view.id.with("header"), &specs, &layout, state.sort)
        {
            state.sort = Some(Sort::clicked(state.sort, column));
            state.clone().store(&ctx, view.id);
        }
        let footer_h = l.row_h - 4.0;
        let height = (ui.available_height() - footer_h).max(l.row_h);
        let view = ListView {
            layout: Layout::Scrolled { height },
            ..view
        };
        // The editor's selection, when it changed elsewhere (the canvas,
        // 3D, the Browser), selects the rows and reveals the cursor row.
        let selected = self.selected();
        let keys: BTreeSet<RowKey> = match tab {
            Tab::Parts => selected.parts.iter().map(|id| part_key(*id)).collect(),
            Tab::Vias => selected.vias.iter().map(|id| via_key(*id)).collect(),
        };
        self.table
            .update_shown(tab, &response.filter, &fields, state.sort);
        let mut reveal = None;
        if self.table.synced[tab.index()].as_ref() != Some(&keys) {
            if state.selection != keys {
                let primary = match tab {
                    Tab::Parts => self.instance.map(part_key),
                    Tab::Vias => self.via.map(via_key),
                };
                let cursor = primary
                    .filter(|k| keys.contains(k))
                    .or_else(|| keys.iter().next().copied());
                state.selection = keys.clone();
                state.cursor = cursor;
                state.anchor = cursor;
                state.clone().store(&ctx, view.id);
                reveal = cursor;
            }
            self.table.synced[tab.index()] = Some(keys);
        }
        let model = &self.table.model;
        let shown = &self.table.shown[tab.index()];
        let position = &shown.position;
        if let Some(at) = reveal.and_then(|c| position.get(&c)) {
            view.reveal(&ctx, *at);
        }
        let lines: Vec<Line<'_>> = shown
            .order
            .iter()
            .filter_map(|&row| Some(Line::item(model.key(tab, row)?, model.name(tab, row))))
            .collect();
        let total = model.len(tab);
        let mut clear = false;
        let out = if total == 0 {
            match tab {
                Tab::Parts => list_view::empty(
                    ui,
                    "No parts on the board yet",
                    "Update the PCB from the schematic to place its parts.",
                ),
                Tab::Vias => list_view::empty(
                    ui,
                    "No vias on the board yet",
                    "The Via tool places one; routing adds them at layer changes.",
                ),
            }
            None
        } else if lines.is_empty() {
            clear = list_view::no_match(ui, text.trim());
            None
        } else {
            let highlighted = view.highlighted;
            Some(view.show(ui, &lines, |ui, rect, line, cx| {
                let row = model.index[tab.index()].get(&line.key).copied();
                let cells = row.map_or_else(Vec::new, |row| row_cells(model, tab, row, &l));
                let at = position.get(&line.key).copied().unwrap_or_default();
                table::row(
                    ui,
                    cx.id,
                    rect,
                    line.key,
                    &specs,
                    &layout,
                    &cells,
                    cx.selected,
                    highlighted == Some(line.key),
                    cx.cursor,
                    at,
                )
            }))
        };
        let selected_count = out.as_ref().map_or(0, |o| o.state.selection.len());
        let noun = match tab {
            Tab::Parts => "parts",
            Tab::Vias => "vias",
        };
        let mut footer = if lines.len() == total {
            format!("{total} {noun}")
        } else {
            format!("{} of {total} {noun}", lines.len())
        };
        if selected_count > 0 {
            footer.push_str(&format!(" \u{b7} {selected_count} selected"));
        }
        list_view::footer(ui, &footer);
        // What the rows did, applied once the model is no longer borrowed.
        let pick_of = |key: RowKey| -> Option<Pick> {
            let row = *self.table.model.index[tab.index()].get(&key)?;
            match tab {
                Tab::Parts => self.table.model.parts.get(row).map(|p| Pick::Part(p.id)),
                Tab::Vias => self.table.model.vias.get(row).map(|v| Pick::Via(v.id)),
            }
        };
        let mut select = None;
        let mut locate = None;
        let mut hovered = None;
        if let Some(out) = &out {
            hovered = out.hovered.and_then(pick_of);
            for effect in &out.effects {
                match effect {
                    Effect::Selected => {
                        let mut group = Group::default();
                        for pick in out.state.selection.iter().filter_map(|k| pick_of(*k)) {
                            group.insert(pick);
                        }
                        select = Some((group, out.state.selection.clone()));
                    }
                    Effect::Activate(key) => locate = pick_of(*key),
                    Effect::FocusFilter => bar.focus(&ctx),
                    Effect::ClearFilter => clear = true,
                    _ => {}
                }
            }
        }
        if response.to_rows {
            view.request_focus(&ctx);
        }
        if clear {
            text.clear();
        }
        self.table.queries[tab.index()] = text;
        if let Some(pick) = hovered {
            self.hover.set(pick, HoverFrom::Table);
        }
        if let Some((group, keys)) = select {
            self.set_group(group, project);
            self.table.synced[tab.index()] = Some(keys);
        }
        if let Some(pick) = locate {
            self.locate_row(&ctx, project, pick);
        }
        if let Some(t) = picked_tab {
            self.table.tab = t;
        }
    }

    /// Enter or a double-click on a row: select it, bring it into view on
    /// the canvas at the current zoom, and ping it.
    fn locate_row(&mut self, ctx: &egui::Context, project: &Project, pick: Pick) {
        #[cfg(test)]
        {
            self.table.located = Some(pick);
        }
        let at = match pick {
            Pick::Part(id) => {
                self.set_selection(Some(id), None, None);
                project.placement_of(id).map(|p| p.position)
            }
            Pick::Via(id) => {
                let via = project.via(id);
                self.select_via(id, via.map(|v| v.net));
                via.map(|v| v.position)
            }
            Pick::Track(_) | Pick::Fill(_) => None,
        };
        let keys = self.selected();
        self.table.synced[self.table.tab.index()] = Some(match self.table.tab {
            Tab::Parts => keys.parts.iter().map(|id| part_key(*id)).collect(),
            Tab::Vias => keys.vias.iter().map(|id| via_key(*id)).collect(),
        });
        if let Some(at) = at {
            self.table.reveal = Some(at);
            if !crate::kit::list::tokens::prefs(ctx).reduced_motion {
                self.table.ping = Some((at, ctx.input(|i| i.time)));
            }
        }
    }

    /// The cross-highlight on the Layout canvas: the part or via another
    /// surface hovers, and a Locate's ping. Painted over the cached board;
    /// nothing is rebuilt.
    pub(super) fn paint_cross_highlight(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        view: super::BoardView,
        project: &Project,
    ) {
        let l = ListTokens::get(ui.ctx());
        let stroke = egui::Stroke::new(2.0, l.xh_line);
        match self.hover.elsewhere(HoverFrom::Board) {
            Some(Pick::Part(id)) => {
                if let Some(polygon) = project
                    .placement_of(id)
                    .and_then(|p| super::body_polygon(project, p))
                {
                    let points: Vec<egui::Pos2> = polygon.iter().map(|p| view.screen(*p)).collect();
                    painter.add(egui::Shape::closed_line(points, stroke));
                }
            }
            Some(Pick::Via(id)) => {
                if let Some(via) = project.via(id) {
                    let r = (via.diameter as f32 * view.view.scale / 2.0).max(4.0) + 3.0;
                    painter.circle_stroke(view.screen(via.position), r, stroke);
                }
            }
            _ => {}
        }
        if let Some((at, started)) = self.table.ping {
            let t = (ui.input(|i| i.time) - started) as f32 / PING_SECONDS;
            if (0.0..1.0).contains(&t) {
                let r = 6.0 + 30.0 * t;
                painter.circle_stroke(
                    view.screen(at),
                    r,
                    egui::Stroke::new(2.0, l.sel_line.gamma_multiply(1.0 - t)),
                );
                ui.ctx().request_repaint();
            } else {
                self.table.ping = None;
            }
        }
    }

    /// PCB 3D's hover: the part or via under the pointer, for the table to
    /// cross-highlight. The ray is cast again only when the pointer moved.
    pub(super) fn hover_3d(
        &mut self,
        scene: &crate::scene::SceneController,
        pointer: Option<egui::Pos2>,
    ) {
        let Some(p) = pointer else {
            self.hover_3d_at = None;
            return;
        };
        let pick = match self.hover_3d_at {
            Some((at, pick)) if at == p => pick,
            _ => {
                let pick = scene.viewport.rect().and_then(|rect| {
                    let id = scene.viewport.pick(
                        f64::from(p.x - rect.left()),
                        f64::from(p.y - rect.top()),
                        f64::from(rect.width()),
                        f64::from(rect.height()),
                    )?;
                    let source = scene.source(id)?;
                    match (source.instance, source.via) {
                        (Some(instance), _) => Some(Pick::Part(instance)),
                        (None, Some(via)) => Some(Pick::Via(via)),
                        _ => None,
                    }
                });
                self.hover_3d_at = Some((p, pick));
                pick
            }
        };
        if let Some(pick) = pick {
            self.hover.set(pick, HoverFrom::Board);
        }
    }

    /// Hovering a part on the Layout canvas cross-highlights its row.
    pub(super) fn hover_board(&mut self, pick: Option<Pick>) {
        if let Some(pick) = pick {
            self.hover.set(pick, HoverFrom::Board);
        }
    }
}

#[cfg(test)]
#[path = "parts_table_tests.rs"]
mod tests;
