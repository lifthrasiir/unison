//! Tests for folding a secondary face into the demo page's font.
//!
//! The assertions are about the *built font*: what a shaper produces with the
//! switch off and with it on, since that is the whole of what the page relies
//! on. See [`super::super::ttf_builder::fold`].

use super::*;
use skrifa::MetadataProvider;

/// A source with two faces that disagree about `U+0041`, plus two probe
/// mappings so a test can name a glyph by a code point of its own.
const TWO_FACES: &str = "\
face regular : wide
face term : narrow
slice wide
slice narrow
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph pix 1 1
@@
glyph a
ref pix
glyph a-half
ref pix
map wide|narrow : U+0041 = a($-half)
map U+E000 = a
map U+E001 = a-half
";

fn demo_font(input: &str) -> (crate::render::ttf_builder::DemoFont, Document) {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let faces = crate::faces::FaceSet::collect(&[&doc]);
    let built = build_face_variable(&[&doc], faces.primary()).expect("font should build");
    (built, doc)
}

/// The gids a shaper produces for `text`, with `features` turned on.
fn shaped_gids(ttf: &[u8], text: &str, features: &[&[u8; 4]]) -> Vec<u16> {
    let face = rustybuzz::Face::from_slice(ttf, 0).expect("font should parse");
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    let features: Vec<rustybuzz::Feature> = features
        .iter()
        .map(|tag| rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(tag), 1, ..))
        .collect();
    rustybuzz::shape(&face, &features, buffer)
        .glyph_infos()
        .iter()
        .map(|i| i.glyph_id as u16)
        .collect()
}

/// The advance `gid` carries, in font units.
fn advance_of(ttf: &[u8], gid: u16) -> u32 {
    let font = read_fonts::FontRef::new(ttf).unwrap();
    font.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default())
        .advance_width(skrifa::GlyphId::new(gid as u32))
        .expect("glyph should have an advance") as u32
}

/// The gid the font's cmap gives `cp`.
fn gid_of(ttf: &[u8], cp: char) -> u16 {
    let font = read_fonts::FontRef::new(ttf).unwrap();
    font.charmap()
        .map(cp)
        .unwrap_or_else(|| panic!("{cp:?} should be mapped"))
        .to_u32() as u16
}

#[test]
fn the_switch_substitutes_the_secondary_faces_glyph() {
    let (built, _) = demo_font(TWO_FACES);
    assert_eq!(built.warnings, Vec::<String>::new());
    assert_eq!(built.folded.len(), 1);
    let fold = &built.folded[0];
    assert_eq!(fold.id, "term");
    // Allocated from the top of the range: the source uses no stylistic set.
    assert_eq!(fold.feature, "ss20");

    let wide = gid_of(&built.ttf, '\u{E000}');
    let narrow = gid_of(&built.ttf, '\u{E001}');
    assert_ne!(wide, narrow);
    assert_eq!(
        shaped_gids(&built.ttf, "A", &[]),
        vec![wide],
        "with the switch off the primary face's glyph is what the text draws"
    );
    assert_eq!(
        shaped_gids(&built.ttf, "A", &[b"ss20"]),
        vec![narrow],
        "with it on, the secondary face's"
    );
}

/// A source with one face has nothing to fold, and must not grow a feature for
/// it — an empty lookup under a stylistic set is a table a shaper still reads.
#[test]
fn a_single_face_source_gets_no_switch() {
    let (built, _) = demo_font("glyph pix 1 1\n@@\nglyph a\nref pix\nmap U+0041 = a\n");
    assert!(built.folded.is_empty());
    assert!(built.warnings.is_empty());
    assert_eq!(
        shaped_gids(&built.ttf, "A", &[b"ss20"]),
        shaped_gids(&built.ttf, "A", &[]),
    );
}

/// The switch is appended after every group the source declared, so it applies
/// to what shaping produced rather than to what the text held. A `remap` is
/// written against the primary face's names and has to run while those are
/// still the names in the buffer.
#[test]
fn the_switch_runs_after_the_sources_own_lookups() {
    let input = "\
face regular : wide
face term : narrow
slice wide
slice narrow
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph pix 1 1
@@
glyph a
ref pix
glyph b
ref pix
glyph c
ref pix
glyph c-half
ref pix
map U+0061 = a
map U+0062 = b
map wide|narrow : U+0043 = c($-half)
map U+E001 = c-half
remap lig : a b -> c
feature ccmp for DFLT : lig
";
    let (built, _) = demo_font(input);
    let narrow_c = gid_of(&built.ttf, '\u{E001}');
    assert_eq!(
        shaped_gids(&built.ttf, "ab", &[b"ss20"]),
        vec![narrow_c],
        "the ligature has to fire first; narrowing its output is the switch's job",
    );
}

