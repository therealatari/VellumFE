//! Stage scenes: a saved backdrop plus scenery props for the creature
//! field. Authored in Vellum Studio's Stage today; the client's future
//! scenes feature reads the same files through the same production
//! renderer (`render_creature_field_content` takes the scene as an
//! optional input, None everywhere in the game for now).
//!
//! Whole-file load/save like `config::appearance` — a scene is one
//! authored unit, not layered settings. Files live at
//! `global/scenes/<name>.toml` so scenes are shared across characters
//! like the rest of the pool.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One authored scene: what stands behind (and among) the creature cards.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StageScene {
    /// Backdrop image, pool-relative ("scenes/desert.png"), painted
    /// cover-fit behind the floor. None = no image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Backdrop fill color ("#rrggbb"), painted under the image (and the
    /// only backdrop when no image is set). None = the widget's default
    /// panel fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    /// Scenery props, interleaved with creature cards by depth.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<SceneProp>,
}

/// One placed prop: pool art standing on the floor like a creature card
/// does — feet-anchored at the ground projection of (x, z), scaled through
/// the same perspective the cards use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneProp {
    /// Pool-relative art ("scenery/rock.png"). A `<image>.toml` creature
    /// sidecar beside it supplies feet anchor / footprint / world size.
    pub image: String,
    /// Stage-space x of the foot point (0..solver STAGE_W, 880).
    pub x: f32,
    /// World depth on the camera axis (same units as FieldParams z0/dz).
    pub z: f32,
    /// World-height multiplier on the sidecar size (or the 1.0 default).
    #[serde(default = "default_prop_scale")]
    pub scale: f32,
}

fn default_prop_scale() -> f32 {
    1.0
}

impl StageScene {
    /// File path for a scene name. Names are file stems; path separators
    /// are refused rather than sanitized away (a scene named "a/b" would
    /// silently save somewhere its lister never looks).
    pub fn path(name: &str) -> anyhow::Result<PathBuf> {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\']) {
            anyhow::bail!("invalid scene name '{name}'");
        }
        Ok(super::Config::scenes_dir()?.join(format!("{name}.toml")))
    }

    /// Load one scene by name.
    pub fn load(name: &str) -> anyhow::Result<Self> {
        let path = Self::path(name)?;
        let contents = std::fs::read_to_string(&path)
            .map_err(|err| anyhow::anyhow!("cannot read {}: {}", path.display(), err))?;
        toml::from_str(&contents)
            .map_err(|err| anyhow::anyhow!("invalid {}: {}", path.display(), err))
    }

    /// Atomic whole-file save (creates `global/scenes/` on first use).
    pub fn save(&self, name: &str) -> anyhow::Result<()> {
        let path = Self::path(name)?;
        let contents = toml::to_string_pretty(self)
            .map_err(|err| anyhow::anyhow!("cannot serialize scene: {}", err))?;
        super::write_atomic(&path, contents)
            .map_err(|err| anyhow::anyhow!("cannot write {}: {}", path.display(), err))?;
        Ok(())
    }
}

/// Saved scene names (file stems), sorted. A missing folder is an empty
/// list, not an error.
pub fn list_scenes() -> Vec<String> {
    let Ok(dir) = super::Config::scenes_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("toml") {
                return None;
            }
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_roundtrip_and_listing() {
        let _guard = crate::config::VELLUM_FE_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("VELLUM_FE_DIR", dir.path());

        let scene = StageScene {
            background: Some("scenes/desert.png".into()),
            background_color: Some("#203040".into()),
            props: vec![
                SceneProp {
                    image: "scenery/rock.png".into(),
                    x: 440.0,
                    z: 3.1,
                    scale: 1.25,
                },
                SceneProp {
                    image: "scenery/cactus.png".into(),
                    x: 120.5,
                    z: 4.9,
                    scale: 1.0,
                },
            ],
        };
        scene.save("desert").unwrap();
        assert_eq!(StageScene::load("desert").unwrap(), scene);

        // A minimal file loads with defaults filled (scale = 1.0).
        let path = StageScene::path("bare").unwrap();
        std::fs::write(
            &path,
            "[[props]]\nimage = \"scenery/rock.png\"\nx = 10.0\nz = 2.5\n",
        )
        .unwrap();
        let bare = StageScene::load("bare").unwrap();
        assert_eq!(bare.background, None);
        assert_eq!(bare.props[0].scale, 1.0);

        assert_eq!(list_scenes(), ["bare", "desert"]);

        // Path separators in a name are refused, never sanitized.
        assert!(StageScene::path("a/b").is_err());
        assert!(StageScene::path("  ").is_err());

        std::env::remove_var("VELLUM_FE_DIR");
    }
}
