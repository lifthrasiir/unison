//! Tests for the `vectoronly` glyph flag: a drawing that is not meant to be
//! rendered as pixels, so the bitmap build draws it as the vector build does.
//!
//! `desync`'s mirror, and tested the same way — the flag only means anything
//! as a *difference* between the two builds, so every test builds both.

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

const HEAD: &str = "\
meta height 4
meta ascent 4
meta descent 0
";

/// A sub-pixel cell is the whole test: the bitmap build normally squares it
/// off to a full pixel, and `vectoronly` says not to. The control is the same
/// drawing without the flag, so the assertion is that the flag changed the
/// bitmap build into agreeing with the vector one — not a hard-coded outline.
#[test]
fn a_vectoronly_grid_keeps_its_sub_pixel_geometry_in_the_bitmap_build() {
    let input = format!(
        "{HEAD}\
glyph plain 2 2
b...
....
glyph flagged 2 2 vectoronly
b...
....
map A = plain
map B = flagged
"
    );

    // The control: squared off in the bitmap build, sub-pixel in the vector one.
    assert_ne!(
        contours_of(&input, true, "plain"),
        contours_of(&input, false, "plain"),
        "an unflagged sub-pixel cell should differ between the two builds",
    );

    // The flag: both builds draw the vector form.
    assert_eq!(
        contours_of(&input, true, "flagged"),
        contours_of(&input, false, "flagged"),
        "vectoronly should make the bitmap build draw the vector geometry",
    );
    assert_eq!(
        contours_of(&input, false, "flagged"),
        contours_of(&input, false, "plain"),
        "the vector build is unaffected by the flag",
    );
}

/// The exemption has to reach through `ref`. The bitmap flavor squares a grid
/// off *into the cache*, so a composite exempted on its own would still be
/// assembled out of squared-off components — this is the test that would
/// catch that.
///
/// The two composites are deliberately given *different* components: sharing
/// one would put the control inside the exempt closure as well, which is the
/// separate consequence `reaching_down_also_exempts_the_component_itself`
/// pins.
#[test]
fn the_exemption_reaches_a_component_through_a_ref() {
    let input = format!(
        "{HEAD}\
glyph plain-part 2 2
b...
....
glyph flagged-part 2 2
b...
....
glyph plain-outer 2 2
ref plain-part 0 0
glyph outer 2 2 vectoronly
ref flagged-part 0 0
map A = plain-outer
map B = outer
"
    );

    assert_ne!(
        contours_of(&input, true, "plain-outer"),
        contours_of(&input, false, "plain-outer"),
        "the unflagged composite should still square its component off",
    );
    assert_eq!(
        contours_of(&input, true, "outer"),
        contours_of(&input, false, "outer"),
        "a vectoronly composite must not be assembled from squared-off parts",
    );
}

/// The cost of reaching down: the component is drawn as vector artwork for
/// *every* glyph in the bitmap build, not only for the flagged one. This
/// pins that consequence so it cannot change silently — `issues` is what
/// reports it to a source that did not want it.
#[test]
fn reaching_down_also_exempts_the_component_itself() {
    let input = format!(
        "{HEAD}\
glyph part 2 2
b...
....
glyph outer 2 2 vectoronly
ref part 0 0
map A = outer
map B = part
"
    );

    assert_eq!(
        contours_of(&input, true, "part"),
        contours_of(&input, false, "part"),
        "a component inside the exempt closure is exempt in its own right too",
    );
}

/// Nothing about the flag touches the vector build, which already draws
/// everything this way: an exempt closure is empty there, so no cache key
/// and no traced contour moves.
#[test]
fn the_flag_changes_nothing_in_the_vector_build() {
    let with = format!(
        "{HEAD}\
glyph a 2 2 vectoronly
b...
....
map A = a
"
    );
    let without = format!(
        "{HEAD}\
glyph a 2 2
b...
....
map A = a
"
    );
    assert_eq!(
        contours_of(&with, false, "a"),
        contours_of(&without, false, "a"),
    );
}
