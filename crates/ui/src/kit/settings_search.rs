//! Search across a settings page the kit draws; the Preferences window is SolidCraft's use of it.
//! The field is the Keyboard Shortcuts magnifier. A match is the control's own label, its
//! section, its page, or the value a dropdown is showing, scored the same way as command search.
//! English counts too, from the message catalog, so a new setting is found by being drawn —
//! nothing is registered for search.

use std::collections::{HashMap, HashSet};

use egui::{Context, Id, Pos2, Rect, Response, RichText, Sense, Ui, UiBuilder, Vec2, vec2};

use crate::tokens::Tokens;

const STATE: &str = "prefs_search_state";
const SCROLL: &str = "prefs_search_force_scroll";
const NAV: &str = "prefs_search_nav";
const SINK_NOW: &str = "prefs_search_key_sink";
const REVEAL_ID: &str = "prefs_search_reveal";

/// One keyboard-reachable match from the last finished pass.
#[derive(Clone)]
pub struct NavHit {
    pub page: usize,
    pub label: String,
}

#[derive(Clone)]
struct PaintedHit {
    page: usize,
    label: String,
}

#[derive(Clone, Default)]
struct NavStore {
    query: String,
    cursor: usize,
    hits: Vec<NavHit>,
    /// The cursor moved this frame, so this frame's passes scroll the selected row into view.
    scroll: bool,
}

/// The row to scroll to and outline after a result is chosen.
#[derive(Clone)]
pub struct Reveal {
    pub page: usize,
    pub label: String,
    pub until: f64,
    /// What the outlined row's name starts with: `{prefix}:revealed`.
    pub prefix: String,
}

#[derive(Clone, Default)]
struct State {
    live: bool,
    query: String,
    /// What the names this pass publishes start with: `{prefix}:search:current`, `{prefix}:revealed`.
    prefix: String,
    page: String,
    page_shown: bool,
    section: String,
    section_matches: bool,
    section_shown: String,
    block_matches: bool,
    force: u32,
    /// Set while a matching row draws, so its own slider or buttons stay visible.
    passthrough: u32,
    /// A heading matched, so the controls under it stay visible until the next heading.
    heading_open: bool,
    hiding: u32,
    /// Set while a row with no label of its own runs hidden: what its controls say stands in for it.
    unlabelled: u32,
    /// Rows drawn this page with no label, so each gets a key of its own.
    unlabelled_rows: u32,
    key: String,
    hits: u32,
    page_index: usize,
    /// Rows whose own text matched, in the order they were drawn this pass.
    nav: Vec<PaintedHit>,
    dirty: bool,
    child_matched: bool,
    memory_query: String,
    extras: HashMap<String, String>,
    open_blocks: HashSet<String>,
    seen_blocks: HashSet<String>,
}

/// Held until the preferences body finishes. A dropdown value learned while a row was hidden
/// asks egui to draw the body again in this frame, so the match appears at once.
pub struct SearchFrame {
    ctx: Context,
}

impl Drop for SearchFrame {
    fn drop(&mut self) {
        if finish(&self.ctx) {
            self.ctx.request_discard("preferences search");
        }
    }
}

fn finish(ctx: &Context) -> bool {
    let mut dirty = false;
    ctx.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<State>(Id::unique(STATE));
        dirty = state.live && state.dirty;
        state.live = false;
    });
    dirty
}

fn with<R>(ui: &Ui, f: impl FnOnce(&mut State) -> R) -> Option<R> {
    let mut out = None;
    ui.ctx().data_mut(|data| {
        let state = data.get_temp_mut_or_default::<State>(Id::unique(STATE));
        if state.live {
            out = Some(f(state));
        }
    });
    out
}

pub fn querying(ui: &Ui) -> bool {
    with(ui, |state| !state.query.is_empty()).unwrap_or(false)
}

pub struct SearchLayoutMark {
    page_shown: bool,
    section_shown: String,
    hits: u32,
    heading_open: bool,
    child_matched: bool,
}

pub fn shows_all(ui: &Ui) -> bool {
    with(ui, |state| state.section_matches || state.block_matches || state.force > 0 || state.heading_open || state.passthrough > 0).unwrap_or(false)
}

