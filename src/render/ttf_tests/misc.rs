//! Assorted builder tests: the build digest, `font-meta` handling, `map`
//! expansion helpers and the `maxp` limits.

use super::*;

#[test]
fn ttf_build_digest_is_deterministic() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 2 2
@@@@
..@@

glyph wide 3 2
@@..@@
..@@@@

glyph comp
ref base
ref wide

glyph alias = base

map A = base
map B = wide
map C = comp
map D = alias
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let doc_refs = vec![&doc];

    let (_, _, glyph_data_a, _, _) =
        collect_glyph_data(&doc_refs, false).expect("expected glyph data");
    let (_, _, glyph_data_b, _, _) =
        collect_glyph_data(&doc_refs, false).expect("expected glyph data");

    let mut canon_a: Vec<_> = glyph_data_a.iter().map(canonicalize_glyph).collect();
    let mut canon_b: Vec<_> = glyph_data_b.iter().map(canonicalize_glyph).collect();
    canon_a.sort();
    canon_b.sort();
    assert_eq!(canon_a, canon_b, "canonicalized glyph data should be deterministic");
    assert!(!canon_a.is_empty(), "should produce glyphs");
}

#[test]
fn unmapped_empty_sticky_glyph_is_retained() {
    let doc = document_io::parse_document_from_str(
        "glyph keep sticky advance 0\n",
        "test.unf".into(),
    )
    .unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let keep = glyphs.iter().find(|glyph| glyph.name == "keep").unwrap();
    assert_eq!(keep.codepoint, None);
    assert_eq!(keep.advance_width, 0);
    assert!(keep.contours.is_empty());
}

#[test]
fn font_meta_height_zero_returns_none() {
    let doc = document_io::parse_document_from_str(
        "font-meta height 0 ascent 0 descent 0\nglyph a 1 1\n@@\nmap A = a\n",
        "test.unf".into(),
    ).unwrap();
    let result = build_font_from_documents(&[&doc]);
    assert!(result.is_none(), "height 0 should reject build");
}

#[test]
fn parse_map_char_accepts_lowercase_u_plus() {
    assert_eq!(parse_map_char("u+0041"), Some(0x41));
    assert_eq!(parse_map_char("U+0041"), Some(0x41));
}

#[test]
fn expand_map_pairs_depth_aware_pipe_split() {
    // g(a|b) has a pipe inside parens — must not be split at top level
    let pairs = expand_map_pairs("A|B", "g(a|b)");
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0], (0x41, "ga".to_string()));
    assert_eq!(pairs[1], (0x42, "gb".to_string()));
}

#[test]
fn expand_map_pairs_cycles_glyph_names() {
    // 3 chars, 2 glyph names — should cycle
    let pairs = expand_map_pairs("A|B|C", "ga|gb");
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[2].1, "ga");
}

#[test]
fn expand_map_pairs_single_char_expands_pattern() {
    let pairs = expand_map_pairs("A", "g(a|b)");
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0], (0x41, "ga".to_string()));
}

#[test]
fn expand_map_pairs_lowercase_u_plus_list() {
    let pairs = expand_map_pairs("u+0041|u+0042", "ga|gb");
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, 0x41);
    assert_eq!(pairs[1].0, 0x42);
}

#[test]
fn expand_map_pairs_reverse_range_returns_empty() {
    let pairs = expand_map_pairs("U+0042..0041", "g(a|b)");
    assert!(pairs.is_empty());
}

#[test]
fn expand_map_pairs_bare_pipe_char() {
    // "map | = pipe" — the pipe character itself, not a separator
    let pairs = expand_map_pairs("|", "pipe");
    assert_eq!(pairs, vec![('|' as u32, "pipe".to_string())]);
}

#[test]
fn mark_flag_roundtrips() {
    let input = "\
glyph dia 3 2 mark
@@@@@@
@@@@@@
anchor -above 1 1
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert!(body.mark, "mark flag should be parsed");
    } else {
        panic!("expected glyph");
    }

    let mut output = Vec::new();
    document_io::serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("mark"), "mark flag should be serialized");

    let doc2 = document_io::parse_document_from_str(&output_str, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc2.items[0] {
        assert!(body.mark, "mark flag should survive roundtrip");
    }
}

