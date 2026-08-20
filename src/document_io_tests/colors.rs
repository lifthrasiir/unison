//! `color`, `ref … fill` and layer visibility.

use super::*;

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