/// Stray notes, figures and buttons. A matching row opts out with [`push_passthrough`].
pub fn suppress_chrome(ui: &Ui) -> bool {
    querying(ui) && !shows_all(ui)
}

pub fn set_heading_open(ui: &Ui, open: bool) {
    with(ui, |state| state.heading_open = open);
}

pub fn force_scroll(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp(Id::unique(SCROLL))).unwrap_or(false)
}

pub struct BlockGate {
    key: String,
    discover: bool,
    pub show: bool,
    /// The block is a result in its own right: its label or hint, its section or a forced pass
    /// matches. Such a block counts as a hit even when the control inside it (a plain number
    /// field) does not report one.
    pub matched: bool,
}

/// A control inside a row with no label says what it is. Stored for the row, so the next pass can
/// match the row on those words and draw it, as it does for a dropdown's current value.
pub fn note_unlabelled(ui: &Ui, texts: &[&str]) {
    if !querying(ui) {
        return;
    }
    with(ui, |state| {
        if state.hiding == 0 || state.unlabelled == 0 || state.key.is_empty() {
            return;
        }
        let mut stored = state.extras.get(&state.key).cloned().unwrap_or_default();
        let was = extra_hit_text(state, Some(stored.as_str()));
        for text in texts.iter().filter(|text| !text.is_empty()) {
            if !stored.contains(text) {
                if !stored.is_empty() {
                    stored.push(' ');
                }
                stored.push_str(text);
            }
        }
        let now = extra_hit_text(state, Some(stored.as_str()));
        let key = state.key.clone();
        state.extras.insert(key, stored);
        if was != now {
            state.child_matched = true;
            state.dirty = true;
        }
    });
}

/// Draw this control. A miss returns false and draws nothing; the caller still runs the control
/// off-screen when it needs the widget's current value.
pub fn visible(ui: &mut Ui, texts: &[&str]) -> bool {
    let show = wanted(ui, texts);
    if show && querying(ui) {
        if with(ui, |state| state.hiding > 0).unwrap_or(false) {
            with(ui, |state| state.child_matched = true);
        } else {
            reveal(ui);
        }
    }
    show
}

fn wanted(ui: &Ui, texts: &[&str]) -> bool {
    if !querying(ui) {
        return true;
    }
    if shows_all(ui) || texts.iter().any(|text| text_hit(ui, text)) {
        return true;
    }
    with(ui, |state| {
        let key = state.key.clone();
        extra_hit(state, &key)
    })
    .unwrap_or(false)
}

pub fn reveal(ui: &mut Ui) {
    let Some((page, section, draw_page, draw_section)) = with(ui, |state| {
        if state.hiding > 0 {
            return None;
        }
        let draw_page = !state.page_shown && !state.page.is_empty();
        let draw_section = !state.section.is_empty() && state.section_shown != state.section;
        if draw_page {
            state.page_shown = true;
            state.section_shown.clear();
        }
        if draw_section {
            state.section_shown = state.section.clone();
        }
        state.hits = state.hits.saturating_add(1);
        Some((state.page.clone(), state.section.clone(), draw_page, draw_section))
    })
    .flatten() else {
        return;
    };
    let t = Tokens::get();
    if draw_page {
        ui.add_space(t.space_8);
        ui.label(RichText::new(page).strong().size(t.font_label).color(t.form_ink));
    }
    if draw_section {
        let response = ui.label(RichText::new(section.to_uppercase()).size(t.font_small).color(t.text_dim));
        // A setting with the section's name owns its search stop and reveal target.
        let setting_owns_label = with(ui, |state| state.key.rsplit('\n').next() == Some(section.as_str())).unwrap_or(false);
        if !setting_owns_label {
            remember(ui, &section, response.rect);
        }
    }
}

pub fn hidden_response(ui: &mut Ui) -> Response {
    let mut child = ui.new_child(UiBuilder::new().invisible().max_rect(Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0))));
    child.allocate_response(Vec2::ZERO, Sense::hover())
}

fn extra_hit(state: &State, key: &str) -> bool {
    extra_hit_text(state, state.extras.get(key).map(String::as_str))
}

fn extra_hit_text(state: &State, text: Option<&str>) -> bool {
    text.is_some_and(|text| setting_score(&state.query, text) > 0 || english_hit(&state.query, text))
}

