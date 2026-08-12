//! `map BASE SELECTOR = GLYPH`: the cmap format 14 subtable and the GSUB
//! fallback lookup that stands behind it.
//!
//! Shaping is the assertion that matters. A table-level check cannot tell
//! "emitted" from "emitted and actually reachable", and the whole reason this
//! feature exists is that a GSUB-only variation sequence *was* emitted and was
//! not reachable — DirectWrite drops the selector before GSUB ever runs. So the
//! shaping tests below pin the cmap 14 path, and the table-level ones pin the
//! two things a shaper cannot show: that the fallback lookup was built at all,
//! and that the selector kept the plain cmap entry that lookup needs.

use super::*;

/// A base glyph, a distinct emoji-style glyph, and the pair that joins them.
const KEYCAP_SRC: &str = "\
meta height 16
meta ascent 12
meta descent 4

glyph zero 4 4
@@@@@@@@
@@......
@@......
@@......

glyph zero-emoji 4 4
@@@@@@@@
@@@@@@@@
@@......
@@......

map U+0030 = zero
map U+0030 U+FE0F = zero-emoji
";

/// The base and the selector go in, one glyph comes out. Two glyphs would mean
/// the selector survived as its own cluster, which is the failure this whole
/// change is about.
#[test]
fn a_variation_sequence_shapes_to_its_own_glyph() {
    assert_eq!(
        shape_glyph_names(KEYCAP_SRC, "0\u{FE0F}"),
        vec!["zero-emoji".to_string()],
    );
}

/// The base on its own is untouched: declaring a pair must not move the plain
/// mapping it is built on top of.
#[test]
fn the_base_alone_still_shapes_to_the_base_glyph() {
    assert_eq!(shape_glyph_names(KEYCAP_SRC, "0"), vec!["zero".to_string()],);
}

/// A selector the font states no pair for stays a default-ignorable: the base
/// keeps its own glyph and the selector contributes nothing visible.
#[test]
fn an_undeclared_selector_leaves_the_base_alone() {
    let names = shape_glyph_names(KEYCAP_SRC, "0\u{FE0E}");
    assert_eq!(names.first().map(String::as_str), Some("zero"));
}

/// Which of the two arrays a pair lands in, read back off the built font.
///
/// The shaping tests above cannot tell the arrays apart — a hidden
/// default-ignorable selector and a Default UVS entry produce the same glyph
/// run — so this is the only place the split is actually pinned.
#[test]
fn the_two_uvs_arrays_split_by_whether_the_target_is_the_base_glyph() {
    use read_fonts::TableProvider;
    use read_fonts::tables::cmap::{Cmap14, CmapSubtable, MapVariant};

    let src = "\
meta height 16
meta ascent 12
meta descent 4

glyph circle 4 4
@@@@@@@@
@@....@@
@@....@@
@@@@@@@@

glyph zero 4 4
@@@@@@@@
@@......
@@......
@@......

glyph zero-emoji 4 4
@@@@@@@@
@@@@@@@@
@@......
@@......

map U+26AA = circle
map U+26AA U+FE0E = circle
map U+0030 = zero
map U+0030 U+FE0F = zero-emoji
";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let built = build_font_with_gid_map(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&built.ttf).unwrap();
    let cmap = font.cmap().unwrap();

    let cmap14: Cmap14 = cmap
        .encoding_records()
        .iter()
        .find_map(|rec| match rec.subtable(cmap.offset_data()) {
            Ok(CmapSubtable::Format14(t)) => Some(t),
            _ => None,
        })
        .expect("a format 14 subtable should be emitted");

    // Target is the base's own glyph: valid sequence, no glyph id of its own.
    assert_eq!(
        cmap14.map_variant('\u{26AA}', '\u{FE0E}'),
        Some(MapVariant::UseDefault),
    );

    // Target is a glyph of its own: a Non-default entry naming it.
    let emoji_gid = built
        .gid_to_name
        .iter()
        .find(|(_, name)| name.as_str() == "zero-emoji")
        .map(|(gid, _)| *gid)
        .expect("zero-emoji should be in the font");
    assert_eq!(
        cmap14.map_variant('0', '\u{FE0F}'),
        Some(MapVariant::Variant(read_fonts::types::GlyphId::new(
            emoji_gid as u32
        ))),
    );

    // A sequence the source never stated is not claimed either way.
    assert_eq!(cmap14.map_variant('0', '\u{FE0E}'), None);
}

