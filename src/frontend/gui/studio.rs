//! Vellum Studio: standalone art-authoring shell. Boots straight into
//! eframe — no network, no AppCore — and rehosts the pool calibrators so
//! frames and creature sprites can be calibrated without launching the
//! game. The Stage (scene composition) lands in a later slice.

use anyhow::anyhow;
use eframe::egui;

use super::app::editors::{CalibrationOutcome, CreatureCalibrationState, FrameCalibrationState};
use super::app::{theme as app_theme, widgets};
use super::persistence::FontRef;
use super::skin;

/// Oldest status lines drop past this.
const STATUS_CAP: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StudioMode {
    Anchorer,
    Stage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnchorTab {
    Frames,
    Creatures,
}

pub struct StudioApp {
    skin_state: skin::SkinState,
    mode: StudioMode,
    anchor_tab: AnchorTab,
    frame_calibration: Option<FrameCalibrationState>,
    creature_calibration: Option<CreatureCalibrationState>,
    /// Auto-open happens once per tab; after the user closes a calibrator
    /// it stays closed until Refresh, so the X isn't fought every frame.
    frames_opened: bool,
    creatures_opened: bool,
    status: Vec<String>,
    styled: bool,
}

impl Default for StudioApp {
    fn default() -> Self {
        Self {
            skin_state: skin::SkinState::default(),
            mode: StudioMode::Anchorer,
            anchor_tab: AnchorTab::Frames,
            frame_calibration: None,
            creature_calibration: None,
            frames_opened: false,
            creatures_opened: false,
            status: Vec::new(),
            styled: false,
        }
    }
}

impl StudioApp {
    /// Font and visuals parity with the game GUI's default theme; fonts
    /// can only be set from inside the egui run loop, hence first-frame.
    fn ensure_style(&mut self, ctx: &egui::Context) {
        if self.styled {
            return;
        }
        ctx.set_fonts(app_theme::build_font_definitions(
            &FontRef::SystemDefault,
            &[],
        ));
        let theme = crate::theme::AppTheme::default();
        let visuals = app_theme::visuals_from_theme(&theme);
        widgets::set_widget_accent(ctx, visuals.selection.bg_fill);
        ctx.set_visuals(visuals);
        self.styled = true;
    }

    fn push_status(&mut self, message: String) {
        self.status.push(message);
        if self.status.len() > STATUS_CAP {
            let excess = self.status.len() - STATUS_CAP;
            self.status.drain(..excess);
        }
    }

    fn drain_outcome(&mut self, outcome: CalibrationOutcome) {
        for message in outcome.messages {
            self.push_status(message);
        }
        if outcome.reload_art {
            self.skin_state.force_reload();
        }
    }

    fn open_frames(&mut self) {
        let (state, outcome) = FrameCalibrationState::open(None);
        self.frame_calibration = state;
        self.frames_opened = true;
        self.drain_outcome(outcome);
    }

    fn open_creatures(&mut self) {
        let (state, outcome) = CreatureCalibrationState::open();
        self.creature_calibration = state;
        self.creatures_opened = true;
        self.drain_outcome(outcome);
    }

    fn anchorer_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.anchor_tab == AnchorTab::Frames, "Frames")
                .clicked()
            {
                self.anchor_tab = AnchorTab::Frames;
            }
            if ui
                .selectable_label(self.anchor_tab == AnchorTab::Creatures, "Creatures")
                .clicked()
            {
                self.anchor_tab = AnchorTab::Creatures;
            }
            if ui
                .button("Refresh")
                .on_hover_text("Re-scan the pool and reopen the calibrator")
                .clicked()
            {
                match self.anchor_tab {
                    AnchorTab::Frames => self.open_frames(),
                    AnchorTab::Creatures => self.open_creatures(),
                }
            }
        });
        ui.add_space(8.0);
        ui.weak(
            "Pick an image in the calibrator window; saves write the sidecar + embed \
             metadata in the PNG",
        );

        match self.anchor_tab {
            AnchorTab::Frames => {
                if !self.frames_opened {
                    self.open_frames();
                }
                if let Some(state) = self.frame_calibration.as_mut() {
                    let outcome = state.ui(ctx);
                    if outcome.closed {
                        self.frame_calibration = None;
                    }
                    self.drain_outcome(outcome);
                }
            }
            AnchorTab::Creatures => {
                if !self.creatures_opened {
                    self.open_creatures();
                }
                if let Some(state) = self.creature_calibration.as_mut() {
                    let outcome = state.ui(ctx);
                    if outcome.closed {
                        self.creature_calibration = None;
                    }
                    self.drain_outcome(outcome);
                }
            }
        }
    }
}

impl eframe::App for StudioApp {
    // This egui fork's App trait hands the root Ui instead of update(ctx).
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        self.ensure_style(ctx);
        // force_reload only takes effect on the next apply; the early
        // return makes the per-frame call cheap.
        self.skin_state.apply_if_changed(ctx, None);

        egui::Panel::top("studio_mode_bar").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Vellum Studio");
                ui.separator();
                if ui
                    .selectable_label(self.mode == StudioMode::Anchorer, "Anchorer")
                    .clicked()
                {
                    self.mode = StudioMode::Anchorer;
                }
                if ui
                    .selectable_label(self.mode == StudioMode::Stage, "Stage")
                    .clicked()
                {
                    self.mode = StudioMode::Stage;
                }
            });
        });
        egui::Panel::bottom("studio_status_bar").show(root, |ui| {
            match self.status.last() {
                Some(message) => ui.label(message),
                None => ui.weak("Ready"),
            }
            .on_hover_ui(|ui| {
                for message in &self.status {
                    ui.label(message);
                }
            });
        });
        egui::CentralPanel::default().show(root, |ui| match self.mode {
            StudioMode::Anchorer => self.anchorer_ui(ui, ctx),
            StudioMode::Stage => {
                ui.heading("Stage");
                ui.weak("Stage — coming in the next slice");
            }
        });
    }
}

/// Boot the Studio window.
pub fn run_studio() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vellum Studio")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Vellum Studio",
        options,
        Box::new(|_cc| Ok(Box::new(StudioApp::default()))),
    )
    .map_err(|err| anyhow!("Failed to run Vellum Studio: {}", err))
}
