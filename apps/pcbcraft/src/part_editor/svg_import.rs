//! Import SVG… on the Symbol and Footprint tabs. The native picker (the
//! file service's) chooses the file; reading and conversion run off the UI
//! thread through `pcbcraft_io::artwork` and `pcbcraft_solidcraft::svg`; a
//! dialog shows the size and takes the scale (and, on a footprint, the
//! layer); the artwork is then added at the view centre with the core's
//! `library.edit_graphics` edit code, committed as one undo step, and
//! selected on the Symbol tab so it can be moved.
use crate::kit::severity::Level;
use pcbcraft_core::geometry::Point;
use pcbcraft_core::library::graphics::{DrawingLayer, SILK_STROKE_MM, SYMBOL_STROKE_MM};
use pcbcraft_solidcraft::svg::{self, Artwork};
use pcbcraft_ui::theme::Tokens;
use pcbcraft_ui::ui_kit::{self, Button};
use std::path::PathBuf;
use std::sync::{Arc, mpsc::Receiver};

/// The ribbon tool ID.
pub const IMPORT_SVG: &str = "import-svg";

/// Which tab asked for the import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Symbol,
    Footprint,
}

impl Target {
    fn fallback_stroke(self) -> f64 {
        match self {
            Self::Symbol => SYMBOL_STROKE_MM,
            Self::Footprint => SILK_STROKE_MM,
        }
    }
}

/// A converted file waiting for the author's scale.
struct Loaded {
    name: String,
    text: Arc<String>,
    /// Size at scale 1, in millimetres.
    size: Point,
    notes: Vec<String>,
}

enum State {
    Idle,
    Picking(Receiver<Option<PathBuf>>),
    Reading(Receiver<Result<Loaded, String>>),
    Choosing(Loaded),
    Converting(Receiver<Result<Artwork, String>>, Vec<String>),
}

/// Artwork ready to place on `target` (`layer` on a footprint).
pub(super) struct Placement {
    pub artwork: Artwork,
    pub target: Target,
    pub layer: DrawingLayer,
    pub notes: Vec<String>,
}

pub(super) struct SvgImport {
    state: State,
    target: Target,
    scale: f64,
    layer: DrawingLayer,
}

impl Default for SvgImport {
    fn default() -> Self {
        Self {
            state: State::Idle,
            target: Target::Symbol,
            scale: 1.0,
            layer: DrawingLayer::Silkscreen,
        }
    }
}

pub(super) enum Event {
    Place(Placement),
    Failed(String),
}

fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = sender.send(work());
        ctx.request_repaint();
    });
    receiver
}

impl SvgImport {
    /// Whether an import is under way (the ribbon button waits).
    pub fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Wait for the picker's answer; the dialog follows.
    pub fn start(&mut self, picked: Receiver<Option<PathBuf>>, target: Target) {
        if self.busy() {
            return;
        }
        self.target = target;
        self.state = State::Picking(picked);
    }