/// The Default UVS case end to end: the selector still has to be swallowed —
/// one glyph out, not two.
#[test]
fn a_pair_targeting_the_base_glyph_still_swallows_the_selector() {
    let src = "\
meta height 16
meta ascent 12
meta descent 4

glyph circle 4 4
@@@@@@@@
@@....@@
@@....@@
@@@@@@@@

map U+26AA = circle
map U+26AA U+FE0E = circle
";
    assert_eq!(
        shape_glyph_names(src, "\u{26AA}\u{FE0E}"),
        vec!["circle".to_string()],
    );
}

/// A range on the base half, one selector held fixed — the keycap shape.
#[test]
fn a_range_of_bases_against_one_selector_shapes_each_pair() {
    let src = "\
meta height 16
meta ascent 12
meta descent 4

glyph num-0 4 4
@@@@@@@@
@@......
@@......
@@......

glyph num-1 4 4
@@@@@@@@
..@@....
..@@....
..@@....

glyph num-0-emoji 4 4
@@@@@@@@
@@@@@@@@
@@......
@@......

glyph num-1-emoji 4 4
@@@@@@@@
@@@@@@@@
..@@....
..@@....

map U+0030..0031 = num-(0|1)
map U+0030..0031 U+FE0F = num-(0|1)-emoji
";
    assert_eq!(
        shape_glyph_names(src, "0\u{FE0F}1\u{FE0F}"),
        vec!["num-0-emoji".to_string(), "num-1-emoji".to_string()],
    );
}

/// The fallback lookup's first element is the *selector's* glyph, reached
/// through a plain cmap lookup — `hb_font_get_nominal_glyph`, not the format 14
/// subtable. Without this entry the selector becomes `.notdef` on any shaper
/// that skips cmap 14 and the whole fallback path is dead code.
#[test]
fn the_selector_keeps_a_plain_cmap_entry() {
    use read_fonts::TableProvider;

    let doc = document_io::parse_document_from_str(KEYCAP_SRC, "test.unf".into()).unwrap();
    let built = build_font_with_gid_map(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&built.ttf).unwrap();
    let cmap = font.cmap().unwrap();

    let gid = cmap.map_codepoint(0xFE0Fu32);
    assert!(
        gid.is_some_and(|g| g.to_u32() != 0),
        "U+FE0F should have a plain cmap entry, got {gid:?}",
    );
}

/// Declaring no pair declares no selector: a font that never states a variation
/// sequence must not start claiming coverage of `U+FE00..FE0F`.
#[test]
fn a_font_with_no_pairs_claims_no_selector() {
    use read_fonts::TableProvider;

    let src = "\
meta height 16
meta ascent 12
meta descent 4

glyph zero 4 4
@@@@@@@@
@@......
@@......
@@......

map U+0030 = zero
";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let built = build_font_with_gid_map(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&built.ttf).unwrap();
    let cmap = font.cmap().unwrap();

    assert!(
        cmap.map_codepoint(0xFE0Fu32)
            .is_none_or(|g| g.to_u32() == 0),
        "a font with no variation sequences should not map U+FE0F",
    );
}

/// The fallback lookup has to run before anything the source wrote, or a rule
/// written against the pair's *target* would still be looking at the base and
/// the selector as two separate glyphs when it ran.
#[test]
fn the_fallback_lookup_precedes_every_source_lookup() {
    use read_fonts::TableProvider;

    let src = "\
meta height 16
meta ascent 12
meta descent 4

glyph zero 4 4
@@@@@@@@
@@......
@@......
@@......

glyph zero-emoji 4 4
@@@@@@@@
@@@@@@@@
@@......
@@......

glyph keycap 4 4
@@@@@@@@
@@....@@
@@....@@
@@......

glyph keycap-zero 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

map U+0030 = zero
map U+20E3 = keycap
map U+0030 U+FE0F = zero-emoji

remap group keycaps
feature ccmp for DFLT : keycaps
remap keycaps : zero-emoji keycap -> keycap-zero
";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let built = build_font_with_gid_map(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&built.ttf).unwrap();
    let gsub = font.gsub().expect("font should have GSUB");
    assert!(
        gsub.lookup_list().unwrap().lookups().len() >= 2,
        "expected the fallback lookup plus the source's own",
    );

    // The end-to-end statement of the same thing: the whole keycap sequence
    // collapses to one glyph, which only happens if the pair was folded first.
    assert_eq!(
        shape_glyph_names(src, "0\u{FE0F}\u{20E3}"),
        vec!["keycap-zero".to_string()],
    );
}
