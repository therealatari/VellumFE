//! AppCore surface for the native skill trainer panel.
//!
//! Flow: the user sends `goals` (typed, or via `.goals`) → [`arm_skill_trainer`]
//! marks the next `<LaunchURL>` as trainer traffic → `poll_skill_trainer`
//! (called every frame from `poll_map`) hands that URL to the worker instead
//! of the browser, then drains fetch/submit results into
//! `ui_state.skill_trainer` for both frontends to render.

use crate::core::skill_trainer::{self, TrainerEvent};
use crate::data::skill_trainer::{GoalProfile, TrainerStatus};

use super::AppCore;

/// How long an armed GOALS request may wait for its LaunchURL before an
/// unrelated LaunchURL falls back to the browser. Generous: the game answers
/// in well under a second.
const ARM_WINDOW: std::time::Duration = std::time::Duration::from_secs(15);

impl AppCore {
    /// Mark the next LaunchURL as skill-trainer traffic and show the panel
    /// in its loading state.
    pub fn arm_skill_trainer(&mut self) {
        self.skill_trainer_armed = Some(std::time::Instant::now());
        let ui = &mut self.ui_state.skill_trainer;
        ui.open = true;
        ui.status = TrainerStatus::Loading;
    }

    /// Per-frame: route a pending LaunchURL and drain worker results.
    pub fn poll_skill_trainer(&mut self) {
        if let Some(url) = self.message_processor.pending_launch_url.take() {
            let armed = self
                .skill_trainer_armed
                .take()
                .is_some_and(|t| t.elapsed() < ARM_WINDOW);
            if armed {
                self.add_system_message("[goals] fetching the skill manager page...");
                self.skill_trainer_worker.fetch(&url);
            } else {
                let full_url = format!("https://www.play.net{url}");
                tracing::info!("Launching URL in browser: {}", full_url);
                if let Err(e) = crate::platform::open_url(&full_url) {
                    tracing::error!("Failed to open browser: {}", e);
                }
            }
        }

        for event in self.skill_trainer_worker.poll() {
            let ui = &mut self.ui_state.skill_trainer;
            match event {
                TrainerEvent::Loaded(goals) => {
                    ui.data = Some(*goals);
                    ui.status = TrainerStatus::Idle;
                    ui.revision += 1;
                    ui.open = true;
                    self.add_system_message("[goals] skill manager loaded.");
                }
                TrainerEvent::Applied(goals) => {
                    ui.data = Some(*goals);
                    ui.status = TrainerStatus::Idle;
                    ui.revision += 1;
                    self.add_system_message("[goals] skill goals applied.");
                }
                TrainerEvent::Saved => {
                    // Goals committed and pushed to the game. The confirmation
                    // page isn't a trainer form, so re-fetch to refresh the
                    // panel with the now-committed ranks.
                    ui.status = TrainerStatus::Loading;
                    self.add_system_message(
                        "[goals] skill goals saved and sent to the game. Refreshing…",
                    );
                    let cmd = self.skill_trainer_reload_command();
                    // Small delay so play.net's write settles before the
                    // fresh GOALS link fetches the updated page.
                    self.queue_timed_command(std::time::Duration::from_millis(750), cmd);
                }
                TrainerEvent::Failed(msg) => {
                    ui.status = TrainerStatus::Error(msg.clone());
                    self.add_system_message(&format!("[goals] {msg}"));
                }
            }
        }

        // Mirror the panel to remote clients when anything observable changed.
        self.push_skill_trainer_remote();
    }

    /// Broadcast the skill-trainer panel to remote clients, but only when its
    /// observable state (open, status, data revision) changed since the last
    /// push. Guarded on `remote.is_some()` exactly like `flush_remote_state`.
    pub fn push_skill_trainer_remote(&mut self) {
        if self.message_processor.remote.is_none() {
            return;
        }
        let (open, status, revision) = {
            let ui = &self.ui_state.skill_trainer;
            (
                ui.open,
                Self::skill_trainer_status_tag(&ui.status),
                ui.revision,
            )
        };
        // The sink owns dedup (its `last_*` fingerprints already live there);
        // it only asks us to serialize when the fingerprint actually changed.
        let needs_push = self
            .message_processor
            .remote
            .as_ref()
            .is_some_and(|r| r.skill_trainer_changed(open, &status, revision));
        if !needs_push {
            return;
        }
        let data = self.skill_trainer_remote_json();
        if let Some(remote) = self.message_processor.remote.as_mut() {
            remote.push_skill_trainer(open, status, revision, data);
        }
    }

    /// Force the next `push_skill_trainer_remote` to broadcast even if the
    /// (open, status, revision) fingerprint is unchanged — used when only the
    /// profile list changed (profile save/delete), which the fingerprint does
    /// not observe.
    pub fn invalidate_skill_trainer_remote(&mut self) {
        if let Some(remote) = self.message_processor.remote.as_mut() {
            remote.invalidate_skill_trainer();
        }
    }

    /// A stable wire tag for the trainer status. `Error` carries its message
    /// as `error:<msg>` so the phone can show it.
    fn skill_trainer_status_tag(status: &TrainerStatus) -> String {
        match status {
            TrainerStatus::Idle => "idle".to_string(),
            TrainerStatus::Loading => "loading".to_string(),
            TrainerStatus::Applying => "applying".to_string(),
            TrainerStatus::Error(msg) => format!("error:{msg}"),
        }
    }

