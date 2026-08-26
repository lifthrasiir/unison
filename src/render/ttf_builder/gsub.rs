//! GSUB generation: `remap` collection, lookup classification and the
//! individual lookup builders.
//!
//! # A remap group is one lookup
//!
//! Every group becomes exactly one lookup, and a lookup is one left-to-right
//! pass over the glyph sequence; the group's rules are that lookup's subtables,
//! in declaration order. Nothing in OpenType forces this — a rule could equally
//! be given a lookup of its own — but collapsing the group keeps the pass count
//! down, and the resulting subtable order is a useful tool in its own right.
//! What it costs is that three things become order-dependent, and all three have
//! produced bugs:
//!
//! * **The first matching subtable wins.** The shaper tries them in order and
//!   returns on the first success, then advances past the input that rule
//!   consumed (rustybuzz `SubstLookup::apply`). A rule whose source is a prefix
//!   of a longer rule's source must therefore come *after* it, or the short one
//!   eats the prefix and the long one never fires.
//! * **No subtable sees another's output at the same position.** Re-entry is a
//!   property of passes, not of subtables, so re-substituting what the group
//!   produced needs a second group.
//! * **Lookbehind is matched against rewritten glyphs, lookahead against
//!   untouched ones**, since the pass has passed the former and not the latter.
//!   A lookbehind chain repeats over a run of unbounded length; a lookahead
//!   chain cannot, and has to enumerate its context instead — unless the group
//!   is `reversed`, which turns the pass around and with it which side of a
//!   rule can chain (see [`build_reverse_chain_lookup`]).
//!
//! [`classify_remap_set`] is where the collapse happens: a group with any
//! context becomes a single chain-context lookup whose subtables carry one
//! nested helper lookup each, which is also how rules of different types come to
//! coexist in one group at all.
//!
//! # Feature targets and scope fallback
//!
//! A `feature` target is written either as a script tag (`latn`, `DFLT`) or as a
//! script narrowed to one language system (`latn/ROM`). The two forms are
//! explicit rather than told apart by the tag itself, for two reasons: the
//! registries' one apparent collision is inverted (`DFLT` is the default
//! *script*, `dflt` the default *language*), and a language tag means nothing
//! without its script (`SRB` lives under both `latn` and `cyrl`).
//!
//! Directives sharing a tag *and* a target become one feature record, lookups
//! accumulating in declaration order, because a shaper only ever finds the first
//! record for a tag. The same tag under different targets stays separate.
//!
//! **Both fallbacks are replacements, not extensions**, which is what
//! [`inherit_tags`] is for: a shaper reads `DFLT` only when the script it wants
//! has no record at all, and reads a `LangSys` *instead of* its script's
//! default. So the builder folds `DFLT`'s features into every declared script
//! and each script's default into every language below it, merging per feature
//! tag so an inherited tag and a redeclared one end up as one record. Left out,
//! adding a single `locl for latn/ROM` silently costs all Latin text its
//! `ccmp` — and every mark attachment with it.

use super::tables::{ScriptFeatures, build_script_records, make_tag, parse_script_lang};
use super::*;

