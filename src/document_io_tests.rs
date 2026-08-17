//! Tests for [`super::document_io`]: parser round-trips and derivation.
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items while keeping the source at a readable size.

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
        matches!(&doc.items[0], DocumentItem::Map { slices, char_repr, glyph, .. }
            if slices.is_empty() && char_repr == ":" && glyph == "colon"),
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
        matches!(&doc.items[2], DocumentItem::Map { slices, glyph, .. }
            if slices == &["wide", "narrow"] && glyph == "triple-star($half)"),
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

fn docline_roundtrip(input: &str) {
    let lines = parse_doclines(input);
    let mut output = Vec::new();
    serialize_doclines(&lines, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(input, output_str, "serialize_doclines did not round-trip");
}

#[test]
fn docline_roundtrip_simple() {
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
    docline_roundtrip(input);

    let lines = parse_doclines(input);
    // comment, 3x meta, blank, glyph-header, Grid, blank, glyph-header, ref, blank, directive
    assert_eq!(lines.len(), 12);
    assert!(matches!(lines[0], DocLine::Text(ref s) if s.starts_with("//")));
    assert!(matches!(lines[1], DocLine::Text(ref s) if s.starts_with("meta ")));
    assert!(matches!(lines[5], DocLine::Text(ref s) if s.starts_with("glyph test-glyph")));
    assert!(matches!(lines[6], DocLine::Grid(_)));
    assert!(matches!(lines[8], DocLine::Text(ref s) if s.starts_with("glyph uni0041")));
    assert!(matches!(lines[9], DocLine::Text(ref s) if s.starts_with("ref ")));
}

#[test]
fn docline_roundtrip_alias() {
    docline_roundtrip("glyph uni0041 = test-glyph\n");
    let lines = parse_doclines("glyph uni0041 = test-glyph\n");
    assert_eq!(lines.len(), 1);
    assert!(matches!(lines[0], DocLine::Text(_)));
}

#[test]
fn docline_roundtrip_ref_only_glyph() {
    let input = "\
glyph composite
ref part-a 0 0
ref part-b 4 2
";
    docline_roundtrip(input);
    let lines = parse_doclines(input);
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|l| matches!(l, DocLine::Text(_))));
}

#[test]
fn docline_roundtrip_glyph_with_pixels_and_refs() {
    let input = "\
glyph mixed 2 2
..@@
@@..
ref other 1 1
";
    docline_roundtrip(input);
    let lines = parse_doclines(input);
    assert_eq!(lines.len(), 3);
    assert!(matches!(lines[0], DocLine::Text(_)));
    assert!(matches!(lines[1], DocLine::Grid(_)));
    assert!(matches!(lines[2], DocLine::Text(ref s) if s.starts_with("ref ")));
}

// -----------------------------------------------------------------------
// derive_document equivalence tests
// -----------------------------------------------------------------------

fn assert_derive_equivalent(input: &str) {
    let old_doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let lines = parse_doclines(input);
    let (new_doc, starts) = derive_document(&lines, "test.unf".into()).unwrap();

    assert_eq!(
        old_doc.items.len(),
        new_doc.items.len(),
        "item count mismatch"
    );
    assert_eq!(starts.len(), new_doc.items.len());

    for (idx, (old_item, new_item)) in old_doc.items.iter().zip(new_doc.items.iter()).enumerate() {
        match (old_item, new_item) {
            (DocumentItem::BlankLine, DocumentItem::BlankLine) => {}
            (DocumentItem::Comment(a), DocumentItem::Comment(b)) => {
                assert_eq!(a, b, "comment mismatch at item {idx}");
            }
            (DocumentItem::Meta(a), DocumentItem::Meta(b)) => {
                assert_eq!(a, b, "meta mismatch at item {idx}");
            }
            (DocumentItem::Audit(a), DocumentItem::Audit(b)) => {
                assert_eq!(a, b, "audit mismatch at item {idx}");
            }
            (DocumentItem::Directive(a), DocumentItem::Directive(b)) => {
                assert_eq!(a, b, "directive mismatch at item {idx}");
            }
            (
                DocumentItem::NameParts {
                    name: n1,
                    values: v1,
                    ..
                },
                DocumentItem::NameParts {
                    name: n2,
                    values: v2,
                    ..
                },
            ) => {
                assert_eq!(n1, n2, "name-parts name mismatch at item {idx}");
                assert_eq!(v1, v2, "name-parts values mismatch at item {idx}");
            }
            (
                DocumentItem::Remap {
                    feature: f1,
                    lookbehind: lb1,
                    source: s1,
                    target: t1,
                    lookahead: la1,
                    ..
                },
                DocumentItem::Remap {
                    feature: f2,
                    lookbehind: lb2,
                    source: s2,
                    target: t2,
                    lookahead: la2,
                    ..
                },
            ) => {
                assert_eq!(f1, f2, "remap feature mismatch at item {idx}");
                assert_eq!(lb1, lb2, "remap lookbehind mismatch at item {idx}");
                assert_eq!(s1, s2, "remap source mismatch at item {idx}");
                assert_eq!(t1, t2, "remap target mismatch at item {idx}");
                assert_eq!(la1, la2, "remap lookahead mismatch at item {idx}");
            }
            (
                DocumentItem::Feature {
                    name: n1,
                    scripts: s1,
                    remap_group: r1,
                    ..
                },
                DocumentItem::Feature {
                    name: n2,
                    scripts: s2,
                    remap_group: r2,
                    ..
                },
            ) => {
                assert_eq!(n1, n2, "feature name mismatch at item {idx}");
                assert_eq!(s1, s2, "feature scripts mismatch at item {idx}");
                assert_eq!(r1, r2, "feature remap_group mismatch at item {idx}");
            }
            (
                DocumentItem::Glyph { name: n1, body: b1 },
                DocumentItem::Glyph { name: n2, body: b2 },
            ) => {
                assert_eq!(n1.display(), n2.display(), "name mismatch at item {idx}");
                assert_eq!(b1.pixels, b2.pixels, "pixels mismatch at item {idx}");
                assert_eq!(
                    b1.refs.len(),
                    b2.refs.len(),
                    "ref count mismatch at item {idx}"
                );
                for (ri, (r1, r2)) in b1.refs.iter().zip(b2.refs.iter()).enumerate() {
                    assert_eq!(r1.name, r2.name, "ref name mismatch at item {idx} ref {ri}");
                    assert_eq!(
                        r1.offset, r2.offset,
                        "ref offset mismatch at item {idx} ref {ri}"
                    );
                    assert_eq!(
                        r1.negated, r2.negated,
                        "ref negation mismatch at item {idx} ref {ri}"
                    );
                }
            }
            (
                DocumentItem::GlyphAlias {
                    name: n1,
                    target: t1,
                    ..
                },
                DocumentItem::GlyphAlias {
                    name: n2,
                    target: t2,
                    ..
                },
            ) => {
                assert_eq!(n1.display(), n2.display(), "name mismatch at item {idx}");
                assert_eq!(t1, t2, "alias target mismatch at item {idx}");
            }
            _ => panic!(
                "item kind mismatch at item {idx}: {:?} vs {:?}",
                std::mem::discriminant(old_item),
                std::mem::discriminant(new_item),
            ),
        }
    }
}

#[test]
fn derive_equivalent_simple() {
    assert_derive_equivalent(
        "\
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
",
    );
}

