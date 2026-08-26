//! GPOS/GDEF generation from `anchor` features (mark attachment).

use super::gsub::{build_single_subst_from_pairs, make_coverage};
use super::tables::{ScriptFeatures, build_script_records, make_tag, parse_script_lang};
use super::*;

pub(super) struct AnchorGposData {
    pub(super) gpos: Option<Gpos>,
    pub(super) gdef: Gdef,
    /// Per-feature-tag GSUB lookups for anchor-based substitution.
    /// Each entry: (feature_tag, scripts, lookups).
    pub(super) feature_lookups: Vec<(String, Vec<String>, Vec<SubstitutionLookup>)>,
    /// Mark glyph sets for GDEF MarkGlyphSets table, used by
    /// USE_MARK_FILTERING_SET on mark-subst lookups.
    pub(super) mark_glyph_sets: Vec<CoverageTable>,
    /// Base substitution entries: (source, target, anchor_name, mark names the
    /// rule keys on).
    #[cfg(test)]
    pub(super) base_subst_entries: Vec<(String, String, String, Vec<String>)>,
    /// Mark substitution entries: (mark, mark_alt, anchor_name, backtrack_bases).
    #[cfg(test)]
    pub(super) mark_subst_entries: Vec<(String, String, String, Vec<String>)>,
}

/// Converts an anchor point's grid position to font units, applying the
/// glyph's left/top offsets. Grid rows grow downward while font units grow
/// upward from the baseline, so the row is flipped against `ascent`.
///
/// A *ranged* anchor becomes a point under its class's [`AnchorAlign`], the
/// same reduction on the `+` side and the `-` side — that is what makes the
/// difference the shaper computes mean anything, and it is why the alignment
/// is the class's and not a glyph's. The default reduction takes the low end
/// of each axis, which is what this did before there was one to state.
fn anchor_font_units(
    pt: &GlyphPoint,
    align: AnchorAlign,
    scale: f32,
    ascent: u16,
    left_offset: i16,
    top_offset: i16,
) -> (i16, i16) {
    let (col, row) = pt.aligned_point(align);
    let x = (col * scale).round() as i16 + left_offset;
    let y = ((ascent as f32 - row) * scale).round() as i16 - top_offset;
    (x, y)
}

/// The reduction one anchor class states, defaulting for a class no
/// declaration named — a `-anchor` can carry a name the feature list never
/// mentions, and the default is what it had before `align` existed.
fn align_of(aligns: &HashMap<&str, AnchorAlign>, anchor_name: &str) -> AnchorAlign {
    aligns.get(anchor_name).copied().unwrap_or_default()
}

/// A base (or mark2) glyph with one attachment anchor per mark class, in font
/// units; `None` where the glyph offers no anchor for that class.
struct AnchoredGlyph {
    gid: GlyphId16,
    anchors: Vec<Option<(i16, i16)>>,
}

/// The `source`/`target` glyph names of one ccmp anchor group, in step.
///
/// One group is one rule: the bases it substitutes, and the mark size that
/// reaches them. Grouping by size and not by anchor name alone is what lets a
/// base offer several slots — the marks that fit one slot must not drag in the
/// alternative built for another.
#[derive(Default)]
struct CcmpGroup {
    sources: Vec<String>,
    targets: Vec<String>,
}

/// A ccmp anchor group's key: the class, and the mark footprints its rule is
/// for, ascending. `None` reaches every mark of the class.
type CcmpKey = (String, Option<Vec<AnchorSize>>);

/// A `+`/`-` anchor's `(width, height)` in grid cells — what a base and a mark
/// are matched on.
type AnchorSize = (u16, u16);

/// One base's substitution: the glyph, the alternative it gives way to, the
/// anchor class, and the mark footprints that reach it.
type CcmpEntry = (String, String, String, Option<Vec<AnchorSize>>);

