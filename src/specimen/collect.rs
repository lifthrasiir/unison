//! Step 1 of the specimen's three: what the source says — the cmap pairs, the variation
//! sequences, the remap-only glyphs, the `prop` lines and the blocks ([`SpecimenData`]).

use crate::hash::{HashMap, HashSet};
use std::collections::{BTreeMap, BTreeSet};

use crate::document::{
    Document, DocumentItem, NamePartsMap, expand_name_element, substitute_name_parts,
};
use crate::glyph_flags::GlyphFlags;
use crate::render::ttf_builder::{
    decomposed_map_pairs, expand_map_pairs_per_alternative, expand_uvs_map_triples,
};
use crate::resolve::ItemRef;
use crate::ucd::BlockMap;

use super::{RemapEntry, SpecimenData};

/// The alternative one cell takes, and whether it had to settle for a name the
/// font has no glyph for. `None` for a mapping that is not there at all.
///
/// `per_alt[k][i]` is alternative `k`'s expansion at position `i`; the
/// alternatives all expand over the same characters, so the position is the
/// character. A cell that matches nothing keeps the *first* name written, which
/// is the one the author most likely meant and so the one worth showing in the
/// status bar and jumping to on a click — unless the line ends in the empty
/// target, which says a character that matched nothing is simply not in the
/// font. Then there is no cell: the grid shows what the build produced, and the
/// build dropped the mapping.
pub(super) fn pick_target<T>(
    per_alt: &[Vec<T>],
    i: usize,
    name_of: impl Fn(&T) -> &String,
    usable: &impl Fn(&str) -> bool,
    optional: bool,
) -> Option<(String, bool)> {
    let mut first = None;
    for alt in per_alt {
        let Some(entry) = alt.get(i) else {
            continue;
        };
        let name = name_of(entry);
        if name.is_empty() {
            continue;
        }
        if usable(name) {
            return Some((name.clone(), false));
        }
        first.get_or_insert_with(|| name.clone());
    }
    if optional {
        return None;
    }
    Some((first.unwrap_or_default(), true))
}

