//! Round-trips through the parser and serializer: glyph blocks and their
//! flags, `meta`, `audit`, faces and slices, anchors, aliases and `ref`s.

use super::*;

/// `glyph_header_dims` and `derive_document` must agree on which headers
/// own a pixel grid — a disagreement leaves reconciliation and the
/// document model permanently fighting over the grid DocLine.
#[test]
fn header_dims_match_derive_for_valued_flags() {
    // Valued flags may precede W H; their argument is not a dimension.
    let dims = glyph_header_dims(&["foo", "advance", "0", "4", "3"]);
    assert_eq!(
        dims,
        Some(GlyphHeaderDims {
            width: 4,
            height: 3,
            scale: 1
        })
    );

    let dims = glyph_header_dims(&["foo", "advance", "2", "3"]);
    assert_eq!(dims, None, "width 3 without height is not a grid header");

    let dims = glyph_header_dims(&["foo", "4", "3", "advance", "0"]);
    assert_eq!(
        dims,
        Some(GlyphHeaderDims {
            width: 4,
            height: 3,
            scale: 1
        })
    );

    let dims = glyph_header_dims(&["foo", "keep", "4", "3"]);
    assert_eq!(
        dims,
        Some(GlyphHeaderDims {
            width: 4,
            height: 3,
            scale: 1
        })
    );

    // Cross-check against derive_document on the same headers.
    for (header, expected) in [
        ("glyph foo advance 0 4 3", Some((4u16, 3u16))),
        ("glyph foo advance 2 3", None),
        ("glyph foo 4 3 advance 0", Some((4, 3))),
    ] {
        let lines = vec![DocLine::Text(header.to_string())];
        let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        let Some(DocumentItem::Glyph { body, .. }) = doc.items.first() else {
            panic!("expected glyph item for {header:?}");
        };
        let derived = body.pixels.as_ref().map(|g| (g.width, g.height));
        assert_eq!(derived, expected, "derive mismatch for {header:?}");
        let tokens = tokenize_tokens(header).unwrap();
        let dims = glyph_header_dims(&tokens[1..]).map(|d| (d.width, d.height));
        assert_eq!(dims, expected, "glyph_header_dims mismatch for {header:?}");
    }
}

/// `meta` carries one key per line, so the round-trip has to keep each line
/// separate — and keep its comment, like every other item.
#[test]
fn meta_roundtrip_one_key_per_line() {
    let input = "\
meta height 16
meta ascent 14 // the interesting one
meta descent 2
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 3);
    assert!(
        matches!(&doc.items[0], DocumentItem::Meta(t) if t == "height 16"),
        "expected a Meta item, got {:?}",
        doc.items[0],
    );
    assert!(
        matches!(&doc.items[1], DocumentItem::Meta(t) if t == "ascent 14 // the interesting one"),
        "the trailing comment must survive parsing, got {:?}",
        doc.items[1],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `audit` is `meta`'s shape — one key per line, the text kept verbatim — but
/// its own item, because what it states is a rule about the source rather than
/// a value the font carries. See [`crate::audit`].
#[test]
fn audit_lines_round_trip() {
    let input = "\
audit ideal-clearance han-* 0 1 // the band every han glyph is held to
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Audit(t)
            if t == "ideal-clearance han-* 0 1 // the band every han glyph is held to"),
        "expected an Audit item, got {:?}",
        doc.items[0],
    );
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `exists` is one token — a regex — and it binds the item on the next line by
/// adjacency, so the scoped `glyph` block round-trips as an ordinary one.
#[test]
fn exists_lines_round_trip() {
    let input = "\
exists han-([0-9a-f]{4,5}):15x16 // wherever a 15x16 han was drawn
glyph han-($1) 16 16 advance 16
ref ($0)
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Exists { pattern, comment }
            if pattern == "han-([0-9a-f]{4,5}):15x16"
                && comment.as_deref() == Some("wherever a 15x16 han was drawn")),
        "expected an Exists item, got {:?}",
        doc.items[0],
    );
    assert!(matches!(&doc.items[1], DocumentItem::Glyph { .. }));
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A second token is neither a flag nor a second pattern, so the line stays an
/// unrecognized directive for `issues` to report — and round-trips verbatim.
#[test]
fn an_exists_with_extra_tokens_is_not_an_exists_item() {
    let input = "exists han-(x) han-(y)\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(matches!(&doc.items[0], DocumentItem::Directive(_)));
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// The face/slice grammar, through a parse/serialize round trip. A qualifier is
/// `SLICE :` in front of what the line already said, so the unqualified form
/// keeps parsing exactly as before.
#[test]
fn face_and_slice_grammar_roundtrips() {
    let input = "\
slice wide
slice both = wide narrow
face narrow
face wide : wide // the ambiguous-wide one
meta wide : family `Unison Wide`
map wide : ° = degree-wide
map wide : generate U+00C5 = aring-wide
feature wide : liga for latn : ligatures
assert shape AB for wide both : a : b
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[1], DocumentItem::Slice { id, inherits, .. }
            if id == "both" && inherits == &["wide", "narrow"]),
        "got {:?}",
        doc.items[1],
    );
    assert!(
        matches!(&doc.items[3], DocumentItem::Face { id, slices, comment }
            if id == "wide" && slices == &["wide"]
                && comment.as_deref() == Some("the ambiguous-wide one")),
        "got {:?}",
        doc.items[3],
    );
    assert!(
        matches!(&doc.items[5], DocumentItem::Map { slices, char_repr, .. }
            if slices == &["wide"] && char_repr == "°"),
        "got {:?}",
        doc.items[5],
    );
    assert!(
        matches!(&doc.items[8], DocumentItem::AssertShape { slices, .. }
            if slices == &["wide", "both"]),
        "got {:?}",
        doc.items[8],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `map : = colon` maps U+003A and must not be read as a slice qualifier. The
/// two are told apart by which token is the bare `:`.
#[test]
fn a_colon_being_mapped_is_not_a_slice_qualifier() {
    let doc = parse_document_from_str("map : = colon\n", "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { slices, char_repr, glyphs, .. }
            if slices.is_empty() && char_repr == ":" && glyphs == &["colon"]),
        "got {:?}",
        doc.items[0],
    );

    // ...and a slice-qualified mapping *of* a colon still works.
    let doc = parse_document_from_str("map wide : : = colon\n", "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { slices, char_repr, .. }
            if slices == &["wide"] && char_repr == ":"),
        "got {:?}",
        doc.items[0],
    );
}

