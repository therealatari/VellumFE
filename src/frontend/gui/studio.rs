//! Vellum Studio: standalone art-authoring shell. Boots straight into
//! eframe — no network — and rehosts the pool calibrators so frames and
//! creature sprites can be calibrated without launching the game. The
//! Stage is a live creature-field sandbox: fabricated roster + crtrStatus
//! state driven through the REAL field renderer
//! (`render_creature_field_content`), never a reimplementation.

use anyhow::anyhow;
use eframe::egui;

use crate::core::state::{Creature, CreatureFlags, CRTR_STATUS_FLAGS};
use crate::core::AppCore;

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
    /// Built lazily on first Stage entry (AppCore::new is FS-only here).
    stage: Option<StageState>,
}

/// One castable pool-art entry: a base image's token, humanized.
struct CastEntry {
    /// Display name, '_' -> ' ' (name_token slugs it back to the art).
    display: String,
    /// Last word of the display name.
    noun: String,
}

/// The Stage sandbox: a fabricated AppCore whose room roster and target
/// state feed the production creature-field pipeline.
struct StageState {
    app_core: AppCore,
    cast: Vec<CastEntry>,
    filter: String,
    next_id: u64,
    selected: Option<String>,
    show_grid: bool,
    show_order: bool,
    /// Currently applied rider/mount pair (first flagged rider + mount).
    pair: Option<(String, String)>,
    /// The Stage's scene, held as the Arc the render settings take; edits
    /// go through `Arc::make_mut` (cheap — the render clone is per-frame).
    scene: std::sync::Arc<crate::config::scenes::StageScene>,
    /// Scene name box (save target / last loaded).
    scene_name: String,
    /// Scenery-pool filter box.
    prop_filter: String,
    /// Selected placed prop (index into scene.props); drag-to-place moves it.
    selected_prop: Option<usize>,
    /// Status lines raised inside panel closures, drained by stage_ui.
    pending_status: Vec<String>,
}

/// Fabricated layout window carrying the Stage's grid/order toggles.
const STAGE_WINDOW: &str = "studio-stage";

impl StageState {
    fn new() -> anyhow::Result<Self> {
        let config = crate::config::Config::load()?;
        let mut app_core = AppCore::new(config)?;
        // In-memory only: the def carries show_grid/show_order for the
        // renderer's per-window lookup. Never saved.
        if let Some(mut def) = crate::config::Config::get_window_template("creaturefield") {
            def.base_mut().name = STAGE_WINDOW.to_string();
            app_core.layout.windows.push(def);
        }
        Ok(Self {
            app_core,
            cast: build_cast(),
            filter: String::new(),
            next_id: 0,
            selected: None,
            show_grid: true,
            show_order: false,
            pair: None,
            scene: std::sync::Arc::new(crate::config::scenes::StageScene::default()),
            scene_name: String::new(),
            prop_filter: String::new(),
            selected_prop: None,
            pending_status: Vec::new(),
        })
    }

    fn resync(&mut self) {
        self.app_core.game_state.room_creatures_generation += 1;
        crate::core::creature_cards::sync_field(
            &mut self.app_core.creature_field,
            &mut self.app_core.creature_field_synced_gen,
            &self.app_core.game_state,
            &[],
        );
        self.refresh_mounts();
    }

