//! Tests for composite glyphs: bearings, forwarded anchors, scaling and
//! decomposed-map synthesis.

use super::*;

#[test]
fn non_pattern_glyphs_resolve_substituted_and_pattern_refs() {
    let input = "\
name-parts $base = stem

glyph stem 1 1
@@

glyph stem-a 1 1
@@

glyph stem-b 1 1
@@

glyph via-parts
ref $base
map A = via-parts

glyph via-pattern
ref stem-(a|b)
map B = via-pattern
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();

    for name in ["via-parts", "via-pattern"] {
        assert!(
            glyphs
                .iter()
                .any(|glyph| glyph.name == name && !glyph.contours.is_empty()),
            "{name} did not resolve through the TTF dependency cache"
        );
    }
}

/// Negative ref offsets are *bearings*, not something to normalize away:
/// the outline keeps its negative coordinates (so the glyph gets a
/// negative lsb / a rise above the ascent) and the advance measures only
/// the extent to the right of the origin.  Every composite path — own
/// pixels, negated refs, pure refs — has to agree on that, and a parent
/// referencing a negative-origin child must not lose the part of the
/// child that sits left of its origin.
#[test]
fn negative_ref_offsets_become_bearings_not_normalized() {
    let input = "\
glyph box 2 2
@@@@
@@@@

glyph dot 1 1
@@

glyph ownpix 2 2
@@@@
@@@@
ref box -1 0
map A = ownpix

glyph negated 3 2
@@@@@@
@@@@@@
ref box -1 0 negated
map B = negated

glyph child
ref box -1 0

glyph parent 1 1
@@
ref child 3 0
map C = parent

glyph vert 2 2
@@@@
@@@@
ref box 0 -1
map D = vert
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let metrics = |name: &str| -> (u16, i16, i16, i16, i16) {
        let g = glyphs.iter().find(|g| g.name == name).unwrap();
        let pts: Vec<(i16, i16)> = g.contours.iter().flatten().copied().collect();
        (
            g.advance_width,
            pts.iter().map(|p| p.0).min().unwrap(),
            pts.iter().map(|p| p.0).max().unwrap(),
            pts.iter().map(|p| p.1).min().unwrap(),
            pts.iter().map(|p| p.1).max().unwrap(),
        )
    };

    // Own pixels 0..2 plus a ref reaching to -1: ink -1..2, advance 2.
    assert_eq!(metrics("ownpix"), (128, -64, 128, 768, 896));

    // Own pixels 0..3 minus a negated ref covering -1..1: ink 1..3.
    assert_eq!(metrics("negated"), (192, 64, 192, 768, 896));

    // `child` spans -1..1 around its own origin; placed at column 3 it
    // must occupy 2..4, and the parent's own dot 0..1 keeps min at 0.
    assert_eq!(metrics("parent"), (256, 0, 256, 768, 896));
    let parent = glyphs.iter().find(|g| g.name == "parent").unwrap();
    assert!(
        parent.contours.iter().flatten().any(|p| p.0 == 128),
        "the part of `child` left of its own origin must survive into the parent",
    );

    // Vertical: a ref one row above the origin rises above the ascent.
    assert_eq!(metrics("vert"), (128, 0, 128, 768, 960));
}

/// A negative offset into a ref's *empty* margin draws nothing before the
/// origin, so it must not create a bearing.  Raising a glyph by pulling it
/// into its own blank top rows is the usual way to nudge a composite, and
/// it has to stay metrically identical to the same ink placed directly.
#[test]
fn blank_margin_before_the_origin_is_not_a_bearing() {
    let input = "\
glyph padded 2 4
....
....
@@@@
@@@@

glyph raised 2 4
....
....
....
....
ref padded 0 -2
map A = raised

glyph direct 2 4
@@@@
@@@@
....
....
map B = direct
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let metrics = |name: &str| {
        let g = glyphs.iter().find(|g| g.name == name).unwrap();
        let pts: Vec<(i16, i16)> = g.contours.iter().flatten().copied().collect();
        (
            g.advance_width,
            pts.iter().map(|p| p.0).min().unwrap(),
            pts.iter().map(|p| p.1).max().unwrap(),
        )
    };
    assert_eq!(metrics("raised"), metrics("direct"));
}

