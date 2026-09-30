//! `map-decomposed` directives turned into synthesized composite glyphs and `map` entries.

use super::*;

/// A `map decomposed` directive waiting to be expanded, lifted out of
/// `all_items` so the expansion can push back into it.
struct PendingDecomposition {
    slices: Vec<String>,
    char_repr: String,
    /// Always invalid, and carried this far only so the rejection can name what
    /// was written. See [`DocumentItem::MapDecomposed`].
    selector: Option<String>,
    glyph: Option<String>,
    origin: Option<ItemRef>,
}

/// Turn `map <decomposable codepoint>` into a synthesized composite glyph plus
/// a plain `map` to it, reporting the characters that cannot be synthesized.
pub(super) fn expand_decomposed_maps(
    all_items: &mut Vec<ExpandedItem>,
    cp_to_glyph: &HashMap<u32, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use unicode_normalization::UnicodeNormalization;

    let mut decomposed_items: Vec<ExpandedItem> = Vec::new();
    let pending: Vec<PendingDecomposition> = all_items
        .iter()
        .filter_map(|e| match &e.item {
            DocumentItem::MapDecomposed {
                slices,
                char_repr,
                selector,
                glyph,
                ..
            } => Some(PendingDecomposition {
                slices: slices.clone(),
                char_repr: char_repr.clone(),
                selector: selector.clone(),
                glyph: glyph.clone(),
                origin: e.origin,
            }),
            _ => None,
        })
        .collect();

    for PendingDecomposition {
        slices,
        char_repr,
        selector,
        glyph,
        origin,
    } in pending
    {
        // A variation sequence is its own canonical decomposition, so there is
        // nothing here to synthesize from. Rejected before the pairs are even
        // expanded, so the message names the sequence rather than complaining
        // about the base character alone.
        if let Some(sel) = &selector {
            diagnostics.push(Diagnostic::error(
                origin,
                format!(
                    "map generate takes a single character, not the variation \
                     sequence '{char_repr} {sel}'; a variation sequence has no \
                     canonical decomposition — write `map {char_repr} {sel} = GLYPH`",
                ),
            ));
            continue;
        }

        // The synthesized composite's *name* is not canonicalized — it is a
        // glyph this expansion is about to define, not a reference to one.
        let pairs = decomposed_map_pairs(&char_repr, glyph.as_deref());
        if pairs.is_empty() {
            diagnostics.push(Diagnostic::error(
                origin,
                format!("map has no valid codepoints ('{char_repr}')"),
            ));
            continue;
        }

        for (cp, composite_name) in pairs {
            let Some(ch) = char::from_u32(cp) else {
                diagnostics.push(Diagnostic::error(
                    origin,
                    format!("map 'U+{cp:04X}' is not a valid character"),
                ));
                continue;
            };

            let nfd: Vec<char> = ch.nfd().collect();
            if nfd.len() == 1 && nfd[0] == ch {
                diagnostics.push(Diagnostic::error(
                    origin,
                    format!(
                        "map '{ch}' (U+{cp:04X}) has no canonical decomposition; \
                         use `map {ch} = GLYPH` instead",
                    ),
                ));
                continue;
            }

            let missing: Vec<String> = nfd
                .iter()
                .filter(|c| !cp_to_glyph.contains_key(&(**c as u32)))
                .map(|c| format!("U+{:04X}", *c as u32))
                .collect();
            if !missing.is_empty() {
                diagnostics.push(Diagnostic::error(
                    origin,
                    format!(
                        "map '{}' (U+{:04X}) decomposes to unmapped codepoint{} {}",
                        ch,
                        cp,
                        if missing.len() == 1 { "" } else { "s" },
                        missing.join(", "),
                    ),
                ));
                continue;
            }

            let refs: Vec<GlyphRef> = nfd
                .iter()
                .map(|c| GlyphRef {
                    raw_name: None,
                    comment: None,
                    name: cp_to_glyph[&(*c as u32)].clone(),
                    offset: None,
                    negated: false,
                    // A generated composite stands in for its decomposition, so
                    // forwarding the components' surviving anchors is the right
                    // default — a hand-written replacement decides per ref.
                    inherit: true,
                    goto: false,
                    fill: None,
                    visibility: None,
                })
                .collect();

            decomposed_items.push(ExpandedItem {
                item: DocumentItem::Glyph {
                    name: GlyphName(composite_name.clone()),
                    body: GlyphBody {
                        refs,
                        ..GlyphBody::new()
                    },
                },
                origin,
            });
            decomposed_items.push(ExpandedItem {
                item: DocumentItem::Map {
                    slices: slices.clone(),
                    comment: None,
                    char_repr: format!("U+{cp:04X}"),
                    selector: None,
                    glyphs: vec![composite_name],
                },
                origin,
            });
        }
    }

    all_items.retain(|e| !matches!(e.item, DocumentItem::MapDecomposed { .. }));
    all_items.extend(decomposed_items);
}
