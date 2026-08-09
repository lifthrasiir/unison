//! Tests for the `refonly` glyph flag: a pixel grid that is bitmap ink only.
//!
//! The font is built twice from the same source (see `ttf_builder`'s module
//! docs), so the flag is only meaningful as a *difference* between the two
//! builds — every test here builds both and compares.

use super::*;

fn contours_of(input: &str, bitmap: bool, name: &str) -> Vec<Vec<(i16, i16)>> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], bitmap).expect("expected glyph data");
    let glyph = glyphs
        .iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("glyph '{name}' is missing from the build"));
    let mut contours: Vec<Vec<(i16, i16)>> = glyph
        .contours
        .iter()
        .map(|c| canonicalize_contour(c))
        .collect();
    contours.sort();
    contours
}

/// The whole point of the flag: the same source, built twice, resolves the
/// glyph from its refs alone once and from its own grid alone the other time.
/// `refs` and `pixels` are the two halves written out as ordinary glyphs, so
/// the assertion is against what each build *should* have produced rather
/// than against a hard-coded outline.
#[test]
fn refonly_grid_draws_the_bitmap_face_only() {
    let input = "\
meta height 4
meta ascent 4
meta descent 0

glyph both refonly 2 2
@@..
@@..
ref 2x1:zero 0 0
map A = both

glyph refs-half
ref 2x1:zero 0 0
map B = refs-half

glyph pixels-half 2 2
@@..
@@..
map C = pixels-half
";
    let vector_both = contours_of(input, false, "both");
    let vector_refs = contours_of(input, false, "refs-half");
    let vector_pixels = contours_of(input, false, "pixels-half");
    assert!(!vector_refs.is_empty(), "`2x1:zero` has a vector outline");
    assert_eq!(
        vector_both, vector_refs,
        "the vector build must resolve a refonly glyph from its refs alone"
    );
    assert_ne!(
        vector_both, vector_pixels,
        "the refonly grid must not reach the vector build"
    );

    let bitmap_both = contours_of(input, true, "both");
    let bitmap_refs = contours_of(input, true, "refs-half");
    let bitmap_pixels = contours_of(input, true, "pixels-half");
    assert!(
        bitmap_refs.is_empty(),
        "`:zero` lights no pixel, so the ref contributes no bitmap ink"
    );
    assert!(!bitmap_pixels.is_empty(), "the grid is lit");
    assert_eq!(
        bitmap_both, bitmap_pixels,
        "the bitmap build must draw the refonly grid"
    );
}

/// A refonly glyph with no refs at all is blank in the vector build. Its
/// declared dimensions are still its dimensions — the grid is what says how
/// wide the glyph is, whichever build reads it.
#[test]
fn refonly_glyph_without_refs_is_blank_in_the_vector_build() {
    let input = "\
meta height 4
meta ascent 4
meta descent 0

glyph blank refonly 2 2
@@@@
@@@@
map A = blank

glyph plain 2 2
@@@@
@@@@
map B = plain
";
    assert!(
        contours_of(input, false, "blank").is_empty(),
        "nothing is left to draw in the vector build"
    );
    assert_eq!(
        contours_of(input, true, "blank"),
        contours_of(input, true, "plain"),
        "the bitmap build is unaffected"
    );

    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let advance = |name: &str| {
        glyphs
            .iter()
            .find(|g| g.name == name)
            .expect("glyph present")
            .advance_width
    };
    assert_eq!(
        advance("blank"),
        advance("plain"),
        "a suppressed outline must not shrink the advance"
    );
}

/// A parent referencing a refonly glyph sees the same two faces: the refonly
/// grid is absent from its vector outline and present in its bitmap one. The
/// parent has own pixels here on purpose, since that is the path that
/// composes ref *grids* rather than translating their contours.
#[test]
fn a_ref_to_a_refonly_glyph_carries_the_same_split() {
    let input = "\
meta height 4
meta ascent 4
meta descent 0

glyph inner refonly 2 2
@@..
@@..
ref 2x1:zero 0 1

glyph outer 2 2
....
..@@
ref inner 0 0
map A = outer

glyph outer-vector 2 2
....
..@@
ref 2x1:zero 0 1
map B = outer-vector
";
    assert_eq!(
        contours_of(input, false, "outer"),
        contours_of(input, false, "outer-vector"),
        "the refonly grid must not reach the parent's vector outline"
    );
    let bitmap_outer = contours_of(input, true, "outer");
    assert_ne!(
        bitmap_outer,
        contours_of(input, true, "outer-vector"),
        "the parent's bitmap must include the refonly grid"
    );
    assert!(!bitmap_outer.is_empty());
}

/// A pattern glyph block's flags belong to every glyph it expands to, and
/// `refonly` is invisible in the bitmap face — dropping it there builds a
/// clean font whose vector face silently regains the grid.
#[test]
fn pattern_glyph_expansions_keep_the_refonly_flag() {
    let input = "\
meta height 4
meta ascent 4
meta descent 0

glyph dot 1 1
@@

glyph p-(a|b) refonly 2 2
@@..
@@..
ref dot 0 0
map A = p-a
map B = p-b
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    for bitmap in [false, true] {
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], bitmap).unwrap();
        for name in ["p-a", "p-b"] {
            let g = glyphs.iter().find(|g| g.name == name).expect("expanded");
            // One 1x1 ref in the vector build; the 1x2 own grid plus that
            // ref's cell in the bitmap one.
            let n = g.contours.iter().map(|c| c.len()).sum::<usize>();
            assert!(
                n > 0,
                "{name} should draw something in the {} build",
                if bitmap { "bitmap" } else { "vector" }
            );
        }
        let a = glyphs.iter().find(|g| g.name == "p-a").unwrap();
        let expect_grid = bitmap;
        // The own grid covers column 0, rows 0..2; the ref covers a single
        // cell. Winding at the center of cell (row 1, col 0) is nonzero only
        // when the grid is drawn.
        let cell = UNITS_PER_EM as f32 / 4.0;
        let inside = winding_at(&a.contours, cell * 0.5, cell * 2.5) != 0;
        assert_eq!(
            inside,
            expect_grid,
            "the refonly grid should {} the {} build",
            if expect_grid { "reach" } else { "stay out of" },
            if bitmap { "bitmap" } else { "vector" }
        );
    }
}