pub(super) fn collect_gsub_data(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    aliases: &crate::alias::AliasMap,
) -> GsubData {
    let mut remap_sets: BTreeMap<String, Vec<ExpandedRemap>> = BTreeMap::new();
    let mut features: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut anchor_features: Vec<AnchorFeature> = Vec::new();

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                DocumentItem::Remap {
                    feature,
                    lookbehind,
                    source,
                    target,
                    lookahead,
                    ..
                } => {
                    let source_patterns: Vec<NamePattern> = source
                        .iter()
                        .map(|s| parse_name_element(s, name_parts))
                        .collect();
                    let target_patterns: Vec<NamePattern> = target
                        .iter()
                        .map(|s| parse_name_element(s, name_parts))
                        .collect();

                    // The number of remap entries is the longest position's
                    // expansion; every other position cycles inside it. A
                    // position that does not divide it is a warning from
                    // `issues.rs` rather than a silently longer rule.
                    let entry_count = crate::pattern::combined_len(
                        source_patterns.iter().chain(target_patterns.iter()),
                    );

                    let mut source_seqs = Vec::with_capacity(entry_count);
                    let mut target_seqs = Vec::with_capacity(entry_count);
                    for i in 0..entry_count {
                        let mut seq: Vec<String> =
                            source_patterns.iter().map(|pos| pos.get(i)).collect();
                        aliases.canonicalize_all(&mut seq);
                        source_seqs.push(seq);
                        let mut tseq: Vec<String> =
                            target_patterns.iter().map(|pos| pos.get(i)).collect();
                        aliases.canonicalize_all(&mut tseq);
                        target_seqs.push(tseq);
                    }

                    let lb: Vec<Vec<String>> = lookbehind
                        .iter()
                        .map(|s| {
                            let mut names = expand_name_element(s, name_parts);
                            aliases.canonicalize_all(&mut names);
                            names
                        })
                        .collect();
                    let la: Vec<Vec<String>> = lookahead
                        .iter()
                        .map(|s| {
                            let mut names = expand_name_element(s, name_parts);
                            aliases.canonicalize_all(&mut names);
                            names
                        })
                        .collect();

                    remap_sets
                        .entry(feature.clone())
                        .or_default()
                        .push(ExpandedRemap {
                            origin: Some(ItemRef::new(doc_idx, item_idx)),
                            lookbehind: lb,
                            source: source_seqs,
                            target: target_seqs,
                            lookahead: la,
                        });
                }
                DocumentItem::Feature {
                    name,
                    scripts,
                    remap_group,
                    ..
                } => {
                    features.push((name.clone(), scripts.clone(), vec![remap_group.clone()]));
                }
                DocumentItem::FeatureAnchor {
                    name,
                    scripts,
                    anchor,
                    align,
                    ..
                } => {
                    anchor_features.push(AnchorFeature {
                        tag: name.clone(),
                        scripts: scripts.clone(),
                        anchor: anchor.clone(),
                        align: *align,
                    });
                }
                _ => {}
            }
        }
    }

    GsubData {
        remap_sets,
        groups: crate::document::remap_group_order(docs),
        features,
        anchor_features,
        // Filled by `collect::compute_shared_font_input_for`, which is where
        // the face's slice expansion lives — a pair has to be read per face,
        // and this collector only ever sees the raw documents.
        uvs_pairs: Vec::new(),
        uvs_selectors: Vec::new(),
    }
}

/// Every variation selector any slice of `docs` mentions, ascending.
pub(super) fn collect_uvs_selectors(docs: &[&Document]) -> Vec<u32> {
    let mut out: Vec<u32> = docs
        .iter()
        .flat_map(|doc| &doc.items)
        .filter_map(|item| match item {
            DocumentItem::Map {
                selector: Some(sel),
                ..
            } => Some(super::expand_map_codepoints(sel)),
            _ => None,
        })
        .flatten()
        .filter(|cp| crate::ucd::is_variation_selector(*cp))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The face's own variation sequences, read from its expanded items.
pub(super) fn collect_uvs_pairs(
    all_items: &[DocumentItem],
    aliases: &crate::alias::AliasMap,
) -> Vec<UvsPair> {
    let mut out = Vec::new();
    for item in all_items {
        let DocumentItem::Map {
            char_repr,
            selector: Some(sel),
            glyphs,
            ..
        } = item
        else {
            continue;
        };
        let glyph = super::resolved_map_target(glyphs);
        let Ok(triples) = super::expand_uvs_map_triples(char_repr, sel, glyph) else {
            // Every rejection has already been reported by `crate::issues`;
            // the build's job here is only to not emit half a sequence.
            continue;
        };
        for (base, selector, mut glyph) in triples {
            // Canonicalized like any other map target, so a pair aimed at an
            // alias reaches the glyph the alias names rather than a dead name.
            aliases.canonicalize(&mut glyph);
            out.push(UvsPair {
                base,
                selector,
                glyph,
            });
        }
    }
    out
}

/// Prepend the tags of a broader scope (`DFLT`, or a script's default LangSys)
/// to a narrower one that inherits from it, merging lookups where both declare
/// the same feature tag. The broader scope goes first so declaration order
/// survives; duplicate lookup indices are dropped, since a lookup listed twice
/// in one feature record is applied twice.
/// One feature tag with the lookup indices declared for it, before a language
/// system's tags become `FeatureRecord`s.
#[derive(Clone)]
struct TagLookups {
    tag: String,
    lookups: Vec<u16>,
}

fn inherit_tags(tags: &mut Vec<TagLookups>, inherited: &[TagLookups]) {
    let own = std::mem::replace(tags, inherited.to_vec());
    for entry in own {
        match tags.iter_mut().find(|t| t.tag == entry.tag) {
            Some(existing) => {
                for idx in entry.lookups {
                    if !existing.lookups.contains(&idx) {
                        existing.lookups.push(idx);
                    }
                }
            }
            None => tags.push(entry),
        }
    }
}

enum RemapSetKind {
    Single,
    Multiple,
    Ligature,
    ChainContext,
    Reverse,
}

/// The GSUB lookup type a single `remap` line needs, ignoring its context.
///
/// `None` means the rule cannot be expressed in OpenType at all: many-to-many
/// and many-to-nothing have no lookup type. Those are reported as errors by
/// [`crate::issues`]; here they are simply dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemapRuleKind {
    Single,
    Multiple,
    Ligature,
}