#[test]
fn ttf_offsets_use_transitively_forwarded_anchors_without_mutation() {
    let input = "\
glyph link 1 1
@@
point -join 0 0
point +join 2 0

glyph wrapped
ref link

glyph chain
point +join 0 0
ref wrapped
ref wrapped
map C = chain
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let chain = glyphs.iter().find(|glyph| glyph.name == "chain").unwrap();
    let xs: Vec<i16> = chain
        .contours
        .iter()
        .flat_map(|contour| contour.iter().map(|point| point.0))
        .collect();
    assert_eq!(xs.iter().copied().min(), Some(0));
    assert_eq!(xs.iter().copied().max(), Some(192));

    let chain_body = doc.items.iter().find_map(|item| match item {
        DocumentItem::Glyph { name, body } if name.display() == "chain" => Some(body),
        _ => None,
    }).unwrap();
    assert!(chain_body.refs.iter().all(|gref| gref.offset.is_none()));
}

#[test]
fn map_decomposed_roundtrips() {
    let input = "\
map generate ä
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::MapDecomposed { char_repr, glyph, .. } = &doc.items[0] {
        assert_eq!(char_repr, "ä");
        assert_eq!(glyph.as_deref(), None);
    } else {
        panic!("expected MapDecomposed, got {:?}", doc.items[0]);
    }

    let mut output = Vec::new();
    document_io::serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("map generate ä"));
}

#[test]
fn map_decomposed_with_explicit_name_roundtrips() {
    let input = "\
map generate ä = a-dieresis
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::MapDecomposed { char_repr, glyph, .. } = &doc.items[0] {
        assert_eq!(char_repr, "ä");
        assert_eq!(glyph.as_deref(), Some("a-dieresis"));
    } else {
        panic!("expected MapDecomposed, got {:?}", doc.items[0]);
    }

    let mut output = Vec::new();
    document_io::serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("map generate ä = a-dieresis"));
}

#[test]
fn map_decomposed_generates_composite_glyph() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph a-lower 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = a-lower
map \u{0308} = dia-above
map generate ä
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];
    let font_data = build_font_from_documents(&docs);
    assert!(font_data.is_some(), "font should build");

    let bytes = font_data.unwrap();
    let font = read_fonts::FontRef::new(&bytes).unwrap();

    // ä (U+00E4) should be in the cmap
    let cmap = font.cmap().unwrap();
    let gid = cmap.map_codepoint('ä');
    assert!(gid.is_some(), "ä should be mapped in cmap");
}

#[test]
fn map_decomposed_explicit_name_names_the_generated_glyph() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph a-lower 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = a-lower
map \u{0308} = dia-above
map generate ä = a-dieresis
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];

    let (_, _, glyphs, _, _) = collect_glyph_data(&docs, false).expect("should collect");
    assert!(
        glyphs.iter().any(|g| g.name == "a-dieresis"),
        "generated glyph should carry the declared name, got: {:?}",
        glyphs.iter().map(|g| &g.name).collect::<Vec<_>>(),
    );
    assert!(
        !glyphs.iter().any(|g| g.name == "uni00E4"),
        "the default uniXXXX name should not also be emitted",
    );

    let bytes = build_font_from_documents(&docs).expect("font should build");
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    assert!(font.cmap().unwrap().map_codepoint('ä').is_some(), "ä should be mapped");
}

#[test]
fn map_decomposed_forwards_mark_anchors() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph a-lower 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0
anchor +below 2 3

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1
anchor +above 1 -1

map a = a-lower
map \u{0308} = dia-above
map generate ä

