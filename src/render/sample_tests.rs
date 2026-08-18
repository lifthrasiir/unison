//! Tests for [`crate::render::sample`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;
use crate::document_io;

fn parse(input: &str) -> Document {
    document_io::parse_document_from_str(input, "test.unf".into()).unwrap()
}

#[test]
fn subdivision_flag_is_a_tag_sequence() {
    assert_eq!(
        subdivision_flag_seq("gbsct").as_deref(),
        Some("\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}")
    );
    // UTS #51 restricts tag_spec to [0-9a-z], 1..=6 characters.
    assert_eq!(subdivision_flag_seq("us-tx"), None);
    assert_eq!(subdivision_flag_seq("GBSCT"), None);
    assert_eq!(subdivision_flag_seq(""), None);
    assert_eq!(subdivision_flag_seq("abcdefg"), None);
}

#[test]
fn sample_selects_alternative_glyph_on_anchor_size_mismatch() {
    // Mirrors ref_composite::tests::alternative_glyph_selected_on_size_mismatch,
    // but exercised through the sample-rendering path (collect_sample_data),
    // which used to never consider alternatives because it passed
    // `|_| Vec::new()` as `lookup_alternatives`.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 0..1 0

glyph container 6 2
............
............
anchor +join 3..4 0
ref stem

map A = container
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let container = data
        .glyphs
        .get("container")
        .expect("container glyph present");
    // stem (1-wide -join) doesn't size-match +join (2-wide), so stem:wide
    // (2-wide -join) must be selected instead, placed at offset col=3.
    // Total width becomes max(6, 3 + 4) = 7; without alternative
    // selection, stem (width 2) is placed at (0, 0) giving width 6.
    assert_eq!(
        container.width, 7,
        "stem:wide should have been selected via anchor-size matching"
    );
}

#[test]
fn sample_includes_map_decomposed_composite_glyph() {
    // `map <precomposed char>` (DocumentItem::MapDecomposed) synthesizes a
    // composite glyph via NFD decomposition; it used to be silently
    // skipped when collecting sample data.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

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
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let gid = data
        .cmap
        .get(&('ä' as u32))
        .cloned()
        .expect("'a with combining diaeresis' should be mapped in cmap");
    assert!(
        data.glyphs.contains_key(&gid),
        "sample glyph entry should exist for the map-decomposed character"
    );
}

#[test]
fn sample_map_decomposed_mark_does_not_widen_advance() {
    // A zero-advance mark glyph (`glyph m 0 H mark` with a ref at a
    // negative column) used to have its own all-empty declared grid
    // treated as a real layer, which shifted the whole composite to
    // positive columns and gave the mark a non-zero width.  The
    // `map <precomposed>` composite then laid the mark out *after* the
    // base instead of anchoring it on top, inflating the advance.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph a-lower 16 16
................................
................................
................................
................................
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
................................
................................
................................
................................
anchor +above 13 2

glyph dia0 5 5
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph dia-above 0 16 mark
ref dia0 -5 3
anchor -above -3 3

map a = a-lower
map \u{0308} = dia-above
map generate ä
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");

    let mark = data
        .glyphs
        .get("dia-above")
        .expect("mark should be in sample glyphs");
    assert_eq!(
        mark.width, 0,
        "a `0 H mark` glyph whose ref sits at a negative column must keep width 0"
    );

    let gid = data
        .cmap
        .get(&('ä' as u32))
        .cloned()
        .expect("precomposed char should be mapped");
    let composite = data.glyphs.get(&gid).expect("composite sample glyph");
    assert_eq!(
        composite.width, 16,
        "the mark should be absorbed into the base advance, not appended after it"
    );
}

#[test]
fn sample_composite_survives_gridless_ref() {
    // A ref to a glyph with no raster grid (a `keep` placeholder, or a
    // composite that fell back to empty) used to abort the *whole*
    // composite in the simple no-own-pixels branch (`sg.as_ref()?`),
    // rendering it empty — while the TTF builder skips just that ref's
    // grid and keeps the rest (`from_components_inner`).
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph placeholder keep

glyph part 2 2
@@@@
@@@@

glyph combo
ref placeholder
ref part

map A = combo
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data.glyphs.get("combo").expect("combo should be present");
    assert_eq!(
        g.width, 2,
        "the real ref must survive a grid-less sibling ref"
    );
    assert!(
        !g.components.is_empty(),
        "the real ref's components must be kept"
    );
}

