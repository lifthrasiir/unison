//! GSUB generation: `remap` collection, lookup classification and the
//! individual lookup builders.

use super::*;
use super::tables::{ScriptFeatures, build_script_records, make_tag, parse_script_lang};

pub(super) fn collect_gsub_data(docs: &[&Document], name_parts: &NamePartsMap) -> GsubData {
    let mut remap_sets: BTreeMap<String, Vec<ExpandedRemap>> = BTreeMap::new();
    let mut features: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut anchor_features: Vec<(String, Vec<String>, String)> = Vec::new();

    for doc in docs {
        for item in &doc.items {
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

                    // The number of remap entries is the LCM of all position
                    // expansion counts (each position cycles independently).
                    let entry_count = crate::pattern::combined_len(
                        source_patterns.iter().chain(target_patterns.iter()),
                    );

                    let mut source_seqs = Vec::with_capacity(entry_count);
                    let mut target_seqs = Vec::with_capacity(entry_count);
                    for i in 0..entry_count {
                        let seq: Vec<String> =
                            source_patterns.iter().map(|pos| pos.get(i)).collect();
                        source_seqs.push(seq);
                        let tseq: Vec<String> =
                            target_patterns.iter().map(|pos| pos.get(i)).collect();
                        target_seqs.push(tseq);
                    }

                    let lb: Vec<Vec<String>> = lookbehind
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();
                    let la: Vec<Vec<String>> = lookahead
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();

                    remap_sets.entry(feature.clone()).or_default().push(
                        ExpandedRemap {
                            lookbehind: lb,
                            source: source_seqs,
                            target: target_seqs,
                            lookahead: la,
                        },
                    );
                }
                DocumentItem::Feature { name, scripts, remap_group, .. } => {
                    features.push((name.clone(), scripts.clone(), vec![remap_group.clone()]));
                }
                DocumentItem::FeatureAnchor { name, scripts, anchor, .. } => {
                    anchor_features.push((name.clone(), scripts.clone(), anchor.clone()));
                }
                _ => {}
            }
        }
    }

    GsubData {
        remap_sets,
        features,
        anchor_features,
    }
}

/// Prepend the tags of a broader scope (`DFLT`, or a script's default LangSys)
/// to a narrower one that inherits from it, merging lookups where both declare
/// the same feature tag. The broader scope goes first so declaration order
/// survives; duplicate lookup indices are dropped, since a lookup listed twice
/// in one feature record is applied twice.
fn inherit_tags(tags: &mut Vec<(String, Vec<u16>)>, inherited: &[(String, Vec<u16>)]) {
    let own = std::mem::replace(tags, inherited.to_vec());
    for (feat_tag, lookup_indices) in own {
        match tags.iter_mut().find(|(t, _)| *t == feat_tag) {
            Some((_, indices)) => {
                for idx in lookup_indices {
                    if !indices.contains(&idx) {
                        indices.push(idx);
                    }
                }
            }
            None => tags.push((feat_tag, lookup_indices)),
        }
    }
}

enum RemapSetKind {
    Single,
    Multiple,
    Ligature,
    ChainContext,
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

fn classify_remap_set(remaps: &[ExpandedRemap]) -> RemapSetKind {
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

pub(super) fn build_gsub(
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
) -> Option<Gsub> {
    if gsub_data.features.is_empty() {
        return None;
    }

    let mut lookups: Vec<SubstitutionLookup> = Vec::new();
    let mut set_to_lookup: HashMap<String, u16> = HashMap::new();

    // Build lookups in feature declaration order so that lookup indices
    // respect the intended application order (e.g. ljmo < vjmo < tjmo).
    let mut ordered_sets: Vec<&String> = Vec::new();
    for (_, _, set_names) in &gsub_data.features {
        for sn in set_names {
            if !ordered_sets.contains(&sn) {
                ordered_sets.push(sn);
            }
        }
    }
    for setname in gsub_data.remap_sets.keys() {
        if !ordered_sets.contains(&setname) {
            ordered_sets.push(setname);
        }
    }

    for &setname in &ordered_sets {
        let Some(remaps) = gsub_data.remap_sets.get(setname) else { continue };
        match classify_remap_set(remaps) {
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
                            let names: Vec<String> = r.source.iter()
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
                    *sc = ChainedSequenceContext::Format3(
                        ChainedSequenceContextFormat3::new(
                            backtrack,
                            input,
                            lookahead,
                            vec![slr],
                        ),
                    );
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
    let mut per_script: BTreeMap<String, BTreeMap<Option<String>, Vec<(String, Vec<u16>)>>> =
        BTreeMap::new();
    for (feat_tag, targets, set_names) in &gsub_data.features {
        let lookup_indices: Vec<u16> = set_names
            .iter()
            .filter_map(|sn| set_to_lookup.get(sn).copied())
            .collect();

        for target in targets {
            let (script, lang) = parse_script_lang(target);
            let tags = per_script.entry(script).or_default().entry(lang).or_default();
            match tags.iter_mut().find(|(t, _)| t == feat_tag) {
                Some((_, indices)) => indices.extend(lookup_indices.iter().copied()),
                None => tags.push((feat_tag.clone(), lookup_indices.clone())),
            }
        }
    }

    // `DFLT` is what a shaper falls back to only when the script it asked for
    // has no record at all, so declaring *any* feature for a real script makes
    // that script stop seeing DFLT. Fold DFLT's features into every declared
    // script, or adding one `locl for latn/ROM` would cost all Latin text its
    // `ccmp` — every mark attachment with it.
    if let Some(dflt_tags) = per_script.get("DFLT").and_then(|langs| langs.get(&None)).cloned() {
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
        let Some(default_tags) = langs.get(&None).cloned() else { continue };
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
            for (feat_tag, lookup_indices) in tags {
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
            let input_len = r.source.first().map_or(1, |seq| seq.len()) as u16;
            let la_len = r.lookahead.len() as u16;
            let ctx = input_len + la_len;
            max_ctx = max_ctx.max(ctx);
        }
    }
    max_ctx
}

pub(super) fn make_coverage(names: &[String], name_to_gid: &HashMap<String, GlyphId16>) -> CoverageTable {
    let mut gids: Vec<GlyphId16> = names
        .iter()
        .filter_map(|n| name_to_gid.get(n).copied())
        .collect();
    gids.sort();
    gids.dedup();
    CoverageTable::format_1(gids)
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

/// The nested lookup a chain-context rule invokes at input position 0.
fn build_chain_helper(
    r: &ExpandedRemap,
    name_to_gid: &HashMap<String, GlyphId16>,
) -> Option<SubstitutionLookup> {
    let first_sources: Vec<String> = r.source.iter().map(|seq| seq[0].clone()).collect();
    match rule_kind_of(r)? {
        RemapRuleKind::Single => {
            let first_targets: Vec<String> = r.target.iter().map(|seq| seq[0].clone()).collect();
            Some(build_single_subst_from_pairs(&first_sources, &first_targets, name_to_gid))
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
        vec![LigatureSubstFormat1::new(coverage, ligature_sets.into_iter().collect())],
    ))
}
