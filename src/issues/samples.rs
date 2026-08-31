//! `sample` lines and the `||` continuations they take: is there a text, is
//! the mode one that exists, and is each name on the list reachable.
//!
//! See [`crate::samples`] for the model and why a label may carry a text of
//! its own.

use std::collections::HashSet;

use crate::document::DocumentItem;
use crate::samples::SAMPLE_MODES;

use super::{Cx, Issue, Severity, issue_at};

pub(super) fn check_samples(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    // Names are unique across the whole source, not per file: the demo page
    // and the preview offer one list, and which file a text was written in is
    // not something a reader of that list can see.
    let mut seen: HashSet<(&str, Option<&str>)> = HashSet::new();
    for doc in cx.docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Sample {
                label,
                sublabel,
                mode,
                text,
                ..
            } = item
            else {
                continue;
            };
            let named = match sublabel {
                Some(sublabel) => format!("`{label}` `{sublabel}`"),
                None => format!("`{label}`"),
            };
            if label.is_empty() || sublabel.as_deref() == Some("") {
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    "a sample label is what a reader picks the text by, and cannot be empty"
                        .to_string(),
                ));
            } else if !seen.insert((label.as_str(), sublabel.as_deref())) {
                // The second one is unreachable: the list shows one entry per
                // name, and `SampleSet::collect` keeps the first.
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    match sublabel {
                        Some(_) => format!(
                            "sample {named} is declared more than once; only the first is offered"
                        ),
                        None => format!(
                            "sample {named} already carries a text of its own; \
                             give this one a sublabel"
                        ),
                    },
                ));
            }
            if text.is_empty() {
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    format!("sample {named} has no text: it needs at least one `||` line under it"),
                ));
            }
            for mode in mode {
                if !SAMPLE_MODES.contains(&mode.as_str()) {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "unknown sample mode `{mode}`: the modes are {}",
                            SAMPLE_MODES
                                .iter()
                                .map(|m| format!("`{m}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
            }
        }
    }
}
