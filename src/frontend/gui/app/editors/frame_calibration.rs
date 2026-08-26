//! Frame calibrator: pick any pool frame image — including ones with no
//! sidecar yet, which the Appearance picker can't offer — drag the four
//! nine-slice guides (or type insets), preview the sliced frame live, and
//! save the geometry to the image's sidecar. A frame goes from "dropped a
//! PNG in the pool" to "assignable everywhere" without touching TOML.

use super::super::VellumGuiApp;
use crate::config::pool;
use crate::frontend::gui::image_store;
use crate::frontend::gui::skin::{self as gui_skin, ResolvedBorder, SkinTexture};
use eframe::egui;

/// One selectable pool frame.
struct FrameChoice {
    stem: String,
    pool_path: String,
    abs_path: std::path::PathBuf,
    has_sidecar: bool,
}

pub(in super::super) struct FrameCalibrationState {
    choices: Vec<FrameChoice>,
    selected: Option<usize>,
    /// Full-size texture of the selected frame, loaded outside the synced
    /// store (editor-lifetime only).
    texture: Option<egui::TextureHandle>,
    /// Slice insets in SOURCE pixels: [top, right, bottom, left].
    insets: [f32; 4],
    /// Derive the on-screen scale from the largest inset (the sidecar's
    /// `effective_scale` behavior) instead of storing an explicit one.
    auto_scale: bool,
    scale: f32,
    /// Guide currently being dragged (index into `insets`).
    dragging: Option<usize>,
    error: Option<String>,
}

impl FrameCalibrationState {
    fn effective_scale(&self) -> f32 {
        if self.auto_scale {
            let max_inset = self.insets.into_iter().fold(0.0_f32, f32::max);
            if max_inset > 0.0 {
                15.0 / max_inset
            } else {
                1.0
            }
        } else {
            self.scale
        }
    }
}

impl VellumGuiApp {
    pub(in super::super) fn open_frame_calibration(&mut self, initial: Option<String>) {
        if self.frame_calibration.is_some() && initial.is_none() {
            self.raise_editor(egui::Id::new("gui_frame_calibration"));
            return;
        }
        let choices: Vec<FrameChoice> = pool::list_category("frames")
            .into_iter()
            .map(|image| FrameChoice {
                stem: image.stem().to_owned(),
                pool_path: image.pool_path.clone(),
                abs_path: image.abs_path.clone(),
                has_sidecar: image.has_sidecar,
            })
            .collect();
        if choices.is_empty() {
            self.app_core.add_system_message(
                "No frame images in the pool (global/images/frames/). Drop PNGs there or \
                 install some with .jinx, then calibrate.",
            );
            return;
        }
        let selected = initial
            .as_deref()
            .and_then(|want| {
                choices
                    .iter()
                    .position(|c| c.stem.eq_ignore_ascii_case(want))
            })
            .or(if choices.len() == 1 { Some(0) } else { None });
        let mut state = FrameCalibrationState {
            choices,
            selected,
            texture: None,
            insets: [0.0; 4],
            auto_scale: true,
            scale: 1.0,
            dragging: None,
            error: None,
        };
        if let Some(index) = selected {
            load_frame_choice(&mut state, index);
        }
        self.frame_calibration = Some(state);
    }