feature ccmp for DFLT : anchor above
feature ccmp for DFLT : anchor below
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs: Vec<&Document> = vec![&doc];

    let (_, _, glyphs, _, _) =
        collect_glyph_data(&docs, false).expect("should collect");

    let composite = glyphs.iter().find(|g| g.name == "uni00E4").unwrap();
    let has_plus_above = composite.resolved_anchors.iter()
        .any(|p| p.position == "+above");
    let has_plus_below = composite.resolved_anchors.iter()
        .any(|p| p.position == "+below");
    assert!(has_plus_above,
        "uni00E4 should forward +above from dia-above; anchors: {:?}",
        composite.resolved_anchors);
    assert!(has_plus_below,
        "uni00E4 should forward +below from a-lower; anchors: {:?}",
        composite.resolved_anchors);

    // Verify that the composite is registered as a base in GPOS
    let (meta, scale, glyphs, gsub_data, _) =
        collect_glyph_data(&docs, false).unwrap();
    let name_to_gid: HashMap<String, GlyphId16> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
        .collect();
    let anchor_data = build_anchor_gpos(
        &glyphs, &gsub_data, &name_to_gid, scale, meta.ascent(),
    );
    assert!(anchor_data.gpos.is_some(), "GPOS should exist");

    // Check that uni00E4 is in the MarkBasePos base coverage
    let gpos = anchor_data.gpos.unwrap();
    let lookups = &gpos.lookup_list.lookups;
    let composite_gid = *name_to_gid.get("uni00E4").unwrap();
    let mut found_in_base_coverage = false;
    for lookup in lookups {
        if let PositionLookup::MarkToBase(ref lk) = *lookup.as_ref() {
            for sub in &lk.subtables {
                if let CoverageTable::Format1(ref cov) = *sub.base_coverage
                    && cov.glyph_array.contains(&composite_gid) {
                        found_in_base_coverage = true;
                    }
            }
        }
    }
    assert!(found_in_base_coverage,
        "uni00E4 (gid {:?}) should be in MarkBasePos base coverage",
        composite_gid);
}

/// Regression test: a composite glyph's component `y_offset` must be
/// `-dy * scale` (plus top-offset compensation), not
/// `(ascent - dy) * scale`. The latter double-counts the ascent and
/// shifts every composite (ref-built) glyph up by a full ascender.
#[test]
fn composite_y_offset_is_negative_dy_not_ascent_relative() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 2 2
@@@@
@@@@

glyph comp
ref base 0 3

map A = base
map B = comp
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, scale, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let comp = glyphs.iter().find(|g| g.name == "comp").unwrap();
    assert_eq!(comp.composite_refs.len(), 1, "comp should keep its composite representation");
    let dy = 3.0f32;
    let expected_y = (-dy * scale).round() as i16;
    assert_eq!(
        comp.composite_refs[0].y_offset, expected_y,
        "composite y_offset must be -dy*scale, not ascent-relative"
    );
}

/// Regression test: subpixel-conflict detection between composite ref
/// layers must look at actual pixel shapes, not just bounding-box
/// overlap. Two grids whose bboxes fully overlap but whose filled cells
/// never coincide must NOT be flagged as conflicting; two grids that
/// fill the *same* cell with different (non-empty) shapes must be.
#[test]
fn layers_have_subpixel_conflicts_checks_pixels_not_just_bbox() {
    let mut a = PixelGrid::new(2, 2);
    a.set(0, 0, PixelShape::new(PX_ALMOSTFULL, true));
    let mut b = PixelGrid::new(2, 2);
    b.set(1, 1, PixelShape::new(PX_ALMOSTFULL, true));
    assert!(
        !layers_have_subpixel_conflicts(&[(&a, 0, 0), (&b, 0, 0)]),
        "overlapping bboxes with disjoint filled cells must not conflict"
    );

    let mut c = PixelGrid::new(1, 1);
    c.set(0, 0, PixelShape::new(PX_ALMOSTFULL, true));
    let mut d = PixelGrid::new(1, 1);
    d.set(0, 0, PixelShape::new(crate::pixel::PX_HALF3, true));
    assert!(
        layers_have_subpixel_conflicts(&[(&c, 0, 0), (&d, 0, 0)]),
        "the same cell filled with two different shapes must conflict"
    );
}

