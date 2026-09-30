//! Validation across slices.

use super::*;

/// Validation used to expand for the *primary* face alone, so a line stated
/// for a slice only some other face includes was dropped before any diagnostic
/// about it could exist — the same line reported an error under the primary
/// slice and nothing at all under the other one. The expansion validation
/// reads is the union of every declared slice for exactly this reason.
#[test]
fn a_non_primary_slice_is_validated_too() {
    let input = "\
face main : sa
meta main : family Main
face other : sb
meta other : family Other
slice sa
slice sb

glyph aa 2 1
@@..

map sa : U+0041 = aa
map sb : U+0042 = nosuchglyph
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("nosuchglyph")),
        "a bad `map` in a non-primary face's slice must still be reported: {issues:?}",
    );
}
