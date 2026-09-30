//! Remap rule shapes, groups and shadowing.

use super::*;

#[test]
fn many_to_many_remap_is_an_error() {
    // Neither a ligature nor a multiple substitution can express this, and
    // guessing one of them silently loses half the rule.
    let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
glyph d 1 1
@@
map A = a
map B = b
remap liga : a b -> c d
feature liga for DFLT : liga
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("no OpenType lookup type")),
            "a 2-to-2 remap must be an error, got: {issues:?}",
        );
}

#[test]
fn many_to_nothing_remap_is_an_error() {
    let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
map A = a
map B = b
remap liga : a b ->
feature liga for DFLT : liga
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("no OpenType lookup type")),
            "deleting a multi-glyph sequence must be an error, got: {issues:?}",
        );
}

#[test]
fn expressible_remap_shapes_are_quiet() {
    // one-to-one, one-to-many, one-to-nothing and many-to-one all have a
    // lookup type, so none of them may be reported.
    let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
map A = a
map B = b
map C = c
remap g1 : a -> b
remap g2 : a -> b c
remap g3 : a ->
remap g4 : a b -> c
feature liga for DFLT : g1
feature liga for DFLT : g2
feature liga for DFLT : g3
feature liga for DFLT : g4
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("no OpenType lookup type")),
        "expressible remaps must be quiet, got: {issues:?}",
    );
}

#[test]
fn remap_pattern_operand_expansions_are_checked() {
    // Remap operands keep their patterns until the GSUB builder expands
    // them, and that builder drops rules whose glyphs have no id without
    // a word. Validation therefore has to expand them the same way.
    let input = "\
name-parts $ab = a b

glyph ok 2 1
@@..
map A = ok

remap liga : ok -> missing-($ab)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.message.contains("missing-a")),
        "expected the expanded remap target to be reported, got: {issues:?}",
    );
    assert!(
        issues.iter().any(|i| i.message.contains("missing-b")),
        "every expansion should be reported, got: {issues:?}",
    );
}

#[test]
fn remap_pattern_operand_that_resolves_is_quiet() {
    let input = "\
name-parts $ab = a b

glyph ok-a 2 1
@@..
glyph ok-b 2 1
.@@.
glyph present-a 2 1
@@..
glyph present-b 2 1
..@@
map A = ok-a
map B = ok-b

remap liga : ok-($ab) -> present-($ab)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("remap")),
        "a remap whose expansions all exist must be quiet, got: {issues:?}",
    );
}

fn group_issues(text: &str) -> Vec<Issue> {
    let doc = document_io::parse_document_from_str(text, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.message.contains("remap group"))
        .collect()
}

#[test]
fn remap_group_ordering_cycle_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap y : a -> a\n\
             remap group x after y\nremap group y after x\n",
    );
    assert_eq!(issues.len(), 2, "one per declaration, got: {issues:?}");
    assert!(
        issues
            .iter()
            .all(|i| i.severity == Severity::Error && i.message.contains("ordering cycle")),
        "got: {issues:?}",
    );
}

#[test]
fn remap_group_after_undefined_group_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap group x after nope\n",
    );
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("undefined group 'nope'")),
        "got: {issues:?}",
    );
}

#[test]
fn remap_group_declared_twice_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap group x\nremap group x\n",
    );
    assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("declared more than once")),
            "got: {issues:?}",
        );
}

#[test]
fn remap_group_without_rules_reported() {
    let issues = group_issues("remap group lonely\n");
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("has no rules")),
        "got: {issues:?}",
    );
}

/// A `feature` may be written above every rule of the group it attaches;
/// the check used to depend on scan order and would call that undefined.
#[test]
fn feature_may_precede_the_rules_of_its_group() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nmap A = a\nmap B = b\n\
             feature ccmp for DFLT : late\nremap late : a -> b\n",
    );
    assert!(issues.is_empty(), "got: {issues:?}");
}

#[test]
fn reversed_group_with_a_non_single_rule_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nglyph c = pix\n\
             map A = a\nmap B = b\nmap C = c\n\
             remap x : a -> b\nremap x : a b -> c\nremap group x reversed\n",
    );
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("reversed")
            && i.message.contains("one glyph")),
        "got: {issues:?}",
    );
}

/// The same rule is perfectly fine in a group that is not reversed.
#[test]
fn a_ligature_is_only_rejected_when_the_group_is_reversed() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nglyph c = pix\n\
             map A = a\nmap B = b\nmap C = c\n\
             remap x : a b -> c\nremap group x\n",
    );
    assert!(issues.is_empty(), "got: {issues:?}");
}

/// Messages of one severity, for the tests that name their own filter.
fn messages_matching(input: &str, needle: &str) -> Vec<String> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.message.contains(needle))
        .map(|i| format!("{:?}: {}", i.severity, i.message))
        .collect()
}

/// A single-substitution lookup covers each glyph once, so a second rule for
/// a glyph the group already substitutes is dropped when the lookup is built.
/// It used to be dropped in silence — and two rules land on one glyph without
/// looking like it whenever a name reaches the same glyph twice: through an
/// alias, or through an implicit merge (`crate::merge`), which is the whole
/// reason a merge cannot break a font quietly.
#[test]
fn a_remap_rule_shadowed_by_an_earlier_one_warns() {
    let msgs = messages_matching(
        "\
glyph a 1 1
@@
glyph x 1 1
..
glyph y 1 1
@@
glyph a-alias = a
map A = a
map X = x
map Y = y
remap sub : a -> x
remap sub : a-alias -> y
feature ccmp for DFLT : sub
",
        "shadow",
    );
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].starts_with("Warning"), "{msgs:?}");
}

/// The same source with the same target is a duplicate, not a lost rule, and
/// a group that is not one single-substitution lookup keeps its rule order in
/// the lookup itself — neither may warn.
#[test]
fn an_identical_or_contextual_rule_is_not_shadowed() {
    let msgs = messages_matching(
        "\
glyph a 1 1
@@
glyph x 1 1
..
glyph y 1 1
@@
glyph a-alias = a
map A = a
map X = x
map Y = y
remap sub : a -> x
remap sub : a-alias -> x
remap ctx : a -> x
remap ctx : y | a-alias -> y
feature ccmp for DFLT : sub
feature ccmp for DFLT : ctx
",
        "shadow",
    );
    assert!(msgs.is_empty(), "{msgs:?}");
}
