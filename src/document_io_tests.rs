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
    assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 3, scale: 1 }));

    let dims = glyph_header_dims(&["foo", "left", "2", "3"]);
    assert_eq!(dims, None, "width 3 without height is not a grid header");

    let dims = glyph_header_dims(&["foo", "4", "3", "advance", "0"]);
    assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 3, scale: 1 }));

    let dims = glyph_header_dims(&["foo", "sticky", "4", "3"]);
    assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 3, scale: 1 }));

    // Cross-check against derive_document on the same headers.
    for (header, expected) in [
        ("glyph foo advance 0 4 3", Some((4u16, 3u16))),
        ("glyph foo left 2 3", None),
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
        let dims = glyph_header_dims(&tokens[1..])
            .map(|d| (d.width, d.height));
        assert_eq!(dims, expected, "glyph_header_dims mismatch for {header:?}");
    }
}

#[test]
fn roundtrip_simple() {
    let input = "\
// test comment
font-meta height 16 ascent 14 descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph U+0041
ref test-glyph 2 0

exclude-from-sample U+AD00
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    // comment, font-meta, blank, glyph, blank, glyph, blank, directive = 8
    assert_eq!(doc.items.len(), 8);

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
fn legacy_point_parsed_as_single_cell_anchor() {
    let input = "glyph foo 1 1\n@@\npoint +bar 3 5\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert_eq!(body.points.len(), 1);
        let p = &body.points[0];
        assert_eq!(p.position, "+bar");
        assert_eq!((p.col, p.col_end), (3, 3));
        assert_eq!((p.row, p.row_end), (5, 5));
        assert!(p.is_single_cell());
    } else {
        panic!("expected glyph");
    }
}

#[test]
fn parse_glyph_with_all_shapes() {
    let input = "glyph shapes 4 1\n..@@1\\1>\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        let grid = body.pixels.as_ref().expect("expected glyph with pixels");
        assert!(grid.get(0, 0).is_empty());
        assert_eq!(grid.get(0, 1).shape_id(), crate::pixel::PX_ALMOSTFULL);
        assert!(grid.get(0, 1).is_filled());
        assert_eq!(grid.get(0, 2).shape_id(), crate::pixel::PX_HALF1);
        assert!(grid.get(0, 2).is_filled());
        assert_eq!(grid.get(0, 3).shape_id(), crate::pixel::PX_QUAD1);
        assert!(grid.get(0, 3).is_filled());
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
    let input = "glyph U+0041 = test-glyph\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { name, body } = &doc.items[0] {
        assert_eq!(name.display(), "U+0041");
        assert!(body.pixels.is_none());
        assert_eq!(body.refs.len(), 1);
        assert_eq!(body.refs[0].name, "test-glyph");
        assert_eq!(body.refs[0].row(), 0);
        assert_eq!(body.refs[0].col(), 0);
    } else {
        panic!("expected glyph");
    }

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, "glyph U+0041 = test-glyph\n");
}

#[test]
fn explicit_zero_ref_roundtrips_as_explicit() {
    let input = "glyph composite\nref target 0 0\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected glyph");
    };
    assert_eq!(body.refs[0].offset, Some((0, 0)));
    assert!(!body.is_simple_alias());

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
font-meta height 16 ascent 14 descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph U+0041
ref test-glyph 2 0

exclude-from-sample U+AD00
";
    docline_roundtrip(input);

    let lines = parse_doclines(input);
    // comment, font-meta, blank, glyph-header, Grid, blank, glyph-header, ref, blank, directive
    assert_eq!(lines.len(), 10);
    assert!(matches!(lines[0], DocLine::Text(ref s) if s.starts_with("//")));
    assert!(matches!(lines[3], DocLine::Text(ref s) if s.starts_with("glyph test-glyph")));
    assert!(matches!(lines[4], DocLine::Grid(_)));
    assert!(matches!(lines[6], DocLine::Text(ref s) if s.starts_with("glyph U+0041")));
    assert!(matches!(lines[7], DocLine::Text(ref s) if s.starts_with("ref ")));
}

