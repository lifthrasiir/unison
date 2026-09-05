//! Folding a secondary face into the demo page's font.
//!
//! Two faces of one source differ in their **cmap and nothing else**: expansion
//! never filters a glyph by slice, so every face draws from the same glyph set
//! and what a slice changes is which character reaches which glyph (see
//! [`crate::faces`] and [`super::collect::collect_face_cmap`]). The two shipping
//! files are therefore ~99.8% the same bytes, and a demo page that embedded both
//! would pay for the second glyph store to show a different set of cmap entries.
//!
//! So the demo embeds the primary face and carries the others as a **GSUB
//! switch**: one uncontextual single substitution per code point the two faces
//! disagree about, under a stylistic-set feature the page turns on with one CSS
//! declaration. It costs `6 + 2N` bytes of subtable rather than a second font.
//!
//! This is a demo-only fold, the way [`super::build_face_variable`]'s axis is a
//! demo-only axis: `meta bitmap-axis` and the `face` lines are decisions about
//! what the *shipping* files are, and the demo is not one of them.
//!
//! # Why a substitution and not a second cmap
//!
//! The alternative is to give every divergent character a second code point in
//! a Private Use plane and let the page render those instead. It is smaller
//! still — cmap entries rather than a lookup — but the sample panel is a live
//! `<textarea>`: every keystroke would have to be mapped on the way in and
//! unmapped on the way out, and a copy out of the page would yield PUA. A
//! substitution leaves the text the reader's own.
//!
//! # Why the switch runs last
//!
//! The lookup is appended after every group the source declares, so it applies
//! to what shaping produced rather than to what the text held. That matters for
//! exactly one glyph in Unison today — `wsp`, which `x-sitelen-pona.unf`'s
//! `tok-long` rules name while `special.unf` also maps `U+2001`/`U+2003` to it
//! per slice — but the ordering is what makes the general case safe: a `remap`
//! is written against the primary face's names, and it has to run while those
//! are still the names in the buffer.
//!
//! # What the switch cannot carry
//!
//! A code point the primary face does not map has no glyph to substitute *from*,
//! so a face that maps one the primary does not cannot be shown this way
//! ([`FoldDelta::only_other`]). The reverse — a character the primary maps and
//! the secondary does not — is not a substitution either; it is the absence of
//! one, and the page is told about it ([`FoldDelta::unmapped`]) so it can grey
//! the cell out rather than draw a glyph that face has no entry for.
//!
//! Unison is entirely the second case: `map narrow :` appears nowhere, so Term's
//! repertoire is a subset of Regular's.

use std::collections::{BTreeMap, HashMap, HashSet};

/// The last registered stylistic set. `ss01`..`ss20` is the whole range; there
/// is no `ss21`.
const LAST_STYLISTIC_SET: u32 = 20;

/// One secondary face as the demo font carries it.
#[derive(Clone, Debug)]
pub struct FoldedFace {
    /// The `face` id it was declared with.
    pub id: String,
    /// Its own `meta family`, which is the name the page calls it by.
    pub family: String,
    /// The stylistic-set tag that switches the font to this face.
    pub feature: String,
    /// Code points the primary face maps and this one does not, ascending. The
    /// page greys these out; nothing in the font says they are missing, because
    /// a cmap entry is not something a feature can take away.
    pub unmapped: Vec<u32>,
}

/// How one face differs from the primary, in the terms the fold needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct FoldDelta {
    /// `(primary glyph, this face's glyph)` for every code point the two map
    /// differently, by source glyph. Ascending by source name, so a build is
    /// reproducible.
    pub(super) pairs: Vec<(String, String)>,
    /// Code points the primary face maps and this one does not, ascending.
    pub(super) unmapped: Vec<u32>,
    /// Code points only this face maps, ascending. Nothing can be substituted
    /// *to* them, so the fold cannot show them; the caller warns.
    pub(super) only_other: Vec<u32>,
    /// A primary glyph this face replaces with two different glyphs depending
    /// on the code point, as `(source, first target, second target)`. A single
    /// substitution is a function on glyphs and cannot express it, so the source
    /// glyph is left out of [`FoldDelta::pairs`] entirely rather than resolved
    /// one way; the caller warns.
    pub(super) conflicts: Vec<(String, String, String)>,
}

/// `FaceCmap::per_name` read the other way round.
///
/// A code point two names both claim is a duplicate the validation reports; the
/// smaller name wins here so that a build stays reproducible either way.
pub(super) fn by_codepoint(per_name: &HashMap<String, Vec<u32>>) -> BTreeMap<u32, &str> {
    let mut out: BTreeMap<u32, &str> = BTreeMap::new();
    for (name, cps) in per_name {
        for &cp in cps {
            out.entry(cp)
                .and_modify(|n| {
                    if name.as_str() < *n {
                        *n = name.as_str();
                    }
                })
                .or_insert(name.as_str());
        }
    }
    out
}

