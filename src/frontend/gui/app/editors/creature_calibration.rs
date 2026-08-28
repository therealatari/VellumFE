//! Creature calibrator: pick any pool creature image, click to place its
//! anchors — the built-in grounding pair (feet/head) plus arbitrary named
//! anchors for overlay layers (mouth, back, doll parts for wounds) — set
//! the floor footprint ellipse, world size, and lift, and save it all to
//! the image's sidecar (embedded in the PNG too, so the file travels
//! calibrated). The same metadata the field renderer consumes.

use std::collections::HashMap;

use super::super::VellumGuiApp;
use super::CalibrationOutcome;
use crate::config::pool::{self, CreatureFootprint, CreatureSidecar};
use crate::frontend::gui::image_store;
use crate::frontend::gui::skin::{self as gui_skin, SkinTexture};
use eframe::egui;

/// Anchor names offered up front; any other name can be added freely.
const SUGGESTED_ANCHORS: &[&str] = &["feet", "head", "mouth", "back", "saddle"];

struct CreatureChoice {
    label: String,
    pool_path: String,
    abs_path: std::path::PathBuf,
}

pub(crate) struct CreatureCalibrationState {
    choices: Vec<CreatureChoice>,
    selected: Option<usize>,
    texture: Option<egui::TextureHandle>,
    /// Working anchors, lowercase name -> image fractions.
    anchors: HashMap<String, [f32; 2]>,
    /// The anchor the next canvas click places.
    selected_anchor: String,
    new_anchor_name: String,
    footprint_on: bool,
    rx: f32,
    ry_auto: bool,
    ry: f32,
    size_on: bool,
    size: f32,
    lift_on: bool,
    lift: f32,
    error: Option<String>,
}

impl CreatureCalibrationState {
    /// Build the picker state from the pool listing. `None` (with the
    /// explanation in the outcome) when the pool has no creature images.
    pub(crate) fn open() -> (Option<Self>, CalibrationOutcome) {
        let mut outcome = CalibrationOutcome::default();
        // Deep listing: variant folders (creatures/<noun>/<variant>/) are
        // below the generic scanner's depth.
        let choices: Vec<CreatureChoice> = pool::list_creature_images()
            .into_iter()
            .map(|image| CreatureChoice {
                label: image.display_label(),
                pool_path: image.pool_path.clone(),
                abs_path: image.abs_path.clone(),
            })
            .collect();
        if choices.is_empty() {
            outcome.messages.push(
                "No creature images in the pool (global/images/creatures/). Drop PNGs there \
                 or install some with .jinx, then calibrate."
                    .to_owned(),
            );
            return (None, outcome);
        }
        let selected = (choices.len() == 1).then_some(0);
        let mut state = CreatureCalibrationState {
            choices,
            selected,
            texture: None,
            anchors: HashMap::new(),
            selected_anchor: "feet".to_owned(),
            new_anchor_name: String::new(),
            footprint_on: false,
            rx: 0.35,
            ry_auto: true,
            ry: 0.35 * 0.24,
            size_on: false,
            size: 1.0,
            lift_on: false,
            lift: 0.1,
            error: None,
        };
        if let Some(index) = selected {
            load_creature_choice(&mut state, index);
        }
        (Some(state), outcome)
    }

