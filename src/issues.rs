use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::document::{
    Document, DocumentItem, GlyphName, collect_name_parts, expand_name_pattern,
    is_name_pattern, substitute_name_parts,
};

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
    let mut issues = Vec::new();

    let name_parts = collect_name_parts(docs);

    let mut all_glyph_names: HashSet<String> = HashSet::new();
    let mut glyph_defs: HashMap<String, (PathBuf, usize)> = HashMap::new();
    let mut remap_groups: HashSet<String> = HashSet::new();

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
            let file_line = docline_to_file_line(doc, line);

            match item {
                DocumentItem::Glyph { name: GlyphName(n), body } => {
                    let name_str = substitute_name_parts(n, &name_parts);
                    if is_name_pattern(&name_str) {
                        match expand_name_pattern(&name_str) {
                            Ok(expanded) => {
                                for en in &expanded {
                                    if let Some((prev_file, prev_line)) =
                                        glyph_defs.get(en.as_str())
                                    {
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
                                        glyph_defs.insert(
                                            en.clone(),
                                            (doc.path.clone(), file_line),
                                        );
                                    }
                                    all_glyph_names.insert(en.clone());
                                }
                            }
                            Err(e) => {
                                issues.push(Issue {
                                    severity: Severity::Error,
                                    message: format!("name pattern error: {e}"),
                                    file: doc.path.clone(),
                                    line,
                                    file_line,
                                });
                            }
                        }
                    } else {
                        if let Some((prev_file, prev_line)) = glyph_defs.get(name_str.as_str())
                        {
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "duplicate glyph '{}' (first defined at {}:{})",
                                    name_str,
                                    short_path(prev_file),
                                    prev_line,
                                ),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
                        } else {
                            glyph_defs
                                .insert(name_str.clone(), (doc.path.clone(), file_line));
                        }
                        all_glyph_names.insert(name_str.clone());
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

    // font-meta validation
    {
        let mut height: Option<u16> = None;
        let mut ascent: Option<u16> = None;
        let mut descent: Option<u16> = None;
        let mut meta_file = PathBuf::new();
        let mut meta_line = 0usize;
        let mut meta_file_line = 0usize;
        for doc in docs {
            for (item_idx, item) in doc.items.iter().enumerate() {
                if let DocumentItem::FontMeta(s) = item {
                    let dl = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
                    meta_file = doc.path.clone();
                    meta_line = dl;
                    meta_file_line = docline_to_file_line(doc, dl);
                    for pair in s.split_whitespace().collect::<Vec<_>>().chunks(2) {
                        if pair.len() == 2 {
                            match pair[0] {
                                "height" => height = pair[1].parse().ok(),
                                "ascent" => ascent = pair[1].parse().ok(),
                                "descent" => descent = pair[1].parse().ok(),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        if let (Some(h), Some(a), Some(d)) = (height, ascent, descent) {
            if a + d != h {
                issues.push(Issue {
                    severity: Severity::Warning,
                    message: format!(
                        "font-meta ascent ({a}) + descent ({d}) != height ({h})",
                    ),
                    file: meta_file.clone(),
                    line: meta_line,
                    file_line: meta_file_line,
                });
            }
        }
        if height == Some(0) {
            issues.push(Issue {
                severity: Severity::Error,
                message: "font-meta height is 0".to_string(),
                file: meta_file,
                line: meta_line,
                file_line: meta_file_line,
            });
        }
    }

    let mut mapped_codepoints: HashMap<u32, (PathBuf, usize)> = HashMap::new();

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
            let file_line = docline_to_file_line(doc, line);

            match item {
                DocumentItem::Glyph { name: _, body } => {
                    for gref in &body.refs {
                        let ref_name = substitute_name_parts(&gref.name, &name_parts);
                        if is_name_pattern(&ref_name) {
                            if let Ok(expanded) = expand_name_pattern(&ref_name) {
                                for en in &expanded {
                                    if !all_glyph_names.contains(en.as_str()) {
                                        issues.push(Issue {
                                            severity: Severity::Error,
                                            message: format!(
                                                "unresolved ref '{}'",
                                                en,
                                            ),
                                            file: doc.path.clone(),
                                            line,
                                            file_line,
                                        });
                                        break;
                                    }
                                }
                            }
                        } else if !all_glyph_names.contains(ref_name.as_str()) {
                            issues.push(Issue {
                                severity: Severity::Error,
                                message: format!("unresolved ref '{}'", ref_name),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
                        }
                    }
                }
                DocumentItem::Map { char_repr, glyph } => {
                    let subst_glyph = substitute_name_parts(glyph, &name_parts);
                    let expanded_pairs =
                        crate::render::ttf_builder::expand_map_pairs(
                            char_repr, &subst_glyph,
                        );
                    if expanded_pairs.is_empty() {
                        issues.push(Issue {
                            severity: Severity::Error,
                            message: format!(
                                "map has no valid codepoints ('{}')",
                                char_repr,
                            ),
                            file: doc.path.clone(),
                            line,
                            file_line,
                        });
                    }
                    for (cp, target) in &expanded_pairs {
                        if !all_glyph_names.contains(target.as_str()) {
                            issues.push(Issue {
                                severity: Severity::Error,
                                message: format!(
                                    "map '{}' targets undefined glyph '{}'",
                                    char_repr, target,
                                ),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
                            break;
                        }
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
                DocumentItem::Remap {
                    source, target, lookbehind, lookahead, ..
                } => {
                    for name in std::iter::once(source.as_str())
                        .chain(std::iter::once(target.as_str()))
                        .chain(lookbehind.iter().map(|s| s.as_str()))
                        .chain(lookahead.iter().map(|s| s.as_str()))
                    {
                        let resolved = substitute_name_parts(name, &name_parts);
                        if is_name_pattern(&resolved) {
                            continue;
                        }
                        if !all_glyph_names.contains(resolved.as_str()) {
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "remap references undefined glyph '{}'",
                                    resolved,
                                ),
                                file: doc.path.clone(),
                                line,
                                file_line,
                            });
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
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with("exclude-from-sample ") {
                        issues.push(Issue {
                            severity: Severity::Warning,
                            message: format!("unrecognized directive '{}'", trimmed),
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

    // Detect alternative glyphs with ambiguous anchor matches.
    // For base "foo", if "foo" and "foo:bar" both have a `-name` anchor with
    // the same dimensions, warn that they are ambiguous (the first alphabetically wins).
    {
        let mut bases_to_alts: HashMap<String, Vec<(String, PathBuf, usize, usize)>> = HashMap::new();
        for doc in docs {
            for (item_idx, item) in doc.items.iter().enumerate() {
                if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
                    if body.points.iter().any(|p| p.position.starts_with('-')) {
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
                    let mut sorted_names: Vec<&str> = names.iter().copied().collect();
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
}