/// What one secondary face's cmap says that the primary's does not.
pub(super) fn fold_delta(primary: &BTreeMap<u32, &str>, other: &BTreeMap<u32, &str>) -> FoldDelta {
    let mut out = FoldDelta::default();
    // Keyed by source glyph rather than by code point: the substitution is on
    // glyphs, so two code points sharing a primary glyph must agree about what
    // it becomes.
    let mut by_source: BTreeMap<&str, &str> = BTreeMap::new();
    let mut conflicted: HashSet<&str> = HashSet::new();

    for (&cp, &p) in primary {
        let Some(&o) = other.get(&cp) else {
            out.unmapped.push(cp);
            continue;
        };
        if o == p {
            continue;
        }
        match by_source.get(p) {
            Some(&seen) if seen != o => {
                if conflicted.insert(p) {
                    out.conflicts
                        .push((p.to_string(), seen.to_string(), o.to_string()));
                }
            }
            Some(_) => {}
            None => {
                by_source.insert(p, o);
            }
        }
    }
    out.only_other = other.keys().filter(|cp| !primary.contains_key(cp)).copied().collect();

    out.pairs = by_source
        .into_iter()
        .filter(|(src, _)| !conflicted.contains(src))
        .map(|(s, t)| (s.to_string(), t.to_string()))
        .collect();
    out
}

/// `n` stylistic-set tags no feature in `used` claims, highest first.
///
/// Highest first because a source adds stylistic sets from `ss01` upward, the
/// way every font does, while a fold needs a tag that nothing will grow into.
/// `used` is still consulted rather than assumed empty: the collision would be
/// silent — two feature records with one tag, and a shaper takes the first — and
/// it would appear the day the font gained its twentieth stylistic set.
///
/// `None` if the range cannot supply that many, which takes 20 faces.
pub(super) fn allocate_feature_tags(used: &HashSet<String>, n: usize) -> Option<Vec<String>> {
    let tags: Vec<String> = (1..=LAST_STYLISTIC_SET)
        .rev()
        .map(|i| format!("ss{i:02}"))
        .filter(|t| !used.contains(t))
        .take(n)
        .collect();
    (tags.len() == n).then_some(tags)
}