#[test]
fn derive_equivalent_alias() {
    assert_derive_equivalent("glyph uni0041 = test-glyph\n");
}

#[test]
fn derive_equivalent_mixed_refs() {
    assert_derive_equivalent(
        "\
glyph mixed 2 2
..@@
@@..
ref other 1 1
",
    );
}

#[test]
fn derive_item_line_starts() {
    let input = "\
// comment
glyph foo 2 1
..@@
ref bar 0 0
";
    let lines = parse_doclines(input);
    let (doc, starts) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    assert_eq!(starts, vec![0, 1]); // comment at line 0, glyph header at line 1
}

// -----------------------------------------------------------------------
// Intermediate editing states (derive_document tolerance)
// -----------------------------------------------------------------------

#[test]
fn derive_empty_body_glyph() {
    let input = "glyph foo\n";
    let lines = parse_doclines(input);
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert!(body.pixels.is_none());
        assert!(body.refs.is_empty());
    } else {
        panic!("expected glyph");
    }
}

#[test]
fn derive_glyph_header_split_from_alias() {
    let input = "glyph foo\n= bar\n";
    let lines = parse_doclines(input);
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert!(body.pixels.is_none());
        assert!(body.refs.is_empty());
    } else {
        panic!("expected glyph at item 0");
    }
    assert!(matches!(doc.items[1], DocumentItem::Directive(_)));
}

#[test]
fn derive_glyph_with_dims_no_grid_docline() {
    // Simulates editing state: header with dims but Grid DocLine removed
    let lines = vec![DocLine::Text("glyph foo 8 16".to_string())];
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        let grid = body.pixels.as_ref().expect("should have empty grid");
        assert_eq!(grid.width, 8);
        assert_eq!(grid.height, 16);
        assert!(grid.is_all_empty());
        assert!(body.refs.is_empty());
    } else {
        panic!("expected glyph");
    }
}

#[test]
fn docline_roundtrip_all_directive_types() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

// a comment
name-parts $base = stem wide

glyph stem 2 2
@@@@
..@@

glyph wide 3 1
@@..@@

glyph alias = stem

glyph comp
ref stem
ref wide 1 0
anchor -join 0 0
anchor +join 2 0

glyph batch
ref stem-(a|b)

glyph keep-empty keep advance 0

map A = stem
map B = wide
remap set1 : stem -> wide
remap liga1 : stem wide -> batch
remap liga2 : stem wide -> batch stem
feature liga for latn : set1
exclude-from-sample stem
";
    let lines = parse_doclines(input);
    let mut output = Vec::new();
    serialize_doclines(&lines, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(input, output_str, "DocLine round-trip failed");

    let old_doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let (new_doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(
        old_doc.items.len(),
        new_doc.items.len(),
        "item count mismatch"
    );
}

#[test]
fn strict_parse_rejects_partial_glyph_header() {
    let input = "glyph x 2 nope\n..@@\n";
    assert!(parse_document_from_str(input, "test.unf".into()).is_err());
}

#[test]
fn strict_parse_accepts_valid_glyph_headers() {
    for input in [
        "glyph foo\n",
        "glyph foo 2 1\n..@@\n",
        "glyph foo keep\n",
        "glyph foo 2 1 keep\n..@@\n",
        "glyph foo advance 5\n",
        "glyph foo origin -1 2\n",
        "glyph foo 2 1 keep advance 5 origin -1 2\n..@@\n",
        "glyph foo 2 1 desync\n..@@\n",
        "glyph foo desync 2 1\n..@@\n",
        "glyph foo = bar\n",
    ] {
        assert!(
            parse_document_from_str(input, "test.unf".into()).is_ok(),
            "should accept: {input:?}"
        );
    }
}

/// `desync` says the grid that follows is bitmap ink only, so a header
/// carrying it still owns a pixel grid — reconciliation and the document model
/// have to agree about that, whichever side of `W H` the flag sits on.
#[test]
fn desync_header_owns_its_pixel_grid_and_round_trips() {
    for input in [
        "glyph foo 2 1 desync\n..@@\n",
        "glyph foo desync 2 1\n..@@\n",
    ] {
        let tokens = tokenize_tokens(input.lines().next().unwrap()).unwrap();
        assert_eq!(
            glyph_header_dims(&tokens[1..]).map(|d| (d.width, d.height)),
            Some((2, 1)),
            "desync header should still own a grid: {input:?}"
        );

        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {:?}", doc.items[0]);
        };
        assert!(body.desync, "desync should reach the body: {input:?}");
        assert!(body.pixels.is_some(), "the grid is still parsed: {input:?}");

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "glyph foo 2 1 desync\n..@@\n",
        );
    }
}

/// `origin C R` and `extent W H` are the only two-valued header flags, so the
/// parser has to take both components before it hands the walker back — and a
/// bare `W H` pair on the same line must still be the grid's, not theirs.
#[test]
fn origin_and_extent_parse_in_any_order_and_round_trip() {
    for input in [
        "glyph foo 2 1 origin -1 3 extent 4 5\n..@@\n",
        "glyph foo origin -1 3 2 1 extent 4 5\n..@@\n",
        "glyph foo extent 4 5 origin -1 3 2 1\n..@@\n",
    ] {
        let tokens = tokenize_tokens(input.lines().next().unwrap()).unwrap();
        assert_eq!(
            glyph_header_dims(&tokens[1..]).map(|d| (d.width, d.height)),
            Some((2, 1)),
            "the grid's own W H must not be eaten by a two-valued flag: {input:?}"
        );

        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {:?}", doc.items[0]);
        };
        assert_eq!(body.origin, Some((-1, 3)), "{input:?}");
        assert_eq!(body.extent, Some((4, 5)), "{input:?}");
        assert_eq!(body.declared_origin(), (-1, 3), "{input:?}");
        assert_eq!(body.declared_extent(), Some((4, 5)), "{input:?}");

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "glyph foo 2 1 origin -1 3 extent 4 5\n..@@\n",
        );
    }
}

/// `scale 0` is not a scale. It used to parse, multiply the header's `W H` by
/// zero, and turn the pixel rows that followed into unrecognized directives —
/// an error cascade naming everything except the flag that caused it. Every
/// consumer clamps the scale to 1 to stay out of a division by zero; this is
/// what makes those clamps defensive rather than load-bearing.
#[test]
fn a_scale_of_zero_is_rejected_where_it_is_written() {
    let input = "glyph foo 2 2 scale 0\n@@@@\n@@@@\n";
    let err =
        parse_document_from_str(input, "test.unf".into()).expect_err("`scale 0` must not parse");
    assert!(
        format!("{err}").contains("scale"),
        "the error must name the flag, got {err}"
    );
    assert!(
        parse_document_from_str("glyph foo 2 2 scale 1\n@@@@\n@@@@\n", "test.unf".into()).is_ok()
    );
}

