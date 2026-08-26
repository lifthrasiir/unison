//! Tests for anchor-driven GPOS/GDEF generation and the `ccmp` substitutions
//! that go with it.

use super::*;

#[test]
fn ttf_build_selects_alternative_glyph_by_anchor_size() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph enclosing 16 16
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
anchor +center 8 7..8

glyph inner 8 16
................
................
................
................
................
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
................
................
................
................
anchor -center 4 8

glyph inner:compressed 8 16
................
................
................
................
................
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
................
................
................
................
anchor -center 4 7..8

glyph combo
ref enclosing
ref inner
map a = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
    // inner:compressed has -center 1x2 matching +center 1x2.
    // With compressed selected at offset (4, 0): contours shift right by 4 pixels.
    // Without: contours at (0, 0).
    // Verify by checking that contour points are shifted.
    assert!(
        !combo.contours.is_empty(),
        "combo glyph should have contours"
    );
    // The inner glyph is 8px wide. With offset col=4, leftmost contour x ≥ 4.
    // Without offset, leftmost contour x = 0.
    let min_x: i16 = combo
        .contours
        .iter()
        .flat_map(|c| c.iter().map(|&(x, _)| x))
        .min()
        .unwrap();
    let scale = UNITS_PER_EM as f32 / 16.0;
    let expected_min = (4.0 * scale).round() as i16;
    assert!(
        min_x >= expected_min,
        "inner:compressed should be selected (offset col=4), but min_x={min_x}, expected>={expected_min}"
    );
}

#[test]
fn gpos_mark_base_from_anchor_feature() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = base-letter
map \u{0308} = dia

feature ccmp for DFLT : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];

    // Verify parsed data
    let (_, _, glyph_data, gsub_data, _) = collect_glyph_data(&docs, false).unwrap();
    let dia_glyph = glyph_data.iter().find(|g| g.name == "dia").unwrap();
    assert!(dia_glyph.mark, "dia should be marked");
    assert!(
        !dia_glyph.resolved_anchors.is_empty(),
        "dia should have anchors"
    );
    assert!(
        !gsub_data.anchor_features.is_empty(),
        "anchor features should be collected"
    );

    let base_glyph = glyph_data.iter().find(|g| g.name == "base-letter").unwrap();
    assert!(
        !base_glyph.resolved_anchors.is_empty(),
        "base-letter should have anchors"
    );

    let font_data = build_font_from_documents(&docs);
    assert!(font_data.is_some(), "font should build successfully");

    let bytes = font_data.unwrap();
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    let gpos = font.gpos();
    assert!(gpos.is_ok(), "GPOS table should be present");

    let gdef = font.gdef();
    assert!(gdef.is_ok(), "GDEF table should be present");
    let gdef = gdef.unwrap();
    let class_def = gdef.glyph_class_def().unwrap();
    assert!(class_def.is_ok(), "GDEF should have glyph class def");
}

#[test]
fn gpos_mark_to_mark_from_anchor_feature() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1
anchor +above 1 -1

map a = base-letter
map \u{0308} = dia

feature ccmp for DFLT : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];

    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");

    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();

    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());

    assert!(anchor_data.gpos.is_some(), "GPOS should exist");
    let gpos = anchor_data.gpos.unwrap();

    let lookups = &gpos.lookup_list.lookups;
    assert!(
        lookups.len() >= 2,
        "should have MarkBasePos and MarkMarkPos lookups"
    );

    let has_mark_to_base = lookups
        .iter()
        .any(|l| matches!(l.as_ref(), PositionLookup::MarkToBase(_)));
    let has_mark_to_mark = lookups
        .iter()
        .any(|l| matches!(l.as_ref(), PositionLookup::MarkToMark(_)));
    assert!(has_mark_to_base, "MarkBasePos lookup should exist");
    assert!(has_mark_to_mark, "MarkMarkPos lookup should exist");

    let font_data = build_font_from_documents(&docs);
    assert!(font_data.is_some(), "font should build successfully");

    let bytes = font_data.unwrap();
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    let gpos_table = font.gpos().expect("GPOS table should be present");
    let feature_list = gpos_table.feature_list().expect("feature list");
    let feature_tags: Vec<_> = feature_list
        .feature_records()
        .iter()
        .map(|r| r.feature_tag())
        .collect();
    assert!(
        feature_tags.iter().any(|t| *t == Tag::new(b"mark")),
        "mark feature should exist"
    );
    assert!(
        feature_tags.iter().any(|t| *t == Tag::new(b"mkmk")),
        "mkmk feature should exist"
    );
}

#[test]
fn gpos_ccmp_generated_for_alternative() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph base:alt 4 4
@@@@@@@@
..@@@@..
..@@@@..
@@@@@@@@
anchor +above 2 0

glyph dia mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = base
map \u{0308} = dia

