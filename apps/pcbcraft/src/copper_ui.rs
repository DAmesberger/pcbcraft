use crate::viewport;

use pcbcraft_solidcraft::copper::{Board, Command, Geometry, Snapshot, ViewMode};
use solidcraft_ui_egui::{
    theme,
    ui_kit::{self, Button},
};
use std::sync::mpsc::{self, Receiver, Sender};

struct Update {
    result: Result<Snapshot, String>,
    geometry: Option<Vec<Geometry>>,
}

pub struct CopperUi {
    sender: Sender<Command>,
    receiver: Receiver<Update>,
    snapshot: Option<Snapshot>,
    viewport: viewport::Viewport,
    width: f64,
    pub status: String,
    path: String,
    pending: usize,
}

impl CopperUi {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        theme::set_dark(true);
        theme::apply(&cc.egui_ctx);
        let rs = cc.wgpu_render_state.as_ref().ok_or("wgpu is required")?;
        let viewport = viewport::Viewport::new(rs);
        let (sender, commands) = mpsc::channel();
        let (updates, receiver) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let mut board = match Board::fixture() {
                Ok(board) => board,
                Err(error) => {
                    let _ = updates.send(Update {
                        result: Err(error.to_string()),
                        geometry: None,
                    });
                    ctx.request_repaint();
                    return;
                }
            };
            let _ = updates.send(Update {
                result: board.snapshot().map_err(|e| e.to_string()),
                geometry: Some(board.geometry().to_vec()),
            });
            ctx.request_repaint();
            while let Ok(command) = commands.recv() {
                let revision = board.revision();
                let result = board.execute(command).map_err(|e| e.to_string());
                let geometry = (board.revision() != revision).then(|| board.geometry().to_vec());
                if updates.send(Update { result, geometry }).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Ok(Self {
            sender,
            receiver,
            snapshot: None,
            viewport,
            width: 0.5,
            status: "Building native copper…".into(),
            path: "copper.solidcraft".into(),
            pending: 1,
        })
    }
    fn send(&mut self, command: Command) {
        match self.sender.send(command) {
            Ok(()) => self.pending += 1,
            Err(e) => self.status = e.to_string(),
        }
    }
    pub fn receive(&mut self) {
        while let Ok(update) = self.receiver.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            match update.result {
                Ok(snapshot) => {
                    if let Some(geometry) = update.geometry {
                        self.viewport.set_geometry(geometry, snapshot.revision);
                    }
                    self.width = snapshot.trace_width_mm;
                    self.viewport.set_state(&snapshot);
                    self.status = "Ready".into();
                    self.snapshot = Some(snapshot);
                }
                Err(error) => self.status = error,
            }
        }
    }
    pub fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mode = self.snapshot.as_ref().map(|s| s.mode).unwrap_or_default();
            for (label, value) in [
                ("2D layout", ViewMode::Layout),
                ("3D board", ViewMode::Solid),
            ] {
                if ui_kit::button(
                    ui,
                    label,
                    if mode == value {
                        Button::Primary
                    } else {
                        Button::Normal
                    },
                )
                .clicked()
                {
                    self.send(Command::View { mode: value });
                }
            }
            if ui_kit::button(ui, "Fit", Button::Normal).clicked() {
                self.viewport.fit();
            }
            let undo = self.pending == 0 && self.snapshot.as_ref().is_some_and(|s| s.can_undo);
            let redo = self.pending == 0 && self.snapshot.as_ref().is_some_and(|s| s.can_redo);
            if ui_kit::button(ui, "Undo", Button::Enabled(undo)).clicked() {
                self.send(Command::Undo {});
            }
            if ui_kit::button(ui, "Redo", Button::Enabled(redo)).clicked() {
                self.send(Command::Redo {});
            }
        });
    }
    pub fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Board");
        ui.label("40 × 30 mm · 2 copper layers");
        ui.label("35 µm copper · 1.6 mm substrate");
        ui.separator();
        if let Some(snapshot) = &self.snapshot {
            let items = snapshot.items.clone();
            let selected = snapshot.selected;
            for item in &items {
                let text = format!("{}  ·  {}", item.label, item.layer);
                if ui_kit::button(
                    ui,
                    text,
                    if selected == Some(item.id) {
                        Button::SmallSelected
                    } else {
                        Button::Ghost
                    },
                )
                .clicked()
                {
                    self.send(Command::Select {
                        item: Some(item.id),
                    });
                }
            }
            ui.separator();
            ui.strong("Selection");
            if let Some(item) = items.iter().find(|i| Some(i.id) == selected) {
                ui.label(format!("{} · {}", item.kind, item.label));
                ui.label(format!("Net: {}", item.net));
                ui.label(format!("Layer: {}", item.layer));
                if ui_kit::button(ui, "Highlight net", Button::Normal).clicked() {
                    self.send(Command::HighlightNet {
                        net: Some(item.net.clone()),
                    });
                }
            } else {
                ui.label("Click copper to inspect it.");
            }
            if ui_kit::button(ui, "Clear highlight", Button::Small).clicked() {
                self.send(Command::HighlightNet { net: None });
                self.send(Command::Select { item: None });
            }
        }
        ui.separator();
        ui.strong("Trace A width");
        ui.add(
            egui::DragValue::new(&mut self.width)
                .range(0.2..=1.5)
                .speed(0.05)
                .suffix(" mm"),
        );
        if ui_kit::button(ui, "Apply width", Button::Enabled(self.pending == 0)).clicked()
            && let Some(s) = &self.snapshot
        {
            self.send(Command::Width {
                expected_revision: s.revision,
                width_mm: self.width,
            });
        }
        ui.separator();
        if ui
            .checkbox(
                &mut self.viewport.hide_substrate,
                "Hide substrate (inspect both layers)",
            )
            .changed()
        {
            self.viewport.refresh();
        }
        ui.label("Orbit: right drag · Pan: middle drag\nZoom: scroll · Select: left click");
        ui.separator();
        ui.strong("Native Solidcraft file");
        ui.text_edit_singleline(&mut self.path);
        ui.horizontal(|ui| {
            if ui_kit::button(ui, "Save as", Button::Enabled(self.pending == 0)).clicked() {
                self.send(Command::Save {
                    path: self.path.clone(),
                });
            }
            if ui_kit::button(ui, "Open", Button::Enabled(self.pending == 0)).clicked() {
                self.send(Command::Open {
                    path: self.path.clone(),
                });
            }
        });
        ui.small("Save as uses a new filename.");
    }
}

impl CopperUi {
    pub fn canvas(&mut self, ui: &mut egui::Ui) {
        if let Some(item) = self.viewport.show(ui) {
            self.send(Command::Select { item });
        }
    }
}
