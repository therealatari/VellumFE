//! Creature name → art token normalization, ported from the Lich
//! creature-spawns fork (`lib/gemstone/creature.rb`) so VellumFE's art
//! folders and Lich's creature files agree on every token.
//!
//! The canonical registry is the creature-spawns file set (mirrored in
//! the bundled bestiary): tokens are display names, article-free,
//! lowercase, spaces/hyphens as underscores — "big ugly kobold" →
//! `big_ugly_kobold`. Boon-decorated live names ("a shimmering mongrel
//! kobold") normalize to the same token via a leading-adjective strip.
//! The slug is LOSSY (`shield-maiden` and `shield maiden` collide), which
//! is fine one-directionally: names become tokens; tokens are never
//! parsed back into names.

/// Boon adjectives the game prefixes onto creature names, verbatim from
/// Lich's `BOON_ADJECTIVES` — including the multi-word "sickly green" and
/// the game's own "tattoed" spelling. Sorted longest-first at the call
/// site so "sickly green" wins over any single-word prefix.
pub const BOON_ADJECTIVES: &[&str] = &[
    "adroit",
    "afflicted",
    "apt",
    "barbed",
    "belligerent",
    "blurry",
    "canny",
    "combative",
    "dazzling",
    "deft",
    "diseased",
    "drab",
    "dreary",
    "ethereal",
    "flashy",
    "flexile",
    "flickering",
    "flinty",
    "frenzied",
    "ghastly",
    "ghostly",
    "gleaming",
    "glittering",
    "glorious",
    "glowing",
    "grotesque",
    "hardy",
    "illustrious",
    "indistinct",
    "keen",
    "lanky",
    "luminous",
    "lustrous",
    "muculent",
    "nebulous",
    "oozing",
    "pestilent",
    "radiant",
    "raging",
    "ready",
    "resolute",
    "robust",
    "rune-covered",
    "shadowy",
    "shifting",
    "shimmering",
    "shining",
    "sickly green",
    "sinuous",
    "slimy",
    "sparkling",
    "spindly",
    "spiny",
    "stalwart",
    "steadfast",
    "stout",
    "tattoed",
    "tenebrous",
    "tough",
    "twinkling",
    "unflinching",
    "unyielding",
    "wavering",
    "wispy",
];

/// The canonical (registry) form of a live creature name: article
/// dropped, lowercased, ONE leading boon adjective stripped (Lich's
/// `fix_template_name` semantics — a single `sub`, not a loop).
pub fn canonical_name(name: &str) -> String {
    let mut name = name.trim().to_ascii_lowercase();
    for article in ["a ", "an ", "some ", "the "] {
        if let Some(rest) = name.strip_prefix(article) {
            name = rest.to_string();
            break;
        }
    }
    // Longest match first so "sickly green" beats a hypothetical "sickly".
    // Sorted once — this runs per creature per frame in the render path.
    static SORTED: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    let adjectives = SORTED.get_or_init(|| {
        let mut adjectives: Vec<&str> = BOON_ADJECTIVES.to_vec();
        adjectives.sort_by_key(|adj| std::cmp::Reverse(adj.len()));
        adjectives
    });
    for adjective in adjectives {
        if let Some(rest) = name.strip_prefix(adjective) {
            if let Some(rest) = rest.strip_prefix(' ') {
                return rest.to_string();
            }
        }
    }
    name
}

/// The art-folder/file token for a canonical name: spaces and hyphens
/// become underscores, apostrophes (and any other punctuation) drop —
/// matching how the creature-spawns files are named on disk.
pub fn slug(canonical: &str) -> String {
    let mut out = String::with_capacity(canonical.len());
    for ch in canonical.chars() {
        match ch {
            ' ' | '-' => out.push('_'),
            ch if ch.is_ascii_alphanumeric() => out.push(ch.to_ascii_lowercase()),
            '_' => out.push('_'),
            _ => {}
        }
    }
    // Collapse runs from dropped punctuation ("bre'naere" style names).
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

/// Name straight to token: canonicalize then slug. The primary art key
/// for a live creature.
pub fn name_token(name: &str) -> String {
    slug(&canonical_name(name))
}

/// Like [`slug`] but hyphens survive as hyphens ("shield-maiden" →
/// `shield-maiden`). Art folders in the wild are named both ways, so the
/// resolver probes this form alongside the canonical underscore slug.
pub fn slug_keeping_hyphens(canonical: &str) -> String {
    let mut out = String::with_capacity(canonical.len());
    for ch in canonical.chars() {
        match ch {
            ' ' => out.push('_'),
            '-' | '_' => out.push(ch),
            ch if ch.is_ascii_alphanumeric() => out.push(ch.to_ascii_lowercase()),
            _ => {}
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

/// All accepted art tokens for a live creature name. The article-dropped
/// full name comes first: a creature whose real name starts with a boon
/// word ("a shining winged disir") must find its own art before the boon
/// strip guesses `winged_disir`. The boon-stripped form follows so
/// boon-decorated spawns still fold onto their base art. Each form in
/// canonical (underscore) and hyphen-preserving spelling, deduped.
pub fn name_token_variants(name: &str) -> Vec<String> {
    let mut name = name.trim().to_ascii_lowercase();
    for article in ["a ", "an ", "some ", "the "] {
        if let Some(rest) = name.strip_prefix(article) {
            name = rest.to_string();
            break;
        }
    }
    let stripped = canonical_name(&name);
    let mut out: Vec<String> = Vec::new();
    for form in [&name, &stripped] {
        for token in [slug(form), slug_keeping_hyphens(form)] {
            if !token.is_empty() && !out.contains(&token) {
                out.push(token);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_strips_article_case_and_one_boon_adjective() {
        assert_eq!(canonical_name("a big ugly kobold"), "big ugly kobold");
        assert_eq!(canonical_name("A Shimmering Mongrel Kobold"), "mongrel kobold");
        assert_eq!(canonical_name("an ethereal triton dissembler"), "triton dissembler");
        // Multi-word adjective wins as a unit.
        assert_eq!(canonical_name("a sickly green forest troll"), "forest troll");
        // One strip only, Lich-style: a name that legitimately starts
        // with a second adjective keeps it.
        assert_eq!(canonical_name("ghostly ghostly pooka"), "ghostly pooka");
        // Adjective must be a whole word ("readied" is not "ready").
        assert_eq!(canonical_name("readied ambusher"), "readied ambusher");
        // No article, no boon: unchanged.
        assert_eq!(canonical_name("rolton"), "rolton");
    }

    #[test]
    fn slug_matches_creature_spawns_file_naming() {
        assert_eq!(name_token("a big ugly kobold"), "big_ugly_kobold");
        assert_eq!(
            name_token("a battle-worn empyrean captain"),
            "battle_worn_empyrean_captain"
        );
        // Apostrophes drop without leaving separators.
        assert_eq!(slug("k'tafali zealot"), "ktafali_zealot");
        // Boon-decorated live name lands on the canonical token.
        assert_eq!(name_token("a shimmering mongrel kobold"), "mongrel_kobold");
    }
}
