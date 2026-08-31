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

use super::skins::{CreatureFieldCamera, CreatureFieldSolver};

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
    /// The ground-plane camera this backdrop was tuned against — the
    /// perspective is a property of the painting, so it rides in the scene.
    /// Unset keys keep the solver's built-in defaults; the Studio's Save
    /// embeds all six so a saved scene reproduces exactly.
    #[serde(default, skip_serializing_if = "CreatureFieldCamera::is_empty")]
    pub camera: CreatureFieldCamera,
    /// Placement tunables for this scene ("this cave is narrow"). Same
    /// unset-keeps-default semantics as the camera.
    #[serde(default, skip_serializing_if = "CreatureFieldSolver::is_empty")]
    pub solver: CreatureFieldSolver,
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
    /// Draw a contact shadow under the prop (footprint ellipse). Props
    /// default shadowless — most scenery is painted with its own grounding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shadow: bool,
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

// ---------------------------------------------------------------------------
// Scene selection: the FILENAME is the binding (owner decision). A stem
// starting with a room uid binds that room — the rest of the stem is human
// garnish. Otherwise the stem is matched (sanitized, case-insensitive)
// against the room title, then the mapdb location, then the literal
// "default". No bindings file, no metadata: rename a file, rebind a room.
// ---------------------------------------------------------------------------

/// Both sides of every match go through this: lowercase, strip characters
/// that can't appear in a filename (defensive — Windows forbids them),
/// collapse runs of whitespace, and treat `_` as a space (filenames use
/// `_` for spaces; menus render the space back). Deterministic so
/// `matchable(stem) == matchable(title)` holds for generated stems.
pub fn matchable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() || ch == '_' {
            pending_space = true;
            continue;
        }
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

/// A filename stem for a wire string: same stripping as `matchable`,
/// original case kept, spaces written as `_` (owner convention — menus
/// render them back as spaces via [`display_name`]).
pub fn filename_stem(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() || ch == '_' {
            pending_space = true;
            continue;
        }
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push('_');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

/// How a scene stem reads in menus and filter lists: `_` back to space.
pub fn display_name(stem: &str) -> String {
    stem.replace('_', " ")
}

/// The room uid a scene stem binds, when it leads with one: digits up to
/// the first non-digit. `"47009 - Kitchen Garden"` -> 47009; a stem with
/// no leading digits binds by text instead.
pub fn stem_uid(stem: &str) -> Option<i64> {
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Pick the scene for a room from the saved scene names:
/// uid -> room title -> mapdb location -> "default". Text tiers compare
/// sanitized and case-insensitive; ties break to the first name in sorted
/// order (the caller passes `list_scenes()`, which is sorted).
pub fn resolve_scene<'a>(
    names: &'a [String],
    uid: Option<i64>,
    title: Option<&str>,
    location: Option<&str>,
) -> Option<&'a str> {
    if let Some(uid) = uid {
        if let Some(name) = names.iter().find(|n| stem_uid(n) == Some(uid)) {
            return Some(name);
        }
    }
    let text_match = |wanted: Option<&str>| -> Option<&'a str> {
        let wanted = matchable(wanted?);
        if wanted.is_empty() {
            return None;
        }
        names
            .iter()
            .find(|n| stem_uid(n).is_none() && matchable(n) == wanted)
            .map(String::as_str)
    };
    text_match(title)
        .or_else(|| text_match(location))
        .or_else(|| {
            names
                .iter()
                .find(|n| matchable(n) == "default")
                .map(String::as_str)
        })
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
                    shadow: true,
                },
                SceneProp {
                    image: "scenery/cactus.png".into(),
                    x: 120.5,
                    z: 4.9,
                    scale: 1.0,
                    shadow: false,
                },
            ],
            camera: CreatureFieldCamera {
                focal: Some(450.0),
                horizon: Some(160.0),
                ..Default::default()
            },
            solver: CreatureFieldSolver {
                zone: Some("grid".into()),
                relax_steps: Some(7),
                ..Default::default()
            },
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
        assert!(!bare.props[0].shadow, "shadows are opt-in");
        assert!(bare.camera.is_empty(), "no camera table = empty camera");
        assert!(bare.solver.is_empty());

        assert_eq!(list_scenes(), ["bare", "desert"]);

        // Path separators in a name are refused, never sanitized.
        assert!(StageScene::path("a/b").is_err());
        assert!(StageScene::path("  ").is_err());

        std::env::remove_var("VELLUM_FE_DIR");
    }

    #[test]
    fn filename_resolution_uid_then_title_then_location_then_default() {
        let names: Vec<String> = [
            "47009 - Kitchen Garden, Castle Anwyn",
            "Castle Anwyn, Kitchen Garden",
            "Wehnimer's Landing",
            "default",
        ]
        .map(str::to_string)
        .to_vec();
        // Leading uid wins; the rest of the stem is human garnish.
        assert_eq!(
            resolve_scene(&names, Some(47009), Some("Anywhere"), Some("Anywhere")),
            Some("47009 - Kitchen Garden, Castle Anwyn")
        );
        // No uid match -> exact (sanitized, case-insensitive) title.
        assert_eq!(
            resolve_scene(&names, Some(1), Some("castle anwyn, kitchen garden"), None),
            Some("Castle Anwyn, Kitchen Garden")
        );
        // Then mapdb location, then the literal default.
        assert_eq!(
            resolve_scene(&names, None, Some("Elsewhere"), Some("Wehnimer's Landing")),
            Some("Wehnimer's Landing")
        );
        assert_eq!(resolve_scene(&names, None, None, None), Some("default"));
        assert_eq!(resolve_scene(&[], None, Some("x"), None), None);
    }

    #[test]
    fn matchable_strips_filename_illegal_chars_and_case() {
        // A title with characters that can't be in a filename still matches
        // the stem Studio would have written for it.
        let title = "Sewers:  Junction \"North\"";
        assert_eq!(matchable(title), matchable(&filename_stem(title)));
        assert_eq!(matchable("  Barley   Field "), "barley field");
        // Filenames carry `_` for spaces; matching and display undo it.
        assert_eq!(
            filename_stem("Castle Anwyn, Kitchen Garden"),
            "Castle_Anwyn,_Kitchen_Garden"
        );
        assert_eq!(matchable("Castle_Anwyn,_Kitchen_Garden"), matchable("Castle Anwyn, Kitchen Garden"));
        assert_eq!(
            display_name("Castle_Anwyn,_Kitchen_Garden"),
            "Castle Anwyn, Kitchen Garden"
        );
        // Numbered stems never text-match (they're uid bindings).
        assert_eq!(stem_uid("47009 - Anything"), Some(47009));
        assert_eq!(stem_uid("Barley Field"), None);
        assert_eq!(
            resolve_scene(
                &["47009 - Barley Field".to_string()],
                None,
                Some("47009 - Barley Field"),
                None
            ),
            None,
            "uid-stem scenes are invisible to the text tiers"
        );
    }
}