/// The one rewriter the box editor uses: it replaces a flag's value where the
/// flag already is, drops a flag whose value is gone, and appends the ones that
/// were never written — leaving the name's quoting, the other flags, the
/// spacing and the trailing comment exactly as they were.
#[test]
fn a_box_flag_is_rewritten_where_it_stands() {
    let cases = [
        // Adding to a header that states nothing.
        (
            "glyph foo 4 2",
            (Some((1, -2)), Some(5), None),
            "glyph foo 4 2 origin 1 -2 advance 5",
        ),
        // Replacing in place, comment and flag order untouched.
        (
            "glyph foo 4 2 advance 5 mark // hi",
            (None, Some(7), None),
            "glyph foo 4 2 advance 7 mark // hi",
        ),
        (
            "glyph 'a b' 4 2 origin 1 -2 keep",
            (Some((3, 0)), None, None),
            "glyph 'a b' 4 2 origin 3 0 keep",
        ),
        // A flag whose value is gone goes with it.
        (
            "glyph foo 4 2 origin 1 -2 mark",
            (None, None, None),
            "glyph foo 4 2 mark",
        ),
        // `extent` replaces `advance`: the two state the same slot, so writing
        // one has to unwrite the other.
        (
            "glyph foo 4 2 advance 5",
            (None, None, Some((5, 16))),
            "glyph foo 4 2 extent 5 16",
        ),
    ];
    for (line, (origin, advance, extent), expected) in cases {
        assert_eq!(
            replace_glyph_box_flags(line, origin, advance, extent).as_deref(),
            Some(expected),
            "{line:?}"
        );
    }

    // Not a glyph header, or an alias: nothing to rewrite.
    assert_eq!(
        replace_glyph_box_flags("ref foo 1 2", None, Some(1), None),
        None
    );
    assert_eq!(
        replace_glyph_box_flags("glyph foo = bar", None, Some(1), None),
        None
    );
}

/// Stating the box's width twice is a mistake, not a precedence question:
/// `advance` and `extent` both say it. Reporting it beats picking a winner,
/// since the source that writes both plainly expects both to count.
#[test]
fn one_header_may_not_state_a_box_slot_twice() {
    let input = "glyph foo 4 2 advance 3 extent 3 2\n@@@@@@@@\n@@@@@@@@\n";
    assert!(
        parse_document_from_str(input, "test.unf".into()).is_err(),
        "{input:?} states the width twice and must not parse"
    );
    // The two flags that state *different* slots are the ordinary case.
    let accepted = "glyph foo 4 2 origin 1 -1 advance 0\n@@@@@@@@\n@@@@@@@@\n";
    let doc = parse_document_from_str(accepted, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected a glyph item");
    };
    assert_eq!(
        (body.declared_origin(), body.declared_extent()),
        ((1, -1), Some((0, 2)))
    );
}

/// Zero is a declared box's real answer, not a missing one: a combining mark
/// says `extent 0 H` (or, in the old spelling, `advance 0`) to take no width at
/// all. `None` is reserved for a glyph that declares no box — the composite
/// whose box is the union of what it places.
#[test]
fn a_zero_extent_is_declared_and_an_absent_one_is_not() {
    let cases = [
        (
            "glyph foo 4 2 extent 0 0\n@@@@@@@@\n@@@@@@@@\n",
            Some((0, 0)),
        ),
        (
            "glyph foo 4 2 extent 0 9\n@@@@@@@@\n@@@@@@@@\n",
            Some((0, 9)),
        ),
        (
            "glyph foo 4 2 extent 9 0\n@@@@@@@@\n@@@@@@@@\n",
            Some((9, 0)),
        ),
        (
            "glyph foo 4 2 advance 0\n@@@@@@@@\n@@@@@@@@\n",
            Some((0, 2)),
        ),
        ("glyph foo 4 2\n@@@@@@@@\n@@@@@@@@\n", Some((4, 2))),
        ("glyph foo\nref bar\n", None),
    ];
    for (input, expected) in cases {
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {input:?}");
        };
        assert_eq!(body.declared_extent(), expected, "{input:?}");
    }
}

/// An alias is a name for a glyph and nothing else, so the flags a real glyph
/// takes are a mistake on one — silently dropping them would build a font that
/// does not say what the file says.
#[test]
fn strict_parse_rejects_flags_on_an_alias() {
    for input in [
        "glyph foo keep = bar\n",
        "glyph foo advance 5 = bar\n",
        "glyph foo desync = bar\n",
        "glyph foo 2 1 = bar\n",
    ] {
        let err = parse_document_from_str(input, "test.unf".into())
            .expect_err(&format!("should reject: {input:?}"))
            .to_string();
        assert!(err.contains("takes no flags"), "unexpected error: {err}");
    }
}

// -----------------------------------------------------------------------
// Backtick-quoting tokenizer tests
// -----------------------------------------------------------------------

#[test]
fn comment_lines_are_not_tokenized() {
    // A comment is free text; backticks in it are not quoting syntax.
    // Tokenizing comments made one stray backtick abort the whole file,
    // and the CLI build then silently proceeded without that file.
    let input = "// see `foo`/`bar`\nglyph a 2 1\n@@..\n";
    let doc = parse_document_from_str(input, "test.unf".into())
        .expect("comment with backticks must parse");
    assert!(matches!(&doc.items[0], DocumentItem::Comment(_)));
}

#[test]
fn tokenize_simple_whitespace() {
    assert_eq!(
        tokenize_tokens("hello world").unwrap(),
        vec!["hello", "world"],
    );
}

#[test]
fn tokenize_empty_string() {
    assert!(tokenize_tokens("").unwrap().is_empty());
    assert!(tokenize_tokens("   ").unwrap().is_empty());
}

#[test]
fn tokenize_unquoted_backtick() {
    // a`b = 3 chars, single unquoted token
    assert_eq!(tokenize_tokens("a`b").unwrap(), vec!["a`b"]);
}

#[test]
fn tokenize_quoted_empty() {
    // `` = empty string
    assert_eq!(tokenize_tokens("``").unwrap(), vec![""]);
}

#[test]
fn tokenize_quoted_backtick() {
    // ```` = one backtick character
    assert_eq!(tokenize_tokens("````").unwrap(), vec!["`"]);
}

#[test]
fn tokenize_quoted_with_spaces() {
    // `a b` = "a b" (3 chars)
    assert_eq!(tokenize_tokens("`a b`").unwrap(), vec!["a b"]);
}

#[test]
fn tokenize_quoted_error_no_space() {
    // `ab`c = error
    assert!(tokenize_tokens("`ab`c").is_err());
}

#[test]
fn tokenize_unclosed_quote() {
    assert!(tokenize_tokens("`abc").is_err());
}

#[test]
fn tokenize_mixed() {
    assert_eq!(
        tokenize_tokens("glyph `foo bar` 8 16").unwrap(),
        vec!["glyph", "foo bar", "8", "16"],
    );
}

#[test]
fn tokenize_multiple_quoted() {
    assert_eq!(tokenize_tokens("`` `a` ````").unwrap(), vec!["", "a", "`"],);
}

#[test]
fn tokenize_quoted_with_escaped_backtick() {
    // `a``b` = "a`b"
    assert_eq!(tokenize_tokens("`a``b`").unwrap(), vec!["a`b"]);
}

#[test]
fn quote_token_simple() {
    assert_eq!(quote_token("hello"), "hello");
}

#[test]
fn quote_token_empty() {
    assert_eq!(quote_token(""), "``");
}

#[test]
fn quote_token_with_space() {
    assert_eq!(quote_token("a b"), "`a b`");
}

#[test]
fn quote_token_backtick() {
    assert_eq!(quote_token("`"), "````");
}

#[test]
fn quote_token_starts_with_backtick() {
    assert_eq!(quote_token("`foo"), "```foo`");
}