/// A qualifier may list slices, and `name-parts` takes one too. Both are the
/// same single token the parser already looked for, so `wide|narrow` round-trips
/// as written rather than being re-spelled on the way out.
#[test]
fn a_slice_list_qualifier_roundtrips() {
    let input = "\
name-parts wide : $half = ``
name-parts narrow : $half = -half
map wide|narrow : ⁂ = triple-star($half)
feature wide|narrow : ccmp for DFLT : deemojify
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[1], DocumentItem::NameParts { slices, name, values, .. }
            if slices == &["narrow"] && name == "$half" && values == &["-half"]),
        "got {:?}",
        doc.items[1],
    );
    assert!(
        matches!(&doc.items[2], DocumentItem::Map { slices, glyphs, .. }
            if slices == &["wide", "narrow"] && glyphs == &["triple-star($half)"]),
        "got {:?}",
        doc.items[2],
    );
    assert!(
        matches!(&doc.items[3], DocumentItem::Feature { slices, .. }
            if slices == &["wide", "narrow"]),
        "got {:?}",
        doc.items[3],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `$$` is a pixel pair like any other: it parses into a hardblank and is
/// written back verbatim, which is the whole point of distinguishing it from
/// the `..` it draws the same as.
#[test]
fn roundtrip_hardblank() {
    let input = "\
glyph test 3 1
$$@@..
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected a glyph, got {:?}", doc.items[0]);
    };
    let grid = body.pixels.as_ref().expect("the glyph has a grid");
    assert!(grid.get(0, 0).is_hardblank());
    assert!(grid.get(0, 1).is_bitmap_filled());
    assert!(grid.get(0, 2).is_clear());

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn roundtrip_simple() {
    let input = "\
// test comment
meta height 16
meta ascent 14
meta descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph uni0041
ref test-glyph 2 0

exclude-from-sample U+AD00
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    // comment, 3x meta, blank, glyph, blank, glyph, blank, directive = 10
    assert_eq!(doc.items.len(), 10);

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();

    let doc2 = parse_document_from_str(&output_str, "test2.unf".into()).unwrap();
    assert_eq!(doc2.items.len(), doc.items.len());
}

#[test]
fn anchor_range_roundtrip() {
    let input = "\
glyph foo 2 2
@@@@
@@@@
anchor +join 1..3 0..2
anchor -bar 5 7
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert_eq!(body.points.len(), 2);
        let p0 = &body.points[0];
        assert_eq!(p0.position, "+join");
        assert_eq!((p0.col, p0.col_end), (1, 3));
        assert_eq!((p0.row, p0.row_end), (0, 2));
        assert_eq!(p0.width(), 3);
        assert_eq!(p0.height(), 3);
        let p1 = &body.points[1];
        assert_eq!(p1.position, "-bar");
        assert_eq!((p1.col, p1.col_end), (5, 5));
        assert_eq!((p1.row, p1.row_end), (7, 7));
        assert!(p1.is_single_cell());
    } else {
        panic!("expected glyph");
    }

    // Roundtrip through serialize_document
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("anchor +join 1..3 0..2"));
    assert!(output_str.contains("anchor -bar 5 7"));

    // Re-parse the serialized output
    let doc2 = parse_document_from_str(&output_str, "test2.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc2.items[0] {
        assert_eq!(body.points.len(), 2);
        assert_eq!(body.points[0].width(), 3);
        assert!(body.points[1].is_single_cell());
    } else {
        panic!("expected glyph on re-parse");
    }
}