    /// First flagged rider pairs with first flagged mount; changes tear
    /// down the old pair (when both halves still stand) before applying.
    fn refresh_mounts(&mut self) {
        let gs = &self.app_core.game_state;
        let rider = gs
            .room_creatures
            .iter()
            .find(|c| c.flags.as_ref().is_some_and(|f| f.rider))
            .map(|c| c.id.clone());
        let mount = gs
            .room_creatures
            .iter()
            .find(|c| c.flags.as_ref().is_some_and(|f| f.mount) && Some(&c.id) != rider.as_ref())
            .map(|c| c.id.clone());
        let desired = rider.zip(mount);
        if desired == self.pair {
            return;
        }
        if let Some((old_rider, _)) = self.pair.take() {
            let still_paired = self
                .app_core
                .creature_field
                .unit_of(&old_rider)
                .is_some_and(|u| u.members.len() > 1);
            if still_paired {
                let size = self
                    .app_core
                    .game_state
                    .room_creatures
                    .iter()
                    .find(|c| c.id == old_rider)
                    .map(crate::core::creature_cards::card_size_for)
                    .unwrap_or_default();
                self.app_core.creature_field.dismount(&old_rider, size);
            }
        }
        if let Some((rider, mount)) = &desired {
            let field = &mut self.app_core.creature_field;
            if field.unit_of(rider).is_some_and(|u| u.members.len() == 1)
                && field.unit_of(mount).is_some_and(|u| u.members.len() == 1)
            {
                field.mount(rider, mount);
                self.pair = desired;
            }
        } else {
            self.pair = None;
        }
    }

    fn spawn(&mut self, display: &str, noun: Option<String>) {
        self.next_id += 1;
        let id = format!("studio-{}", self.next_id);
        self.app_core.game_state.room_creatures.push(Creature {
            name: display.to_string(),
            noun,
            id: id.clone(),
            status: None,
            flags: Some(CreatureFlags {
                hostile: true,
                ..Default::default()
            }),
        });
        self.selected = Some(id);
        self.resync();
    }