/// A composite that subtracts is not a list of independent layers, so its
/// parts must not be spliced into a parent's flat layer list: the child's
/// negated part would go on erasing the parent's own ink under it.
#[test]
fn sample_nested_negation_does_not_erase_the_parent() {
    let d = parse(
        "\
meta height 3
meta ascent 3
meta descent 0

glyph box 3 3
@@@@@@
@@@@@@
@@@@@@

glyph dot 1 1
@@

glyph ring
ref box
ref dot 1 1 negated

glyph combo
ref box
ref ring

map A = combo
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let comps = data.glyphs["combo"].normalized_components();
    let bitmap = composite_components(3, 3, &comps);
    assert!(
        bitmap.get(1, 1).is_bitmap_filled(),
        "the ring's hole must not punch through the parent's own solid layer"
    );
}

/// A negated `ref` subtracts the target's *result*. Flipping each of the
/// target's layers instead turns its holes into ink the parent never had.
#[test]
fn sample_negated_ref_subtracts_the_composed_target() {
    let d = parse(
        "\
meta height 3
meta ascent 3
meta descent 0

glyph box 3 3
@@@@@@
@@@@@@
@@@@@@

glyph dot 1 1
@@

glyph ring
ref box
ref dot 1 2 negated

glyph top 3 2
@@@@@@
@@@@@@

glyph combo
ref top
ref ring negated

map A = combo
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let comps = data.glyphs["combo"].normalized_components();
    let bitmap = composite_components(3, 3, &comps);
    for r in 0..3u16 {
        for c in 0..3u16 {
            assert!(
                !bitmap.get(r, c).is_bitmap_filled(),
                "({r}, {c}): subtracting a superset must leave nothing; \
                     the target's hole is not ink"
            );
        }
    }
}

/// U+25CC: a `desync` grid whose glyph also refs a composite that
/// subtracts. The bitmap ink is the glyph's own layer and the ref's
/// internal negation has no business erasing it.
#[test]
fn sample_desync_bitmap_survives_a_negating_ref() {
    let d = parse(
        "\
meta height 3
meta ascent 3
meta descent 0

glyph box 3 3
@@@@@@
@@@@@@
@@@@@@

glyph dot 1 1
@@

glyph ring
ref box
ref dot 1 1 negated

glyph g desync 3 3
@@@@@@
@@@@@@
@@@@@@
ref ring

map A = g
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let comps = data.glyphs["g"].normalized_components();
    let bitmap = composite_components(3, 3, &comps);
    for r in 0..3u16 {
        for c in 0..3u16 {
            assert!(
                bitmap.get(r, c).is_bitmap_filled(),
                "({r}, {c}): the desync grid is the bitmap face's ink"
            );
        }
    }
}

/// The sample draws small glyphs from the ink flags (the bitmap face) and
/// large ones from the sub-pixel geometry (the vector face), so a
/// `desync` grid has to appear in the first and not in the second — the
/// same split the TTF builder's two passes make.
#[test]
fn sample_desync_grid_is_bitmap_ink_only() {
    let d = parse(
        "\
meta height 4
meta ascent 4
meta descent 0

glyph g desync 2 2
@@..
@@..
ref 2x1:zero 0 1

map A = g
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data.glyphs.get("g").expect("g should be present");
    let comps = g.normalized_components();

    // Small (full-pixel) rendering: the grid's own ink, and nothing from
    // the `:zero` ref, which lights no pixel.
    let bitmap = composite_components(2, 4, &comps);
    assert!(
        bitmap.get(0, 0).is_bitmap_filled() && bitmap.get(1, 0).is_bitmap_filled(),
        "the desync grid is what the bitmap face draws"
    );
    assert!(
        !bitmap.get(1, 1).is_bitmap_filled(),
        "`:zero` contributes no bitmap ink"
    );

    // Large (sub-pixel) rendering: the ref's geometry only.
    let vector: Vec<&SampleComponent> = comps.iter().filter(|c| !c.desync).collect();
    assert!(
        vector
            .iter()
            .all(|c| c.grid.get(1, 0).is_clear() || !c.grid.get(1, 0).is_bitmap_filled()),
        "no vector layer may carry the desync grid's ink"
    );
    let vector_ink = vector.iter().any(|c| {
        (0..c.grid.height).any(|r| (0..c.grid.width).any(|x| !c.grid.get(r, x).is_clear()))
    });
    assert!(vector_ink, "the `:zero` ref still has a vector outline");
    assert_eq!(
        comps.iter().filter(|c| c.desync).count(),
        1,
        "the own grid is the one desync layer"
    );
}

#[test]
fn sample_expanded_glyph_retains_declared_pixel_dims() {
    // Callsites of expand_glyph_block used to copy over the expanded
    // glyph items but drop `body.pixels`, so a pattern-named glyph with
    // declared dims + an all-empty grid + refs lost its declared
    // width/height in sample rendering.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph part 2 2
@@@@
@@@@

glyph test-(a|b) 4 4
........
........
........
........
ref part

map A = test-a
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data
        .glyphs
        .get("test-a")
        .expect("test-a should be in sample glyphs");
    assert_eq!(
        g.width, 4,
        "expanded glyph should retain its declared width despite an empty own grid"
    );
}

#[test]
fn sample_display_metrics_reflects_a_vertical_bearing() {
    // `sample_display_metrics` used to hardcode the vertical offset to 0, so
    // a glyph's declared origin had no effect in sample output.
    let sg_with_top = SampleGlyph {
        width: 5,
        _height: 5,
        components: Vec::new(),
        origin_row: 0,
        origin_col: 0,
        left: 0,
        top: 3,
        declared_width: None,
        scale: 1,
    };
    let (_, _, _, row_off) = sample_display_metrics(&sg_with_top, 16);
    assert_eq!(row_off, 3);

    let sg_without_top = SampleGlyph {
        width: 5,
        _height: 5,
        components: Vec::new(),
        origin_row: 0,
        origin_col: 0,
        left: 0,
        top: 0,
        declared_width: None,
        scale: 1,
    };
    let (_, _, _, row_off) = sample_display_metrics(&sg_without_top, 16);
    assert_eq!(row_off, 0);
}

#[test]
fn sample_keeps_negative_ref_offsets_as_bearings() {
    // The sample used to normalize a negative ref offset away, so its
    // idea of the glyph disagreed with the font the builder emitted.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph part 2 2
@@@@
@@@@

glyph shifted 2 2
@@@@
@@@@
ref part -1 0

map A = shifted
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data
        .glyphs
        .get("shifted")
        .expect("shifted should be in sample glyphs");
    assert_eq!((g.origin_col, g.width), (-1, 2));
    let (display_w, _, col_off, _) = sample_display_metrics(g, data.height);
    assert_eq!((display_w, col_off), (3, 1));
}

#[test]
fn blank_margin_before_the_origin_is_not_a_bearing() {
    // Pulling a ref up into its own empty top rows is the usual way to
    // nudge a composite; nothing is drawn before the origin, so it must
    // not become a bearing and must not pad the sample cell.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

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
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data
        .glyphs
        .get("raised")
        .expect("raised should be in sample glyphs");
    assert_eq!((g.origin_row, g.origin_col), (0, 0));
    assert_eq!(sample_display_metrics(g, data.height), (2, 16, 0, 0));
}

/// The white a sample cell paints is the glyph's *declared box*, not the
/// area its ink happens to need: a combining mark declares no width, so it
/// draws onto the page instead of onto a cell of its own, which is the one
/// way a sample shows an advance of zero at all.
///
/// Only the width comes from the declared box. Its height is the grid's
/// whenever the source does not say otherwise, and for a mark that is the
/// two rows of ink sitting fourteen rows below the box's own corner — a
/// rectangle nothing would want painted. The em box is what a cell is tall,
/// so that is what the background keeps.
#[test]
fn the_sample_background_is_the_declared_box_not_the_ink() {
    let mark = SampleGlyph {
        width: 6,
        _height: 2,
        components: Vec::new(),
        origin_row: 0,
        origin_col: 0,
        left: -3,
        top: 14,
        declared_width: Some(0),
        scale: 1,
    };
    let (dw, dh, ..) = sample_display_metrics(&mark, 16);
    assert_eq!((dw, dh), (9, 16), "the cell still holds the ink");
    assert_eq!(
        sample_background(&mark, 16),
        (3, 0, 16),
        "a zero-width mark paints nothing, at the pen"
    );

    let plain = SampleGlyph {
        width: 8,
        _height: 16,
        components: Vec::new(),
        origin_row: 0,
        origin_col: 0,
        left: 0,
        top: 0,
        declared_width: Some(8),
        scale: 1,
    };
    assert_eq!(sample_background(&plain, 16), (0, 8, 16));

    // A composite with no box of its own: the raster is all there is.
    let composite = SampleGlyph {
        declared_width: None,
        ..plain
    };
    assert_eq!(sample_background(&composite, 16), (0, 8, 16));
}

/// A `ref` offset names the target's *box* corner, so the sample has to
/// take that box out of the offset exactly as the builder does — or every
/// glyph placing a combining mark draws it shifted by the mark's own
/// bearing, which is the sample being wrong about a font that is right.
#[test]
fn the_sample_places_a_ref_by_the_targets_box() {
    let source = |flags: &str, offset: &str| {
        format!(
            "\
meta height 16
meta ascent 12
meta descent 4

glyph part 2 2 {flags}
@@@@
@@@@

glyph places
ref part {offset}

map A = places
"
        )
    };
    let placed = |flags: &str, offset: &str| {
        let d = parse(&source(flags, offset));
        let data = collect_sample_data(&[&d]).expect("sample data");
        let g = data.glyphs.get("places").expect("the parent is sampled");
        g.components
            .iter()
            .map(|c| (c.row, c.col))
            .collect::<Vec<_>>()
    };

    // The same drawing in the same place, spelled two ways: the box's
    // corner one column into the grid, and the offset that names it moved
    // to match.
    assert_eq!(placed("origin 1 0", "3 0"), placed("", "2 0"));
}

/// The width the background is painted over comes from the source's own
/// declared box, which is the half of it a `map`ped mark actually states.
#[test]
fn a_marks_declared_width_reaches_the_sample() {
    let d = parse(
        "\
meta height 16
meta ascent 14
meta descent 2

glyph dia-below 6 2 mark advance 0 origin 3 -14
..............
@@@@@@@@@@@@..

map \u{0323} = dia-below

glyph a 8 16 advance 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

map A = a
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data");
    let mark = data.glyphs.get("dia-below").expect("the mark is sampled");
    assert_eq!(mark.declared_width, Some(0));
    assert_eq!(sample_background(mark, data.height).1, 0);
}

#[test]
fn sample_display_metrics_makes_room_for_negative_bearings() {
    // A glyph reaching before its origin — via a negative ref offset or a
    // declared `origin` — used to be drawn at cell column 0 and clipped.
    let with_negative_origin = SampleGlyph {
        width: 8,
        _height: 16,
        components: Vec::new(),
        origin_row: -1,
        origin_col: -3,
        left: 0,
        top: 0,
        declared_width: None,
        scale: 1,
    };
    let (w, h, col_off, row_off) = sample_display_metrics(&with_negative_origin, 16);
    assert_eq!((w, h, col_off, row_off), (11, 17, 3, 1));

    let with_negative_left = SampleGlyph {
        width: 8,
        _height: 16,
        components: Vec::new(),
        origin_row: 0,
        origin_col: 0,
        left: -3,
        top: 0,
        declared_width: None,
        scale: 1,
    };
    let (w, _, col_off, _) = sample_display_metrics(&with_negative_left, 16);
    assert_eq!((w, col_off), (11, 0));
}

#[test]
fn sample_fractional_on_demand_ref_rescaled_to_parent() {
    // A glyph at scale=1 referencing a fractional on-demand glyph
    // (scale=3) must rescale the ref grid to the parent scale.
    // Without rescaling, the sub-pixel grid bleeds out at 3x size.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph container 8 16
................
................
................
................
................
................
................
................
................
................
................
................
................
................
................
................
ref 4x5p1r3

map A = container
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data
        .glyphs
        .get("container")
        .expect("container glyph present");
    assert_eq!(
        g.width, 8,
        "width must match parent, not inflated by sub-pixel ref"
    );
}

#[test]
fn sample_slanted_sextant_fits_in_cell() {
    // sextant-5-dl references 4x-5p1r3-dl (triangle, scale=3).
    // The component grids must be rescaled so the rendered glyph
    // fits within the 8×16 cell.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph slant 8 16
ref 4x-5p1r3-dl 0 10

map A = slant
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data.glyphs.get("slant").expect("slant glyph present");
    assert_eq!(g.width, 8, "slanted sextant width");
    assert_components_fit(g, 8, 16, "slant");
}

#[test]
fn sample_multi_ref_slanted_sextant_fits_in_cell() {
    // sextant-1234-dl: triangle + rect, both fractional scale=3
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph combo 8 16
ref 8x10p2r3-dl
ref 8x-5p1r3 0 10

map A = combo
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data.glyphs.get("combo").expect("combo glyph present");
    assert_eq!(g.width, 8, "combo sextant width");
    assert_components_fit(g, 8, 16, "combo");
}

#[test]
fn sample_composed_slanted_sextant_fits_in_cell() {
    // Full chain: final sextant composed from -off/-on/-dl parts,
    // where the -dl part internally uses fractional scale=3 refs.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph part-off 8 16 inline
glyph part-on 8 16 inline
ref 8x10p2r3-dl
ref 8x-5p1r3 0 10

glyph final-(|1) 8 16
ref part-(off|on)

map A = final-1
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data.glyphs.get("final-1").expect("final-1 glyph present");
    assert_eq!(g.width, 8, "final-1 width");
    assert_components_fit(g, 8, 16, "final-1");
}

#[test]
fn sample_directly_mapped_scale3_glyph_normalized() {
    // A glyph declared with `scale 3` that is directly mapped must
    // have its width/height and components normalized to scale=1.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph diag 8 16 scale 3
ref 8x5p1r3-dr 0 16
ref 8x-5p1r3 0 30

map A = diag
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    let g = data.glyphs.get("diag").expect("diag glyph present");
    assert_eq!(g.width, 8, "width must be normalized to scale=1");
    assert_eq!(g.scale, 3, "scale must be preserved");
    assert_components_fit(g, 8, 16, "diag");
}

#[test]
fn sample_html_scale3_glyph_has_fractional_offsets() {
    // The large-glyph SVG path must use fractional offsets for
    // scale>1 components, not integer-truncated ones.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph diag 8 16 scale 3
ref 8x5p1r3-dr 0 16
ref 8x-5p1r3 0 30

map A = diag
",
    );
    let mut buf = Vec::new();
    let resolution = crate::resolve::Resolution::compute(&[&d]);
    write_sample_html(
        &mut buf,
        &SampleSource::collect_with(&[&d], &resolution).unwrap(),
    )
    .unwrap();
    let html = String::from_utf8(buf).unwrap();
    // Extract the large-glyph SVG (id='u41' for 'A')
    let svg = html.split("id='u41'").nth(1).unwrap();
    let svg = &svg[..svg.find("</span>").unwrap()];
    // viewBox must be 16×32 (8*2 × 16*2), not 48×32
    assert!(svg.contains("viewBox=\"0 0 16 32\""), "viewBox: {svg}");
    // The triangle ref is at row=16 in scale-3 grid → pixel 16/3=5.33
    // In SVG coords (×2): 10.666...
    // The path must NOT start at integer 10 or 0.
    let path_start = svg.split("<path").nth(1).unwrap();
    let d_attr = path_start.split("d='").nth(1).unwrap();
    let d_attr = &d_attr[..d_attr.find('\'').unwrap()];
    let y_start: f32 = d_attr
        .strip_prefix('M')
        .unwrap()
        .split(['l', 'h', 'v'])
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let expected = 16.0 / 3.0 * 2.0; // 10.666...
    assert!(
        (y_start - expected).abs() < 0.01,
        "first path y={y_start}, expected ~{expected}"
    );
}

/// The sample's cmap is the font's cmap: the mirror of
/// `ttf_tests::misc::an_ifexists_mapping_whose_target_is_absent_reaches_no_cmap_entry`.
/// A glyph nothing builds — because its own `ifexists` ref named a glyph
/// nothing defines — must take its mapping down with it here too, or the
/// sample shows a cell for a character the font does not map.
#[test]
fn an_ifexists_mapping_whose_target_is_absent_reaches_no_sample_cmap_entry() {
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph real 1 1
@@

glyph via-ref 1 1
ref real ifexists

glyph via-missing-ref 1 1
ref absent ifexists

map U+E000 = real ifexists
map U+E001 = gone ifexists
map U+E002 = via-ref
map U+E003 = via-missing-ref
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    assert!(data.cmap.contains_key(&0xE000), "{:?}", data.cmap);
    assert!(!data.cmap.contains_key(&0xE001), "{:?}", data.cmap);
    assert!(data.cmap.contains_key(&0xE002), "{:?}", data.cmap);
    assert!(!data.cmap.contains_key(&0xE003), "{:?}", data.cmap);
}

fn assert_components_fit(g: &SampleGlyph, max_w: i32, max_h: i32, label: &str) {
    let norm = g.normalized_components();
    for (i, comp) in norm.iter().enumerate() {
        let bottom = comp.row + comp.grid.height as i32;
        let right = comp.col + comp.grid.width as i32;
        assert!(
            bottom <= max_h && right <= max_w,
            "{label} component {i} overflows: row={} h={} col={} w={} (bottom={bottom}, right={right})",
            comp.row,
            comp.grid.height,
            comp.col,
            comp.grid.width,
        );
    }
}
