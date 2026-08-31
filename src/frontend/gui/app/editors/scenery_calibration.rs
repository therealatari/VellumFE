//! Scenery calibrator: pick a scenery pool image, click to place its feet
//! anchor, set the world size, and drag the two EXCLUSION EDGES — the
//! lateral span (fractions of the image width) behind which the creature
//! solver never places anyone. Saved to the image's sidecar along with the
//! image aspect (so core can size the span without decoding the image).
//! Uncalibrated scenery renders fine but excludes nothing — the
//! calibrator IS the exclusion data.

use super::super::VellumGuiApp;
use super::CalibrationOutcome;
use crate::config::pool::{self, CreatureFootprint, CreatureSidecar};
use crate::frontend::gui::image_store;
use eframe::egui;

struct SceneryChoice {
    label: String,
    pool_path: String,
    abs_path: std::path::PathBuf,
}

pub(crate) struct SceneryCalibrationState {
    choices: Vec<SceneryChoice>,
    selected: Option<usize>,
    texture: Option<egui::TextureHandle>,
    /// Feet anchor in image fractions.
    feet: [f32; 2],
    /// World-unit height (the same scale creature cards use).
    size: f32,
    exclude_on: bool,
    /// Exclusion edges, fractions of the image width.
    exclude: [f32; 2],
    /// Optional contact-shadow footprint (used when a scene prop opts
    /// into `shadow = true`).
    footprint_on: bool,
    footprint_rx: f32,
    error: Option<String>,
}

impl SceneryCalibrationState {
    pub(crate) fn open() -> (Option<Self>, CalibrationOutcome) {
        let mut outcome = CalibrationOutcome::default();
        let choices: Vec<SceneryChoice> = pool::list_category("scenery")
            .into_iter()
            .map(|image| SceneryChoice {
                label: image.display_label(),
                pool_path: image.pool_path.clone(),
                abs_path: image.abs_path.clone(),
            })
            .collect();
        if choices.is_empty() {
            outcome.messages.push(
                "No scenery images in the pool (global/images/scenery/). Drop PNGs there, \
                 then calibrate."
                    .to_owned(),
            );
            return (None, outcome);
        }
        let selected = (choices.len() == 1).then_some(0);
        let mut state = SceneryCalibrationState {
            choices,
            selected,
            texture: None,
            feet: [0.5, 1.0],
            size: 1.0,
            exclude_on: false,
            exclude: [0.1, 0.9],
            footprint_on: false,
            footprint_rx: 0.35,
            error: None,
        };
        if let Some(index) = selected {
            load_scenery_choice(&mut state, index);
        }
        (Some(state), outcome)
    }

