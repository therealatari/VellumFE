//! Shareable skin packs (skin-system overhaul, phase 5).
//!
//! A skin pack is a single zip: an assignments-only `skin.toml` plus art
//! files laid out exactly as the shared image pool expects them
//! (`dolls/elf.png`, `frames/ornate.png`, `compass/<set>/rose.png`,
//! `creatures/<noun>/<token>.png`, …). PNGs carry their calibration as
//! embedded `vellum-meta` chunks (see `png_meta`); explicit `.toml`
//! sidecar entries are also accepted and win over the embedded copy.
//!
//! Unlike the legacy skin format there is no live manifest: installing a
//! pack copies the art into the pool (never overwriting different
//! existing art — colliding units are renamed, convention art is
//! skipped), then writes the assignment slots into the per-character
//! appearance store. The pack itself is inert afterwards — the pool and
//! the appearance store are the only runtime sources. The export zip IS
//! the jinx `skin` pack: one format for `.exportskin`, Discord sharing,
//! vellum-assets submission, and `.jinx install`.
//!
//! Pure config layer: no UI-toolkit imports, no image decoding, so the
//! validator runs headless (`vellum-fe validate-skin`, vellum-assets CI).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::appearance::AppearanceSettings;
use super::pool::{
    BackgroundSidecar, CreatureSidecar, DollSidecar, EdgeSidecar, FrameSidecar, SidecarKind,
};
use super::Config;

/// Newest pack format this build understands. Readers refuse newer packs
/// (a downgrade can't know what it would silently drop).
pub const FORMAT_VERSION: u32 = 1;

/// Image extensions accepted in a pack (matches the pool scanner).
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

/// Pool categories whose members live in `<category>/<set>/` folders.
const SET_CATEGORIES: &[&str] = &["compass", "statusicons", "hands", "edges"];

/// Categories resolved by convention (name/token lookup), never by an
/// assignment slot. Colliding files here are skipped on install — renaming
/// would break the token match that makes them work at all.
const CONVENTION_CATEGORIES: &[&str] = &["creatures", "scenes", "scenery"];

/// Every top-level folder a pack may ship art under. Anything else is a
/// warning (forward compatibility: newer packs may know categories this
/// build doesn't).
fn known_categories() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = Config::IMAGE_CATEGORIES.to_vec();
    all.extend(["edges", "creatures"]);
    all
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// The pack's `skin.toml`. The `format` key is what distinguishes a pack
/// from a legacy live-manifest skin zip (which has `[meta]`/art tables but
/// never a top-level `format`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinPackManifest {
    /// Defaulted (not required) so a legacy manifest parses far enough to
    /// produce the "this is a legacy skin" error instead of a raw serde
    /// missing-field message.
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub meta: PackMeta,
    #[serde(default)]
    pub assignments: Assignments,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// The appearance slots a pack sets on install — the same vocabulary as
/// [`AppearanceSettings`], minus `active_skin` (packs ARE the look; there
/// is no live skin to point at) and minus per-indicator icon overrides
/// (personal pins that shouldn't travel).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Assignments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doll_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compass_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_set: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub doll_grayscale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hand_icon_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_icon_set: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub status_gray_inactive: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub control_frames: HashMap<String, String>,
}

impl Assignments {
    /// Snapshot the current appearance into pack assignments.
    pub fn from_appearance(appearance: &AppearanceSettings) -> Self {
        Self {
            doll_image: appearance.doll_image.clone(),
            compass_set: appearance.compass_set.clone(),
            default_frame: appearance.default_frame.clone(),
            default_background: appearance.default_background.clone(),
            edge_set: appearance.edge_set.clone(),
            doll_grayscale: appearance.doll_grayscale,
            hand_icon_size: Some(appearance.hand_icon_size),
            status_icon_set: appearance.status_icons.set.clone(),
            status_gray_inactive: appearance.status_icons.gray_inactive,
            control_frames: appearance.control_frames.clone(),
        }
    }

    /// Write these assignments into an appearance store. `active_skin` is
    /// cleared — under the preset model the assignments themselves are the
    /// look. Per-indicator status-icon overrides and gray exceptions are
    /// the user's personal pins and are left alone.
    pub fn apply_to(&self, appearance: &mut AppearanceSettings) {
        appearance.active_skin = None;
        appearance.doll_image = self.doll_image.clone();
        appearance.compass_set = self.compass_set.clone();
        appearance.default_frame = self.default_frame.clone();
        appearance.default_background = self.default_background.clone();
        appearance.edge_set = self.edge_set.clone();
        appearance.doll_grayscale = self.doll_grayscale;
        if let Some(size) = self.hand_icon_size {
            appearance.hand_icon_size = size;
        }
        appearance.status_icons.set = self.status_icon_set.clone();
        appearance.status_icons.gray_inactive = self.status_gray_inactive;
        appearance.control_frames = self.control_frames.clone();
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A pack in memory: parsed manifest plus every art/sidecar entry, keyed
/// by forward-slash relative path (validated safe).
#[derive(Debug, Clone)]
pub struct SkinPack {
    pub manifest: SkinPackManifest,
    /// Relative path (forward slashes) -> bytes. `skin.toml` excluded.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Whether a skin zip is the packed (phase-5) format: its `skin.toml` has
/// a top-level `format` key. Used by the jinx installer to route between
/// this module and the legacy extract-to-skins-dir path.
pub fn is_pack_format(zip_bytes: &[u8]) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)) else {
        return false;
    };
    let Ok(mut entry) = zip.by_name("skin.toml") else {
        return false;
    };
    let mut text = String::new();
    if entry.read_to_string(&mut text).is_err() {
        return false;
    }
    toml::from_str::<toml::Value>(&text)
        .ok()
        .is_some_and(|value| value.get("format").is_some())
}

/// Parse a pack zip. Every entry path is validated to stay inside the
/// extraction root (same discipline as `core::jinx::bundle`); a single bad
/// entry rejects the whole archive.
pub fn read_pack_bytes(zip_bytes: &[u8]) -> Result<SkinPack, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("not a valid skin pack zip: {e}"))?;
    let mut manifest_text: Option<String> = None;
    let mut files = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("reading pack entry {i}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let raw = entry.name().to_string();
        let rel = safe_relative(&raw).ok_or_else(|| format!("unsafe path in pack: '{raw}'"))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("reading '{raw}': {e}"))?;
        if rel == "skin.toml" {
            manifest_text = Some(
                String::from_utf8(bytes).map_err(|_| "skin.toml is not UTF-8".to_string())?,
            );
        } else {
            files.insert(rel, bytes);
        }
    }
    let manifest_text = manifest_text.ok_or_else(|| "pack has no skin.toml".to_string())?;
    let manifest = parse_manifest(&manifest_text)?;
    Ok(SkinPack { manifest, files })
}