pub(crate) fn remap_rule_kind(source_len: usize, target_len: usize) -> Option<RemapRuleKind> {
    match (source_len, target_len) {
        (0, _) => None,
        (1, 1) => Some(RemapRuleKind::Single),
        // Zero targets is a deletion, which MultipleSubst expresses as an
        // empty sequence — disallowed by the spec, honoured by every shaper.
        (1, _) => Some(RemapRuleKind::Multiple),
        (_, 1) => Some(RemapRuleKind::Ligature),
        _ => None,
    }
}

fn rule_kind_of(r: &ExpandedRemap) -> Option<RemapRuleKind> {
    remap_rule_kind(
        r.source.first().map_or(0, |seq| seq.len()),
        r.target.first().map_or(0, |seq| seq.len()),
    )
}

fn classify_remap_set(remaps: &[ExpandedRemap], reversed: bool) -> RemapSetKind {
    // `reversed` is the group's own decision and overrides the shape-based
    // classification: the reverse lookup is the only one that runs right to
    // left, and it cannot be reached any other way. Rules it cannot express
    // (anything but 1 → 1) are an error `issues.rs` reports; here the offending
    // rule is dropped rather than quietly rebuilt as something forward.
    if reversed {
        return RemapSetKind::Reverse;
    }

    let has_context = remaps
        .iter()
        .any(|r| !r.lookbehind.is_empty() || !r.lookahead.is_empty());
    if has_context {
        return RemapSetKind::ChainContext;
    }

    // A homogeneous group collapses into one lookup of that type. A mixed one
    // cannot: the aggregate builders each ignore rules of the other kinds, so
    // whichever kind lost used to vanish without a word. Chain context with an
    // empty context expresses every kind at once, one nested lookup per rule,
    // and keeps the declaration order that a single lookup would have had.
    let mut kinds = remaps.iter().filter_map(rule_kind_of);
    let Some(first) = kinds.next() else {
        return RemapSetKind::ChainContext;
    };
    if !kinds.all(|k| k == first) {
        return RemapSetKind::ChainContext;
    }
    match first {
        RemapRuleKind::Single => RemapSetKind::Single,
        RemapRuleKind::Multiple => RemapSetKind::Multiple,
        RemapRuleKind::Ligature => RemapSetKind::Ligature,
    }
}

/// The GSUB half of a variation sequence: `base selector -> target`, as one
/// ligature lookup over every pair the face states.
///
/// A ligature (2→1) and not a single substitution, because the selector has to
/// *leave* the buffer. That matters most in the case where the target is the
/// base's own glyph — a "default" variation sequence — where a 1→1 rule would
/// substitute nothing and leave the selector sitting there.
fn build_uvs_fallback_lookup(
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
    cp_to_gid: &HashMap<u32, GlyphId16>,
) -> Option<SubstitutionLookup> {
    let mut by_first: BTreeMap<GlyphId16, BTreeMap<GlyphId16, GlyphId16>> = BTreeMap::new();
    for pair in &gsub_data.uvs_pairs {
        let (Some(&base), Some(&sel), Some(&target)) = (
            cp_to_gid.get(&pair.base),
            name_to_gid.get(super::vs_glyph_name(pair.selector).as_str()),
            name_to_gid.get(pair.glyph.as_str()),
        ) else {
            continue;
        };
        // First rule wins, matching how a ligature set is searched. A second
        // pair colliding here is reported by `crate::issues`; silently keeping
        // both would make which one applies depend on document order.
        by_first
            .entry(base)
            .or_default()
            .entry(sel)
            .or_insert(target);
    }
    if by_first.is_empty() {
        return None;
    }

    let coverage = CoverageTable::format_1(by_first.keys().copied().collect());
    let ligature_sets: Vec<LigatureSet> = by_first
        .values()
        .map(|entries| {
            LigatureSet::new(
                entries
                    .iter()
                    .map(|(sel, target)| Ligature::new(*target, vec![*sel]))
                    .collect(),
            )
        })
        .collect();

    Some(SubstitutionLookup::Ligature(Lookup::new(
        LookupFlag::empty(),
        vec![LigatureSubstFormat1::new(
            coverage,
            ligature_sets.into_iter().collect(),
        )],
    )))
}