    pub(in super::super) fn render_frame_calibration(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.frame_calibration.take() else {
            return;
        };
        state.ensure_texture(ctx);
        let mut open = true;
        let mut save_request = false;
        let mut load_request: Option<usize> = None;

        egui::Window::new("Frame Calibration")
            .id(egui::Id::new("gui_frame_calibration"))
            .order(egui::Order::Foreground)
            .open(&mut open)
            .default_width(680.0)
            .default_height(540.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    "Drag the four guides (or type insets) to mark the frame's fixed corners. \
                     Frames without saved geometry don't appear in the Appearance picker.",
                );
                let selected_label = state
                    .selected
                    .map(|index| {
                        let choice = &state.choices[index];
                        if choice.has_sidecar {
                            choice.stem.clone()
                        } else {
                            format!("{} (uncalibrated)", choice.stem)
                        }
                    })
                    .unwrap_or_else(|| "Pick a frame".to_owned());
                egui::ComboBox::from_label("Frame image")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for (index, choice) in state.choices.iter().enumerate() {
                            let label = if choice.has_sidecar {
                                choice.stem.clone()
                            } else {
                                format!("{} (uncalibrated)", choice.stem)
                            };
                            if ui
                                .selectable_label(state.selected == Some(index), label)
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

                // Side-by-side: source image with guides | live preview.
                const CONTROLS_HEIGHT: f32 = 110.0;
                let avail = ui.available_size();
                let panel = egui::Vec2::new(
                    (avail.x / 2.0 - 8.0).max(160.0),
                    (avail.y - CONTROLS_HEIGHT).max(200.0),
                );
                ui.horizontal_top(|ui| {
                    // Source canvas with draggable guides.
                    let (rect, response) =
                        ui.allocate_exact_size(panel, egui::Sense::click_and_drag());
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
                    let dest = gui_skin::sprite_dest(&sprite, rect);
                    gui_skin::paint_sprite(&painter, dest, &sprite, egui::Color32::WHITE);

                    // Guide positions in screen space. Insets are source
                    // pixels; dest is the aspect-fit rect.
                    let sx = dest.width() / sprite.size.x.max(1.0);
                    let sy = dest.height() / sprite.size.y.max(1.0);
                    let guide_pos = |guide: usize, insets: &[f32; 4]| -> (egui::Pos2, egui::Pos2) {
                        match guide {
                            0 => {
                                let y = dest.min.y + insets[0] * sy;
                                (egui::pos2(dest.min.x, y), egui::pos2(dest.max.x, y))
                            }
                            1 => {
                                let x = dest.max.x - insets[1] * sx;
                                (egui::pos2(x, dest.min.y), egui::pos2(x, dest.max.y))
                            }
                            2 => {
                                let y = dest.max.y - insets[2] * sy;
                                (egui::pos2(dest.min.x, y), egui::pos2(dest.max.x, y))
                            }
                            _ => {
                                let x = dest.min.x + insets[3] * sx;
                                (egui::pos2(x, dest.min.y), egui::pos2(x, dest.max.y))
                            }
                        }
                    };

                    // Drag: pick the nearest guide on press, follow while held.
                    if response.drag_started() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let mut best: Option<(usize, f32)> = None;
                            for guide in 0..4 {
                                let (a, b) = guide_pos(guide, &state.insets);
                                let d = if guide % 2 == 0 {
                                    (pos.y - a.y).abs()
                                } else {
                                    (pos.x - a.x).abs()
                                };
                                let _ = b;
                                if d <= 14.0 && best.is_none_or(|(_, bd)| d < bd) {
                                    best = Some((guide, d));
                                }
                            }
                            state.dragging = best.map(|(guide, _)| guide);
                        }
                    }
                    if response.drag_stopped() {
                        state.dragging = None;
                    }
                    if let (Some(guide), Some(pos)) =
                        (state.dragging, response.interact_pointer_pos())
                    {
                        let value = match guide {
                            0 => (pos.y - dest.min.y) / sy.max(1e-6),
                            1 => (dest.max.x - pos.x) / sx.max(1e-6),
                            2 => (dest.max.y - pos.y) / sy.max(1e-6),
                            _ => (pos.x - dest.min.x) / sx.max(1e-6),
                        };
                        let limit = if guide % 2 == 0 {
                            sprite.size.y
                        } else {
                            sprite.size.x
                        };
                        state.insets[guide] = value.clamp(0.0, limit / 2.0).round();
                    }

                    for guide in 0..4 {
                        let (a, b) = guide_pos(guide, &state.insets);
                        let active = state.dragging == Some(guide);
                        let color = if active {
                            ui.visuals().hyperlink_color
                        } else {
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180)
                        };
                        painter.line_segment([a, b], egui::Stroke::new(
                            if active { 2.0 } else { 1.0 },
                            color,
                        ));
                    }

                    // Live preview: the frame nine-sliced around a sample rect.
                    let (preview, _) = ui.allocate_exact_size(panel, egui::Sense::hover());
                    let painter = ui.painter().with_clip_rect(preview);
                    painter.rect_filled(preview, 4.0, ui.visuals().extreme_bg_color);
                    let border = ResolvedBorder {
                        texture: sprite.texture,
                        tex_size: sprite.size,
                        slice: state.insets,
                        scale: state.effective_scale().max(0.05),
                    };
                    gui_skin::paint_nine_slice(
                        &painter,
                        preview.shrink(16.0),
                        &border,
                        [true; 4],
                    );
                });

                ui.separator();
                ui.horizontal(|ui| {
                    for (label, guide) in
                        [("Top", 0usize), ("Right", 1), ("Bottom", 2), ("Left", 3)]
                    {
                        ui.label(label);
                        let limit = if guide % 2 == 0 {
                            sprite.size.y
                        } else {
                            sprite.size.x
                        } / 2.0;
                        ui.add(
                            egui::DragValue::new(&mut state.insets[guide])
                                .range(0.0..=limit)
                                .suffix("px"),
                        );
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut state.auto_scale, "Auto scale").on_hover_text(
                        "Sizes the border to ~15pt on screen from the largest inset. \
                         Uncheck to pin an exact source-pixels -> points multiplier.",
                    );
                    if !state.auto_scale {
                        ui.add(
                            egui::Slider::new(&mut state.scale, 0.01..=2.0)
                                .logarithmic(true)
                                .text("scale"),
                        );
                    } else {
                        ui.label(format!("(scale {:.3})", state.effective_scale()));
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .button("Save")
                        .on_hover_text(
                            "Writes the nine-slice geometry to the image's sidecar (and \
                             embeds it in the PNG) — the frame appears in every \
                             Appearance picker and travels with the file",
                        )
                        .clicked()
                    {
                        save_request = true;
                    }
                    if ui
                        .button("Reset")
                        .on_hover_text("Reload the last saved geometry")
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
            load_frame_choice(&mut state, index);
        }
        if save_request {
            if let Some(index) = state.selected {
                let choice = &state.choices[index];
                let scale = (!state.auto_scale).then_some(state.scale);
                match pool::write_frame_sidecar(&choice.abs_path, state.insets, scale) {
                    Ok(()) => {
                        state.error = None;
                        state.choices[index].has_sidecar = true;
                        // Assigned windows using this frame re-slice now.
                        self.skin_state.force_reload();
                        self.app_core.add_system_message(&format!(
                            "Frame geometry saved for '{}'.",
                            state.choices[index].stem
                        ));
                    }
                    Err(err) => state.error = Some(format!("Failed to save: {}", err)),
                }
            }
        }

        if open {
            self.frame_calibration = Some(state);
        }
    }
}