/// Parse a pack from a directory laid out like the zip (used by
/// `validate-skin` on an unzipped pack and by tests).
pub fn read_pack_dir(root: &Path) -> Result<SkinPack, String> {
    let manifest_path = root.join("skin.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let manifest = parse_manifest(&manifest_text)?;
    let mut files = BTreeMap::new();
    collect_dir_files(root, root, &mut files)?;
    files.remove("skin.toml");
    Ok(SkinPack { manifest, files })
}

fn parse_manifest(text: &str) -> Result<SkinPackManifest, String> {
    let manifest: SkinPackManifest =
        toml::from_str(text).map_err(|e| format!("skin.toml does not parse: {e}"))?;
    if manifest.format == 0 {
        return Err("skin.toml has no `format` key — this is a legacy skin, not a pack \
                    (use `vellum-fe migrate-skin` to convert it)"
            .to_string());
    }
    if manifest.format > FORMAT_VERSION {
        return Err(format!(
            "pack format {} is newer than this build understands (max {})",
            manifest.format, FORMAT_VERSION
        ));
    }
    Ok(manifest)
}

fn collect_dir_files(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dir_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            let key = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            out.insert(key, bytes);
        }
    }
    Ok(())
}

/// A zip entry path validated to stay within the extraction root:
/// relative, no `..`/`.`/empty components, no backslashes, no absolute
/// prefix. Returns the normalized forward-slash path.
fn safe_relative(raw: &str) -> Option<String> {
    if raw.contains('\\') || raw.starts_with('/') || raw.is_empty() {
        return None;
    }
    for part in raw.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return None;
        }
    }
    Some(raw.to_string())
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validator output. Errors block install/export; warnings inform.
#[derive(Debug, Default, Clone)]
pub struct Findings {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Findings {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// The "none" sentinel: an assignment that deliberately strips art needs
/// no file to back it.
fn is_none_sentinel(value: &str) -> bool {
    value.eq_ignore_ascii_case("none")
}

/// Validate a pack: every assignment resolves inside the pack, every
/// entry lives under a known category, embedded/explicit metadata parses
/// per its category's typed schema with a matching `kind`. Run at export
/// time, install time, and by `vellum-fe validate-skin` (vellum-assets CI).
pub fn validate(pack: &SkinPack) -> Findings {
    let mut findings = Findings::default();
    let known = known_categories();

    if pack.manifest.meta.name.trim().is_empty() {
        findings
            .warnings
            .push("meta.name is empty — the pack will install under its file name".into());
    }

    // Entry hygiene: category placement + per-file metadata.
    for (path, bytes) in &pack.files {
        let category = path.split('/').next().unwrap_or_default();
        if !known.contains(&category) {
            findings.warnings.push(format!(
                "'{path}': unknown category folder '{category}' (installed verbatim, nothing will use it)"
            ));
            continue;
        }
        if path.to_ascii_lowercase().ends_with(".png") {
            if let Some(meta) = super::png_meta::read_embedded_bytes(bytes) {
                if let Err(err) = check_metadata(category, &meta) {
                    findings
                        .errors
                        .push(format!("'{path}': embedded metadata invalid: {err}"));
                }
            }
        } else if path.to_ascii_lowercase().ends_with(".toml") {
            let stem = &path[..path.len() - ".toml".len()];
            let has_image = IMAGE_EXTS
                .iter()
                .any(|ext| pack.files.contains_key(&format!("{stem}.{ext}")));
            if !has_image {
                findings
                    .warnings
                    .push(format!("'{path}': sidecar with no matching image in the pack"));
            }
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    if let Err(err) = check_metadata(category, text) {
                        findings.errors.push(format!("'{path}': {err}"));
                    }
                }
                Err(_) => findings.errors.push(format!("'{path}': sidecar is not UTF-8")),
            }
        }
    }

    // Assignments must resolve inside the pack — a shareable pack carries
    // the art it names (the recipient's pool can't be assumed).
    let a = &pack.manifest.assignments;
    for (label, path) in [
        ("assignments.doll_image", &a.doll_image),
        ("assignments.default_background", &a.default_background),
    ] {
        if let Some(path) = path {
            if !is_none_sentinel(path) && !pack.files.contains_key(path) {
                findings
                    .errors
                    .push(format!("{label} = '{path}' is not in the pack"));
            }
        }
    }
    let frame_stems: Vec<(String, String)> = a
        .default_frame
        .iter()
        .map(|stem| ("assignments.default_frame".to_string(), stem.clone()))
        .chain(
            a.control_frames
                .iter()
                .map(|(control, stem)| (format!("assignments.control_frames.{control}"), stem.clone())),
        )
        .collect();
    for (label, stem) in frame_stems {
        if is_none_sentinel(&stem) {
            continue;
        }
        let found = IMAGE_EXTS.iter().any(|ext| {
            pack.files
                .keys()
                .any(|key| key.eq_ignore_ascii_case(&format!("frames/{stem}.{ext}")))
        });
        if !found {
            findings
                .errors
                .push(format!("{label} = '{stem}' has no frames/{stem}.* in the pack"));
        }
    }
    for (label, category, set, required_role) in [
        ("assignments.compass_set", "compass", &a.compass_set, Some("rose")),
        ("assignments.edge_set", "edges", &a.edge_set, None),
        ("assignments.status_icon_set", "statusicons", &a.status_icon_set, None),
    ] {
        let Some(set) = set else { continue };
        if is_none_sentinel(set) {
            continue;
        }
        let prefix = format!("{category}/{set}/").to_ascii_lowercase();
        let members: Vec<&String> = pack
            .files
            .keys()
            .filter(|key| key.to_ascii_lowercase().starts_with(&prefix))
            .collect();
        if members.is_empty() {
            findings
                .errors
                .push(format!("{label} = '{set}' has no {category}/{set}/ folder in the pack"));
            continue;
        }
        if let Some(role) = required_role {
            let has_role = members.iter().any(|key| {
                key.rsplit('/')
                    .next()
                    .and_then(|file| file.rsplit_once('.'))
                    .is_some_and(|(stem, _)| stem.eq_ignore_ascii_case(role))
            });
            if !has_role {
                findings.errors.push(format!(
                    "{label} = '{set}': the set has no {role}.* (required for this category)"
                ));
            }
        }
    }

    findings
}

