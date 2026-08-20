//! Tests for [`crate::document`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;
use crate::pixel::{PX_ALMOSTFULL, PX_CUSTOM, PixelShape};

/// A block whose only content is one `ref`, which is what the name-expansion
/// tests below vary the *name* of.
fn ref_body(name: &str) -> GlyphBody {
    GlyphBody {
        refs: vec![pattern_ref(name)],
        scale: 1,
        ..GlyphBody::new()
    }
}

fn pattern_ref(name: &str) -> GlyphRef {
    GlyphRef {
        raw_name: None,
        comment: None,
        name: name.to_string(),
        offset: None,
        negated: false,
        inherit: false,
        fill: None,
        visibility: None,
    }
}

#[test]
fn classify_directive_recognizes_exactly_the_untyped_directives() {
    use super::{Directive, classify_directive};
    assert_eq!(
        classify_directive("exclude-from-sample a b"),
        Directive::ExcludeFromSample("a b"),
    );
    assert_eq!(
        classify_directive("  assume unused foo  "),
        Directive::AssumeUnused("foo"),
    );
    assert_eq!(classify_directive("   "), Directive::Empty);
    // No arguments means no match: `assume unused` alone says nothing.
    assert_eq!(classify_directive("assume unused"), Directive::Unrecognized);
    assert_eq!(
        classify_directive("assume something"),
        Directive::Unrecognized
    );
    // Malformed forms of directives that normally parse into typed items
    // must still be reported rather than silently accepted.
    assert_eq!(classify_directive("assert bogus"), Directive::Unrecognized);
    assert_eq!(classify_directive("whatever"), Directive::Unrecognized);
}

/// The group name is the only thing allowed before a rule's first colon.
/// Anything else used to be dropped on the floor: `remap a b : c -> d`
/// parsed as group `a` with source `c`, and `b` simply vanished.
#[test]
fn remap_rejects_stray_tokens_before_the_first_colon() {
    fn parse(line: &str) -> DocumentItem {
        let tokens: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        DocumentItem::parse_directive(&tokens, None)
    }

    assert!(
        matches!(parse("remap grp : a -> b"), DocumentItem::Remap { .. }),
        "the plain form still parses"
    );
    assert!(
        matches!(parse("remap grp: a -> b"), DocumentItem::Remap { .. }),
        "and so does the attached-colon spelling"
    );
    assert_eq!(
        parse("remap grp stray : a -> b"),
        DocumentItem::Directive("remap grp stray : a -> b".to_string()),
        "a stray token must make the line unrecognized, not disappear"
    );
}

fn group_order(text: &str) -> RemapGroupOrder {
    let doc = crate::document_io::parse_document_from_str(text, "test.unf".into()).unwrap();
    remap_group_order(&[&doc])
}

#[test]
fn groups_default_to_the_order_their_first_rule_appears() {
    let o = group_order("remap b : x -> y\nremap a : x -> y\nremap b : y -> x\n");
    assert_eq!(o.order, vec!["b".to_string(), "a".to_string()]);
    assert!(o.cycle.is_empty() && o.unknown_after.is_empty());
}

/// The whole reason for a *stable* topological sort: constraining one pair
/// must not shuffle the groups that said nothing.
#[test]
fn after_moves_only_what_it_names() {
    let o = group_order(
        "remap a : x -> y\nremap b : x -> y\nremap c : x -> y\nremap d : x -> y\n\
             remap group a after c\n",
    );
    assert_eq!(
        o.order,
        vec![
            "b".to_string(),
            "c".to_string(),
            "a".to_string(),
            "d".to_string()
        ],
        "a lands right after c; b and d keep their places"
    );
}

#[test]
fn after_chains_transitively() {
    let o = group_order(
        "remap a : x -> y\nremap b : x -> y\nremap c : x -> y\n\
             remap group a after b\nremap group b after c\n",
    );
    assert_eq!(
        o.order,
        vec!["c".to_string(), "b".to_string(), "a".to_string()]
    );
}