    /// Advance the import and show its dialog. Call once per frame.
    pub fn frame(&mut self, ctx: &egui::Context) -> Option<Event> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => None,
            State::Picking(picked) => match picked.try_recv() {
                Ok(Some(path)) => {
                    let fallback = self.target.fallback_stroke();
                    self.state = State::Reading(spawn(ctx, move || read(&path, fallback)));
                    None
                }
                Ok(None) => None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.state = State::Picking(picked);
                    None
                }
                Err(_) => Some(Event::Failed("The file picker closed unexpectedly.".into())),
            },
            State::Reading(reading) => match reading.try_recv() {
                Ok(Ok(loaded)) => {
                    self.scale = 1.0;
                    self.state = State::Choosing(loaded);
                    None
                }
                Ok(Err(error)) => Some(Event::Failed(error)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.state = State::Reading(reading);
                    None
                }
                Err(_) => Some(Event::Failed(
                    "Reading the SVG stopped unexpectedly.".into(),
                )),
            },
            State::Choosing(loaded) => self.dialog(ctx, loaded),
            State::Converting(converting, notes) => match converting.try_recv() {
                Ok(Ok(artwork)) => Some(Event::Place(Placement {
                    artwork,
                    target: self.target,
                    layer: self.layer,
                    notes,
                })),
                Ok(Err(error)) => Some(Event::Failed(error)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.state = State::Converting(converting, notes);
                    None
                }
                Err(_) => Some(Event::Failed(
                    "Converting the SVG stopped unexpectedly.".into(),
                )),
            },
        }
    }

    fn dialog(&mut self, ctx: &egui::Context, loaded: Loaded) -> Option<Event> {
        let mut choice = None;
        ui_kit::Window::new(egui::Id::unique("svg-import"), "import", "Import SVG")
            .required()
            .show_body(ctx, |ui| {
                let t = Tokens::get();
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(t.space_12 as i8, t.space_8 as i8))
                    .show(ui, |ui| {
                        ui.set_min_width(360.0);
                        crate::kit::form(ui, |ui| {
                            crate::kit::value_row(ui, "File", &loaded.name);
                            crate::kit::number_row(ui, "Scale", &mut self.scale, "", svg::SCALE);
                            let size = format!(
                                "{:.2} × {:.2} mm",
                                loaded.size[0] * self.scale,
                                loaded.size[1] * self.scale
                            );
                            crate::kit::value_row(ui, "Size", &size);
                            if self.target == Target::Footprint {
                                crate::kit::choice_row(
                                    ui,
                                    "Layer",
                                    &mut self.layer,
                                    &[
                                        (DrawingLayer::Silkscreen, "Silkscreen".to_owned()),
                                        (DrawingLayer::Assembly, "Assembly".to_owned()),
                                    ],
                                );
                            }
                            for note in &loaded.notes {
                                crate::kit::note(ui, note);
                            }
                            crate::kit::note(
                                ui,
                                "Placed at the view centre as editable lines, arcs, circles, rectangles and polygons.",
                            );
                            crate::kit::actions(ui, |ui| {
                                if crate::kit::primary(ui, "Import", true).clicked() {
                                    choice = Some(true);
                                }
                                if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                                    choice = Some(false);
                                }
                            });
                        });
                    });
            });
        match choice {
            Some(true) => {
                let text = Arc::clone(&loaded.text);
                let options = svg::Options {
                    scale: self.scale,
                    ..svg::Options::new(self.target.fallback_stroke())
                };
                let notes = loaded.notes.clone();
                self.state = State::Converting(
                    spawn(ctx, move || {
                        svg::convert(&text, &options).map_err(|e| e.to_string())
                    }),
                    notes,
                );
                None
            }
            Some(false) => None,
            None => {
                self.state = State::Choosing(loaded);
                None
            }
        }
    }
}

/// Read and convert at scale 1, for the size and notes.
fn read(path: &std::path::Path, fallback_stroke_mm: f64) -> Result<Loaded, String> {
    let text = pcbcraft_io::artwork::read_svg(path).map_err(|e| e.to_string())?;
    let artwork =
        svg::convert(&text, &svg::Options::new(fallback_stroke_mm)).map_err(|e| e.to_string())?;
    Ok(Loaded {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        text: Arc::new(text),
        size: artwork.size,
        notes: artwork.notes,
    })
}

impl super::PartEditor {
    /// Start Import SVG… with the file service's picker answer.
    pub fn import_svg(&mut self, picked: Receiver<Option<PathBuf>>) {
        let target = if self.view == super::View::Footprint {
            Target::Footprint
        } else {
            Target::Symbol
        };
        self.svg.start(picked, target);
    }

    /// The ribbon's Import SVG… button (Symbol Draw group, Footprint
    /// Artwork group); the app answers it with the file picker.
    pub(super) fn import_svg_tool(&self) -> crate::shell::Tool {
        let tip = if self.view == super::View::Footprint {
            "Import an SVG drawing onto the silkscreen or assembly layer at the view centre"
        } else {
            "Import an SVG drawing as editable graphics at the view centre"
        };
        crate::shell::Tool::new(IMPORT_SVG, "import", "Import SVG…")
            .enabled(self.draft.is_some() && !self.svg.busy())
            .tip(tip)
    }

