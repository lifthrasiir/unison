//! The `vectoronly` flag's component-sharing warnings.

use super::*;

fn vectoronly_warnings(input: &str) -> Vec<String> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.message.contains("vector artwork in the bitmap face"))
        .map(|i| i.message)
        .collect()
}

/// `vectoronly` reaches down the `ref` graph, so a component it reaches is
/// drawn as vector artwork for everyone. Where the component is the flagged
/// glyph's own, that is exactly what was asked for and nothing is said.
#[test]
fn a_vectoronly_glyph_with_its_own_parts_warns_about_nothing() {
    assert!(
        vectoronly_warnings(
            "\
glyph part 1 1
@@
glyph flag 1 1 vectoronly
ref part
map A = flag
",
        )
        .is_empty(),
    );
}

/// Shared with an unflagged glyph, the reach is a surprise: the other glyph
/// gets vector artwork in the bitmap face without asking, so it is reported
/// on the component both of them name.
#[test]
fn a_component_shared_with_an_unflagged_glyph_is_reported() {
    let msgs = vectoronly_warnings(
        "\
glyph part 1 1
@@
glyph flag 1 1 vectoronly
ref part
glyph plain 1 1
ref part
map A = flag
map B = plain
",
    );
    assert_eq!(msgs.len(), 1, "expected one warning, got {msgs:?}");
    assert!(msgs[0].contains("'part'"), "{}", msgs[0]);
    assert!(msgs[0].contains("'plain'"), "{}", msgs[0]);
}

/// A component that is a character in its own right is reached the same way,
/// and the character's own drawing changes with it — so it is reported even
/// with no second glyph referring to it.
#[test]
fn a_mapped_component_inside_the_reach_is_reported() {
    let msgs = vectoronly_warnings(
        "\
glyph part 1 1
@@
glyph flag 1 1 vectoronly
ref part
map A = flag
map B = part
",
    );
    assert_eq!(msgs.len(), 1, "expected one warning, got {msgs:?}");
    assert!(msgs[0].contains("mapped to a character"), "{}", msgs[0]);
}

/// One half of a color/mono pair asking for the exemption must not drag the
/// other half's components in with it: the mono fallback of a flagged colour
/// drawing is ordinary pixel work, shared with ordinary glyphs, and reporting
/// it was the symptom of the merged glyph carrying the flag for both halves.
#[test]
fn a_vectoronly_colour_half_says_nothing_about_the_mono_halfs_components() {
    let msgs = vectoronly_warnings(
        "\
glyph mono-part 1 1
@@
glyph colour-part 1 1
@@
glyph x:mono 1 1
ref mono-part
glyph x:color 1 1 vectoronly
ref colour-part fill #ff0000
map A = x
map B = mono-part
",
    );
    assert!(msgs.is_empty(), "expected no warning, got {msgs:?}");
}

/// The colour half's own components are still reached, and still reported
/// where they are shared — the scope narrows the walk, it does not stop it.
#[test]
fn a_vectoronly_colour_half_still_reports_its_own_shared_component() {
    let msgs = vectoronly_warnings(
        "\
glyph colour-part 1 1
@@
glyph x:mono 1 1
ref colour-part
glyph x:color 1 1 vectoronly
ref colour-part fill #ff0000
map A = x
map B = colour-part
",
    );
    assert_eq!(msgs.len(), 1, "expected one warning, got {msgs:?}");
    assert!(msgs[0].contains("'colour-part'"), "{}", msgs[0]);
}

/// Nothing is said about a source with no `vectoronly` in it at all — the
/// walk has to bail before it builds anything.
#[test]
fn a_source_without_the_flag_is_never_walked() {
    assert!(
        vectoronly_warnings(
            "\
glyph part 1 1
@@
glyph plain 1 1
ref part
map A = plain
",
        )
        .is_empty(),
    );
}
