//! Unresolved refs, duplicate glyphs, undefined map targets, contentless glyphs.

use super::*;

#[test]
fn unresolved_ref_reported() {
    let input = "glyph foo\nref nonexistent 0 0\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("unresolved ref")),
        "expected unresolved ref error, got: {issues:?}",
    );
}

#[test]
fn duplicate_glyph_reported() {
    let input = "glyph foo 2 1\n..@@\nglyph foo 2 1\n@@..\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.message.contains("duplicate glyph")),
        "expected duplicate glyph warning, got: {issues:?}",
    );
}

#[test]
fn undefined_map_target_reported() {
    let input = "map A = nonexistent\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("nonexistent")),
        "expected undefined map target error, got: {issues:?}",
    );
}
#[test]
fn valid_document_has_no_issues() {
    let input = "\
glyph foo 2 1
..@@
map A = foo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(issues.is_empty(), "expected no issues, got: {issues:?}",);
}

// `testdata/` declares a single consistent `meta` because it has to
// stay a coherent project, so the broken variants are covered here.

#[test]
fn a_map_to_a_contentless_glyph_is_an_error() {
    // Neither a pixel grid nor a ref means the glyph never enters the
    // resolution cache, so it silently vanishes from the cmap. `advance`
    // does not make it buildable, but it does suppress the "has no
    // content" warning, so this used to pass without a single word.
    let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
map B = blank
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("'blank'")
            && i.message.contains("not built")),
        "mapping a contentless glyph must be an error, got: {issues:?}",
    );
}

#[test]
fn a_ref_and_a_remap_to_a_contentless_glyph_are_errors() {
    let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
glyph host
ref blank
map A = vis
remap liga : vis -> blank
feature liga for DFLT : liga
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error && i.message.contains("'blank'"))
        .collect();
    assert!(
        errors.len() >= 2,
        "both the ref and the remap must be reported, got: {issues:?}",
    );
}

/// A glyph that is contentless but never used stays a warning — it builds
/// nothing, but it also breaks nothing.
#[test]
fn an_unused_contentless_glyph_is_not_an_error() {
    let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
assume unused blank
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "an unused contentless glyph must not be an error, got: {issues:?}",
    );
}