    /// Advance a running import; place its artwork when it is ready.
    pub(super) fn svg_frame(&mut self, ctx: &egui::Context, engine: &pcbcraft_core::Engine) {
        match self.svg.frame(ctx) {
            Some(Event::Place(placement)) => {
                let grid = engine.drafts().grid;
                let step = match placement.target {
                    Target::Symbol => grid.symbol_mm(),
                    Target::Footprint => grid.footprint_mm(),
                };
                self.place_svg(placement, step);
            }
            Some(Event::Failed(error)) => {
                self.tell(Level::Error, format!("SVG not imported: {error}"));
            }
            None => {}
        }
    }

    /// The millimetre point at the centre of the tab's canvas, on the grid.
    fn view_centre(&self, target: Target, grid: f64) -> Point {
        let view = match target {
            Target::Symbol => self.symbol_view,
            Target::Footprint => self.footprint_view,
        };
        let [x, y] = view.map_or([0.0, 0.0], |v| {
            [
                -f64::from(v.pan.x) / f64::from(v.scale),
                f64::from(v.pan.y) / f64::from(v.scale),
            ]
        });
        let snap = |v: f64| {
            if grid > 0.0 {
                (v / grid).round() * grid
            } else {
                v
            }
        };
        [snap(x), snap(y)]
    }

    fn place_svg(&mut self, placement: Placement, grid: f64) {
        use pcbcraft_core::library::Definition;
        use pcbcraft_core::library::graphics::{
            GraphicTarget, MAX_GRAPHICS_PER_UNIT, MAX_VERTICES, Side, apply,
        };
        let at = self.view_centre(placement.target, grid);
        let unit = self
            .unit_index()
            .and_then(|i| self.draft.as_ref()?.symbol.units.get(i))
            .map(|u| u.id);
        let Some(set) = &mut self.draft else {
            return;
        };
        let (target, definition) = match placement.target {
            Target::Symbol => {
                let Some(unit) = unit else {
                    self.tell(Level::Error, "SVG not imported: the symbol has no unit");
                    return;
                };
                (
                    GraphicTarget::Unit(unit),
                    Definition::Symbol(set.symbol.clone()),
                )
            }
            Target::Footprint => (
                GraphicTarget::Footprint {
                    layer: placement.layer,
                    side: Side::Component,
                },
                Definition::Footprint(set.footprint.clone()),
            ),
        };
        let edited = placement
            .artwork
            .edits(target, at)
            .map_err(|e| e.to_string())
            .and_then(|edits| {
                apply(&definition, &Default::default(), &edits).map_err(|e| e.to_string())
            });
        let count = placement.artwork.items.len();
        match edited {
            Ok((Definition::Symbol(symbol), added)) => {
                let full = symbol
                    .units
                    .iter()
                    .any(|u| u.graphics.len() > MAX_GRAPHICS_PER_UNIT)
                    || symbol
                        .units
                        .iter()
                        .flat_map(|u| &u.graphics)
                        .map(|g| g.item.vertices())
                        .sum::<usize>()
                        > MAX_VERTICES;
                if full {
                    self.tell(
                        Level::Error,
                        format!(
                            "SVG not imported: a unit holds {MAX_GRAPHICS_PER_UNIT} graphics, \
                             {MAX_VERTICES} points"
                        ),
                    );
                    return;
                }
                set.symbol = symbol;
                self.drawing.arm(super::drawing::Tool::Select);
                self.selection = added.into_iter().map(super::Selected::Graphic).collect();
            }
            Ok((Definition::Footprint(footprint), _)) => set.footprint = footprint,
            Ok(_) => return,
            Err(error) => {
                self.tell(Level::Error, format!("SVG not imported: {error}"));
                return;
            }
        }
        self.tell(Level::Ok, format!("Imported {count} graphics from SVG"));
        for note in &placement.notes {
            self.tell(Level::Info, note.clone());
        }
    }
}
