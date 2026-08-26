//! One image decoder and one incrementally-invalidated texture cache for
//! skin and pool art (skin-system overhaul, phase 2).
//!
//! Before this module, the GUI had four independent decode paths and the
//! main texture cache was torn down wholesale on ANY appearance change —
//! toggling one checkbox re-decoded every loaded image. `ImageStore::sync`
//! replaces the teardown: entries whose file is unchanged keep their
//! texture (and their `TextureId`), dropped entries free theirs, and only
//! new or stale files decode. The decode helpers below are the single
//! decode path for everything else (thumbnails, palette sampling,
//! creature art), so "how VellumFE reads an image file" has one answer.
//!
//! The animated emoji/inline-image caches stay separate on purpose: they
//! are frame-sequence decoders with their own lifecycle and were never
//! coupled to skins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Key suffix for a desaturated twin, shared with `SkinState`'s lookup
/// keys ("<path>#gray").
pub const GRAY_SUFFIX: &str = "#gray";

/// One image the store should hold: the cache/lookup key (manifest-relative
/// path, plus `#gray` for twins), the resolved file it comes from, and
/// whether to desaturate. Order matters to `sync`: list a gray twin AFTER
/// its base so a failed base suppresses the twin's decode and warning.
pub struct WantedImage {
    pub key: String,
    pub path: PathBuf,
    pub gray: bool,
}

struct Entry {
    path: PathBuf,
    mtime: Option<std::time::SystemTime>,
    /// None records a decode failure so a bad file warns once per change,
    /// not once per frame.
    handle: Option<egui::TextureHandle>,
}

/// Path-keyed texture cache with incremental sync.
#[derive(Default)]
pub struct ImageStore {
    entries: HashMap<String, Entry>,
}

impl ImageStore {
    /// Bring the cache in line with `wanted`: entries no longer wanted are
    /// dropped (freeing their textures), entries whose resolved file and
    /// mtime are unchanged are kept untouched (stable `TextureId`), and
    /// everything else (re)loads. A file listed for both color and gray
    /// decodes once. `label` names the load context in warnings and
    /// texture debug names (the skin name, or "shared-icons").
    pub fn sync(&mut self, ctx: &egui::Context, wanted: &[WantedImage], label: &str) {
        let keys: std::collections::HashSet<&str> =
            wanted.iter().map(|want| want.key.as_str()).collect();
        self.entries.retain(|key, _| keys.contains(key.as_str()));

        // Per-sync decode memo: color + gray of the same file share one
        // decode. Rc because the gray twin clones pixels to recolor.
        let mut decoded: HashMap<PathBuf, Option<Rc<image::RgbaImage>>> = HashMap::new();
        for want in wanted {
            let mtime = std::fs::metadata(&want.path)
                .and_then(|meta| meta.modified())
                .ok();
            if let Some(entry) = self.entries.get(&want.key) {
                if entry.path == want.path && entry.mtime == mtime {
                    continue;
                }
            }
            // A gray twin whose base already failed records the failure
            // without a second decode (one warning is enough).
            if want.gray {
                let base = want.key.strip_suffix(GRAY_SUFFIX).unwrap_or(&want.key);
                if self
                    .entries
                    .get(base)
                    .is_some_and(|entry| entry.handle.is_none())
                {
                    self.entries.insert(
                        want.key.clone(),
                        Entry {
                            path: want.path.clone(),
                            mtime,
                            handle: None,
                        },
                    );
                    continue;
                }
            }
            let rgba = decoded
                .entry(want.path.clone())
                .or_insert_with(|| decode_rgba_logged(&want.path, label).map(Rc::new))
                .clone();
            let handle = rgba.map(|rgba| {
                let color_image = if want.gray {
                    let mut gray = (*rgba).clone();
                    desaturate_in_place(&mut gray);
                    to_color_image(&gray)
                } else {
                    to_color_image(&rgba)
                };
                ctx.load_texture(
                    format!("skin:{label}:{}", want.key),
                    color_image,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.entries.insert(
                want.key.clone(),
                Entry {
                    path: want.path.clone(),
                    mtime,
                    handle,
                },
            );
        }
    }

    /// The loaded texture for `key`, if it loaded successfully.
    pub fn texture(&self, key: &str) -> Option<&egui::TextureHandle> {
        self.entries.get(key)?.handle.as_ref()
    }
}

/// Decode one image file to RGBA. Quiet: a missing or broken file is None
/// (callers that owe the user a warning use `decode_rgba_logged`).
pub fn decode_rgba(path: &Path) -> Option<image::RgbaImage> {
    let bytes = std::fs::read(path).ok()?;
    Some(image::load_from_memory(&bytes).ok()?.to_rgba8())
}

/// Decode one image file to RGBA, warning (with the load context) on a
/// missing or undecodable file.
pub fn decode_rgba_logged(path: &Path, label: &str) -> Option<image::RgbaImage> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!("'{}': cannot read {}: {}", label, path.display(), err);
            return None;
        }
    };
    match image::load_from_memory(&bytes) {
        Ok(decoded) => Some(decoded.to_rgba8()),
        Err(err) => {
            tracing::warn!("'{}': cannot decode {}: {}", label, path.display(), err);
            None
        }
    }
}