#[test]
fn a_cycle_falls_back_to_source_order_and_is_reported() {
    let o = group_order(
        "remap a : x -> y\nremap b : x -> y\n\
             remap group a after b\nremap group b after a\n",
    );
    assert_eq!(o.order, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(o.cycle, vec!["a".to_string(), "b".to_string()]);
}

/// A group naming itself is a cycle of one; dropping the edge as a no-op
/// would let a plainly wrong line pass unremarked.
#[test]
fn a_group_after_itself_is_a_cycle() {
    let o = group_order("remap a : x -> y\nremap group a after a\n");
    assert_eq!(o.cycle, vec!["a".to_string()]);
}

#[test]
fn unknown_after_targets_are_reported_and_ignored() {
    let o = group_order("remap a : x -> y\nremap group a after nope\n");
    assert_eq!(o.order, vec!["a".to_string()]);
    assert_eq!(o.unknown_after, vec![("a".to_string(), "nope".to_string())]);
    assert!(o.cycle.is_empty());
}

#[test]
fn a_declaration_alone_places_and_describes_a_group() {
    let o = group_order("remap group early reversed\nremap late : x -> y\n");
    assert_eq!(o.order, vec!["early".to_string(), "late".to_string()]);
    assert!(o.info["early"].reversed && o.info["early"].declared);
    assert!(!o.info["late"].reversed && !o.info["late"].declared);
}

#[test]
fn a_second_declaration_is_reported_and_does_not_win() {
    let o = group_order("remap group a reversed\nremap group a\n");
    assert_eq!(o.duplicate_decls, vec!["a".to_string()]);
    assert!(o.info["a"].reversed, "the first declaration stands");
}

#[test]
fn collect_name_parts_decodes_empty_alternative() {
    let mut doc = Document::new("test.unf".into());
    doc.items.push(DocumentItem::NameParts {
        slices: Vec::new(),
        comment: None,
        name: "$part".to_string(),
        values: vec!["``|a".to_string()],
    });

    let parts = collect_name_parts(&[&doc]);
    assert_eq!(
        parts.get("$part"),
        Some(&vec![String::new(), "a".to_string()]),
    );
}

#[test]
fn collect_name_parts_preserves_repeat_that_exceeds_cumulative_limit() {
    let mut doc = Document::new("test.unf".into());
    let oversized = format!("b*{}", MAX_EXPANSION);
    doc.items.push(DocumentItem::NameParts {
        slices: Vec::new(),
        comment: None,
        name: "$part".to_string(),
        values: vec!["a".to_string(), oversized.clone()],
    });

    assert!(
        try_resolve_name_part_values(&["a".to_string(), oversized.clone()], &NamePartsMap::new())
            .is_err(),
        "a binding over the expansion limit is an error",
    );
    let parts = collect_name_parts(&[&doc]);
    assert_eq!(parts.get("$part"), Some(&vec!["a".to_string(), oversized]),);
}

/// A value is a name pattern like any other: groups, inline ranges and
/// `$ref`s nested inside them expand, so `bar-($1..3)` states exactly what
/// `bar1 bar2 bar3` states.
#[test]
fn name_part_values_expand_patterns() {
    let mut defined = NamePartsMap::new();
    defined.insert("$ab".to_string(), vec!["a".to_string(), "b".to_string()]);

    let resolve = |values: &[&str]| {
        resolve_name_part_values(
            &values.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &defined,
        )
    };

    assert_eq!(resolve(&["bar($1..3)"]), ["bar1", "bar2", "bar3"]);
    assert_eq!(resolve(&["bar-($1..3)"]), ["bar-1", "bar-2", "bar-3"]);
    assert_eq!(resolve(&["x-($ab)"]), ["x-a", "x-b"]);
    assert_eq!(resolve(&["x-($ab)-y", "z"]), ["x-a-y", "x-b-y", "z"]);
    assert_eq!(resolve(&["($#0e..10)"]), ["0e", "0f", "10"]);
    // Plain values, `|` lists, `$ref` splices and `*N` repeats are what
    // they always were.
    assert_eq!(
        resolve(&["a", "b|c", "$ab", "d*2"]),
        ["a", "b", "c", "a", "b", "d", "d"]
    );
}

/// The cap applies to the declaration itself, not only to the names a
/// glyph line later builds out of it.
#[test]
fn a_name_part_value_over_the_expansion_limit_is_an_error() {
    let over = format!("x-($1..{})", MAX_EXPANSION + 1);
    let one = std::slice::from_ref(&over);
    assert!(try_resolve_name_part_values(one, &NamePartsMap::new()).is_err());
    assert_eq!(
        resolve_name_part_values(one, &NamePartsMap::new()),
        std::slice::from_ref(&over)
    );

    let half = format!("x-($1..{})", MAX_EXPANSION / 2 + 1);
    assert!(
        try_resolve_name_part_values(&[half.clone(), half.clone()], &NamePartsMap::new()).is_err(),
        "the limit is cumulative over the whole binding",
    );
}

#[test]
fn expand_glyph_block_rejects_zero_repeat_without_panicking() {
    let result = expand_glyph_block(&GlyphName("glyph*0".to_string()), &ref_body("base"));

    assert!(result.is_err());
}

/// An oversized inline range is reported by `find_invalid_inline_ranges`,
/// against the range itself rather than the whole pattern.
#[test]
fn an_oversized_inline_range_is_reported() {
    assert_eq!(
        find_invalid_inline_ranges("uni($#00000000..FFFFFFFF)"),
        vec!["$#00000000..FFFFFFFF".to_string()],
    );
}

/// The block's body is shared by every name it declares, the grid included:
/// there is one of it written, so there is one of it in each expansion. A
/// block that draws (or just boxes) but refers to nothing still declares its
/// glyphs — the count comes from the name, never from what fills the block.
#[test]
fn expand_glyph_block_shares_the_block_body_with_every_expansion() {
    let mut pixels = PixelGrid::new(2, 2);
    pixels.set(0, 0, PixelShape::new(0, true));
    let body = GlyphBody {
        pixels: Some(pixels.clone()),
        extent: Some((3, 4)),
        mark: true,
        scale: 1,
        ..GlyphBody::new()
    };

    let items = expand_glyph_block(&GlyphName("out-(a|b)".to_string()), &body).unwrap();

    let names: Vec<String> = items
        .iter()
        .map(|item| match item {
            DocumentItem::Glyph { name, .. } => name.display(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(names, vec!["out-a".to_string(), "out-b".to_string()]);
    for item in &items {
        let DocumentItem::Glyph { name, body } = item else {
            unreachable!()
        };
        assert_eq!(body.pixels.as_ref(), Some(&pixels), "{}", name.display());
        assert_eq!(body.extent, Some((3, 4)), "{}", name.display());
        assert!(body.mark, "{}", name.display());
    }
}

#[test]
fn expand_glyph_block_expands_a_hex_range() {
    let items = expand_glyph_block(
        &GlyphName(substitute_name_parts(
            "uni($#2800..2801)",
            &NamePartsMap::new(),
        )),
        &ref_body("base"),
    )
    .unwrap();
    let names: Vec<String> = items
        .into_iter()
        .map(|item| match item {
            DocumentItem::Glyph { name, .. } => name.display(),
            _ => unreachable!(),
        })
        .collect();

    assert_eq!(names, vec!["uni2800".to_string(), "uni2801".to_string()]);
}

#[test]
fn glyph_name_count_drives_ref_pattern_expansion() {
    let items = expand_glyph_block(
        &GlyphName("out-(a|b)".to_string()),
        &ref_body("dep-(1|2|3|4)"),
    )
    .unwrap();

    assert_eq!(items.len(), 2);
    let expanded: Vec<(String, String)> = items
        .into_iter()
        .map(|item| match item {
            DocumentItem::Glyph { name, body } => (name.display(), body.refs[0].name.clone()),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        expanded,
        vec![
            ("out-a".to_string(), "dep-1".to_string()),
            ("out-b".to_string(), "dep-2".to_string()),
        ],
    );
}

#[test]
fn glyph_block_group_mult() {
    let items =
        expand_glyph_block(&GlyphName("out-(a|b**3)".to_string()), &ref_body("base")).unwrap();

    let names: Vec<String> = items
        .into_iter()
        .map(|item| match item {
            DocumentItem::Glyph { name, .. } => name.display(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        names,
        vec!["out-a", "out-a", "out-a", "out-b", "out-b", "out-b",],
    );
}

#[test]
fn glyph_block_group_mult_with_individual_repeats() {
    let items =
        expand_glyph_block(&GlyphName("out-(a*2|b**3)".to_string()), &ref_body("base")).unwrap();

    let names: Vec<String> = items
        .into_iter()
        .map(|item| match item {
            DocumentItem::Glyph { name, .. } => name.display(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "out-a", "out-a", "out-a", "out-a", "out-a", "out-a", "out-b", "out-b", "out-b",
        ],
    );
}

#[test]
fn glyph_block_expands_to_its_largest_alternation_group() {
    let items = expand_glyph_block(
        &GlyphName("out-(a|b)-(1|2|3)".to_string()),
        &ref_body("base"),
    )
    .unwrap();

    let names: Vec<String> = items
        .into_iter()
        .map(|item| match item {
            DocumentItem::Glyph { name, .. } => name.display(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "out-a-1".to_string(),
            "out-b-2".to_string(),
            "out-a-3".to_string(),
        ],
    );
}

#[test]
fn compute_docline_file_lines_skips_omitted_empty_grids() {
    use crate::document_io::serialize_doclines;
    use crate::pixel::{PX_ALMOSTFULL, PX_FULL, PixelShape};

    // "glyph a" has declared dims but an all-empty grid, which the
    // serializer omits entirely; lines after it must still map to their
    // real (post-omission) line numbers in the serialized file.
    let mut filled = PixelGrid::new(1, 1);
    filled.set(0, 0, PixelShape(PX_ALMOSTFULL | PX_FULL));

    let lines = vec![
        DocLine::Text("glyph a 2 2".to_string()),
        DocLine::Grid(PixelGrid::new(2, 2)),
        DocLine::Text("glyph b 1 1".to_string()),
        DocLine::Grid(filled),
        DocLine::Text("map A = b".to_string()),
    ];

    let file_lines = compute_docline_file_lines(&lines);
    assert_eq!(file_lines, vec![0, 1, 1, 2, 3]);

    // Cross-check against the actual serialized output.
    let mut buf = Vec::new();
    serialize_doclines(&lines, &mut buf).unwrap();
    let serialized = String::from_utf8(buf).unwrap();
    let serialized_lines: Vec<&str> = serialized.lines().collect();
    assert_eq!(serialized_lines.len(), 4);
    assert_eq!(serialized_lines[file_lines[0]], "glyph a 2 2");
    assert_eq!(serialized_lines[file_lines[2]], "glyph b 1 1");
    assert_eq!(serialized_lines[file_lines[4]], "map A = b");
}

#[cfg(feature = "editor")]
#[test]
fn snap_details_keeps_straight_edges_straight() {
    // The top half of a logical pixel at scale 2, rescaled to scale 3:
    // the middle row of cells is half covered by a rectangle no shape
    // code can spell. Snapping must round it to a full cell rather than
    // break the straight edge into a row of triangles.
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    let mut g = PixelGrid::new(2, 2);
    g.set(0, 0, full);
    g.set(0, 1, full);

    let mut r = g.rescale(2, 3);
    assert!(
        !r.details.is_empty(),
        "the exact rescale keeps the geometry"
    );
    r.snap_details_to_catalog();
    assert!(r.details.is_empty());
    assert_eq!(r.den, 1);
    for col in 0..3 {
        assert_eq!(r.get(0, col), full, "row 0 col {col}");
        assert_eq!(r.get(1, col), full, "row 1 col {col}");
        assert_eq!(
            r.get(2, col).shape_id(),
            crate::pixel::PX_EMPTY,
            "row 2 col {col}"
        );
    }
}

#[cfg(feature = "editor")]
#[test]
fn snap_details_keeps_diagonals_diagonal() {
    // Same rescale over a diagonal: the cells the diagonal crosses do
    // have a diagonal boundary, so they keep a diagonal shape code
    // instead of rounding to a staircase of full cells.
    let mut g = PixelGrid::new(2, 2);
    g.set(0, 1, PixelShape::new(crate::pixel::PX_HALF1, true));

    let exact = g.rescale(2, 3);
    let mut r = exact.clone();
    r.snap_details_to_catalog();
    assert!(r.details.is_empty());
    let rows: Vec<String> = (0..3)
        .map(|row| {
            (0..3)
                .map(|col| {
                    crate::pixel::shape_to_chars(r.get(row, col))
                        .iter()
                        .collect::<String>()
                })
                .collect()
        })
        .collect();
    assert_eq!(rows, ["..\\bb.", "....\\b", "......"]);
    assert_eq!(
        exact.get(0, 1).shape_id(),
        PX_CUSTOM,
        "this cell needed snapping"
    );
}

#[test]
fn rescale_up() {
    // 2×2 grid at scale 1, rescale to scale 2 → 4×4
    let mut g = PixelGrid::new(2, 2);
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    g.set(0, 0, full); // top-left filled
    g.set(1, 1, full); // bottom-right filled

    let r = g.rescale(1, 2);
    assert_eq!((r.width, r.height), (4, 4));
    // Each source pixel becomes a 2×2 block
    assert_eq!(r.get(0, 0), full);
    assert_eq!(r.get(0, 1), full);
    assert_eq!(r.get(1, 0), full);
    assert_eq!(r.get(1, 1), full);
    assert_eq!(r.get(0, 2), PixelShape::EMPTY);
    assert_eq!(r.get(2, 0), PixelShape::EMPTY);
    assert_eq!(r.get(2, 2), full);
    assert_eq!(r.get(2, 3), full);
    assert_eq!(r.get(3, 2), full);
    assert_eq!(r.get(3, 3), full);
}

#[test]
fn rescale_down() {
    // 4×4 grid at scale 2, rescale to scale 1 → 2×2
    let mut g = PixelGrid::new(4, 4);
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    for r in 0..2 {
        for c in 0..2 {
            g.set(r, c, full);
        }
    }
    for r in 2..4 {
        for c in 2..4 {
            g.set(r, c, full);
        }
    }

    let r = g.rescale(2, 1);
    assert_eq!((r.width, r.height), (2, 2));
    assert_eq!(r.get(0, 0), full);
    assert_eq!(r.get(0, 1), PixelShape::EMPTY);
    assert_eq!(r.get(1, 0), PixelShape::EMPTY);
    assert_eq!(r.get(1, 1), full);
}

#[test]
fn rescale_fractional_ratio() {
    // 6×3 grid at scale 3, rescale to scale 2 → 4×2
    let mut g = PixelGrid::new(6, 3);
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    for r in 0..3 {
        for c in 0..3 {
            g.set(r, c, full);
        }
    }

    let r = g.rescale(3, 2);
    assert_eq!((r.width, r.height), (4, 2));
    assert_eq!(r.get(0, 0), full);
    assert_eq!(r.get(0, 1), full);
    assert_eq!(r.get(0, 2), PixelShape::EMPTY);
    assert_eq!(r.get(1, 0), full);
}

#[test]
fn rescale_up_subpixel_shape_exact() {
    // A HALF1 diagonal upscaled 3× must stay one straight diagonal:
    // cells on the diagonal become HALF1, cells below it full, cells
    // above it empty — all plain codes, no details. (The former
    // nearest-neighbor rescale duplicated the diagonal into every
    // cell, visibly snapping mixed-scale composites to one grid.)
    let mut g = PixelGrid::new(1, 1);
    g.set(0, 0, PixelShape::new(crate::pixel::PX_HALF1, true));
    let r = g.rescale(1, 3);
    assert_eq!((r.width, r.height), (3, 3));
    assert!(r.details.is_empty());
    for row in 0..3u16 {
        for col in 0..3u16 {
            let expected = if row == col {
                crate::pixel::PX_HALF1
            } else if row > col {
                PX_ALMOSTFULL
            } else {
                crate::pixel::PX_EMPTY
            };
            assert_eq!(r.get(row, col).shape_id(), expected, "cell ({row}, {col})");
        }
    }
}

#[test]
fn rescale_fractional_creates_exact_details() {
    // A logical pixel two-thirds covered (2 of 3 columns full at scale
    // 3) rescaled to scale 2: the filled region is 4/3 destination
    // pixels wide. The right column's sliver is not representable as a
    // plain code and must become an exact custom detail, and the
    // contour tracer must produce a single clean rectangle outline.
    let mut g = PixelGrid::new(3, 3);
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    for r in 0..3 {
        for c in 0..2 {
            g.set(r, c, full);
        }
    }

    let out = g.rescale(3, 2);
    assert_eq!((out.width, out.height), (2, 2));
    assert_eq!(out.get(0, 0).shape_id(), PX_ALMOSTFULL);
    assert_eq!(out.get(1, 0).shape_id(), PX_ALMOSTFULL);
    assert_eq!(out.get(0, 1).shape_id(), PX_CUSTOM);
    assert_eq!(out.get(1, 1).shape_id(), PX_CUSTOM);
    let d = out.details.get(&(0, 1)).unwrap();
    assert_eq!(d.den, 3);
    assert_eq!(d.area2(), 2.0 / 3.0);

    let paths = crate::render::contour::track_contour(&out, crate::pixel::PX_SUBPIXEL);
    assert_eq!(paths.len(), 1, "one rectangle outline, got {paths:?}");
    let mut pts = paths[0].clone();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let expected = [
        (0.0f32, 0.0f32),
        (0.0, 2.0),
        (4.0 / 3.0, 0.0),
        (4.0 / 3.0, 2.0),
    ];
    assert_eq!(pts.len(), 4, "rectangle has 4 corners: {pts:?}");
    for (p, e) in pts.iter().zip(expected.iter()) {
        assert!(
            (p.0 - e.0).abs() < 1e-5 && (p.1 - e.1).abs() < 1e-5,
            "vertex {p:?} != {e:?} in {pts:?}"
        );
    }
}

/// A hardblank is a *claim* on a cell, not geometry, so it composes on its
/// own level: a claim survives being blitted over another claim, ink wins
/// over a claim either way round, and only a claim cancels a claim.
///
/// The table is the whole rule. `blit` used to route every non-empty pair
/// through the region layer, where a hardblank reads as the empty region it
/// draws — so two overlapping claims annihilated into a truly empty cell
/// and a claim under ink was erased by a negated blit. Both silently
/// changed what [`crate::compose::InkProfile`] measures.
#[test]
fn blit_composes_hardblanks_on_their_own_level() {
    use crate::pixel::PX_HARDBLANK;
    let hb = PixelShape::new(PX_HARDBLANK, false);
    let ink = PixelShape::new(PX_ALMOSTFULL, true);

    // (destination, source, negated) -> expected
    let cases: [(PixelShape, PixelShape, bool, PixelShape); 8] = [
        (hb, hb, false, hb),
        (hb, ink, false, ink),
        (ink, hb, false, ink),
        (PixelShape::EMPTY, hb, false, hb),
        (hb, hb, true, PixelShape::EMPTY),
        (hb, ink, true, hb),
        (ink, hb, true, ink),
        (PixelShape::EMPTY, hb, true, PixelShape::EMPTY),
    ];

    for (dst_shape, src_shape, negated, expected) in cases {
        let mut dst = PixelGrid::new(1, 1);
        dst.set(0, 0, dst_shape);
        let mut src = PixelGrid::new(1, 1);
        src.set(0, 0, src_shape);
        dst.blit(&src, 0, 0, negated);
        assert_eq!(
            dst.get(0, 0).0,
            expected.0,
            "{:?} {} {:?}",
            dst_shape.0,
            if negated { "-" } else { "|" },
            src_shape.0,
        );
    }
}

/// Rescaling carries a hardblank the same way it carries ink: the cells a
/// claim covers in the destination are claimed too, and a destination cell
/// that covers both a claim and ink comes out inked (`pixel::blank_op`).
///
/// `rescale` used to ask only whether a cell's shape id was `PX_EMPTY`,
/// which put a hardblank on the geometry path — where it reads as the empty
/// region it draws — so the claim vanished in both directions and silently
/// changed what [`crate::compose::InkProfile`] measures.
#[test]
fn rescale_carries_hardblanks() {
    use crate::pixel::PX_HARDBLANK;
    let hb = PixelShape::new(PX_HARDBLANK, false);
    let ink = PixelShape::new(PX_ALMOSTFULL, true);

    // Up: one claim becomes the 2×2 block of claims it covers.
    let mut g = PixelGrid::new(2, 1);
    g.set(0, 0, hb);
    g.set(0, 1, ink);
    let up = g.rescale(1, 2);
    assert_eq!((up.width, up.height), (4, 2));
    for r in 0..2u16 {
        for c in 0..2u16 {
            assert_eq!(up.get(r, c).0, hb.0, "claim at ({r}, {c})");
            assert_eq!(up.get(r, c + 2).0, ink.0, "ink at ({r}, {})", c + 2);
        }
    }

    // Down: a block of claims is one claim, and it round-trips.
    let down = up.rescale(2, 1);
    assert_eq!((down.width, down.height), (2, 1));
    assert_eq!(down.get(0, 0).0, hb.0);
    assert_eq!(down.get(0, 1).0, ink.0);

    // Down over a mixed block: the ink outranks the claim, and it keeps
    // the quarter of the cell it actually covers.
    let mut m = PixelGrid::new(2, 2);
    m.set(0, 0, hb);
    m.set(0, 1, hb);
    m.set(1, 0, hb);
    m.set(1, 1, ink);
    let mixed = m.rescale(2, 1);
    assert_eq!((mixed.width, mixed.height), (1, 1));
    assert!(!mixed.get(0, 0).is_contour_empty());
    assert!(mixed.get(0, 0).is_bitmap_filled());
    assert_eq!(mixed.region_at(0, 0).area2(), 0.5);

    // A fractional ratio: the destination cell covering only source
    // claims is a claim, the one covering ink is ink.
    let mut f = PixelGrid::new(3, 3);
    for r in 0..3u16 {
        f.set(r, 0, ink);
        f.set(r, 1, hb);
        f.set(r, 2, hb);
    }
    let frac = f.rescale(3, 2);
    assert_eq!((frac.width, frac.height), (2, 2));
    for r in 0..2u16 {
        assert!(!frac.get(r, 0).is_contour_empty(), "ink at ({r}, 0)");
        assert_eq!(frac.get(r, 1).0, hb.0, "claim at ({r}, 1)");
    }
}

/// A subcell with no geometry but the ink flag set — what `BitmapFill`
/// writes beside the geometry of a logical pixel the bitmap face inks —
/// keeps its flag through a rescale, which is the OR
/// [`crate::on_demand`]'s `apply_bitmap_fill` documents relying on.
#[test]
fn rescale_carries_bitmap_fill_without_geometry() {
    let mut g = PixelGrid::new(2, 2);
    let ink = PixelShape::new(PX_ALMOSTFULL, true);
    let fill_only = PixelShape::new(crate::pixel::PX_EMPTY, true);
    g.set(0, 0, ink);
    g.set(0, 1, fill_only);
    g.set(1, 0, fill_only);
    g.set(1, 1, fill_only);

    let up = g.rescale(2, 3);
    assert_eq!((up.width, up.height), (3, 3));
    for r in 0..3u16 {
        for c in 0..3u16 {
            assert!(
                up.get(r, c).is_bitmap_filled(),
                "cell ({r}, {c}) lost its ink flag"
            );
        }
    }
}

#[test]
fn blit_negated_subtracts_exactly() {
    // Subtracting a third-of-a-pixel bar from a full pixel leaves an
    // exact custom remainder instead of a raster-snapped catalog shape.
    let mut dst = PixelGrid::new(1, 1);
    dst.set(0, 0, PixelShape::new(PX_ALMOSTFULL, true));

    let mut src = PixelGrid::new(1, 1);
    let bar = crate::detail::DetailRegion {
        den: 3,
        rings: vec![vec![(0, 0), (1, 0), (1, 3), (0, 3)]],
    };
    src.set_detail(0, 0, &bar, true);

    dst.blit(&src, 0, 0, true);
    assert_eq!(dst.get(0, 0).shape_id(), PX_CUSTOM);
    let d = dst.details.get(&(0, 0)).unwrap();
    assert_eq!(d.area2(), 2.0 * 2.0 / 3.0);
}