impl SpecimenData {
    /// `exists` and `aliases` come from the caller because it already has them:
    /// the rebuild that collects this expanded the very same documents a moment
    /// earlier, and both cost as much as a fifth of an expansion to derive. A
    /// scope is keyed by *item index*, so they may only be handed in by a
    /// caller holding the same document set they were resolved over — which is
    /// the whole point of collecting this inside the rebuild rather than after
    /// it.
    #[expect(clippy::too_many_arguments)]
    pub fn collect(
        docs: &[&Document],
        name_parts: &NamePartsMap,
        exists: &crate::exists::ExistsScopes,
        aliases: &crate::alias::AliasMap,
        name_to_gid: &HashMap<String, u16>,
        face_id: Option<&str>,
        glyph_flags: &GlyphFlags,
        cancel: &crate::cancel::CancelToken,
    ) -> Self {
        // `cancel` stops the walk over the documents, which is nearly all of
        // this, and what comes back is then however far it got: the rebuild
        // that collects this drops a cancelled result whole (see
        // `crate::cancel`), and the next rebuild is waiting for it to return.
        let glyph_flags = glyph_flags.clone();
        let char_props = crate::ucd::CharProps::collect(docs);
        let blocks = BlockMap::collect(docs);

        // An `exists` above a line binds `$0`/`$N` over the names the source
        // declares and unrolls the line below it once per matched name. A
        // source that maps a few thousand han glyphs that way — `exists
        // han-([0-9a-f]{4,5})` / `map U+($1) = han-($1)` — has no literal `map`
        // line for any of them, so a pass that reads the items as written finds
        // no character at all. Hence `exists`, and hence `aliases` beside it:
        // `name_to_gid` comes from the built font, which knows a glyph only by
        // its canonical name, so a character mapped through an alias has to be
        // asked for under that name. Merged names count as aliases here for the
        // same reason they do in the expansion: the font carries one of them.

        // The specimen draws one face's font bytes, so it has to read the
        // source the way `expand_for` did when building them: a slice-qualified
        // line is stated once per slice the face includes, with *that slice's*
        // name parts in force. Substituting a `map wide|narrow : … = f($-half)`
        // with the unqualified parts leaves `$-half` in the glyph name, which
        // then names no glyph — no gid to draw, and nothing for a click to
        // link to.
        let faces = crate::faces::FaceSet::collect(docs);
        let face = face_id
            .and_then(|id| faces.faces.iter().find(|f| f.id == id))
            .unwrap_or_else(|| faces.primary());
        let scoped = crate::document::SliceNameParts::with_base(docs, name_parts.clone());

        // The oracle the ordered alternatives of a `map` line are asked
        // against, and the one the grid's error tint rests on: the *built
        // font*'s own glyph set. It answers the same question the build asked
        // (`resolve_map_alternatives`) without re-deriving it, and it is the
        // only honest answer for a cell — a name with no glyph id is a
        // character the font cannot draw, whatever the source says.
        //
        // A build that produced nothing at all leaves it empty; treating that
        // as "no glyph exists" would tint the entire grid red over a transient
        // state, so nothing is faulted until there is a font to fault against.
        let have_font = !name_to_gid.is_empty();
        let usable =
            |name: &str| !name.is_empty() && (!have_font || name_to_gid.contains_key(name));

        let mut map: BTreeMap<u32, (String, bool)> = BTreeMap::new();
        let mut uvs: BTreeMap<u32, BTreeMap<u32, (String, bool)>> = BTreeMap::new();
        // Only the names a `remap` targets are ever asked of `mapped_glyphs`
        // below, so only those are worth remembering: a `map` line with ordered
        // alternatives over a range names hundreds of thousands of glyphs, and
        // keeping every one of them was most of what this walk cost.
        let remap_targets: HashSet<String> = docs
            .iter()
            .flat_map(|d| &d.items)
            .filter_map(|item| match item {
                DocumentItem::Remap { target, .. } => Some(target),
                _ => None,
            })
            .flatten()
            .flat_map(|t| expand_name_element(t, name_parts))
            .collect();
        let mut mapped_glyphs: HashSet<String> = HashSet::default();
        for (doc_idx, doc) in docs.iter().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            for (item_idx, item) in doc.items.iter().enumerate() {
                let origin = ItemRef::new(doc_idx, item_idx);
                // The `exists` line itself states nothing of its own, and a
                // search that found nothing leaves the line below it standing
                // for nothing.
                if exists.is_directive(origin) {
                    continue;
                }
                // Only a `map` states a character. Asked before anything is
                // bound: every multi-alias is a scoped item, and binding the
                // name parts for thousands of them only to match nothing below
                // was most of what this walk cost.
                if !matches!(
                    item,
                    DocumentItem::Map { .. } | DocumentItem::MapDecomposed { .. }
                ) {
                    continue;
                }
                let scope = exists.scope(origin);
                if scope.is_some_and(|s| s.matches.is_empty()) {
                    continue;
                }
                let mut slices: Vec<Option<&str>> = match item.slice_qualifier() {
                    [] => vec![None],
                    qual => qual.iter().map(|s| Some(s.as_str())).collect(),
                };
                slices.retain(|s| face.includes(*s));
                for slice in slices {
                    let slice_parts = scoped.for_slice(slice);
                    // A scoped `map` unrolls to one mapping per match, with the
                    // code point *computed* from that match; an unscoped one is
                    // stated once, exactly as written. Same machinery as
                    // `expand_inner`.
                    // The base of the bindings is cloned once for the line,
                    // not once per match: a search over the han glyphs matches
                    // thousands of names, and the base is every `name-parts`
                    // the source declares.
                    let mut bound = scope.map(|_| slice_parts.clone());
                    for round in 0..scope.map_or(1, |s| s.matches.len()) {
                        let (parts, caps) = match (scope, &mut bound) {
                            (Some(scope), Some(bound)) => {
                                scope.rebind(bound, round);
                                (&*bound, Some(&scope.matches[round][..]))
                            }
                            _ => (slice_parts, None),
                        };
                        // A spelling `exists` cannot evaluate (`U+($9)` with no
                        // ninth group) fails every match the same way; it is
                        // reported by the build, and draws nothing here.
                        let evaluated = |spec: &str| match caps {
                            Some(caps) => crate::exists::eval_codepoint(spec, caps).ok(),
                            None => Some(spec.to_string()),
                        };
                        match item {
                            // A variation sequence gets a cell of its own, beside
                            // the base's. A malformed one (both halves varying, a
                            // selector that is not one) is left to `issues` to
                            // report and draws nothing here.
                            DocumentItem::Map {
                                char_repr,
                                selector: Some(selector),
                                glyphs,
                                ..
                            } => {
                                let (Some(char_repr), Some(selector)) =
                                    (evaluated(char_repr), evaluated(selector))
                                else {
                                    continue;
                                };
                                let per_alt: Vec<Vec<(u32, u32, String)>> = glyphs
                                    .iter()
                                    .map(|g| {
                                        let subst = substitute_name_parts(g, parts);
                                        let mut triples =
                                            expand_uvs_map_triples(&char_repr, &selector, &subst)
                                                .unwrap_or_default();
                                        for t in &mut triples {
                                            aliases.canonicalize(&mut t.2);
                                        }
                                        triples
                                    })
                                    .collect();
                                for t in per_alt.iter().flatten() {
                                    if remap_targets.contains(&t.2) {
                                        mapped_glyphs.insert(t.2.clone());
                                    }
                                }
                                let Some(first) = per_alt.first() else {
                                    continue;
                                };
                                let optional = glyphs.last().is_some_and(|g| g.is_empty());
                                for (i, &(base, sel, _)) in first.iter().enumerate() {
                                    let Some(target) =
                                        pick_target(&per_alt, i, |t| &t.2, &usable, optional)
                                    else {
                                        continue;
                                    };
                                    uvs.entry(base).or_default().entry(sel).or_insert(target);
                                }
                            }
                            DocumentItem::Map {
                                char_repr, glyphs, ..
                            } => {
                                let Some(char_repr) = evaluated(char_repr) else {
                                    continue;
                                };
                                // Expanded together rather than one alternative
                                // at a time: they range over the same
                                // characters, and a range line is thousands of
                                // them wide.
                                let substituted: Vec<String> = glyphs
                                    .iter()
                                    .map(|g| substitute_name_parts(g, parts))
                                    .collect();
                                let mut per_alt =
                                    expand_map_pairs_per_alternative(&char_repr, &substituted);
                                for alt in &mut per_alt {
                                    aliases.canonicalize_pairs(alt);
                                }
                                for p in per_alt.iter().flatten() {
                                    if remap_targets.contains(&p.1) {
                                        mapped_glyphs.insert(p.1.clone());
                                    }
                                }
                                let Some(first) = per_alt.first() else {
                                    continue;
                                };
                                let optional = glyphs.last().is_some_and(|g| g.is_empty());
                                for (i, &(cp, _)) in first.iter().enumerate() {
                                    if let std::collections::btree_map::Entry::Vacant(slot) =
                                        map.entry(cp)
                                        && let Some(target) =
                                            pick_target(&per_alt, i, |p| &p.1, &usable, optional)
                                    {
                                        slot.insert(target);
                                    }
                                }
                            }
                            DocumentItem::MapDecomposed {
                                char_repr, glyph, ..
                            } => {
                                let subst = glyph.as_ref().map(|g| substitute_name_parts(g, parts));
                                let Some(char_repr) = evaluated(char_repr) else {
                                    continue;
                                };
                                for (cp, name) in decomposed_map_pairs(&char_repr, subst.as_deref())
                                {
                                    if remap_targets.contains(&name) {
                                        mapped_glyphs.insert(name.clone());
                                    }
                                    let unresolved = !usable(&name);
                                    map.entry(cp).or_insert((name, unresolved));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        let declared = map;

        // Build reverse map: glyph_name → smallest codepoint.
        let mut glyph_to_cp: HashMap<&str, u32> = HashMap::default();
        for (cp, (glyph_name, _)) in &declared {
            glyph_to_cp.entry(glyph_name.as_str()).or_insert(*cp);
        }

        // Collect remap-only glyph names and their originating feature.
        let mut remap_only: BTreeSet<String> = BTreeSet::new();

        // Context-free remap rules (no lookbehind/lookahead) are eligible
        // for codepoint-sequence labels.
        struct RemapRule {
            source: Vec<String>,
            target: Vec<String>,
        }
        let mut ligature_rules: Vec<RemapRule> = Vec::new();
        // feature name for each remap-only glyph (first seen wins).
        let mut glyph_feature: HashMap<String, String> = HashMap::default();

        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Remap {
                    feature,
                    source,
                    target,
                    lookbehind,
                    lookahead,
                    ..
                } = item
                {
                    // `remap` takes no slice qualifier, so the unqualified name
                    // parts are the only ones in force here.
                    let tgt_expanded: Vec<Vec<String>> = target
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();
                    let src_expanded: Vec<Vec<String>> = source
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();

                    let max_len = src_expanded
                        .iter()
                        .chain(tgt_expanded.iter())
                        .map(|v| v.len())
                        .max()
                        .unwrap_or(1);

                    let has_context = !lookbehind.is_empty() || !lookahead.is_empty();

                    for i in 0..max_len {
                        let tgt: Vec<String> = tgt_expanded
                            .iter()
                            .map(|v| v[i % v.len()].clone())
                            .collect();
                        for name in &tgt {
                            if !mapped_glyphs.contains(name) {
                                remap_only.insert(name.clone());
                                glyph_feature
                                    .entry(name.clone())
                                    .or_insert_with(|| feature.clone());
                            }
                        }
                        if !has_context {
                            let src: Vec<String> = src_expanded
                                .iter()
                                .map(|v| v[i % v.len()].clone())
                                .collect();
                            ligature_rules.push(RemapRule {
                                source: src,
                                target: tgt,
                            });
                        }
                    }
                }
            }
        }

        // Build remap entries, trying to compute codepoint sequences.
        let mut with_cp: Vec<RemapEntry> = Vec::new();
        let mut without_cp: Vec<RemapEntry> = Vec::new();

        for glyph_name in remap_only {
            let Some(&gid) = name_to_gid.get(&glyph_name) else {
                continue;
            };
            let feature = glyph_feature.get(&glyph_name).cloned().unwrap_or_default();

            // Find a context-free remap rule where this glyph appears in
            // the target and all source glyphs have direct cmap mappings.
            let cp_seq = ligature_rules.iter().find_map(|rule| {
                if !rule.target.contains(&glyph_name) {
                    return None;
                }
                rule.source
                    .iter()
                    .map(|s| glyph_to_cp.get(s.as_str()).copied())
                    .collect::<Option<Vec<u32>>>()
            });

            let label = if let Some(ref cps) = cp_seq {
                let hex = cps
                    .iter()
                    .map(|cp| format!("{cp:04X}"))
                    .collect::<Vec<_>>()
                    .join("+");
                format!("{hex} ({glyph_name})")
            } else {
                glyph_name.clone()
            };

            let entry = RemapEntry {
                label,
                glyph_name,
                feature,
                gid,
                cp_sequence: cp_seq.clone(),
            };
            if cp_seq.is_some() {
                with_cp.push(entry);
            } else {
                without_cp.push(entry);
            }
        }

        // Sort ligature remaps by codepoint sequence, then append others
        // (already sorted by glyph name via BTreeSet).
        with_cp.sort_by(|a, b| a.cp_sequence.cmp(&b.cp_sequence));
        let mut remap_entries = with_cp;
        remap_entries.append(&mut without_cp);

        Self {
            declared,
            uvs,
            remap_entries,
            blocks,
            char_props,
            glyph_flags,
        }
    }
}