/// Load one file straight to a texture, outside the synced cache (creature
/// overlays and other lazily-resolved art with their own caches).
pub fn load_texture_file(
    ctx: &egui::Context,
    path: &Path,
    texture_name: &str,
    label: &str,
) -> Option<egui::TextureHandle> {
    let rgba = decode_rgba_logged(path, label)?;
    Some(ctx.load_texture(
        texture_name.to_owned(),
        to_color_image(&rgba),
        egui::TextureOptions::LINEAR,
    ))
}

/// Luminance recolor, alpha preserved (barbar's gs variant).
fn desaturate_in_place(rgba: &mut image::RgbaImage) {
    for px in rgba.pixels_mut() {
        let [r, g, b, a] = px.0;
        let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).round() as u8;
        px.0 = [luma, luma, luma, a];
    }
}

fn to_color_image(rgba: &image::RgbaImage) -> egui::ColorImage {
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, px: u32) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let pixels = vec![0xffu8; (px * px * 4) as usize];
        image::save_buffer(path, &pixels, px, px, image::ExtendedColorType::Rgba8).unwrap();
    }

    fn want(key: &str, path: PathBuf, gray: bool) -> WantedImage {
        WantedImage {
            key: key.to_string(),
            path,
            gray,
        }
    }

    #[test]
    fn sync_keeps_unchanged_textures_and_their_ids() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        write_png(&a, 2);
        write_png(&b, 2);

        let mut store = ImageStore::default();
        store.sync(
            &ctx,
            &[want("a", a.clone(), false), want("b", b.clone(), false)],
            "test",
        );
        let id_a = store.texture("a").unwrap().id();

        // Re-sync with the same set: a kept texture keeps its identity —
        // the whole point over the old teardown-and-reload.
        store.sync(
            &ctx,
            &[want("a", a.clone(), false), want("b", b.clone(), false)],
            "test",
        );
        assert_eq!(store.texture("a").unwrap().id(), id_a);

        // Dropping b from the wanted set evicts it; a is still untouched.
        store.sync(&ctx, &[want("a", a.clone(), false)], "test");
        assert!(store.texture("b").is_none());
        assert_eq!(store.texture("a").unwrap().id(), id_a);
    }

    #[test]
    fn sync_reloads_when_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let a = dir.path().join("a.png");
        write_png(&a, 2);

        let mut store = ImageStore::default();
        store.sync(&ctx, &[want("a", a.clone(), false)], "test");
        assert_eq!(store.texture("a").unwrap().size_vec2(), egui::vec2(2.0, 2.0));

        // Same key, new content + mtime: the entry reloads.
        write_png(&a, 4);
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let file = std::fs::File::options().write(true).open(&a).unwrap();
        file.set_modified(future).unwrap();
        drop(file);
        store.sync(&ctx, &[want("a", a.clone(), false)], "test");
        assert_eq!(store.texture("a").unwrap().size_vec2(), egui::vec2(4.0, 4.0));

        // Same key resolved to a DIFFERENT file (skin switch): reloads too.
        let other = dir.path().join("other.png");
        write_png(&other, 8);
        store.sync(&ctx, &[want("a", other.clone(), false)], "test");
        assert_eq!(store.texture("a").unwrap().size_vec2(), egui::vec2(8.0, 8.0));
    }

    #[test]
    fn gray_twin_shares_the_decode_and_skips_when_base_failed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let a = dir.path().join("a.png");
        write_png(&a, 2);

        let mut store = ImageStore::default();
        store.sync(
            &ctx,
            &[
                want("a", a.clone(), false),
                want("a#gray", a.clone(), true),
            ],
            "test",
        );
        assert!(store.texture("a").is_some());
        assert!(store.texture("a#gray").is_some());

        // A missing base records failure for both; the twin never decodes.
        let ghost = dir.path().join("ghost.png");
        store.sync(
            &ctx,
            &[
                want("g", ghost.clone(), false),
                want("g#gray", ghost.clone(), true),
            ],
            "test",
        );
        assert!(store.texture("g").is_none());
        assert!(store.texture("g#gray").is_none());
    }
}