/// Regression test: a pure-ref composite whose component bounding boxes
/// overlap, but whose actual pixels never conflict, must keep its
/// TrueType composite-component representation rather than being
/// flattened into full contours (which used to balloon font size).
#[test]
fn non_conflicting_overlapping_bbox_refs_keep_composite_representation() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph partA 2 2
@@..
....

glyph partB 2 2
....
..@@

glyph combo
ref partA 0 0
ref partB 0 0

map A = partA
map B = partB
map C = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyphs.iter().find(|g| g.name == "combo").unwrap();
    assert_eq!(
        combo.composite_refs.len(), 2,
        "non-conflicting overlapping-bbox refs should stay as 2 composite components"
    );
}

/// Regression test: `glyph foo W H` with an all-empty own pixel grid
/// (declared dims but no filled pixels) plus refs must (a) still use
/// the declared width for the advance and (b) not force the composite
/// to flatten into full contours just because an (empty) own grid is
/// present.
#[test]
fn declared_dims_with_empty_grid_keeps_advance_and_composite() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph combo 16 16
ref base 0 0

map A = base
map B = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, scale, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyphs.iter().find(|g| g.name == "combo").unwrap();
    let expected_advance = (16.0f32 * scale).round() as u16;
    assert_eq!(combo.advance_width, expected_advance, "advance should come from declared width");
    assert_eq!(
        combo.composite_refs.len(), 1,
        "an all-empty own grid must not force flattening of the composite"
    );
}

/// Regression test for `build_composite_refs`'s `top`/`left` offset
/// compensation: a child glyph's OWN `left`/`top` offset (already baked
/// into the child's own glyph outline) must not leak into a parent
/// composite that refs it — the parent must subtract the child's own
/// offset so the ref's declared position isn't double-shifted.
#[test]
fn composite_ref_compensates_for_childs_own_left_offset() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph child 2 2 left 5
@@@@
@@@@

glyph parent
ref child 0 0

map A = child
map B = parent
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, scale, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let parent = glyphs.iter().find(|g| g.name == "parent").unwrap();
    assert_eq!(parent.composite_refs.len(), 1);
    let comp_left = (5.0f32 * scale).round() as i16;
    assert_eq!(
        parent.composite_refs[0].x_offset, -comp_left,
        "parent composite must subtract the child's own left offset (already baked into \
         the child's own glyph outline) so it isn't double-applied"
    );
}

/// A `negated` ref only *removes* area from the layers below it; it never
/// draws anything of its own. The color-layer rebuild path (taken as soon
/// as any ref carries a fill or a visibility flag) used to translate every
/// ref's contours in unconditionally, so a negated ref inside a monoonly
/// stack filled its own shape in instead of punching a hole.
#[test]
fn negated_ref_subtracts_in_fallback_contours() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph hole 2 2
@@@@
@@@@

glyph combo
ref base monoonly
ref hole 1 1 negated monoonly
ref base coloronly fill #ff0000

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyphs.iter().find(|g| g.name == "combo").unwrap();

    // scale = UNITS_PER_EM / height = 1024 / 16 = 64, y = (ascent - row) * scale.
    let s = 64.0;
    assert_ne!(
        winding_at(&combo.contours, 0.5 * s, (12.0 - 0.5) * s),
        0,
        "the base layer should be filled"
    );
    assert_eq!(
        winding_at(&combo.contours, 2.0 * s, (12.0 - 2.0) * s),
        0,
        "the negated ref should punch a hole, not fill itself in"
    );
}

