//! GPOS/GDEF generation from `anchor` features (mark attachment).

use super::*;
use super::gsub::{build_single_subst_from_pairs, make_coverage};
use super::tables::{ScriptFeatures, build_script_records, make_tag, parse_script_lang};

pub(super) struct AnchorGposData {
    pub(super) gpos: Option<Gpos>,
    pub(super) gdef: Gdef,
    /// Per-feature-tag GSUB lookups for anchor-based substitution.
    /// Each entry: (feature_tag, scripts, lookups).
    pub(super) feature_lookups: Vec<(String, Vec<String>, Vec<SubstitutionLookup>)>,
    /// Mark glyph sets for GDEF MarkGlyphSets table, used by
    /// USE_MARK_FILTERING_SET on mark-subst lookups.
    pub(super) mark_glyph_sets: Vec<CoverageTable>,
    /// Base substitution entries: (source, target, anchor_name).
    #[cfg(test)]
    pub(super) base_subst_entries: Vec<(String, String, String)>,
    /// Mark substitution entries: (mark, mark_alt, anchor_name, backtrack_bases).
    #[cfg(test)]
    pub(super) mark_subst_entries: Vec<(String, String, String, Vec<String>)>,
}

/// Converts an anchor point's grid position to font units, applying the
/// glyph's left/top offsets. Grid rows grow downward while font units grow
/// upward from the baseline, so the row is flipped against `ascent`.
fn anchor_font_units(
    pt: &GlyphPoint,
    scale: f32,
    ascent: u16,
    left_offset: i16,
    top_offset: i16,
) -> (i16, i16) {
    let x = (pt.col as f32 * scale).round() as i16 + left_offset;
    let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - top_offset;
    (x, y)
}