/// Whether any of `texts` matches the current query. Does not draw a heading.
pub fn texts_hit(ui: &Ui, texts: &[&str]) -> bool {
    texts.iter().any(|text| text_hit(ui, text))
}

fn text_hit(ui: &Ui, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    with(ui, |state| text_matches(&state.query, text)).unwrap_or(false)
}

fn text_matches(query: &str, text: &str) -> bool {
    // A right-to-left label reaches here in display order (see `Catalog::shown`), while the query
    // is typed in reading order: compare the query in display order too, so a multi-word Hebrew
    // or Arabic label is found as it is typed (#10471; #7532 fixed only the test's half).
    setting_score(query, text) > 0
        || setting_score(&solidcraft_ui_text::bidi::visual(query), text) > 0
        || english_hit(query, text)
        || keywords(text).iter().any(|word| setting_score(query, word) > 0)
}

/// Prefix, the start of a word, or a run of at least three characters. Scattered letters are not
/// a match: "zoom" finds "Reverse zoom direction", and "rzd" does not. Marks are ignored, so
/// "preferences" finds "préférences".
fn setting_score(query: &str, text: &str) -> u32 {
    if query.chars().count() < 2 || text.is_empty() {
        return 0;
    }
    let query = fold(query);
    let text = fold(text);
    if text.starts_with(&query) {
        return 1000;
    }
    if text.split(|c: char| !c.is_alphanumeric()).any(|word| !word.is_empty() && word.starts_with(&query)) {
        return 800;
    }
    if query.chars().count() >= 3 && text.contains(&query) {
        return 600;
    }
    0
}

