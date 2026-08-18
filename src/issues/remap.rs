//! The per-document scan over glyph and `remap` items: duplicate and
//! contentless glyph definitions, and every group-level `remap` problem.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{
    DocumentItem, GlyphName, find_invalid_inline_ranges, is_name_pattern, substitute_name_parts,
};
use crate::pattern::NamePattern;

use super::{Cx, Issue, Severity, issue_at, short_path};

pub(super) fn check_glyphs_and_remaps(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let name_parts = cx.name_parts;
    let _expansion = cx.expansion;
    let groups = &cx.groups;
    let _resolution = cx.resolution;
    let mut glyph_defs: HashMap<String, (PathBuf, usize)> = HashMap::new();

    // Groups are collected up front rather than as the scan reaches them: a
    // `feature` line may precede every rule of the group it attaches, and a
    // declaration may follow them.
    let mut remap_group_issues: Vec<(String, Severity, String)> = Vec::new();
    for (group, target) in &groups.unknown_after {
        remap_group_issues.push((
            group.clone(),
            Severity::Error,
            format!(
                "remap group '{}' is ordered after undefined group '{}'",
                group, target,
            ),
        ));
    }
    if !groups.cycle.is_empty() {
        let names = groups.cycle.join("', '");
        for group in &groups.cycle {
            remap_group_issues.push((
                group.clone(),
                Severity::Error,
                format!(
                    "remap group '{}' is in an ordering cycle with '{}'; \
                 the groups fall back to source order",
                    group, names,
                ),
            ));
        }
    }
    for group in &groups.duplicate_decls {
        remap_group_issues.push((
            group.clone(),
            Severity::Error,
            format!("remap group '{}' is declared more than once", group,),
        ));
    }
    // Over `order`, not over `info`: a HashMap would make the report's wording
    // stable but its order not.
    for group in &groups.order {
        let info = &groups.info[group];
        if info.declared && !info.has_rules {
            remap_group_issues.push((
                group.clone(),
                Severity::Warning,
                format!("remap group '{}' is declared but has no rules", group,),
            ));
        }
    }

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                DocumentItem::Glyph {
                    name: GlyphName(n),
                    body,
                } => {
                    for bad in find_invalid_inline_ranges(n) {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("invalid inline range '{}' (end < start or too large)", bad,),
                        ));
                    }
                    // Duplicate detection needs the *defining* line of each
                    // expanded name, which the expansion does not retain, so
                    // it expands names (not bodies) once more here.
                    let name_str = substitute_name_parts(n, name_parts);
                    let expanded: Vec<String> = if is_name_pattern(&name_str) {
                        // A pattern that fails to expand is already reported
                        // by the resolution pass.
                        NamePattern::parse(&name_str)
                            .map(|e| e.into_vec())
                            .unwrap_or_default()
                    } else {
                        vec![name_str]
                    };
                    for en in expanded {
                        if let Some((prev_file, prev_line)) = glyph_defs.get(en.as_str()) {
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Warning,
                                format!(
                                    "duplicate glyph '{}' (first defined at {}:{})",
                                    en,
                                    short_path(prev_file),
                                    prev_line,
                                ),
                            ));
                        } else {
                            let (_, file_line) = doc.item_lines(item_idx);
                            glyph_defs.insert(en, (doc.path.clone(), file_line));
                        }
                    }

                    if body.pixels.is_none()
                        && body.refs.is_empty()
                        && !body.keep
                        && body.advance.is_none()
                        && body.origin.is_none()
                        && body.points.is_empty()
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Warning,
                            format!("glyph '{}' has no content", n,),
                        ));
                    }
                }
                DocumentItem::RemapGroup { name, .. } => {
                    // Every group-level problem is reported here, on the line
                    // that declares the group — the constraint is written here
                    // even where its effect is felt somewhere else entirely.
                    for (group, severity, message) in &remap_group_issues {
                        if group == name {
                            issues.push(issue_at(doc, item_idx, *severity, message.clone()));
                        }
                    }
                }
                DocumentItem::Remap {
                    feature,
                    source,
                    target,
                    ..
                } => {
                    // OpenType has a lookup type for one-to-one, one-to-many
                    // (including one-to-nothing) and many-to-one, and nothing
                    // for the rest. The builder used to emit whatever was
                    // closest and lose the difference in silence.
                    if crate::render::ttf_builder::remap_rule_kind(source.len(), target.len())
                        .is_none()
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "remap of {} glyph(s) to {} glyph(s) has no OpenType lookup type; \
                             a source of more than one glyph needs exactly one target",
                                source.len(),
                                target.len(),
                            ),
                        ));
                    }

                    // The reverse lookup substitutes one glyph for one glyph
                    // and has no ligature or multiple form at all, so a rule of
                    // any other shape cannot be built here — and rebuilding the
                    // group forward to accommodate it would silently take away
                    // the very thing `reversed` was asked for.
                    if groups.info.get(feature).is_some_and(|i| i.reversed)
                        && (source.len() != 1 || target.len() != 1)
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "remap group '{}' is reversed, so each of its rules must \
                             substitute one glyph for one glyph; this one has {} source \
                             glyph(s) and {} target glyph(s)",
                                feature,
                                source.len(),
                                target.len(),
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}