feature ccmp for DFLT : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let font_data = build_font_from_documents(&docs);
    assert!(font_data.is_some(), "font should build successfully");

    let bytes = font_data.unwrap();
    let font = read_fonts::FontRef::new(&bytes).unwrap();

    // GSUB should exist with ccmp feature
    let gsub = font.gsub();
    assert!(gsub.is_ok(), "GSUB table should be present");

    // GPOS should exist
    let gpos = font.gpos();
    assert!(gpos.is_ok(), "GPOS table should be present");
}

/// Regression test for anchor-based ccmp and GPOS with three base
/// glyphs that have different anchor/alternative configurations:
///
/// - `ii`: own `+below` (2-cell), ref `ii:dotless` (has `+above` 2-cell).
///   +above: substitute to ii:dotless.  +below: no substitution.
/// - `jj`: no own + anchors, ref `jj:dotless` (has `+above` 1-cell).
///   Alt `jj:compressed` has `+below` (1-cell).
///   +above: substitute to jj:dotless.  +below: substitute to jj:compressed.
/// - `kk`: own `+above` (1-cell) and `+below` (1-cell).
///   No substitution needed for either anchor.
///
/// Mark glyphs `dia-above` (1-cell `-above`) and `dia-below` (1-cell
/// `-below`) each have a `:wide` variant with a 2-cell anchor.  The
/// wide variant should be selected via ccmp when the base's `+` anchor
/// is 2-cells wide.
#[test]
fn anchor_ccmp_base_and_mark_substitution() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph dia 2 1
@@@@

glyph dia-wide 3 1
@@@@@@

glyph dia-below 2 1 mark
ref dia 0 0
anchor -below 1 0

glyph dia-below:wide 3 1 mark
ref dia-wide 0 0
anchor -below 1..2 0

glyph dia-above 2 1 mark
ref dia 0 0
anchor -above 1 0

glyph dia-above:wide 3 1 mark
ref dia-wide 0 0
anchor -above 1..2 0

glyph ii:dotless 4 8
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2..3 0

glyph ii 4 10
@@@@@@@@
@@@@@@@@
........
........
........
........
........
........
........
........
ref ii:dotless 0 2
anchor +below 2..3 9

glyph jj:dotless 4 8
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph jj 4 10
@@@@@@@@
@@@@@@@@
........
........
........
........
........
........
........
........
ref jj:dotless 0 2

glyph jj:compressed 4 10
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +below 2 9

glyph kk 4 10
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0
anchor +below 2 9

map a = ii
map b = jj
map c = kk
map \u{0308} = dia-above
map \u{0324} = dia-below

feature ccmp for DFLT : anchor above
feature ccmp for DFLT : anchor below
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];

    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");

    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();

    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());

    // --- Base substitution ------------------------------------------------

    // ii + dia-above: ii lacks own +above → substituted to ii:dotless
    assert!(
        anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, t, a, _)| s == "ii" && t == "ii:dotless" && a == "above"),
        "ii should be substituted to ii:dotless for anchor above"
    );
    // ii + dia-below: ii has own +below → NOT substituted
    assert!(
        !anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, _, a, _)| s == "ii" && a == "below"),
        "ii must not be substituted for anchor below (has own +below)"
    );

    // jj + dia-above: jj lacks own +above → substituted to jj:dotless
    assert!(
        anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, t, a, _)| s == "jj" && t == "jj:dotless" && a == "above"),
        "jj should be substituted to jj:dotless for anchor above"
    );
    // jj + dia-below: jj lacks own +below → substituted to jj:compressed
    assert!(
        anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, t, a, _)| s == "jj" && t == "jj:compressed" && a == "below"),
        "jj should be substituted to jj:compressed for anchor below"
    );

    // kk: has own +above and +below → NOT substituted for either
    assert!(
        !anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, _, _, _)| s == "kk"),
        "kk should not have any base substitution"
    );

    // --- Mark substitution ------------------------------------------------

    // dia-above → dia-above:wide after bases with 2-cell +above
    let da_entry = anchor_data
        .mark_subst_entries
        .iter()
        .find(|(m, alt, a, _)| m == "dia-above" && alt == "dia-above:wide" && a == "above");
    assert!(
        da_entry.is_some(),
        "dia-above should be substituted to dia-above:wide"
    );
    let da_bases = &da_entry.unwrap().3;
    assert!(
        da_bases.contains(&"ii:dotless".to_string()),
        "ii:dotless (2-cell +above) should trigger dia-above:wide"
    );
    assert!(
        !da_bases.contains(&"kk".to_string()),
        "kk (1-cell +above) must not trigger dia-above:wide"
    );

    // dia-below → dia-below:wide after bases with 2-cell +below
    let db_entry = anchor_data
        .mark_subst_entries
        .iter()
        .find(|(m, alt, a, _)| m == "dia-below" && alt == "dia-below:wide" && a == "below");
    assert!(
        db_entry.is_some(),
        "dia-below should be substituted to dia-below:wide"
    );
    let db_bases = &db_entry.unwrap().3;
    assert!(
        db_bases.contains(&"ii".to_string()),
        "ii (2-cell +below) should trigger dia-below:wide"
    );
    assert!(
        !db_bases.contains(&"kk".to_string()),
        "kk (1-cell +below) must not trigger dia-below:wide"
    );

    // --- GPOS exists ------------------------------------------------------

    assert!(anchor_data.gpos.is_some(), "GPOS should exist");
    assert!(
        !anchor_data.feature_lookups.is_empty(),
        "feature lookups should exist"
    );
}

