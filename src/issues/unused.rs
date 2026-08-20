//! Reachability over the glyph graph: which glyphs and aliases no `map`,
//! `remap` or `ref` root can reach.

use std::collections::{HashMap, HashSet};

use crate::document::{
    Directive, DocumentItem, GlyphName, classify_directive, expand_name_element,
};

use super::{Cx, Issue, Severity, issue_at};

pub(super) fn check_unused_glyphs(
    cx: &Cx<'_>,
    mapped_glyphs: HashSet<String>,
    issues: &mut Vec<Issue>,
) {
    let docs = cx.docs;
    let name_parts = cx.name_parts;
    let _expansion = cx.expansion;
    let aliases = cx.aliases;
    let all_glyph_names = &cx.all_glyph_names;
    // Detect unused glyphs: glyphs not reachable from any map/remap root.
    // Works at glyph-item granularity to avoid expensive repeated pattern expansion.
    // Assign each glyph item an index; track which items are reachable.
    // name_to_item: expanded glyph name -> item index
    let mut name_to_item: HashMap<String, usize> = HashMap::new();
    // item_refs[i]: ref target names (expanded) for item i
    let mut item_refs: Vec<Vec<String>> = Vec::new();
    // item_location[i]: (doc_idx, item_idx, raw_name, is_alias) for reporting
    let mut item_location: Vec<(usize, usize, &str, bool)> = Vec::new();

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            // An alias is a node of this graph like any glyph: it is
            // reachable from what names it and it keeps its target
            // reachable in turn, so `map x = A` where `glyph A = B` leaves
            // neither the alias nor `B` looking unused.
            // A block an `exists` governs names `$N`, which is not a glyph
            // name until the search binds it — and if the search found
            // nothing, the block is not in the graph at all.
            let Some(name_parts) = cx
                .expansion
                .exists
                .parts_at(name_parts, crate::resolve::ItemRef::new(doc_idx, item_idx))
            else {
                continue;
            };
            let name_parts = name_parts.as_ref();
            let (name, refs, is_alias) = match item {
                DocumentItem::Glyph {
                    name: GlyphName(n),
                    body,
                } => {
                    let mut refs = Vec::new();
                    for gref in &body.refs {
                        refs.extend(expand_name_element(&gref.name, name_parts));
                    }
                    // An IDC component is a use of the glyph like a `ref`
                    // is; this pass reads the source rather than the
                    // expansion, so the line has to be walked here too or
                    // every part of every composed glyph reads as unused.
                    for part in body.compose.iter().flat_map(|c| c.part_names()) {
                        refs.extend(expand_name_element(part, name_parts));
                    }
                    (n, refs, false)
                }
                DocumentItem::GlyphAlias {
                    name: GlyphName(n),
                    target,
                    ..
                } => (n, expand_name_element(target, name_parts), true),
                _ => continue,
            };

            let idx = item_refs.len();
            item_location.push((doc_idx, item_idx, name, is_alias));
            for en in expand_name_element(name, name_parts) {
                name_to_item.entry(en).or_insert(idx);
            }
            item_refs.push(refs);
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
                DocumentItem::Glyph {
                    name: GlyphName(n),
                    body,
                } => {
                    if body.keep || body.mark {
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
    // Alias names belong here too: `glyph x:color = y:color` is what makes
    // the color/mono pair of `x` complete, so it is used by every use of
    // `x` — and it is absent from `all_glyph_names`, which holds glyphs.
    let mut alt_names: HashMap<&str, Vec<&str>> = HashMap::new();
    let alt_candidates = all_glyph_names
        .iter()
        .map(|n| n.as_str())
        .chain(aliases.decls().iter().map(|d| d.name.as_str()));
    for name in alt_candidates {
        if let Some(colon_pos) = name.find(':') {
            let base = &name[..colon_pos];
            alt_names.entry(base).or_default().push(name);
        }
    }

    // BFS over items.
    let mut reachable_items: Vec<bool> = vec![false; item_refs.len()];
    let mut queue: Vec<usize> = Vec::new();

    // Seed with root names.
    for name in &root_names {
        if let Some(&idx) = name_to_item.get(name.as_str())
            && !reachable_items[idx]
        {
            reachable_items[idx] = true;
            queue.push(idx);
        }
        // Alternatives of root names are also roots.
        if let Some(alts) = alt_names.get(name.as_str()) {
            for &alt in alts {
                if let Some(&idx) = name_to_item.get(alt)
                    && !reachable_items[idx]
                {
                    reachable_items[idx] = true;
                    queue.push(idx);
                }
            }
        }
    }

    while let Some(item_idx) = queue.pop() {
        for ref_name in &item_refs[item_idx] {
            if let Some(&target_item) = name_to_item.get(ref_name.as_str())
                && !reachable_items[target_item]
            {
                reachable_items[target_item] = true;
                queue.push(target_item);
            }
            // Alternatives of ref targets are also reachable.
            if let Some(alts) = alt_names.get(ref_name.as_str()) {
                for &alt in alts {
                    if let Some(&alt_item) = name_to_item.get(alt)
                        && !reachable_items[alt_item]
                    {
                        reachable_items[alt_item] = true;
                        queue.push(alt_item);
                    }
                }
            }
        }
    }

    // Report unreachable items.
    for (idx, &reached) in reachable_items.iter().enumerate() {
        if !reached {
            let (doc_idx, doc_item_idx, name, is_alias) = item_location[idx];
            let doc = docs[doc_idx];
            let what = if is_alias { "glyph alias" } else { "glyph" };
            issues.push(issue_at(
                doc,
                doc_item_idx,
                Severity::Warning,
                format!("{what} '{}' is unused", name,),
            ));
        }
    }
}