#[test]
fn quote_roundtrip() {
    for val in ["", "hello", "a b", "`", "a`b", "`foo", "``", "a b c"] {
        let quoted = quote_token(val);
        let parsed = tokenize_tokens(&quoted).unwrap();
        assert_eq!(parsed, vec![val], "roundtrip failed for {val:?}");
    }
}

/// A `map` may name a Unicode variation sequence: a base character and a
/// variation selector. The two written forms — `U+XXXX U+YYYY` as separate
/// tokens, and the two characters written literally as one token — are
/// different text and each has to come back out exactly as it went in.
#[test]
fn parse_map_uvs_pair_forms() {
    let input = "\
map U+0030 U+FE0F = num-zero-emoji
map 0\u{FE0F} = num-zero-emoji
map wide : U+26AA U+FE0E = circle
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { char_repr, selector, glyph, .. }
            if char_repr == "U+0030" && selector.as_deref() == Some("U+FE0F")
                && glyph == "num-zero-emoji"),
        "got {:?}",
        doc.items[0],
    );
    // Written literally the pair is a single token, and the parser splits it
    // only when it really is base + selector.
    assert!(
        matches!(&doc.items[1], DocumentItem::Map { char_repr, selector, .. }
            if char_repr == "0" && selector.as_deref() == Some("\u{FE0F}")),
        "got {:?}",
        doc.items[1],
    );
    assert!(
        matches!(&doc.items[2], DocumentItem::Map { slices, char_repr, selector, .. }
            if slices == &["wide"] && char_repr == "U+26AA"
                && selector.as_deref() == Some("U+FE0E")),
        "got {:?}",
        doc.items[2],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// Only a two-character token whose second character is a selector splits. A
/// longer sequence — what pasting `0️⃣` gives you — stays one `char_repr` so
/// that validation can reject it with a message about splitting the line,
/// rather than the parser silently keeping the first two characters.
#[test]
fn a_longer_sequence_is_not_split_into_a_uvs_pair() {
    let doc =
        parse_document_from_str("map 0\u{FE0F}\u{20E3} = keycap-zero\n", "t.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { char_repr, selector, .. }
            if char_repr == "0\u{FE0F}\u{20E3}" && selector.is_none()),
        "got {:?}",
        doc.items[0],
    );

    // A pipe list keeps its shape too: the last character of `a|b` is not a
    // selector, and nothing may be shaved off the end of an alternation.
    let doc = parse_document_from_str("map a|b = ab\n", "t.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { char_repr, selector, .. }
            if char_repr == "a|b" && selector.is_none()),
        "got {:?}",
        doc.items[0],
    );
}

/// `map generate` parses the extended syntax so that a sequence written there
/// is a validation error with a real message, not an unreadable line. It stays
/// a single codepoint semantically: a variation sequence has no canonical
/// decomposition, so there is nothing for `generate` to synthesize.
#[test]
fn map_generate_parses_but_does_not_take_a_sequence() {
    let input = "\
map generate U+0030 U+FE0F
map generate U+0030 U+FE0F = num-zero-emoji
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::MapDecomposed { char_repr, selector, glyph, .. }
            if char_repr == "U+0030" && selector.as_deref() == Some("U+FE0F") && glyph.is_none()),
        "got {:?}",
        doc.items[0],
    );
    assert!(
        matches!(&doc.items[1], DocumentItem::MapDecomposed { char_repr, selector, glyph, .. }
            if char_repr == "U+0030" && selector.as_deref() == Some("U+FE0F")
                && glyph.as_deref() == Some("num-zero-emoji")),
        "got {:?}",
        doc.items[1],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `map generate Á = a-acute` and `map U+0030 U+FE0F = x` have the same arity.
/// The `generate` keyword is what tells them apart, so a glyph reached through
/// the plain form must not be captured by the decomposed one.
#[test]
fn map_generate_wins_the_arity_it_shares_with_a_uvs_pair() {
    let doc = parse_document_from_str("map generate Á = a-acute\n", "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::MapDecomposed { char_repr, .. } if char_repr == "Á"),
        "got {:?}",
        doc.items[0],
    );
}

#[test]
fn parse_map_generate_forms() {
    let input = "\
map generate À // plain
map generate Á = a-acute // named
map generate = g
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::MapDecomposed { char_repr, glyph, comment, .. }
            if char_repr == "À" && glyph.is_none() && comment.as_deref() == Some("plain")),
        "got {:?}",
        doc.items[0],
    );
    assert!(
        matches!(&doc.items[1], DocumentItem::MapDecomposed { char_repr, glyph, comment, .. }
            if char_repr == "Á" && glyph.as_deref() == Some("a-acute")
                && comment.as_deref() == Some("named")),
        "got {:?}",
        doc.items[1],
    );
    // `generate` in the plain form's own arity stays a plain `map`, so a glyph
    // that happens to be called `generate` is still reachable.
    assert!(
        matches!(&doc.items[2], DocumentItem::Map { char_repr, glyph, .. }
            if char_repr == "generate" && glyph == "g"),
        "got {:?}",
        doc.items[2],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn parse_map_with_quoted_backtick() {
    // map ```` = bquot  →  map backtick-char to "bquot"
    let input = "map ```` = bquot\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Map {
        char_repr, glyph, ..
    } = &doc.items[0]
    {
        assert_eq!(char_repr, "`");
        assert_eq!(glyph, "bquot");
    } else {
        panic!("expected Map");
    }

    // Roundtrip
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn parse_name_parts_with_empty_token() {
    // name-parts $init0 = `` $init
    let input = "name-parts $init0 = `` $init\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::NameParts { name, values, .. } = &doc.items[0] {
        assert_eq!(name, "$init0");
        assert_eq!(values, &vec!["".to_string(), "$init".to_string()]);
    } else {
        panic!("expected NameParts");
    }

    // Roundtrip
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn parse_glyph_with_quoted_name() {
    let input = "`glyph` `foo bar` 2 1\n..@@\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { name, body } = &doc.items[0] {
        assert_eq!(name.display(), "foo bar");
        assert!(body.pixels.is_some());
    } else {
        panic!("expected Glyph");
    }
}

#[test]
fn tokenize_with_spans_basic() {
    let spans = tokenize_with_spans("glyph `foo` 8").unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].value, "glyph");
    assert_eq!(spans[0].raw_start, 0);
    assert_eq!(spans[0].raw_end, 5);
    assert_eq!(spans[1].value, "foo");
    assert_eq!(spans[1].raw_start, 6);
    assert_eq!(spans[1].raw_end, 11); // includes backticks
    assert_eq!(spans[2].value, "8");
    assert_eq!(spans[2].raw_start, 12);
    assert_eq!(spans[2].raw_end, 13);
}