/// Regression test: GPOS mark classification must use a mark glyph's
/// own (declared) anchors, not anchors forwarded from a ref'd glyph.
/// A mark glyph that refs another mark inherits (forwards) its
/// anchors; classification used to use that forwarded set, causing a
/// mark like `dia-above` (which refs `dia-below`) to register in the
/// `below` class with the wrong anchor coordinates.
#[test]
fn gpos_mark_classification_uses_declared_not_forwarded_anchors() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-below mark 3 2
@@@@@@
@@@@@@
anchor -below 1 1

glyph dia-above mark
ref dia-below inherit
anchor -above 1 0

map a = base-letter
map \u{0323} = dia-below
map \u{0308} = dia-above

feature ccmp for DFLT : anchor below
feature ccmp for DFLT : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect");

    // Sanity check on the setup itself: dia-above does forward -below
    // from its ref (that forwarding is correct and NOT the bug), but
    // its *declared* anchors must only contain its own -above.
    let dia_above = glyphs.iter().find(|g| g.name == "dia-above").unwrap();
    assert!(
        dia_above
            .resolved_anchors
            .iter()
            .any(|p| p.position == "-below"),
        "dia-above should forward -below from dia-below via the ref"
    );
    assert!(
        dia_above
            .declared_anchors
            .iter()
            .any(|p| p.position == "-above"),
        "dia-above should have its own declared -above anchor"
    );
    assert!(
        !dia_above
            .declared_anchors
            .iter()
            .any(|p| p.position == "-below"),
        "dia-above's declared anchors must not include the forwarded -below"
    );

    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
    let gpos = anchor_data.gpos.expect("GPOS should exist");
    let dia_above_gid = *name_to_gid.get("dia-above").unwrap();

    let mut found = false;
    for lookup in &gpos.lookup_list.lookups {
        if let PositionLookup::MarkToBase(lk) = lookup.as_ref() {
            for sub in &lk.subtables {
                let CoverageTable::Format1(cov) = &*sub.mark_coverage else {
                    continue;
                };
                let Some(idx) = cov.glyph_array.iter().position(|&g| g == dia_above_gid) else {
                    continue;
                };
                let record = &sub.mark_array.mark_records[idx];
                let AnchorTable::Format1(anchor) = &*record.mark_anchor else {
                    panic!("expected AnchorFormat1");
                };
                // dia-above's own -above anchor is at col=1,row=0:
                // x = 1*64 = 64, y = (12-0)*64 = 768.
                // The forwarded -below anchor (col=1,row=1) would give
                // y = (12-1)*64 = 704 instead — that's the bug.
                assert_eq!(
                    anchor.x_coordinate, 64,
                    "x should come from dia-above's own -above anchor"
                );
                assert_eq!(
                    anchor.y_coordinate, 768,
                    "dia-above must be positioned using its OWN -above anchor (y=768), \
                     not the forwarded -below anchor from dia-below (which would give y=704)"
                );
                found = true;
            }
        }
    }
    assert!(found, "dia-above should appear in a MarkBasePos mark array");
}

/// Regression test: a mark glyph that declares *no* `-anchor` of its own
/// must still be classified, from what `inherit` forwarded. A composite
/// mark built purely out of ref'd marks (a merged accent pair, say) used
/// to be dropped from the mark-to-base coverage entirely — it stayed a
/// GDEF mark with nowhere to attach, so it rendered at the origin.
///
/// The companion rule lives in
/// `gpos_mark_classification_uses_declared_not_forwarded_anchors`:
/// declared anchors win when there are any. Only the gap falls back.
#[test]
fn gpos_mark_classification_falls_back_to_inherited_anchors() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-plain mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1
anchor +above 1 0

glyph dia-merged mark
ref dia-plain inherit

map a = base-letter
map \u{0308} = dia-plain
map \u{0344} = dia-merged