/// Words that find a setting even though they are not written on it.
fn keywords(text: &str) -> &'static [&'static str] {
    let sketch: &'static [&'static str] = match fold(text).as_str() {
        "center and midpoint guides" | "centre and midpoint guides" => {
            &["center and midpoint alignment guides", "centre and midpoint alignment guides"]
        }
        "guides from sketch points too" => &["alignment guides from end points and corners while drawing"],
        "snap to where two guides cross" => &["auto snap to recent centers and midpoints", "auto snap to recent centres and midpoints"],
        "line up with other sketch points" => &["line up with other sketch points while drawing"],
        "center-point arc follows the cursor" | "centre-point arc follows the cursor" => {
            &["center point arc follows the cursor's direction", "centre point arc follows the cursor's direction"]
        }
        "dimension new shapes fully" => &["dimension new shapes"],
        "place from the sketch origin" => &["also place them from the origin (fully constrained)"],
        "shortcut again switches mode" => &["pressing a tool's shortcut again switches its mode"],
        "ask for project options first" => &["project options dialog"],
        "return to the view before" => &["return to the previous view"],
        "keep" => &["keep the current view"],
        "unless turned" => &["return unless rotated"],
        "state icons" => &["sketch icons in the color of their state", "sketch icons in the colour of their state"],
        "conflict color" | "conflict colour" => &["conflicting geometry in the conflict color", "conflicting geometry in the conflict colour"],
        "hide glyphs zoomed out" => &["hide constraint glyphs when zoomed out"],
        "prefer movable points where they overlap" => &["prefer movable sketch points when points overlap"],
        "select other reaches hidden geometry" => &["select hidden geometry with select other"],
        "whole bodies" => &["click selects whole bodies"],
        _ => &[],
    };
    if !sketch.is_empty() {
        return sketch;
    }
    match fold(text).as_str() {
        "design" => &["design defaults"],
        "step values with arrows and wheel" => &["step values with the arrow keys and the wheel"],
        "show values beside their handles" => &["show values beside their handles in the view"],
        "files from unknown programs" => &["for files from unknown programs"],
        "hide the origin folder in new designs" => &["hide origin by default"],
        "update changed links when a tab comes back" => &["update changed linked designs when a tab comes back"],
        "double-click a component to activate it" => &["double-click a component to activate it, or to edit a linked one in place"],
        "insert design lists open designs first" => &["insert design lists the designs open in other tabs first"],
        "snap to centers" | "snap to centres" => &["snap to face and edge centers", "snap to face and edge centres", "hole snapping"],
        "move by click or drag" => &["move holes by clicking or dragging"],
        "use joint motion handles for move" => &["use joint motion handles for assembly move"],
        "near-axis step" => &["near-axis rotation step (°)", "coarse rotation"],
        "far-axis step" => &["far-axis rotation step (°)", "fine rotation"],
        "axis distances in measure" => &["show x/y/z distances"],
        "tool at the cursor" => &["show tool at cursor"],
        "theme" => &["dark", "light", "appearance"],
        "availability" => &["assistant controls"],
        "let ai agents connect" => &["let ai agents connect (control channel / mcp)"],
        "add solidcraft to ai apps" => &["make solidcraft available to ai apps on this computer"],
        "run slow commands in the background" => &["run agent commands in the background"],
        "connect the last agent at start" => &["connect the last-used agent when solidcraft starts"],
        "open the chat at start" => &["open the assistant panel when solidcraft starts"],
        "ask first" => &["ask before changes", "ask every time"],
        "make edits" => &["allow model edits", "auto-allow model edits"],
        "do everything" => &["allow everything"],
        "dock the chat" => &["agent chat side"],
        "show the agent's pointer" => &["show agent pointer"],
        "follow the camera" => &["turn the camera to sketches"],
        "speed" => &["motion speed"],
        "agent pointer" => &["agent cursor"],
        "assistant button in the application bar" => &["show assistant button in application bar"],
        "same as above" => &["as set above"],
        "anti-aliasing" | "antialiasing" => &["antialiasing", "msaa"],
        "curved surfaces" => &["display quality"],
        "perspective" => &["perspective view"],
        "quick measurement" => &["quick measurement in the view's corner"],
        "analysis" => &["show viewport analysis status"],
        "section" => &["show viewport section status"],
        "live details" => &["live viewport details"],
        "fold items" => &["fold viewport status items"],
        "reference geometry through bodies" => &["show reference geometry through bodies"],
        "single surface analysis" => &["use a single surface analysis"],
        "only while the view is still" => &["only when the view is still"],
        "supersampling" => &["supersampling (ssaa)"],
        "free" => &["free orbit"],
        "turn about the world's up axis" => &["turn around the vertical axis"],
        "reverse horizontal pan" => &["reverse horizontal pan direction"],
        "reverse vertical pan" => &["reverse vertical pan direction"],
        "reverse trackpad orbit" => &["reverse trackpad orbit direction"],
        "two-finger zoom while holding" => &["zoom with two-finger scroll while holding"],
        "show it in the 3d view" => &["show the 3d mouse in the 3d view"],
        "picture position" => &["position"],
        "turn the view while a sketch is edited" => &["turn sketches with the 3d mouse"],
        "model" => &["traced"],
        "view center" | "view centre" => &["classic"],
        "axes" | "axes (signed)" => &["axes", "signed axes", "axes (signed)"],
        "cube" | "cube (compact)" => &["cube", "cube (compact)"],
        "under cursor" => &["under the cursor"],
        _ => &[],
    }
}

/// Lower case, without the marks that distinguish "é" from "e".
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'œ' | 'Œ' => out.push_str("oe"),
            'æ' | 'Æ' => out.push_str("ae"),
            'ß' => out.push_str("ss"),
            c => out.extend(strip_mark(c).to_lowercase()),
        }
    }
    out
}

fn strip_mark(c: char) -> char {
    match c {
        'À'..='Å' | 'à'..='å' | 'Ā' | 'ā' | 'Ă' | 'ă' | 'Ą' | 'ą' => 'a',
        'Ç' | 'ç' | 'Ć' | 'ć' | 'Č' | 'č' => 'c',
        'È'..='Ë' | 'è'..='ë' | 'Ē' | 'ē' | 'Ĕ' | 'ĕ' | 'Ė' | 'ė' | 'Ę' | 'ę' | 'Ě' | 'ě' => 'e',
        'Ì'..='Ï' | 'ì'..='ï' | 'Ī' | 'ī' | 'Į' | 'į' => 'i',
        'Ñ' | 'ñ' | 'Ń' | 'ń' | 'Ň' | 'ň' => 'n',
        'Ò'..='Ö' | 'ò'..='ö' | 'Ō' | 'ō' | 'Ő' | 'ő' => 'o',
        'Ù'..='Ü' | 'ù'..='ü' | 'Ū' | 'ū' | 'Ů' | 'ů' | 'Ű' | 'ű' | 'Ų' | 'ų' => 'u',
        'Ý' | 'ý' | 'Ÿ' | 'ÿ' => 'y',
        'Ž' | 'ž' => 'z',
        c => c,
    }
}

