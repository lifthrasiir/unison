//! The bitmap build of a `scale N` glyph: a real bitmap has no subpixels, so a
//! logical pixel any of whose N×N cells is inked is lit whole.

use super::*;

/// The bitmap build's outline of `name` in `source`, flattened through its
/// components.
fn bitmap_of(source: &str, name: &str) -> Vec<Vec<(i16, i16)>> {
    let doc = document_io::parse_document_from_str(source, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], true).unwrap();
    flattened_contours(&glyphs, name)
}

const HEADER: &str = "meta height 4\nmeta ascent 3\nmeta descent 1\n\n";

/// `rows` as a `scale 1` glyph `g`, the reference every case is compared to.
fn whole(width: u16, rows: &str) -> Vec<Vec<(i16, i16)>> {
    bitmap_of(
        &format!("{HEADER}glyph g {width} 2\n{rows}\n\nmap A = g\n"),
        "g",
    )
}

#[test]
fn one_subcell_lights_its_whole_pixel() {
    let scaled = bitmap_of(
        &format!(
            "{HEADER}glyph g 2 2 scale 2\n..@@....\n........\n....@@..\n........\n\nmap A = g\n"
        ),
        "g",
    );
    assert_eq!(scaled, whole(2, "@@..\n..@@"));
}

/// A whole pixel placed half a pixel over in a `scale 2` composite straddles
/// two pixels, and lights both — through the TrueType-composite path, which
/// otherwise just translates the component's outline.
#[test]
fn a_component_off_the_pixel_grid_lights_every_pixel_it_touches() {
    let scaled = bitmap_of(
        &format!(
            "{HEADER}glyph dot 1 1 scale 2\n@@@@\n@@@@\n\n\
             glyph g 2 2 scale 2\nref dot 1 1\n\nmap A = g\n"
        ),
        "g",
    );
    assert_eq!(scaled, whole(2, "@@@@\n@@@@"));
}

/// What a negated ref leaves behind is what is quantized: a hole narrower than
/// a pixel leaves part of each pixel inked, so both stay lit.
#[test]
fn a_subpixel_hole_does_not_unlight_a_pixel() {
    let scaled = bitmap_of(
        &format!(
            "{HEADER}glyph hole 1 1 scale 2\n@@@@\n@@@@\n\n\
             glyph g 2 2 scale 2\n@@@@@@@@\n@@@@@@@@\n........\n........\nref hole 1 0 negated\n\nmap A = g\n"
        ),
        "g",
    );
    assert_eq!(scaled, whole(2, "@@@@\n...."));
}

/// `meta bitmap-axis` varies a composite only through its components, so both
/// builds must agree on which glyphs are composites. The bitmap build cannot
/// keep one whose component sits off the pixel grid, so the vector build gives
/// it up too.
#[test]
fn both_builds_inline_a_component_off_the_pixel_grid() {
    let doc = document_io::parse_document_from_str(
        &format!(
            "meta bitmap-axis\n{HEADER}glyph dot 1 1 scale 2\n@@@@\n@@@@\n\n\
             glyph g 2 2 scale 2\nref dot 1 1\n\nmap A = g\nmap B = dot\n"
        ),
        "test.unf".into(),
    )
    .unwrap();
    for bitmap in [false, true] {
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], bitmap).unwrap();
        let g = glyphs.iter().find(|g| g.name == "g").unwrap();
        assert!(
            g.composite_refs.is_empty(),
            "bitmap {bitmap}: still a composite"
        );
    }
    build_font_from_documents(&[&doc]).expect("font should build");
}
