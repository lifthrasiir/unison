//! Implicit merging: the names one `glyph` pattern block declares that turn
//! out to describe the same glyph.
//!
//! `glyph han-6cb3-(g|j|k):15x16` followed by a grid declares three names for
//! one drawing. Nothing about that font wants three glyph ids, and writing
//! `glyph han-6cb3-gjk` plus three `glyph … = …` lines by hand is not an
//! option at twenty thousand characters. So the expansions of one pattern
//! block that provably describe the same glyph are folded into **one glyph
//! with several names** — which is exactly what [`crate::alias`] already is,
//! and this module produces its input rather than a mechanism of its own.
//! Every consumer that already canonicalizes an alias therefore needs no
//! change at all.
//!
//! # What is compared
//!
//! Only the expansions of **one block** are ever candidates. Two blocks that
//! happen to draw the same shape are left alone: measured over `font/` as it
//! stands, merging those saves 49 glyphs out of 8,719 while landing squarely
//! on `remap` operands (`hangul-oa` vs `hangul-med-oa-nof`, `zwsp`/`zwj`), and
//! a rule keyed on a glyph id is the one thing merging can change. The gain
//! from a pattern block is the opposite: every han character written with
//! per-region names is a duplicate.
//!
//! Within one block the comparison is cheap, because
//! [`expand_glyph_block`](crate::document::expand_glyph_block) rewrites only
//! two things per expansion — `ref` names and IDC component names. The grid,
//! the box, the flags, the anchors and the gaps are shared verbatim. So two
//! expansions are the same glyph exactly when each of those name *slots* names
//! the same glyph in both, and [`expand_glyph_block_slots`] is the reading of
//! a block that says nothing else.
//!
//! # The fixpoint
//!
//! A slot may name another block's expansion, which may itself be merged —
//! `glyph b-(g|j|k)` with `ref a-(g|j|k)` is one glyph exactly when the three
//! `a-*` are. So the comparison is over the merge relation σ itself, computed
//! as a least fixpoint: start from the declared aliases, and repeatedly group
//! each block's expansions by their slots mapped through σ. Merges are only
//! ever added, never taken back, so this terminates, and it needs no
//! topological order over the `ref` graph — a chain of depth *n* settles in
//! *n* rounds, and a reference cycle (already an error) merely stops merging
//! rather than spinning.
//!
//! The representative of a group is its **first** expansion, which is the
//! name the font keeps: for `(g|j|k)` merged whole that is the `-g` form, and
//! for `-k` drawn differently the `{g,j}` group keeps `-g` while `-k` stays a
//! glyph of its own.
//!
//! # Why this is sound, including `ifexists`
//!
//! σ-equal names denote one glyph, by induction over the `ref` graph: the
//! bodies agree by construction, and each slot's target is one glyph by the
//! hypothesis. Nothing has to be resolved, traced or measured.
//!
//! `ifexists` needs no rule of its own, which is worth stating because it
//! looks like it should. A line whose component nothing defines stands for
//! something else entirely ([`crate::compose::stands_for_nothing`]), so a
//! merge that ignored existence would be wrong — but σ only ever relates names
//! a `glyph` block declares, so a name that does not exist is σ-equal to
//! nothing but itself, and two expansions that differ in whether a slot exists
//! never compare equal. The cost is one deliberate incompleteness: two
//! expansions whose `ifexists` lines both stand for nothing, by *different*
//! missing names, are not merged.
//!
//! # What a `remap` takes out of it
//!
//! Merging two glyphs is invisible to everything that draws, and to the cmap,
//! which maps many characters to one glyph as a matter of course. The one
//! thing it can change is a rule *keyed on a glyph id*: a lookup that matches
//! `hangul-med-comb--f` would, once that name and `hangul-med-comb--nof` were
//! one glyph, match both — and the two are drawn identically on purpose, the
//! difference between them being what the shaping does with them, not what
//! they look like.
//!
//! So a name any `remap` matches on — a source, a lookbehind or a lookahead,
//! anywhere in the sources, whatever slice the rule is stated for — is never
//! merged. A name a rule only *produces* is left alone by this: substituting
//! two identical glyphs is substituting one, and nothing downstream can tell
//! the outputs apart that could not tell the glyphs apart. That split is what
//! keeps the case this exists for — per-region han forms selected by `locl`,
//! which are targets — mergeable, while the hangul jamo above are not.
//!
//! Which rules name a glyph is read from the documents whole, with no regard
//! for the slices a rule is stated for. A face-dependent answer would give
//! two faces two different glyph orders, and a collection's faces share one
//! glyph store (`faces.rs`).
//!
//! # `keep` is the opt-out
//!
//! A block flagged `keep` declares a glyph per name. `keep` already says the
//! glyph is wanted whether or not anything reaches it — that it exists in its
//! own right — and this is the other half of the same statement. It is the one
//! escape hatch, and it is stated per block, which is where the pattern that
//! would merge is written.

