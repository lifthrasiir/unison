//! The two single-assignment directives, [`crate::audit`] and
//! [`crate::meta`]: does the line parse, is its slot already taken, and do
//! the effective numbers add up.

use std::collections::{BTreeMap, HashMap};

use crate::document::DocumentItem;
use crate::resolve::{Diagnostic, ItemRef};

use super::{Cx, Issue, Severity};

pub(super) fn check_audit(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let docset = &cx.docset;
    // `audit` validation: does the line parse, and is its slot already taken?
    // Single-assignment like `meta`, and for the same reason — a rule stated
    // twice has no precedence rule to appeal to. There is no scope to consider,
    // since an `audit` line applies to the one glyph set every face draws from.
    let mut declared_audit: BTreeMap<String, ItemRef> = BTreeMap::new();
    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Audit(text) = item else {
                continue;
            };
            let here = ItemRef::new(doc_idx, item_idx);
            let entry = match crate::audit::parse_audit_entry(text) {
                Ok(entry) => entry,
                Err(message) => {
                    issues.push(docset.to_issue(&Diagnostic::error(here, message)));
                    continue;
                }
            };
            if let Some(&first) = declared_audit.get(&entry.slot()) {
                let (path, _, file_line) = docset.location(first);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                issues.push(docset.to_issue(&Diagnostic::error(
                    here,
                    format!(
                        "{} is set more than once (also at {name}:{file_line})",
                        entry.describe_slot(),
                    ),
                )));
            } else {
                declared_audit.insert(entry.slot(), here);
            }
        }
    }
}

pub(super) fn check_meta(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let docset = &cx.docset;
    let faces = cx.faces;
    let resolution = cx.resolution;
    // `meta` validation. Two passes over different things: each line on its own
    // (does it parse, is its key already taken), then the effective numbers.
    // For the latter, the distinction between "not declared" and "declared as
    // the default" is what decides whether to complain at all.
    // Slot -> scope -> where it was declared. A slot set twice in one
    // scope is an outright duplicate; a slot set both bare and for a face
    // gives *that face* two values, which is the same conflict a face
    // including two slices that map one character has. There is no
    // precedence rule in either place, by design.
    let mut declared_meta: BTreeMap<String, BTreeMap<Option<String>, ItemRef>> = BTreeMap::new();
    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Meta(text) = item else {
                continue;
            };
            let here = ItemRef::new(doc_idx, item_idx);
            let (scope, entry) = match crate::meta::parse_meta_entry(text) {
                Ok(parsed) => parsed,
                Err(message) => {
                    issues.push(docset.to_issue(&Diagnostic::error(here, message)));
                    continue;
                }
            };
            if let Some(face) = &scope
                && !faces.faces.iter().any(|f| &f.id == face)
            {
                issues.push(docset.to_issue(&Diagnostic::error(
                    here,
                    format!("`meta` is scoped to undeclared face `{face}`"),
                )));
                continue;
            }
            let by_scope = declared_meta.entry(entry.slot()).or_default();
            if let Some(&first) = by_scope.get(&scope) {
                let (path, _, file_line) = docset.location(first);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                issues.push(docset.to_issue(&Diagnostic::error(
                    here,
                    format!(
                        "{} is set more than once (also at {name}:{file_line})",
                        entry.describe_slot(),
                    ),
                )));
            } else {
                by_scope.insert(scope, here);
            }
        }
    }
    for (slot, by_scope) in &declared_meta {
        if by_scope.len() < 2 {
            continue;
        }
        for face in &faces.faces {
            let reaching: Vec<(&Option<String>, &ItemRef)> = by_scope
                .iter()
                .filter(|(scope, _)| match scope {
                    None => true,
                    Some(f) => *f == face.id,
                })
                .collect();
            if reaching.len() < 2 {
                continue;
            }
            let (_, first) = reaching[0];
            let (path, _, file_line) = docset.location(*first);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            for (_, here) in &reaching[1..] {
                issues.push(docset.to_issue(&Diagnostic::error(
                    **here,
                    format!(
                        "`meta {slot}` is set both for every face and for face `{}`, \
                             so face `{}` has two values (the other at {name}:{file_line})",
                        face.label(),
                        face.label(),
                    ),
                )));
            }
        }
    }

    // Two faces the OS cannot tell apart are two faces the user cannot pick
    // between, and a duplicate PostScript name additionally breaks PDF
    // embedding. Checked across faces because no single face can see it.
    if faces.faces.len() > 1 {
        let mut seen_full: HashMap<(String, String), &str> = HashMap::new();
        let mut seen_ps: HashMap<String, &str> = HashMap::new();
        for face in &faces.faces {
            let id = if face.id.is_empty() {
                None
            } else {
                Some(face.id.as_str())
            };
            let m = crate::meta::FontMeta::for_face(docs, id);
            let key = (m.family().to_string(), m.subfamily().to_string());
            if let Some(prev) = seen_full.get(&key) {
                issues.push(docset.to_issue(&Diagnostic::error(
                    face.origin,
                    format!(
                        "faces `{prev}` and `{}` both name themselves `{} {}`; \
                             the OS files fonts by family and subfamily, so one would hide \
                             the other",
                        face.label(),
                        key.0,
                        key.1,
                    ),
                )));
            } else {
                seen_full.insert(key, face.label());
            }
            let ps = m.postscript_name();
            if let Some(prev) = seen_ps.get(&ps) {
                issues.push(docset.to_issue(&Diagnostic::error(
                    face.origin,
                    format!(
                        "faces `{prev}` and `{}` share the PostScript name `{ps}`",
                        face.label(),
                    ),
                )));
            } else {
                seen_ps.insert(ps, face.label());
            }
        }
    }

    let meta = &resolution.meta;
    let origin = meta.origin;
    if let (Some(h), Some(a), Some(d)) = (
        meta.metrics.height,
        meta.metrics.ascent,
        meta.metrics.descent,
    ) && a as u32 + d as u32 != h as u32
    {
        issues.push(docset.to_issue(&Diagnostic::new(
            Severity::Warning,
            origin,
            format!("meta ascent ({a}) + descent ({d}) != height ({h})"),
        )));
    }
    if meta.metrics.height == Some(0) {
        issues.push(docset.to_issue(&Diagnostic::error(origin, "meta height is 0")));
    }
}