/// Add one switch per secondary face to `gsub_data`, and say what each needs.
///
/// The lookups are appended *after* every group the source declared — see the
/// module docs on why the switch runs last — which
/// [`super::gsub::build_gsub`] guarantees by walking `groups.order` in order.
/// The synthesized group names carry an `@`, which is not in the glyph-name
/// character set, so they cannot collide with a `remap group` the source names.
///
/// A face this cannot fold is skipped with a warning rather than dropped
/// silently: the page would otherwise show the primary face under a Term label.
pub(super) fn fold_secondary_faces(
    docs: &[&crate::document::Document],
    primary: &crate::faces::Face,
    others: &[&crate::faces::Face],
    expansion: &super::expand::Expansion,
    gsub_data: &mut super::GsubData,
    cancel: &crate::cancel::CancelToken,
) -> (Vec<FoldedFace>, Vec<String>) {
    let mut warnings = Vec::new();
    if others.is_empty() {
        return (Vec::new(), warnings);
    }
    let Some(primary_cmap) = super::collect::collect_face_cmap(docs, primary, expansion, cancel)
    else {
        return (Vec::new(), warnings);
    };
    let primary_by_cp = by_codepoint(&primary_cmap.per_name);

    let used: HashSet<String> = gsub_data.features.iter().map(|(tag, ..)| tag.clone()).collect();
    let Some(tags) = allocate_feature_tags(&used, others.len()) else {
        warnings.push(format!(
            "the demo page can carry at most {LAST_STYLISTIC_SET} folded faces minus the stylistic sets the source uses; {} faces is too many",
            others.len(),
        ));
        return (Vec::new(), warnings);
    };

    let mut out = Vec::new();
    for (face, feature) in others.iter().zip(tags) {
        let Some(cmap) = super::collect::collect_face_cmap(docs, face, expansion, cancel) else {
            warnings.push(format!("face '{}' has no cmap to fold", face.label()));
            continue;
        };
        let delta = fold_delta(&primary_by_cp, &by_codepoint(&cmap.per_name));
        for (source, a, b) in &delta.conflicts {
            warnings.push(format!(
                "face '{}' replaces '{source}' with both '{a}' and '{b}', so the demo page cannot switch it",
                face.label(),
            ));
        }
        if !delta.only_other.is_empty() {
            warnings.push(format!(
                "face '{}' maps {} code points the primary face does not, which the demo page cannot show (first: U+{:04X})",
                face.label(),
                delta.only_other.len(),
                delta.only_other[0],
            ));
        }

        let set = format!("@fold-{}", face.id);
        gsub_data.remap_sets.insert(
            set.clone(),
            delta
                .pairs
                .iter()
                .map(|(s, t)| super::ExpandedRemap {
                    origin: None,
                    lookbehind: Vec::new(),
                    source: vec![vec![s.clone()]],
                    target: vec![vec![t.clone()]],
                    lookahead: Vec::new(),
                })
                .collect(),
        );
        gsub_data.groups.order.push(set.clone());
        gsub_data
            .features
            .push((feature.clone(), vec!["DFLT".to_string()], vec![set]));

        out.push(FoldedFace {
            id: face.id.clone(),
            family: cmap.meta.family().to_string(),
            feature,
            unmapped: delta.unmapped,
        });
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmap(pairs: &[(u32, &'static str)]) -> BTreeMap<u32, &'static str> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn a_face_that_maps_a_character_to_another_glyph_becomes_one_pair() {
        let primary = cmap(&[(0x41, "a"), (0x42, "b")]);
        let other = cmap(&[(0x41, "a-half"), (0x42, "b")]);
        let d = fold_delta(&primary, &other);
        assert_eq!(d.pairs, vec![("a".to_string(), "a-half".to_string())]);
        assert!(d.unmapped.is_empty() && d.only_other.is_empty() && d.conflicts.is_empty());
    }

    #[test]
    fn two_code_points_on_one_glyph_state_the_pair_once() {
        let primary = cmap(&[(0x2001, "wsp"), (0x2003, "wsp")]);
        let other = cmap(&[(0x2001, "sp"), (0x2003, "sp")]);
        assert_eq!(
            fold_delta(&primary, &other).pairs,
            vec![("wsp".to_string(), "sp".to_string())]
        );
    }

    #[test]
    fn a_glyph_the_face_replaces_two_ways_is_dropped_rather_than_resolved() {
        // A single substitution is a function on glyphs: it cannot send `wsp`
        // to `sp` for one code point and to `nbsp` for another.
        let primary = cmap(&[(0x2001, "wsp"), (0x2003, "wsp"), (0x41, "a")]);
        let other = cmap(&[(0x2001, "sp"), (0x2003, "nbsp"), (0x41, "a-half")]);
        let d = fold_delta(&primary, &other);
        assert_eq!(d.pairs, vec![("a".to_string(), "a-half".to_string())]);
        assert_eq!(
            d.conflicts,
            vec![("wsp".to_string(), "sp".to_string(), "nbsp".to_string())]
        );
    }

    #[test]
    fn a_character_the_face_does_not_map_is_reported_rather_than_substituted() {
        let primary = cmap(&[(0x41, "a"), (0xFB13, "hy-lig")]);
        let other = cmap(&[(0x41, "a")]);
        let d = fold_delta(&primary, &other);
        assert!(d.pairs.is_empty());
        assert_eq!(d.unmapped, vec![0xFB13]);
    }

    #[test]
    fn a_character_only_the_face_maps_cannot_be_folded() {
        let primary = cmap(&[(0x41, "a")]);
        let other = cmap(&[(0x41, "a"), (0x42, "b")]);
        assert_eq!(fold_delta(&primary, &other).only_other, vec![0x42]);
    }

    #[test]
    fn tags_are_taken_from_the_top_of_the_stylistic_set_range() {
        let used = HashSet::new();
        assert_eq!(allocate_feature_tags(&used, 2).unwrap(), ["ss20", "ss19"]);
    }

    #[test]
    fn a_tag_the_source_already_uses_is_skipped() {
        let used: HashSet<String> = ["ss20", "ss18", "ccmp"].iter().map(|s| s.to_string()).collect();
        assert_eq!(allocate_feature_tags(&used, 3).unwrap(), ["ss19", "ss17", "ss16"]);
    }

    #[test]
    fn there_are_only_twenty_stylistic_sets() {
        assert!(allocate_feature_tags(&HashSet::new(), 20).is_some());
        assert!(allocate_feature_tags(&HashSet::new(), 21).is_none());
        let used: HashSet<String> = ["ss01"].iter().map(|s| s.to_string()).collect();
        assert!(allocate_feature_tags(&used, 20).is_none());
    }
}
