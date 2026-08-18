//! Anchor problems: alternatives whose anchors cannot be told apart, and
//! the derivations that resolve to nothing.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{DocumentItem, GlyphName, substitute_name_parts};
use crate::resolve::Diagnostic;

use super::{Cx, Issue, Severity};

pub(super) fn check_ambiguous_anchors(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let name_parts = cx.name_parts;
    // Detect alternative glyphs with ambiguous anchor matches.
    // For base "foo", if "foo" and "foo:bar" both have a `-name` anchor with
    // the same dimensions, warn that they are ambiguous (the first alphabetically wins).
    let mut bases_to_alts: HashMap<String, Vec<(String, PathBuf, usize, usize)>> = HashMap::new();
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            if let DocumentItem::Glyph {
                name: GlyphName(n),
                body,
            } = item
                && body.points.iter().any(|p| p.position.starts_with('-'))
            {
                let resolved_name = substitute_name_parts(n, name_parts);
                let (line, file_line) = doc.item_lines(item_idx);
                // Find all base prefixes (foo:bar:quux is alt for "foo" and "foo:bar")
                for prefix in crate::ref_composite::alternative_prefixes(&resolved_name) {
                    bases_to_alts.entry(prefix.to_string()).or_default().push((
                        resolved_name.clone(),
                        doc.path.clone(),
                        line,
                        file_line,
                    ));
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
            if let DocumentItem::Glyph {
                name: GlyphName(n),
                body,
            } = item
            {
                let resolved_name = substitute_name_parts(n, name_parts);
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
                    if let Some((_, file, line, file_line)) =
                        alts.iter().find(|(n, _, _, _)| n == dup)
                    {
                        issues.push(Issue {
                                severity: Severity::Warning,
                                glyph: None,
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

pub(super) fn check_anchor_derivation(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docset = &cx.docset;
    let expansion = cx.expansion;
    let _resolution = cx.resolution;
    // Anchor derivation problems: a composite that would expose the same
    // anchor name from more than one source, and a `-` anchor with more than
    // one `+` candidate to attach to. This runs an anchors-only pass through
    // the same shared driver and the same derivation the font build uses
    // (`glyph_cache`/`derive_ref_offsets_with`), so what is reported here is
    // exactly what resolution dropped.
    struct AnchorsOnly {
        anchors: Vec<crate::document::GlyphPoint>,
        w: u16,
        h: u16,
    }
    impl AnchorsOnly {
        fn new() -> Self {
            Self {
                anchors: Vec::new(),
                w: 0,
                h: 0,
            }
        }
    }
    impl crate::render::glyph_cache::CachedGlyphEntry for AnchorsOnly {
        fn anchors(&self) -> &[crate::document::GlyphPoint] {
            &self.anchors
        }
        fn declared_origin(&self) -> (i16, i16) {
            (0, 0)
        }
        fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
            (&mut self.w, &mut self.h)
        }
        fn set_resolution(
            &mut self,
            anchors: Vec<crate::document::GlyphPoint>,
            _scale: u8,
            _origin: (i16, i16),
        ) {
            self.anchors = anchors;
        }
    }

    let mut declared_anchors: HashMap<&str, &[crate::document::GlyphPoint]> = HashMap::new();
    let mut origin_of: HashMap<&str, Option<crate::resolve::ItemRef>> = HashMap::new();
    for e in &expansion.items {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = &e.item
        {
            declared_anchors.entry(n).or_insert(&body.points);
            origin_of.entry(n).or_insert(e.origin);
        }
    }

    let (mut cache, pending) = crate::render::glyph_cache::seed_cache(
        expansion.items(),
        |_, _| AnchorsOnly::new(),
        AnchorsOnly::new,
        &crate::cancel::CancelToken::never(),
    );
    let mut derive_issues: Vec<(String, crate::ref_composite::DeriveIssue)> = Vec::new();
    crate::render::glyph_cache::resolve_pending(
        &mut cache,
        pending,
        |name| declared_anchors.get(name).map(|pts| pts.to_vec()),
        &mut crate::render::glyph_cache::FnBuilder(|_: &_, _: &_, _: &_| AnchorsOnly::new()),
        |name, issue| derive_issues.push((name.to_string(), issue)),
        &crate::cancel::CancelToken::never(),
    );
    // Every derive issue is an error: each one means an anchor derived to
    // nothing, and the glyph it was reported for is dropped from the build
    // (`glyph_cache::resolve_pending`) rather than shipped mis-composed.
    for (name, issue) in derive_issues {
        // Named, not just located: an anchor derives against whatever the
        // refs resolve to, which differs between the expansions of one
        // pattern line the same way a missing ref does.
        issues.push(
            docset.to_issue(
                &Diagnostic::error(
                    origin_of.get(name.as_str()).copied().flatten(),
                    issue.message(&name),
                )
                .about(&name),
            ),
        );
    }
}