/// (Re)load one frame choice's texture and sidecar geometry into the state.
fn load_frame_choice(state: &mut FrameCalibrationState, index: usize) {
    let choice = &state.choices[index];
    // The texture reloads lazily from render (`ensure_texture` needs ctx).
    state.texture = None;
    state.dragging = None;
    let sidecar: Option<pool::FrameSidecar> = pool::read_sidecar(&choice.abs_path);
    match sidecar {
        Some(sidecar) => {
            state.insets = sidecar.slice.insets();
            match sidecar.scale {
                Some(scale) => {
                    state.auto_scale = false;
                    state.scale = scale;
                }
                None => {
                    state.auto_scale = true;
                    state.scale = sidecar.effective_scale();
                }
            }
        }
        None => {
            state.insets = [0.0; 4];
            state.auto_scale = true;
            state.scale = 1.0;
        }
    }
}

impl FrameCalibrationState {
    /// Ensure the selected image's texture is loaded (needs the egui ctx,
    /// so it runs from render rather than the picker click).
    pub(in super::super) fn ensure_texture(&mut self, ctx: &egui::Context) {
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
            &format!("frame-cal:{}", choice.pool_path),
            "frame calibration",
        );
        if self.texture.is_none() {
            self.error = Some(format!("Cannot load {}", choice.pool_path));
        }
    }
}