#[test]
fn docline_roundtrip_alias() {
    docline_roundtrip("glyph U+0041 = test-glyph\n");
    let lines = parse_doclines("glyph U+0041 = test-glyph\n");
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

    for (idx, (old_item, new_item)) in
        old_doc.items.iter().zip(new_doc.items.iter()).enumerate()
    {
        match (old_item, new_item) {
            (DocumentItem::BlankLine, DocumentItem::BlankLine) => {}
            (DocumentItem::Comment(a), DocumentItem::Comment(b)) => {
                assert_eq!(a, b, "comment mismatch at item {idx}");
            }
            (DocumentItem::FontMeta(a), DocumentItem::FontMeta(b)) => {
                assert_eq!(a, b, "font-meta mismatch at item {idx}");
            }
            (DocumentItem::Directive(a), DocumentItem::Directive(b)) => {
                assert_eq!(a, b, "directive mismatch at item {idx}");
            }
            (
                DocumentItem::NameParts { name: n1, values: v1, .. },
                DocumentItem::NameParts { name: n2, values: v2, .. },
            ) => {
                assert_eq!(n1, n2, "name-parts name mismatch at item {idx}");
                assert_eq!(v1, v2, "name-parts values mismatch at item {idx}");
            }
            (
                DocumentItem::Remap { feature: f1, lookbehind: lb1, source: s1, target: t1, lookahead: la1, .. },
                DocumentItem::Remap { feature: f2, lookbehind: lb2, source: s2, target: t2, lookahead: la2, .. },
            ) => {
                assert_eq!(f1, f2, "remap feature mismatch at item {idx}");
                assert_eq!(lb1, lb2, "remap lookbehind mismatch at item {idx}");
                assert_eq!(s1, s2, "remap source mismatch at item {idx}");
                assert_eq!(t1, t2, "remap target mismatch at item {idx}");
                assert_eq!(la1, la2, "remap lookahead mismatch at item {idx}");
            }
            (
                DocumentItem::Feature { name: n1, scripts: s1, remap_group: r1, .. },
                DocumentItem::Feature { name: n2, scripts: s2, remap_group: r2, .. },
            ) => {
                assert_eq!(n1, n2, "feature name mismatch at item {idx}");
                assert_eq!(s1, s2, "feature scripts mismatch at item {idx}");
                assert_eq!(r1, r2, "feature remap_group mismatch at item {idx}");
            }
            (
                DocumentItem::Glyph {
                    name: n1,
                    body: b1,
                },
                DocumentItem::Glyph {
                    name: n2,
                    body: b2,
                },
            ) => {
                assert_eq!(
                    n1.display(),
                    n2.display(),
                    "name mismatch at item {idx}"
                );
                assert_eq!(
                    b1.pixels, b2.pixels,
                    "pixels mismatch at item {idx}"
                );
                assert_eq!(
                    b1.refs.len(),
                    b2.refs.len(),
                    "ref count mismatch at item {idx}"
                );
                for (ri, (r1, r2)) in b1.refs.iter().zip(b2.refs.iter()).enumerate() {
                    assert_eq!(r1.name, r2.name, "ref name mismatch at item {idx} ref {ri}");
                    assert_eq!(r1.offset, r2.offset, "ref offset mismatch at item {idx} ref {ri}");
                    assert_eq!(r1.negated, r2.negated, "ref negation mismatch at item {idx} ref {ri}");
                }
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
font-meta height 16 ascent 14 descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph U+0041
ref test-glyph 2 0

exclude-from-sample U+AD00
",
    );
}

#[test]
fn derive_equivalent_alias() {
    assert_derive_equivalent("glyph U+0041 = test-glyph\n");
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
font-meta height 16 ascent 12 descent 4

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
point -join 0 0
point +join 2 0

glyph batch
ref stem-(a|b)

glyph sticky-empty sticky advance 0

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
        "glyph foo sticky\n",
        "glyph foo 2 1 sticky\n..@@\n",
        "glyph foo advance 5\n",
        "glyph foo left -1\n",
        "glyph foo 2 1 sticky advance 5 left -1\n..@@\n",
        "glyph foo = bar\n",
        "glyph foo sticky = bar\n",
    ] {
        assert!(
            parse_document_from_str(input, "test.unf".into()).is_ok(),
            "should accept: {input:?}"
        );
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
    assert_eq!(
        tokenize_tokens("`` `a` ````").unwrap(),
        vec!["", "a", "`"],
    );
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

#[test]
fn parse_map_generate_forms() {
    let input = "\
map generate À // plain
map generate Á = a-acute // named
map generate = g
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::MapDecomposed { char_repr, glyph, comment }
            if char_repr == "À" && glyph.is_none() && comment.as_deref() == Some("plain")),
        "got {:?}", doc.items[0],
    );
    assert!(
        matches!(&doc.items[1], DocumentItem::MapDecomposed { char_repr, glyph, comment }
            if char_repr == "Á" && glyph.as_deref() == Some("a-acute")
                && comment.as_deref() == Some("named")),
        "got {:?}", doc.items[1],
    );
    // `generate` in the plain form's own arity stays a plain `map`, so a glyph
    // that happens to be called `generate` is still reachable.
    assert!(
        matches!(&doc.items[2], DocumentItem::Map { char_repr, glyph, .. }
            if char_repr == "generate" && glyph == "g"),
        "got {:?}", doc.items[2],
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
    if let DocumentItem::Map { char_repr, glyph, .. } = &doc.items[0] {
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
    if let DocumentItem::Color { name, value, visibility, .. } = &doc.items[0] {
        assert_eq!(name, "red");
        assert_eq!(value, "#ff0000");
        assert!(visibility.is_none());
    } else {
        panic!("expected Color");
    }
    if let DocumentItem::Color { name, value, visibility, .. } = &doc.items[1] {
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
    assert!(output_str.contains("ref auto-inherit inherit\n"), "{output_str}");
    assert!(output_str.contains("ref offset-inherit 1 -2 inherit\n"), "{output_str}");
    assert!(output_str.contains("ref full 1 2 negated inherit fill #00ff00 coloronly\n"), "{output_str}");
}

#[test]
fn parse_assert_shape_basic() {
    let input = "assert shape `AB` : a-upper : b-upper\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::AssertShape { text, features, expected, comment, .. } = &doc.items[0] {
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
    if let DocumentItem::AssertShape { language, features, expected, .. } = &doc.items[0] {
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
    if let DocumentItem::AssertShape { text, features, expected, .. } = &doc.items[0] {
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
    assert_eq!(output_str, "assert shape AB +liga : a-upper advance 512 : b-upper offset 10 20\n");
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
    assert_eq!(tokenize_tokens("map A = a // hi").unwrap(), vec!["map", "A", "=", "a"]);
    let spans = tokenize_with_spans("map A = a // hi").unwrap();
    assert_eq!(spans.len(), 4);
}

/// Every directive form keeps its meaning with a comment attached, and the
/// comment survives a parse/serialize round trip.
#[test]
fn comments_on_directives_round_trip() {
    let input = "\
font-meta height 16 ascent 12 descent 4 // metrics
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
        matches!(&doc.items[1], DocumentItem::Map { char_repr, glyph, comment }
            if char_repr == "A" && glyph == "latin-a" && comment.as_deref() == Some("the letter")),
        "got {:?}", doc.items[1],
    );
    assert!(
        matches!(&doc.items[2], DocumentItem::MapDecomposed { char_repr, .. }
            if char_repr == "U+00C0"),
        "got {:?}", doc.items[2],
    );
    assert!(
        matches!(&doc.items[3], DocumentItem::NameParts { values, comment, .. }
            if values == &["a".to_string(), "b".to_string()]
                && comment.as_deref() == Some("parts")),
        "got {:?}", doc.items[3],
    );
    assert!(
        matches!(&doc.items[4], DocumentItem::Remap { source, target, comment, .. }
            if source == &["a".to_string(), "b".to_string()]
                && target == &["ab".to_string()]
                && comment.as_deref() == Some("ligature")),
        "got {:?}", doc.items[4],
    );
    assert!(
        matches!(&doc.items[5], DocumentItem::Feature { remap_group, comment, .. }
            if remap_group == "liga" && comment.as_deref() == Some("feature")),
        "got {:?}", doc.items[5],
    );
    assert!(
        matches!(&doc.items[6], DocumentItem::Color { value, comment, .. }
            if value == "#ff0000" && comment.as_deref() == Some("brand")),
        "got {:?}", doc.items[6],
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
    let DocumentItem::Directive(a) = &doc.items[0] else { panic!() };
    let DocumentItem::Directive(b) = &doc.items[1] else { panic!() };
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
        Some(GlyphHeaderDims { width: 4, height: 2, scale: 1 }),
    );
}

/// Pixel rows are the one place `//` stays a pixel pair.
#[test]
fn pixel_rows_are_never_comments() {
    let input = "glyph slash 2 1\n0//1\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else { panic!() };
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
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else { panic!() };
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
    assert_eq!(dims, Some(GlyphHeaderDims { width: 20, height: 10, scale: 2 }));

    let dims = glyph_header_dims(&["foo", "scale", "3", "4", "2"]);
    assert_eq!(dims, Some(GlyphHeaderDims { width: 12, height: 6, scale: 3 }));

    let dims = glyph_header_dims(&["foo", "4", "2"]);
    assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 2, scale: 1 }));
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