fn english_hit(query: &str, shown: &str) -> bool {
    use std::cell::RefCell;
    thread_local! {
        static MAP: RefCell<(String, HashMap<String, Vec<String>>)> = RefCell::new((String::new(), HashMap::new()));
    }
    MAP.with(|slot| {
        let lang = solidcraft_ui_text::language().to_string();
        let mut slot = slot.borrow_mut();
        if slot.0 != lang {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            let locale = solidcraft_ui_text::Locale::for_language(&lang);
            solidcraft_ui_text::each_english_message(|id, english| {
                if english.contains('{') {
                    return;
                }
                let shown = locale.text(id);
                if shown != english {
                    map.entry(shown.to_string()).or_default().push(english.to_string());
                }
            });
            *slot = (lang, map);
        }
        slot.1.get(shown).is_some_and(|lines| {
            lines.iter().any(|english| setting_score(query, english) > 0 || keywords(english).iter().any(|word| setting_score(query, word) > 0))
        })
    })
}

pub fn note_key_sink(ctx: &Context) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(SINK_NOW), true));
}

/// A row whose own words matched. Section titles and controls both call this once they have a rect.
pub fn remember(ui: &mut Ui, label: &str, rect: Rect) {
    remember_matched(ui, label, None, rect);
}

/// [`remember`] for a row whose hint can match too: a row found only by its hint is still a
/// stop for the arrow keys, and its label stays the target Enter opens.
pub fn remember_matched(ui: &mut Ui, label: &str, hint: Option<&str>, rect: Rect) {
    if !ui.is_visible() || !querying(ui) || label.is_empty() || !(text_hit(ui, label) || hint.is_some_and(|hint| text_hit(ui, hint))) {
        return;
    }
    if with(ui, |state| state.hiding > 0).unwrap_or(true) {
        return;
    }
    let (cursor, scroll) = ui.ctx().data(|data| {
        let store = data.get_temp::<NavStore>(Id::unique(NAV)).unwrap_or_default();
        (store.cursor, store.scroll)
    });
    let selected = with(ui, |state| {
        let selected = state.nav.len() == cursor;
        state.nav.push(PaintedHit { page: state.page_index, label: label.to_string() });
        selected.then(|| state.prefix.clone())
    })
    .flatten();
    let Some(prefix) = selected else { return };
    let t = Tokens::get();
    ui.painter().rect_stroke(rect, t.radius_sm, egui::Stroke::new(t.space_2, t.form_primary), egui::StrokeKind::Outside);
    // Only when Up/Down moved the cursor: following it every frame pins the list to the match.
    if scroll {
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
    }
    solidcraft_ui_registry::publish_handle(&format!("{prefix}:search:current"), rect.center());
}

impl Reveal {
    /// Whether the outline's time is up, on the same clock that set [`Reveal::until`].
    pub fn expired(&self) -> bool {
        now_ms() > self.until
    }
}

/// Scroll the opened setting into view and outline it until [`Reveal::until`].
pub fn follow(ui: &Ui, label: &str, rect: Rect) {
    let Some(reveal) = ui.data(|data| data.get_temp::<Reveal>(Id::unique(REVEAL_ID))) else { return };
    if reveal.expired() || fold(label) != fold(&reveal.label) {
        return;
    }
    let page = with(ui, |state| state.page_index);
    if page.is_some_and(|page| page != reveal.page) {
        return;
    }
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
    let t = Tokens::get();
    ui.painter().rect_stroke(rect, t.radius_sm, egui::Stroke::new(t.space_2, t.form_primary), egui::StrokeKind::Outside);
    solidcraft_ui_registry::publish_handle(&format!("{}:revealed", reveal.prefix), rect.center());
}

/// Wall clock in milliseconds, for how long a reveal stays outlined.
fn now_ms() -> f64 {
    use web_time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}
