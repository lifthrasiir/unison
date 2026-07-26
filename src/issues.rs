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

fn docline_to_file_line(doc: &Document, docline_idx: usize) -> usize {
    doc.docline_file_lines.get(docline_idx).copied().unwrap_or(docline_idx) + 1
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
    let mut remap_groups: HashSet<String> = HashSet::new();

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
            let file_line = docline_to_file_line(doc, line);

            match item {
                DocumentItem::Glyph { name: GlyphName(n), body } => {
                    for bad in find_invalid_inline_ranges(n) {
                        issues.push(Issue {
                            severity: Severity::Error,
                            message: format!(
                                "invalid inline range '{}' (end < start or too large)",
                                bad,
                            ),
                            file: doc.path.clone(),
                            line,
                            file_line,
                        });
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
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "duplicate glyph '{}' (first defined at {}:{})",
                                    en,
                                    short_path(prev_file),
                                    prev_line,
                                ),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
                        } else {
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
                        issues.push(Issue {
                            severity: Severity::Warning,
                            message: format!("glyph '{}' has no content", n),
                            file: doc.path.clone(),
                            line,
                            file_line,
                        });
                    }
                }
                DocumentItem::Remap { feature, .. } => {
                    remap_groups.insert(feature.clone());
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
            let line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
            let file_line = docline_to_file_line(doc, line);

            match item {
                // Unresolvable refs, map targets and remap operands are all
                // reported by the resolution pass above.
                DocumentItem::Map { char_repr, glyph } => {
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
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "duplicate codepoint mapping U+{:04X} (first at {}:{})",
                                    cp,
                                    short_path(prev_file),
                                    prev_line,
                                ),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
                        } else {
                            mapped_codepoints
                                .insert(*cp, (doc.path.clone(), file_line));
                        }
                    }
                }
                DocumentItem::Feature {
                    remap_group, ..
                } => {
                    if !remap_groups.contains(remap_group.as_str()) {
                        issues.push(Issue {
                            severity: Severity::Error,
                            message: format!(
                                "feature references undefined remap group '{}'",
                                remap_group,
                            ),
                            file: doc.path.clone(),
                            line,
                            file_line,
                        });
                    }
                }
                DocumentItem::NameParts { values, .. } => {
                    for val in values {
                        if val.starts_with('$') && !name_parts.contains_key(val.as_str()) {
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "undefined name-parts reference '{}'",
                                    val,
                                ),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
                        }
                    }
                }
                DocumentItem::Directive(text) => {
                    if classify_directive(text) == Directive::Unrecognized {
                        issues.push(Issue {
                            severity: Severity::Warning,
                            message: format!("unrecognized directive '{}'", text.trim()),
                            file: doc.path.clone(),
                            line,
                            file_line,
                        });
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
                let line = doc.item_line_starts.get(doc_item_idx).copied().unwrap_or(0);
                let file_line = docline_to_file_line(doc, line);
                issues.push(Issue {
                    severity: Severity::Warning,
                    message: format!("glyph '{}' is unused", name),
                    file: doc.path.clone(),
                    line,
                    file_line,
                });
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
                        let line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
                        let file_line = docline_to_file_line(doc, line);
                        // Find all base prefixes (foo:bar:quux is alt for "foo" and "foo:bar")
                        let mut prefix = resolved_name.as_str();
                        while let Some(colon_pos) = prefix.rfind(':') {
                            prefix = &prefix[..colon_pos];
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
map A
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
map Ä
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
map Ä
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

}