pub(super) fn build_gsub(
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
    cp_to_gid: &HashMap<u32, GlyphId16>,
) -> Option<Gsub> {
    if gsub_data.features.is_empty() && gsub_data.uvs_pairs.is_empty() {
        return None;
    }

    let mut lookups: Vec<SubstitutionLookup> = Vec::new();
    let mut set_to_lookup: HashMap<String, u16> = HashMap::new();

    // Lookup 0, before anything the source wrote. A rule aimed at a pair's
    // *target* has to see one glyph where the text had two, so the fold has to
    // have happened by the time any source lookup runs. On a shaper that honors
    // cmap 14 this never fires — the selector is gone before GSUB starts — and
    // that is the point: it is the same statement, kept for the shaper that
    // does not.
    let uvs_lookup_idx =
        build_uvs_fallback_lookup(gsub_data, name_to_gid, cp_to_gid).map(|lookup| {
            let idx = lookups.len() as u16;
            lookups.push(lookup);
            idx
        });

    // Lookup index order is application order — a shaper runs the lookups of a
    // stage sorted by index, across features — so this is where "which pass
    // happens first" is decided, and it belongs to the groups. Where a group is
    // attached with `feature` deliberately has no say in it.
    for setname in &gsub_data.groups.order {
        let Some(remaps) = gsub_data.remap_sets.get(setname) else {
            continue;
        };
        let reversed = gsub_data
            .groups
            .info
            .get(setname)
            .is_some_and(|i| i.reversed);
        match classify_remap_set(remaps, reversed) {
            RemapSetKind::Single => {
                let lookup = build_single_subst_lookup(remaps, name_to_gid);
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(lookup);
            }
            RemapSetKind::Multiple => {
                let lookup = build_multiple_subst_lookup(remaps, name_to_gid);
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(lookup);
            }
            RemapSetKind::Ligature => {
                let lookup = build_ligature_subst_lookup(remaps, name_to_gid);
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(lookup);
            }
            RemapSetKind::Reverse => {
                // Rules the reverse format cannot express are dropped (and
                // reported); if that leaves nothing, the group is skipped
                // entirely rather than emitting a lookup with no subtables.
                let Some(lookup) = build_reverse_chain_lookup(remaps, name_to_gid) else {
                    continue;
                };
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(lookup);
            }
            RemapSetKind::ChainContext => {
                let helper_start = lookups.len();
                let mut chain_subtables: Vec<SubstitutionChainContext> = Vec::new();

                for r in remaps {
                    // The nested lookup does the actual substitution. Its type
                    // has to follow the rule: a multi-glyph source needs a
                    // ligature there, and forcing a SingleSubst on the first
                    // input position instead used to turn `a b -> c` into
                    // "replace a, keep b" without a word.
                    let Some(helper) = build_chain_helper(r, name_to_gid) else {
                        continue;
                    };
                    let helper_idx = lookups.len() as u16;
                    lookups.push(helper);

                    let backtrack: Vec<CoverageTable> = r
                        .lookbehind
                        .iter()
                        .rev()
                        .map(|names| make_coverage(names, name_to_gid))
                        .collect();

                    // Input coverages: one per source position
                    let input_len = r.source.first().map_or(1, |seq| seq.len());
                    let input: Vec<CoverageTable> = (0..input_len)
                        .map(|pos| {
                            let names: Vec<String> = r
                                .source
                                .iter()
                                .filter_map(|seq| seq.get(pos).cloned())
                                .collect();
                            make_coverage(&names, name_to_gid)
                        })
                        .collect();

                    let lookahead: Vec<CoverageTable> = r
                        .lookahead
                        .iter()
                        .map(|names| make_coverage(names, name_to_gid))
                        .collect();

                    let slr = SequenceLookupRecord::new(0, helper_idx);

                    let mut sc = SubstitutionChainContext::default();
                    *sc = ChainedSequenceContext::Format3(ChainedSequenceContextFormat3::new(
                        backtrack,
                        input,
                        lookahead,
                        vec![slr],
                    ));
                    chain_subtables.push(sc);
                }

                let chain_lookup = SubstitutionLookup::ChainContextual(Lookup::new(
                    LookupFlag::empty(),
                    chain_subtables,
                ));
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(chain_lookup);
                let _ = helper_start;
            }
        }
    }

    // One feature record per (script, language, tag). A shaper resolves a tag
    // to the first record listed for the language system and never looks
    // further, so two `feature` directives sharing a tag *and* a target must
    // contribute to one record — otherwise every group after the first is dead
    // weight, and a font could only ever use as many remap groups as there are
    // feature tags. Lookups accumulate in declaration order, which is also
    // application order.
    let mut per_script: BTreeMap<String, BTreeMap<Option<String>, Vec<TagLookups>>> =
        BTreeMap::new();

    // Seeded before the source's own features so that a `ccmp for DFLT` the
    // source declares *extends* this record rather than replacing it, leaving
    // the fallback lookup first in application order. The DFLT fold below then
    // carries it to every script the font declares.
    if let Some(idx) = uvs_lookup_idx {
        per_script
            .entry("DFLT".to_string())
            .or_default()
            .entry(None)
            .or_default()
            .push(TagLookups {
                tag: "ccmp".to_string(),
                lookups: vec![idx],
            });
    }

    for (feat_tag, targets, set_names) in &gsub_data.features {
        let lookup_indices: Vec<u16> = set_names
            .iter()
            .filter_map(|sn| set_to_lookup.get(sn).copied())
            .collect();

        for target in targets {
            let (script, lang) = parse_script_lang(target);
            let tags = per_script
                .entry(script)
                .or_default()
                .entry(lang)
                .or_default();
            match tags.iter_mut().find(|t| &t.tag == feat_tag) {
                Some(existing) => existing.lookups.extend(lookup_indices.iter().copied()),
                None => tags.push(TagLookups {
                    tag: feat_tag.clone(),
                    lookups: lookup_indices.clone(),
                }),
            }
        }
    }

    // `DFLT` is what a shaper falls back to only when the script it asked for
    // has no record at all, so declaring *any* feature for a real script makes
    // that script stop seeing DFLT. Fold DFLT's features into every declared
    // script, or adding one `locl for latn/ROM` would cost all Latin text its
    // `ccmp` — every mark attachment with it.
    if let Some(dflt_tags) = per_script
        .get("DFLT")
        .and_then(|langs| langs.get(&None))
        .cloned()
    {
        for (script, langs) in per_script.iter_mut() {
            if script == "DFLT" {
                continue;
            }
            // A script named only through a language (`latn/ROM` with no bare
            // `latn`) still needs the default LangSys DFLT used to cover.
            inherit_tags(langs.entry(None).or_default(), &dflt_tags);
        }
    }

    // An explicit language system likewise replaces the script's default rather
    // than extending it, so fold that default into every language below it.
    // Merging at tag level (and not later, at feature-record level) is what
    // keeps `locl for latn/ROM` from producing a second `ccmp` record beside
    // the inherited one, which the shaper would have to choose between.
    for langs in per_script.values_mut() {
        let Some(default_tags) = langs.get(&None).cloned() else {
            continue;
        };
        for (lang, tags) in langs.iter_mut() {
            if lang.is_some() {
                inherit_tags(tags, &default_tags);
            }
        }
    }

    let mut feature_records: Vec<FeatureRecord> = Vec::new();
    let mut record_of: HashMap<(String, Vec<u16>), u16> = HashMap::new();
    // Collect which feature indices belong to which language system
    let mut script_features: BTreeMap<String, ScriptFeatures> = BTreeMap::new();
    for (script, langs) in per_script {
        for (lang, tags) in langs {
            for TagLookups {
                tag: feat_tag,
                lookups: lookup_indices,
            } in tags
            {
                // Language systems that end up with the same tag and the same
                // lookups share one record rather than duplicating it.
                let feat_idx = *record_of
                    .entry((feat_tag.clone(), lookup_indices.clone()))
                    .or_insert_with(|| {
                        let idx = feature_records.len() as u16;
                        feature_records.push(FeatureRecord::new(
                            make_tag(&feat_tag),
                            Feature::new(None, lookup_indices),
                        ));
                        idx
                    });
                script_features
                    .entry(script.clone())
                    .or_default()
                    .push(lang.as_deref(), feat_idx);
            }
        }
    }

    let script_records = build_script_records(&script_features);

    let script_list = ScriptList::new(script_records);
    let feature_list = FeatureList::new(feature_records);
    let lookup_list: LookupList<SubstitutionLookup> = LookupList::new(lookups);

    Some(Gsub::new(script_list, feature_list, lookup_list))
}

