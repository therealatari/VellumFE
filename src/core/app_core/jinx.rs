//! AppCore surface for the jinx asset-manager worker: drain its results each
//! frame (`poll_jinx`, called from `poll_map`) and apply post-install side
//! effects on the main thread (`apply_jinx_effect` — reloads touch `AppCore`
//! and can't run on the worker).

use super::AppCore;

impl AppCore {
    /// Drain the asset-manager worker: print each line to the game text and
    /// apply any post-install effect. Called once per frame from `poll_map`,
    /// alongside the map worker it mirrors.
    pub fn poll_jinx(&mut self) {
        let updates = self.jinx_worker.poll();
        for update in updates {
            // Effect-only updates carry no line (a set install reports once
            // for the whole set, then fires one effect); don't print blanks.
            if !update.line.is_empty() {
                self.add_system_message(&update.line);
            }
            if let Some(effect) = update.effect {
                self.apply_jinx_effect(effect);
            }
        }
    }

    /// Apply a post-install side effect on the main thread (reloads touch
    /// `AppCore` and can't run on the worker). Reloads that already exist run
    /// live; kinds whose reload plumbing isn't built yet say so plainly rather
    /// than silently leaving a stale in-memory copy.
    fn apply_jinx_effect(&mut self, effect: crate::core::jinx::worker::Effect) {
        use crate::core::jinx::worker::Effect;
        match effect {
            Effect::Installed { name, kind } => match name.as_str() {
                // gameobj-data.xml re-resolves live: drop the cache and the
                // next classify() reads the freshly installed global/data copy.
                "gameobj-data.xml" => {
                    let types = self.reload_data_pack();
                    self.add_system_message(&format!(
                        "[jinx] gameobj classifier reloaded ({types} types)"
                    ));
                }
                // effect-list.xml re-reads live: spell_table prefers the
                // freshly installed global/data copy and swaps its table.
                "effect-list.xml" => {
                    let count = crate::core::spell_table::reload();
                    self.add_system_message(&format!(
                        "[jinx] spell table reloaded ({count} spells)"
                    ));
                }
                // mapdb.json landed in the map dir; resolve_source now
                // recognizes a plain mapdb.json (below any versioned release),
                // so re-resolving the source loads it live.
                "mapdb.json" => {
                    self.refresh_map_source();
                    self.add_system_message("[jinx] map database reloaded");
                }
                _ => match kind.as_str() {
                    // A skin's files land under skins/<name>/; list_skins and
                    // load_manifest read that dir live, so the new skin is
                    // immediately selectable. Activation stays user-driven
                    // (accessibility-first: never auto-restyle). Suggest the
                    // exact .setskin command, using the skin's dir name (the
                    // archive extension stripped).
                    "skin" => {
                        let skin_name = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
                        self.add_system_message(&format!(
                            "[jinx] skin installed — activate with .setskin {skin_name}"
                        ));
                    }
                    "iconmap" | "image" | "icon" => self
                        .add_system_message(&format!("[jinx] {name} installed to the icon pool")),
                    // A doll base image lands in the doll pool; a skin points
                    // its [injury_doll] base at it (paths may be absolute).
                    "doll" => self
                        .add_system_message(&format!("[jinx] {name} installed to the doll pool")),
                    _ => {
                        tracing::info!("jinx installed {name} ({kind}); no reload hook");
                    }
                },
            },
            // Stash the catalog for the GUI Assets panel to read; no core
            // side effect (the panel renders it and drives install/update).
            Effect::Catalog(entries) => {
                self.jinx_catalog = Some(entries);
            }
        }
    }
}