#[test]
fn roundtrip_color_directive() {
    let input = "color red = #ff0000\ncolor blue = #0000ffcc coloronly\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    if let DocumentItem::Color {
        name,
        value,
        visibility,
        ..
    } = &doc.items[0]
    {
        assert_eq!(name, "red");
        assert_eq!(value, "#ff0000");
        assert!(visibility.is_none());
    } else {
        panic!("expected Color");
    }
    if let DocumentItem::Color {
        name,
        value,
        visibility,
        ..
    } = &doc.items[1]
    {
        assert_eq!(name, "blue");
        assert_eq!(value, "#0000ffcc");
        assert_eq!(*visibility, Some(LayerVisibility::ColorOnly));
    } else {
        panic!("expected Color");
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn roundtrip_ref_fill() {
    let input = "\
glyph combo
ref part-a fill #ff0000
ref part-b 2 3 fill fg coloronly
ref part-c fill blue monoonly
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert_eq!(body.refs.len(), 3);
        let r0 = &body.refs[0];
        assert_eq!(r0.name, "part-a");
        assert_eq!(r0.offset, None);
        let f0 = r0.fill.as_ref().unwrap();
        assert_eq!(f0.color, "#ff0000");
        assert!(r0.visibility.is_none());

        let r1 = &body.refs[1];
        assert_eq!(r1.name, "part-b");
        assert_eq!(r1.offset, Some((2, 3)));
        let f1 = r1.fill.as_ref().unwrap();
        assert_eq!(f1.color, "fg");
        assert_eq!(r1.visibility, Some(LayerVisibility::ColorOnly));

        let r2 = &body.refs[2];
        assert_eq!(r2.fill.as_ref().unwrap().color, "blue");
        assert_eq!(r2.visibility, Some(LayerVisibility::MonoOnly));
    } else {
        panic!("expected Glyph");
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn ref_visibility_without_fill() {
    let input = "\
glyph combo
ref part-a coloronly
ref part-b monoonly
ref part-c fill #ff0000 monoonly
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert_eq!(body.refs.len(), 3);

        let r0 = &body.refs[0];
        assert!(r0.fill.is_none());
        assert_eq!(r0.visibility, Some(LayerVisibility::ColorOnly));

        let r1 = &body.refs[1];
        assert!(r1.fill.is_none());
        assert_eq!(r1.visibility, Some(LayerVisibility::MonoOnly));

        let r2 = &body.refs[2];
        assert_eq!(r2.fill.as_ref().unwrap().color, "#ff0000");
        assert_eq!(r2.visibility, Some(LayerVisibility::MonoOnly));
    } else {
        panic!("expected Glyph");
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn ref_fill_negated_combined() {
    let input = "glyph foo\nref bar 1 2 negated fill #00ff00\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        let r = &body.refs[0];
        assert_eq!(r.name, "bar");
        assert_eq!(r.offset, Some((1, 2)));
        assert!(r.negated);
        assert_eq!(r.fill.as_ref().unwrap().color, "#00ff00");
    } else {
        panic!("expected Glyph");
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn ref_inherit_roundtrip() {
    let input = "\
glyph foo
ref plain
ref auto-inherit inherit
ref offset-inherit 1 -2 inherit
ref full 1 2 negated inherit fill #00ff00 coloronly
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected Glyph");
    };
    assert_eq!(body.refs.len(), 4);
    assert!(!body.refs[0].inherit);
    assert!(body.refs[1].inherit);
    assert_eq!(body.refs[1].offset, None);
    assert!(body.refs[2].inherit);
    assert_eq!(body.refs[2].offset, Some((1, -2)));
    assert!(body.refs[3].inherit);
    assert!(body.refs[3].negated);
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    // Canonical order: negated, inherit, fill, visibility.
    assert!(
        output_str.contains("ref auto-inherit inherit\n"),
        "{output_str}"
    );
    assert!(
        output_str.contains("ref offset-inherit 1 -2 inherit\n"),
        "{output_str}"
    );
    assert!(
        output_str.contains("ref full 1 2 negated inherit fill #00ff00 coloronly\n"),
        "{output_str}"
    );
}

/// `ifexists` is a flag like the others: it survives a round trip beside them,
/// on a ref with an offset and on one without.
#[test]
fn ref_ifexists_roundtrip() {
    let input = "\
glyph foo
ref plain
ref maybe ifexists
ref placed 1 -2 ifexists
ref full 1 2 negated inherit ifexists fill #00ff00 coloronly
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected Glyph");
    };
    let flags: Vec<bool> = body.refs.iter().map(|r| r.if_exists).collect();
    assert_eq!(flags, vec![false, true, true, true]);
    assert_eq!(body.refs[2].offset, Some((1, -2)));
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// The `map` flag is trailing, so it has to come off before the arities are
/// counted — including for the `BASE SELECTOR` form, which has one more token.
#[test]
fn map_ifexists_roundtrip() {
    let input = "\
map A = a-upper ifexists
map wide : U+E000..E00F = private-($#e000..e00f) ifexists
map U+0030 U+FE0F = zero-text ifexists
map B = b-upper
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let flags: Vec<(bool, Option<&str>)> = doc
        .items
        .iter()
        .filter_map(|item| match item {
            DocumentItem::Map {
                if_exists,
                selector,
                ..
            } => Some((*if_exists, selector.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        flags,
        vec![
            (true, None),
            (true, None),
            (true, Some("U+FE0F")),
            (false, None),
        ],
        "{:?}",
        doc.items,
    );
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `generate` synthesizes its target instead of naming one, so there is nothing
/// for the flag to be conditional on: the line stays unreadable rather than
/// parsing and losing the token on the next save.
#[test]
fn map_generate_rejects_ifexists() {
    let doc = parse_document_from_str("map generate Á ifexists\n", "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Directive(text) if text.contains("ifexists")),
        "{:?}",
        doc.items,
    );
}

#[test]
fn parse_assert_shape_basic() {
    let input = "assert shape `AB` : a-upper : b-upper\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::AssertShape {
        text,
        features,
        expected,
        comment,
        ..
    } = &doc.items[0]
    {
        assert_eq!(text, "AB");
        assert!(features.is_empty());
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[0].name, "a-upper");
        assert_eq!(expected[1].name, "b-upper");
        assert!(expected[0].advance.is_none());
        assert!(comment.is_none());
    } else {
        panic!("expected AssertShape");
    }
}

/// The language rides along with the feature flags, before the first `:`.
/// Order between `@lang` and `+feat`/`-feat` is free on the way in;
/// serializing normalizes it to language first.
#[test]
fn parse_assert_shape_with_language() {
    let input = "assert shape `\u{15f}` +liga @ro : uni0219\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::AssertShape {
        language,
        features,
        expected,
        ..
    } = &doc.items[0]
    {
        assert_eq!(language.as_deref(), Some("ro"));
        assert_eq!(features.len(), 1, "the feature flag must survive beside it");
        assert_eq!(expected[0].name, "uni0219");
    } else {
        panic!("expected AssertShape");
    }
    assert_eq!(
        doc.items[0].serialize_line().unwrap(),
        "assert shape \u{15f} @ro +liga : uni0219",
    );
}

/// Without an `@tag` the directive must round-trip exactly as before, `@`
/// included nowhere.
#[test]
fn assert_shape_without_a_language_is_unchanged() {
    let input = "assert shape `AB` : a-upper : b-upper\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::AssertShape { language, .. } = &doc.items[0] {
        assert!(language.is_none());
    } else {
        panic!("expected AssertShape");
    }
    assert_eq!(
        doc.items[0].serialize_line().unwrap(),
        "assert shape AB : a-upper : b-upper",
    );
}

#[test]
fn parse_assert_shape_with_features_and_props() {
    let input = "assert shape `fi` +liga -frac : fi-lig advance 512 : x offset 10 20\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::AssertShape {
        text,
        features,
        expected,
        ..
    } = &doc.items[0]
    {
        assert_eq!(text, "fi");
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].tag, "liga");
        assert!(features[0].enable);
        assert_eq!(features[1].tag, "frac");
        assert!(!features[1].enable);
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[0].name, "fi-lig");
        assert_eq!(expected[0].advance, Some(512));
        assert_eq!(expected[1].name, "x");
        assert_eq!(expected[1].offset, Some((10, 20)));
    } else {
        panic!("expected AssertShape");
    }
}

