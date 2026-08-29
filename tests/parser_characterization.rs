//! Characterization snapshot suite for the XML parser.
//!
//! Pins the parser's exact output over a corpus of real captured wire lines
//! plus known-nasty edge cases, so behavior changes (Saga-parser adoption
//! phases and beyond) show up as reviewable snapshot diffs instead of silent
//! regressions.
//!
//! To regenerate the golden file after an INTENTIONAL behavior change:
//!   UPDATE_PARSER_GOLDEN=1 cargo test --test parser_characterization
//! then review the diff of tests/data/parser_golden.snap in the commit.

use std::fmt::Write as _;
use vellum_fe::parser::XmlParser;

/// Every fixture in the corpus, in a fixed order. Each fixture is parsed by a
/// fresh parser so snapshots stay independent of corpus ordering.
const CORPUS: &[(&str, &str)] = &[
    ("session_start", include_str!("fixtures/session_start.xml")),
    ("room_components", include_str!("fixtures/room_components.xml")),
    ("room_navigation", include_str!("fixtures/room_navigation.xml")),
    ("room_with_compass", include_str!("fixtures/room_with_compass.xml")),
    ("combat_roundtime", include_str!("fixtures/combat_roundtime.xml")),
    ("combat_targets", include_str!("fixtures/combat_targets.xml")),
    ("countdown_casttime", include_str!("fixtures/countdown_casttime.xml")),
    ("vitals_indicators", include_str!("fixtures/vitals_indicators.xml")),
    ("indicators_multi", include_str!("fixtures/indicators_multi.xml")),
    ("injuries", include_str!("fixtures/injuries.xml")),
    ("active_spells", include_str!("fixtures/active_spells.xml")),
    ("active_effects", include_str!("fixtures/active_effects.xml")),
    ("active_cooldowns", include_str!("fixtures/active_cooldowns.xml")),
    ("active_debuffs", include_str!("fixtures/active_debuffs.xml")),
    ("buffs_progress", include_str!("fixtures/buffs_progress.xml")),
    ("progress_variants", include_str!("fixtures/progress_variants.xml")),
    ("progress_mindstate", include_str!("fixtures/progress_mindstate.xml")),
    ("spell_hand", include_str!("fixtures/spell_hand.xml")),
    ("left_hand_link", include_str!("fixtures/left_hand_link.xml")),
    ("playerlist_stream", include_str!("fixtures/playerlist_stream.xml")),
    ("spells_stream", include_str!("fixtures/spells_stream.xml")),
    ("speech_duplicate", include_str!("fixtures/speech_duplicate.xml")),
    ("text_routing", include_str!("fixtures/text_routing.xml")),
    ("unknown_stream", include_str!("fixtures/unknown_stream.xml")),
    ("icon_dialogdata", include_str!("fixtures/icon_dialogdata.xml")),
    ("uberbar_frame", include_str!("fixtures/uberbar_frame.xml")),
    ("parser_edge_cases", include_str!("fixtures/parser_edge_cases.xml")),
    ("objectives", include_str!("fixtures/objectives.xml")),
];

const GOLDEN_PATH: &str = "tests/data/parser_golden.snap";

fn render_snapshot() -> String {
    let mut out = String::new();
    for (name, xml) in CORPUS {
        writeln!(out, "==== fixture: {name} ====").unwrap();
        let mut parser = XmlParser::new();
        for (idx, line) in xml.lines().enumerate() {
            let elements = parser.parse_line(line);
            if elements.is_empty() {
                continue;
            }
            writeln!(out, "-- line {}: {:?}", idx + 1, line).unwrap();
            for element in &elements {
                writeln!(out, "{element:?}").unwrap();
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn parser_output_matches_golden_snapshot() {
    let actual = render_snapshot();
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH);

    if std::env::var("UPDATE_PARSER_GOLDEN").is_ok() {
        std::fs::write(&golden_path, &actual).expect("write golden snapshot");
        eprintln!("golden snapshot updated: {}", golden_path.display());
        return;
    }

    // Normalize CRLF so autocrlf checkouts compare cleanly against our LF output.
    let expected = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden file {GOLDEN_PATH}; run with UPDATE_PARSER_GOLDEN=1 to create it"
            )
        })
        .replace("\r\n", "\n");

    if actual != expected {
        // Point at the first differing line so failures are diagnosable
        // without diffing the whole (large) snapshot in test output.
        let mismatch = actual
            .lines()
            .zip(expected.lines())
            .enumerate()
            .find(|(_, (a, e))| a != e);
        match mismatch {
            Some((n, (a, e))) => panic!(
                "parser output diverged from golden snapshot at line {}:\n  actual:   {a}\n  expected: {e}\n\
                 If this change is intentional, regenerate with UPDATE_PARSER_GOLDEN=1 and review the diff.",
                n + 1
            ),
            None => panic!(
                "parser output length diverged from golden snapshot ({} vs {} lines). \
                 If intentional, regenerate with UPDATE_PARSER_GOLDEN=1 and review the diff.",
                actual.lines().count(),
                expected.lines().count()
            ),
        }
    }
}
