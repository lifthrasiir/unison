//! Checks of assorted directives: headings, `assert`, `map decomposed`, `assume`, `prop`, unreadable lines.

use super::*;

/// A line the grammar cannot read is an error, and the pixel row that
/// misses its glyph's width is why. It parses as a directive like any
/// other unreadable line, so the row is dropped and the glyph builds from
/// whatever rows did fit — a blank or half-drawn glyph that a `map` then
/// maps a character to, with nothing but a warning to say so.
#[test]
fn a_pixel_row_that_does_not_fit_is_an_error() {
    let input = "\
glyph wide 4 2
@@@@
@@@@
map A = wide
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("unrecognized directive")
            && i.message.contains("@@@@")),
        "expected the short row to be an error, got: {issues:?}",
    );
}

#[test]
fn a_fourth_heading_level_is_an_error_and_the_three_are_not() {
    let input = "# one\n## two\n### three\n#### four\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    let heading: Vec<&Issue> = issues
        .iter()
        .filter(|i| i.message.contains("heading level"))
        .collect();
    assert_eq!(heading.len(), 1, "only `####` is reported: {issues:?}");
    assert_eq!(heading[0].severity, Severity::Error);
    assert!(heading[0].message.contains("level 4"), "{:?}", heading[0]);
}

#[test]
fn assert_same_distinct_not_unrecognized() {
    let input = "\
glyph a 2 1
..@@
glyph b 2 1
@@..
map A = a
map B = b

assert same a b
assert distinct a b
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("unrecognized directive")),
        "assert same/distinct should not be flagged as unrecognized: {issues:?}",
    );
}

#[test]
fn map_decomposed_without_decomposition_reported() {
    // 'A' is already in NFD, so `map A` cannot synthesize anything.
    let input = "\
glyph a 2 1
..@@
map U+0041 = a
map generate A
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error
                && i.message.contains("no canonical decomposition")),
        "expected no-decomposition error, got: {issues:?}",
    );
}

#[test]
fn map_decomposed_with_unmapped_component_reported() {
    // 'Ä' decomposes to U+0041 U+0308; U+0308 is not mapped.
    let input = "\
glyph a 2 1
..@@
map A = a
map generate Ä
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("unmapped codepoint")
            && i.message.contains("U+0308")),
        "expected unmapped component error, got: {issues:?}",
    );
}

#[test]
fn map_decomposed_fully_mapped_accepted() {
    let input = "\
glyph a 2 1
..@@
glyph dieresis 2 1
@@..
map A = a
map U+0308 = dieresis
map generate Ä
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(
            |i| i.message.contains("decomposition") || i.message.contains("unmapped codepoint")
        ),
        "fully mapped decomposition should be accepted, got: {issues:?}",
    );
}

#[test]
fn assume_unused_suppresses_warning() {
    let input = "\
glyph orphan 2 1
@@..

glyph other 2 1
..@@

assume unused orphan
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'orphan' is unused")),
        "assume unused should suppress warning: {issues:?}",
    );
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("glyph 'other' is unused")),
        "non-assumed glyph should still be reported: {issues:?}",
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("unrecognized directive")),
        "assume unused should not be flagged as unrecognized: {issues:?}",
    );
}

/// A property value the UCD does not use is an error: the line exists to be
/// read, and a value nothing can be checked against is worse than silence.
#[test]
fn prop_property_values_are_checked_against_the_ucd_short_names() {
    let src = "prop U+E000 = `X` gc Xx eaw WW\nprop U+E001 gc So eaw W\n";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("`gc Xx` is not a General_Category")),
        "{msgs:?}",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("`eaw WW` is not an East_Asian_Width")),
        "{msgs:?}",
    );
    // The well-formed line draws no complaint of its own.
    assert!(!msgs.iter().any(|m| m.contains("U+E001")), "{msgs:?}");
}

/// A character spelling that covers nothing — a backwards range — would
/// otherwise be a line that quietly never applies to anything.
#[test]
fn a_prop_line_that_names_no_character_is_an_error() {
    let src = "prop U+E00F..E000 gc So\n";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("names no character")),
        "{issues:?}",
    );
}