#[test]
fn roundtrip_assert_shape() {
    let input = "assert shape `AB` +liga : a-upper advance 512 : b-upper offset 10 20\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(
        output_str,
        "assert shape AB +liga : a-upper advance 512 : b-upper offset 10 20\n"
    );
}

#[test]
fn roundtrip_assert_shape_quoted_text() {
    let input = "assert shape `hello world` : hw\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn parse_assert_same() {
    let input = "assert same foo bar\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::AssertSame { names, comment } = &doc.items[0] {
        assert_eq!(names, &["foo", "bar"]);
        assert!(comment.is_none());
    } else {
        panic!("expected AssertSame, got {:?}", doc.items[0]);
    }
}

#[test]
fn parse_assert_distinct() {
    let input = "assert distinct a b c\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::AssertDistinct { names, .. } = &doc.items[0] {
        assert_eq!(names, &["a", "b", "c"]);
    } else {
        panic!("expected AssertDistinct, got {:?}", doc.items[0]);
    }
}

#[test]
fn roundtrip_assert_same() {
    let input = "assert same foo bar baz\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn roundtrip_assert_distinct() {
    let input = "assert distinct foo bar\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn assert_same_too_few_names_falls_back() {
    let input = "assert same foo\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(matches!(&doc.items[0], DocumentItem::Directive(_)));
}

#[test]
fn roundtrip_assert_same_quoted() {
    let input = "assert same `foo bar` `baz quux`\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::AssertSame { names, .. } = &doc.items[0] {
        assert_eq!(names, &["foo bar", "baz quux"]);
    } else {
        panic!("expected AssertSame, got {:?}", doc.items[0]);
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn parse_assert_same_with_comment() {
    let input = "assert same foo bar // they should match\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::AssertSame { names, comment } = &doc.items[0] {
        assert_eq!(names, &["foo", "bar"]);
        assert_eq!(comment.as_deref(), Some("they should match"));
    } else {
        panic!("expected AssertSame, got {:?}", doc.items[0]);
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn roundtrip_assert_shape_with_comment() {
    let input = "assert shape AB : a-upper : b-upper // check shaping\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::AssertShape { comment, .. } = &doc.items[0] {
        assert_eq!(comment.as_deref(), Some("check shaping"));
    } else {
        panic!("expected AssertShape, got {:?}", doc.items[0]);
    }
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

// -----------------------------------------------------------------------
// Inline `// …` comments (every directive except pixel rows)
// -----------------------------------------------------------------------

#[test]
fn comment_is_one_unquotable_token_at_the_end_of_the_line() {
    // `foo `//` bar // quux` is four tokens: the quoted `//` is an
    // ordinary token, and everything from the unquoted `//` on is one
    // comment token that quoting no longer applies to.
    let (body, comment) = split_comment("foo ```` bar // quux");
    assert_eq!(tokenize_tokens(body).unwrap(), vec!["foo", "`", "bar"]);
    assert_eq!(comment, Some("// quux"));

    let (body, comment) = split_comment("foo `//` bar // quux `x");
    assert_eq!(tokenize_tokens(body).unwrap(), vec!["foo", "//", "bar"]);
    assert_eq!(comment, Some("// quux `x"));
    assert_eq!(comment.map(comment_text), Some("quux `x"));

    // No comment at all, and a `//` inside a token is not one either.
    assert_eq!(split_comment("foo bar"), ("foo bar", None));
    assert_eq!(split_comment("http://x y"), ("http://x y", None));

    // The tokenizers drop the comment for callers that only want tokens.
    assert_eq!(
        tokenize_tokens("map A = a // hi").unwrap(),
        vec!["map", "A", "=", "a"]
    );
    let spans = tokenize_with_spans("map A = a // hi").unwrap();
    assert_eq!(spans.len(), 4);
}

/// Every directive form keeps its meaning with a comment attached, and the
/// comment survives a parse/serialize round trip.
#[test]
fn comments_on_directives_round_trip() {
    let input = "\
meta height 16 // metrics
map A = latin-a // the letter
map generate U+00C0 // decomposed
name-parts $x = a b // parts
remap liga : a b -> ab // ligature
feature liga for latn : liga // feature
color red = #ff0000 // brand
exclude-from-sample foo // not interesting
assume unused bar // deliberate
glyph a-b 2 1 // header
@@..
ref other 1 2 // layer
anchor top 0 0 // where marks go
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();

    assert!(
        matches!(&doc.items[1], DocumentItem::Map { char_repr, glyph, comment, .. }
            if char_repr == "A" && glyph == "latin-a" && comment.as_deref() == Some("the letter")),
        "got {:?}",
        doc.items[1],
    );
    assert!(
        matches!(&doc.items[2], DocumentItem::MapDecomposed { char_repr, .. }
            if char_repr == "U+00C0"),
        "got {:?}",
        doc.items[2],
    );
    assert!(
        matches!(&doc.items[3], DocumentItem::NameParts { values, comment, .. }
            if values == &["a".to_string(), "b".to_string()]
                && comment.as_deref() == Some("parts")),
        "got {:?}",
        doc.items[3],
    );
    assert!(
        matches!(&doc.items[4], DocumentItem::Remap { source, target, comment, .. }
            if source == &["a".to_string(), "b".to_string()]
                && target == &["ab".to_string()]
                && comment.as_deref() == Some("ligature")),
        "got {:?}",
        doc.items[4],
    );
    assert!(
        matches!(&doc.items[5], DocumentItem::Feature { remap_group, comment, .. }
            if remap_group == "liga" && comment.as_deref() == Some("feature")),
        "got {:?}",
        doc.items[5],
    );
    assert!(
        matches!(&doc.items[6], DocumentItem::Color { value, comment, .. }
            if value == "#ff0000" && comment.as_deref() == Some("brand")),
        "got {:?}",
        doc.items[6],
    );
    let DocumentItem::Glyph { body, .. } = &doc.items[9] else {
        panic!("expected Glyph, got {:?}", doc.items[9]);
    };
    assert_eq!(body.comment.as_deref(), Some("header"));
    assert!(body.pixels.is_some(), "the pixel row must still be a grid");
    assert_eq!(body.refs[0].comment.as_deref(), Some("layer"));
    assert_eq!(body.points[0].comment.as_deref(), Some("where marks go"));

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A comment must not leak into the arguments of the raw-text directives.
#[test]
fn comment_is_not_a_directive_argument() {
    let input = "exclude-from-sample foo // not interesting\nassume unused bar // deliberate\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Directive(a) = &doc.items[0] else {
        panic!()
    };
    let DocumentItem::Directive(b) = &doc.items[1] else {
        panic!()
    };
    assert_eq!(
        crate::document::classify_directive(a),
        crate::document::Directive::ExcludeFromSample("foo"),
    );
    assert_eq!(
        crate::document::classify_directive(b),
        crate::document::Directive::AssumeUnused("bar"),
    );
}

/// Text appended to a commented line has to land *before* the comment,
/// or the comment swallows it.
#[test]
fn appending_to_a_line_keeps_the_comment_last() {
    assert_eq!(append_to_line("glyph foo", "4 2"), "glyph foo 4 2");
    assert_eq!(
        append_to_line("glyph foo // a note", "4 2"),
        "glyph foo 4 2 // a note",
    );
    let appended = append_to_line("glyph foo // a note", "4 2");
    assert_eq!(
        glyph_header_dims(&tokenize_tokens(&appended).unwrap()[1..]),
        Some(GlyphHeaderDims {
            width: 4,
            height: 2,
            scale: 1
        }),
    );
}

/// Pixel rows are the one place `//` stays a pixel pair.
#[test]
fn pixel_rows_are_never_comments() {
    let input = "glyph slash 2 1\n0//1\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!()
    };
    let grid = body.pixels.as_ref().unwrap();
    assert_eq!(grid.get(0, 0), chars_to_shape('0', '/').unwrap());
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn scale_roundtrip() {
    let input = "\
glyph flag 10 5 scale 2
....................@@@@@@@@@@..........@@@@@@@@@@
....................@@@@@@@@@@..........@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!()
    };
    assert_eq!(body.scale, 2);
    let grid = body.pixels.as_ref().unwrap();
    assert_eq!((grid.width, grid.height), (20, 10));

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, input);
}

