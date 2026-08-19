//! Tests for implicit merging at the font level: several names one `glyph`
//! pattern block declares, proved to describe the same glyph, cost one glyph
//! id between them. See [`crate::merge`].
//!
//! The thing to check is always the same — how many glyphs the font ends up
//! with, and which name the survivor carries — so every test here counts
//! `gid_to_name` and compares the ids a character reaches.

use super::*;

fn build(input: &str) -> crate::render::FontWithGidMap {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    build_font_with_gid_map(&[&doc]).expect("font should build")
}

/// `cmap` lookups by codepoint, through the built font.
fn gids_for(built: &crate::render::FontWithGidMap, chars: &str) -> Vec<u16> {
    let face = rustybuzz::Face::from_slice(&built.ttf, 0).expect("font should parse");
    chars
        .chars()
        .map(|c| face.glyph_index(c).expect("mapped character").0)
        .collect()
}

fn names(built: &crate::render::FontWithGidMap) -> Vec<String> {
    let mut names: Vec<String> = built.gid_to_name.values().cloned().collect();
    names.sort();
    names
}

/// The motivating case: a pattern block whose body holds nothing that varies
/// declares one glyph under three names, not three glyphs.
#[test]
fn identical_expansions_of_one_pattern_block_share_one_glyph_id() {
    let built = build(
        "\
glyph a-(g|j|k) 1 1
@@
map A = a-g
map B = a-j
map C = a-k
",
    );
    let gids = gids_for(&built, "ABC");
    assert_eq!(gids[0], gids[1]);
    assert_eq!(gids[1], gids[2]);
    assert_eq!(names(&built), vec![".notdef", "a-g"], "{:?}", names(&built));
}

/// Two blocks are never merged with each other, however alike they are: the
/// candidates are the expansions of one block and nothing else.
#[test]
fn separate_blocks_are_never_merged() {
    let built = build(
        "\
glyph a 1 1
@@
glyph b 1 1
@@
map A = a
map B = b
",
    );
    let gids = gids_for(&built, "AB");
    assert_ne!(gids[0], gids[1]);
    assert_eq!(names(&built), vec![".notdef", "a", "b"]);
}

/// A `ref` written as a pattern names a different glyph per expansion, so
/// whether the expansions are one glyph is whatever their targets are: here
/// `a-g`/`a-j` are one glyph and `a-k` is not, and `b` follows exactly.
#[test]
fn a_ref_pattern_merges_with_the_glyphs_it_names() {
    let built = build(
        "\
glyph a-(g|j) 1 1
@@
glyph a-k 1 1
..
glyph b-(g|j|k) 1 1
ref a-(g|j|k) 0 0
map A = b-g
map B = b-j
map C = b-k
",
    );
    let gids = gids_for(&built, "ABC");
    assert_eq!(gids[0], gids[1], "b-g and b-j reference one glyph");
    assert_ne!(gids[1], gids[2], "b-k references a glyph of its own");
    assert_eq!(
        names(&built),
        vec![".notdef", "a-g", "a-k", "b-g", "b-k"],
        "{:?}",
        names(&built)
    );
}

/// The merge is transitive: `c` refers to `b`, which refers to `a`. One
/// fixpoint settles the whole chain.
#[test]
fn a_chain_of_ref_patterns_merges_all_the_way_up() {
    let built = build(
        "\
glyph a-(g|j) 1 1
@@
glyph a-k 1 1
..
glyph b-(g|j|k) 1 1
ref a-(g|j|k) 0 0
glyph c-(g|j|k) 1 1
ref b-(g|j|k) 0 0
map A = c-g
map B = c-j
map C = c-k
",
    );
    let gids = gids_for(&built, "ABC");
    assert_eq!(gids[0], gids[1]);
    assert_ne!(gids[1], gids[2]);
    // Only the middle of the chain is asserted by name: which of the deeper
    // components the builder keeps a glyph id for is its own business, and the
    // partition is the point here.
    let names = names(&built);
    assert!(
        !names.iter().any(|n| n.ends_with("-j")),
        "no `-j` glyph survives: {names:?}"
    );
    assert!(names.contains(&"c-g".to_string()) && names.contains(&"c-k".to_string()));
}

/// An IDC line is expanded exactly as a `ref` is, so a split whose parts merge
/// merges too.
#[test]
fn an_idc_line_merges_with_its_parts() {
    let built = build(
        "\
glyph part-(g|j):2x4-l 2 4
@@..
@@..
@@..
@@..

glyph right:2x4-r 2 4
..@@
..@@
..@@
..@@

glyph whole-(g|j) 4 4
\u{2FF0} part-(g|j):2x4-l right:2x4-r
map U+4E00 = whole-g
map U+4E01 = whole-j
",
    );
    let gids = gids_for(&built, "\u{4E00}\u{4E01}");
    assert_eq!(gids[0], gids[1]);
    assert_eq!(
        names(&built),
        vec![".notdef", "part-g:2x4-l", "right:2x4-r", "whole-g"],
        "{:?}",
        names(&built)
    );
}

/// `ifexists` needs no rule of its own: a name nothing defines is merged with
/// nothing, so the expansion that names it keeps to itself — and, having no
/// target, is not built at all.
#[test]
fn an_ifexists_ref_to_an_undefined_name_is_not_merged() {
    let built = build(
        "\
glyph a-(g|j) 1 1
@@
glyph b-(g|j|k) 1 1
ref a-(g|j|k) 0 0 ifexists
map A = b-g
map B = b-j
",
    );
    let gids = gids_for(&built, "AB");
    assert_eq!(gids[0], gids[1]);
    assert_eq!(
        names(&built),
        vec![".notdef", "a-g", "b-g"],
        "{:?}",
        names(&built)
    );
}

/// `keep` says the glyph is wanted in its own right, which is also what makes
/// it a glyph of its own: it is the opt-out.
#[test]
fn keep_gives_every_expansion_a_glyph_id_of_its_own() {
    let built = build(
        "\
glyph a-(g|j|k) 1 1 keep
@@
",
    );
    assert_eq!(
        names(&built),
        vec![".notdef", "a-g", "a-j", "a-k"],
        "{:?}",
        names(&built)
    );
}

/// `@` is expanded before the pattern is, so `@-part` under `glyph a-(g|j)` is
/// the pattern `a-(g|j)-part` and merges with whatever that names.
#[test]
fn an_at_ref_follows_the_names_it_expands_to() {
    let built = build(
        "\
glyph a-(g|j)-part 1 1
@@
glyph a-(g|j) 1 1
ref @-part 0 0
map A = a-g
map B = a-j
",
    );
    let gids = gids_for(&built, "AB");
    assert_eq!(gids[0], gids[1]);
    assert_eq!(
        names(&built),
        vec![".notdef", "a-g", "a-g-part"],
        "{:?}",
        names(&built)
    );
}

/// A merged-away name is a name for the surviving glyph, like an alias — so
/// everything that names a glyph accepts it. `assert shape` names glyphs as
/// the built font names them, where only the survivor has a name at all.
#[test]
fn an_assert_shape_may_name_a_merged_away_name() {
    let input = "\
glyph a-(g|j) 1 1
@@
map A = a-g
assert shape `A` : a-j
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let result = crate::render::assert::run_assertions(&docs, &mut |face| {
        crate::render::build_font_with_gid_map_for(&docs, face)
    });
    assert_eq!(result.total, 1);
    assert_eq!(result.passed, 1, "{:?}", result.issues);
}
