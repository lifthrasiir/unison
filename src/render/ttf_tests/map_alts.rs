//! `map CHAR = FIRST SECOND ...` — ordered alternatives.
//!
//! The choice is made once, in
//! [`expand::resolve_map_alternatives`](super::super::expand), and these tests
//! read it back where it becomes observable: which glyph ends up carrying the
//! codepoint.

use super::*;

const HEAD: &str = "\
meta height 4
meta ascent 4
meta descent 0
";

/// Which glyph name claims `cp`, as the collected font data has it.
fn glyph_for(src: &str, cp: u32) -> Option<String> {
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let (_, _, glyph_data, _, _) = collect_glyph_data(&docs, false).expect("expected glyph data");
    glyph_data
        .iter()
        .find(|g| g.codepoints.contains(&cp))
        .map(|g| g.name.clone())
}

fn diagnostics(src: &str) -> Vec<String> {
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    crate::render::ttf_builder::expand_documents(&docs, &name_parts)
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn first_alternative_wins_when_it_exists() {
    let src = format!("{HEAD}\nglyph first 1 1\n@\n\nglyph second 1 1\n@\n\nmap A = first second\n");
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("first"));
}

#[test]
fn a_missing_first_alternative_falls_through() {
    let src = format!("{HEAD}\nglyph second 1 1\n@\n\nmap A = first second\n");
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("second"));
    assert!(diagnostics(&src).is_empty(), "{:?}", diagnostics(&src));
}

/// The point of the whole feature: one line over a range, whose characters do
/// not all come from the same family.
#[test]
fn the_choice_is_per_codepoint() {
    let src = format!(
        "{HEAD}\nglyph a-0041 1 1\n@\n\nglyph b-0042 1 1\n@\n\n\
         map U+($#0041..0042) = a-($-1) b-($-1)\n"
    );
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("a-0041"));
    assert_eq!(glyph_for(&src, 0x42).as_deref(), Some("b-0042"));
}

/// A glyph that is declared but has nothing to draw is never built, so it is
/// not a target either — the same rule the single-target form is held to.
#[test]
fn a_contentless_alternative_is_passed_over() {
    let src = format!("{HEAD}\nglyph first\n\nglyph second 1 1\n@\n\nmap A = first second\n");
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("second"));
}

/// An on-demand name stands for a glyph the font generates, so it counts as
/// present without being declared anywhere.
#[test]
fn an_on_demand_alternative_counts_as_present() {
    let src = format!("{HEAD}\nglyph fallback 1 1\n@\n\nmap A = 2x2 fallback\n");
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("2x2"));
}

/// An alias is a second name for a glyph that does exist.
#[test]
fn an_alias_alternative_counts_as_present() {
    let src = format!("{HEAD}\nglyph real 1 1\n@\n\nglyph nick = real\n\nmap A = nick other\n");
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("real"));
}

#[test]
fn nothing_matching_falls_back_to_notdef() {
    let src = format!("{HEAD}\nglyph .notdef 1 1\n@\n\nmap A = first second\n");
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some(".notdef"));
    let d = diagnostics(&src);
    assert!(
        d.iter().any(|m| m.contains("has no target")
            && m.contains("'first'")
            && m.contains("'second'")),
        "{d:?}",
    );
}

/// With no `.notdef` to fall back on either, the character is simply left
/// unmapped — which a renderer shows as glyph id 0, the same empty box.
#[test]
fn nothing_matching_and_no_notdef_leaves_the_character_unmapped() {
    let src = format!("{HEAD}\nglyph other 1 1\n@\n\nmap B = other\nmap A = first second\n");
    assert_eq!(glyph_for(&src, 0x41), None);
    assert!(diagnostics(&src).iter().any(|m| m.contains("has no target")));
}

