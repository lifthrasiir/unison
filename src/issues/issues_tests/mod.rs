//! Tests for [`crate::issues`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items. One file per check, named after it; the helpers
//! two or more of them share live here.

use super::*;
use crate::document_io;

mod aliases;
mod anchors;
mod basics;
mod colors;
mod directives;
mod flags;
mod meta;
mod patterns;
mod ragged;
mod remap;
mod samples;
mod slices;
mod unused;
mod variation_sequences;

/// A validation nobody wants any more stops instead of finishing: the editor
/// cancels the rebuild an edit supersedes, and the next rebuild waits for this
/// one to return. Uncancelled, it is exactly [`collect_issues_with`].
#[test]
fn a_cancelled_validation_returns_nothing() {
    let doc = document_io::parse_document_from_str(
        "glyph a 1 1\n@@\nglyph b 1 1\n@@\nmap A = a\nmap A = b\nglyph out-(x|y)-(1|2|3)\nref a\n",
        "test.unf".into(),
    )
    .unwrap();
    let docs = [&doc];
    let resolution = Resolution::compute(&docs);
    let messages = |issues: Vec<Issue>| issues.into_iter().map(|i| i.message).collect::<Vec<_>>();

    let cancel = crate::cancel::CancelToken::new();
    let uncancelled = collect_issues_cancellable(&docs, &resolution, &cancel).map(messages);
    assert_eq!(
        uncancelled,
        Some(messages(collect_issues_with(&docs, &resolution)))
    );
    assert!(
        uncancelled.is_some_and(|m| !m.is_empty()),
        "the source has findings, so a cancelled run has something to drop"
    );

    cancel.cancel();
    assert!(collect_issues_cancellable(&docs, &resolution, &cancel).is_none());
}

pub(super) fn issues_for(input: &str) -> Vec<Issue> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
}

pub(super) fn has(issues: &[Issue], severity: Severity, needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.severity == severity && i.message.contains(needle))
}
