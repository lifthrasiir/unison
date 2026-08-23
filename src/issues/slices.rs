//! Checks over slice qualifiers and the `name-parts` bindings scoped to
//! them: which slices a line may name, which slice binds a part, and which
//! declared slice nothing is qualified to.

use std::collections::{HashMap, HashSet};

use crate::document::DocumentItem;
use crate::resolve::Diagnostic;

use super::{Cx, Issue, Severity, issue_at};

/// Per-item slice checks: the slices a line names, and the name parts it
/// leaves unbound in one of them.
pub(super) fn check_slice_qualifiers(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let name_parts = cx.name_parts;
    let scoped_parts = &cx.scoped_parts;
    let faces = cx.faces;
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let referenced: Vec<&String> = match item {
                DocumentItem::AssertShape { slices, .. } => slices.iter().collect(),
                DocumentItem::Slice { inherits, .. } => inherits.iter().collect(),
                item => item.slice_qualifier().iter().collect(),
            };
            for name in referenced {
                if !faces.declared.contains_key(name) {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!("undeclared slice `{name}`",),
                    ));
                }
            }
            // The same slice twice in one qualifier would state the line twice
            // for it, which is a duplicate mapping and never what was meant.
            let qualifier = item.slice_qualifier();
            for (i, name) in qualifier.iter().enumerate() {
                if qualifier[..i].contains(name) {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!("slice `{name}` is listed twice in this qualifier"),
                    ));
                }
            }
            // A name part bound per slice reaches only the lines stated for
            // that slice. Checked against what was written, because what it
            // fails as downstream — an odd glyph name, a `ref` to nothing —
            // never mentions the binding.
            {
                let stated: Vec<Option<&str>> = if qualifier.is_empty() {
                    vec![None]
                } else {
                    qualifier.iter().map(|s| Some(s.as_str())).collect()
                };
                let mut reported: Vec<String> = Vec::new();
                for slice in stated {
                    let parts = scoped_parts.for_slice(slice);
                    for name in written_names(item) {
                        let Some(part) = unbound_scoped_part(name, parts, scoped_parts) else {
                            continue;
                        };
                        if reported.contains(&part) {
                            continue;
                        }
                        let where_ = match slice {
                            Some(s) => format!("slice `{s}`"),
                            None => "the base slice".to_string(),
                        };
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "name part `{part}` is bound per slice and not for {where_}, \
                                 so it substitutes nothing here",
                            ),
                        ));
                        reported.push(part);
                    }
                }
            }
            // A slice-scoped binding stands for one name part in one slice, so
            // it is one value. A list would go back to being an alternation
            // that the slices no longer control.
            if let DocumentItem::NameParts { slices, values, .. } = item
                && !slices.is_empty()
            {
                let resolved = crate::document::resolve_name_part_values(values, name_parts);
                if resolved.len() != 1 {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "a slice-scoped `name-parts` takes exactly one value, not {}",
                            resolved.len(),
                        ),
                    ));
                }
            }
            // An assertion whose slice combination no face satisfies would
            // never run, and a test that silently does not run is worse than
            // one that fails.
            if let DocumentItem::AssertShape { slices, .. } = item
                && !slices.is_empty()
                && faces.declared.keys().any(|k| slices.contains(k))
                && !faces.faces.iter().any(|f| f.includes_all(slices))
            {
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    format!(
                        "no face includes all of `{}`, so this assertion would never run",
                        slices.join("`, `"),
                    ),
                ));
            }
        }
    }
}

pub(super) fn check_name_part_bindings(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let _faces = cx.faces;
    // A name part is bound unqualified or per slice, never both: an
    // unqualified binding that a slice overrode would be a precedence rule,
    // and `crate::faces` has none. Two bindings for one slice are the same
    // conflict a slice deeper in.
    let mut seen: HashMap<(&str, Option<&str>), ()> = HashMap::new();
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::NameParts { slices, name, .. } = item else {
                continue;
            };
            let stated: Vec<Option<&str>> = if slices.is_empty() {
                vec![None]
            } else {
                slices.iter().map(|s| Some(s.as_str())).collect()
            };
            for slice in stated {
                if seen.insert((name.as_str(), slice), ()).is_some() {
                    let where_ = match slice {
                        Some(s) => format!("slice `{s}`"),
                        None => "no slice".to_string(),
                    };
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!("name part `{name}` is already bound for {where_}"),
                    ));
                }
                // The unqualified binding is the one that would be
                // overridden, so it is what the conflict is reported
                // against — whichever line came second.
                let other = if slice.is_none() {
                    seen.keys().any(|(n, s)| n == &name.as_str() && s.is_some())
                } else {
                    seen.contains_key(&(name.as_str(), None))
                };
                if other {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "name part `{name}` is bound both unqualified and per slice; \
                                 a slice-scoped binding is not an override",
                        ),
                    ));
                }
            }
        }
    }
}

