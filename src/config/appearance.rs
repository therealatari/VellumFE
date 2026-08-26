//! The per-character appearance store (skin-system overhaul, phase 4).
//!
//! Canonical home of "which art is assigned" state that every frontend —
//! GUI, TUI, web, headless remote — needs to read and write. Before this
//! file, the canonical values lived in the GUI layout's `ui_settings`
//! (unreadable from core) and were hand-mirrored into config.toml for
//! core/web, a duplication that bred a whole class of restart-amnesia
//! merge bugs. `appearance.toml` in the character profile dir replaces
//! the mirrors: one file, one owner, no merge machinery — a plain
//! whole-file load/save, because assignments are not layered settings.
//!
//! The GUI layout keeps a COPY of these values so layouts/checkpoints
//! still carry a look with them (preset semantics: loading a layout that
//! states a skin applies it by writing this store). Fields migrate in
//! here slice by slice; active_skin and doll_image are the first.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppearanceSettings {
    /// Active GUI skin (dir name under `global/skins/`); None = plain
    /// theme colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_skin: Option<String>,
    /// Injury doll image override (pool-relative, "dolls/x.png"); None =
    /// the active skin's doll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doll_image: Option<String>,
}

impl AppearanceSettings {
    pub fn path(character: Option<&str>) -> anyhow::Result<PathBuf> {
        Ok(super::Config::profile_dir(character)?.join("appearance.toml"))
    }

    /// Load the store, or migrate it into existence from the legacy
    /// config.toml mirror keys (profile wins over global, the "" sentinel
    /// reads as None). Migration writes the file, so it runs once; a
    /// broken appearance.toml warns and loads as defaults rather than
    /// resurrecting the legacy values over the user's (unreadable) edits.
    pub fn load_or_migrate(character: Option<&str>) -> Self {
        let Ok(path) = Self::path(character) else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(settings) => settings,
                Err(err) => {
                    tracing::warn!("invalid {}: {} — using defaults", path.display(), err);
                    Self::default()
                }
            },
            Err(_) => {
                let migrated = Self::from_legacy_config(character);
                if migrated != Self::default() {
                    if let Err(err) = migrated.save(character) {
                        tracing::warn!("cannot write migrated {}: {}", path.display(), err);
                    }
                }
                migrated
            }
        }
    }

    /// Atomic whole-file save.
    pub fn save(&self, character: Option<&str>) -> anyhow::Result<()> {
        let path = Self::path(character)?;
        let contents = toml::to_string_pretty(self)
            .map_err(|err| anyhow::anyhow!("cannot serialize appearance: {}", err))?;
        super::write_atomic(&path, contents)
            .map_err(|err| anyhow::anyhow!("cannot write {}: {}", path.display(), err))?;
        Ok(())
    }

    /// One-time migration source: the raw legacy `active_skin` /
    /// `doll_image` keys in the character's config.toml (falling back to
    /// the global one). Raw-parsed because the Config struct no longer
    /// carries these fields.
    fn from_legacy_config(character: Option<&str>) -> Self {
        // Outer None = key absent (keep the lower layer); inner None = the
        // "" cleared-Option sentinel (an explicit "no skin" that must
        // override an inherited value).
        let legacy_key = |value: &toml::Value, key: &str| -> Option<Option<String>> {
            let text = value.get(key)?.as_str()?;
            Some((!text.trim().is_empty()).then(|| text.to_owned()))
        };
        let mut out = Self::default();
        // Global first, then the profile overrides it — same precedence
        // the old merge had.
        let mut paths: Vec<PathBuf> = Vec::new();
        if character.is_some() {
            if let Ok(path) = super::Config::config_path(None) {
                paths.push(path);
            }
        }
        if let Ok(path) = super::Config::config_path(character) {
            paths.push(path);
        }
        for path in paths {
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = toml::from_str::<toml::Value>(&contents) else {
                continue;
            };
            if let Some(skin) = legacy_key(&value, "active_skin") {
                out.active_skin = skin;
            }
            if let Some(doll) = legacy_key(&value, "doll_image") {
                out.doll_image = doll;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_guard(dir: &std::path::Path) -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::config::VELLUM_FE_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VELLUM_FE_DIR", dir);
        guard
    }

    #[test]
    fn save_load_roundtrip_and_broken_file_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = env_guard(dir.path());

        let settings = AppearanceSettings {
            active_skin: Some("stealth".into()),
            doll_image: Some("dolls/elf.png".into()),
        };
        settings.save(Some("Ultz")).unwrap();
        assert_eq!(AppearanceSettings::load_or_migrate(Some("Ultz")), settings);

        // Clearing persists: no merge machinery to resurrect old values.
        let cleared = AppearanceSettings::default();
        cleared.save(Some("Ultz")).unwrap();
        assert_eq!(AppearanceSettings::load_or_migrate(Some("Ultz")), cleared);

        // A broken file loads as defaults, never as the legacy migration.
        let path = AppearanceSettings::path(Some("Ultz")).unwrap();
        std::fs::write(&path, "not = = toml").unwrap();
        assert_eq!(
            AppearanceSettings::load_or_migrate(Some("Ultz")),
            AppearanceSettings::default()
        );

        std::env::remove_var("VELLUM_FE_DIR");
    }

    #[test]
    fn migrates_legacy_config_keys_once_profile_wins() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = env_guard(dir.path());

        // Legacy layout: global config.toml sets a skin, the profile
        // overrides it; doll_image only at the global level; the ""
        // sentinel (cleared Option) reads as None.
        let global = crate::config::Config::config_path(None).unwrap();
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        std::fs::write(
            &global,
            "active_skin = \"stale\"\ndoll_image = \"dolls/human.png\"\n",
        )
        .unwrap();
        let profile = crate::config::Config::config_path(Some("Ultz")).unwrap();
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(&profile, "active_skin = \"stealth\"\n").unwrap();

        let migrated = AppearanceSettings::load_or_migrate(Some("Ultz"));
        assert_eq!(migrated.active_skin.as_deref(), Some("stealth"));
        assert_eq!(migrated.doll_image.as_deref(), Some("dolls/human.png"));
        // Migration wrote the store; edits to the legacy file no longer
        // feed through.
        std::fs::write(&profile, "active_skin = \"other\"\n").unwrap();
        assert_eq!(
            AppearanceSettings::load_or_migrate(Some("Ultz"))
                .active_skin
                .as_deref(),
            Some("stealth")
        );

        // The "" sentinel migrates to None.
        let _ = std::fs::remove_file(AppearanceSettings::path(Some("Niffy")).unwrap());
        let profile = crate::config::Config::config_path(Some("Niffy")).unwrap();
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(&profile, "active_skin = \"\"\n").unwrap();
        assert_eq!(
            AppearanceSettings::load_or_migrate(Some("Niffy")).active_skin,
            None
        );

        std::env::remove_var("VELLUM_FE_DIR");
    }
}