feature ccmp for DFLT : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect");

    // Setup sanity: dia-merged declares nothing and gets -above only
    // through the `inherit` ref.
    let merged = glyphs.iter().find(|g| g.name == "dia-merged").unwrap();
    assert!(
        merged.declared_anchors.is_empty(),
        "dia-merged declares no anchors of its own"
    );
    assert!(
        merged
            .resolved_anchors
            .iter()
            .any(|p| p.position == "-above"),
        "dia-merged should forward -above from dia-plain via the `inherit` ref"
    );

    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
    let gpos = anchor_data.gpos.expect("GPOS should exist");
    let merged_gid = *name_to_gid.get("dia-merged").unwrap();

    let mut found = false;
    for lookup in &gpos.lookup_list.lookups {
        if let PositionLookup::MarkToBase(lk) = lookup.as_ref() {
            for sub in &lk.subtables {
                let CoverageTable::Format1(cov) = &*sub.mark_coverage else {
                    continue;
                };
                let Some(idx) = cov.glyph_array.iter().position(|&g| g == merged_gid) else {
                    continue;
                };
                let record = &sub.mark_array.mark_records[idx];
                let AnchorTable::Format1(anchor) = &*record.mark_anchor else {
                    panic!("expected AnchorFormat1");
                };
                // Inherited from dia-plain's -above at col=1,row=1:
                // x = 1*64 = 64, y = (12-1)*64 = 704.
                assert_eq!(anchor.x_coordinate, 64);
                assert_eq!(anchor.y_coordinate, 704);
                found = true;
            }
        }
    }
    assert!(
        found,
        "dia-merged must appear in a MarkBasePos mark array through its inherited -above"
    );
}

/// Regression test: when the used anchor classes are non-contiguous
/// (e.g. classes {0, 2} because the middle anchor class has no marks
/// using it), they must be compacted to contiguous 0-based indices so
/// that `MarkArray::class_count()` (which counts unique classes) still
/// matches the number of anchor slots in each `BaseRecord`.
#[test]
fn mark_class_compaction_keeps_base_record_slots_consistent() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +below 2 3
anchor +middle 2 2
anchor +above 2 0

glyph dia-below mark 3 2
@@@@@@
@@@@@@
anchor -below 1 1

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = base-letter
map \u{0323} = dia-below
map \u{0308} = dia-above

feature ccmp for DFLT : anchor below
feature ccmp for DFLT : anchor middle
feature ccmp for DFLT : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect");

    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
    let gpos = anchor_data.gpos.expect("GPOS should exist");

    let dia_below_gid = *name_to_gid.get("dia-below").unwrap();
    let dia_above_gid = *name_to_gid.get("dia-above").unwrap();

    let mut checked = false;
    for lookup in &gpos.lookup_list.lookups {
        if let PositionLookup::MarkToBase(lk) = lookup.as_ref() {
            for sub in &lk.subtables {
                let CoverageTable::Format1(mark_cov) = &*sub.mark_coverage else {
                    continue;
                };
                let CoverageTable::Format1(base_cov) = &*sub.base_coverage else {
                    continue;
                };
                assert_eq!(
                    base_cov.glyph_array.len(),
                    1,
                    "expected a single base glyph"
                );

                // Only 2 of the 3 declared classes (below, above) are
                // actually used by any mark ("middle" is unused), so
                // classes must be compacted down to 2 slots.
                let base_record = &sub.base_array.base_records[0];
                assert_eq!(
                    base_record.base_anchors.len(),
                    2,
                    "base record anchor slots must match the compacted class count, not the declared count"
                );

                let below_idx = mark_cov
                    .glyph_array
                    .iter()
                    .position(|&g| g == dia_below_gid)
                    .unwrap();
                let above_idx = mark_cov
                    .glyph_array
                    .iter()
                    .position(|&g| g == dia_above_gid)
                    .unwrap();
                let below_class = sub.mark_array.mark_records[below_idx].mark_class;
                let above_class = sub.mark_array.mark_records[above_idx].mark_class;

                assert!((below_class as usize) < base_record.base_anchors.len());
                assert!((above_class as usize) < base_record.base_anchors.len());
                assert_ne!(below_class, above_class);

                // Each mark's class must resolve to a present (non-null)
                // anchor on the base record.
                assert!(
                    base_record.base_anchors[below_class as usize]
                        .as_ref()
                        .is_some()
                );
                assert!(
                    base_record.base_anchors[above_class as usize]
                        .as_ref()
                        .is_some()
                );

                checked = true;
            }
        }
    }
    assert!(checked, "expected a MarkBasePos subtable to inspect");
}