pub(super) fn check_empty_slices(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let docset = &cx.docset;
    let faces = cx.faces;
    // A slice nothing is qualified to gives every face that includes it
    // nothing. Mirrors the "remap group is declared but has no rules" warning,
    // and matters most mid-migration: moving characters out of the base into
    // two slices is exactly where a typo leaves one of them empty.
    //
    // Content is counted transitively, so a slice that exists only to compose
    // others (`slice both = narrow wide`) is not empty when they are not.
    let mut has_own: HashSet<&str> = HashSet::new();
    for doc in docs {
        for item in &doc.items {
            match item {
                // `name-parts` is deliberately not counted: a binding is
                // how a slice spells a name, not something a face gets.
                DocumentItem::Map { slices, .. }
                | DocumentItem::MapDecomposed { slices, .. }
                | DocumentItem::Feature { slices, .. }
                | DocumentItem::FeatureAnchor { slices, .. } => {
                    has_own.extend(slices.iter().map(String::as_str));
                }
                DocumentItem::AssertShape { slices, .. } => {
                    has_own.extend(slices.iter().map(String::as_str));
                }
                _ => {}
            }
        }
    }
    // Reachability over `inherits`, bounded by the number of slices, so a
    // cycle (reported elsewhere) cannot spin here.
    for (name, (_, origin)) in &faces.declared {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![name.as_str()];
        let mut found = false;
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur) {
                continue;
            }
            if has_own.contains(cur) {
                found = true;
                break;
            }
            if let Some((inherits, _)) = faces.declared.get(cur) {
                stack.extend(inherits.iter().map(String::as_str));
            }
        }
        if !found {
            issues.push(docset.to_issue(&Diagnostic::new(
                Severity::Warning,
                Some(*origin),
                format!(
                    "slice `{name}` is declared but nothing is qualified to it, \
                         so every face including it gets nothing from it",
                ),
            )));
        }
    }
}

/// Problems in a `feature ... for ...` target list.
///
/// A target is an OpenType script tag, optionally narrowed to one language
/// system below it as `script/LANG`. Both registries use 4-byte tags, so a
/// longer part would be silently truncated to something that resolves to
/// nothing — worth an error rather than a font that quietly ignores the
/// declaration.
/// The first `$part` in `name` that `parts` does not bind but some slice does.
///
/// A slice-scoped binding substitutes nothing where it does not apply, and what
/// is left behind — a `$` in a glyph name, a `ref` to nothing — says nothing
/// about why. This is what lets the report say it.
fn unbound_scoped_part(
    name: &str,
    parts: &crate::pattern::NamePartsMap,
    scoped: &crate::document::SliceNameParts,
) -> Option<String> {
    if !name.contains('$') {
        return None;
    }
    name.match_indices('$').find_map(|(at, _)| {
        let end = name[at + 1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .map_or(name.len(), |i| at + 1 + i);
        let part = &name[at..end];
        (!parts.contains_key(part) && scoped.is_slice_scoped(part)).then(|| part.to_string())
    })
}

/// The names written on one item, for a check that works on the source text
/// rather than on what expansion made of it.
fn written_names(item: &DocumentItem) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    match item {
        DocumentItem::Glyph { name, body } => {
            names.push(name.0.as_str());
            names.extend(body.refs.iter().map(|r| r.name.as_str()));
        }
        DocumentItem::GlyphAlias { name, target, .. } => {
            names.push(name.0.as_str());
            names.push(target.as_str());
        }
        DocumentItem::Map { glyphs, .. } => names.extend(glyphs.iter().map(String::as_str)),
        DocumentItem::MapDecomposed { glyph, .. } => {
            names.extend(glyph.as_deref());
        }
        DocumentItem::Remap { .. } => names.extend(item.remap_operands().map(String::as_str)),
        _ => {}
    }
    names
}
