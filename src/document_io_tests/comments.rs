//! Trailing `// …` comments: which lines carry one, and where it stays.

use super::*;

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