/// An anchor feature whose tag already has a feature record in GSUB (a
/// `remap` with the same tag creates one) used to merge its lookups into
/// that record without registering its own scripts — in any script the
/// remap did not already cover, the merged lookups never applied.
#[test]
fn merging_into_an_existing_feature_record_registers_its_new_scripts() {
    use crate::render::ttf_builder::gpos::merge_anchor_feature_lookups;

    // A GSUB as a remap would leave it: a 'ccmp' feature registered for
    // DFLT only.
    let dflt_lang_sys = LangSys {
        required_feature_index: 0xFFFF,
        feature_indices: vec![0],
    };
    let existing = Gsub::new(
        ScriptList::new(vec![ScriptRecord::new(
            Tag::new(b"DFLT"),
            Script::new(Some(dflt_lang_sys), vec![]),
        )]),
        FeatureList::new(vec![FeatureRecord::new(
            Tag::new(b"ccmp"),
            Feature::new(None, vec![]),
        )]),
        LookupList::new(vec![]),
    );
    let mut gsub = Some(existing);

    // One anchor chain lookup scoped to 'hang'.
    let mut sc = SubstitutionChainContext::default();
    *sc = ChainedSequenceContext::Format3(ChainedSequenceContextFormat3::new(
        vec![],
        vec![],
        vec![],
        vec![],
    ));
    let chain = SubstitutionLookup::ChainContextual(Lookup::new(LookupFlag::empty(), vec![sc]));
    merge_anchor_feature_lookups(
        &mut gsub,
        vec![("ccmp".to_string(), vec!["hang".to_string()], vec![chain])],
    );

    let gsub = gsub.unwrap();
    // The lookups landed in the existing record...
    assert_eq!(gsub.feature_list.feature_records.len(), 1);
    assert_eq!(
        gsub.feature_list.feature_records[0]
            .feature
            .lookup_list_indices,
        vec![0u16]
    );
    // ...and 'hang' must still be registered to reach them.
    let hang = gsub
        .script_list
        .script_records
        .iter()
        .find(|r| r.script_tag == Tag::new(b"hang"))
        .expect("'hang' must be registered even when the feature record already existed");
    let Some(ref ls) = *hang.script.default_lang_sys else {
        panic!("'hang' should carry a default LangSys");
    };
    assert!(ls.feature_indices.contains(&0));
    // Registration must keep the script list sorted — the shaper
    // binary-searches it, and 'hang' appended after 'DFLT' would otherwise
    // work while hiding any record it displaced.
    let tags: Vec<_> = gsub
        .script_list
        .script_records
        .iter()
        .map(|r| r.script_tag)
        .collect();
    let mut sorted = tags.clone();
    sorted.sort();
    assert_eq!(tags, sorted, "script records must stay sorted by tag");
}

/// A script that already names a feature record of its own must not be given
/// a second one with the same tag. A shaper stops at the first record whose
/// tag matches (`hb_ot_layout_language_find_feature` returns on the first
/// hit), so the record it did not stop at is dead: the anchor substitutions
/// merged into it never run, every base keeps its plain form, and every mark
/// falls back to bearing placement.
///
/// The shape that produced this: `feature ccmp for hebr : anchor he-below`
/// beside `feature ccmp for hebr : he-meteg`, where the remap had already
/// given `hebr` a `ccmp` record and the merge picked the *first* `ccmp` in
/// the list — DFLT's — and registered that with `hebr` as well.
#[test]
fn a_script_that_already_names_the_tag_gets_no_second_feature_record() {
    use crate::render::ttf_builder::gpos::merge_anchor_feature_lookups;

    // A GSUB as the remap stage leaves it: two 'ccmp' records, DFLT on the
    // first and 'hebr' on a second of its own.
    let existing = Gsub::new(
        ScriptList::new(vec![
            ScriptRecord::new(
                Tag::new(b"DFLT"),
                Script::new(
                    Some(LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: vec![0],
                    }),
                    vec![],
                ),
            ),
            ScriptRecord::new(
                Tag::new(b"hebr"),
                Script::new(
                    Some(LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: vec![1],
                    }),
                    vec![],
                ),
            ),
        ]),
        FeatureList::new(vec![
            FeatureRecord::new(Tag::new(b"ccmp"), Feature::new(None, vec![])),
            FeatureRecord::new(Tag::new(b"ccmp"), Feature::new(None, vec![])),
        ]),
        LookupList::new(vec![]),
    );
    let mut gsub = Some(existing);

    let mut sc = SubstitutionChainContext::default();
    *sc = ChainedSequenceContext::Format3(ChainedSequenceContextFormat3::new(
        vec![],
        vec![],
        vec![],
        vec![],
    ));
    let chain = SubstitutionLookup::ChainContextual(Lookup::new(LookupFlag::empty(), vec![sc]));
    merge_anchor_feature_lookups(
        &mut gsub,
        vec![("ccmp".to_string(), vec!["hebr".to_string()], vec![chain])],
    );

    let gsub = gsub.unwrap();
    let hebr = gsub
        .script_list
        .script_records
        .iter()
        .find(|r| r.script_tag == Tag::new(b"hebr"))
        .expect("'hebr' must stay registered");
    let Some(ref ls) = *hebr.script.default_lang_sys else {
        panic!("'hebr' should carry a default LangSys");
    };

    let ccmp_indices: Vec<u16> = ls
        .feature_indices
        .iter()
        .copied()
        .filter(|&i| {
            gsub.feature_list.feature_records[i as usize].feature_tag == Tag::new(b"ccmp")
        })
        .collect();
    assert_eq!(
        ccmp_indices.len(),
        1,
        "one LangSys must name 'ccmp' once; a shaper reads only the first"
    );

    // ...and the one it names has to be the one the lookups landed in.
    let named = &gsub.feature_list.feature_records[ccmp_indices[0] as usize];
    assert_eq!(
        named.feature.lookup_list_indices,
        vec![0u16],
        "the record 'hebr' names must carry the merged anchor lookups"
    );
}

