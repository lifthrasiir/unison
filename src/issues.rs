//! Cross-document validation: everything that can only be judged with all
//! `.unf` files in hand.
//!
//! Missing and dangling refs, duplicate `map`s, unused glyphs, remap sanity.
//! Resolution itself emits its diagnostics directly (see [`crate::resolve`]);
//! this module is for what no single item's resolution can see. Both the build
//! and the editor print the same report, `error:`/`warning:` prefixed and
//! `file:line:` located, and a font with only warnings still builds — so the
//! report is meant to be read, not just exit-coded.
//!
//! A few rules are worth knowing about because they are refusals rather than
//! best-effort output:
//!
//! - a `remap` whose source and target lists are N→M or N→0 has no OpenType
//!   lookup type at all, so it is an error here instead of something the builder
//!   emits close-but-wrong;
//! - referring to a contentless glyph — one with no pixel grid and no `ref`, see
//!   [`crate::document_io`] — from a `map`, `ref` or `remap` is an error, since
//!   such a glyph never enters the resolution cache;
//! - the two anchor-exposure ambiguities in [`crate::ref_composite`] are errors,
//!   reported through an anchors-only resolution pass.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::document::{
    Directive, Document, DocumentItem, GlyphName, classify_directive, expand_name_element,
    find_invalid_inline_ranges, is_name_pattern, substitute_name_parts,
};
use crate::pattern::NamePattern;
use crate::resolve::{Diagnostic, DocSet, Resolution};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
    pub file: PathBuf,
    /// DocLine index (0-based), used for editor navigation.
    pub line: usize,
    /// 1-based file line number, used for display.
    pub file_line: usize,
}

/// An issue anchored at item `item_idx`'s defining line in `doc`.
fn issue_at(doc: &Document, item_idx: usize, severity: Severity, message: String) -> Issue {
    let (line, file_line) = doc.item_lines(item_idx);
    Issue { severity, message, file: doc.path.clone(), line, file_line }
}

/// Problems in a `feature ... for ...` target list.
///
/// A target is an OpenType script tag, optionally narrowed to one language
/// system below it as `script/LANG`. Both registries use 4-byte tags, so a
/// longer part would be silently truncated to something that resolves to
/// nothing — worth an error rather than a font that quietly ignores the
/// declaration.
fn script_lang_issues(targets: &[String]) -> Vec<String> {
    let mut issues = Vec::new();
    for target in targets {
        let mut parts = target.split('/');
        let script = parts.next().unwrap_or("");
        let lang = parts.next();
        if parts.next().is_some() {
            issues.push(format!(
                "feature target '{target}' has more than one '/'; \
                 write it as SCRIPT or SCRIPT/LANGUAGE",
            ));
            continue;
        }
        for (what, tag) in [("script", Some(script)), ("language", lang)] {
            let Some(tag) = tag else { continue };
            if tag.is_empty() || tag.len() > 4 || !tag.is_ascii() {
                issues.push(format!(
                    "feature target '{target}' has an invalid {what} tag '{tag}'; \
                     OpenType tags are 1 to 4 ASCII characters",
                ));
            }
        }
    }
    issues
}

pub fn collect_issues(docs: &[&Document]) -> Vec<Issue> {
    collect_issues_with(docs, &Resolution::compute(docs))
}

