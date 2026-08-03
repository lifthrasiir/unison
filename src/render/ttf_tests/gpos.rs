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
            .any(|(s, t, a)| s == "ii" && t == "ii:dotless" && a == "above"),
        "ii should be substituted to ii:dotless for anchor above"
    );
    // ii + dia-below: ii has own +below → NOT substituted
    assert!(
        !anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, _, a)| s == "ii" && a == "below"),
        "ii must not be substituted for anchor below (has own +below)"
    );

    // jj + dia-above: jj lacks own +above → substituted to jj:dotless
    assert!(
        anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, t, a)| s == "jj" && t == "jj:dotless" && a == "above"),
        "jj should be substituted to jj:dotless for anchor above"
    );
    // jj + dia-below: jj lacks own +below → substituted to jj:compressed
    assert!(
        anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, t, a)| s == "jj" && t == "jj:compressed" && a == "below"),
        "jj should be substituted to jj:compressed for anchor below"
    );

    // kk: has own +above and +below → NOT substituted for either
    assert!(
        !anchor_data
            .base_subst_entries
            .iter()
            .any(|(s, _, _)| s == "kk"),
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
        merged.resolved_anchors.iter().any(|p| p.position == "-above"),
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
