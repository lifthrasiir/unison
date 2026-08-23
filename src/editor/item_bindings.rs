//! The names in force *inside one item*, as the view resolves them.
//!
//! A `ref` line can name something no line of its own says. A `$-N` names a
//! group of the `glyph` header above it ([`crate::pattern`]); a `$0`/`$N` names
//! what the `exists` search above the block matched ([`crate::exists`]). The
//! build substitutes both while expanding the block —
//! [`crate::document::expand_glyph_block`] and
//! [`crate::render::ttf_builder::expand`] — but the editor expands nothing: it
//! draws the block as written, so the lookup behind the grid overlay is handed
//! `part-($-1)` and resolves nothing at all.
//!
//! This is that substitution for the view. It is written as *bindings* rather
//! than as a substitution pass of its own because the lookup already runs one:
//! [`crate::ref_composite::resolve_ref_name_for_view`] substitutes the name
//! parts of a ref name and then takes the first expansion of the pattern that
//! is left. Binding `$-1` to the header's first group therefore draws the
//! block's *first expansion*, which is exactly what a pattern block with no
//! back-reference in it has always drawn — and one `$N` slot bound to one
//! string is the same shape [`crate::exists::Scope::rebind`] gives the build.
//!
//! The map is borrowed unchanged for every item that needs neither, so a file
//! with no patterns in it clones nothing.

use std::borrow::Cow;

use crate::document::{Document, DocumentItem, NamePartsMap};
use crate::exists::FirstMatches;
use crate::pattern::{capture_groups, mentions_back_reference, substitute_name_parts};

/// The name parts `base` holds, plus whatever item `idx` of `doc` binds on top
/// of them: `$0`…`$N` from the first match of the search scoping it, and `$-N`
/// for each group its own header wrote.
///
/// The search bindings go in first: a header is written `han-($1)` under an
/// `exists`, so the groups a `$-N` names are the ones left *after* the search
/// filled its slots in — the same order [`crate::document::expand_glyph_block_slots`]
/// reads them in.
pub(crate) fn item_bindings<'a>(
    doc: &Document,
    idx: usize,
    base: &'a NamePartsMap,
    exists: &FirstMatches,
) -> Cow<'a, NamePartsMap> {
    let DocumentItem::Glyph { name, body } = &doc.items[idx] else {
        return Cow::Borrowed(base);
    };
    let matched = exists.get(&doc.path, idx);
    let back_referenced = body
        .refs
        .iter()
        .map(|r| r.name.as_str())
        .chain(body.compose.iter().flat_map(|c| c.part_names()))
        .any(mentions_back_reference);
    if matched.is_none() && !back_referenced {
        return Cow::Borrowed(base);
    }
    let mut map = base.clone();
    if let Some(matched) = matched {
        for (slot, value) in matched.iter().enumerate() {
            map.insert(format!("${slot}"), vec![value.clone()]);
        }
    }
    if back_referenced {
        let name_str = substitute_name_parts(&name.display(), &map);
        for (i, group) in capture_groups(&name_str).into_iter().enumerate() {
            // An empty group stands for nothing and is left unbound, so the
            // `$-N` naming it stays verbatim rather than expanding to a name
            // with a hole in it — the rule `capture_value` states.
            if !group.is_empty() {
                map.insert(format!("$-{}", i + 1), group);
            }
        }
    }
    Cow::Owned(map)
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, DocumentItem, NamePartsMap};
    use crate::document_io::{derive_document, parse_doclines};
    use crate::editor::grid_render::build_composites;
    use crate::editor::ref_composite;

    fn fixture(source: &str) -> (Document, NamePartsMap) {
        let lines = parse_doclines(source);
        let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        let name_parts = crate::document::collect_name_parts(&[&doc]);
        (doc, name_parts)
    }

    fn glyph_idx(doc: &Document, want: &str) -> usize {
        doc.items
            .iter()
            .position(|i| matches!(i, DocumentItem::Glyph { name, .. } if name.display() == want))
            .unwrap_or_else(|| panic!("no glyph named {want}"))
    }

    /// A `ref` naming a group of its own block header draws the first
    /// expansion's part, the way a `ref` written as a plain pattern does.
    #[test]
    fn a_back_reference_ref_draws_on_the_grid() {
        let (doc, name_parts) = fixture(
            "glyph part-a 2 2\n\
             @@..\n\
             ..@@\n\
             \n\
             glyph part-b 2 2\n\
             ..@@\n\
             @@..\n\
             \n\
             glyph whole-(a|b)\n\
             ref part-($-1) 0 0\n",
        );
        let (named, alt_index) =
            ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        let composites = build_composites(
            &doc,
            &named,
            &name_parts,
            &alt_index,
            &Default::default(),
            &Default::default(),
        );
        let idx = glyph_idx(&doc, "whole-(a|b)");
        let comp = composites
            .get(&idx)
            .expect("a block whose only ref is a back-reference still draws it");
        assert_eq!(
            comp.layers.len(),
            1,
            "the back-reference resolved to nothing"
        );
    }

    /// The same for a block under an `exists`: `ref ($0)` names what the search
    /// matched, and the grid shows the first match.
    #[test]
    fn a_search_capture_ref_draws_on_the_grid() {
        let (doc, name_parts) = fixture(
            "glyph han-4e00:2x2 2 2\n\
             @@..\n\
             ..@@\n\
             \n\
             exists han-([0-9a-f]{4}):2x2\n\
             glyph han-($1) 2 2\n\
             ref ($0) 0 0\n",
        );
        let (scopes, _) = crate::exists::resolve_scopes(&[&doc], &name_parts);
        let first = crate::exists::FirstMatches::collect(&[&doc], &scopes);
        let (named, alt_index) =
            ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        let composites = build_composites(
            &doc,
            &named,
            &name_parts,
            &alt_index,
            &Default::default(),
            &first,
        );
        let idx = glyph_idx(&doc, "han-($1)");
        let comp = composites
            .get(&idx)
            .expect("a block under a search still draws its first match");
        assert_eq!(
            comp.layers.len(),
            1,
            "the search capture resolved to nothing"
        );
    }
}
