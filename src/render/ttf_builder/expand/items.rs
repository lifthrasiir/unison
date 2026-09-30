//! Expansion of glyph blocks and their items: one `glyph` block per binding of the name parts.

use super::*;

/// One `glyph` block for one binding of the name parts: the header expanded,
/// and the body substituted alongside it.
///
/// `round` is which match of the governing `exists` this is (`0` when none
/// governs it), and is what keeps a fault of the *line* — a pattern that
/// expands to no names at all — from being reported once per match.
fn expand_glyph_item(
    name: &GlyphName,
    body: &crate::document::GlyphBody,
    name_parts: &NamePartsMap,
    origin: ItemRef,
    round: usize,
    all_items: &mut Vec<ExpandedItem>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let name_str = substitute_name_parts(&name.display(), name_parts);
    if is_name_pattern(&name_str) {
        let subst_name = GlyphName(name_str);
        // The block as written, with every name in it substituted:
        // the expansion shares this body, so a grid, a box or a
        // flag written once holds for every glyph the pattern
        // declares, exactly as a non-pattern block's does for its
        // one glyph.
        //
        // An IDC line expands with the block exactly as the refs
        // do. What it *derives* does not: the split is solved per
        // glyph, below, from the boxes each expansion's own parts
        // declare, so one line can write a different layout for
        // every glyph it stands for.
        let mut subst_body = body.clone();
        for gref in &mut subst_body.refs {
            gref.name = substitute_name_parts(&gref.name, name_parts);
        }
        for (name, _) in subst_body.compose.iter_mut().flat_map(|c| c.parts_mut()) {
            *name = substitute_name_parts(name, name_parts);
        }
        match expand_glyph_block(&subst_name, &subst_body) {
            Ok(expanded) if expanded.is_empty() => {
                // The name expanded to nothing at all — an empty
                // alternation, or a reversed range, which expands
                // to no names rather than failing to parse. The
                // block declares no glyph, and every use of the
                // name it looks like it declares would otherwise
                // report as merely undefined.
                // Reported once for the line rather than once per
                // match: the pattern is the line's, so every match
                // fails it the same way.
                if round == 0 {
                    diagnostics.push(Diagnostic::error(
                        origin,
                        format!(
                            "glyph pattern '{}' expands to no names, so it \
                             declares no glyphs",
                            subst_name.display(),
                        ),
                    ));
                }
            }
            Ok(expanded) => {
                for item in expanded {
                    all_items.push(ExpandedItem {
                        item,
                        origin: Some(origin),
                    });
                }
            }
            Err(e) => {
                if round == 0 {
                    diagnostics.push(Diagnostic::error(origin, e));
                }
            }
        }
    } else {
        let mut body = body.clone();
        for gref in &mut body.refs {
            gref.name = substitute_name_parts(&gref.name, name_parts);
        }
        for (name, _) in body.compose.iter_mut().flat_map(|c| c.parts_mut()) {
            *name = substitute_name_parts(name, name_parts);
        }
        all_items.push(ExpandedItem {
            item: DocumentItem::Glyph {
                name: GlyphName(name_str),
                body,
            },
            origin: Some(origin),
        });
    }
}