/// Parse metadata text against its category's typed sidecar schema. For
/// categories without a schema, only well-formed TOML is required.
fn check_metadata(category: &str, text: &str) -> Result<(), String> {
    fn typed<T: serde::de::DeserializeOwned + SidecarKind>(text: &str) -> Result<(), String> {
        let value: T = toml::from_str(text).map_err(|e| e.to_string())?;
        match value.declared_kind() {
            Some(kind) if !kind.eq_ignore_ascii_case(T::KIND) => Err(format!(
                "declares kind '{kind}', this category wants '{}'",
                T::KIND
            )),
            _ => Ok(()),
        }
    }
    match category {
        "dolls" => typed::<DollSidecar>(text),
        "frames" => typed::<FrameSidecar>(text),
        // Scenery props anchor and ground exactly like creature sprites
        // (feet anchor + footprint + world size), so they share the schema.
        "creatures" | "scenery" => typed::<CreatureSidecar>(text),
        "edges" => typed::<EdgeSidecar>(text),
        "backgrounds" => typed::<BackgroundSidecar>(text),
        _ => toml::from_str::<toml::Value>(text)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

/// What an install did, for reporting to the user.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InstallReport {
    /// Pool-relative paths written.
    pub installed: Vec<String>,
    /// (pack path, pool path actually written) for renamed collisions.
    pub renamed: Vec<(String, String)>,
    /// Paths skipped because identical art already exists.
    pub identical: Vec<String>,
    /// Paths skipped because DIFFERENT art exists that can't be renamed
    /// (convention categories, where the name IS the lookup key).
    pub kept_existing: Vec<String>,
    pub warnings: Vec<String>,
    /// The assignments actually applied — rewritten where collisions
    /// renamed the art they point at.
    pub assignments: Assignments,
}

/// Copy a validated pack's art into the shared pool and return the
/// (possibly rewritten) assignments. Never overwrites different existing
/// art: assignable flat files and set folders are renamed (`stem-2`,
/// `set-2`, …) with the assignments rewritten to match; convention art
/// (creatures, scenes) keeps the existing file and warns. Identical bytes
/// are skipped. PNGs written without an explicit sidecar entry get one
/// extracted from their embedded metadata.
///
/// Assignments are NOT persisted here — call [`apply_assignments`] with
/// the report's `assignments` (the jinx worker installs files off-thread
/// and hands the assignments to the main thread to apply).
pub fn install_files(pack: &SkinPack) -> anyhow::Result<InstallReport> {
    let findings = validate(pack);
    if !findings.ok() {
        anyhow::bail!("pack failed validation:\n  {}", findings.errors.join("\n  "));
    }
    let pool_root = Config::global_images_dir()?;
    let mut report = InstallReport {
        warnings: findings.warnings.clone(),
        assignments: pack.manifest.assignments.clone(),
        ..Default::default()
    };

    // Group entries into rename units: a set folder moves as one, a flat
    // image moves with its sidecar, convention art never moves.
    #[derive(PartialEq)]
    enum Unit {
        Set { category: String, set: String },
        Flat { category: String, stem: String },
        Convention,
    }
    let unit_of = |path: &str| -> Unit {
        let mut parts = path.split('/');
        let category = parts.next().unwrap_or_default().to_string();
        if CONVENTION_CATEGORIES.contains(&category.as_str()) {
            return Unit::Convention;
        }
        let rest: Vec<&str> = parts.collect();
        if rest.len() >= 2 && SET_CATEGORIES.contains(&category.as_str()) {
            return Unit::Set {
                category,
                set: rest[0].to_string(),
            };
        }
        let file = rest.last().copied().unwrap_or_default();
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        Unit::Flat {
            category,
            stem: stem.to_string(),
        }
    };

    // Collect distinct units in insertion order.
    let mut units: Vec<(Unit, Vec<String>)> = Vec::new();
    for path in pack.files.keys() {
        let unit = unit_of(path);
        if let Some(entry) = units.iter_mut().find(|(u, _)| *u == unit) {
            entry.1.push(path.clone());
        } else {
            units.push((unit, vec![path.clone()]));
        }
    }

    let differs = |dest: &Path, bytes: &[u8]| -> bool {
        match std::fs::read(dest) {
            Ok(existing) => existing != bytes,
            Err(_) => false,
        }
    };
    let write_entry = |report: &mut InstallReport,
                       pack_path: &str,
                       dest_rel: &str,
                       bytes: &[u8]|
     -> anyhow::Result<()> {
        let dest = pool_root.join(dest_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        super::write_atomic(&dest, bytes)
            .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", dest.display()))?;
        if dest_rel == pack_path {
            report.installed.push(dest_rel.to_string());
        } else {
            report.renamed.push((pack_path.to_string(), dest_rel.to_string()));
        }
        // Extract embedded metadata to a sidecar when the pack shipped
        // none — the pool working-copy convention (read_sidecar would do
        // this lazily; doing it now makes the install inspectable).
        if dest_rel.to_ascii_lowercase().ends_with(".png") {
            let sidecar_key = format!(
                "{}.toml",
                pack_path.rsplit_once('.').map_or(pack_path, |(s, _)| s)
            );
            if !pack.files.contains_key(&sidecar_key) {
                if let Some(meta) = super::png_meta::read_embedded_bytes(bytes) {
                    let _ = super::write_atomic(&dest.with_extension("toml"), meta);
                }
            }
        }
        Ok(())
    };

    for (unit, paths) in &units {
        // Does any file in this unit collide with different existing art?
        let collision = paths.iter().any(|path| {
            differs(&pool_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)), &pack.files[path])
        });
        match unit {
            Unit::Convention => {
                for path in paths {
                    let dest = pool_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                    let bytes = &pack.files[path];
                    if dest.exists() {
                        if differs(&dest, bytes) {
                            report.kept_existing.push(path.clone());
                            report.warnings.push(format!(
                                "'{path}': different art already in the pool — kept yours \
                                 (convention art is matched by name, so it can't be renamed)"
                            ));
                        } else {
                            report.identical.push(path.clone());
                        }
                    } else {
                        write_entry(&mut report, path, path, bytes)?;
                    }
                }
            }
            Unit::Flat { category, stem } if collision => {
                let fresh = fresh_flat_stem(&pool_root, category, stem);
                for path in paths {
                    let file = path.rsplit('/').next().unwrap_or(path);
                    let renamed_file = replace_stem(file, stem, &fresh);
                    let dest_rel = format!("{category}/{renamed_file}");
                    write_entry(&mut report, path, &dest_rel, &pack.files[path])?;
                }
                rewrite_flat_assignment(&mut report.assignments, category, stem, &fresh);
            }
            Unit::Set { category, set } if collision => {
                let fresh = fresh_set_name(&pool_root, category, set);
                for path in paths {
                    let dest_rel = path.replacen(
                        &format!("{category}/{set}/"),
                        &format!("{category}/{fresh}/"),
                        1,
                    );
                    write_entry(&mut report, path, &dest_rel, &pack.files[path])?;
                }
                rewrite_set_assignment(&mut report.assignments, category, set, &fresh);
            }
            _ => {
                for path in paths {
                    let dest = pool_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                    if dest.exists() {
                        report.identical.push(path.clone());
                    } else {
                        write_entry(&mut report, path, path, &pack.files[path])?;
                    }
                }
            }
        }
    }

    super::pool::invalidate_cache();
    Ok(report)
}

/// Persist install-time assignments into the appearance store: the
/// character file plus the characterless base copy (the same pair the GUI
/// funnel writes, so headless installs and the web doll stay in step).
pub fn apply_assignments(assignments: &Assignments, character: Option<&str>) -> anyhow::Result<()> {
    let mut appearance = AppearanceSettings::load_or_migrate(character);
    assignments.apply_to(&mut appearance);
    appearance.save(character)?;
    if character.is_some() {
        if let Err(err) = appearance.save(None) {
            tracing::warn!("base appearance.toml not updated: {err:#}");
        }
    }
    Ok(())
}