#[test]
fn parse_glyph_with_all_shapes() {
    let input = "glyph shapes 4 1\n..@@1\\1>\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        let grid = body.pixels.as_ref().expect("expected glyph with pixels");
        assert!(grid.get(0, 0).is_clear());
        assert_eq!(grid.get(0, 1).shape_id(), crate::pixel::PX_ALMOSTFULL);
        assert!(grid.get(0, 1).is_bitmap_filled());
        assert_eq!(grid.get(0, 2).shape_id(), crate::pixel::PX_HALF1);
        assert!(grid.get(0, 2).is_bitmap_filled());
        assert_eq!(grid.get(0, 3).shape_id(), crate::pixel::PX_QUAD1);
        assert!(grid.get(0, 3).is_bitmap_filled());
    } else {
        panic!("expected glyph");
    }
}

#[test]
fn parse_glyph_without_pixel_rows() {
    let input = "glyph empty 4 3\nref other 0 0\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        let grid = body.pixels.as_ref().expect("should produce empty grid");
        assert_eq!(grid.width, 4);
        assert_eq!(grid.height, 3);
        assert!(grid.is_all_empty());
        assert_eq!(body.refs.len(), 1);
        assert_eq!(body.refs[0].name, "other");
    } else {
        panic!("expected glyph");
    }

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, "glyph empty 4 3\nref other 0 0\n");
}

#[test]
fn roundtrip_alias() {
    let input = "glyph uni0041 = test-glyph\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::GlyphAlias { name, target, .. } = &doc.items[0] {
        assert_eq!(name.display(), "uni0041");
        assert_eq!(target, "test-glyph");
    } else {
        panic!("expected glyph alias");
    }

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, "glyph uni0041 = test-glyph\n");
}

#[test]
fn explicit_zero_ref_roundtrips_as_explicit() {
    let input = "glyph composite\nref target 0 0\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected glyph");
    };
    assert_eq!(body.refs[0].offset, Some((0, 0)));

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output, input);

    let reparsed = parse_document_from_str(&output, "test2.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &reparsed.items[0] else {
        panic!("expected glyph");
    };
    assert_eq!(body.refs[0].offset, Some((0, 0)));
}

#[test]
fn derive_accepts_only_complete_ref_forms() {
    let input = "\
glyph composite
ref auto
ref auto-negated negated
ref explicit 0 0
ref explicit-negated 1 -2 negated
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected glyph");
    };
    assert_eq!(body.refs.len(), 4);
    assert_eq!(body.refs[0].offset, None);
    assert!(!body.refs[0].negated);
    assert_eq!(body.refs[1].offset, None);
    assert!(body.refs[1].negated);
    assert_eq!(body.refs[2].offset, Some((0, 0)));
    assert!(!body.refs[2].negated);
    assert_eq!(body.refs[3].offset, Some((1, -2)));
    assert!(body.refs[3].negated);
}

#[test]
fn malformed_ref_is_not_reinterpreted_as_auto_ref() {
    for malformed in [
        "ref target 1",
        "ref target garbage",
        "ref target 32768 0",
        "ref target 1 2 extra",
        "ref target negated extra",
    ] {
        let input = format!("glyph composite\n{malformed}\n");
        let doc = parse_document_from_str(&input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 2, "input: {malformed}");
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected glyph for input: {malformed}");
        };
        assert!(body.refs.is_empty(), "input: {malformed}");
        assert!(
            matches!(&doc.items[1], DocumentItem::Directive(line) if line == malformed),
            "input: {malformed}",
        );
    }
}

// -----------------------------------------------------------------------
// DocLine round-trip tests
// -----------------------------------------------------------------------