/// A primary glyph the secondary face replaces two ways cannot be a single
/// substitution. The pair is dropped and said out loud, rather than resolved
/// one way and rendered wrong in the other.
#[test]
fn a_glyph_the_secondary_face_replaces_two_ways_warns() {
    let input = "\
face regular : wide
face term : narrow
slice wide
slice narrow
glyph pix 1 1
@@
glyph sp
ref pix
glyph nbsp
ref pix
glyph wsp
ref pix
map wide : U+2001|U+2003 = wsp
map narrow : U+2001 = sp
map narrow : U+2003 = nbsp
map U+E000 = wsp
";
    let (built, _) = demo_font(input);
    assert_eq!(built.folded.len(), 1);
    assert!(
        built.warnings.iter().any(|w| w.contains("'wsp'")),
        "the conflict has to be reported: {:?}",
        built.warnings
    );
    let wsp = gid_of(&built.ttf, '\u{E000}');
    assert_eq!(
        shaped_gids(&built.ttf, "\u{2001}", &[b"ss20"]),
        vec![wsp],
        "an unresolvable glyph is left alone rather than substituted at random"
    );
}

/// A character the secondary face does not map is not a substitution but the
/// absence of one, and a feature cannot take a cmap entry away. The page is
/// told instead.
#[test]
fn a_character_the_secondary_face_drops_is_reported_to_the_page() {
    let input = "\
face regular : wide
face term : narrow
slice wide
slice narrow
glyph pix 1 1
@@
glyph a
ref pix
glyph lig
ref pix
map U+0041 = a
map wide : U+FB13 = lig
";
    let (built, _) = demo_font(input);
    assert_eq!(built.folded[0].unmapped, vec![0xFB13]);
}

/// A character that also carries a variation sequence still switches.
///
/// `map BASE SELECTOR` puts the base's glyph into cmap 14 and into the
/// fallback lookup beside its plain cmap entry, and none of that is supposed to
/// change what the bare character draws.
#[test]
fn a_character_with_a_variation_sequence_still_switches() {
    let input = format!("{TWO_FACES}map wide|narrow : U+0041 U+FE0E = a($-half)\n");
    let (built, _) = demo_font(&input);
    let wide = gid_of(&built.ttf, '\u{E000}');
    let narrow = gid_of(&built.ttf, '\u{E001}');
    assert_eq!(shaped_gids(&built.ttf, "A", &[]), vec![wide]);
    assert_eq!(shaped_gids(&built.ttf, "A", &[b"ss20"]), vec![narrow]);
}

/// A glyph only the secondary face's `map` reaches is still in the font.
///
/// The demo font traces the *union* face for exactly this: the fold
/// substitutes to glyphs the primary face never mentions, and a rule whose
/// target has no glyph id is dropped where the lookup is built — silently,
/// since a `remap` naming a glyph the font does not have is an ordinary thing
/// for a source to do. Tracing the primary face left Unison's demo font with
/// 13 of the 724 substitutions it should carry: the ones whose narrow glyph
/// the primary face happens to map too, at a halfwidth code point of its own.
#[test]
fn a_glyph_only_the_secondary_face_maps_is_in_the_font() {
    let input = "\
face regular : wide
face term : narrow
slice wide
slice narrow
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph pix 1 1
@@
glyph b advance 16
ref pix
glyph b-half advance 8
ref pix
map wide|narrow : U+0042 = b($-half)
";
    let (built, _) = demo_font(input);
    let wide = shaped_gids(&built.ttf, "B", &[]);
    let narrow = shaped_gids(&built.ttf, "B", &[b"ss20"]);
    assert_eq!(wide.len(), 1);
    assert_ne!(narrow, wide, "the switch has to reach a glyph nothing else maps");
    assert_ne!(narrow[0], 0, "and not `.notdef`");
    assert_eq!(
        advance_of(&built.ttf, narrow[0]) * 2,
        advance_of(&built.ttf, wide[0]),
        "the narrow glyph's own advance is what makes the switch worth anything"
    );
}