    /// Render the calibrator window for one frame. Sets `closed` in the
    /// outcome when the user dismissed the window.
    pub(crate) fn ui(&mut self, ctx: &egui::Context) -> CalibrationOutcome {
        let mut outcome = CalibrationOutcome::default();
        let state = self;
        state.ensure_texture(ctx);
        let mut open = true;
        let mut save_request = false;
        let mut load_request: Option<usize> = None;

        egui::Window::new("Creature Calibration")
            .id(egui::Id::new("gui_creature_calibration"))
            .order(egui::Order::Foreground)
            .open(&mut open)
            .default_width(640.0)
            .default_height(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    "Click the sprite to place the selected anchor. feet grounds the sprite \
                     on the field; named anchors position status/wound overlay layers.",
                );
                let selected_label = state
                    .selected
                    .map(|index| state.choices[index].label.clone())
                    .unwrap_or_else(|| "Pick a creature".to_owned());
                egui::ComboBox::from_label("Creature image")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for (index, choice) in state.choices.iter().enumerate() {
                            if ui
                                .selectable_label(state.selected == Some(index), &choice.label)
                                .clicked()
                            {
                                load_request = Some(index);
                            }
                        }
                    });

                let (Some(texture), Some(_)) = (&state.texture, state.selected) else {
                    return;
                };
                let sprite = SkinTexture {
                    texture: texture.id(),
                    size: texture.size_vec2(),
                };
                ui.separator();

                ui.horizontal_top(|ui| {
                    // Anchor list: suggested + present + add-your-own.
                    ui.vertical(|ui| {
                        ui.set_width(160.0);
                        let mut names: Vec<String> =
                            SUGGESTED_ANCHORS.iter().map(|s| s.to_string()).collect();
                        for key in state.anchors.keys() {
                            if !names.iter().any(|n| n.eq_ignore_ascii_case(key)) {
                                names.push(key.clone());
                            }
                        }
                        names.sort();
                        for name in names {
                            let placed = state.anchors.contains_key(&name);
                            let label = if placed {
                                format!("{name} \u{2022}")
                            } else {
                                name.clone()
                            };
                            if ui
                                .selectable_label(state.selected_anchor == name, label)
                                .clicked()
                            {
                                state.selected_anchor = name;
                            }
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let field = egui::TextEdit::singleline(&mut state.new_anchor_name)
                                .hint_text("new anchor")
                                .desired_width(90.0);
                            let submitted = ui.add(field).lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if (ui.button("+").clicked() || submitted)
                                && !state.new_anchor_name.trim().is_empty()
                            {
                                state.selected_anchor =
                                    state.new_anchor_name.trim().to_ascii_lowercase();
                                state.new_anchor_name.clear();
                            }
                        });
                        if ui
                            .button("Remove anchor")
                            .on_hover_text("Drop the selected anchor's placement")
                            .clicked()
                        {
                            let key = state.selected_anchor.to_ascii_lowercase();
                            state.anchors.remove(&key);
                        }
                    });

                    // Sprite canvas: anchors, ground line, footprint.
                    const CONTROLS_HEIGHT: f32 = 150.0;
                    let avail = ui.available_size();
                    let canvas = egui::Vec2::new(
                        avail.x.max(160.0),
                        (avail.y - CONTROLS_HEIGHT).max(200.0),
                    );
                    let (rect, response) = ui.allocate_exact_size(canvas, egui::Sense::click());
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
                    let dest = gui_skin::sprite_dest(&sprite, rect);
                    gui_skin::paint_sprite(&painter, dest, &sprite, egui::Color32::WHITE);

                    let at = |anchor: [f32; 2]| {
                        dest.min
                            + egui::Vec2::new(anchor[0] * dest.width(), anchor[1] * dest.height())
                    };
                    let feet = state
                        .anchors
                        .get("feet")
                        .copied()
                        .unwrap_or([0.5, 0.95]);

                    // Ground line through the feet anchor.
                    let ground_y = at(feet).y;
                    painter.line_segment(
                        [
                            egui::pos2(dest.min.x, ground_y),
                            egui::pos2(dest.max.x, ground_y),
                        ],
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(120, 200, 120, 160),
                        ),
                    );
                    // Footprint ellipse on the ground line.
                    if state.footprint_on {
                        let rx = state.rx * dest.width();
                        let ry = if state.ry_auto {
                            state.rx * 0.24
                        } else {
                            state.ry
                        } * dest.width();
                        let center = egui::pos2(at(feet).x, ground_y);
                        paint_ellipse(
                            &painter,
                            center,
                            rx,
                            ry,
                            egui::Stroke::new(
                                1.5,
                                egui::Color32::from_rgba_unmultiplied(120, 200, 120, 200),
                            ),
                        );
                    }

                    // All anchors; the selected one cross-haired.
                    let highlight = ui.visuals().hyperlink_color;
                    for (name, anchor) in &state.anchors {
                        let pos = at(*anchor);
                        let selected = name.eq_ignore_ascii_case(&state.selected_anchor);
                        let color = if selected {
                            highlight
                        } else {
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)
                        };
                        painter.circle_stroke(pos, 4.0, egui::Stroke::new(1.5, color));
                        painter.text(
                            pos + egui::vec2(6.0, -6.0),
                            egui::Align2::LEFT_BOTTOM,
                            name,
                            egui::FontId::proportional(11.0),
                            color,
                        );
                    }
                    if let Some(anchor) = state
                        .anchors
                        .get(&state.selected_anchor.to_ascii_lowercase())
                    {
                        let center = at(*anchor);
                        let stroke = egui::Stroke::new(1.0, highlight);
                        painter.line_segment(
                            [
                                egui::pos2(dest.min.x, center.y),
                                egui::pos2(dest.max.x, center.y),
                            ],
                            stroke,
                        );
                        painter.line_segment(
                            [
                                egui::pos2(center.x, dest.min.y),
                                egui::pos2(center.x, dest.max.y),
                            ],
                            stroke,
                        );
                    }

                    if response.clicked() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            if dest.contains(pos) && dest.width() > 0.0 && dest.height() > 0.0 {
                                let normalized = [
                                    ((pos.x - dest.min.x) / dest.width()).clamp(0.0, 1.0),
                                    ((pos.y - dest.min.y) / dest.height()).clamp(0.0, 1.0),
                                ];
                                let key = state.selected_anchor.to_ascii_lowercase();
                                state.anchors.insert(key, normalized);
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut state.footprint_on, "Footprint").on_hover_text(
                        "Floor ellipse for the contact shadow, centered on the feet anchor. \
                         Off = the generic standee shadow.",
                    );
                    if state.footprint_on {
                        ui.add(
                            egui::Slider::new(&mut state.rx, 0.05..=0.8)
                                .text("width")
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        );
                        ui.checkbox(&mut state.ry_auto, "auto depth");
                        if !state.ry_auto {
                            ui.add(
                                egui::Slider::new(&mut state.ry, 0.02..=0.5)
                                    .text("depth")
                                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                            );
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut state.size_on, "World size").on_hover_text(
                        "This creature's height in world units, overriding the family \
                         default — keeps art from different sources in scale.",
                    );
                    if state.size_on {
                        ui.add(
                            egui::DragValue::new(&mut state.size)
                                .range(0.05..=20.0)
                                .speed(0.05),
                        );
                    }
                    ui.checkbox(&mut state.lift_on, "Lift").on_hover_text(
                        "Ground clearance for a neutral pose that floats (wisps, spectres), \
                         as a fraction of the sprite height.",
                    );
                    if state.lift_on {
                        ui.add(
                            egui::Slider::new(&mut state.lift, 0.0..=1.0)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        );
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .button("Save")
                        .on_hover_text(
                            "Writes anchors, footprint, size and lift to the image's sidecar \
                             (and embeds them in the PNG) — the calibration travels with the file",
                        )
                        .clicked()
                    {
                        save_request = true;
                    }
                    if ui
                        .button("Reset")
                        .on_hover_text("Reload the last saved calibration")
                        .clicked()
                    {
                        if let Some(index) = state.selected {
                            load_request = Some(index);
                        }
                    }
                });
                if let Some(error) = &state.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });

        if let Some(index) = load_request {
            state.selected = Some(index);
            state.error = None;
            load_creature_choice(state, index);
        }
        if save_request {
            if let Some(index) = state.selected {
                let sidecar = CreatureSidecar {
                    kind: None, // the writer stamps it
                    anchors: state.anchors.clone(),
                    footprint: state.footprint_on.then(|| CreatureFootprint {
                        rx: state.rx,
                        ry: (!state.ry_auto).then_some(state.ry),
                        center: None, // feet-centered; the renderer's default
                    }),
                    size: state.size_on.then_some(state.size),
                    lift: state.lift_on.then_some(state.lift),
                };
                match pool::write_creature_sidecar(&state.choices[index].abs_path, &sidecar) {
                    Ok(()) => {
                        state.error = None;
                        // The per-noun creature cache re-resolves on reload.
                        outcome.reload_art = true;
                        outcome.messages.push(format!(
                            "Creature calibration saved for '{}'.",
                            state.choices[index].pool_path
                        ));
                    }
                    Err(err) => state.error = Some(format!("Failed to save: {}", err)),
                }
            }
        }

        outcome.closed = !open;
        outcome
    }
}

