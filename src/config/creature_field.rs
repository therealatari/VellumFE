//! Creature-field camera/solver overrides (`global/creature_field.toml`).
//!
//! Personal geometry corrections layered over whatever scene is active:
//! a per-scene override (this backdrop's camera is wrong for me) wins over
//! the blanket override (I always want a flatter field), which wins over
//! the scene's own embedded camera/solver, which wins over the built-in
//! defaults — all field-by-field, unset keys falling through.
//!
//! Deliberately its OWN file, not part of the appearance store: skin
//! commands (.setskin/.saveskin/pack install) must never touch camera
//! tuning, and a separate file needs no carve-outs to guarantee that.
//! Global rather than per-character, like scenes and their bindings — the
//! whole creature-field domain is shared world/preference data.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::skins::{CreatureFieldCamera, CreatureFieldSolver};

/// One override at one scope: values plus an enable flag, so "show me it
/// without my tweak" is a toggle instead of destroying the numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldOverride {
    /// Off = keep the values but stop applying them.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "CreatureFieldCamera::is_empty")]
    pub camera: CreatureFieldCamera,
    #[serde(default, skip_serializing_if = "CreatureFieldSolver::is_empty")]
    pub solver: CreatureFieldSolver,
}

fn default_true() -> bool {
    true
}

/// `enabled` defaults true in code too (not just serde), so a freshly
/// created override applies as soon as a knob is armed.
impl Default for FieldOverride {
    fn default() -> Self {
        Self {
            enabled: true,
            camera: CreatureFieldCamera::default(),
            solver: CreatureFieldSolver::default(),
        }
    }
}

impl FieldOverride {
    /// Nothing to persist: no values and the flag at its default.
    pub fn is_empty(&self) -> bool {
        self.camera.is_empty() && self.solver.is_empty()
    }
}

/// The whole override store.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldOverrides {
    /// Applies to every scene (and to the sceneless default camera).
    #[serde(default, skip_serializing_if = "FieldOverride::is_empty")]
    pub blanket: FieldOverride,
    /// Scene name -> override for that scene only. Wins over the blanket.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scenes: HashMap<String, FieldOverride>,
}

impl FieldOverrides {
    pub fn path() -> anyhow::Result<PathBuf> {
        Ok(super::Config::global_dir()?.join("creature_field.toml"))
    }

    /// Load the store; missing file = defaults, broken file warns and
    /// loads defaults (never a hard error in the game loop).
    pub fn load() -> Self {
        let Ok(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|err| {
                tracing::warn!("invalid {}: {} — ignoring overrides", path.display(), err);
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Atomic whole-file save. Disabled-but-valued entries persist (the
    /// toggle promise); truly empty ones are dropped by the serde skips.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path()?;
        // Scene entries someone cleared down to nothing would round-trip
        // as bare `enabled = true` tables; drop them instead.
        let mut clean = self.clone();
        clean.scenes.retain(|_, entry| !entry.is_empty());
        let contents = toml::to_string_pretty(&clean)
            .map_err(|err| anyhow::anyhow!("cannot serialize field overrides: {}", err))?;
        super::write_atomic(&path, contents)
            .map_err(|err| anyhow::anyhow!("cannot write {}: {}", path.display(), err))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_missing_file_defaults() {
        let _guard = crate::config::VELLUM_FE_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("VELLUM_FE_DIR", dir.path());

        assert_eq!(FieldOverrides::load(), FieldOverrides::default());

        let mut overrides = FieldOverrides::default();
        overrides.blanket.camera.focal = Some(450.0);
        overrides.blanket.enabled = false;
        overrides.scenes.insert(
            "cave".to_string(),
            FieldOverride {
                enabled: true,
                camera: CreatureFieldCamera {
                    horizon: Some(160.0),
                    ..Default::default()
                },
                solver: CreatureFieldSolver {
                    zone: Some("grid".to_string()),
                    ..Default::default()
                },
            },
        );
        // An emptied scene entry is dropped on save, not round-tripped.
        overrides
            .scenes
            .insert("stale".to_string(), FieldOverride::default());
        overrides.save().unwrap();

        let loaded = FieldOverrides::load();
        assert_eq!(loaded.blanket.camera.focal, Some(450.0));
        assert!(!loaded.blanket.enabled, "disabled flag persists");
        assert_eq!(
            loaded.scenes["cave"].camera.horizon,
            Some(160.0),
            "per-scene camera persists"
        );
        assert_eq!(loaded.scenes["cave"].solver.zone.as_deref(), Some("grid"));
        assert!(!loaded.scenes.contains_key("stale"));

        std::env::remove_var("VELLUM_FE_DIR");
    }
}
