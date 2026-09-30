//! Unused-glyph and unused-alias reachability checks.

use super::*;

/// An alternative of an alternative is reached through the one it varies,
/// exactly as the build registers it (`foo:a:b` under `foo:a` as well as
/// `foo`): a ref that reaches `foo:a` alone may pick it.
#[test]
fn an_alternative_of_a_used_alternative_is_not_unused() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph foo:a
ref pix
glyph foo:a:b
ref pix
glyph user
ref foo:a
map U = user
",
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("'foo:a:b' is unused")),
        "`foo:a:b` is an alternative of the used `foo:a`, got: {issues:?}",
    );
}

#[test]
fn unused_glyph_reported() {
    let input = "\
glyph used 2 1
..@@
map A = used

glyph orphan 2 1
@@..
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Warning
                && i.message.contains("glyph 'orphan' is unused")),
        "expected unused glyph warning, got: {issues:?}",
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'used' is unused")),
        "mapped glyph should not be reported as unused",
    );
}

#[test]
fn transitively_used_glyph_not_reported() {
    let input = "\
glyph base 2 1
..@@

glyph composite 2 1
@@..
ref base 0 0

map A = composite
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        "transitively used glyph should not be unused: {issues:?}",
    );
}

#[test]
fn mutually_referencing_cluster_reported() {
    let input = "\
glyph a 2 1
..@@
ref b 0 0

glyph b 2 1
@@..
ref a 0 0
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("glyph 'a' is unused")),
        "mutual ref cluster should be unused: {issues:?}",
    );
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("glyph 'b' is unused")),
        "mutual ref cluster should be unused: {issues:?}",
    );
}

#[test]
fn remap_target_counts_as_used() {
    let input = "\
glyph base 2 1
..@@
map A = base

glyph alt 2 1
@@..

remap liga : base -> alt
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'alt' is unused")),
        "remap target should count as used: {issues:?}",
    );
}

#[test]
fn alternative_glyph_used_when_base_used() {
    let input = "\
glyph stem 2 1
..@@
map A = stem

glyph stem:wide 2 1
@@..
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'stem:wide' is unused")),
        "alternative of used base should not be unused: {issues:?}",
    );
}

/// `keep` says the glyph is wanted whether or not anything reaches it, so
/// the unused warning — which exists to find glyphs nothing reaches — must
/// stay quiet for one, whether it has a body or not.
#[test]
fn kept_glyph_not_reported_unused() {
    for input in [
        "glyph held keep advance 0\n",
        "glyph held 2 1 keep\n@@..\n",
        "glyph held keep\nanchor +join 0 0\n",
    ] {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("is unused")),
            "a kept glyph is never unused, for {input:?}: {issues:?}",
        );
    }
}

/// `.notdef` is kept without saying `keep`: it is the glyph a renderer
/// draws for an uncovered character, so nothing in the source names it and
/// the unused warning would fire on every font that draws one.
#[test]
fn notdef_not_reported_unused_without_keep() {
    let input = "glyph .notdef 2 1\n@@..\nglyph a 2 1\n..@@\nmap A = a\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        ".notdef is kept automatically: {issues:?}",
    );
}

#[test]
fn ref_to_bodiless_kept_glyph_not_reported_unbuilt() {
    // A dimension-less `glyph NAME keep` is a placeholder that *is*
    // built (an empty anchor-carrying entry, see `glyph_cache::seed_cache`)
    // and is exempt from the "has no content" warning above; the
    // expansion's "is not built" error must exempt it the same way.
    let input = "\
glyph held keep
anchor +join 0 0

glyph user 2 1
@@..
ref held
map A = user
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("not built")),
        "keep placeholder is built; a ref to it is fine: {issues:?}",
    );
}

/// A `$-N` back-reference on a `ref`, an IDC component or an alias target is a
/// use of what the group names, exactly as writing the group out again is. The
/// reachability walk reads the source rather than the expansion, so it has to
/// substitute the item's own captures itself or every glyph named that way
/// reads as unused.
#[test]
fn back_referenced_ref_target_is_a_use() {
    let input = "\
glyph part-a 2 1
..@@

glyph part-b 2 1
@@..

glyph outer-(a|b) 2 1
ref part-($-1) 0 0

map A|B = outer-(a|b)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        "a `$-N` ref target uses the glyphs its group names: {issues:?}",
    );
}

#[test]
fn back_referenced_alias_target_is_a_use() {
    let input = "\
glyph part-a 2 1
..@@

glyph part-b 2 1
@@..

glyph outer-(a|b) = part-($-1)

map A|B = outer-(a|b)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        "a `$-N` alias target uses the glyphs its group names: {issues:?}",
    );
}