/// `align c` centres a mark in a slot wider than itself, where the default
/// `ul` puts it flush against the slot's low edge. Both readings come off one
/// source with only the `align` token differing, so what is pinned is the
/// difference the reduction makes and nothing else.
#[test]
fn a_centred_anchor_class_centres_a_mark_in_a_wider_slot() {
    /// `(base anchor x, mark anchor x, cell size)` in font units. The offset a
    /// shaper applies is the difference of the two.
    fn anchor_xs(align: &str) -> (i16, i16, i16) {
        // A 7-wide slot on the base against a 3-wide footprint on the mark.
        // Under `ul` the base's col 1 meets the mark's col 3; under `c` their
        // middles, 4 and 4, meet.
        let input = format!(
            "\
meta height 4
meta ascent 3
meta descent 1

glyph base-letter 8 4
................
................
................
................
anchor +slot 1..7 0

glyph tick 3 4 mark
......
......
......
......
anchor -slot 3..5 0

map U+0041 = base-letter
map U+0301 = tick

feature ccmp for DFLT : anchor slot{align}
"
        );
        let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];
        let (meta, scale, glyphs, gsub_data, _) =
            collect_glyph_data(&docs, false).expect("should collect glyph data");
        let name_to_gid: HashMap<String, GlyphId16> = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
            .collect();
        let anchor_data =
            build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
        let gpos = anchor_data.gpos.expect("GPOS should exist");

        let sub = gpos
            .lookup_list
            .lookups
            .iter()
            .find_map(|l| match l.as_ref() {
                PositionLookup::MarkToBase(lk) => Some(lk.subtables[0].clone()),
                _ => None,
            })
            .expect("MarkBasePos lookup");
        let AnchorTable::Format1(mark_anchor) = &*sub.mark_array.mark_records[0].mark_anchor else {
            panic!("expected AnchorFormat1 on the mark");
        };
        let class = sub.mark_array.mark_records[0].mark_class as usize;
        let base_anchor = sub.base_array.base_records[0].base_anchors[class]
            .as_ref()
            .expect("the base must offer this class an anchor");
        let AnchorTable::Format1(base_anchor) = base_anchor else {
            panic!("expected AnchorFormat1 on the base");
        };
        (
            base_anchor.x_coordinate,
            mark_anchor.x_coordinate,
            scale as i16,
        )
    }

    let (flush_base, flush_mark, cell) = anchor_xs("");
    let (centred_base, centred_mark, _) = anchor_xs(" align c");

    // Low ends: the base is read at col 1, the mark at col 3.
    assert_eq!(flush_base, cell);
    assert_eq!(flush_mark, 3 * cell);
    // Middles: 1..7 and 3..5 share the middle 4, so the two coincide and the
    // mark sits exactly where its own grid drew it.
    assert_eq!(centred_base, 4 * cell);
    assert_eq!(centred_mark, 4 * cell);

    let flush_offset = flush_base - flush_mark;
    let centred_offset = centred_base - centred_mark;
    assert_eq!(
        centred_offset - flush_offset,
        2 * cell,
        "centring shifts by half the difference of the two sizes"
    );
}

/// A base offering several slot sizes has to hand a following mark the one
/// that fits it, the way a mark carrying several drawings is handed the one
/// its slot wants. The two sides used to be asymmetric: a mark alternative was
/// chosen by the preceding base's `+X` size, but a base alternative was
/// whichever came first alphabetically, triggered by *any* mark of the class.
/// A base could therefore advertise only one slot, so a wide mark meeting a
/// letter with a narrow slot overflowed it with nothing to swap in.
#[test]
fn a_base_offers_the_slot_that_fits_the_following_mark() {
    let input = "\
meta height 4
meta ascent 3
meta descent 1

glyph letter 8 4
................
................
................
................

glyph letter:w3-slot 8 4
................
................
................
................
anchor +slot 3..5 0

glyph letter:w7-slot 8 4
................
................
................
................
anchor +slot 1..7 0

glyph narrow 8 4 mark advance 0
................
................
................
................
anchor -slot 3..5 0

glyph wide 8 4 mark advance 0
................
................
................
................
anchor -slot 1..7 0

map U+0041 = letter
map U+0301 = narrow
map U+0302 = wide

feature ccmp for DFLT : anchor slot
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());

    // Each entry is (base, base:alt, anchor, mark names the rule keys on).
    let mut entries: Vec<(String, String, Vec<String>)> = anchor_data
        .base_subst_entries
        .iter()
        .map(|(source, target, _, marks)| {
            let mut marks = marks.clone();
            marks.sort();
            (source.clone(), target.clone(), marks)
        })
        .collect();
    entries.sort();

    assert_eq!(
        entries,
        vec![
            (
                "letter".to_string(),
                "letter:w3-slot".to_string(),
                vec!["narrow".to_string()],
            ),
            (
                "letter".to_string(),
                "letter:w7-slot".to_string(),
                vec!["wide".to_string()],
            ),
        ],
        "each slot size must be reached by the marks that fit it, and only those"
    );
}