#[test]
fn scale_header_dims() {
    let dims = glyph_header_dims(&["foo", "10", "5", "scale", "2"]);
    assert_eq!(
        dims,
        Some(GlyphHeaderDims {
            width: 20,
            height: 10,
            scale: 2
        })
    );

    let dims = glyph_header_dims(&["foo", "scale", "3", "4", "2"]);
    assert_eq!(
        dims,
        Some(GlyphHeaderDims {
            width: 12,
            height: 6,
            scale: 3
        })
    );

    let dims = glyph_header_dims(&["foo", "4", "2"]);
    assert_eq!(
        dims,
        Some(GlyphHeaderDims {
            width: 4,
            height: 2,
            scale: 1
        })
    );
}

/// `write_and_sync` stages every save as `.~name.unf`, a name that ends in
/// `.unf` like any other. A directory read that catches a save in flight must
/// not take it for a second copy of the document being saved — which is what
/// the font builder, the sidebar list and the file watcher all rely on.
#[test]
fn the_save_staging_file_is_not_a_source_file() {
    use std::path::Path;
    assert!(is_source_file(Path::new("/f/num.unf")));
    assert!(!is_source_file(Path::new("/f/.~num.unf")));
    assert!(!is_source_file(Path::new("/f/num.ttf")));
    assert!(!is_source_file(Path::new("/f/README")));
}

#[test]
fn remap_group_declaration_round_trips() {
    let input = "\
remap group plain
remap group ordered reversed after first after second // ordering
remap ordered : a -> b
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();

    assert!(
        matches!(&doc.items[0], DocumentItem::RemapGroup { name, reversed, after, comment }
            if name == "plain" && !*reversed && after.is_empty() && comment.is_none()),
        "got {:?}",
        doc.items[0],
    );
    assert!(
        matches!(&doc.items[1], DocumentItem::RemapGroup { name, reversed, after, comment }
            if name == "ordered"
                && *reversed
                && after == &["first".to_string(), "second".to_string()]
                && comment.as_deref() == Some("ordering")),
        "got {:?}",
        doc.items[1],
    );
    // A rule is still a rule; the colon is what separates the two forms.
    assert!(
        matches!(&doc.items[2], DocumentItem::Remap { .. }),
        "got {:?}",
        doc.items[2]
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A group may be named `group`; its rules keep the ordinary spelling because
/// a rule always puts a colon right after the group name.
#[test]
fn a_group_named_group_is_not_a_declaration() {
    let doc = parse_document_from_str(
        "remap group : a -> b\nremap group: c -> d\n",
        "test.unf".into(),
    )
    .unwrap();
    for item in &doc.items {
        assert!(
            matches!(item, DocumentItem::Remap { feature, .. } if feature == "group"),
            "got {item:?}",
        );
    }
}

/// Flags are checked rather than skipped: a half-understood declaration would
/// drop an ordering constraint and only show up as a mis-shaped glyph.
#[test]
fn malformed_remap_group_declarations_stay_unrecognized() {
    for line in [
        "remap group",                         // no name
        "remap group foo bogus",               // unknown flag
        "remap group foo after",               // `after` with no operand
        "remap group foo reversed reversed",   // repeated flag
        "remap group foo after bar after bar", // repeated edge
        "remap group after bar",               // name is a keyword
    ] {
        let doc = parse_document_from_str(&format!("{line}\n"), "test.unf".into()).unwrap();
        assert!(
            matches!(&doc.items[0], DocumentItem::Directive(_)),
            "{line:?} should not parse, got {:?}",
            doc.items[0],
        );
    }
}

/// The two `prop` forms survive a round trip, comment included. The property
/// keywords come back in the brace-group order whatever order they were
/// written in, which is the one canonicalization this line has.
#[test]
fn roundtrip_prop_directives() {
    let input = "\
prop block `Unison Symbols` = U+F0000..F00FF
prop U+F0000 = `UNISON LOGO` gc So eaw W // the mark
prop U+F0010..F001F eaw W gc So ccc 230
prop 한 = `HANGUL SYLLABLE HAN`
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 4);

    let DocumentItem::PropBlock {
        name, start, end, ..
    } = &doc.items[0]
    else {
        panic!("expected PropBlock, got {:?}", doc.items[0]);
    };
    assert_eq!(
        (name.as_str(), *start, *end),
        ("Unison Symbols", 0xF0000, 0xF00FF)
    );

    let DocumentItem::PropChar {
        char_repr,
        name,
        values,
        comment,
    } = &doc.items[1]
    else {
        panic!("expected PropChar, got {:?}", doc.items[1]);
    };
    assert_eq!(char_repr, "U+F0000");
    assert_eq!(name.as_deref(), Some("UNISON LOGO"));
    assert_eq!(values.gc.as_deref(), Some("So"));
    assert_eq!(values.eaw.as_deref(), Some("W"));
    assert_eq!(values.ccc, None);
    assert_eq!(comment.as_deref(), Some("the mark"));

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "\
prop block `Unison Symbols` = U+F0000..F00FF
prop U+F0000 = `UNISON LOGO` gc So eaw W // the mark
prop U+F0010..F001F gc So ccc 230 eaw W
prop 한 = `HANGUL SYLLABLE HAN`
",
    );
}

/// A `prop` line that states nothing, names an unknown keyword or carries a
/// non-numeric `ccc` is kept verbatim rather than half-read — the same
/// treatment every other malformed directive gets.
#[test]
fn malformed_prop_lines_stay_raw_text() {
    for line in [
        "prop U+F0000",
        "prop U+F0000 = X bidi L",
        "prop U+F0000 ccc high",
        "prop U+F0000 gc",
        "prop block X = notarange",
        "prop block X U+F0000",
    ] {
        let doc = parse_document_from_str(&format!("{line}\n"), "test.unf".into()).unwrap();
        assert!(
            matches!(&doc.items[0], DocumentItem::Directive(t) if t == line),
            "expected `{line}` to stay raw, got {:?}",
            doc.items[0],
        );
    }
}

// ---------------------------------------------------------------------------
// `@` names
// ---------------------------------------------------------------------------