    fn remove(&mut self, id: &str) {
        self.app_core
            .game_state
            .room_creatures
            .retain(|c| c.id != id);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        }
        if self.app_core.game_state.target_list.current_target == id {
            self.app_core.game_state.target_list.current_target = String::new();
        }
        self.resync();
    }

    fn clear(&mut self) {
        self.app_core.game_state.room_creatures.clear();
        self.selected = None;
        self.app_core.game_state.target_list.current_target = String::new();
        self.resync();
    }

    /// Push the panel toggles into the fabricated layout def the renderer
    /// reads its per-window options from.
    fn apply_view_options(&mut self) {
        if let Some(crate::config::WindowDef::CreatureField { data, .. }) = self
            .app_core
            .layout
            .windows
            .iter_mut()
            .find(|w| w.name() == STAGE_WINDOW)
        {
            data.show_grid = self.show_grid;
            data.show_order = self.show_order;
        }
    }

    /// Art wanted for the current roster — same recipe as the game's
    /// update loop (family from the bestiary, prone + wounds from flags).
    fn wanted_art(&self) -> Vec<super::skin::WantedCreature> {
        self.app_core
            .game_state
            .room_creatures
            .iter()
            .map(|c| {
                let family = c
                    .noun
                    .as_deref()
                    .and_then(crate::core::creature_cards::family_for_noun);
                super::skin::WantedCreature {
                    name: c.name.clone(),
                    noun: c.noun.clone(),
                    family,
                    prone: c.flags.as_ref().is_some_and(|f| f.has_flag("prone")),
                    injuries: c
                        .flags
                        .as_ref()
                        .map(|f| f.injuries.clone())
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// The Scene section: backdrop + scenery props, persisted whole-file
    /// to `global/scenes/<name>.toml`.
    fn scene_ui(&mut self, ui: &mut egui::Ui) {
        use crate::config::scenes::{self, SceneProp, StageScene};
        use std::sync::Arc;

        ui.heading("Scene");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.scene_name)
                    .hint_text("scene name")
                    .desired_width(110.0),
            );
            if ui.button("Save").clicked() {
                match self.scene.save(&self.scene_name) {
                    Ok(()) => self
                        .pending_status
                        .push(format!("Scene '{}' saved", self.scene_name.trim())),
                    Err(err) => self.pending_status.push(format!("Scene save failed: {err:#}")),
                }
            }
            if ui.button("New").clicked() {
                self.scene = Arc::new(StageScene::default());
                self.scene_name.clear();
                self.selected_prop = None;
            }
        });
        let saved = scenes::list_scenes();
        if !saved.is_empty() {
            egui::ComboBox::from_id_salt("scene_load")
                .selected_text("Load…")
                .show_ui(ui, |ui| {
                    for name in &saved {
                        if ui.selectable_label(false, name).clicked() {
                            match StageScene::load(name) {
                                Ok(scene) => {
                                    self.scene = Arc::new(scene);
                                    self.scene_name = name.clone();
                                    self.selected_prop = None;
                                }
                                Err(err) => self
                                    .pending_status
                                    .push(format!("Scene load failed: {err:#}")),
                            }
                        }
                    }
                });
        }

        let backgrounds = crate::config::pool::list_category("scenes");
        ui.horizontal(|ui| {
            ui.label("Backdrop");
            let current = self.scene.background.clone();
            let text = current
                .as_deref()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                .unwrap_or_else(|| "(none)".to_string());
            egui::ComboBox::from_id_salt("scene_bg")
                .selected_text(text)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "(none)").clicked() {
                        Arc::make_mut(&mut self.scene).background = None;
                    }
                    for image in &backgrounds {
                        let on = current.as_deref() == Some(image.pool_path.as_str());
                        if ui.selectable_label(on, image.display_label()).clicked() {
                            Arc::make_mut(&mut self.scene).background =
                                Some(image.pool_path.clone());
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Color");
            // The picker needs a concrete color; an unset scene edits a
            // neutral slate and only stores once touched (None keeps the
            // default panel fill).
            let mut rgb = self
                .scene
                .background_color
                .as_deref()
                .and_then(parse_hex_rgb)
                .unwrap_or([64, 64, 72]);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                Arc::make_mut(&mut self.scene).background_color =
                    Some(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]));
            }
            if self.scene.background_color.is_some()
                && ui.small_button("x").on_hover_text("Clear color").clicked()
            {
                Arc::make_mut(&mut self.scene).background_color = None;
            }
        });

        ui.label("Props");
        let pool = crate::config::pool::list_category("scenery");
        if backgrounds.is_empty() && pool.is_empty() {
            ui.weak(
                "Drop PNGs into global/images/scenes (backgrounds, author at 880x470) \
                 or global/images/scenery (props)",
            );
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.prop_filter)
                .hint_text("filter")
                .desired_width(140.0),
        );
        let filter = self.prop_filter.to_ascii_lowercase();
        let mut add_request: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("scene_pool")
            .max_height(110.0)
            .show(ui, |ui| {
                for image in &pool {
                    if !filter.is_empty()
                        && !image.pool_path.to_ascii_lowercase().contains(&filter)
                    {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("Add").clicked() {
                            add_request = Some(image.pool_path.clone());
                        }
                        ui.label(image.display_label());
                    });
                }
            });
        let (z_near, z_far) = self.app_core.creature_field.depth_range();
        if let Some(image) = add_request {
            // New props spawn at stage centre, mid depth.
            let scene = Arc::make_mut(&mut self.scene);
            scene.props.push(SceneProp {
                image,
                x: crate::core::creature_cards::solver::STAGE_W / 2.0,
                z: (z_near + z_far) / 2.0,
                scale: 1.0,
            });
            self.selected_prop = Some(scene.props.len() - 1);
        }
        let placed: Vec<String> = self
            .scene
            .props
            .iter()
            .map(|p| p.image.rsplit('/').next().unwrap_or(&p.image).to_string())
            .collect();
        let mut remove_request: Option<usize> = None;
        for (k, name) in placed.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("x").on_hover_text("Remove").clicked() {
                    remove_request = Some(k);
                }
                if ui
                    .selectable_label(self.selected_prop == Some(k), name)
                    .clicked()
                {
                    self.selected_prop = Some(k);
                }
            });
        }
        if let Some(k) = remove_request {
            Arc::make_mut(&mut self.scene).props.remove(k);
            self.selected_prop = match self.selected_prop {
                Some(s) if s == k => None,
                Some(s) if s > k => Some(s - 1),
                other => other,
            };
        }
        if let Some(k) = self.selected_prop {
            let scene = Arc::make_mut(&mut self.scene);
            if let Some(prop) = scene.props.get_mut(k) {
                ui.horizontal(|ui| {
                    ui.label("x");
                    ui.add(egui::DragValue::new(&mut prop.x).speed(2.0));
                    ui.label("z");
                    ui.add(egui::DragValue::new(&mut prop.z).speed(0.02));
                });
                prop.x = prop
                    .x
                    .clamp(0.0, crate::core::creature_cards::solver::STAGE_W);
                prop.z = prop.z.clamp(z_near, z_far);
                ui.add(egui::Slider::new(&mut prop.scale, 0.1..=5.0).text("scale"));
                ui.weak("Drag on the stage to move the selected prop");
            }
        }
    }

    fn panel_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.scene_ui(ui);
            ui.separator();
            ui.heading("Cast");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("filter")
                        .desired_width(140.0),
                );
                if ui.button("Add generic").clicked() {
                    self.spawn("training dummy", Some("dummy".to_string()));
                }
            });
            let filter = self.filter.to_ascii_lowercase();
            let mut spawn_request: Option<(String, String)> = None;
            egui::ScrollArea::vertical()
                .id_salt("stage_cast")
                .max_height(160.0)
                .show(ui, |ui| {
                    for entry in &self.cast {
                        if !filter.is_empty() && !entry.display.to_ascii_lowercase().contains(&filter)
                        {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("Spawn").clicked() {
                                spawn_request =
                                    Some((entry.display.clone(), entry.noun.clone()));
                            }
                            ui.label(&entry.display);
                        });
                    }
                });
            if let Some((display, noun)) = spawn_request {
                self.spawn(&display, Some(noun));
            }

            ui.separator();
            ui.heading("Roster");
            ui.horizontal(|ui| {
                if ui.button("Clear stage").clicked() {
                    self.clear();
                }
            });
            let roster: Vec<(String, String)> = self
                .app_core
                .game_state
                .room_creatures
                .iter()
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect();
            let mut remove_request: Option<String> = None;
            for (id, name) in &roster {
                ui.horizontal(|ui| {
                    if ui.small_button("x").on_hover_text("Remove").clicked() {
                        remove_request = Some(id.clone());
                    }
                    if ui
                        .selectable_label(self.selected.as_deref() == Some(id), name)
                        .clicked()
                    {
                        self.selected = Some(id.clone());
                    }
                });
            }
            if let Some(id) = remove_request {
                self.remove(&id);
            }

            if let Some(id) = self.selected.clone() {
                ui.separator();
                self.simulator_ui(ui, &id);
            }

            ui.separator();
            ui.heading("Camera");
            // Studio-only tuning; the game never mutates live params.
            let params = &mut self.app_core.creature_field.params;
            egui::Grid::new("stage_camera").num_columns(2).show(ui, |ui| {
                ui.label("focal");
                ui.add(egui::DragValue::new(&mut params.focal).speed(2.0));
                ui.end_row();
                ui.label("cam_h");
                ui.add(egui::DragValue::new(&mut params.cam_h).speed(0.02));
                ui.end_row();
                ui.label("z0");
                ui.add(egui::DragValue::new(&mut params.z0).speed(0.02));
                ui.end_row();
                ui.label("dz");
                ui.add(egui::DragValue::new(&mut params.dz).speed(0.02));
                ui.end_row();
                ui.label("horizon");
                ui.add(egui::DragValue::new(&mut params.horizon).speed(1.0));
                ui.end_row();
            });
            if ui.button("Reset camera").clicked() {
                let default = crate::core::creature_cards::solver::FieldParams::default();
                params.focal = default.focal;
                params.cam_h = default.cam_h;
                params.z0 = default.z0;
                params.dz = default.dz;
                params.horizon = default.horizon;
            }
        });
    }

    /// State editor for one spawned creature: crtrStatus flags, class
    /// bools, wounds, health, targeting, mounting.
    fn simulator_ui(&mut self, ui: &mut egui::Ui, id: &str) {
        let mut structural = false;
        let is_target = self.app_core.game_state.target_list.current_target == id;
        let mut want_target = is_target;
        {
            let Some(creature) = self
                .app_core
                .game_state
                .room_creatures
                .iter_mut()
                .find(|c| c.id == id)
            else {
                return;
            };
            ui.heading(&creature.name);
            let flags = creature.flags.get_or_insert_with(|| CreatureFlags {
                hostile: true,
                ..Default::default()
            });

            ui.checkbox(&mut want_target, "Target");

            ui.label("Status");
            ui.horizontal_wrapped(|ui| {
                for (_, canonical) in CRTR_STATUS_FLAGS.iter() {
                    let mut on = flags.statuses.iter().any(|s| s == canonical);
                    if ui.checkbox(&mut on, *canonical).changed() {
                        if on {
                            flags.statuses.push((*canonical).to_string());
                        } else {
                            flags.statuses.retain(|s| s != canonical);
                        }
                    }
                }
            });

            ui.label("Class");
            ui.horizontal_wrapped(|ui| {
                // dead flips field membership; boss flips card size: both
                // structural, so the field re-syncs below.
                structural |= ui.checkbox(&mut flags.dead, "dead").changed();
                structural |= ui.checkbox(&mut flags.ascension_boss, "boss").changed();
                structural |= ui.checkbox(&mut flags.mini_boss, "mini boss").changed();
                ui.checkbox(&mut flags.sympathetic, "sympathetic");
                structural |= ui.checkbox(&mut flags.rider, "rider").changed();
                structural |= ui.checkbox(&mut flags.mount, "mount").changed();
            });

            let mut has_health = flags.health.is_some();
            if ui.checkbox(&mut has_health, "Health bar").changed() {
                if has_health {
                    flags.health = Some(100);
                    flags.max_health = Some(100);
                } else {
                    flags.health = None;
                    flags.max_health = None;
                }
            }
            if let Some(health) = flags.health.as_mut() {
                flags.max_health = Some(100);
                let mut hp = *health;
                if ui
                    .add(egui::Slider::new(&mut hp, 0..=100).text("health"))
                    .changed()
                {
                    *health = hp;
                }
            }

            ui.label("Wounds (rank 0-3)");
            egui::Grid::new("stage_wounds").num_columns(2).show(ui, |ui| {
                for part in crate::config::INJURY_AREAS.iter() {
                    let mut rank = flags
                        .injuries
                        .iter()
                        .find(|(p, _)| p == part)
                        .map(|(_, r)| *r)
                        .unwrap_or(0);
                    ui.label(*part);
                    ui.horizontal(|ui| {
                        for r in 0u8..=3 {
                            if ui
                                .selectable_label(rank == r, format!("{r}"))
                                .clicked()
                            {
                                rank = r;
                                flags.injuries.retain(|(p, _)| p != part);
                                if r > 0 {
                                    flags.injuries.push(((*part).to_string(), r));
                                }
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        }
        if want_target != is_target {
            self.app_core.game_state.target_list.current_target =
                if want_target { id.to_string() } else { String::new() };
        }
        if structural {
            self.resync();
        }
    }

    fn field_ui(&mut self, ui: &mut egui::Ui, art: super::skin::SharedCreatureArt) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_grid, "Grid");
            ui.checkbox(&mut self.show_order, "Draw order");
        });
        self.apply_view_options();
        // The renderer allocates the remaining region; remember it for the
        // drag interact below (same origin + size).
        let rect = egui::Rect::from_min_size(ui.next_widget_position(), ui.available_size());
        let settings =
            super::app::WidgetRenderSettings::for_creature_field(art, Some(self.scene.clone()));
        let click = super::app::VellumGuiApp::render_creature_field_content(
            &self.app_core,
            ui,
            STAGE_WINDOW,
            &settings,
        );
        // Drag-to-place: while a prop is selected, dragging the field view
        // moves it on the ground plane (screen → stage → ground, through
        // the renderer's own mapping and the solver's inverse projection).
        if let Some(k) = self.selected_prop {
            let response = ui.interact(
                rect,
                ui.id().with("scene_prop_drag"),
                egui::Sense::drag(),
            );
            if response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let (sx, sy) =
                        super::app::VellumGuiApp::creature_field_stage_pos(rect, pos);
                    let (x, z) = self.app_core.creature_field.ground_from_screen(sx, sy);
                    let scene = std::sync::Arc::make_mut(&mut self.scene);
                    if let Some(prop) = scene.props.get_mut(k) {
                        prop.x = x;
                        prop.z = z;
                    }
                }
            }
        }
        // The renderer's click-to-target emits the game command; the Stage
        // applies it locally instead of sending it anywhere.
        if let Some(click) = click {
            if let Some(id) = click.link_data.noun.strip_prefix("target #") {
                self.app_core.game_state.target_list.current_target = id.to_string();
            }
        }
    }
}

