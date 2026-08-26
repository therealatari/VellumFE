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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppearanceSettings {
    /// Active GUI skin (dir name under `global/skins/`); None = plain
    /// theme colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_skin: Option<String>,
    /// Injury doll image override (pool-relative, "dolls/x.png"); None =
    /// the active skin's doll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doll_image: Option<String>,
    /// Compass art set from the pool (`compass/<set>/`); None follows the
    /// active skin's `[compass]`. "none" strips compass art entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compass_set: Option<String>,
    /// Global default frame for windows without a per-window override (a
    /// skin `[frames.*]` name or pool frame stem).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_frame: Option<String>,
    /// Global default background (pool-relative path, or "none" to
    /// suppress skin backgrounds everywhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_background: Option<String>,
    /// Render the injury doll's art in grayscale (dots keep color).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub doll_grayscale: bool,
    /// Hand widget icon size in points.
    #[serde(default = "default_hand_icon_size")]
    pub hand_icon_size: f32,
    /// Status icon art selection (pool set + per-indicator overrides +
    /// grayscale rules).
    #[serde(default, skip_serializing_if = "StatusIconSettings::is_default")]
    pub status_icons: StatusIconSettings,
    /// Dialog-control face assignments: control key ("button",
    /// "button.hover", "dropdown", "tab", "link", "progressbar",
    /// "titlebar") -> pool frame stem. Wins over the skin's `[controls]`.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub control_frames: std::collections::HashMap<String, String>,
    /// Decorative edge-overlay set from the pool (`edges/<set>/`); None
    /// follows the active skin's `[edges]`, "none" strips edge art.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_set: Option<String>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            active_skin: None,
            doll_image: None,
            compass_set: None,
            default_frame: None,
            default_background: None,
            doll_grayscale: false,
            hand_icon_size: default_hand_icon_size(),
            status_icons: StatusIconSettings::default(),
            control_frames: std::collections::HashMap::new(),
            edge_set: None,
        }
    }
}

/// Default matches Wrayth, whose hand icons span about two text lines.
pub fn default_hand_icon_size() -> f32 {
    30.0
}

/// Which art status indicators use, resolved ahead of the built-in vector
/// pictograms: an optional statusicons pool set supplies defaults by glyph
/// name, and per-indicator overrides pin any icon reference.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusIconSettings {
    /// Pool set (`global/images/statusicons/<set>/`); None = no pool
    /// defaults (skin `[icons]` / vector only).
    #[serde(default)]
    pub set: Option<String>,
    /// Indicator id (any case) -> icon override. `Default` entries are
    /// dropped on save; absence means "no override".
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, crate::data::IconRef>,
    /// Inactive statuses render their icon in grayscale (instead of the
    /// default alpha dim). Off = no gray twins are ever built (unless a
    /// per-indicator override below turns one on).
    #[serde(default)]
    pub gray_inactive: bool,
    /// Per-indicator exceptions to `gray_inactive` (indicator id → force
    /// on/off). Absent = follow the global toggle.
    #[serde(default)]
    pub gray_overrides: std::collections::HashMap<String, bool>,
}

impl StatusIconSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Repoint image overrides whose art moved into a set folder, returning
    /// whether anything changed (the caller persists if so).
    ///
    /// An override stores a pool path (`statusicons/runic_stunned.png`).
    /// Foldering the pool makes that path stale, and a stale override
    /// renders as a missing icon rather than an error — so this rewrites to
    /// the foldered path when the flat file is gone and the set folder has
    /// the same role.
    ///
    /// Deliberately resolved against the pool as it is now, not against the
    /// migration's return value: a user who folders their art by hand, or
    /// whose migration half-completed, gets healed the same way. Overrides
    /// whose file still exists are never touched.
    pub fn rewrite_pool_paths(&mut self) -> bool {
        let mut changed = false;
        for icon in self.overrides.values_mut() {
            let crate::data::IconRef::Image { path } = icon else {
                continue;
            };
            // "statusicons/runic_stunned.png" -> category "statusicons",
            // file "runic_stunned.png". A path already containing a set
            // folder has two slashes and is left alone.
            let Some((category, file)) = path.split_once('/') else {
                continue;
            };
            if file.contains('/') {
                continue;
            }
            let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
            let Some((set, role)) = stem.split_once('_') else {
                continue;
            };
            if set.is_empty() || role.is_empty() {
                continue;
            }
            // set_members is keyed by role, so the lookup is direct — and it
            // returns the foldered path only if that art actually exists.
            let Some(foldered) =
                crate::config::pool::set_members(category, set).remove(&role.to_ascii_lowercase())
            else {
                continue;
            };
            if foldered != *path {
                *path = foldered;
                changed = true;
            }
        }
        changed
    }

    /// Whether this indicator grays out when inactive: its override if it
    /// has one, else the global toggle.
    pub fn gray_for(&self, indicator_id: &str) -> bool {
        self.gray_overrides
            .get(indicator_id)
            .or_else(|| self.gray_overrides.get(&indicator_id.to_ascii_uppercase()))
            .copied()
            .unwrap_or(self.gray_inactive)
    }

    /// Whether ANY indicator needs a gray twin built.
    pub fn any_gray(&self) -> bool {
        self.gray_inactive || self.gray_overrides.values().any(|on| *on)
    }
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
            ..Default::default()
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
