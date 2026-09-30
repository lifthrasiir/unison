//! Glyph aliases (`glyph NAME = TARGET`); see `crate::alias`.

use super::*;

#[test]
fn an_alias_to_an_undefined_glyph_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = nope
map A = pix
map B = a
",
    );
    assert!(
        has(&issues, Severity::Error, "names undefined glyph `nope`"),
        "{issues:?}",
    );
}

/// An alias is a second name for a glyph, so a name that is both is two
/// answers to one question — and the expansion would silently keep the
/// glyph and drop the alias.
#[test]
fn a_name_that_is_both_a_glyph_and_an_alias_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a 1 1
@@
glyph a = pix
map A = a
",
    );
    assert!(
        has(&issues, Severity::Error, "both a glyph and an alias"),
        "{issues:?}",
    );
}

#[test]
fn an_alias_cycle_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = b
glyph b = a
map A = pix
",
    );
    assert!(has(&issues, Severity::Error, "is in a cycle"), "{issues:?}");
}

#[test]
fn a_duplicate_alias_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph other 1 1
..
glyph a = pix
glyph a = other
map A = a
",
    );
    assert!(
        has(&issues, Severity::Error, "declared more than once"),
        "{issues:?}",
    );
}

/// An alias nothing names is dead source, reported like an unused glyph —
/// but named as what it is, since the fix is to delete a line rather than
/// to find a home for a drawing.
#[test]
fn an_unused_alias_is_a_warning() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = pix
map A = pix
",
    );
    assert!(
        has(&issues, Severity::Warning, "glyph alias 'a' is unused"),
        "{issues:?}",
    );
}

/// The alias is a node of the reachability walk: naming it must keep both
/// it and its target alive.
#[test]
fn a_used_alias_keeps_its_target_used() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = pix
map A = a
",
    );
    assert!(
        !issues.iter().any(|i| i.message.contains("unused")),
        "neither the alias nor its target is unused, got: {issues:?}",
    );
}

/// An alias standing in for one half of a color/mono pair is used by the
/// name that pair synthesizes, exactly as a written-out `x:color` would be:
/// alternatives of a root name are roots, and an alias is one of them.
#[test]
fn an_alias_used_as_a_color_mono_half_is_not_unused() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph y:mono
ref pix
glyph y:color
ref pix fill #ff0000
glyph x:mono
ref pix
glyph x:color = y:color
map X = x
map Y = y
",
    );
    assert!(
        !issues.iter().any(|i| i.message.contains("unused")),
        "the aliased color half is used by the synthesized `x`, got: {issues:?}",
    );
}

/// A component named through a `glyph A = B` alias: the name the check sees has
/// been canonicalized to the drawing's own (`-r`), but the slot the author
/// picked is the one the *written* name states (`-l`). Ranking the canonical
/// name warns that every aliased component sits in the wrong slot, which is the
/// one thing the alias was written to say is fine.
#[test]
fn an_aliased_component_is_ranked_on_the_name_as_written() {
    let source = |left: &str| {
        format!(
            "\
glyph r:4x4-r 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r:4x4-l = r:4x4-r

glyph b:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 8 4
\u{2FF0} {left} b:4x4
"
        )
    };
    let slots = |input: &str| -> Vec<String> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.message.contains("sits in the"))
            .map(|i| i.message)
            .collect()
    };
    // The alias names the left slot, which is the slot it sits in.
    assert!(
        slots(&source("r:4x4-l")).is_empty(),
        "{:?}",
        slots(&source("r:4x4-l"))
    );
    // The drawing's own name still says `-r`, and that one does warn.
    assert_eq!(slots(&source("r:4x4-r")).len(), 1);
}