#[test]
fn feature_anchor_roundtrips() {
    let input = "\
feature ccmp for DFLT latn : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::FeatureAnchor { name, scripts, anchor, .. } = &doc.items[0] {
        assert_eq!(name, "ccmp");
        assert_eq!(scripts, &["DFLT", "latn"]);
        assert_eq!(anchor, "above");
    } else {
        panic!("expected FeatureAnchor, got {:?}", doc.items[0]);
    }

    let mut output = Vec::new();
    document_io::serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("anchor above"));
}

/// Regression test: `glyph_bounds` on an empty contour list must return
/// a degenerate (0,0,0,0) box, not (MAX,MAX,MIN,MIN) — the latter is an
/// invalid bbox (x_min > x_max) that Chrome's OTS sanitizer rejects.
#[test]
fn glyph_bounds_empty_contours_is_degenerate_zero_box() {
    assert_eq!(glyph_bounds(&[]), (0, 0, 0, 0));
}

/// Firefox reports every `maxp` limit the outlines actually exceed
/// ("Component depth exceeds maxp maxComponentDepth", "Number of composite
/// points … exceeds maxp maxCompositePoints", "Number of contour points
/// exceeds maxp maxPoints").  Composite totals must be counted after full
/// decomposition, nesting depth must be measured rather than assumed to be
/// 1, and COLR layer glyphs must be counted at all.
#[test]
fn maxp_limits_cover_the_emitted_outlines() {
    // A merged foreground COLR layer made of on-demand pieces: they are
    // inlined, so the layer's points exist nowhere else in the font.
    let color_refs: String = (0..10)
        .flat_map(|i| [(i * 2, 0), (i * 2, 2)])
        .map(|(c, r)| format!("ref 1x1 {c} {r} coloronly\n"))
        .collect();
    let input = format!(
        "\
font-meta height 16 ascent 12 descent 4

// diagonal edges gain intermediate points when the glyph is emitted with
// grid-snap hints, so a component carries more points than the parent's own
// pre-hinting outline suggests
glyph tri 4 4
/1@@@@@@
../1@@@@
..../1@@
....../1

// three levels of nesting.  Each level is mapped on purpose: a glyph pulled
// into the font only as a component is collected without its own `ref`s and
// would flatten into a simple outline instead of nesting.
glyph nest1
ref tri
ref tri 4 0

glyph nest2
ref nest1
ref nest1 8 0

glyph nest3
ref nest2
ref nest2 16 0

color red = #ff0000

glyph colored 20 4
@@......................................
........................................
........................................
........................................
{color_refs}ref 1x1 0 3 coloronly fill red

map A = nest1
map B = nest2
map C = nest3
map D = colored
"
    );
    let doc = document_io::parse_document_from_str(&input, "t.unf".into()).unwrap();
    let bytes = build_font_from_documents(&[&doc]).expect("font should build");

    let want = recomputed_maxp(&bytes);
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    let maxp = font.maxp().unwrap();
    let got: HashMap<&'static str, u16> = HashMap::from([
        ("maxPoints", maxp.max_points().unwrap()),
        ("maxContours", maxp.max_contours().unwrap()),
        ("maxCompositePoints", maxp.max_composite_points().unwrap()),
        ("maxCompositeContours", maxp.max_composite_contours().unwrap()),
        ("maxComponentElements", maxp.max_component_elements().unwrap()),
        ("maxComponentDepth", maxp.max_component_depth().unwrap()),
    ]);

    // The fixture has to actually exercise each limit, or the comparison
    // below would pass on a font that never nests or never colors.
    assert_eq!(want["maxComponentDepth"], 3, "fixture should nest three deep");

    for key in got.keys() {
        assert_eq!(
            got[key],
            want[key],
            "maxp {key}: stored {} but the outlines need {}",
            got[key],
            want[key],
        );
    }
}