/// `@` stands for the last glyph name declared without one, on a header and on
/// a `ref` alike — and a helper glyph does *not* become the base itself, so a
/// chain of them all hangs off the same glyph.
#[test]
fn at_expands_to_the_last_plain_glyph_name() {
    let src = "\
glyph foo
ref @-bar
glyph @-bar
ref @-baz
glyph @-baz
ref plain
";
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let names: Vec<String> = doc
        .items
        .iter()
        .filter_map(|i| match i {
            DocumentItem::Glyph { name, .. } => Some(name.display()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["foo", "foo-bar", "foo-baz"]);

    let refs: Vec<String> = doc
        .items
        .iter()
        .filter_map(|i| match i {
            DocumentItem::Glyph { body, .. } => Some(body.refs.iter().map(|r| r.name.clone())),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(refs, vec!["foo-bar", "foo-baz", "plain"]);
}

/// The base is the declared name without its `:variant` suffix, so a variant's
/// helpers hang off the glyph and not off the variant — which is what lets the
/// mono variant of a helper be written under the mono variant of its base.
#[test]
fn a_variant_suffix_is_not_part_of_the_base() {
    let src = "\
glyph foo:mono
ref @-bar:mono
glyph @-bar:mono
ref @-baz
";
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let names: Vec<String> = doc
        .items
        .iter()
        .filter_map(|i| match i {
            DocumentItem::Glyph { name, .. } => Some(name.display()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["foo:mono", "foo-bar:mono"]);

    let refs: Vec<String> = doc
        .items
        .iter()
        .filter_map(|i| match i {
            DocumentItem::Glyph { body, .. } => Some(body.refs.iter().map(|r| r.name.clone())),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(refs, vec!["foo-bar:mono", "foo-baz"]);
}

/// A variant suffix is what `@:mono` is for, and a full name always still
/// works — `@` is a shorthand, not a mode.
#[test]
fn at_takes_any_suffix_and_full_names_still_work() {
    let src = "\
glyph foo
ref @:mono
ref foo-bar
glyph @:mono
";
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected a glyph");
    };
    assert_eq!(
        body.refs
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["foo:mono", "foo-bar"],
    );
    let DocumentItem::Glyph { name, .. } = &doc.items[1] else {
        panic!("expected a glyph");
    };
    assert_eq!(name.display(), "foo:mono");
}

/// `@` is expanded before name patterns are, so a patterned base carries into
/// every name written against it.
#[test]
fn at_carries_a_patterned_base_through() {
    let src = "\
glyph a($1..3)
ref @-b
";
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected a glyph");
    };
    assert_eq!(body.refs[0].name, "a($1..3)-b");
}

/// An alias names a glyph on both sides, so both take `@`.
#[test]
fn at_expands_on_both_sides_of_an_alias() {
    let src = "\
glyph foo
glyph @-bar = @-baz
";
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let DocumentItem::GlyphAlias { name, target, .. } = &doc.items[1] else {
        panic!("expected an alias");
    };
    assert_eq!(
        (name.display().as_str(), target.as_str()),
        ("foo-bar", "foo-baz")
    );
}

/// Serializing puts back what was written: `@` is source syntax, and the
/// editor canonicalizes every file it opens through this path.
#[test]
fn serializing_keeps_the_at_form() {
    let src = "\
glyph foo
ref @-bar // helper
glyph @-bar
glyph @-alias = @-bar
";
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), src);
}

/// With no glyph declared yet there is nothing for `@` to stand for, so the
/// written form survives into the name — where `issues.rs` reports it rather
/// than the parser silently inventing a base.
#[test]
fn an_at_with_no_base_keeps_its_at() {
    let doc = parse_document_from_str("glyph @-bar\nref @-baz\n", "test.unf".into()).unwrap();
    let DocumentItem::Glyph { name, body } = &doc.items[0] else {
        panic!("expected a glyph");
    };
    assert_eq!(name.display(), "@-bar");
    assert_eq!(body.refs[0].name, "@-baz");
    assert!(!crate::document::is_valid_glyph_name(&name.display()));
}

/// The two halves carry their spellings independently, so a line may mix them.
/// Concatenating on the way out is only safe when *both* are literal: with a
/// `U+XXXX` base and a literal selector it would glue them into a single
/// seven-character token that re-parses as something else entirely.
#[test]
fn a_mixed_spelling_variation_sequence_round_trips() {
    let input = "map 0 U+FE0F = x\nmap U+0030 \u{FE0F} = y\n";
    let doc = parse_document_from_str(input, "t.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[1], DocumentItem::Map { char_repr, selector, .. }
            if char_repr == "U+0030" && selector.as_deref() == Some("\u{FE0F}")),
        "got {:?}",
        doc.items[1],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A heading is its own item, its `#` run has to be a token, and everything
/// after it is prose that survives a round-trip verbatim.
#[test]
fn headings_parse_at_three_levels_and_round_trip() {
    let input = "\
# a title\n\
## a section\n\
### a subsection\n\
#\n\
#### too deep\n\
###not a heading\n\
# `backticks` and // not a comment marker to the parser\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let levels: Vec<Option<u8>> = doc
        .items
        .iter()
        .map(|item| match item {
            DocumentItem::Heading { level, .. } => Some(*level),
            _ => None,
        })
        .collect();
    assert_eq!(
        levels,
        vec![Some(1), Some(2), Some(3), Some(1), Some(4), None, Some(1)],
        "`####` still parses (so `issues` can report it) but `###foo` does not"
    );
    assert!(
        matches!(&doc.items[5], DocumentItem::Directive(t) if t == "###not a heading"),
        "a `#` run that is not a token of its own is not a heading: {:?}",
        doc.items[5]
    );
    assert!(
        matches!(&doc.items[3], DocumentItem::Heading { text, .. } if text.is_empty()),
        "a bare `#` is a heading with no text"
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A heading builds nothing: the font is what it would be with the line gone.
/// `affects_font` is what the editor rebuilds on, hence the gate.
#[cfg(feature = "editor")]
#[test]
fn a_heading_does_not_affect_the_font() {
    let doc = parse_document_from_str("# title\n", "test.unf".into()).unwrap();
    assert!(!doc.items[0].affects_font());
}

/// A zero-width glyph has no pixel row to read: every row would be the empty
/// string, so a grid of one would swallow the blank lines (and, in the strict
/// parser, fail on whatever non-blank line came within `height` lines of the
/// header). Both parsers must leave the following lines alone.
#[test]
fn zero_width_glyph_reads_no_grid() {
    let input = "\
glyph foo 0 16

glyph bar 2 1
..@@
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let names: Vec<&str> = doc
        .items
        .iter()
        .filter_map(|item| match item {
            DocumentItem::Glyph { name, .. } => Some(name.0.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["foo", "bar"], "got {:?}", doc.items);

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// The lenient parser must not consume the blank lines after a zero-width
/// header either — the editor and the build have to agree on where the glyph
/// block ends.
#[cfg(feature = "editor")]
#[test]
fn zero_width_glyph_reads_no_grid_lenient() {
    let lines = parse_doclines("glyph foo 0 16\n\n\nref bar\n");
    let texts: Vec<&str> = lines
        .iter()
        .filter_map(|l| match l {
            DocLine::Text(t) => Some(t.as_str()),
            DocLine::Grid(_) => None,
        })
        .collect();
    assert_eq!(texts, vec!["glyph foo 0 16", "", "", "ref bar"]);
}