/// A mark no slot matches exactly still has to land somewhere, and the slot it
/// wants is the smallest one it *fits* — a `+` range is the room a base hands
/// over and a `-` range the room a mark takes, so a 5-wide mark offered a
/// 3-wide and a 7-wide slot belongs in the 7-wide one, centred. Matching
/// exactly and giving up otherwise left it with no anchor at all on a base
/// that had no slot of its own, which is the silent one-glyph drift that
/// bearing-placed marks fall into.
#[test]
fn a_mark_takes_the_smallest_slot_it_fits() {
    let slot = |name: &str, range: &str| {
        format!(
            "\nglyph letter:{name} 8 4\n................\n................\n\
             ................\n................\nanchor +slot {range} 0\n"
        )
    };
    let mark = |name: &str, range: &str, cp: u32| {
        format!(
            "\nglyph {name} 8 4 mark advance 0\n................\n................\n\
             ................\n................\nanchor -slot {range} 0\nmap U+{cp:04X} = {name}\n"
        )
    };
    let input = format!(
        "meta height 4\nmeta ascent 3\nmeta descent 1\n\
         \nglyph letter 8 4\n................\n................\n\
         ................\n................\nmap U+0041 = letter\n{}{}{}{}{}\n\
         feature ccmp for DFLT : anchor slot align c\n",
        slot("w3", "3..5"),
        slot("w7", "1..7"),
        mark("narrow", "3..5", 0x0301),
        mark("middle", "2..6", 0x0302),
        mark("wide", "1..7", 0x0303),
    );
    let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());

    let mut entries: Vec<(String, Vec<String>)> = anchor_data
        .base_subst_entries
        .iter()
        .map(|(_, target, _, marks)| {
            let mut marks = marks.clone();
            marks.sort();
            (target.clone(), marks)
        })
        .collect();
    entries.sort();

    assert_eq!(
        entries,
        vec![
            ("letter:w3".to_string(), vec!["narrow".to_string()]),
            (
                "letter:w7".to_string(),
                vec!["middle".to_string(), "wide".to_string()],
            ),
        ],
        "the 5-wide mark fits only the 7-wide slot, so that is the one it takes"
    );
}

/// The base's own slot comes first in that order: it is what the glyph is
/// without a substitution, so it is left alone whenever it holds the mark, and
/// an alternative is reached only by the marks it cannot. Ordering the
/// alternatives ahead of it substituted every mark, including the ones the
/// base was already drawn for.
#[test]
fn a_base_keeps_its_own_slot_for_the_marks_it_holds() {
    let input = "\
meta height 4
meta ascent 3
meta descent 1

glyph letter 8 4
................
................
................
................
anchor +slot 3..5 0

glyph letter:w7 8 4
................
................
................
................
anchor +slot 1..7 0

glyph narrow 8 4 mark advance 0
................
................
................
................
anchor -slot 3..5 0

glyph wide 8 4 mark advance 0
................
................
................
................
anchor -slot 1..7 0

map U+0041 = letter
map U+0301 = narrow
map U+0302 = wide

feature ccmp for DFLT : anchor slot align c
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());

    let entries: Vec<(String, Vec<String>)> = anchor_data
        .base_subst_entries
        .iter()
        .map(|(_, target, _, marks)| (target.clone(), marks.clone()))
        .collect();
    assert_eq!(
        entries,
        vec![("letter:w7".to_string(), vec!["wide".to_string()])],
        "only the mark the base's own slot cannot hold may reach an alternative"
    );
}