/// First `stem`, `stem-2`, `stem-3`, … with no existing file of any image
/// extension (or sidecar) in the category folder.
fn fresh_flat_stem(pool_root: &Path, category: &str, stem: &str) -> String {
    let dir = pool_root.join(category);
    let taken = |candidate: &str| {
        IMAGE_EXTS
            .iter()
            .map(|ext| format!("{candidate}.{ext}"))
            .chain([format!("{candidate}.toml")])
            .any(|file| dir.join(file).exists())
    };
    fresh_name(stem, taken)
}

/// First `set`, `set-2`, … with no existing folder in the category.
fn fresh_set_name(pool_root: &Path, category: &str, set: &str) -> String {
    let dir = pool_root.join(category);
    fresh_name(set, |candidate| dir.join(candidate).exists())
}

fn fresh_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{base}-fallback")
}

fn replace_stem(file: &str, stem: &str, fresh: &str) -> String {
    match file.rsplit_once('.') {
        Some((s, ext)) if s == stem => format!("{fresh}.{ext}"),
        _ => file.to_string(),
    }
}

fn rewrite_flat_assignment(a: &mut Assignments, category: &str, stem: &str, fresh: &str) {
    match category {
        "frames" => {
            if a.default_frame.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(stem)) {
                a.default_frame = Some(fresh.to_string());
            }
            for value in a.control_frames.values_mut() {
                if value.eq_ignore_ascii_case(stem) {
                    *value = fresh.to_string();
                }
            }
        }
        _ => {
            // Path-valued slots (doll, background): rewrite the exact path.
            for slot in [&mut a.doll_image, &mut a.default_background] {
                if let Some(path) = slot {
                    let mut parts = path.rsplitn(2, '/');
                    let file = parts.next().unwrap_or_default().to_string();
                    let dir = parts.next().unwrap_or_default();
                    if dir == category {
                        let renamed = replace_stem(&file, stem, fresh);
                        if renamed != file {
                            *path = format!("{category}/{renamed}");
                        }
                    }
                }
            }
        }
    }
}

fn rewrite_set_assignment(a: &mut Assignments, category: &str, set: &str, fresh: &str) {
    let slot = match category {
        "compass" => &mut a.compass_set,
        "edges" => &mut a.edge_set,
        "statusicons" => &mut a.status_icon_set,
        _ => return,
    };
    if slot.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(set)) {
        *slot = Some(fresh.to_string());
    }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Persist an installed pack's manifest (assignments already rewritten for
/// any collision renames) as an inert preset: `global/skins/<name>/skin.toml`
/// with no art beside it — the art lives in the pool. `.setskin <name>`
/// re-applies the assignments any time; `.skins` lists it alongside legacy
/// skins (same dir convention, distinguished by the `format` key).
pub fn write_preset(name: &str, manifest: &SkinPackManifest) -> anyhow::Result<()> {
    let name = sanitize_pack_name(name)
        .ok_or_else(|| anyhow::anyhow!("invalid preset name '{name}'"))?;
    let dir = Config::skins_dir()?.join(&name);
    std::fs::create_dir_all(&dir)?;
    let text = toml::to_string_pretty(manifest)
        .map_err(|e| anyhow::anyhow!("cannot serialize preset: {e}"))?;
    super::write_atomic(&dir.join("skin.toml"), text)
        .map_err(|e| anyhow::anyhow!("cannot write preset '{name}': {e}"))?;
    Ok(())
}

/// Load `global/skins/<name>/skin.toml` as a preset. `None` when the skin
/// doesn't exist or is a LEGACY live-manifest skin (no `format` key) — the
/// caller falls through to the legacy path.
pub fn load_preset(name: &str) -> Option<SkinPackManifest> {
    let path = Config::skins_dir().ok()?.join(name).join("skin.toml");
    let text = std::fs::read_to_string(path).ok()?;
    if toml::from_str::<toml::Value>(&text)
        .ok()?
        .get("format")
        .is_none()
    {
        return None;
    }
    parse_manifest(&text).ok()
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Build a pack from the current appearance and write it to
/// `<config>/exports/<name>-skin.zip`. Collects every pool file the
/// assignments reference (set folders whole, frames by stem, paths
/// exactly), bakes each PNG's sidecar into its bytes as embedded
/// metadata, includes the sidecar files too, validates, and zips.
/// Legacy `<set>_<role>.png` prefix art is re-homed to the foldered form
/// so packs are always foldered.
pub fn export(
    appearance: &AppearanceSettings,
    name: &str,
) -> anyhow::Result<(PathBuf, Findings)> {
    let name = sanitize_pack_name(name)
        .ok_or_else(|| anyhow::anyhow!("invalid pack name '{name}' (letters, digits, - _ only)"))?;
    let assignments = Assignments::from_appearance(appearance);
    let pool_root = Config::global_images_dir()?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    fn add_file(
        files: &mut BTreeMap<String, Vec<u8>>,
        rel_dest: String,
        abs: &Path,
    ) -> anyhow::Result<()> {
        let bytes = std::fs::read(abs)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", abs.display()))?;
        let sidecar = abs.with_extension("toml");
        let sidecar_text = std::fs::read_to_string(&sidecar).ok();
        let bytes = match (&sidecar_text, rel_dest.to_ascii_lowercase().ends_with(".png")) {
            (Some(meta), true) => {
                super::png_meta::write_embedded_bytes(&bytes, meta).unwrap_or(bytes)
            }
            _ => bytes,
        };
        if let Some(meta) = sidecar_text {
            let stem = rel_dest.rsplit_once('.').map_or(rel_dest.as_str(), |(s, _)| s);
            files.insert(format!("{stem}.toml"), meta.into_bytes());
        }
        files.insert(rel_dest, bytes);
        Ok(())
    }

    // Exact-path slots.
    for slot in [&appearance.doll_image, &appearance.default_background] {
        if let Some(path) = slot {
            if !is_none_sentinel(path) {
                add_file(
                    &mut files,
                    path.clone(),
                    &pool_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)),
                )?;
            }
        }
    }
    // Frames by stem.
    let mut frame_stems: Vec<String> = appearance
        .default_frame
        .iter()
        .chain(appearance.control_frames.values())
        .map(|stem| stem.to_ascii_lowercase())
        .filter(|stem| !is_none_sentinel(stem))
        .collect();
    frame_stems.sort();
    frame_stems.dedup();
    for image in super::pool::list_category("frames") {
        if frame_stems.contains(&image.stem().to_ascii_lowercase()) {
            add_file(&mut files, image.pool_path.clone(), &image.abs_path)?;
        }
    }
    for stem in &frame_stems {
        let found = files
            .keys()
            .any(|key| key.to_ascii_lowercase().starts_with(&format!("frames/{stem}.")));
        if !found {
            anyhow::bail!("assigned frame '{stem}' is not in the pool — cannot export");
        }
    }
    // Set folders (foldered on export even when the pool art is legacy
    // prefix form).
    for (category, set) in [
        ("compass", &appearance.compass_set),
        ("edges", &appearance.edge_set),
        ("statusicons", &appearance.status_icons.set),
    ] {
        let Some(set) = set else { continue };
        if is_none_sentinel(set) {
            continue;
        }
        let mut any = false;
        for image in super::pool::list_category(category) {
            let Some((image_set, role)) = image.set_role() else {
                continue;
            };
            if !image_set.eq_ignore_ascii_case(set) {
                continue;
            }
            let ext = image
                .file_name
                .rsplit_once('.')
                .map_or("png", |(_, ext)| ext);
            add_file(
                &mut files,
                format!("{category}/{set}/{role}.{ext}"),
                &image.abs_path,
            )?;
            any = true;
        }
        if !any {
            anyhow::bail!("assigned {category} set '{set}' has no art in the pool — cannot export");
        }
    }

    let manifest = SkinPackManifest {
        format: FORMAT_VERSION,
        meta: PackMeta {
            name: name.clone(),
            description: Some("Exported with .exportskin".to_string()),
            author: None,
        },
        assignments,
    };
    let pack = SkinPack { manifest, files };
    let findings = validate(&pack);
    if !findings.ok() {
        anyhow::bail!(
            "export failed validation:\n  {}",
            findings.errors.join("\n  ")
        );
    }

    let dest_dir = Config::config_dir()?.join("exports");
    std::fs::create_dir_all(&dest_dir)?;
    let dest = dest_dir.join(format!("{name}-skin.zip"));
    write_pack_zip(&pack, &dest)?;
    Ok((dest, findings))
}

