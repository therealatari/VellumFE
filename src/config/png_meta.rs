//! Embedded sidecar metadata in PNG files (skin-system overhaul, phase 3).
//!
//! The travel format for pool-art metadata: the same TOML a sidecar holds,
//! stored in a PNG `tEXt` chunk under the `vellum-meta` keyword. A shared
//! PNG carries its calibration inside the pixels — post the file anywhere
//! and it arrives pre-calibrated. The sidecar file remains the WORKING
//! copy and always wins (art tools strip text chunks on re-save, and
//! calibrator writes stay atomic TOML); the embedded chunk is written on
//! export/save and extracted to a sidecar when a chunk-bearing image is
//! read without one.
//!
//! Implemented as raw chunk splicing (length/type/data/CRC framing per the
//! PNG spec) so no PNG-encoder dependency is needed and the image data is
//! never re-encoded — pixels pass through byte-for-byte.

use std::path::Path;

/// `tEXt` keyword identifying our metadata chunk.
pub const KEYWORD: &str = "vellum-meta";

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Read the embedded metadata TOML from a PNG, if present. None for
/// non-PNG files, PNGs without the chunk, or unreadable files.
pub fn read_embedded(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    read_embedded_bytes(&bytes)
}

/// In-memory form of [`read_embedded`], for callers holding zip entries
/// rather than files (skin-pack validation).
pub fn read_embedded_bytes(bytes: &[u8]) -> Option<String> {
    for (kind, data) in chunks(bytes)? {
        if kind != *b"tEXt" {
            continue;
        }
        // tEXt: keyword, NUL, text (both Latin-1; ours is ASCII TOML).
        let nul = data.iter().position(|&b| b == 0)?;
        if &data[..nul] == KEYWORD.as_bytes() {
            return String::from_utf8(data[nul + 1..].to_vec()).ok();
        }
    }
    None
}

/// Write (or replace) the embedded metadata chunk in a PNG, atomically.
/// Existing `vellum-meta` chunks are removed; the new chunk lands after
/// IHDR so metadata-unaware tools that truncate trailing chunks still
/// keep it. Pixels and all other chunks pass through untouched.
pub fn write_embedded(path: &Path, meta: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)
        .map_err(|err| anyhow::anyhow!("cannot read {}: {}", path.display(), err))?;
    let updated = write_embedded_bytes(&bytes, meta)
        .ok_or_else(|| anyhow::anyhow!("{} is not a valid PNG", path.display()))?;
    crate::config::write_atomic(path, updated)
        .map_err(|err| anyhow::anyhow!("cannot write {}: {}", path.display(), err))?;
    Ok(())
}

/// In-memory form of [`write_embedded`], for callers building archives
/// (skin-pack export bakes each PNG's sidecar in before zipping). None
/// when the bytes are not a valid PNG.
pub fn write_embedded_bytes(bytes: &[u8], meta: &str) -> Option<Vec<u8>> {
    let parsed = chunks(bytes)?;
    let mut out = Vec::with_capacity(bytes.len() + meta.len() + 64);
    out.extend_from_slice(&PNG_SIGNATURE);
    let mut inserted = false;
    for (kind, data) in parsed {
        // Drop any existing vellum-meta chunk (replaced, never duplicated).
        if kind == *b"tEXt" {
            if let Some(nul) = data.iter().position(|&b| b == 0) {
                if &data[..nul] == KEYWORD.as_bytes() {
                    continue;
                }
            }
        }
        push_chunk(&mut out, &kind, data);
        if !inserted && kind == *b"IHDR" {
            let mut text = Vec::with_capacity(KEYWORD.len() + 1 + meta.len());
            text.extend_from_slice(KEYWORD.as_bytes());
            text.push(0);
            text.extend_from_slice(meta.as_bytes());
            push_chunk(&mut out, b"tEXt", &text);
            inserted = true;
        }
    }
    inserted.then_some(out)
}

/// Parse a PNG into (chunk type, chunk data) slices. None when the
/// signature or chunk framing is broken.
fn chunks(bytes: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    if bytes.len() < 8 || bytes[..8] != PNG_SIGNATURE {
        return None;
    }
    let mut out = Vec::new();
    let mut at = 8usize;
    while at + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind: [u8; 4] = bytes[at + 4..at + 8].try_into().ok()?;
        let data_end = at + 8 + len;
        if data_end + 4 > bytes.len() {
            return None;
        }
        out.push((kind, &bytes[at + 8..data_end]));
        at = data_end + 4; // skip CRC
        if kind == *b"IEND" {
            break;
        }
    }
    Some(out)
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// CRC-32 (ISO-HDLC polynomial), as the PNG spec requires. Small bitwise
/// implementation — this runs once per saved chunk, not per pixel.
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Crc32(0xffff_ffff)
    }
    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= byte as u32;
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }
    fn finish(self) -> u32 {
        !self.0
    }
}

// Tests build/verify real PNGs via the image crate, which the gui feature
// gates; the module itself is dependency-free and compiles everywhere.
#[cfg(all(test, feature = "gui"))]
mod tests {
    use super::*;

    /// Minimal valid PNG (1x1) built from raw chunks, checked against the
    /// image crate's decoder so the splicer is proven not to corrupt files.
    fn tiny_png() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.png");
        image::save_buffer(&path, &[0xff; 4], 1, 1, image::ExtendedColorType::Rgba8).unwrap();
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn crc32_matches_the_known_check_value() {
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xcbf4_3926);
    }

    #[test]
    fn write_then_read_roundtrips_and_image_still_decodes() {
        let png = tiny_png();
        assert_eq!(read_embedded_bytes(&png), None);

        let meta = "kind = \"creature\"\n[anchors]\nfeet = [0.5, 0.9]\n";
        let with_meta = write_embedded_bytes(&png, meta).unwrap();
        assert_eq!(read_embedded_bytes(&with_meta).as_deref(), Some(meta));
        // The spliced file is still a decodable PNG.
        assert!(image::load_from_memory(&with_meta).is_ok());

        // Rewriting replaces the chunk instead of stacking a second one.
        let rewritten = write_embedded_bytes(&with_meta, "kind = \"doll\"\n").unwrap();
        assert_eq!(
            read_embedded_bytes(&rewritten).as_deref(),
            Some("kind = \"doll\"\n")
        );
        let metas = chunks(&rewritten)
            .unwrap()
            .into_iter()
            .filter(|(kind, data)| {
                kind == b"tEXt" && data.starts_with(KEYWORD.as_bytes())
            })
            .count();
        assert_eq!(metas, 1);
        assert!(image::load_from_memory(&rewritten).is_ok());
    }

    #[test]
    fn non_png_bytes_are_rejected_not_mangled() {
        assert_eq!(read_embedded_bytes(b"GIF89a...."), None);
        assert!(write_embedded_bytes(b"GIF89a....", "x = 1").is_none());
    }
}
