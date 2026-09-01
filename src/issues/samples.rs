//! `sample` lines and the `||` continuations they take: is there a text, is
//! the mode one that exists, and is each name on the list reachable.
//!
//! See [`crate::samples`] for the model and why a label may carry a text of
//! its own.

use std::collections::HashSet;

use crate::document::DocumentItem;
use crate::samples::{SAMPLE_MODES, SampleMode};

use super::{Cx, Issue, Severity, issue_at};

pub(super) fn check_samples(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    // Names are unique across the whole source, not per file: the demo page
    // and the preview offer one list, and which file a text was written in is
    // not something a reader of that list can see.
    let mut seen: HashSet<(&str, Option<&str>)> = HashSet::new();
    // A label whose heading writes its own *list* of texts, and every line
    // that puts a text of its own under some label: a label cannot be both,
    // since the generated list is the whole of what the heading offers and a
    // sublabel written beside it would never be shown. Checked after the walk
    // because either line may come first.
    let mut group_modes: HashSet<&str> = HashSet::new();
    let mut sublabelled: Vec<(&crate::document::Document, usize, &str, String)> = Vec::new();
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
            // A generated mode is the one line that carries no text of its
            // own: the build writes it. So the two rules are inverted there —
            // a `||` line under one would be text nothing ever shows.
            let parsed = SampleMode::from_tokens(mode);
            if parsed.is_generated() {
                if !text.is_empty() {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "sample {named} writes its own text, so the `||` lines under it \
                             are never shown"
                        ),
                    ));
                }
                if parsed.is_group() && sublabel.is_none() {
                    group_modes.insert(label.as_str());
                }
                if parsed.is_group() && sublabel.is_some() {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "sample {named} stands for several texts and names them itself; \
                             write it with no sublabel"
                        ),
                    ));
                }
            } else if text.is_empty() {
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    format!("sample {named} has no text: it needs at least one `||` line under it"),
                ));
            }
            if sublabel.is_some() && !parsed.is_group() {
                sublabelled.push((doc, item_idx, label.as_str(), named.clone()));
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

    for (doc, item_idx, label, named) in sublabelled {
        if group_modes.contains(label) {
            issues.push(issue_at(
                doc,
                item_idx,
                Severity::Error,
                format!(
                    "sample {named} sits under a label that writes its own list of texts, \
                     so it is never shown"
                ),
            ));
        }
    }
}