/// Serialize a pack to a zip file (deflate, atomic write).
pub fn write_pack_zip(pack: &SkinPack, dest: &Path) -> anyhow::Result<()> {
    use std::io::Write;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let manifest = toml::to_string_pretty(&pack.manifest)
            .map_err(|e| anyhow::anyhow!("cannot serialize skin.toml: {e}"))?;
        writer.start_file("skin.toml", options)?;
        writer.write_all(manifest.as_bytes())?;
        for (path, bytes) in &pack.files {
            writer.start_file(path.as_str(), options)?;
            writer.write_all(bytes)?;
        }
        writer.finish()?;
    }
    super::write_atomic(dest, buf)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", dest.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy migration
// ---------------------------------------------------------------------------

/// Convert a legacy live-manifest skin (`global/skins/<name>/`) into a
/// pack. Maps what the appearance model can express — doll base +
/// calibration, compass, named frames, default background/border, status
/// icons, edges, control faces — and reports everything it cannot
/// (per-window entries, doll variants/part overlays, sheets, ui palette,
/// creature card/field) as warnings. Art is resolved the way the live
/// skin resolved it (skin dir first, then the pool) and synthesized
/// sidecars carry the manifest's calibration, so the pack installs
/// pre-calibrated.
pub fn migrate_legacy(skin_name: &str) -> anyhow::Result<(SkinPack, Vec<String>)> {
    let (manifest, root) = super::skins::load_manifest(skin_name)
        .map_err(|e| anyhow::anyhow!("cannot load skin '{skin_name}': {e}"))?;
    let set_name = sanitize_pack_name(skin_name)
        .ok_or_else(|| anyhow::anyhow!("skin name '{skin_name}' is not pack-safe"))?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut assignments = Assignments::default();
    let mut warnings: Vec<String> = Vec::new();

    let ext_of = |image: &str| -> String {
        image
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .filter(|ext| IMAGE_EXTS.contains(&ext.as_str()))
            .unwrap_or_else(|| "png".to_string())
    };
    // Resolve an image the way the live skin did, read it, optionally bake
    // sidecar metadata, and add both entries.
    let add = |files: &mut BTreeMap<String, Vec<u8>>,
               dest: String,
               image: &str,
               sidecar: Option<String>|
     -> anyhow::Result<()> {
        let abs = super::skins::resolve_image_path(&root, image);
        let bytes = std::fs::read(&abs)
            .map_err(|e| anyhow::anyhow!("'{image}' ({}): {e}", abs.display()))?;
        let bytes = match &sidecar {
            Some(meta) if dest.to_ascii_lowercase().ends_with(".png") => {
                super::png_meta::write_embedded_bytes(&bytes, meta).unwrap_or(bytes)
            }
            _ => bytes,
        };
        if let Some(meta) = sidecar {
            let stem = dest.rsplit_once('.').map_or(dest.as_str(), |(s, _)| s);
            files.insert(format!("{stem}.toml"), meta.into_bytes());
        }
        files.insert(dest, bytes);
        Ok(())
    };

    // Injury doll base + calibration.
    if let Some(base) = &manifest.injury_doll.base {
        let stem = Path::new(base)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| set_name.clone());
        let dest = format!("dolls/{stem}.{}", ext_of(base));
        let mut doc = toml_edit::DocumentMut::new();
        doc.insert("kind", toml_edit::value(DollSidecar::KIND));
        doc.insert(
            "anchors",
            toml_edit::Item::Table(super::pool::anchors_toml_table(&manifest.injury_doll.anchors)),
        );
        doc.insert(
            "dots",
            toml_edit::Item::Table(super::pool::dots_toml_table(&manifest.injury_doll.dots)),
        );
        add(&mut files, dest.clone(), base, Some(doc.to_string()))?;
        assignments.doll_image = Some(dest);
    }

    // Compass sprites -> a foldered set named for the skin.
    if let Some(rose) = &manifest.compass.rose {
        add(
            &mut files,
            format!("compass/{set_name}/rose.{}", ext_of(rose)),
            rose,
            None,
        )?;
        for (direction, image) in &manifest.compass.directions {
            add(
                &mut files,
                format!(
                    "compass/{set_name}/{}.{}",
                    direction.to_ascii_lowercase(),
                    ext_of(image)
                ),
                image,
                None,
            )?;
        }
        assignments.compass_set = Some(set_name.clone());
    }

    // Named frames -> pool frames with nine-slice sidecars.
    let frame_sidecar = |spec: &super::skins::BorderSpec| -> String {
        let mut doc = toml_edit::DocumentMut::new();
        doc.insert("kind", toml_edit::value(FrameSidecar::KIND));
        let mut arr = toml_edit::Array::new();
        for inset in spec.slice {
            arr.push(super::pool::toml_rounded(inset, 10.0));
        }
        doc.insert("slice", toml_edit::value(arr));
        doc.insert(
            "scale",
            toml_edit::value(super::pool::toml_rounded(spec.scale, 10_000.0)),
        );
        doc.to_string()
    };
    let mut frame_stem_for_image: HashMap<String, String> = HashMap::new();
    for (frame_name, spec) in &manifest.frames {
        let stem = frame_name.to_ascii_lowercase();
        add(
            &mut files,
            format!("frames/{stem}.{}", ext_of(&spec.image)),
            &spec.image,
            Some(frame_sidecar(spec)),
        )?;
        frame_stem_for_image.insert(spec.image.clone(), stem);
    }

    // The "default" window entry maps to the global defaults; every other
    // per-window entry has no appearance slot.
    for (window_name, window) in &manifest.windows {
        if window_name != "default" {
            warnings.push(format!(
                "[window.{window_name}] is per-window art — not representable in a pack \
                 (assign it via the window's Appearance menu after install)"
            ));
            continue;
        }
        if let Some(background) = &window.background {
            let stem = Path::new(&background.image)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| set_name.clone());
            let dest = format!("backgrounds/{stem}.{}", ext_of(&background.image));
            add(&mut files, dest.clone(), &background.image, None)?;
            assignments.default_background = Some(dest);
        }
        if let Some(border) = &window.border {
            let stem = match frame_stem_for_image.get(&border.image) {
                Some(stem) => stem.clone(),
                None => {
                    let stem = format!("{set_name}-default");
                    add(
                        &mut files,
                        format!("frames/{stem}.{}", ext_of(&border.image)),
                        &border.image,
                        Some(frame_sidecar(border)),
                    )?;
                    stem
                }
            };
            assignments.default_frame = Some(stem);
        }
    }

    // Status icons -> a statusicons set (roles are indicator ids).
    if !manifest.icons.is_empty() {
        for (id, image) in &manifest.icons {
            add(
                &mut files,
                format!(
                    "statusicons/{set_name}/{}.{}",
                    id.to_ascii_lowercase(),
                    ext_of(image)
                ),
                image,
                None,
            )?;
        }
        assignments.status_icon_set = Some(set_name.clone());
    }

    // Edge overlays -> an edges set ({side}.png + {side}-ornament.png).
    if !manifest.edges.is_empty() {
        for (side, spec) in &manifest.edges {
            let side = side.to_ascii_lowercase();
            if let Some(strip) = &spec.strip {
                let mut doc = toml_edit::DocumentMut::new();
                doc.insert("kind", toml_edit::value(EdgeSidecar::KIND));
                doc.insert("tile", toml_edit::value(spec.tile));
                if let Some(anchor) = &spec.anchor {
                    doc.insert("anchor", toml_edit::value(anchor.as_str()));
                }
                if let Some(thickness) = spec.thickness {
                    doc.insert(
                        "thickness",
                        toml_edit::value(super::pool::toml_rounded(thickness, 10.0)),
                    );
                }
                doc.insert(
                    "scale",
                    toml_edit::value(super::pool::toml_rounded(spec.scale, 10_000.0)),
                );
                add(
                    &mut files,
                    format!("edges/{set_name}/{side}.{}", ext_of(strip)),
                    strip,
                    Some(doc.to_string()),
                )?;
            }
            if let Some(ornament) = &spec.ornament {
                add(
                    &mut files,
                    format!("edges/{set_name}/{side}-ornament.{}", ext_of(ornament)),
                    ornament,
                    None,
                )?;
            }
        }
        assignments.edge_set = Some(set_name.clone());
    }

    // Control faces -> frames assigned per control key ("button",
    // "button.hover", ...); the dot is not filename-safe, so the stem
    // swaps it for a dash.
    for (control, spec) in &manifest.controls {
        let stem = format!("{set_name}-{}", control.to_ascii_lowercase().replace('.', "-"));
        add(
            &mut files,
            format!("frames/{stem}.{}", ext_of(&spec.image)),
            &spec.image,
            Some(frame_sidecar(spec)),
        )?;
        assignments
            .control_frames
            .insert(control.to_ascii_lowercase(), stem);
    }

    // Everything with no appearance slot: detect from the raw TOML so the
    // report is complete even for sections the typed manifest defaults.
    let raw_path = root.join("skin.toml");
    if let Ok(raw) = std::fs::read_to_string(&raw_path) {
        if let Ok(value) = toml::from_str::<toml::Value>(&raw) {
            for (key, label) in [
                ("ui", "the [ui] palette"),
                ("sheets", "icon sprite sheets"),
                ("creature_card", "the creature card template"),
                ("creature_field", "creature field camera tuning"),
            ] {
                if value.get(key).is_some() {
                    warnings.push(format!(
                        "{label} has no pack equivalent — that section stays with the legacy skin"
                    ));
                }
            }
            if let Some(doll) = value.get("injury_doll").and_then(|d| d.as_table()) {
                if doll.keys().any(|k| {
                    !matches!(k.as_str(), "base" | "anchors" | "dots")
                }) {
                    warnings.push(
                        "doll part overlays / variants / sets are authored art with no pack \
                         equivalent — only the base + calibration migrated"
                            .to_string(),
                    );
                }
            }
        }
    }

    let pack = SkinPack {
        manifest: SkinPackManifest {
            format: FORMAT_VERSION,
            meta: PackMeta {
                name: set_name,
                description: Some(format!("Migrated from legacy skin '{skin_name}'")),
                author: None,
            },
            assignments,
        },
        files,
    };
    Ok((pack, warnings))
}