/// One source item as `face` states it: nothing for an `exists` line or an
/// alias, once per match under a search, once per slice for a qualified line.
///
/// `rounds` is which matches of the search above a `glyph` block to expand,
/// so that one block can be several units of work. Every other item is one
/// unit, and is given `0..1`.
#[allow(clippy::too_many_arguments)]
pub(super) fn expand_item(
    item: &DocumentItem,
    origin: ItemRef,
    rounds: std::ops::Range<usize>,
    name_parts: &NamePartsMap,
    scoped: &crate::document::SliceNameParts,
    exists: &crate::exists::ExistsScopes,
    face: &crate::faces::Face,
    all_items: &mut Vec<ExpandedItem>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // An `exists` states a condition on the line below it and nothing
    // of its own.
    if exists.is_directive(origin) {
        return;
    }
    // The search above it, if there is one. Its slots are bound one
    // match at a time below, so from here on a scoped item expands
    // through exactly the machinery an unscoped one does.
    let scope = exists.scope(origin);
    if scope.is_some_and(|s| s.matches.is_empty()) {
        // The search found nothing, so the line below it stands for
        // nothing. Warned about at the `exists`, not here.
        return;
    }
    // A qualifier lists the slices the line is stated for, one at a
    // time; the face keeps the ones it includes. An unqualified line is
    // the base slice, which every face includes.
    let mut slices: Vec<Option<&str>> = match item.slice_qualifier() {
        [] => vec![None],
        qual => qual.iter().map(|s| Some(s.as_str())).collect(),
    };
    slices.retain(|s| face.includes(*s));
    if slices.is_empty() {
        return;
    }
    // An alias declares no glyph. It has already been folded into
    // `aliases`, and the references that named it are rewritten below.
    if matches!(item, DocumentItem::GlyphAlias { .. }) {
        return;
    }
    if let DocumentItem::Glyph { name, body } = item {
        // A scoped block runs once per match, each `$N` bound to one
        // string, so a group written beside a slot expands the way it
        // does on any other line; see [`crate::exists`]. The base is
        // cloned once for the run of matches and rebound per match.
        let mut per = scope.map(|_| name_parts.clone());
        for round in rounds {
            let name_parts = match (scope, &mut per) {
                (Some(scope), Some(per)) => {
                    scope.rebind(per, round);
                    &*per
                }
                _ => name_parts,
            };
            expand_glyph_item(
                name,
                body,
                name_parts,
                origin,
                round,
                all_items,
                diagnostics,
            );
        }
    } else if let (
        Some(scope),
        DocumentItem::Map {
            char_repr,
            selector,
            glyphs,
            ..
        },
    ) = (scope, item)
    {
        // A scoped `map` unrolls per match like everything else, but
        // it also *computes* its code point from the match, which is
        // the one thing a name pattern cannot carry: a `map` line is
        // unrolled per codepoint by `expand_map_pairs`, and there is no
        // pattern left there for the codepoint to come out of.
        for slice in &slices {
            let parts = scoped.for_slice(*slice);
            let one: Vec<String> = slice.iter().map(|s| s.to_string()).collect();
            // Cloned once for the line rather than once per match; see
            // [`crate::exists::Scope::rebind`].
            let mut per = parts.clone();
            for (i, caps) in scope.matches.iter().enumerate() {
                let (char_repr, selector) = match (
                    crate::exists::eval_codepoint(char_repr, caps),
                    selector
                        .as_deref()
                        .map(|s| crate::exists::eval_codepoint(s, caps))
                        .transpose(),
                ) {
                    (Ok(c), Ok(s)) => (c, s),
                    (Err(e), _) | (_, Err(e)) => {
                        // Reported once for the line rather than once
                        // per match: the spelling is the line's, so
                        // every match fails it the same way.
                        if i == 0 {
                            diagnostics.push(Diagnostic::error(origin, e));
                        }
                        continue;
                    }
                };
                scope.rebind(&mut per, i);
                all_items.push(ExpandedItem {
                    item: DocumentItem::Map {
                        slices: one.clone(),
                        comment: None,
                        char_repr,
                        selector,
                        glyphs: glyphs
                            .iter()
                            .map(|g| substitute_name_parts(g, &per))
                            .collect(),
                    },
                    origin: Some(origin),
                });
            }
        }
    } else {
        // Everything else is emitted once per slice it is stated for,
        // each with that slice's name parts. Downstream never sees a
        // multi-slice item: `slices` here is one slice or none.
        for slice in &slices {
            let parts = scoped.for_slice(*slice);
            let one: Vec<String> = slice.iter().map(|s| s.to_string()).collect();
            let item = match item {
                DocumentItem::Map {
                    char_repr,
                    selector,
                    glyphs,
                    ..
                } => DocumentItem::Map {
                    slices: one,
                    comment: None,
                    char_repr: char_repr.clone(),
                    selector: selector.clone(),
                    glyphs: glyphs
                        .iter()
                        .map(|g| substitute_name_parts(g, parts))
                        .collect(),
                },
                DocumentItem::MapDecomposed {
                    char_repr,
                    selector,
                    glyph,
                    ..
                } => DocumentItem::MapDecomposed {
                    slices: one,
                    comment: None,
                    char_repr: char_repr.clone(),
                    selector: selector.clone(),
                    glyph: glyph.as_ref().map(|g| substitute_name_parts(g, parts)),
                },
                DocumentItem::Feature { .. } | DocumentItem::FeatureAnchor { .. } => {
                    let mut item = item.clone();
                    match &mut item {
                        DocumentItem::Feature { slices, .. }
                        | DocumentItem::FeatureAnchor { slices, .. } => *slices = one,
                        _ => unreachable!(),
                    }
                    item
                }
                other => other.clone(),
            };
            all_items.push(ExpandedItem {
                item,
                origin: Some(origin),
            });
        }
    }
}