#[test]
fn scaled_glyph_has_same_advance_as_unscaled() {
    let input_unscaled = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 3
@@@@@@@@
@@@@@@@@
@@@@@@@@

map A = base
";
    let input_scaled = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 3 scale 2
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

map A = base
";
    let doc1 = document_io::parse_document_from_str(input_unscaled, "a.unf".into()).unwrap();
    let doc2 = document_io::parse_document_from_str(input_scaled, "b.unf".into()).unwrap();

    let (_, _, glyphs1, _, _) = collect_glyph_data(&[&doc1], false).unwrap();
    let (_, _, glyphs2, _, _) = collect_glyph_data(&[&doc2], false).unwrap();

    assert_eq!(glyphs1[0].advance_width, glyphs2[0].advance_width);
    assert!(!glyphs2[0].contours.is_empty());
}

/// U+1FB43 and its seven siblings: a smooth-mosaic sextant built from an
/// on-demand triangle whose slope needs custom detail cells, plus two
/// rectangles.  The union must stay the convex pentagon the triangle
/// implies — the shape-id-only tracer used to drop the detail cells and
/// leave a concave whole-pixel staircase.
#[test]
fn smooth_mosaic_sextant_traces_as_a_convex_polygon() {
    let doc = document_io::parse_document_from_str(
        "\
font-meta height 16 ascent 12 descent 4

glyph sextant-13-dr 8 16
ref 4x10p2r3-dr
ref 4x16 4 0
ref 8x-5p1r3 0 10

map A = sextant-13-dr
",
        "test.unf".into(),
    )
    .unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let glyph = glyphs.iter().find(|g| g.name == "sextant-13-dr").unwrap();
    assert_eq!(glyph.contours.len(), 1, "single outline: {:?}", glyph.contours);
    let contour = &glyph.contours[0];
    assert_eq!(contour.len(), 5, "no staircase: {contour:?}");
    // Every turn must go the same way for a convex polygon.
    let n = contour.len();
    let signs: Vec<i64> = (0..n)
        .map(|i| {
            let (p, q, r) = (contour[i], contour[(i + 1) % n], contour[(i + 2) % n]);
            let cross = (q.0 as i64 - p.0 as i64) * (r.1 as i64 - p.1 as i64)
                - (q.1 as i64 - p.1 as i64) * (r.0 as i64 - p.0 as i64);
            cross.signum()
        })
        .collect();
    assert!(
        signs.iter().all(|&s| s == signs[0] && s != 0),
        "contour is not convex: {contour:?} (turns {signs:?})",
    );
}

/// Two custom-detail refs that merely touch (an on-demand triangle on top
/// of a rectangle) share one shape id, so the overlap check used to call
/// them identical and emit both outlines with a coincident edge instead of
/// their union.
#[test]
fn touching_detail_refs_merge_into_one_outline() {
    let doc = document_io::parse_document_from_str(
        "\
font-meta height 16 ascent 12 descent 4

glyph sextant-1234-dr 8 16
ref 8x10p2r3-dr
ref 8x-5p1r3 0 10

map A = sextant-1234-dr
",
        "test.unf".into(),
    )
    .unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let glyph = glyphs.iter().find(|g| g.name == "sextant-1234-dr").unwrap();
    assert_eq!(
        glyph.contours.len(), 1,
        "triangle and rectangle must union: {:?}", glyph.contours,
    );
    assert_eq!(glyph.contours[0].len(), 4, "{:?}", glyph.contours[0]);
}

#[test]
fn scaled_composite_matches_unscaled() {
    let input_unscaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 6 3
ref part 0 0
ref part 0 4

map A = combo
";
    // Same composite but the parent is at scale 2.
    // The refs point at scale-1 parts; offsets are doubled.
    let input_scaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 6 3 scale 2
ref part 0 0
ref part 0 8

map A = combo
";
    let doc1 = document_io::parse_document_from_str(input_unscaled, "a.unf".into()).unwrap();
    let doc2 = document_io::parse_document_from_str(input_scaled, "b.unf".into()).unwrap();

    let (_, _, g1, _, _) = collect_glyph_data(&[&doc1], false).unwrap();
    let (_, _, g2, _, _) = collect_glyph_data(&[&doc2], false).unwrap();

    assert_eq!(
        g1[0].advance_width, g2[0].advance_width,
        "advance: unscaled {} vs scaled {}",
        g1[0].advance_width, g2[0].advance_width
    );
    assert_eq!(
        g1[0].contours, g2[0].contours,
        "contours should match"
    );
}