/// One finding per line, not one per character: a range fails the same way all
/// the way along, and the specimen is what answers per character.
#[test]
fn an_unmatched_range_is_reported_once() {
    let src = format!(
        "{HEAD}\nglyph other 1 1\n@\n\nmap B = other\nmap U+($#0041..0043) = a-($-1) b-($-1)\n"
    );
    let d: Vec<String> = diagnostics(&src)
        .into_iter()
        .filter(|m| m.contains("has no target"))
        .collect();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains("and 2 more characters"), "{d:?}");
}

/// A variation sequence lists alternatives the same way, and picks per pair.
#[test]
fn a_variation_sequence_picks_its_alternative_too() {
    let src = format!(
        "{HEAD}\nglyph base 1 1\n@\n\nglyph plain 1 1\n@\n\n\
         map A = base\nmap A U+FE0F = fancy plain\n"
    );
    let doc = document_io::parse_document_from_str(&src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let (_, _, glyph_data, _, _) = collect_glyph_data(&docs, false).expect("expected glyph data");
    // The selector's target is collected like any other glyph but claims no
    // codepoint, so it is only there at all if the pair resolved.
    assert!(glyph_data.iter().any(|g| g.name == "plain"), "no 'plain'");
    assert!(!glyph_data.iter().any(|g| g.name == "fancy"));
}

/// `map generate` declares its glyph later than this pass runs, but the name is
/// readable off the line, so an alternative may name it.
#[test]
fn a_generated_glyph_counts_as_present() {
    let src = format!(
        "{HEAD}\nglyph a-lower 1 1\n@\n\nglyph acute 1 1\n@\n\n\
         map U+0061 = a-lower\nmap U+0301 = acute\n\
         map generate U+00E1 = a-acute\nmap U+00C1 = a-acute fallback\n"
    );
    assert_eq!(glyph_for(&src, 0xC1).as_deref(), Some("a-acute"));
}

/// The empty target (`` `` ``): a character none of the other alternatives
/// covers is dropped instead of faulted. Not even `.notdef` steps in — the line
/// said the character is not in the font, not that it is missing.
#[test]
fn an_empty_last_target_drops_the_mapping_silently() {
    let src = format!(
        "{HEAD}\nglyph .notdef 1 1\n@\n\nglyph other 1 1\n@\n\n\
         map B = other\nmap A = first ``\n"
    );
    assert_eq!(glyph_for(&src, 0x41), None);
    assert!(diagnostics(&src).is_empty(), "{:?}", diagnostics(&src));
}

/// It applies per character like every other alternative: the ones that matched
/// are still mapped.
#[test]
fn an_empty_last_target_only_drops_what_matched_nothing() {
    let src = format!(
        "{HEAD}\nglyph a-0041 1 1\n@\n\nmap U+($#0041..0042) = a-($-1) ``\n"
    );
    assert_eq!(glyph_for(&src, 0x41).as_deref(), Some("a-0041"));
    assert_eq!(glyph_for(&src, 0x42), None);
    assert!(diagnostics(&src).is_empty(), "{:?}", diagnostics(&src));
}

/// A line with nothing but the empty target maps nothing, and says nothing.
#[test]
fn a_lone_empty_target_maps_nothing() {
    let src = format!("{HEAD}\nglyph other 1 1\n@\n\nmap B = other\nmap A = ``\n");
    assert_eq!(glyph_for(&src, 0x41), None);
    assert!(diagnostics(&src).is_empty(), "{:?}", diagnostics(&src));
}

/// Anything written after the empty target can never be reached, so writing one
/// there is a mistake worth naming.
#[test]
fn an_empty_target_that_is_not_last_is_an_error() {
    let src = format!("{HEAD}\nglyph second 1 1\n@\n\nmap A = `` second\n");
    let doc = document_io::parse_document_from_str(&src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let resolution = crate::resolve::Resolution::compute(&docs);
    let issues = crate::issues::collect_issues_with(&docs, &resolution);
    assert!(
        issues.iter().any(|i| i.severity == crate::issues::Severity::Error
            && i.message.contains("has to be the last one")),
        "{:?}",
        issues.iter().map(|i| &i.message).collect::<Vec<_>>(),
    );
}