    /// Serialize the trainer panel into the JSON the phone renders directly:
    /// `{ rows: [{id,name,section,ranks,goal,max,phy_cost,mnt_cost}],
    ///    points: {phy_left,mnt_left,phy_conv,mnt_conv},
    ///    char: {name,level,prof}, dirty, profiles: [..] }`.
    /// None when no page has loaded yet (panel still empty).
    pub fn skill_trainer_remote_json(&self) -> Option<serde_json::Value> {
        let goals = self.ui_state.skill_trainer.data.as_ref()?;
        let rows: Vec<serde_json::Value> = goals
            .rows
            .iter()
            .map(|row| {
                let (phy_cost, mnt_cost) = goals.cost_to_raise(row.id);
                serde_json::json!({
                    "id": row.id,
                    "name": row.name,
                    "section": row.section,
                    "ranks": goals.start_ranks_of(row.id),
                    "goal": goals.goal_ranks(row.id),
                    "max": goals.max_ranks_of(row.id),
                    "phy_cost": phy_cost,
                    "mnt_cost": mnt_cost,
                })
            })
            .collect();
        Some(serde_json::json!({
            "rows": rows,
            "points": {
                "phy_left": goals.phy_left,
                "mnt_left": goals.mnt_left,
                "phy_conv": goals.phy_conv,
                "mnt_conv": goals.mnt_conv,
            },
            "char": {
                "name": goals.char_name,
                "level": goals.level,
                "prof": goals.prof_name,
            },
            "dirty": goals.dirty(),
            "profiles": self.skill_trainer_profiles(),
        }))
    }

    /// Step a skill's goal by `n` ranks (frontends' +/- buttons; n is the
    /// 1/10/100 step). Returns how many ranks actually applied.
    pub fn skill_trainer_step(&mut self, id: u32, n: u32, raise: bool) -> u32 {
        let applied = self
            .ui_state
            .skill_trainer
            .data
            .as_mut()
            .map(|g| g.step(id, n, raise))
            .unwrap_or(0);
        // Bump the revision so both frontend caches and the remote push
        // fingerprint observe the goal change (step mutates data in place).
        if applied > 0 {
            self.ui_state.skill_trainer.revision += 1;
        }
        applied
    }

    /// POST the current goals to play.net.
    pub fn skill_trainer_apply(&mut self) {
        let ui = &mut self.ui_state.skill_trainer;
        if ui.status == TrainerStatus::Applying {
            return;
        }
        let Some(goals) = ui.data.clone() else {
            return;
        };
        ui.status = TrainerStatus::Applying;
        self.skill_trainer_worker.submit(&goals);
        self.add_system_message("[goals] submitting skill goals...");
    }

    /// Re-request a fresh page: sends `goals` to the game and arms the
    /// trainer. Returns the command for the network layer.
    pub fn skill_trainer_reload_command(&mut self) -> String {
        self.arm_skill_trainer();
        "goals".to_string()
    }

    /// Saved goal profiles for the current character, sorted by name.
    pub fn skill_trainer_profiles(&self) -> Vec<String> {
        let who = self.skill_trainer_character();
        skill_trainer::load_profiles()
            .get(&who)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn skill_trainer_save_profile(&mut self, name: &str) {
        let who = self.skill_trainer_character();
        let Some(goals) = self.ui_state.skill_trainer.data.as_ref() else {
            return;
        };
        let profile = GoalProfile::capture(goals);
        let mut store = skill_trainer::load_profiles();
        store.entry(who).or_default().insert(name.to_string(), profile);
        match skill_trainer::save_profiles(&store) {
            Ok(()) => self.add_system_message(&format!("[goals] profile '{name}' saved.")),
            Err(e) => self.add_system_message(&format!("[goals] saving profile failed: {e}")),
        }
    }

    /// Load a named profile into the editor (goals only — Apply still
    /// required to commit on the server).
    pub fn skill_trainer_load_profile(&mut self, name: &str) {
        let who = self.skill_trainer_character();
        let store = skill_trainer::load_profiles();
        let Some(profile) = store.get(&who).and_then(|m| m.get(name)) else {
            self.add_system_message(&format!("[goals] no profile named '{name}'."));
            return;
        };
        let profile = profile.clone();
        if let Some(goals) = self.ui_state.skill_trainer.data.as_mut() {
            skill_trainer::apply_profile(goals, &profile);
            self.ui_state.skill_trainer.revision += 1;
            self.add_system_message(&format!("[goals] profile '{name}' loaded into the editor."));
        }
    }

    pub fn skill_trainer_delete_profile(&mut self, name: &str) {
        let who = self.skill_trainer_character();
        let mut store = skill_trainer::load_profiles();
        let removed = store
            .get_mut(&who)
            .map(|m| m.remove(name).is_some())
            .unwrap_or(false);
        if removed {
            match skill_trainer::save_profiles(&store) {
                Ok(()) => self.add_system_message(&format!("[goals] profile '{name}' deleted.")),
                Err(e) => self.add_system_message(&format!("[goals] deleting profile failed: {e}")),
            }
        } else {
            self.add_system_message(&format!("[goals] no profile named '{name}'."));
        }
    }

    /// Profile-store key: the trainer page's character name when loaded
    /// (authoritative), else the session character.
    fn skill_trainer_character(&self) -> String {
        self.ui_state
            .skill_trainer
            .data
            .as_ref()
            .map(|g| g.char_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.game_state.character_name.clone().unwrap_or_default())
    }
}
