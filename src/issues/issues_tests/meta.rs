//! `meta` and `audit` directive checks.

use super::*;

#[test]
fn meta_ascent_plus_descent_must_equal_height() {
    let input = "meta height 16\nmeta ascent 12\nmeta descent 3\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("!= height")),
        "expected meta metric mismatch warning, got: {issues:?}",
    );
}

#[test]
fn meta_zero_height_reported() {
    let input = "meta height 0\nmeta ascent 0\nmeta descent 0\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("meta height is 0")),
        "expected zero-height error, got: {issues:?}",
    );
}

/// An unknown key is the whole reason `meta` exists as a checked directive:
/// the value it carries is invisible in the built font, so a typo that is
/// merely ignored is a typo that ships.
#[test]
fn meta_unknown_key_is_error() {
    let input = "meta famliy 16\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("famliy")),
        "expected unknown-key error, got: {issues:?}",
    );
}

/// Every kind of conflict is an error, and two `meta` lines setting the
/// same key are a conflict even when they agree.
#[test]
fn meta_duplicate_key_is_error() {
    let input = "meta height 16\nmeta height 16\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("height")
            && i.message.contains("more than once")),
        "expected duplicate-key error, got: {issues:?}",
    );
}

#[test]
fn meta_wrong_arity_is_error() {
    for input in ["meta height\n", "meta height 16 12\n"] {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error),
            "expected an arity error for {input:?}, got: {issues:?}",
        );
    }
}

#[test]
fn meta_non_numeric_metric_is_error() {
    let input = "meta height sixteen\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error),
        "expected a non-numeric error, got: {issues:?}",
    );
}

/// An `audit` rule is single-assignment like a `meta` key, and unreadable
/// lines are errors rather than rules that quietly stop checking.
#[test]
fn audit_lines_are_checked_and_assigned_once() {
    let errors = |input: &str| -> Vec<String> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.message)
            .collect()
    };
    assert!(errors("audit ideal-clearance han-* 0 1\n").is_empty());
    assert!(
        errors("audit clarence han-* 0 1\n")
            .iter()
            .any(|m| m.contains("unknown `audit` key")),
    );
    assert!(
        errors("audit ideal-clearance han-* 0\n")
            .iter()
            .any(|m| m.contains("takes a glyph-name prefix")),
    );
    assert!(
        errors("audit ideal-clearance han-* 0 1\naudit ideal-clearance han-* 0 2\n")
            .iter()
            .any(|m| m.contains("is set more than once")),
    );
    // A different prefix is a different rule, not a second answer.
    assert!(
        errors("audit ideal-clearance han-* 0 1\naudit ideal-clearance hang-* 0 2\n").is_empty(),
    );
}

/// A component that is a composite draws no pixels of its own, but it does
/// draw: it is flattened before it is measured, so a radical written as a `ref`
/// to a shared drawing is checked like any other part. Before it was, a line
/// through one was silently not measured at all — no warning, right or wrong.
#[test]
fn a_component_that_is_a_composite_is_measured() {
    let source = |right: &str| {
        format!(
            "\
audit ideal-clearance test-* 0 1

glyph l:4x4 4 4
@@@@....
@@@@....
@@@@....
@@@@....

glyph r:4x4 4 4
..@@@@@@
..@@@@@@
..@@@@@@
..@@@@@@

glyph through-ref:4x4 4 4
ref r:4x4

glyph test-x 8 4
⿰ l:4x4 1 {right}:4x4
"
        )
    };
    let clearances = |input: &str| -> Vec<String> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.message.contains("ideal"))
            .map(|i| i.message)
            .collect()
    };
    // The two parts leave two cells between them and none at the right edge.
    let drawn = clearances(&source("r"));
    assert!(!drawn.is_empty(), "the drawn part is measured");
    // The name is the only difference; the ink behind it is the same ink.
    let through_ref = clearances(&source("through-ref"));
    assert_eq!(
        through_ref
            .iter()
            .map(|m| m.replace("through-ref", "r"))
            .collect::<Vec<_>>(),
        drawn,
    );
}

/// `font-meta` became `meta`. A leftover line must not fall through to the
/// generic "unrecognized directive" report: it names the migration, so the
/// author is not left rereading a line that is spelled correctly for the
/// format it was written against.
#[test]
fn legacy_font_meta_is_error() {
    let input = "font-meta height 16 ascent 12 descent 4\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("font-meta")
            && i.message.contains("meta")),
        "expected a migration error, got: {issues:?}",
    );
}