impl VellumGuiApp {
    pub(in super::super) fn open_creature_calibration(&mut self) {
        if self.creature_calibration.is_some() {
            self.raise_editor(egui::Id::new("gui_creature_calibration"));
            return;
        }
        let (state, outcome) = CreatureCalibrationState::open();
        for message in outcome.messages {
            self.app_core.add_system_message(&message);
        }
        if let Some(state) = state {
            self.creature_calibration = Some(state);
        }
    }

    pub(in super::super) fn render_creature_calibration(&mut self, ctx: &egui::Context) {
        let Some(state) = self.creature_calibration.as_mut() else {
            return;
        };
        let outcome = state.ui(ctx);
        if outcome.closed {
            self.creature_calibration = None;
        }
        for message in outcome.messages {
            self.app_core.add_system_message(&message);
        }
        if outcome.reload_art {
            self.skin_state.force_reload();
        }
    }
}

fn load_creature_choice(state: &mut CreatureCalibrationState, index: usize) {
    let choice = &state.choices[index];
    // The texture reloads lazily from render (`ensure_texture` needs ctx).
    state.texture = None;
    let sidecar: CreatureSidecar = pool::read_sidecar(&choice.abs_path).unwrap_or_default();
    state.anchors = sidecar
        .anchors
        .iter()
        .map(|(name, anchor)| (name.to_ascii_lowercase(), *anchor))
        .collect();
    state.footprint_on = sidecar.footprint.is_some();
    if let Some(fp) = sidecar.footprint {
        state.rx = fp.rx;
        state.ry_auto = fp.ry.is_none();
        state.ry = fp.effective_ry();
    }
    state.size_on = sidecar.size.is_some();
    if let Some(size) = sidecar.size {
        state.size = size;
    }
    state.lift_on = sidecar.lift.is_some();
    if let Some(lift) = sidecar.lift {
        state.lift = lift;
    }
}

impl CreatureCalibrationState {
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
            &format!("creature-cal:{}", choice.pool_path),
            "creature calibration",
        );
        if self.texture.is_none() {
            self.error = Some(format!("Cannot load {}", choice.pool_path));
        }
    }
}

/// Stroke an axis-aligned ellipse (egui has no ellipse primitive).
fn paint_ellipse(
    painter: &egui::Painter,
    center: egui::Pos2,
    rx: f32,
    ry: f32,
    stroke: egui::Stroke,
) {
    const SEGMENTS: usize = 48;
    let points: Vec<egui::Pos2> = (0..=SEGMENTS)
        .map(|i| {
            let t = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            egui::pos2(center.x + rx * t.cos(), center.y + ry * t.sin())
        })
        .collect();
    painter.add(egui::Shape::line(points, stroke));
}