/// Pack name from user input: letters, digits, `-`, `_` only (matching
/// the jinx skin-name rule), so it's a safe path component everywhere.
pub fn sanitize_pack_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > 64 {
        return None;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VELLUM_FE_DIR_TEST_LOCK as ENV_LOCK;

    /// Minimal structurally-valid PNG (signature + IHDR + IDAT + IEND with
    /// correct CRCs); png_meta only needs valid chunk framing. `payload`
    /// varies the bytes so collision tests can make "different art".
    fn tiny_png(payload: u8) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &byte in data {
                crc ^= byte as u32;
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        // 1x1, 8-bit grayscale.
        let ihdr = [0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0];
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &[payload]);
        chunk(&mut png, b"IEND", &[]);
        png
    }

    fn png_with_meta(payload: u8, meta: &str) -> Vec<u8> {
        crate::config::png_meta::write_embedded_bytes(&tiny_png(payload), meta).unwrap()
    }

    fn env_guard(dir: &std::path::Path) -> std::sync::MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VELLUM_FE_DIR", dir);
        guard
    }

    fn basic_pack() -> SkinPack {
        let mut files = BTreeMap::new();
        files.insert(
            "dolls/elf.png".to_string(),
            png_with_meta(1, "kind = \"doll\"\n[anchors]\nhead = [0.5, 0.1]\n"),
        );
        files.insert(
            "frames/ornate.png".to_string(),
            png_with_meta(2, "kind = \"frame\"\nslice = 12.0\n"),
        );
        files.insert("compass/parchment/rose.png".to_string(), tiny_png(3));
        files.insert("compass/parchment/ne.png".to_string(), tiny_png(4));
        SkinPack {
            manifest: SkinPackManifest {
                format: FORMAT_VERSION,
                meta: PackMeta {
                    name: "parchment".into(),
                    ..Default::default()
                },
                assignments: Assignments {
                    doll_image: Some("dolls/elf.png".into()),
                    default_frame: Some("ornate".into()),
                    compass_set: Some("parchment".into()),
                    ..Default::default()
                },
            },
            files,
        }
    }

    #[test]
    fn valid_pack_roundtrips_through_zip() {
        let pack = basic_pack();
        assert!(validate(&pack).ok(), "{:?}", validate(&pack).errors);

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("p.zip");
        write_pack_zip(&pack, &dest).unwrap();
        let bytes = std::fs::read(&dest).unwrap();
        assert!(is_pack_format(&bytes));
        let back = read_pack_bytes(&bytes).unwrap();
        assert_eq!(back.manifest, pack.manifest);
        assert_eq!(back.files, pack.files);
    }

    #[test]
    fn legacy_manifest_and_future_format_are_refused() {
        let mut pack = basic_pack();
        let legacy = "[meta]\nname = \"old\"\n";
        assert!(parse_manifest(legacy).unwrap_err().contains("legacy"));
        pack.manifest.format = FORMAT_VERSION + 1;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.zip");
        write_pack_zip(&pack, &dest).unwrap();
        let err = read_pack_bytes(&std::fs::read(&dest).unwrap()).unwrap_err();
        assert!(err.contains("newer"), "{err}");
        // And a legacy zip is not pack format.
        assert!(!is_pack_format(&{
            let mut buf = Vec::new();
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            use std::io::Write;
            w.start_file("skin.toml", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(legacy.as_bytes()).unwrap();
            w.finish().unwrap();
            buf
        }));
    }

    #[test]
    fn validation_catches_broken_assignments_and_metadata() {
        // Missing assigned files.
        let mut pack = basic_pack();
        pack.files.remove("dolls/elf.png");
        pack.files.remove("frames/ornate.png");
        pack.files
            .retain(|key, _| !key.starts_with("compass/"));
        let findings = validate(&pack);
        assert_eq!(findings.errors.len(), 3, "{:?}", findings.errors);

        // Compass set without a rose.
        let mut pack = basic_pack();
        pack.files.remove("compass/parchment/rose.png");
        let findings = validate(&pack);
        assert!(findings.errors.iter().any(|e| e.contains("rose")));

        // Embedded metadata with the wrong kind for its category.
        let mut pack = basic_pack();
        pack.files.insert(
            "dolls/bad.png".into(),
            png_with_meta(9, "kind = \"frame\"\nslice = 4.0\n"),
        );
        let findings = validate(&pack);
        assert!(findings.errors.iter().any(|e| e.contains("dolls/bad.png")));

        // Unknown category and orphan sidecar are warnings, not errors.
        let mut pack = basic_pack();
        pack.files.insert("mystery/x.png".into(), tiny_png(7));
        pack.files
            .insert("dolls/orphan.toml".into(), b"kind = \"doll\"\n".to_vec());
        let findings = validate(&pack);
        assert!(findings.ok());
        assert!(findings.warnings.iter().any(|w| w.contains("mystery")));
        assert!(findings.warnings.iter().any(|w| w.contains("orphan")));

        // A background with its typed fit sidecar validates (embedded and
        // as a loose sidecar file); the wrong kind there is an error.
        let mut pack = basic_pack();
        pack.files.insert(
            "backgrounds/mesh.png".into(),
            png_with_meta(9, "kind = \"background\"\nfit = \"tile\"\nscale = 2.0\n"),
        );
        pack.files.insert(
            "backgrounds/paper.png".into(),
            tiny_png(8),
        );
        pack.files.insert(
            "backgrounds/paper.toml".into(),
            b"kind = \"background\"\nfit = \"center\"\n".to_vec(),
        );
        assert!(validate(&pack).ok(), "{:?}", validate(&pack).errors);
        pack.files.insert(
            "backgrounds/bad.png".into(),
            png_with_meta(9, "kind = \"doll\"\n"),
        );
        let findings = validate(&pack);
        assert!(findings
            .errors
            .iter()
            .any(|e| e.contains("backgrounds/bad.png")));

        // The "none" sentinel needs no art.
        let mut pack = basic_pack();
        pack.manifest.assignments.edge_set = Some("none".into());
        assert!(validate(&pack).ok());
    }

    #[test]
    fn unsafe_zip_paths_are_rejected() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("skin.toml", opts).unwrap();
            w.write_all(b"format = 1\n").unwrap();
            w.start_file("../evil.png", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        assert!(read_pack_bytes(&buf).unwrap_err().contains("unsafe"));
    }

    #[test]
    fn install_writes_pool_extracts_sidecars_and_applies() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = env_guard(dir.path());

        let pack = basic_pack();
        let report = install_files(&pack).unwrap();
        assert_eq!(report.installed.len(), 4);
        assert!(report.renamed.is_empty());

        let pool = Config::global_images_dir().unwrap();
        assert!(pool.join("dolls/elf.png").is_file());
        // Sidecar extracted from the embedded chunk.
        let sidecar = std::fs::read_to_string(pool.join("dolls/elf.toml")).unwrap();
        assert!(sidecar.contains("head"));
        assert!(pool.join("compass/parchment/rose.png").is_file());

        apply_assignments(&report.assignments, Some("Ultz")).unwrap();
        let appearance = AppearanceSettings::load_or_migrate(Some("Ultz"));
        assert_eq!(appearance.doll_image.as_deref(), Some("dolls/elf.png"));
        assert_eq!(appearance.default_frame.as_deref(), Some("ornate"));
        assert_eq!(appearance.compass_set.as_deref(), Some("parchment"));
        assert_eq!(appearance.active_skin, None, "presets clear the live skin");
        // Base copy written too (web doll follows).
        assert_eq!(
            AppearanceSettings::load_or_migrate(None).doll_image.as_deref(),
            Some("dolls/elf.png")
        );

        // Re-install: everything identical, nothing rewritten.
        let again = install_files(&pack).unwrap();
        assert!(again.installed.is_empty());
        assert_eq!(again.identical.len(), 4);

        std::env::remove_var("VELLUM_FE_DIR");
    }

    #[test]
    fn colliding_art_renames_units_and_rewrites_assignments() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = env_guard(dir.path());
        let pool = Config::global_images_dir().unwrap();

        // Pre-existing DIFFERENT art at every colliding location.
        std::fs::create_dir_all(pool.join("dolls")).unwrap();
        std::fs::write(pool.join("dolls/elf.png"), tiny_png(200)).unwrap();
        std::fs::create_dir_all(pool.join("compass/parchment")).unwrap();
        std::fs::write(pool.join("compass/parchment/rose.png"), tiny_png(201)).unwrap();
        // And a creature that must never be renamed.
        std::fs::create_dir_all(pool.join("creatures/kobold")).unwrap();
        std::fs::write(pool.join("creatures/kobold/kobold.png"), tiny_png(202)).unwrap();

        let mut pack = basic_pack();
        pack.files
            .insert("creatures/kobold/kobold.png".into(), tiny_png(5));
        let report = install_files(&pack).unwrap();

        // Flat doll renamed, assignment follows.
        assert!(pool.join("dolls/elf-2.png").is_file());
        assert_eq!(
            report.assignments.doll_image.as_deref(),
            Some("dolls/elf-2.png")
        );
        // Whole compass set renamed, assignment follows.
        assert!(pool.join("compass/parchment-2/rose.png").is_file());
        assert!(pool.join("compass/parchment-2/ne.png").is_file());
        assert_eq!(
            report.assignments.compass_set.as_deref(),
            Some("parchment-2")
        );
        // Convention art kept, warned.
        assert_eq!(
            std::fs::read(pool.join("creatures/kobold/kobold.png")).unwrap(),
            tiny_png(202)
        );
        assert_eq!(report.kept_existing, vec!["creatures/kobold/kobold.png"]);
        // Frame had no collision: installed under its own name.
        assert!(pool.join("frames/ornate.png").is_file());
        assert_eq!(report.assignments.default_frame.as_deref(), Some("ornate"));

        std::env::remove_var("VELLUM_FE_DIR");
    }

    #[test]
    fn migrate_legacy_skin_maps_slots_and_reports_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = env_guard(dir.path());

        let root = Config::skins_dir().unwrap().join("oldskin");
        std::fs::create_dir_all(root.join("art")).unwrap();
        for name in [
            "art/doll.png",
            "art/rose.png",
            "art/n.png",
            "art/frame.png",
            "art/paper.png",
            "art/strip.png",
            "art/button.png",
        ] {
            std::fs::write(root.join(name), tiny_png(1)).unwrap();
        }
        std::fs::write(
            root.join("skin.toml"),
            r##"
[meta]
name = "Old"

[injury_doll]
base = "art/doll.png"
[injury_doll.anchors]
head = [0.5, 0.1]
[injury_doll.chest]
healthy = "art/doll.png"

[compass]
rose = "art/rose.png"
n = "art/n.png"

[frames.ornate]
image = "art/frame.png"
slice = [8.0, 8.0, 8.0, 8.0]
scale = 0.5

[window.default.background]
image = "art/paper.png"
[window.default.border]
image = "art/frame.png"
slice = [8.0, 8.0, 8.0, 8.0]

[window.thoughts.background]
image = "art/paper.png"

[icons]
kneeling = "art/n.png"

[edges.right]
strip = "art/strip.png"
tile = true

[controls."button.hover"]
image = "art/button.png"
slice = [4.0, 4.0, 4.0, 4.0]

[ui]
accent = "#ff0000"
"##,
        )
        .unwrap();

        let (pack, warnings) = migrate_legacy("oldskin").unwrap();
        let a = &pack.manifest.assignments;
        assert_eq!(a.doll_image.as_deref(), Some("dolls/doll.png"));
        assert_eq!(a.compass_set.as_deref(), Some("oldskin"));
        // The default border reuses the named frame it shares art with.
        assert_eq!(a.default_frame.as_deref(), Some("ornate"));
        assert_eq!(a.default_background.as_deref(), Some("backgrounds/paper.png"));
        assert_eq!(a.status_icon_set.as_deref(), Some("oldskin"));
        assert_eq!(a.edge_set.as_deref(), Some("oldskin"));
        assert_eq!(
            a.control_frames.get("button.hover").map(String::as_str),
            Some("oldskin-button-hover")
        );
        // Sidecars synthesized from the manifest.
        assert!(pack.files.contains_key("dolls/doll.toml"));
        assert!(String::from_utf8_lossy(&pack.files["frames/ornate.toml"]).contains("slice"));
        assert!(String::from_utf8_lossy(&pack.files["edges/oldskin/right.toml"]).contains("tile"));
        assert!(pack.files.contains_key("compass/oldskin/rose.png"));
        assert!(pack.files.contains_key("statusicons/oldskin/kneeling.png"));
        // The migrated pack validates clean.
        let findings = validate(&pack);
        assert!(findings.ok(), "{:?}", findings.errors);
        // Non-mappable content is reported.
        assert!(warnings.iter().any(|w| w.contains("window.thoughts")));
        assert!(warnings.iter().any(|w| w.contains("[ui] palette")));
        assert!(warnings.iter().any(|w| w.contains("part overlays")));

        std::env::remove_var("VELLUM_FE_DIR");
    }

    #[test]
    fn export_builds_installable_pack_with_baked_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = env_guard(dir.path());
        let pool = Config::global_images_dir().unwrap();

        std::fs::create_dir_all(pool.join("dolls")).unwrap();
        std::fs::write(pool.join("dolls/elf.png"), tiny_png(1)).unwrap();
        std::fs::write(
            pool.join("dolls/elf.toml"),
            "kind = \"doll\"\n[anchors]\nhead = [0.5, 0.1]\n",
        )
        .unwrap();
        std::fs::create_dir_all(pool.join("frames")).unwrap();
        std::fs::write(pool.join("frames/ornate.png"), tiny_png(2)).unwrap();
        std::fs::write(pool.join("frames/ornate.toml"), "kind = \"frame\"\nslice = 12.0\n")
            .unwrap();
        // Legacy prefix compass art re-homes to foldered form on export.
        std::fs::create_dir_all(pool.join("compass")).unwrap();
        std::fs::write(pool.join("compass/parchment_rose.png"), tiny_png(3)).unwrap();
        crate::config::pool::invalidate_cache();

        let appearance = AppearanceSettings {
            doll_image: Some("dolls/elf.png".into()),
            default_frame: Some("ornate".into()),
            compass_set: Some("parchment".into()),
            ..Default::default()
        };
        let (path, findings) = export(&appearance, "mylook").unwrap();
        assert!(findings.ok());
        assert!(path.ends_with("exports/mylook-skin.zip") || path.ends_with("exports\\mylook-skin.zip"));

        let pack = read_pack_bytes(&std::fs::read(&path).unwrap()).unwrap();
        assert!(pack.files.contains_key("dolls/elf.png"));
        assert!(pack.files.contains_key("dolls/elf.toml"));
        assert!(pack.files.contains_key("frames/ornate.png"));
        assert!(
            pack.files.contains_key("compass/parchment/rose.png"),
            "legacy prefix art re-homed: {:?}",
            pack.files.keys().collect::<Vec<_>>()
        );
        // The exported doll PNG carries its sidecar embedded.
        let meta =
            crate::config::png_meta::read_embedded_bytes(&pack.files["dolls/elf.png"]).unwrap();
        assert!(meta.contains("head"));

        // Exporting an assignment whose art is missing fails loudly.
        let broken = AppearanceSettings {
            default_frame: Some("ghost".into()),
            ..Default::default()
        };
        assert!(export(&broken, "broken").is_err());

        std::env::remove_var("VELLUM_FE_DIR");
    }
}