/// Does a slot hold a mark of `mark`? A `+` range is the room a base hands
/// over and a `-` range the room a mark takes, so it does when it is at least
/// as big on both axes.
fn slot_holds(slot: &GlyphPoint, mark: AnchorSize) -> bool {
    slot.width() >= mark.0 && slot.height() >= mark.1
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

    // The classes this build has, in declaration order and each named once. A
    // class may be declared more than once — offered to two scripts, or a line
    // left in beside the one meant to replace it — and every loop below is per
    // class, so the second naming would repeat the work of the first.
    let mut anchor_names: Vec<String> = Vec::new();
    for feature in &gsub_data.anchor_features {
        if !anchor_names.contains(&feature.anchor) {
            anchor_names.push(feature.anchor.clone());
        }
    }

    // The name each gid carries, for the entry lists the tests read. Built once
    // rather than by scanning the glyph set per gid: the scan is quadratic in a
    // font this size and it is the test build's own cost, which is `cargo test`
    // waiting for something the font never contains.
    #[cfg(test)]
    let gid_to_name: HashMap<GlyphId16, &str> = glyphs
        .iter()
        .filter_map(|g| name_to_gid.get(&g.name).map(|&gid| (gid, g.name.as_str())))
        .collect();

    let mut all_scripts: Vec<String> = Vec::new();
    for feature in &gsub_data.anchor_features {
        for s in &feature.scripts {
            if !all_scripts.contains(s) {
                all_scripts.push(s.clone());
            }
        }
    }

    // Assign anchor classes: each unique anchor name gets a class. The number
    // is how far the map has filled and not the name's place in any list, so
    // that it stays an index into the per-class arrays `num_classes` sizes —
    // counting declarations instead handed a class a number past their end the
    // moment one name was declared twice.
    let mut anchor_class_map: HashMap<String, u16> = HashMap::new();
    for name in &anchor_names {
        let next = anchor_class_map.len() as u16;
        anchor_class_map.entry(name.clone()).or_insert(next);
    }
    let num_classes = anchor_class_map.len() as u16;

    // Classify glyphs: mark glyphs have `-anchor` anchors, base glyphs have `+anchor`.
    // For mark-to-mark: a mark glyph with `+anchor` serves as mark2 (the base mark).
    let mut mark_gids: Vec<(GlyphId16, u16, i16, i16)> = Vec::new(); // (gid, class, x, y)
    let mut base_gids: Vec<AnchoredGlyph> = Vec::new();
    let mut mark2_gids: Vec<AnchoredGlyph> = Vec::new();

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
    // (source, target, anchor_name, mark size the rule keys on). The size is
    // `None` for a base with a single alternative, which is reached by every
    // mark of its class — the rule it has always had, and the only one a base
    // offering one slot can want.
    let mut ccmp_entries: Vec<CcmpEntry> = Vec::new();

    // Map anchor_name → (feature_tag, scripts) from the declarations.
    let mut anchor_to_feature: HashMap<String, (String, Vec<String>)> = HashMap::new();
    for feature in &gsub_data.anchor_features {
        anchor_to_feature
            .entry(feature.anchor.clone())
            .or_insert_with(|| (feature.tag.clone(), feature.scripts.clone()));
    }

    // The footprints the marks of each class take, ascending — what the bases
    // of that class are asked to hold.
    let mut class_mark_sizes: HashMap<&str, Vec<AnchorSize>> = HashMap::new();
    for anchor_name in &anchor_names {
        let minus_name = format!("-{anchor_name}");
        let mut sizes: Vec<AnchorSize> = glyphs
            .iter()
            .filter(|g| g.mark)
            .flat_map(|g| g.declared_anchors.iter())
            .filter(|p| p.position == minus_name)
            .map(|p| (p.width(), p.height()))
            .collect();
        sizes.sort_unstable();
        sizes.dedup();
        class_mark_sizes.insert(anchor_name.as_str(), sizes);
    }

    // The reduction each anchor class applies to a ranged anchor, on both its
    // `+` and its `-` side.
    let anchor_align: HashMap<&str, AnchorAlign> = gsub_data
        .anchor_features
        .iter()
        .map(|f| (f.anchor.as_str(), f.align))
        .collect();

    for g in glyphs {
        let Some(&gid) = name_to_gid.get(&g.name) else {
            continue;
        };
        let loff = g.left_offset;
        let toff = g.top_offset;

        if g.mark {
            mark_gid_set.insert(gid);

            // Mark glyphs: a `-anchor` decides the mark class, and a mark gets
            // exactly one. Declared anchors are consulted first, because the
            // forwarded set may also carry an unrelated `-anchor` from a ref
            // (`dia-above` refs `dia-below`) and the loop below takes whichever
            // anchor name comes first — an order that means nothing here.
            // A mark that declares none of them falls back to what `inherit`
            // forwarded, so a mark composed purely out of ref'd marks (a merged
            // accent pair, say) still attaches instead of silently dropping out
            // of the coverage. `resolved_anchors` is already the exposed set, so
            // that fallback respects `inherit` on its own.
            for source in [&g.declared_anchors, &g.resolved_anchors] {
                let found = anchor_names.iter().find_map(|anchor_name| {
                    let minus_name = format!("-{anchor_name}");
                    source
                        .iter()
                        .find(|p| p.position == minus_name)
                        .map(|pt| (anchor_class_map[anchor_name], anchor_name.as_str(), pt))
                });
                if let Some((class, anchor_name, pt)) = found {
                    let align = align_of(&anchor_align, anchor_name);
                    let (x, y) = anchor_font_units(pt, align, scale, ascent, loff, toff);
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
                    let align = align_of(&anchor_align, anchor_name);
                    let (x, y) = anchor_font_units(pt, align, scale, ascent, loff, toff);
                    plus_anchors[class] = Some((x, y));
                    has_any = true;
                }
            }
            if has_any {
                mark2_gids.push(AnchoredGlyph {
                    gid,
                    anchors: plus_anchors,
                });
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
                let class = anchor_class_map[anchor_name] as usize;
                let align = align_of(&anchor_align, anchor_name);

                // The slots this base can offer: its own, and one per size its
                // alternatives add. First alternative of a given size wins, as
                // on the mark side; `issues::anchors` warns about the ones that
                // loses to.
                let own_pt = g.declared_anchors.iter().find(|p| p.position == plus_name);
                let mut alt_slots: Vec<(&String, &GlyphPoint)> = Vec::new();
                if let Some(alts) = alt_index.get(&g.name) {
                    for (alt_name, alt_anchors) in alts {
                        if let Some(pt) = alt_anchors.iter().find(|p| p.position == plus_name)
                            && !alt_slots.iter().any(|(_, seen)| seen.size_matches(pt))
                        {
                            alt_slots.push((alt_name, pt));
                        }
                    }
                }

                // Which alternative a following mark reaches: the *first*
                // one that holds it, in the order the alternatives are named
                // — the same first-one-wins the equal sizes already go by, and
                // the only rule that stays stated where a name order is not a
                // size order. Every alternative that catches marks becomes one
                // rule, keyed on the marks that landed in it.
                //
                // Matching sizes exactly instead left a mark no alternative is
                // drawn for with nothing at all: on a base with no slot of its
                // own that means no anchor, and a mark with no anchor falls
                // back to bearing placement, which in a right-to-left run puts
                // it a whole glyph away. A slot that is too small still beats
                // that, so a mark nothing holds takes the first alternative
                // anyway — unless the base has a slot of its own to keep.
                //
                // A base with one alternative and no slot of its own keeps the
                // size-blind rule it has always had: every mark reaches the
                // only slot there is, and naming them would spell out the same
                // rule at length.
                let mut to_record: Vec<(&String, &GlyphPoint, Option<Vec<AnchorSize>>)> =
                    Vec::new();
                if let Some(pt) = own_pt {
                    let (x, y) = anchor_font_units(pt, align, scale, ascent, loff, toff);
                    own_plus[class] = Some((x, y));
                    has_own = true;
                }
                // The base's own slot leads: it is the glyph as it stands, so
                // it is kept for every mark it holds and an alternative is
                // reached only by the ones it cannot. An alternative of its
                // size would therefore never be reached at all.
                let candidates: Vec<(Option<&String>, &GlyphPoint)> = own_pt
                    .map(|pt| (None, pt))
                    .into_iter()
                    .chain(
                        alt_slots
                            .iter()
                            .filter(|(_, pt)| !own_pt.is_some_and(|own| own.size_matches(pt)))
                            .map(|(name, pt)| (Some(*name), *pt)),
                    )
                    .collect();

                if own_pt.is_none() && candidates.len() == 1 {
                    let (name, pt) = candidates[0];
                    to_record.push((name.expect("no own slot, so this is an alternative"), pt, None));
                } else if candidates.iter().any(|(name, _)| name.is_some()) {
                    let empty = Vec::new();
                    let mark_sizes = class_mark_sizes.get(anchor_name.as_str()).unwrap_or(&empty);
                    let mut caught: Vec<(&String, &GlyphPoint, Vec<AnchorSize>)> = Vec::new();
                    for &mark_size in mark_sizes {
                        // A mark nothing holds still has to attach somewhere:
                        // the first slot, which for a base with one of its own
                        // is that one, left alone.
                        let picked = candidates
                            .iter()
                            .find(|(_, pt)| slot_holds(pt, mark_size))
                            .or(candidates.first());
                        // The base's own slot needs no substitution.
                        let Some((Some(alt_name), pt)) = picked else {
                            continue;
                        };
                        match caught.iter_mut().find(|(name, _, _)| name == alt_name) {
                            Some((_, _, sizes)) => sizes.push(mark_size),
                            None => caught.push((alt_name, pt, vec![mark_size])),
                        }
                    }
                    for (alt_name, pt, sizes) in caught {
                        to_record.push((alt_name, pt, Some(sizes)));
                    }
                } else if own_pt.is_none()
                    && let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name)
                {
                    let (x, y) = anchor_font_units(pt, align, scale, ascent, loff, toff);
                    own_plus[class] = Some((x, y));
                    has_own = true;
                }

                for (alt_name, pt, size_key) in to_record {
                    let size_key = size_key.clone();
                    let alt_g = glyphs.iter().find(|gg| gg.name == *alt_name);
                    let alt_loff = alt_g.map_or(0, |gg| gg.left_offset);
                    let alt_toff = alt_g.map_or(0, |gg| gg.top_offset);
                    let (x, y) = anchor_font_units(pt, align, scale, ascent, alt_loff, alt_toff);
                    let entry = alt_plus_map
                        .entry(alt_name.clone())
                        .or_insert_with(|| vec![None; num_classes as usize]);
                    entry[class] = Some((x, y));

                    if !ccmp_entries
                        .iter()
                        .any(|(s, _, a, k)| s == &g.name && a == anchor_name && *k == size_key)
                    {
                        ccmp_entries.push((
                            g.name.clone(),
                            alt_name.clone(),
                            anchor_name.clone(),
                            size_key.clone(),
                        ));
                    }
                }
            }

            if has_own {
                base_gids.push(AnchoredGlyph {
                    gid,
                    anchors: own_plus,
                });
            }
            for (alt_name, alt_anchors) in &alt_plus_map {
                if let Some(&alt_gid) = name_to_gid.get(alt_name.as_str()) {
                    base_gids.push(AnchoredGlyph {
                        gid: alt_gid,
                        anchors: alt_anchors.clone(),
                    });
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
        for entry in &mut base_gids {
            entry.anchors = used_classes
                .iter()
                .map(|&old_class| entry.anchors.get(old_class as usize).copied().flatten())
                .collect();
        }
        for entry in &mut mark2_gids {
            entry.anchors = used_classes
                .iter()
                .map(|&old_class| entry.anchors.get(old_class as usize).copied().flatten())
                .collect();
        }
        let _ = compact_num_classes; // used implicitly via compacted vectors
    }

    // Sort by GID for coverage tables
    mark_gids.sort_by_key(|&(gid, _, _, _)| gid);
    mark_gids.dedup_by_key(|entry| entry.0);
    base_gids.sort_by_key(|entry| entry.gid);
    base_gids.dedup_by_key(|entry| entry.gid);
    mark2_gids.sort_by_key(|entry| entry.gid);
    mark2_gids.dedup_by_key(|entry| entry.gid);

    // Build GPOS lookups
    let mut gpos_lookups: Vec<PositionLookup> = Vec::new();
    let mut gpos_lookup_indices: Vec<u16> = Vec::new();

    // MarkBasePos (lookup type 4)
    if !mark_gids.is_empty() && !base_gids.is_empty() {
        let mark_coverage =
            CoverageTable::format_1(mark_gids.iter().map(|&(gid, _, _, _)| gid).collect());
        let base_coverage =
            CoverageTable::format_1(base_gids.iter().map(|entry| entry.gid).collect());
        let mark_array = MarkArray::new(
            mark_gids
                .iter()
                .map(|&(_, class, x, y)| MarkRecord::new(class, AnchorTable::format_1(x, y)))
                .collect(),
        );
        let base_array = BaseArray::new(
            base_gids
                .iter()
                .map(|entry| {
                    BaseRecord::new(
                        entry
                            .anchors
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
        let mark1_coverage =
            CoverageTable::format_1(mark_gids.iter().map(|&(gid, _, _, _)| gid).collect());
        let mark2_coverage =
            CoverageTable::format_1(mark2_gids.iter().map(|entry| entry.gid).collect());
        let mark1_array = MarkArray::new(
            mark_gids
                .iter()
                .map(|&(_, class, x, y)| MarkRecord::new(class, AnchorTable::format_1(x, y)))
                .collect(),
        );
        let mark2_array = Mark2Array::new(
            mark2_gids
                .iter()
                .map(|entry| {
                    Mark2Record::new(
                        entry
                            .anchors
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
            script_features
                .entry(script)
                .or_default()
                .push(lang.as_deref(), mark_feat_idx);
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
                script_features
                    .entry(script)
                    .or_default()
                    .push(lang.as_deref(), mkmk_feat_idx);
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
        let mut tag_groups: BTreeMap<String, BTreeMap<CcmpKey, CcmpGroup>> = BTreeMap::new();
        for (source, target, anchor_name, size_key) in &ccmp_entries {
            let (tag, _) = anchor_to_feature
                .get(anchor_name)
                .cloned()
                .unwrap_or_else(|| ("ccmp".to_string(), vec!["DFLT".to_string()]));
            let group = tag_groups
                .entry(tag)
                .or_default()
                .entry((anchor_name.clone(), size_key.clone()))
                .or_default();
            if !group.sources.contains(source) {
                group.sources.push(source.clone());
                group.targets.push(target.clone());
            }
        }

        for (tag, anchor_groups) in &tag_groups {
            let scripts: Vec<String> = gsub_data
                .anchor_features
                .iter()
                .filter(|f| &f.tag == tag)
                .flat_map(|f| f.scripts.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .fold(Vec::new(), |mut acc, s| {
                    if !acc.contains(&s) {
                        acc.push(s);
                    }
                    acc
                });

            let mut lookups: Vec<SubstitutionLookup> = Vec::new();
            for ((anchor_name, size_key), CcmpGroup { sources, targets }) in anchor_groups {
                let minus_name = format!("-{anchor_name}");
                let marks_of_this_size = |g: &CollectedGlyph| {
                    g.mark
                        && g.declared_anchors.iter().any(|p| {
                            p.position == minus_name
                                && size_key
                                    .as_ref()
                                    .is_none_or(|sizes| sizes.contains(&(p.width(), p.height())))
                        })
                };
                let mark_coverage = CoverageTable::format_1({
                    let mut gids: Vec<GlyphId16> = glyphs
                        .iter()
                        .filter(|g| marks_of_this_size(g))
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
                *sc = ChainedSequenceContext::Format3(ChainedSequenceContextFormat3::new(
                    vec![],
                    vec![source_coverage],
                    vec![mark_coverage],
                    vec![SequenceLookupRecord {
                        sequence_index: 0,
                        lookup_list_index: subst_idx as u16,
                    }],
                ));
                lookups.push(SubstitutionLookup::ChainContextual(Lookup::new(
                    LookupFlag::empty(),
                    vec![sc],
                )));
            }

            feature_lookups.push((tag.clone(), scripts, lookups));
        }
    }

    // The mark names each base rule keys on, resolved for the tests: a size is
    // how the rule is built, but what a reader wants to know is which marks
    // reach it.
    #[cfg(test)]
    let base_subst_entries: Vec<(String, String, String, Vec<String>)> = ccmp_entries
        .iter()
        .map(|(source, target, anchor_name, size_key)| {
            let minus_name = format!("-{anchor_name}");
            let marks: Vec<String> = glyphs
                .iter()
                .filter(|g| {
                    g.mark
                        && g.declared_anchors.iter().any(|p| {
                            p.position == minus_name
                                && size_key
                                    .as_ref()
                                    .is_none_or(|sizes| sizes.contains(&(p.width(), p.height())))
                        })
                })
                .map(|g| g.name.clone())
                .collect();
            (
                source.clone(),
                target.clone(),
                anchor_name.clone(),
                marks,
            )
        })
        .collect();
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
                if !g.mark {
                    continue;
                }
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
            let mut filtering_gids: Vec<GlyphId16> = glyphs
                .iter()
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

            // Every glyph carrying this anchor's `+X`, found once per anchor
            // rather than per (mark, alternative) pair. The backtrack below
            // wants the same answer every time it asks, and asking meant a scan
            // over the whole glyph set with a name lookup per glyph — the
            // product of three loops over a font whose glyph count is now five
            // figures. Declaration order is kept, which is the order the
            // backtrack used to see them in; it sorts and dedups regardless.
            let plus_bases: Vec<(GlyphId16, &crate::document::GlyphPoint)> = glyphs
                .iter()
                .filter_map(|base| {
                    let &gid = name_to_gid.get(&base.name)?;
                    let pt = base
                        .declared_anchors
                        .iter()
                        .find(|p| p.position == plus_name)
                        .or_else(|| {
                            base.resolved_anchors
                                .iter()
                                .find(|p| p.position == plus_name)
                        })?;
                    Some((gid, pt))
                })
                .collect();

            // Collect marks that have alternatives with different `-X` sizes.
            for g in glyphs {
                if !g.mark {
                    continue;
                }
                let Some(&mark_gid) = name_to_gid.get(&g.name) else {
                    continue;
                };
                let Some(mark_minus) = g.declared_anchors.iter().find(|p| p.position == minus_name)
                else {
                    continue;
                };
                let Some(alts) = mark_alt_index.get(&g.name) else {
                    continue;
                };

                for (alt_name, alt_declared) in alts {
                    let Some(&_alt_gid) = name_to_gid.get(alt_name.as_str()) else {
                        continue;
                    };
                    let Some(alt_minus) = alt_declared.iter().find(|p| p.position == minus_name)
                    else {
                        continue;
                    };
                    if alt_minus.size_matches(mark_minus) {
                        continue; // same size, no substitution needed
                    }

                    // Find bases (and mark2 glyphs with `+X`) whose `+X`
                    // matches the alt's `-X` size.  Including marks here
                    // handles mark-to-mark stacking where a second mark
                    // should be substituted based on the first mark's anchor.
                    let mut backtrack_gids: Vec<GlyphId16> = plus_bases
                        .iter()
                        .filter(|(_, pt)| {
                            pt.size_matches(alt_minus) && !pt.size_matches(mark_minus)
                        })
                        .map(|&(gid, _)| gid)
                        .collect();
                    if backtrack_gids.is_empty() {
                        continue;
                    }
                    backtrack_gids.sort();
                    backtrack_gids.dedup();

                    #[cfg(test)]
                    {
                        let bt_names: Vec<String> = backtrack_gids
                            .iter()
                            .filter_map(|gid| gid_to_name.get(gid).map(|n| (*n).to_string()))
                            .collect();
                        mark_subst_entries.push((
                            g.name.clone(),
                            alt_name.clone(),
                            anchor_name.clone(),
                            bt_names,
                        ));
                    }

                    let (tag, _) = anchor_to_feature
                        .get(anchor_name.as_str())
                        .cloned()
                        .unwrap_or_else(|| ("ccmp".to_string(), vec!["DFLT".to_string()]));
                    let scripts: Vec<String> = gsub_data
                        .anchor_features
                        .iter()
                        .filter(|f| f.tag == tag)
                        .flat_map(|f| f.scripts.clone())
                        .fold(Vec::new(), |mut acc, s| {
                            if !acc.contains(&s) {
                                acc.push(s);
                            }
                            acc
                        });

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
                    *sc = ChainedSequenceContext::Format3(ChainedSequenceContextFormat3::new(
                        vec![backtrack_coverage],
                        vec![input_coverage],
                        vec![],
                        vec![SequenceLookupRecord {
                            sequence_index: 0,
                            lookup_list_index: subst_idx as u16,
                        }],
                    ));
                    let chain_lookup = if let Some(set_idx) = filtering_set_idx {
                        let mut lk = Lookup::new(LookupFlag::USE_MARK_FILTERING_SET, vec![sc]);
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

        // Which record each script should carry these lookups on: the one its
        // own LangSys already names with this tag, where it names one.
        //
        // Merging by tag alone and then registering whatever record that found
        // is what left a LangSys naming `ccmp` *twice* once a `remap` had
        // given the script a record of its own — the merge picked the first
        // `ccmp` in the list, which belonged to another script, and added it
        // beside the one already there. A shaper stops at the first record
        // whose tag matches, so the other one is dead: the anchor
        // substitutions in it never run, every base keeps its plain form, and
        // every mark falls back to bearing placement. `feature ccmp for hebr :
        // anchor he-below` beside `feature ccmp for hebr : he-meteg` is the
        // pair that showed it, and `armn`/`grek` were carrying it too.
        let script_targets: Vec<(Tag, Option<u16>)> = scripts
            .iter()
            .map(|script| {
                let script_tag = make_tag(script);
                let named = gsub
                    .script_list
                    .script_records
                    .iter()
                    .find(|sr| sr.script_tag == script_tag)
                    .and_then(|sr| {
                        let lang_sys = (*sr.script.default_lang_sys).as_ref()?;
                        lang_sys.feature_indices.iter().copied().find(|&i| {
                            gsub.feature_list
                                .feature_records
                                .get(i as usize)
                                .is_some_and(|fr| fr.feature_tag == feat_tag)
                        })
                    });
                (script_tag, named)
            })
            .collect();

        // The scripts that name none share one record: an existing one with
        // the tag (its other scripts pick the lookups up too, which the
        // coverage tables make harmless), or a new one. A feature naming no
        // script still wants a record, so that its lookups are not orphaned.
        let needs_shared =
            script_targets.is_empty() || script_targets.iter().any(|(_, idx)| idx.is_none());
        let shared_idx = needs_shared.then(|| {
            match gsub
                .feature_list
                .feature_records
                .iter()
                .position(|fr| fr.feature_tag == feat_tag)
            {
                Some(idx) => idx as u16,
                None => {
                    gsub.feature_list
                        .feature_records
                        .push(FeatureRecord::new(feat_tag, Feature::new(None, vec![])));
                    (gsub.feature_list.feature_records.len() - 1) as u16
                }
            }
        });

        let mut targets: Vec<u16> = script_targets.iter().filter_map(|(_, idx)| *idx).collect();
        targets.extend(shared_idx);
        targets.sort_unstable();
        targets.dedup();
        for idx in targets {
            let indices = &mut gsub.feature_list.feature_records[idx as usize]
                .feature
                .lookup_list_indices;
            for lookup_idx in &chain_indices {
                if !indices.contains(lookup_idx) {
                    indices.push(*lookup_idx);
                }
            }
        }

        // Every script the feature names has to reach a record — also when
        // the record already existed: a `remap` with the same tag registers
        // only its own scripts, and the merged lookups would otherwise never
        // apply in the ones it did not cover. A script that already named one
        // is done; registering the shared record beside it is the duplicate.
        for (script_tag, named) in &script_targets {
            if named.is_some() {
                continue;
            }
            let script_tag = *script_tag;
            let Some(feat_idx) = shared_idx else { continue };

            let existing = gsub
                .script_list
                .script_records
                .iter_mut()
                .find(|sr| sr.script_tag == script_tag);

            if let Some(sr) = existing {
                if let Some(ref mut default_ls) = *sr.script.default_lang_sys {
                    if !default_ls.feature_indices.contains(&feat_idx) {
                        default_ls.feature_indices.push(feat_idx);
                    }
                } else {
                    sr.script.default_lang_sys = Some(LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: vec![feat_idx],
                    })
                    .into();
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

        // ScriptRecords must stay sorted by tag: the shaper binary-searches
        // them, and one appended out of order (`build_script_records` emits
        // them sorted) makes records around it unfindable — 'latn' vanishing
        // took its ROM/MOL LangSys, and the `locl` substitutions, with it.
        gsub.script_list
            .script_records
            .sort_by_key(|sr| sr.script_tag);
    }
}