/// One anchor class named by two declarations — the same class offered to two
/// scripts, or a line left in beside the one that replaced it — must still be
/// one class. Numbering the classes by each declaration's position in the list
/// instead of by the class it named handed the second one a number past the
/// end of the per-class arrays, and the build panicked on the first glyph
/// carrying that anchor.
#[test]
fn an_anchor_class_named_twice_is_still_one_class() {
    let input = "\
meta height 4
meta ascent 3
meta descent 1

glyph letter 8 4
................
................
................
................
anchor +slot 3..5 0

glyph tick 8 4 mark advance 0
................
................
................
................
anchor -slot 3..5 0

glyph other-letter 8 4
................
................
................
................
anchor +other 3..5 0

glyph other-tick 8 4 mark advance 0
................
................
................
................
anchor -other 3..5 0

map U+0041 = letter
map U+0301 = tick
map U+0042 = other-letter
map U+0302 = other-tick

feature ccmp for DFLT : anchor slot
feature ccmp for latn : anchor slot
feature ccmp for DFLT : anchor other
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();

    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
    let gpos = anchor_data.gpos.expect("GPOS should exist");
    let sub = gpos
        .lookup_list
        .lookups
        .iter()
        .find_map(|l| match l.as_ref() {
            PositionLookup::MarkToBase(lk) => Some(lk.subtables[0].clone()),
            _ => None,
        })
        .expect("MarkBasePos lookup");
    assert_eq!(sub.mark_array.mark_records.len(), 2);
    assert_eq!(
        sub.base_array.base_records[0].base_anchors.len(),
        2,
        "two declarations of one anchor name are one mark class, not two"
    );
}

/// A glyph that carries anchors of its own *and* stands in as another glyph's
/// alternative must keep both sets.
///
/// Such a glyph reached the base list twice — once for what it declares, once
/// for the one class the glyph it substitutes for was missing — and the two
/// entries were deduplicated by glyph id, throwing one of them away. Hebrew
/// meets this the moment a letter carries both a below slot and a dagesh slot:
/// the variant that widens the below slot lost the dagesh's, so the two marks
/// could never attach at once.
#[test]
fn an_alternative_keeps_the_anchors_it_declares_as_well_as_the_one_it_stands_in_for() {
    let input = "\
meta height 4
meta ascent 3
meta descent 1

glyph letter 8 4
................
................
................
................
anchor +inside 3 1

glyph letter:slot 8 4
................
................
................
................
anchor +inside 3 1
anchor +below 1..7 3

glyph dot 8 4 mark advance 0
................
................
................
................
anchor -inside 3 1

glyph bar 8 4 mark advance 0
................
................
................
................
anchor -below 1..7 3

map U+0041 = letter
map U+0301 = dot
map U+0302 = bar

feature ccmp for DFLT : anchor inside
feature ccmp for DFLT : anchor below
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
    let gpos = anchor_data.gpos.expect("GPOS should exist");
    let sub = gpos
        .lookup_list
        .lookups
        .iter()
        .find_map(|l| match l.as_ref() {
            PositionLookup::MarkToBase(lk) => Some(lk.subtables[0].clone()),
            _ => None,
        })
        .expect("MarkBasePos lookup");
    let CoverageTable::Format1(base_cov) = &*sub.base_coverage else {
        panic!("expected a format 1 base coverage");
    };
    let alt_gid = name_to_gid["letter:slot"];
    let idx = base_cov
        .glyph_array
        .iter()
        .position(|&g| g == alt_gid)
        .expect("the alternative must be a base");
    let filled = sub.base_array.base_records[idx]
        .base_anchors
        .iter()
        .filter(|a| a.is_some())
        .count();
    assert_eq!(
        filled, 2,
        "the alternative declares two classes and stands in for one; both must survive"
    );
}

/// A glyph nothing maps but a `remap` names still needs its alternatives.
///
/// The reachability pass keeps such a glyph as an *extra*, and the pass that
/// keeps anchor alternatives asked only the glyphs reached directly — so a
/// ligature output (Hebrew's letter-with-dagesh) got none of the slots its
/// alternatives carry, and every mark that wanted one fell off it.
#[test]
fn a_glyph_only_a_remap_names_still_keeps_its_anchor_alternatives() {
    let input = "\
meta height 4
meta ascent 3
meta descent 1

glyph letter 8 4
................
................
................
................

glyph letter-tagged 8 4
................
................
................
................

glyph letter-tagged:slot 8 4
................
................
................
................
anchor +below 1..7 3

glyph tag 8 4 mark advance 0
................
................
................
................

glyph bar 8 4 mark advance 0
................
................
................
................
anchor -below 1..7 3

map U+0041 = letter
map U+0300 = tag
map U+0302 = bar

remap tagging : letter tag -> letter-tagged
feature ccmp for DFLT : anchor below
feature ccmp for DFLT : tagging
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).expect("should collect glyph data");
    assert!(
        glyphs.iter().any(|g| g.name == "letter-tagged:slot"),
        "the alternative of a remap-named glyph has to be built at all"
    );
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent());
    assert!(
        anchor_data
            .base_subst_entries
            .iter()
            .any(|(source, target, _, _)| source == "letter-tagged"
                && target == "letter-tagged:slot"),
        "the ligature output must give way to the alternative carrying the slot"
    );
}