pub(super) fn build_anchor_gpos(
    glyphs: &[CollectedGlyph],
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
    scale: f32,
    ascent: u16,
) -> AnchorGposData {
    if gsub_data.anchor_features.is_empty() {
        return AnchorGposData {
            gpos: None,
            gdef: Gdef::default(),
            feature_lookups: Vec::new(),
            mark_glyph_sets: Vec::new(),
            #[cfg(test)]
            base_subst_entries: Vec::new(),
            #[cfg(test)]
            mark_subst_entries: Vec::new(),
        };
    }

    let anchor_names: Vec<String> = gsub_data
        .anchor_features
        .iter()
        .map(|(_, _, a)| a.clone())
        .collect();

    let mut all_scripts: Vec<String> = Vec::new();
    for (_, scripts, _) in &gsub_data.anchor_features {
        for s in scripts {
            if !all_scripts.contains(s) {
                all_scripts.push(s.clone());
            }
        }
    }

    // Assign anchor classes: each unique anchor name (from feature declarations) gets a class.
    let mut anchor_class_map: HashMap<String, u16> = HashMap::new();
    for (i, name) in anchor_names.iter().enumerate() {
        anchor_class_map.entry(name.clone()).or_insert(i as u16);
    }
    let num_classes = anchor_class_map.len() as u16;

    // Classify glyphs: mark glyphs have `-anchor` anchors, base glyphs have `+anchor`.
    // For mark-to-mark: a mark glyph with `+anchor` serves as mark2 (the base mark).
    let mut mark_gids: Vec<(GlyphId16, u16, i16, i16)> = Vec::new(); // (gid, class, x, y)
    let mut base_gids: Vec<(GlyphId16, Vec<Option<(i16, i16)>>)> = Vec::new();
    let mut mark2_gids: Vec<(GlyphId16, Vec<Option<(i16, i16)>>)> = Vec::new();

    // Collect all mark glyph GIDs for ccmp/GDEF
    let mut mark_gid_set: HashSet<GlyphId16> = HashSet::new();

    // Build alternative index from glyphs: name:variant → base_name
    let mut alt_index: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
    for g in glyphs {
        let mut prefix = g.name.as_str();
        while let Some(colon_pos) = prefix.rfind(':') {
            prefix = &prefix[..colon_pos];
            alt_index
                .entry(prefix.to_string())
                .or_default()
                .push((g.name.clone(), g.resolved_anchors.clone()));
        }
    }
    for alts in alt_index.values_mut() {
        alts.sort_by(|(a, _), (b, _)| a.cmp(b));
    }

    // Track base glyphs that need alternative substitution, grouped
    // by anchor name.  Each entry also records the feature tag so the
    // resulting lookups land under the correct OpenType feature.
    let mut ccmp_entries: Vec<(String, String, String)> = Vec::new(); // (source, target, anchor_name)

    // Map anchor_name → (feature_tag, scripts) from the declarations.
    let mut anchor_to_feature: HashMap<String, (String, Vec<String>)> = HashMap::new();
    for (tag, scripts, anchor_name) in &gsub_data.anchor_features {
        anchor_to_feature.entry(anchor_name.clone())
            .or_insert_with(|| (tag.clone(), scripts.clone()));
    }

    for g in glyphs {
        let Some(&gid) = name_to_gid.get(&g.name) else { continue };
        let loff = g.left_offset;
        let toff = g.top_offset;

        if g.mark {
            mark_gid_set.insert(gid);

            // Mark glyphs: look for `-anchor` anchors in declared_anchors only
            // (not forwarded anchors from refs) to determine mark class.
            for anchor_name in anchor_names.iter() {
                let minus_name = format!("-{anchor_name}");
                if let Some(pt) = g.declared_anchors.iter().find(|p| p.position == minus_name) {
                    let class = anchor_class_map[anchor_name];
                    let (x, y) = anchor_font_units(pt, scale, ascent, loff, toff);
                    mark_gids.push((gid, class, x, y));
                    break;
                }
            }

            // Mark-to-mark: mark glyphs with `+anchor` anchors
            let mut plus_anchors: Vec<Option<(i16, i16)>> = vec![None; num_classes as usize];
            let mut has_any = false;
            for anchor_name in anchor_names.iter() {
                let plus_name = format!("+{anchor_name}");
                if let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name) {
                    let class = anchor_class_map[anchor_name] as usize;
                    let (x, y) = anchor_font_units(pt, scale, ascent, loff, toff);
                    plus_anchors[class] = Some((x, y));
                    has_any = true;
                }
            }
            if has_any {
                mark2_gids.push((gid, plus_anchors));
            }
        } else {
            // Base glyphs: look for `+anchor` anchors (direct or via alternatives).
            // Own anchors go on the original glyph; anchors provided only by
            // alternatives go on the alt glyph (which ccmp substitutes in).
            let mut own_plus: Vec<Option<(i16, i16)>> = vec![None; num_classes as usize];
            let mut has_own = false;
            // alt_name → plus_anchors for each alternative that provides anchors
            let mut alt_plus_map: HashMap<String, Vec<Option<(i16, i16)>>> = HashMap::new();

            for anchor_name in anchor_names.iter() {
                let plus_name = format!("+{anchor_name}");
                if let Some(pt) = g.declared_anchors.iter().find(|p| p.position == plus_name) {
                    let class = anchor_class_map[anchor_name] as usize;
                    let (x, y) = anchor_font_units(pt, scale, ascent, loff, toff);
                    own_plus[class] = Some((x, y));
                    has_own = true;
                } else if let Some(alts) = alt_index.get(&g.name) {
                    let mut alt_found = false;
                    for (alt_name, alt_anchors) in alts {
                        if let Some(pt) = alt_anchors.iter().find(|p| p.position == plus_name) {
                            let class = anchor_class_map[anchor_name] as usize;
                            let alt_g = glyphs.iter().find(|gg| gg.name == *alt_name);
                            let alt_loff = alt_g.map_or(0, |gg| gg.left_offset);
                            let alt_toff = alt_g.map_or(0, |gg| gg.top_offset);
                            let (x, y) = anchor_font_units(pt, scale, ascent, alt_loff, alt_toff);
                            let entry = alt_plus_map
                                .entry(alt_name.clone())
                                .or_insert_with(|| vec![None; num_classes as usize]);
                            entry[class] = Some((x, y));

                            if !ccmp_entries.iter().any(|(s, _, a)| s == &g.name && a == anchor_name) {
                                ccmp_entries.push((g.name.clone(), alt_name.clone(), anchor_name.clone()));
                            }
                            alt_found = true;
                            break;
                        }
                    }
                    if !alt_found
                        && let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name) {
                            let class = anchor_class_map[anchor_name] as usize;
                            let (x, y) = anchor_font_units(pt, scale, ascent, loff, toff);
                            own_plus[class] = Some((x, y));
                            has_own = true;
                        }
                } else if let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name) {
                    let class = anchor_class_map[anchor_name] as usize;
                    let (x, y) = anchor_font_units(pt, scale, ascent, loff, toff);
                    own_plus[class] = Some((x, y));
                    has_own = true;
                }
            }

            if has_own {
                base_gids.push((gid, own_plus));
            }
            for (alt_name, alt_anchors) in &alt_plus_map {
                if let Some(&alt_gid) = name_to_gid.get(alt_name.as_str()) {
                    base_gids.push((alt_gid, alt_anchors.clone()));
                }
            }
        }
    }

    // Compact class indices: only keep classes that are actually used by marks.
    let used_classes: Vec<u16> = {
        let mut s: Vec<u16> = mark_gids.iter().map(|&(_, class, _, _)| class).collect();
        s.sort();
        s.dedup();
        s
    };
    if !used_classes.is_empty() {
        let class_remap: HashMap<u16, u16> = used_classes
            .iter()
            .enumerate()
            .map(|(new_idx, &old_idx)| (old_idx, new_idx as u16))
            .collect();
        let compact_num_classes = used_classes.len();

        for entry in &mut mark_gids {
            entry.1 = class_remap[&entry.1];
        }
        for (_, anchors) in &mut base_gids {
            let compacted: Vec<Option<(i16, i16)>> = used_classes
                .iter()
                .map(|&old_class| anchors.get(old_class as usize).copied().flatten())
                .collect();
            *anchors = compacted;
        }
        for (_, anchors) in &mut mark2_gids {
            let compacted: Vec<Option<(i16, i16)>> = used_classes
                .iter()
                .map(|&old_class| anchors.get(old_class as usize).copied().flatten())
                .collect();
            *anchors = compacted;
        }
        let _ = compact_num_classes; // used implicitly via compacted vectors
    }

    // Sort by GID for coverage tables
    mark_gids.sort_by_key(|&(gid, _, _, _)| gid);
    mark_gids.dedup_by_key(|entry| entry.0);
    base_gids.sort_by_key(|&(gid, _)| gid);
    base_gids.dedup_by_key(|entry| entry.0);
    mark2_gids.sort_by_key(|&(gid, _)| gid);
    mark2_gids.dedup_by_key(|entry| entry.0);

    // Build GPOS lookups
    let mut gpos_lookups: Vec<PositionLookup> = Vec::new();
    let mut gpos_lookup_indices: Vec<u16> = Vec::new();

    // MarkBasePos (lookup type 4)
    if !mark_gids.is_empty() && !base_gids.is_empty() {
        let mark_coverage = CoverageTable::format_1(
            mark_gids.iter().map(|&(gid, _, _, _)| gid).collect(),
        );
        let base_coverage = CoverageTable::format_1(
            base_gids.iter().map(|&(gid, _)| gid).collect(),
        );
        let mark_array = MarkArray::new(
            mark_gids
                .iter()
                .map(|&(_, class, x, y)| {
                    MarkRecord::new(class, AnchorTable::format_1(x, y))
                })
                .collect(),
        );
        let base_array = BaseArray::new(
            base_gids
                .iter()
                .map(|(_, anchors)| {
                    BaseRecord::new(
                        anchors
                            .iter()
                            .map(|opt| opt.map(|(x, y)| AnchorTable::format_1(x, y)))
                            .collect(),
                    )
                })
                .collect(),
        );
        let lookup_idx = gpos_lookups.len() as u16;
        gpos_lookups.push(PositionLookup::MarkToBase(Lookup::new(
            LookupFlag::empty(),
            vec![MarkBasePosFormat1::new(
                mark_coverage,
                base_coverage,
                mark_array,
                base_array,
            )],
        )));
        gpos_lookup_indices.push(lookup_idx);
    }

    // MarkMarkPos (lookup type 6)
    if !mark_gids.is_empty() && !mark2_gids.is_empty() {
        let mark1_coverage = CoverageTable::format_1(
            mark_gids.iter().map(|&(gid, _, _, _)| gid).collect(),
        );
        let mark2_coverage = CoverageTable::format_1(
            mark2_gids.iter().map(|&(gid, _)| gid).collect(),
        );
        let mark1_array = MarkArray::new(
            mark_gids
                .iter()
                .map(|&(_, class, x, y)| {
                    MarkRecord::new(class, AnchorTable::format_1(x, y))
                })
                .collect(),
        );
        let mark2_array = Mark2Array::new(
            mark2_gids
                .iter()
                .map(|(_, anchors)| {
                    Mark2Record::new(
                        anchors
                            .iter()
                            .map(|opt| opt.map(|(x, y)| AnchorTable::format_1(x, y)))
                            .collect(),
                    )
                })
                .collect(),
        );
        let lookup_idx = gpos_lookups.len() as u16;
        gpos_lookups.push(PositionLookup::MarkToMark(Lookup::new(
            LookupFlag::empty(),
            vec![MarkMarkPosFormat1::new(
                mark1_coverage,
                mark2_coverage,
                mark1_array,
                mark2_array,
            )],
        )));
        gpos_lookup_indices.push(lookup_idx);
    }

    // Build GPOS table
    let gpos = if !gpos_lookups.is_empty() {
        let mut feature_records: Vec<FeatureRecord> = Vec::new();
        let mut script_features: BTreeMap<String, ScriptFeatures> = BTreeMap::new();

        // mark feature
        let mark_feat_idx = feature_records.len() as u16;
        feature_records.push(FeatureRecord::new(
            Tag::new(b"mark"),
            Feature::new(None, gpos_lookup_indices.clone()),
        ));
        for target in &all_scripts {
            let (script, lang) = parse_script_lang(target);
            script_features.entry(script).or_default().push(lang.as_deref(), mark_feat_idx);
        }

        // mkmk feature (if MarkMarkPos exists)
        if gpos_lookup_indices.len() > 1 {
            let mkmk_feat_idx = feature_records.len() as u16;
            feature_records.push(FeatureRecord::new(
                Tag::new(b"mkmk"),
                Feature::new(None, vec![gpos_lookup_indices[1]]),
            ));
            for target in &all_scripts {
                let (script, lang) = parse_script_lang(target);
                script_features.entry(script).or_default().push(lang.as_deref(), mkmk_feat_idx);
            }
        }

        let script_records = build_script_records(&script_features);

        let script_list = ScriptList::new(script_records);
        let feature_list = FeatureList::new(feature_records);
        let lookup_list = PositionLookupList::new(gpos_lookups);

        Some(Gpos::new(script_list, feature_list, lookup_list))
    } else {
        None
    };

    // Build GDEF with mark glyph class
    let gdef = if !mark_gid_set.is_empty() {
        let mut mark_gids_sorted: Vec<GlyphId16> = mark_gid_set.into_iter().collect();
        mark_gids_sorted.sort();

        let mut class_ranges: Vec<ClassRangeRecord> = Vec::new();
        let mut i = 0;
        while i < mark_gids_sorted.len() {
            let start = mark_gids_sorted[i];
            let mut end = start;
            while i + 1 < mark_gids_sorted.len()
                && mark_gids_sorted[i + 1].to_u16() == end.to_u16() + 1
            {
                i += 1;
                end = mark_gids_sorted[i];
            }
            class_ranges.push(ClassRangeRecord::new(start, end, 3)); // 3 = Mark
            i += 1;
        }

        let class_def = ClassDef::Format2(ClassDefFormat2 {
            class_range_records: class_ranges,
        });
        Gdef::new(Some(class_def), None, None, None)
    } else {
        Gdef::default()
    };

    // Build ccmp GSUB lookups, grouped by anchor name.
    // Each anchor gets its own chain context + single subst pair so
    // that the lookahead only includes marks carrying that anchor's
    // `-X` (e.g. only dia-above marks for the "above" anchor, not
    // dia-below marks).
    // Build per-feature GSUB lookups grouped by feature tag, then anchor.
    let mut feature_lookups: Vec<(String, Vec<String>, Vec<SubstitutionLookup>)> = Vec::new();
    if !ccmp_entries.is_empty() {
        // Group entries by feature tag, then by anchor within each tag.
        let mut tag_groups: BTreeMap<String, BTreeMap<String, (Vec<String>, Vec<String>)>> = BTreeMap::new();
        for (source, target, anchor_name) in &ccmp_entries {
            let (tag, _) = anchor_to_feature.get(anchor_name)
                .cloned()
                .unwrap_or_else(|| ("ccmp".to_string(), vec!["DFLT".to_string()]));
            let group = tag_groups.entry(tag).or_default()
                .entry(anchor_name.clone()).or_default();
            if !group.0.contains(source) {
                group.0.push(source.clone());
                group.1.push(target.clone());
            }
        }

        for (tag, anchor_groups) in &tag_groups {
            let scripts: Vec<String> = gsub_data.anchor_features.iter()
                .filter(|(t, _, _)| t == tag)
                .flat_map(|(_, s, _)| s.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .fold(Vec::new(), |mut acc, s| { if !acc.contains(&s) { acc.push(s); } acc });

            let mut lookups: Vec<SubstitutionLookup> = Vec::new();
            for (anchor_name, (sources, targets)) in anchor_groups {
                let minus_name = format!("-{anchor_name}");
                let mark_coverage = CoverageTable::format_1({
                    let mut gids: Vec<GlyphId16> = glyphs
                        .iter()
                        .filter(|g| g.mark && g.declared_anchors.iter().any(|p| p.position == minus_name))
                        .filter_map(|g| name_to_gid.get(&g.name).copied())
                        .collect();
                    gids.sort();
                    gids.dedup();
                    gids
                });

                let subst_lookup = build_single_subst_from_pairs(sources, targets, name_to_gid);
                let subst_idx = lookups.len();
                lookups.push(subst_lookup);

                let source_coverage = make_coverage(sources, name_to_gid);
                let mut sc = SubstitutionChainContext::default();
                *sc = ChainedSequenceContext::Format3(
                    ChainedSequenceContextFormat3::new(
                        vec![],
                        vec![source_coverage],
                        vec![mark_coverage],
                        vec![SequenceLookupRecord {
                            sequence_index: 0,
                            lookup_list_index: subst_idx as u16,
                        }],
                    ),
                );
                lookups.push(SubstitutionLookup::ChainContextual(Lookup::new(
                    LookupFlag::empty(),
                    vec![sc],
                )));
            }

            feature_lookups.push((tag.clone(), scripts, lookups));
        }
    }

    #[cfg(test)]
    let base_subst_entries = ccmp_entries.clone();
    #[cfg(test)]
    let mut mark_subst_entries: Vec<(String, String, String, Vec<String>)> = Vec::new();

    // Mark alternative substitution: when a mark's `-X` anchor doesn't
    // size-match the preceding base's `+X`, substitute with a mark:alt
    // whose `-X` does match.
    //
    // For each anchor, collect (mark, mark:alt) pairs where the alt has
    // a differently-sized `-X`.  Then generate a chain context with
    // backtrack = bases whose `+X` matches the alt's `-X` size.
    let mut mark_glyph_sets: Vec<CoverageTable> = Vec::new();
    {
        let mark_alt_index: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = {
            let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
            for g in glyphs {
                if !g.mark { continue; }
                let mut prefix = g.name.as_str();
                while let Some(colon_pos) = prefix.rfind(':') {
                    prefix = &prefix[..colon_pos];
                    map.entry(prefix.to_string())
                        .or_default()
                        .push((g.name.clone(), g.declared_anchors.clone()));
                }
            }
            for alts in map.values_mut() {
                alts.sort_by(|(a, _), (b, _)| a.cmp(b));
            }
            map
        };

        for anchor_name in &anchor_names {
            let minus_name = format!("-{anchor_name}");
            let plus_name = format!("+{anchor_name}");

            // Mark filtering set: all marks carrying `-X` for this anchor,
            // plus their alternatives.  Registered in GDEF MarkGlyphSets
            // so that USE_MARK_FILTERING_SET on mark-subst lookups causes
            // marks of OTHER anchor classes to be skipped during backtrack.
            let mut filtering_gids: Vec<GlyphId16> = glyphs.iter()
                .filter(|g| g.mark && g.declared_anchors.iter().any(|p| p.position == minus_name))
                .filter_map(|g| name_to_gid.get(&g.name).copied())
                .collect();
            filtering_gids.sort();
            filtering_gids.dedup();
            let filtering_set_idx = if !filtering_gids.is_empty() {
                let idx = mark_glyph_sets.len() as u16;
                mark_glyph_sets.push(CoverageTable::format_1(filtering_gids));
                Some(idx)
            } else {
                None
            };

            // Collect marks that have alternatives with different `-X` sizes.
            for g in glyphs {
                if !g.mark { continue; }
                let Some(&mark_gid) = name_to_gid.get(&g.name) else { continue };
                let Some(mark_minus) = g.declared_anchors.iter().find(|p| p.position == minus_name) else { continue };
                let Some(alts) = mark_alt_index.get(&g.name) else { continue };

                for (alt_name, alt_declared) in alts {
                    let Some(&_alt_gid) = name_to_gid.get(alt_name.as_str()) else { continue };
                    let Some(alt_minus) = alt_declared.iter().find(|p| p.position == minus_name) else { continue };
                    if alt_minus.size_matches(mark_minus) {
                        continue; // same size, no substitution needed
                    }

                    // Find bases (and mark2 glyphs with `+X`) whose `+X`
                    // matches the alt's `-X` size.  Including marks here
                    // handles mark-to-mark stacking where a second mark
                    // should be substituted based on the first mark's anchor.
                    let mut backtrack_gids: Vec<GlyphId16> = Vec::new();
                    for base in glyphs {
                        let Some(&base_gid) = name_to_gid.get(&base.name) else { continue };
                        let plus_pt = base.declared_anchors.iter()
                            .find(|p| p.position == plus_name)
                            .or_else(|| base.resolved_anchors.iter().find(|p| p.position == plus_name));
                        if let Some(pt) = plus_pt
                            && pt.size_matches(alt_minus) && !pt.size_matches(mark_minus) {
                                backtrack_gids.push(base_gid);
                            }
                    }
                    if backtrack_gids.is_empty() {
                        continue;
                    }
                    backtrack_gids.sort();
                    backtrack_gids.dedup();

                    #[cfg(test)]
                    {
                        let bt_names: Vec<String> = backtrack_gids.iter()
                            .filter_map(|gid| {
                                glyphs.iter().find(|g| name_to_gid.get(&g.name) == Some(gid))
                                    .map(|g| g.name.clone())
                            })
                            .collect();
                        mark_subst_entries.push((
                            g.name.clone(), alt_name.clone(), anchor_name.clone(), bt_names,
                        ));
                    }

                    let (tag, _) = anchor_to_feature.get(anchor_name.as_str())
                        .cloned()
                        .unwrap_or_else(|| ("ccmp".to_string(), vec!["DFLT".to_string()]));
                    let scripts: Vec<String> = gsub_data.anchor_features.iter()
                        .filter(|(t, _, _)| *t == tag)
                        .flat_map(|(_, s, _)| s.clone())
                        .fold(Vec::new(), |mut acc, s| { if !acc.contains(&s) { acc.push(s); } acc });

                    // Find or create the feature_lookups entry for this tag.
                    let entry = feature_lookups.iter_mut().find(|(t, _, _)| *t == tag);
                    let lookups = if let Some((_, _, lks)) = entry {
                        lks
                    } else {
                        feature_lookups.push((tag.clone(), scripts, Vec::new()));
                        &mut feature_lookups.last_mut().unwrap().2
                    };

                    let subst_lookup = build_single_subst_from_pairs(
                        std::slice::from_ref(&g.name),
                        std::slice::from_ref(alt_name),
                        name_to_gid,
                    );
                    let subst_idx = lookups.len();
                    lookups.push(subst_lookup);

                    let backtrack_coverage = CoverageTable::format_1(backtrack_gids);
                    let input_coverage = CoverageTable::format_1(vec![mark_gid]);
                    let mut sc = SubstitutionChainContext::default();
                    *sc = ChainedSequenceContext::Format3(
                        ChainedSequenceContextFormat3::new(
                            vec![backtrack_coverage],
                            vec![input_coverage],
                            vec![],
                            vec![SequenceLookupRecord {
                                sequence_index: 0,
                                lookup_list_index: subst_idx as u16,
                            }],
                        ),
                    );
                    let chain_lookup = if let Some(set_idx) = filtering_set_idx {
                        let mut lk = Lookup::new(
                            LookupFlag::USE_MARK_FILTERING_SET,
                            vec![sc],
                        );
                        lk.mark_filtering_set = Some(set_idx);
                        lk
                    } else {
                        Lookup::new(LookupFlag::empty(), vec![sc])
                    };
                    lookups.push(SubstitutionLookup::ChainContextual(chain_lookup));
                }
            }
        }
    }

    AnchorGposData {
        gpos,
        gdef,
        feature_lookups,
        mark_glyph_sets,
        #[cfg(test)]
        base_subst_entries,
        #[cfg(test)]
        mark_subst_entries,
    }
}