/// Validate `docs` against an already-computed [`Resolution`].
///
/// Callers that resolve for their own reasons — the editor's glyph cache, the
/// font build — should use this rather than [`collect_issues`], which resolves
/// again from scratch.
pub fn collect_issues_with(docs: &[&Document], resolution: &Resolution) -> Vec<Issue> {
    let mut issues = Vec::new();

    let name_parts = &resolution.name_parts;
    let expansion = &resolution.expansion;
    let docset = DocSet::new(docs);

    // Resolution is the same expansion the font build performs, so the
    // problems it detects — unresolvable references, maps that cannot be
    // synthesized, on-demand names that resolve to nothing — are reported
    // here instead of silently skipped, and this file does not reimplement
    // any of it.
    issues.extend(docset.to_issues(&expansion.diagnostics));

    // Every glyph the font will actually contain, including synthesized
    // on-demand and decomposed-map glyphs.
    let all_glyph_names: HashSet<String> = expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Glyph { name: GlyphName(n), .. } => Some(n.clone()),
            _ => None,
        })
        .collect();

    let mut glyph_defs: HashMap<String, (PathBuf, usize)> = HashMap::new();

    // Groups are collected up front rather than as the scan reaches them: a
    // `feature` line may precede every rule of the group it attaches, and a
    // declaration may follow them.
    let groups = crate::document::remap_group_order(docs);
    let mut remap_group_issues: Vec<(String, Severity, String)> = Vec::new();
    for (group, target) in &groups.unknown_after {
        remap_group_issues.push((group.clone(), Severity::Error, format!(
            "remap group '{}' is ordered after undefined group '{}'", group, target,
        )));
    }
    if !groups.cycle.is_empty() {
        let names = groups.cycle.join("', '");
        for group in &groups.cycle {
            remap_group_issues.push((group.clone(), Severity::Error, format!(
                "remap group '{}' is in an ordering cycle with '{}'; \
                 the groups fall back to source order",
                group, names,
            )));
        }
    }
    for group in &groups.duplicate_decls {
        remap_group_issues.push((group.clone(), Severity::Error, format!(
            "remap group '{}' is declared more than once", group,
        )));
    }
    // Over `order`, not over `info`: a HashMap would make the report's wording
    // stable but its order not.
    for group in &groups.order {
        let info = &groups.info[group];
        if info.declared && !info.has_rules {
            remap_group_issues.push((group.clone(), Severity::Warning, format!(
                "remap group '{}' is declared but has no rules", group,
            )));
        }
    }

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                DocumentItem::Glyph { name: GlyphName(n), body } => {
                    for bad in find_invalid_inline_ranges(n) {
                        issues.push(issue_at(doc, item_idx, Severity::Error, format!(
                            "invalid inline range '{}' (end < start or too large)",
                            bad,
                        )));
                    }
                    // Duplicate detection needs the *defining* line of each
                    // expanded name, which the expansion does not retain, so
                    // it expands names (not bodies) once more here.
                    let name_str = substitute_name_parts(n, &name_parts);
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
                            issues.push(issue_at(doc, item_idx, Severity::Warning, format!(
                                "duplicate glyph '{}' (first defined at {}:{})",
                                en,
                                short_path(prev_file),
                                prev_line,
                            )));
                        } else {
                            let (_, file_line) = doc.item_lines(item_idx);
                            glyph_defs.insert(en, (doc.path.clone(), file_line));
                        }
                    }

                    if body.pixels.is_none()
                        && body.refs.is_empty()
                        && !body.sticky
                        && body.advance.is_none()
                        && body.left.is_none()
                        && body.points.is_empty()
                    {
                        issues.push(issue_at(doc, item_idx, Severity::Warning, format!(
                            "glyph '{}' has no content", n,
                        )));
                    }
                }
                DocumentItem::RemapGroup { name, .. } => {
                    // Every group-level problem is reported here, on the line
                    // that declares the group — the constraint is written here
                    // even where its effect is felt somewhere else entirely.
                    for (group, severity, message) in &remap_group_issues {
                        if group == name {
                            issues.push(issue_at(doc, item_idx, severity.clone(), message.clone()));
                        }
                    }
                }
                DocumentItem::Remap { source, target, .. } => {
                    // OpenType has a lookup type for one-to-one, one-to-many
                    // (including one-to-nothing) and many-to-one, and nothing
                    // for the rest. The builder used to emit whatever was
                    // closest and lose the difference in silence.
                    if crate::render::ttf_builder::remap_rule_kind(
                        source.len(), target.len(),
                    ).is_none() {
                        issues.push(issue_at(doc, item_idx, Severity::Error, format!(
                            "remap of {} glyph(s) to {} glyph(s) has no OpenType lookup type; \
                             a source of more than one glyph needs exactly one target",
                            source.len(), target.len(),
                        )));
                    }
                }
                _ => {}
            }
        }
    }

    // font-meta validation. Only the effective numbers matter to the font
    // build; here the distinction between "not declared" and "declared as the
    // default" is what decides whether to complain at all.
    {
        let meta = &resolution.meta;
        let origin = meta.origin;
        if let (Some(h), Some(a), Some(d)) = (meta.height, meta.ascent, meta.descent)
            && a + d != h
        {
            issues.push(docset.to_issue(&Diagnostic::new(
                Severity::Warning,
                origin,
                format!("font-meta ascent ({a}) + descent ({d}) != height ({h})"),
            )));
        }
        if meta.height == Some(0) {
            issues.push(docset.to_issue(&Diagnostic::error(
                origin,
                "font-meta height is 0",
            )));
        }
    }

    let mut mapped_codepoints: HashMap<u32, (PathBuf, usize)> = HashMap::new();
    let mut mapped_glyphs: HashSet<String> = HashSet::new();

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                // Unresolvable refs, map targets and remap operands are all
                // reported by the resolution pass above.
                DocumentItem::Map { char_repr, glyph, .. } => {
                    let subst_glyph = substitute_name_parts(glyph, &name_parts);
                    let expanded_pairs =
                        crate::render::ttf_builder::expand_map_pairs(
                            char_repr, &subst_glyph,
                        );
                    for (cp, target) in &expanded_pairs {
                        mapped_glyphs.insert(target.clone());
                        if let Some((prev_file, prev_line)) =
                            mapped_codepoints.get(cp)
                        {
                            issues.push(issue_at(doc, item_idx, Severity::Warning, format!(
                                "duplicate codepoint mapping U+{:04X} (first at {}:{})",
                                cp,
                                short_path(prev_file),
                                prev_line,
                            )));
                        } else {
                            let (_, file_line) = doc.item_lines(item_idx);
                            mapped_codepoints
                                .insert(*cp, (doc.path.clone(), file_line));
                        }
                    }
                }
                DocumentItem::Feature {
                    scripts, remap_group, ..
                } => {
                    if !groups.info.contains_key(remap_group.as_str()) {
                        issues.push(issue_at(doc, item_idx, Severity::Error, format!(
                            "feature references undefined remap group '{}'",
                            remap_group,
                        )));
                    }
                    for issue in script_lang_issues(scripts) {
                        issues.push(issue_at(doc, item_idx, Severity::Error, issue));
                    }
                }
                DocumentItem::FeatureAnchor { scripts, .. } => {
                    for issue in script_lang_issues(scripts) {
                        issues.push(issue_at(doc, item_idx, Severity::Error, issue));
                    }
                }
                DocumentItem::NameParts { values, .. } => {
                    for val in values {
                        if val.starts_with('$') && !name_parts.contains_key(val.as_str()) {
                            issues.push(issue_at(doc, item_idx, Severity::Warning, format!(
                                "undefined name-parts reference '{}'",
                                val,
                            )));
                        }
                    }
                }
                DocumentItem::Directive(text) => {
                    if classify_directive(text) == Directive::Unrecognized {
                        issues.push(issue_at(doc, item_idx, Severity::Warning, format!(
                            "unrecognized directive '{}'", text.trim(),
                        )));
                    }
                }
                _ => {}
            }
        }
    }

    // Detect unused glyphs: glyphs not reachable from any map/remap root.
    // Works at glyph-item granularity to avoid expensive repeated pattern expansion.
    {
        // Assign each glyph item an index; track which items are reachable.
        // name_to_item: expanded glyph name -> item index
        let mut name_to_item: HashMap<String, usize> = HashMap::new();
        // item_refs[i]: ref target names (expanded) for item i
        let mut item_refs: Vec<Vec<String>> = Vec::new();
        // item_location[i]: (doc_idx, item_idx, raw_name) for reporting
        let mut item_location: Vec<(usize, usize, &str)> = Vec::new();

        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
                    let idx = item_refs.len();
                    item_location.push((doc_idx, item_idx, n));

                    for en in expand_name_element(n, name_parts) {
                        name_to_item.entry(en).or_insert(idx);
                    }

                    let mut refs = Vec::new();
                    for gref in &body.refs {
                        refs.extend(expand_name_element(&gref.name, name_parts));
                    }
                    item_refs.push(refs);
                }
            }
        }

        // Collect root names from map targets and remap references.
        let mut root_names: HashSet<String> = mapped_glyphs;
        // .notdef is always required in TrueType fonts.
        root_names.insert(".notdef".to_string());

        for doc in docs {
            for item in &doc.items {
                match item {
                    DocumentItem::Remap { .. } => {
                        for token in item.remap_operands() {
                            root_names.extend(expand_name_element(token, name_parts));
                        }
                    }
                    DocumentItem::Glyph { name: GlyphName(n), body } => {
                        if body.sticky || body.mark {
                            root_names.extend(expand_name_element(n, name_parts));
                        }
                    }
                    DocumentItem::Directive(text) => {
                        if let Directive::AssumeUnused(rest) = classify_directive(text) {
                            for token in rest.split_whitespace() {
                                root_names.extend(expand_name_element(token, name_parts));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Build alternative lookup: base name -> list of "base:variant" names.
        let mut alt_names: HashMap<&str, Vec<&str>> = HashMap::new();
        for name in all_glyph_names.iter() {
            if let Some(colon_pos) = name.find(':') {
                let base = &name[..colon_pos];
                alt_names.entry(base).or_default().push(name.as_str());
            }
        }

        // BFS over items.
        let mut reachable_items: Vec<bool> = vec![false; item_refs.len()];
        let mut queue: Vec<usize> = Vec::new();

        // Seed with root names.
        for name in &root_names {
            if let Some(&idx) = name_to_item.get(name.as_str()) {
                if !reachable_items[idx] {
                    reachable_items[idx] = true;
                    queue.push(idx);
                }
            }
            // Alternatives of root names are also roots.
            if let Some(alts) = alt_names.get(name.as_str()) {
                for &alt in alts {
                    if let Some(&idx) = name_to_item.get(alt) {
                        if !reachable_items[idx] {
                            reachable_items[idx] = true;
                            queue.push(idx);
                        }
                    }
                }
            }
        }

        while let Some(item_idx) = queue.pop() {
            for ref_name in &item_refs[item_idx] {
                if let Some(&target_item) = name_to_item.get(ref_name.as_str()) {
                    if !reachable_items[target_item] {
                        reachable_items[target_item] = true;
                        queue.push(target_item);
                    }
                }
                // Alternatives of ref targets are also reachable.
                if let Some(alts) = alt_names.get(ref_name.as_str()) {
                    for &alt in alts {
                        if let Some(&alt_item) = name_to_item.get(alt) {
                            if !reachable_items[alt_item] {
                                reachable_items[alt_item] = true;
                                queue.push(alt_item);
                            }
                        }
                    }
                }
            }
        }

        // Report unreachable items.
        for (idx, &reached) in reachable_items.iter().enumerate() {
            if !reached {
                let (doc_idx, doc_item_idx, name) = item_location[idx];
                let doc = docs[doc_idx];
                issues.push(issue_at(doc, doc_item_idx, Severity::Warning, format!(
                    "glyph '{}' is unused", name,
                )));
            }
        }
    }

    // Detect alternative glyphs with ambiguous anchor matches.
    // For base "foo", if "foo" and "foo:bar" both have a `-name` anchor with
    // the same dimensions, warn that they are ambiguous (the first alphabetically wins).
    {
        let mut bases_to_alts: HashMap<String, Vec<(String, PathBuf, usize, usize)>> = HashMap::new();
        for doc in docs {
            for (item_idx, item) in doc.items.iter().enumerate() {
                if let DocumentItem::Glyph { name: GlyphName(n), body } = item
                    && body.points.iter().any(|p| p.position.starts_with('-')) {
                        let resolved_name = substitute_name_parts(n, &name_parts);
                        let (line, file_line) = doc.item_lines(item_idx);
                        // Find all base prefixes (foo:bar:quux is alt for "foo" and "foo:bar")
                        for prefix in crate::ref_composite::alternative_prefixes(&resolved_name) {
                            bases_to_alts
                                .entry(prefix.to_string())
                                .or_default()
                                .push((resolved_name.clone(), doc.path.clone(), line, file_line));
                        }
                        // Also register as the base itself
                        bases_to_alts
                            .entry(resolved_name.clone())
                            .or_default()
                            .push((resolved_name.clone(), doc.path.clone(), line, file_line));
                    }
            }
        }

        // For each base, find point definitions and check for dimension conflicts.
        let mut glyph_points_map: HashMap<String, Vec<(String, u16, u16)>> = HashMap::new();
        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
                    let resolved_name = substitute_name_parts(n, &name_parts);
                    for pt in &body.points {
                        if pt.position.starts_with('-') {
                            glyph_points_map
                                .entry(resolved_name.clone())
                                .or_default()
                                .push((pt.position.clone(), pt.width(), pt.height()));
                        }
                    }
                }
            }
        }

        for (base, alts) in &bases_to_alts {
            if alts.len() < 2 {
                continue;
            }
            // Group by (position_name, width, height) and find duplicates.
            let mut seen: HashMap<(String, u16, u16), Vec<&str>> = HashMap::new();
            for (alt_name, _, _, _) in alts {
                if let Some(pts) = glyph_points_map.get(alt_name) {
                    for (pos, w, h) in pts {
                        seen.entry((pos.clone(), *w, *h))
                            .or_default()
                            .push(alt_name);
                    }
                }
            }
            for ((pos, _w, _h), names) in &seen {
                if names.len() > 1 {
                    // Warn on all but the first (alphabetically).
                    let mut sorted_names: Vec<&str> = names.to_vec();
                    sorted_names.sort();
                    for &dup in &sorted_names[1..] {
                        if let Some((_, file, line, file_line)) = alts.iter().find(|(n, _, _, _)| n == dup) {
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "alternative '{}' has same anchor dimensions as '{}' for '{}' (base '{}')",
                                    dup, sorted_names[0], pos, base,
                                ),
                                file: file.clone(),
                                line: *line,
                                file_line: *file_line,
                            });
                        }
                    }
                }
            }
        }
    }

    // Anchor derivation problems: a composite that would expose the same
    // anchor name from more than one source, and a `-` anchor with more than
    // one `+` candidate to attach to. This runs an anchors-only pass through
    // the same shared driver and the same derivation the font build uses
    // (`glyph_cache`/`derive_ref_offsets_with`), so what is reported here is
    // exactly what resolution dropped.
    {
        struct AnchorsOnly {
            anchors: Vec<crate::document::GlyphPoint>,
            w: u16,
            h: u16,
        }
        impl AnchorsOnly {
            fn new() -> Self {
                Self { anchors: Vec::new(), w: 0, h: 0 }
            }
        }
        impl crate::render::glyph_cache::CachedGlyphEntry for AnchorsOnly {
            fn anchors(&self) -> &[crate::document::GlyphPoint] {
                &self.anchors
            }
            fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
                (&mut self.w, &mut self.h)
            }
            fn set_resolution(
                &mut self,
                anchors: Vec<crate::document::GlyphPoint>,
                _scale: u8,
            ) {
                self.anchors = anchors;
            }
        }

        let mut declared_anchors: HashMap<&str, &[crate::document::GlyphPoint]> =
            HashMap::new();
        let mut origin_of: HashMap<&str, Option<crate::resolve::ItemRef>> = HashMap::new();
        for e in &expansion.items {
            if let DocumentItem::Glyph { name: GlyphName(n), body } = &e.item {
                declared_anchors.entry(n).or_insert(&body.points);
                origin_of.entry(n).or_insert(e.origin);
            }
        }

        let (mut cache, pending) = crate::render::glyph_cache::seed_cache(
            expansion.items(),
            |_| AnchorsOnly::new(),
            AnchorsOnly::new,
        );
        let mut derive_issues: Vec<(String, crate::ref_composite::DeriveIssue)> = Vec::new();
        crate::render::glyph_cache::resolve_pending(
            &mut cache,
            pending,
            |name| declared_anchors.get(name).map(|pts| pts.to_vec()),
            |_, _, _| AnchorsOnly::new(),
            |name, issue| derive_issues.push((name.to_string(), issue)),
        );
        for (name, issue) in derive_issues {
            let severity = if issue.is_error() { Severity::Error } else { Severity::Warning };
            issues.push(docset.to_issue(&Diagnostic::new(
                severity,
                origin_of.get(name.as_str()).copied().flatten(),
                issue.message(&name),
            )));
        }
    }

    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    issues
}

impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let rank = |s: &Severity| match s {
            Severity::Error => 0,
            Severity::Warning => 1,
        };
        rank(self).cmp(&rank(other))
    }
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io;

    #[test]
    fn unresolved_ref_reported() {
        let input = "glyph foo\nref nonexistent 0 0\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("unresolved ref")),
            "expected unresolved ref error, got: {issues:?}",
        );
    }

    #[test]
    fn duplicate_inherited_anchors_reported() {
        let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0 inherit
ref half 2 0 inherit
map D = digraph
map h = half
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("digraph")
                && i.message.contains("'+above'")),
            "expected duplicate exposed anchor error, got: {issues:?}",
        );
    }

    #[test]
    fn ambiguous_attachment_reported() {
        let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph mark 2 1 mark
@@@@
anchor -above 0 0
glyph combo
ref half 0 0
ref half 2 0
ref mark
map D = combo
map h = half
map m = mark
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("combo")
                && i.message.contains("'mark'")
                && i.message.contains("'-above'")),
            "expected ambiguous attachment error, got: {issues:?}",
        );
    }

    /// A `-` anchor that name-matches a published `+` but size-mismatches it
    /// is a near-miss (usually the wrong `:narrow`/`:wide` variant), reported
    /// as a warning rather than silently not attaching.
    #[test]
    fn size_mismatched_attachment_reported() {
        let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1..2 0
glyph mark 2 1 mark
@@@@
anchor -above 0 0
glyph combo
ref base
ref mark 1 2
map D = combo
map h = base
map m = mark
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("combo")
                && i.message.contains("'mark'")
                && i.message.contains("'-above'")),
            "expected size-mismatch warning, got: {issues:?}",
        );
    }

    /// The validation pass must resolve an alternative *before* any composite
    /// that needs it for size-driven substitution — same guard as the
    /// editor's `resolve_expansion` — or it reports a mismatch the real
    /// resolution does not have.
    #[test]
    fn alternative_pending_in_same_round_still_substitutes() {
        let input = "\
glyph circle 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph circle:alt
ref circle
anchor +center 2 1

glyph j-inner 2 2
@@@@
@@@@
anchor -center 1 0

glyph j-circled
ref circle
ref j-inner
map j = j-circled
map c = circle
map i = j-inner
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("only by name")),
            "circle:alt must be substituted, got: {issues:?}",
        );
    }

    /// A digraph without `inherit` exposes nothing — that is the designed
    /// fallback, not a problem to report.
    #[test]
    fn non_inherited_duplicates_are_quiet() {
        let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0
ref half 2 0
map D = digraph
map h = half
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error),
            "expected no errors, got: {issues:?}",
        );
    }

    #[test]
    fn duplicate_glyph_reported() {
        let input = "glyph foo 2 1\n..@@\nglyph foo 2 1\n@@..\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.message.contains("duplicate glyph")),
            "expected duplicate glyph warning, got: {issues:?}",
        );
    }

    #[test]
    fn undefined_map_target_reported() {
        let input = "map A = nonexistent\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("nonexistent")),
            "expected undefined map target error, got: {issues:?}",
        );
    }

    #[test]
    fn valid_document_has_no_issues() {
        let input = "\
glyph foo 2 1
..@@
map A = foo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.is_empty(),
            "expected no issues, got: {issues:?}",
        );
    }

    // `testdata/` declares a single consistent `font-meta` because it has to
    // stay a coherent project, so the broken variants are covered here.

    #[test]
    fn a_map_to_a_contentless_glyph_is_an_error() {
        // Neither a pixel grid nor a ref means the glyph never enters the
        // resolution cache, so it silently vanishes from the cmap. `advance`
        // does not make it buildable, but it does suppress the "has no
        // content" warning, so this used to pass without a single word.
        let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
map B = blank
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("'blank'")
                && i.message.contains("not built")),
            "mapping a contentless glyph must be an error, got: {issues:?}",
        );
    }

    #[test]
    fn a_ref_and_a_remap_to_a_contentless_glyph_are_errors() {
        let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
glyph host
ref blank
map A = vis
remap liga : vis -> blank
feature liga for DFLT : liga
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error && i.message.contains("'blank'"))
            .collect();
        assert!(
            errors.len() >= 2,
            "both the ref and the remap must be reported, got: {issues:?}",
        );
    }

    /// A glyph that is contentless but never used stays a warning — it builds
    /// nothing, but it also breaks nothing.
    #[test]
    fn an_unused_contentless_glyph_is_not_an_error() {
        let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
assume unused blank
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error),
            "an unused contentless glyph must not be an error, got: {issues:?}",
        );
    }

    /// Pattern glyphs already refuse to be empty, whatever the reason — a
    /// pixel grid cannot be shared across the expansions, so only `ref` lines
    /// can fill them.
    #[test]
    fn an_empty_pattern_glyph_is_an_error() {
        for body in ["", " advance 0"] {
            let input = format!(
                "\
name-parts $ab = a b

glyph pix 1 1
@@
glyph pat-($ab){body}
map A|B = pat-($ab)
"
            );
            let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
            let issues = collect_issues(&[&doc]);
            assert!(
                issues.iter().any(|i| i.severity == Severity::Error
                    && i.message.contains("defines no glyphs")),
                "an empty pattern glyph must be an error (body {body:?}), got: {issues:?}",
            );
        }
    }

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
            issues.iter().any(|i| i.severity == Severity::Error
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
            issues.iter().any(|i| i.severity == Severity::Error
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
            !issues.iter().any(|i| i.message.contains("no OpenType lookup type")),
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

glyph ok 2 1
@@..
glyph present-a 2 1
@@..
glyph present-b 2 1
..@@
map A = ok

remap liga : ok -> present-($ab)
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("remap")),
            "a remap whose expansions all exist must be quiet, got: {issues:?}",
        );
    }

    #[test]
    fn font_meta_ascent_plus_descent_must_equal_height() {
        let input = "font-meta height 16 ascent 12 descent 3\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("!= height")),
            "expected font-meta mismatch warning, got: {issues:?}",
        );
    }

    #[test]
    fn font_meta_zero_height_reported() {
        let input = "font-meta height 0 ascent 0 descent 0\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("font-meta height is 0")),
            "expected zero-height error, got: {issues:?}",
        );
    }

    #[test]
    fn duplicate_alternative_anchor_warns() {
        let input = "\
glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:a 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:b 2 2
@@@@
@@@@
anchor -join 0 0
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("same anchor dimensions")),
            "expected duplicate alternative anchor warning, got: {issues:?}",
        );
    }

    #[test]
    fn unused_glyph_reported() {
        let input = "\
glyph used 2 1
..@@
map A = used

glyph orphan 2 1
@@..
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("glyph 'orphan' is unused")),
            "expected unused glyph warning, got: {issues:?}",
        );
        assert!(
            !issues.iter().any(|i| i.message.contains("glyph 'used' is unused")),
            "mapped glyph should not be reported as unused",
        );
    }

    #[test]
    fn transitively_used_glyph_not_reported() {
        let input = "\
glyph base 2 1
..@@

glyph composite 2 1
@@..
ref base 0 0

map A = composite
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("is unused")),
            "transitively used glyph should not be unused: {issues:?}",
        );
    }

    #[test]
    fn mutually_referencing_cluster_reported() {
        let input = "\
glyph a 2 1
..@@
ref b 0 0

glyph b 2 1
@@..
ref a 0 0
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.message.contains("glyph 'a' is unused")),
            "mutual ref cluster should be unused: {issues:?}",
        );
        assert!(
            issues.iter().any(|i| i.message.contains("glyph 'b' is unused")),
            "mutual ref cluster should be unused: {issues:?}",
        );
    }

    #[test]
    fn remap_target_counts_as_used() {
        let input = "\
glyph base 2 1
..@@
map A = base

glyph alt 2 1
@@..

remap liga : base -> alt
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("glyph 'alt' is unused")),
            "remap target should count as used: {issues:?}",
        );
    }

    #[test]
    fn alternative_glyph_used_when_base_used() {
        let input = "\
glyph stem 2 1
..@@
map A = stem

glyph stem:wide 2 1
@@..
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("glyph 'stem:wide' is unused")),
            "alternative of used base should not be unused: {issues:?}",
        );
    }

    #[test]
    fn sticky_glyph_not_reported_unused() {
        let input = "glyph keep sticky advance 0\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("is unused")),
            "sticky glyph should not be unused: {issues:?}",
        );
    }

    #[test]
    fn assert_same_distinct_not_unrecognized() {
        let input = "\
glyph a 2 1
..@@
glyph b 2 1
@@..
map A = a
map B = b

assert same a b
assert distinct a b
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("unrecognized directive")),
            "assert same/distinct should not be flagged as unrecognized: {issues:?}",
        );
    }

    #[test]
    fn map_decomposed_without_decomposition_reported() {
        // 'A' is already in NFD, so `map A` cannot synthesize anything.
        let input = "\
glyph a 2 1
..@@
map U+0041 = a
map generate A
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("no canonical decomposition")),
            "expected no-decomposition error, got: {issues:?}",
        );
    }

    #[test]
    fn map_decomposed_with_unmapped_component_reported() {
        // 'Ä' decomposes to U+0041 U+0308; U+0308 is not mapped.
        let input = "\
glyph a 2 1
..@@
map A = a
map generate Ä
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("unmapped codepoint")
                && i.message.contains("U+0308")),
            "expected unmapped component error, got: {issues:?}",
        );
    }

    #[test]
    fn map_decomposed_fully_mapped_accepted() {
        let input = "\
glyph a 2 1
..@@
glyph dieresis 2 1
@@..
map A = a
map U+0308 = dieresis
map generate Ä
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("decomposition")
                || i.message.contains("unmapped codepoint")),
            "fully mapped decomposition should be accepted, got: {issues:?}",
        );
    }

    #[test]
    fn assume_unused_suppresses_warning() {
        let input = "\
glyph orphan 2 1
@@..

glyph other 2 1
..@@

assume unused orphan
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("glyph 'orphan' is unused")),
            "assume unused should suppress warning: {issues:?}",
        );
        assert!(
            issues.iter().any(|i| i.message.contains("glyph 'other' is unused")),
            "non-assumed glyph should still be reported: {issues:?}",
        );
        assert!(
            !issues.iter().any(|i| i.message.contains("unrecognized directive")),
            "assume unused should not be flagged as unrecognized: {issues:?}",
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
            issues.iter().all(|i| i.severity == Severity::Error
                && i.message.contains("ordering cycle")),
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
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("undefined group 'nope'")),
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
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("declared more than once")),
            "got: {issues:?}",
        );
    }

    #[test]
    fn remap_group_without_rules_reported() {
        let issues = group_issues("remap group lonely\n");
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("has no rules")),
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

}