use std::collections::{HashMap, HashSet};

use crate::alias::AliasMap;
use crate::document::{
    Document, DocumentItem, NamePartsMap, expand_glyph_block_slots, is_name_pattern,
    substitute_name_parts,
};
use crate::pattern::expand_name_element;

/// One pattern block, as the fixpoint reads it: the names it declares and, per
/// name, what its `ref`/IDC slots expand to.
struct Block {
    members: Vec<String>,
    slots: Vec<Vec<String>>,
}

/// The implicit merges `docs` calls for, as `(name, the glyph it is a name
/// for)` pairs — the same shape a declared alias has.
///
/// `aliases` is the declared aliases, which the fixpoint starts from: a slot
/// naming `x` where `glyph x = y` names `y`, and merging has to see that.
pub fn implicit_merges(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    aliases: &AliasMap,
    exists: &crate::exists::ExistsScopes,
) -> Vec<(String, String)> {
    let blocks = collect_blocks(docs, name_parts, exists);
    if blocks.is_empty() {
        return Vec::new();
    }
    let matched = remap_inputs(docs, name_parts, aliases);

    // Name → the glyph it has been merged into. Values are never keys: a
    // representative is canonicalized before it is stored, so `canon` is one
    // lookup and not a walk.
    let mut merged: HashMap<String, String> = HashMap::new();

    loop {
        let mut changed = false;
        for block in &blocks {
            // The keys of a whole block first, so `merged` is read through
            // once and written after — one round sees one state of σ.
            let keys: Vec<Vec<&str>> = block
                .slots
                .iter()
                .map(|slots| {
                    slots
                        .iter()
                        .map(|n| canon(n, &merged, aliases))
                        .collect::<Vec<&str>>()
                })
                .collect();

            let mut first_with_key: HashMap<&[&str], usize> = HashMap::new();
            let mut new_merges: Vec<(String, String)> = Vec::new();
            for (i, key) in keys.iter().enumerate() {
                // A glyph some rule matches on keeps its own id, and cannot
                // stand for another glyph either.
                if matched.contains(block.members[i].as_str()) {
                    continue;
                }
                match first_with_key.get(key.as_slice()) {
                    None => {
                        first_with_key.insert(key.as_slice(), i);
                    }
                    Some(&first) => {
                        let name = &block.members[i];
                        let rep = canon(&block.members[first], &merged, aliases).to_string();
                        // A name that already stands for the representative —
                        // this round or an earlier one — is not a change, and
                        // saying so is what ends the loop.
                        if *name != rep && canon(name, &merged, aliases) != rep {
                            new_merges.push((name.clone(), rep));
                        }
                    }
                }
            }
            for (name, rep) in new_merges {
                merged.insert(name, rep);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut out: Vec<(String, String)> = merged.into_iter().collect();
    out.sort();
    out
}

/// The glyph `name` stands for under the merges found so far and the declared
/// aliases — the declared alias first, since that is the name a block's slot
/// was written with.
fn canon<'a>(name: &'a str, merged: &'a HashMap<String, String>, aliases: &'a AliasMap) -> &'a str {
    let name = aliases.resolved_target(name).unwrap_or(name);
    merged.get(name).map_or(name, |target| target.as_str())
}

/// Every glyph name a `remap` rule *matches on*: a source, a lookbehind or a
/// lookahead, from every rule in `docs` whatever slice it is stated for.
///
/// Both the name as written and the glyph it names are collected, so a rule
/// that matches through a declared alias excludes the glyph itself.
fn remap_inputs(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    aliases: &AliasMap,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for doc in docs {
        for item in &doc.items {
            let DocumentItem::Remap {
                lookbehind,
                source,
                lookahead,
                ..
            } = item
            else {
                continue;
            };
            for element in lookbehind.iter().chain(source).chain(lookahead) {
                for name in expand_name_element(element, name_parts) {
                    if let Some(target) = aliases.resolved_target(&name) {
                        out.insert(target.to_string());
                    }
                    out.insert(name);
                }
            }
        }
    }
    out
}

/// Every block whose expansions are candidates: a `glyph` block whose name is
/// a pattern standing for more than one name, and that does not say `keep`.
fn collect_blocks(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    exists: &crate::exists::ExistsScopes,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Glyph { name, body } = item else {
                continue;
            };
            // A block governed by an `exists` is a pattern block like any
            // other once `$N` is bound; the candidates it offers are still one
            // block's expansions, which is the rule this module rests on.
            let bound;
            let name_parts = match exists.scope(crate::resolve::ItemRef::new(doc_idx, item_idx)) {
                Some(scope) if scope.matches.is_empty() => continue,
                Some(scope) => {
                    bound = scope.bindings(name_parts);
                    &bound
                }
                None => name_parts,
            };
            if body.keep || !is_name_pattern(&substitute_name_parts(&name.display(), name_parts)) {
                continue;
            }
            // A block that does not expand is reported by the expansion, which
            // is where the line is known; here it simply declares nothing to
            // merge.
            let Ok(expanded) = expand_glyph_block_slots(name, body, name_parts) else {
                continue;
            };
            if expanded.len() < 2 {
                continue;
            }
            let (members, slots) = expanded.into_iter().unzip();
            blocks.push(Block { members, slots });
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::parse_document_from_str;

    fn merges(src: &str) -> Vec<(String, String)> {
        let doc = parse_document_from_str(src, "t.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let aliases = AliasMap::collect(&docs, &name_parts);
        let (exists, _) = crate::exists::resolve_scopes(&docs, &name_parts, &aliases);
        implicit_merges(&docs, &name_parts, &aliases, &exists)
    }

    fn pairs(merges: &[(String, String)]) -> Vec<(&str, &str)> {
        merges
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect()
    }

    #[test]
    fn a_block_with_nothing_that_varies_merges_whole() {
        let m = merges("glyph a-(g|j|k) 1 1\n@@\n");
        assert_eq!(pairs(&m), vec![("a-j", "a-g"), ("a-k", "a-g")]);
    }

    #[test]
    fn a_lone_name_declares_one_glyph_and_merges_nothing() {
        assert!(merges("glyph a 1 1\n@@\n").is_empty());
    }

    #[test]
    fn keep_opts_the_block_out() {
        assert!(merges("glyph a-(g|j|k) 1 1 keep\n@@\n").is_empty());
    }

    /// The fixpoint: `b`'s slots name `a`'s expansions, so `b` merges exactly
    /// where `a` did — and the representative it lands on is `a`'s.
    #[test]
    fn a_ref_pattern_follows_the_glyphs_it_names() {
        let m = merges(
            "\
glyph a-(g|j) 1 1
@@
glyph a-k 1 1
..
glyph b-(g|j|k) 1 1
ref a-(g|j|k) 0 0
",
        );
        assert_eq!(pairs(&m), vec![("a-j", "a-g"), ("b-j", "b-g")]);
    }

    /// Two rounds' worth of chain, settled by one call.
    #[test]
    fn the_fixpoint_reaches_up_a_chain() {
        let m = merges(
            "\
glyph a-(g|j) 1 1
@@
glyph b-(g|j) 1 1
ref a-(g|j) 0 0
glyph c-(g|j) 1 1
ref b-(g|j) 0 0
",
        );
        assert_eq!(
            pairs(&m),
            vec![("a-j", "a-g"), ("b-j", "b-g"), ("c-j", "c-g")]
        );
    }

    /// Slots that name different glyphs keep their expansions apart, however
    /// alike the two glyphs are drawn.
    #[test]
    fn differing_slots_are_not_merged() {
        let m = merges(
            "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c-(g|j) 1 1
ref (a|b) 0 0
",
        );
        assert!(m.is_empty(), "{m:?}");
    }

    /// A declared alias is where the fixpoint starts: `ref a` and `ref a-alt`
    /// name one glyph, so the two expansions do too.
    #[test]
    fn a_declared_alias_is_seen_through() {
        let m = merges(
            "\
glyph a 1 1
@@
glyph a-alt = a
glyph c-(g|j) 1 1
ref (a|a-alt) 0 0
",
        );
        assert_eq!(pairs(&m), vec![("c-j", "c-g")]);
    }

    /// An undefined name is σ-equal to nothing but itself, which is what makes
    /// `ifexists` need no rule of its own.
    #[test]
    fn a_slot_naming_nothing_is_merged_with_nothing() {
        let m = merges(
            "\
glyph a-(g|j) 1 1
@@
glyph b-(g|j|k) 1 1
ref a-(g|j|k) 0 0 ifexists
",
        );
        assert_eq!(pairs(&m), vec![("a-j", "a-g"), ("b-j", "b-g")]);
    }

    /// IDC components are slots like `ref` names, and both are read in written
    /// order.
    #[test]
    fn idc_components_are_slots_too() {
        let m = merges(
            "\
glyph part-(g|j):2x4-l 2 4
@@..
@@..
@@..
@@..
glyph right:2x4-r 2 4
..@@
..@@
..@@
..@@
glyph whole-(g|j) 4 4
\u{2FF0} part-(g|j):2x4-l right:2x4-r
",
        );
        assert_eq!(
            pairs(&m),
            vec![("part-j:2x4-l", "part-g:2x4-l"), ("whole-j", "whole-g")]
        );
    }

    /// A glyph a rule matches on is drawn like its sibling on purpose and
    /// shaped unlike it — the hangul jamo case. It keeps its own id.
    #[test]
    fn a_glyph_a_remap_matches_on_is_never_merged() {
        for rule in [
            "remap g : a-f -> x",
            "remap g : a-f : x -> x",
            "remap g : x | a-f -> x",
            "remap g : a-f | x -> x",
        ] {
            let m = merges(&format!(
                "glyph x 1 1\n@@\nglyph a-(nof|f) advance 0\nref x\n{rule}\n"
            ));
            assert!(m.is_empty(), "{rule}: {m:?}");
        }
    }

    /// A name a rule only *produces* is mergeable: that is the `locl` shape
    /// the whole feature exists for.
    #[test]
    fn a_glyph_a_remap_only_produces_is_merged() {
        let m = merges(
            "\
glyph x 1 1
@@
glyph a-(g|j) advance 0
ref x
remap locl : x -> a-g
remap locl2 : x -> a-j
",
        );
        assert_eq!(pairs(&m), vec![("a-j", "a-g")]);
    }

    /// Matching through a declared alias excludes the glyph the alias names.
    #[test]
    fn a_rule_matching_an_alias_excludes_its_target() {
        let m = merges(
            "\
glyph x 1 1
@@
glyph a-(nof|f) advance 0
ref x
glyph a-alias = a-f
remap g : a-alias -> x
",
        );
        assert!(m.is_empty(), "{m:?}");
    }

    /// `$name-parts` are substituted before anything is compared, exactly as
    /// the expansion does it.
    #[test]
    fn name_parts_are_substituted_first() {
        let m = merges("name-parts $r = g j\nglyph a-($r) 1 1\n@@\n");
        assert_eq!(pairs(&m), vec![("a-j", "a-g")]);
    }
}
