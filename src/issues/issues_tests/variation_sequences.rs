//! `map BASE SELECTOR` variation-sequence checks.

use super::*;

/// Every `map BASE SELECTOR` check, driven through the same entry point the
/// build and the editor use. The shared prelude gives the pair something
/// valid to point at so that only the rule under test can fail.
fn uvs_issues(body: &str) -> Vec<Issue> {
    let input = format!("glyph zero 2 2\n@@@@\n@@@@\nglyph zero-emoji 2 2\n@@@@\n@@@@\n{body}");
    let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
}

fn has_error(issues: &[Issue], needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.severity == Severity::Error && i.message.contains(needle))
}

#[test]
fn a_valid_variation_sequence_is_clean() {
    let issues = uvs_issues("map U+0030 = zero\nmap U+0030 U+FE0F = zero-emoji\n");
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "expected no errors, got: {issues:?}",
    );
}

/// The two halves are not interchangeable, and a swapped line would build a
/// sequence no shaper could ever match — so both directions are errors.
#[test]
fn each_half_of_a_variation_sequence_must_be_the_right_kind() {
    let issues = uvs_issues("map U+0030 = zero\nmap U+0030 U+0031 = zero-emoji\n");
    assert!(
        has_error(&issues, "is not a variation selector"),
        "expected a selector-half error, got: {issues:?}",
    );

    let issues = uvs_issues("map U+FE0F U+FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "is a variation selector"),
        "expected a base-half error, got: {issues:?}",
    );
}

#[test]
fn only_one_half_of_a_variation_sequence_may_vary() {
    let issues = uvs_issues("map U+0030..0039 U+FE0E..FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "only one half"),
        "expected a both-vary error, got: {issues:?}",
    );
}

/// The fallback GSUB rule needs the base's own glyph as its first element,
/// so a pair whose base is unmapped would silently never fire.
#[test]
fn the_base_of_a_variation_sequence_must_be_mapped() {
    let issues = uvs_issues("map U+0030 U+FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "U+0030"),
        "expected an unmapped-base error, got: {issues:?}",
    );

    // ...and a base mapped only in *another* slice does not count, because
    // the pair and the base have to meet in the same face.
    let issues = uvs_issues(
        "slice wide\nslice narrow\nmap wide : U+0030 = zero\n\
             map narrow : U+0030 U+FE0F = zero-emoji\n",
    );
    assert!(
        has_error(&issues, "U+0030"),
        "expected a per-slice unmapped-base error, got: {issues:?}",
    );
}

/// Pasting `0️⃣` gives three characters, which cmap format 14 cannot hold.
/// The message has to say where the rest of the sequence goes, or the only
/// signal is a mapping that quietly never happens.
#[test]
fn a_pasted_longer_sequence_says_how_to_split_it() {
    let issues = uvs_issues("map 0\u{FE0F}\u{20E3} = zero-emoji\n");
    assert!(
        has_error(&issues, "remap"),
        "expected a split-it error naming remap, got: {issues:?}",
    );
}

#[test]
fn map_generate_rejects_a_variation_sequence() {
    let issues = uvs_issues("map U+0030 = zero\nmap generate U+0030 U+FE0F\n");
    assert!(
        has_error(&issues, "single character"),
        "expected a generate-sequence error, got: {issues:?}",
    );
}

/// A variation selector reaches the font only through a sequence. Mapping
/// one on its own would hand a source the glyph the fallback lookup owns,
/// and the two would then disagree about what that glyph is for.
#[test]
fn mapping_a_variation_selector_on_its_own_is_rejected() {
    let issues = uvs_issues("map U+FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "variation selector"),
        "expected a lone-selector error, got: {issues:?}",
    );
}

/// cmap format 14 is keyed by codepoint; the fallback lookup is keyed by
/// glyph. Where two characters share a base glyph the two halves of one
/// declaration stop agreeing, and the source has to be told.
#[test]
fn two_pairs_colliding_on_one_base_glyph_are_an_error() {
    let issues = uvs_issues(
        "glyph other 2 2\n@@@@\n@@@@\n\
             map U+0030 = zero\nmap U+0031 = zero\n\
             map U+0030 U+FE0F = zero-emoji\nmap U+0031 U+FE0F = other\n",
    );
    assert!(
        has_error(&issues, "keyed by glyph"),
        "expected a collision error, got: {issues:?}",
    );
}

#[test]
fn a_pair_on_a_shared_base_glyph_warns_about_over_firing() {
    let issues =
        uvs_issues("map U+0030 = zero\nmap U+0031 = zero\nmap U+0030 U+FE0F = zero-emoji\n");
    assert!(
        issues.iter().any(|i| i.severity == Severity::Warning
            && i.message.contains("U+0031")
            && i.message.contains("fallback lookup")),
        "expected an over-firing warning, got: {issues:?}",
    );
}

/// A base glyph only one character reaches is the ordinary case and has to
/// stay quiet, or the warning above would fire on every well-formed pair.
#[test]
fn a_pair_on_an_unshared_base_glyph_is_quiet() {
    let issues = uvs_issues("map U+0030 = zero\nmap U+0030 U+FE0F = zero-emoji\n");
    assert!(
        issues.is_empty(),
        "expected no issues at all, got: {issues:?}",
    );
}

/// The build names its synthesized selector glyphs `@vs-XXXX`, and that is
/// safe without a reserved-name rule because a source cannot produce the
/// name: `@` expands against the enclosing base into something else, and
/// with no base to expand against the name is invalid outright. This pins
/// the argument, since the safety of the whole scheme rests on it.
#[test]
fn a_source_cannot_write_the_synthesized_selector_name() {
    // With a preceding glyph, `@` expands and the name becomes another one.
    let doc = document_io::parse_document_from_str(
        "glyph base 2 2\n@@@@\n@@@@\nglyph @vs-FE0F 2 2\n@@@@\n@@@@\n",
        "test.unf".into(),
    )
    .unwrap();
    assert!(
        doc.items.iter().all(|item| !matches!(
            item,
            DocumentItem::Glyph { name: GlyphName(n), .. } if n == "@vs-FE0F"
        )),
        "the `@` should have expanded away, got {:?}",
        doc.items,
    );

    // With nothing to expand against — so, only as the very first glyph of
    // a project — it stays literal, and then it is not a valid name.
    let doc =
        document_io::parse_document_from_str("glyph @vs-FE0F 2 2\n@@@@\n@@@@\n", "test.unf".into())
            .unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error),
        "expected an invalid-name error, got: {issues:?}",
    );
}