pub(super) fn compute_max_context(gsub_data: &GsubData) -> u16 {
    let mut max_ctx: u16 = 1;
    for remaps in gsub_data.remap_sets.values() {
        for r in remaps {
            let lb_len = r.lookbehind.len() as u16;
            let input_len = r.source.first().map_or(1, |seq| seq.len()) as u16;
            let la_len = r.lookahead.len() as u16;
            // A chaining lookup's context is the whole sequence it has to look
            // at, backtrack included — a client that buffers `usMaxContext`
            // glyphs around an edit needs the lookbehind to fit in it too.
            let ctx = lb_len + input_len + la_len;
            max_ctx = max_ctx.max(ctx);
        }
    }
    max_ctx
}

pub(super) fn make_coverage(
    names: &[String],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> CoverageTable {
    let mut gids: Vec<GlyphId16> = names
        .iter()
        .filter_map(|n| name_to_gid.get(n).copied())
        .collect();
    gids.sort();
    gids.dedup();
    CoverageTable::format_1(gids)
}

/// The rules a single-substitution group would drop when its lookup is built:
/// a source glyph the group already substitutes, being substituted again.
///
/// A `SingleSubst` coverage names each glyph once, so
/// [`build_single_subst_from_pairs`] keeps the first rule for a glyph and
/// discards the rest. That is right — rule order is match priority
/// ([`super::gsub`]) — but it is invisible, and two rules land on one glyph
/// without looking like it whenever two *names* reach one glyph: through a
/// declared alias, or through an implicit merge ([`crate::merge`]), which is
/// the one way merging a glyph could change what a font does. So the drop is
/// reported here, on the line whose rule loses, rather than left to be found
/// by shaping.
///
/// Only a group that becomes one single-substitution lookup is checked: every
/// other kind keeps its rules in declaration order, one subtable or one nested
/// lookup each, and shadows nothing.
pub(crate) fn shadowed_single_subst_rules(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    aliases: &crate::alias::AliasMap,
) -> Vec<Diagnostic> {
    let data = collect_gsub_data(docs, name_parts, aliases);
    let mut out = Vec::new();
    for (group, remaps) in &data.remap_sets {
        let reversed = data.groups.info.get(group).is_some_and(|i| i.reversed);
        if !matches!(classify_remap_set(remaps, reversed), RemapSetKind::Single) {
            continue;
        }
        // Keyed by the glyph, not the name: the collision this is about is one
        // the names hide.
        let mut claimed: HashMap<&str, &str> = HashMap::new();
        for remap in remaps {
            for (seq, tgt) in remap.source.iter().zip(remap.target.iter()) {
                let ([source], [target]) = (&seq[..], &tgt[..]) else {
                    continue;
                };
                match claimed.get(source.as_str()) {
                    // The same substitution written twice loses nothing.
                    Some(&first) if first == target => {}
                    Some(&first) => out.push(Diagnostic::new(
                        Severity::Warning,
                        remap.origin,
                        format!(
                            "remap of '{source}' to '{target}' in group '{group}' is shadowed \
                             by an earlier rule substituting '{first}'; a group of single \
                             substitutions covers each glyph once, so only the first applies"
                        ),
                    )),
                    None => {
                        claimed.insert(source, target);
                    }
                }
            }
        }
    }
    out
}

pub(super) fn build_single_subst_from_pairs(
    sources: &[String],
    targets: &[String],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut pairs: Vec<(GlyphId16, GlyphId16)> = sources
        .iter()
        .zip(targets.iter())
        .filter_map(|(s, t)| {
            let sg = name_to_gid.get(s)?;
            let tg = name_to_gid.get(t)?;
            Some((*sg, *tg))
        })
        .collect();
    pairs.sort_by_key(|&(s, _)| s);
    pairs.dedup_by_key(|p| p.0);

    let coverage_gids: Vec<GlyphId16> = pairs.iter().map(|&(s, _)| s).collect();
    let substitute_gids: Vec<GlyphId16> = pairs.iter().map(|&(_, t)| t).collect();

    let coverage = CoverageTable::format_1(coverage_gids);
    let subtable = SingleSubst::Format2(SingleSubstFormat2::new(coverage, substitute_gids));

    SubstitutionLookup::Single(Lookup::new(LookupFlag::empty(), vec![subtable]))
}

fn build_single_subst_lookup(
    remaps: &[ExpandedRemap],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut all_sources = Vec::new();
    let mut all_targets = Vec::new();
    for r in remaps {
        for (seq, tgt) in r.source.iter().zip(r.target.iter()) {
            if seq.len() == 1 && !tgt.is_empty() {
                all_sources.push(seq[0].clone());
                all_targets.push(tgt[0].clone());
            }
        }
    }
    build_single_subst_from_pairs(&all_sources, &all_targets, name_to_gid)
}

/// A `reversed` group: one reverse chaining contextual single substitution,
/// one subtable per rule.
///
/// This is the only lookup a shaper applies right to left, which is the whole
/// point of it: at each position the glyphs *ahead* have already been through
/// this same lookup, so a rule whose lookahead names its own output repeats
/// leftward over a run of any length. A forward lookup can only do that with a
/// lookbehind, and a run anchored on its right has nothing to chain off there.
///
/// The price is that the format substitutes one glyph for one glyph and nothing
/// else — there is no reverse ligature — and that it may not be invoked as a
/// nested lookup, so it cannot share a group with rules of other kinds.
fn build_reverse_chain_lookup(
    remaps: &[ExpandedRemap],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> Option<SubstitutionLookup> {
    let mut subtables: Vec<ReverseChainSingleSubstFormat1> = Vec::new();

    for r in remaps {
        // `substitute_glyph_ids` is indexed by coverage index, and a coverage
        // table is sorted by glyph id — so the pairs have to be sorted by
        // source before either array is written, not left in rule order.
        let mut pairs: Vec<(GlyphId16, GlyphId16)> = r
            .source
            .iter()
            .zip(r.target.iter())
            .filter(|(src, tgt)| src.len() == 1 && tgt.len() == 1)
            .filter_map(|(src, tgt)| Some((*name_to_gid.get(&src[0])?, *name_to_gid.get(&tgt[0])?)))
            .collect();
        pairs.sort_by_key(|(src, _)| *src);
        pairs.dedup_by_key(|(src, _)| *src);
        if pairs.is_empty() {
            continue;
        }

        let coverage = CoverageTable::format_1(pairs.iter().map(|(src, _)| *src).collect());
        let substitutes: Vec<GlyphId16> = pairs.iter().map(|(_, tgt)| *tgt).collect();

        // Backtrack runs outward from the input — nearest glyph first — the
        // same way the chain-context builder above orders it.
        let backtrack: Vec<CoverageTable> = r
            .lookbehind
            .iter()
            .rev()
            .map(|names| make_coverage(names, name_to_gid))
            .collect();
        let lookahead: Vec<CoverageTable> = r
            .lookahead
            .iter()
            .map(|names| make_coverage(names, name_to_gid))
            .collect();

        subtables.push(ReverseChainSingleSubstFormat1::new(
            coverage,
            backtrack,
            lookahead,
            substitutes,
        ));
    }

    if subtables.is_empty() {
        return None;
    }
    Some(SubstitutionLookup::Reverse(Lookup::new(
        LookupFlag::empty(),
        subtables,
    )))
}

/// The nested lookup a chain-context rule invokes at input position 0.
fn build_chain_helper(
    r: &ExpandedRemap,
    name_to_gid: &HashMap<String, GlyphId16>,
) -> Option<SubstitutionLookup> {
    // The kind comes first: a rule with no source at all has no lookup type,
    // and indexing its source sequences before asking used to panic instead of
    // dropping it.
    let kind = rule_kind_of(r)?;
    let first_sources: Vec<String> = r
        .source
        .iter()
        .filter_map(|seq| seq.first().cloned())
        .collect();
    match kind {
        RemapRuleKind::Single => {
            let first_targets: Vec<String> = r.target.iter().map(|seq| seq[0].clone()).collect();
            Some(build_single_subst_from_pairs(
                &first_sources,
                &first_targets,
                name_to_gid,
            ))
        }
        RemapRuleKind::Multiple => Some(build_multiple_subst_from_pairs(
            &first_sources,
            &r.target,
            name_to_gid,
        )),
        RemapRuleKind::Ligature => Some(build_ligature_subst_lookup(
            std::slice::from_ref(r),
            name_to_gid,
        )),
    }
}

fn build_multiple_subst_from_pairs(
    sources: &[String],
    targets: &[Vec<String>],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut pairs: Vec<(GlyphId16, Vec<GlyphId16>)> = sources
        .iter()
        .zip(targets.iter())
        .filter_map(|(s, seq)| {
            let sg = *name_to_gid.get(s)?;
            let gids: Vec<GlyphId16> = seq
                .iter()
                .filter_map(|t| name_to_gid.get(t).copied())
                .collect();
            // A target glyph that has no id would silently shorten the
            // sequence, so drop the whole rule instead.
            (gids.len() == seq.len()).then_some((sg, gids))
        })
        .collect();
    pairs.sort_by_key(|(s, _)| *s);
    pairs.dedup_by_key(|p| p.0);

    let coverage = CoverageTable::format_1(pairs.iter().map(|(s, _)| *s).collect());
    let sequences: Vec<Sequence> = pairs
        .into_iter()
        .map(|(_, gids)| Sequence::new(gids))
        .collect();
    let subtable = MultipleSubstFormat1::new(coverage, sequences);

    SubstitutionLookup::Multiple(Lookup::new(LookupFlag::empty(), vec![subtable]))
}

fn build_multiple_subst_lookup(
    remaps: &[ExpandedRemap],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut all_sources = Vec::new();
    let mut all_targets = Vec::new();
    for r in remaps {
        for (seq, tgt) in r.source.iter().zip(r.target.iter()) {
            if seq.len() == 1 {
                all_sources.push(seq[0].clone());
                all_targets.push(tgt.clone());
            }
        }
    }
    build_multiple_subst_from_pairs(&all_sources, &all_targets, name_to_gid)
}

fn build_ligature_subst_lookup(
    remaps: &[ExpandedRemap],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut by_first: BTreeMap<GlyphId16, Vec<(Vec<GlyphId16>, GlyphId16)>> = BTreeMap::new();

    for r in remaps {
        for (seq, tgt) in r.source.iter().zip(r.target.iter()) {
            if seq.len() < 2 {
                continue;
            }
            let gids: Vec<GlyphId16> = seq
                .iter()
                .filter_map(|name| name_to_gid.get(name.as_str()).copied())
                .collect();
            if gids.len() != seq.len() {
                continue;
            }
            if tgt.is_empty() {
                continue;
            }
            let Some(&tgt_gid) = name_to_gid.get(tgt[0].as_str()) else {
                continue;
            };
            let first = gids[0];
            let rest = gids[1..].to_vec();
            by_first.entry(first).or_default().push((rest, tgt_gid));
        }
    }

    let coverage_gids: Vec<GlyphId16> = by_first.keys().copied().collect();
    let coverage = CoverageTable::format_1(coverage_gids);

    let ligature_sets: Vec<LigatureSet> = by_first
        .values()
        .map(|entries| {
            let mut ligs: Vec<Ligature> = entries
                .iter()
                .map(|(components, lig_glyph)| Ligature::new(*lig_glyph, components.clone()))
                .collect();
            ligs.sort_by(|a, b| {
                b.component_glyph_ids
                    .len()
                    .cmp(&a.component_glyph_ids.len())
                    .then_with(|| a.component_glyph_ids.cmp(&b.component_glyph_ids))
            });
            LigatureSet::new(ligs)
        })
        .collect();

    SubstitutionLookup::Ligature(Lookup::new(
        LookupFlag::empty(),
        vec![LigatureSubstFormat1::new(
            coverage,
            ligature_sets.into_iter().collect(),
        )],
    ))
}
