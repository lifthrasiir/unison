//! Tests for `glyph NAME = TARGET` at the font level: an alias is a second
//! *name* for one glyph, so the thing to check is that it costs no glyph id
//! and that every way of naming a glyph accepts it. See [`crate::alias`].

use super::*;

/// `cmap` lookups by codepoint, through the built font.
fn gids_for(built: &crate::render::FontWithGidMap, chars: &str) -> Vec<u16> {
    let face = rustybuzz::Face::from_slice(&built.ttf, 0).expect("font should parse");
    chars
        .chars()
        .map(|c| face.glyph_index(c).expect("mapped character").0)
        .collect()
}

fn build(input: &str) -> crate::render::FontWithGidMap {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    build_font_with_gid_map(&[&doc]).expect("font should build")
}

/// The whole point of the form: two names, one glyph id. Under the old
/// `glyph A; ref B` reading these were two glyphs with identical outlines.
#[test]
fn an_alias_and_its_target_share_one_glyph_id() {
    let built = build(
        "\
glyph a 1 1
@@
glyph b = a
map A = a
map B = b
",
    );
    let gids = gids_for(&built, "AB");
    assert_eq!(gids[0], gids[1], "the alias must not get an id of its own");
    assert_eq!(
        built.gid_to_name.get(&gids[0]).map(String::as_str),
        Some("a"),
        "the glyph keeps the target's name",
    );
    // .notdef plus exactly one real glyph.
    assert_eq!(built.gid_to_name.len(), 2, "{:?}", built.gid_to_name);
}

#[test]
fn an_alias_chain_collapses_to_the_end_of_the_chain() {
    let built = build(
        "\
glyph a 1 1
@@
glyph b = a
glyph c = b
map A = a
map C = c
",
    );
    let gids = gids_for(&built, "AC");
    assert_eq!(gids[0], gids[1]);
    // .notdef plus exactly one real glyph.
    assert_eq!(built.gid_to_name.len(), 2, "{:?}", built.gid_to_name);
}

#[test]
fn a_ref_to_an_alias_composes_the_target() {
    let built = build(
        "\
glyph a 1 1
@@
glyph b = a
glyph pair
ref b 0 0
ref b 1 0
map P = pair
",
    );
    let gids = gids_for(&built, "P");
    assert_eq!(
        built.gid_to_name.get(&gids[0]).map(String::as_str),
        Some("pair"),
    );
    // .notdef, `a` (referenced) and `pair` (mapped) — no glyph of its own for `b`.
    assert_eq!(built.gid_to_name.len(), 3, "{:?}", built.gid_to_name);
}

/// GSUB expands `remap` patterns straight from the documents rather than from
/// the expanded item list, so it canonicalizes for itself; a rule naming an
/// alias has to substitute the target's id like any other rule.
#[test]
fn a_remap_may_name_an_alias_on_either_side() {
    let names = shape_glyph_names(
        "\
glyph a 1 1
@@
glyph b 1 1
..
glyph a-alias = a
glyph b-alias = b
map A = a
map B = b
remap ccmp : a-alias -> b-alias
feature ccmp for DFLT : ccmp
",
        "A",
    );
    assert_eq!(names, vec!["b"]);
}

/// An assertion names glyphs as the built font names them, where an alias has
/// no name at all — so writing one must mean its target.
#[test]
fn an_assert_shape_may_name_an_alias() {
    let input = "\
glyph a 1 1
@@
glyph b = a
map A = a
assert shape `A` : b
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let result = crate::render::assert::run_assertions(&docs, &mut |face| {
        crate::render::build_font_with_gid_map_for(&docs, face)
    });
    assert_eq!(result.total, 1);
    assert_eq!(result.passed, 1, "{:?}", result.issues);
}