#[test]
fn scaled_composite_with_own_pixels_matches_unscaled() {
    // Parent has own pixels AND refs
    let input_unscaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 4 3
@@......@@
@@......@@
@@......@@
ref part 0 2

map A = combo
";
    let input_scaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 4 3 scale 2
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
ref part 0 4

map A = combo
";
    let doc1 = document_io::parse_document_from_str(input_unscaled, "a.unf".into()).unwrap();
    let doc2 = document_io::parse_document_from_str(input_scaled, "b.unf".into()).unwrap();

    let (_, _, g1, _, _) = collect_glyph_data(&[&doc1], false).unwrap();
    let (_, _, g2, _, _) = collect_glyph_data(&[&doc2], false).unwrap();

    assert_eq!(
        g1[0].advance_width, g2[0].advance_width,
        "advance: unscaled {} vs scaled {}",
        g1[0].advance_width, g2[0].advance_width
    );
    assert_eq!(
        g1[0].contours, g2[0].contours,
        "contours should match"
    );
}

/// A `ref` to a contentless glyph (the `ref sp` placeholder idiom) must not
/// survive into `glyf` as a component: OTS warns "empty gid N used as
/// component in glyph M" for every such component, and when it is the only
/// component it cannot even repair it.
#[test]
fn composites_do_not_reference_empty_glyphs() {
    let solid_rows = "@@@@@@@@@@@@@@@@\n".repeat(16);
    let input = format!(
        "\
font-meta height 16 ascent 12 descent 4

glyph blank 8 16 sticky

glyph solid 8 16
{solid_rows}
glyph placeholder advance 0
ref blank

glyph mixed
ref solid
ref blank 8 0

glyph nested advance 0
ref placeholder

map A = solid
map B = placeholder
map C = mixed
map D = nested
"
    );
    let doc = document_io::parse_document_from_str(&input, "t.unf".into()).unwrap();
    let built = build_font_with_gid_map(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&built.ttf).unwrap();
    let glyf = font.glyf().unwrap();
    let loca = font.loca(None).unwrap();
    let cmap = font.cmap().unwrap();
    let maxp = font.maxp().unwrap();

    let name_of = |gid: GlyphId| -> String {
        built
            .gid_to_name
            .get(&(gid.to_u32() as u16))
            .cloned()
            .unwrap_or_else(|| format!("gid {}", gid.to_u32()))
    };
    let is_empty = |gid: GlyphId| loca.get_glyf(gid, &glyf).unwrap().is_none();

    for raw in 0..maxp.num_glyphs() as u32 {
        let gid = GlyphId::new(raw);
        let Some(read_fonts::tables::glyf::Glyph::Composite(c)) =
            loca.get_glyf(gid, &glyf).unwrap()
        else {
            continue;
        };
        for comp in c.components() {
            let comp_gid = GlyphId::from(comp.glyph);
            assert!(
                !is_empty(comp_gid),
                "glyph '{}' uses empty glyph '{}' as a component",
                name_of(gid),
                name_of(comp_gid),
            );
        }
    }

    // `placeholder` and `nested` collapse to plain empty glyphs...
    for ch in ['B', 'D'] {
        let gid = cmap.map_codepoint(ch).expect("mapped");
        assert!(is_empty(gid), "{ch} should become an empty glyph");
    }
    // ...while `mixed` stays a composite, minus the blank layer.
    let mixed = cmap.map_codepoint('C').expect("mapped");
    let solid = cmap.map_codepoint('A').expect("mapped");
    let Some(read_fonts::tables::glyf::Glyph::Composite(c)) =
        loca.get_glyf(mixed, &glyf).unwrap()
    else {
        panic!("mixed should stay a composite");
    };
    let comps: Vec<_> = c.components().map(|c| GlyphId::from(c.glyph)).collect();
    assert_eq!(comps, vec![solid], "mixed should keep only the solid layer");
}