/// "#rrggbb" -> srgb components, for seeding the color picker. None for
/// anything malformed (the picker then edits its neutral default).
fn parse_hex_rgb(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if hex.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

/// Distinct castable base tokens from the creature pool: a BASE image is
/// flat `creatures/<stem>.png`, or one whose stem equals its parent folder
/// (noun or variant folder). `default`/`status` folders are reserved;
/// `{token}_`-suffixed extras (prone/wounds) fail the stem==folder test.
fn build_cast() -> Vec<CastEntry> {
    let mut tokens = std::collections::BTreeSet::new();
    for image in crate::config::pool::list_creature_images() {
        let stem = image.stem();
        match image.set.as_deref() {
            None => {}
            Some(set) => {
                if set.split('/').any(|seg| seg == "default" || seg == "status") {
                    continue;
                }
                let parent = set.rsplit('/').next().unwrap_or(set);
                if stem != parent {
                    continue;
                }
            }
        }
        if !stem.is_empty() {
            tokens.insert(stem.to_string());
        }
    }
    tokens
        .into_iter()
        .map(|token| {
            let display = token.replace('_', " ");
            let noun = display
                .rsplit(' ')
                .next()
                .unwrap_or(&display)
                .to_string();
            CastEntry { display, noun }
        })
        .collect()
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
            stage: None,
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

    fn stage_ui(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        if self.stage.is_none() {
            match StageState::new() {
                Ok(stage) => {
                    self.push_status(format!("Stage ready: {} castable bases", stage.cast.len()));
                    self.stage = Some(stage);
                }
                Err(err) => {
                    self.push_status(format!("Stage init failed: {err:#}"));
                }
            }
        }
        let Some(stage) = self.stage.as_mut() else {
            egui::CentralPanel::default().show(root, |ui| {
                ui.heading("Stage");
                ui.weak("Stage unavailable — see the status bar");
            });
            return;
        };
        // Roster sync is generation-gated (cheap when unchanged); art prep
        // is cached, so a settled stage costs a few hash lookups.
        crate::core::creature_cards::sync_field(
            &mut stage.app_core.creature_field,
            &mut stage.app_core.creature_field_synced_gen,
            &stage.app_core.game_state,
            &[],
        );
        let wanted = stage.wanted_art();
        if !wanted.is_empty() {
            self.skin_state.prepare_creature_art(ctx, &wanted);
        }
        self.skin_state.prepare_scene_art(ctx, &stage.scene);
        let art = self.skin_state.creature_art();
        egui::Panel::right("stage_panel")
            .default_size(300.0)
            .show(root, |ui| stage.panel_ui(ui));
        egui::CentralPanel::default().show(root, |ui| stage.field_ui(ui, art));
        let pending: Vec<String> = stage.pending_status.drain(..).collect();
        for message in pending {
            self.push_status(message);
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
        match self.mode {
            StudioMode::Anchorer => {
                egui::CentralPanel::default().show(root, |ui| self.anchorer_ui(ui, ctx));
            }
            StudioMode::Stage => self.stage_ui(root, ctx),
        }
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