    pub(crate) fn ui(&mut self, ctx: &egui::Context) -> CalibrationOutcome {
        let mut outcome = CalibrationOutcome::default();
        self.ensure_texture(ctx);
        let mut open = true;
        let mut save_request = false;
        let mut load_request: Option<usize> = None;

        egui::Window::new("Scenery Calibration")
            .id(egui::Id::new("gui_scenery_calibration"))
            .order(egui::Order::Foreground)
            .open(&mut open)
            .default_width(560.0)
            .default_height(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let selected_label = self
                        .selected
                        .map(|i| self.choices[i].label.clone())
                        .unwrap_or_else(|| "(pick an image)".to_string());
                    egui::ComboBox::from_id_salt("scenery_cal_pick")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for (index, choice) in self.choices.iter().enumerate() {
                                if ui
                                    .selectable_label(self.selected == Some(index), &choice.label)
                                    .clicked()
                                {
                                    load_request = Some(index);
                                }
                            }
                        });
                    if self.selected.is_some() && ui.button("Save").clicked() {
                        save_request = true;
                    }
                });
                ui.weak(
                    "Click the image to place the feet anchor. The exclusion edges are the \
                     span the creature solver blocks BEHIND the prop — in front and beside \
                     stay open.",
                );
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.horizontal(|ui| {
                    ui.label("World size");
                    ui.add(
                        egui::DragValue::new(&mut self.size)
                            .speed(0.02)
                            .range(0.05..=20.0),
                    )
                    .on_hover_text("Height in world units — creature cards are ~1.2");
                    ui.checkbox(&mut self.exclude_on, "Exclusion zone");
                    ui.checkbox(&mut self.footprint_on, "Shadow footprint");
                });
                if self.exclude_on {
                    ui.horizontal(|ui| {
                        ui.label("Edges");
                        ui.add(egui::Slider::new(&mut self.exclude[0], 0.0..=1.0).text("left"));
                        ui.add(egui::Slider::new(&mut self.exclude[1], 0.0..=1.0).text("right"));
                    });
                    if self.exclude[1] < self.exclude[0] {
                        self.exclude.swap(0, 1);
                    }
                }
                if self.footprint_on {
                    ui.add(
                        egui::Slider::new(&mut self.footprint_rx, 0.05..=0.8)
                            .text("footprint rx (fraction of width)"),
                    );
                }

                let Some(texture) = &self.texture else {
                    ui.weak("(no image loaded)");
                    return;
                };
                let ts = texture.size_vec2();
                let avail = ui.available_size();
                let fit = (avail.x / ts.x).min(avail.y / ts.y).min(4.0).max(0.05);
                let (rect, response) = ui.allocate_exact_size(ts * fit, egui::Sense::click());
                ui.painter().image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        self.feet = [
                            ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
                            ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
                        ];
                    }
                }
                let painter = ui.painter();
                // Feet marker.
                let feet = egui::pos2(
                    rect.left() + self.feet[0] * rect.width(),
                    rect.top() + self.feet[1] * rect.height(),
                );
                painter.circle_stroke(feet, 5.0, egui::Stroke::new(2.0, egui::Color32::YELLOW));
                // Exclusion edges + the blocked band between them.
                if self.exclude_on {
                    let x0 = rect.left() + self.exclude[0] * rect.width();
                    let x1 = rect.left() + self.exclude[1] * rect.width();
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x0, rect.top()),
                            egui::pos2(x1, rect.bottom()),
                        ),
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(220, 60, 60, 26),
                    );
                    for x in [x0, x1] {
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 60, 60)),
                        );
                    }
                }
            });

        if let Some(index) = load_request {
            self.selected = Some(index);
            load_scenery_choice(self, index);
        }
        if save_request {
            self.save(&mut outcome);
        }
        outcome.closed = !open;
        outcome
    }

    fn save(&mut self, outcome: &mut CalibrationOutcome) {
        let Some(index) = self.selected else {
            return;
        };
        let choice = &self.choices[index];
        // Aspect from the loaded texture — recorded so core can size the
        // exclusion span without decoding the image.
        let aspect = self
            .texture
            .as_ref()
            .map(|t| {
                let ts = t.size_vec2();
                ts.x / ts.y.max(1.0)
            })
            .filter(|a| a.is_finite() && *a > 0.0);
        let mut sidecar: CreatureSidecar =
            pool::read_sidecar(&choice.abs_path).unwrap_or_default();
        sidecar
            .anchors
            .insert("feet".to_string(), self.feet);
        sidecar.size = Some(self.size);
        sidecar.exclude = self.exclude_on.then_some(self.exclude);
        sidecar.aspect = aspect;
        sidecar.footprint = self.footprint_on.then(|| CreatureFootprint {
            rx: self.footprint_rx,
            ry: None,
            center: None,
        });
        match pool::write_creature_sidecar(&choice.abs_path, &sidecar) {
            Ok(()) => {
                outcome
                    .messages
                    .push(format!("Scenery calibration saved for {}", choice.pool_path));
                outcome.reload_art = true;
            }
            Err(err) => {
                self.error = Some(format!("Save failed: {err:#}"));
            }
        }
    }

    fn ensure_texture(&mut self, ctx: &egui::Context) {
        if self.texture.is_some() {
            return;
        }
        let Some(index) = self.selected else {
            return;
        };
        let choice = &self.choices[index];
        self.texture = image_store::load_texture_file(
            ctx,
            &choice.abs_path,
            &format!("scenery-cal:{}", choice.pool_path),
            "scenery calibration",
        );
        if self.texture.is_none() {
            self.error = Some(format!("Cannot load {}", choice.pool_path));
        }
    }
}

fn load_scenery_choice(state: &mut SceneryCalibrationState, index: usize) {
    let choice = &state.choices[index];
    state.texture = None;
    state.error = None;
    let sidecar: CreatureSidecar = pool::read_sidecar(&choice.abs_path).unwrap_or_default();
    state.feet = sidecar
        .anchors
        .get("feet")
        .copied()
        .unwrap_or([0.5, 1.0]);
    state.size = sidecar.size.unwrap_or(1.0);
    state.exclude_on = sidecar.exclude.is_some();
    if let Some(exclude) = sidecar.exclude {
        state.exclude = exclude;
    }
    state.footprint_on = sidecar.footprint.is_some();
    if let Some(fp) = sidecar.footprint {
        state.footprint_rx = fp.rx;
    }
}

impl VellumGuiApp {
    pub(in super::super) fn open_scenery_calibration(&mut self) {
        if self.scenery_calibration.is_some() {
            self.raise_editor(egui::Id::new("gui_scenery_calibration"));
            return;
        }
        let (state, outcome) = SceneryCalibrationState::open();
        for message in outcome.messages {
            self.app_core.add_system_message(&message);
        }
        if let Some(state) = state {
            self.scenery_calibration = Some(state);
        }
    }

    pub(in super::super) fn render_scenery_calibration(&mut self, ctx: &egui::Context) {
        let Some(state) = self.scenery_calibration.as_mut() else {
            return;
        };
        let outcome = state.ui(ctx);
        if outcome.closed {
            self.scenery_calibration = None;
        }
        for message in outcome.messages {
            self.app_core.add_system_message(&message);
        }
        if outcome.reload_art {
            self.skin_state.force_reload();
            // Exclusion spans read the sidecar; force a re-project.
            self.app_core.reload_creature_field_files();
        }
    }
}