/// Merges the anchor-based per-feature GSUB lookups into `gsub` (creating an
/// empty GSUB if none exists), rebasing chained-context lookup indices and
/// folding into existing feature records with the same tag (duplicate
/// feature entries are ignored by some shapers).
pub(super) fn merge_anchor_feature_lookups(
    gsub: &mut Option<Gsub>,
    feature_lookups: Vec<(String, Vec<String>, Vec<SubstitutionLookup>)>,
) {
    for (feature_tag, scripts, lookups) in feature_lookups {
        if lookups.is_empty() {
            continue;
        }
        let gsub = gsub.get_or_insert_with(|| {
            Gsub::new(
                ScriptList::new(vec![]),
                FeatureList::new(vec![]),
                LookupList::new(vec![]),
            )
        });

        let base_idx = gsub.lookup_list.lookups.len() as u16;
        let mut chain_indices: Vec<u16> = Vec::new();
        for (local_idx, mut lookup) in lookups.into_iter().enumerate() {
            let global_idx = base_idx + local_idx as u16;
            if let SubstitutionLookup::ChainContextual(ref mut lk) = lookup {
                for subtable in &mut lk.subtables {
                    if let ChainedSequenceContext::Format3(ref mut f3) = ***subtable {
                        for rec in &mut f3.seq_lookup_records {
                            rec.lookup_list_index += base_idx;
                        }
                    }
                }
                chain_indices.push(global_idx);
            }
            gsub.lookup_list.lookups.push(lookup.into());
        }

        let feat_tag = make_tag(&feature_tag);

        // Try to merge into an existing feature record with the same tag
        // to avoid duplicate feature entries (which some shapers ignore).
        let existing_feat = gsub
            .feature_list
            .feature_records
            .iter_mut()
            .find(|fr| fr.feature_tag == feat_tag);

        if let Some(fr) = existing_feat {
            fr.feature.lookup_list_indices.extend(chain_indices);
        } else {
            let feat_idx = gsub.feature_list.feature_records.len() as u16;
            gsub.feature_list.feature_records.push(FeatureRecord::new(
                feat_tag,
                Feature::new(None, chain_indices),
            ));

            for script in &scripts {
                let script_tag = make_tag(script);

                let existing = gsub
                    .script_list
                    .script_records
                    .iter_mut()
                    .find(|sr| sr.script_tag == script_tag);

                if let Some(sr) = existing {
                    if let Some(ref mut default_ls) = *sr.script.default_lang_sys {
                        default_ls.feature_indices.push(feat_idx);
                    }
                } else {
                    let lang_sys = LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: vec![feat_idx],
                    };
                    let script_obj = Script::new(Some(lang_sys), vec![]);
                    gsub.script_list
                        .script_records
                        .push(ScriptRecord::new(script_tag, script_obj));
                }
            }
        }
    }
}
