//! `scale`, the save-staging file, `remap group` declarations and `prop`.

use super::*;

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
