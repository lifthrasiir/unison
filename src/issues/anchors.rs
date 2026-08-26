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
        &crate::document::collect_anchor_aligns(expansion.items()),
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

/// A centred anchor class whose two sides cannot meet on the pixel grid.
///
/// Centring reduces each side's range to its middle, so the offset the shaper
/// computes is half the difference of the two sizes. That lands on a whole
/// pixel only when the sizes share a parity; a 3-wide mark centred in a 4-wide
/// slot wants half a pixel, which the bitmap face cannot draw and the vector
/// face draws off the grid.
///
/// Reported per *class*, against the sizes it declares rather than against
/// every base-and-mark pair: the pairs are the product of two glyph sets and
/// would bury the one thing worth saying, which is that a size was picked with
/// the wrong parity. `align c` on one axis only checks that axis.
pub(super) fn check_centred_anchor_parity(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    use crate::document::{Align1, AnchorAlign};

    // The classes that centre, and on which axis.
    let mut centred: HashMap<&str, AnchorAlign> = HashMap::new();
    for doc in cx.docs {
        for item in &doc.items {
            if let DocumentItem::FeatureAnchor { anchor, align, .. } = item
                && (align.horizontal == Align1::Center || align.vertical == Align1::Center)
            {
                centred.insert(anchor.as_str(), *align);
            }
        }
    }
    if centred.is_empty() {
        return;
    }

    // Per class and side, the sizes declared and one place each was written.
    // `(anchor, is_plus)` → `(width, height)` → first site.
    type Site = (PathBuf, usize, usize, String);
    let mut sizes: HashMap<(&str, bool), HashMap<(u16, u16), Site>> = HashMap::new();
    for doc in cx.docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Glyph {
                name: GlyphName(n),
                body,
            } = item
            else {
                continue;
            };
            for pt in &body.points {
                let Some((sign, name)) = pt.position.split_at_checked(1) else {
                    continue;
                };
                let is_plus = match sign {
                    "+" => true,
                    "-" => false,
                    _ => continue,
                };
                if !centred.contains_key(name) {
                    continue;
                }
                let (line, file_line) = doc.item_lines(item_idx);
                sizes
                    .entry((centred.get_key_value(name).expect("just checked").0, is_plus))
                    .or_default()
                    .entry((pt.width(), pt.height()))
                    .or_insert((
                        doc.path.clone(),
                        line,
                        file_line,
                        substitute_name_parts(n, cx.name_parts),
                    ));
            }
        }
    }

    let mut anchors: Vec<&str> = centred.keys().copied().collect();
    anchors.sort_unstable();
    for anchor in anchors {
        let align = centred[anchor];
        let (Some(plus), Some(minus)) = (
            sizes.get(&(anchor, true)).cloned(),
            sizes.get(&(anchor, false)),
        ) else {
            continue;
        };
        let mut plus_sizes: Vec<_> = plus.iter().collect();
        plus_sizes.sort_by_key(|(size, _)| **size);
        let mut minus_sizes: Vec<_> = minus.iter().collect();
        minus_sizes.sort_by_key(|(size, _)| **size);

        for (plus_size, _) in &plus_sizes {
            let (pw, ph) = **plus_size;
            for (minus_size, site) in &minus_sizes {
                let (mw, mh) = **minus_size;
                let (file, line, file_line, mark) = site;
                let axis = if align.horizontal == Align1::Center && (pw + mw) % 2 == 1 {
                    Some(("width", pw, mw))
                } else if align.vertical == Align1::Center && (ph + mh) % 2 == 1 {
                    Some(("height", ph, mh))
                } else {
                    None
                };
                let Some((axis, slot, mark_size)) = axis else {
                    continue;
                };
                issues.push(Issue {
                    severity: Severity::Warning,
                    glyph: None,
                    message: format!(
                        "anchor '{anchor}' is `align {}`, but a {axis}-{mark_size} `-{anchor}` \
                         (on '{mark}') centred in a {axis}-{slot} `+{anchor}` lands half a pixel \
                         off; give the two the same parity",
                        align.to_token().unwrap_or_else(|| "ul".to_string()),
                    ),
                    file: file.clone(),
                    line: *line,
                    file_line: *file_line,
                });
            }
        }
    }
}
