//! Pattern glyph blocks.

use super::*;

/// A pattern glyph declares one glyph per expanded name, whatever the block
/// holds — the expansions share the block's body, its pixel grid included,
/// exactly as they share its `ref` lines. So a block that states only a box is
/// the pattern form of `glyph blank 3 4`, and each expansion is that blank
/// glyph.
#[test]
fn a_pattern_glyph_stating_only_a_box_declares_each_expansion() {
    let input = "\
name-parts $ab = a b

glyph pat-($ab) 3 4
map A|B = pat-($ab)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "a boxed pattern glyph declares `pat-a`/`pat-b` like any other glyph, got: {issues:?}",
    );
}

/// A block that declares *nothing* is still an error — but by the ordinary
/// contentless-glyph rule, reported per expanded name rather than by a rule of
/// its own about patterns.
#[test]
fn an_empty_pattern_glyph_is_an_error() {
    for body in ["", " advance 0"] {
        let input = format!(
            "\
name-parts $ab = a b

glyph pix 1 1
@@
glyph pat-($ab){body}
map A|B = pat-($ab)
"
        );
        let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("'pat-a'")
                && i.message.contains("not built")),
            "an empty pattern glyph must be an error (body {body:?}), got: {issues:?}",
        );
    }
}

/// A `name-parts` value is a pattern, so the declaration itself can be
/// over the expansion limit — before any glyph line refers to it.
#[test]
fn an_oversized_name_parts_binding_is_an_error() {
    let input = format!(
        "name-parts $many = x($1..{})\n",
        crate::pattern::MAX_EXPANSION + 1
    );
    let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("name part `$many`")),
        "an oversized binding must be an error, got: {issues:?}",
    );
}
