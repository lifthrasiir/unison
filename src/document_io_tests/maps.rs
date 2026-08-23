//! `map` in its several forms — variation sequences, `generate`, quoted
//! names — and the `name-parts` line beside it.

use super::*;

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
        matches!(&doc.items[0], DocumentItem::Map { char_repr, selector, glyphs, .. }
            if char_repr == "U+0030" && selector.as_deref() == Some("U+FE0F")
                && glyphs == &["num-zero-emoji"]),
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
        matches!(&doc.items[2], DocumentItem::Map { char_repr, glyphs, .. }
            if char_repr == "generate" && glyphs == &["g"]),
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
        char_repr, glyphs, ..
    } = &doc.items[0]
    {
        assert_eq!(char_repr, "`");
        assert_eq!(glyphs, &["bquot"]);
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

/// `map CHAR = A B C` — the targets are *ordered alternatives*, kept as
/// written; which one a character gets is decided by the build.
#[test]
fn parse_map_with_several_targets() {
    let input = "\
map A = first second third
map U+0030 U+FE0F = fancy-zero zero
map wide : B = w-first w-second
";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { char_repr, selector, glyphs, .. }
            if char_repr == "A" && selector.is_none()
                && glyphs == &["first", "second", "third"]),
        "got {:?}",
        doc.items[0],
    );
    assert!(
        matches!(&doc.items[1], DocumentItem::Map { char_repr, selector, glyphs, .. }
            if char_repr == "U+0030" && selector.as_deref() == Some("U+FE0F")
                && glyphs == &["fancy-zero", "zero"]),
        "got {:?}",
        doc.items[1],
    );
    assert!(
        matches!(&doc.items[2], DocumentItem::Map { slices, glyphs, .. }
            if slices == &["wide"] && glyphs == &["w-first", "w-second"]),
        "got {:?}",
        doc.items[2],
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// `generate` keeps its own arities: the one it shares with the alternatives
/// form stays a decomposition rather than becoming a variation sequence.
#[test]
fn map_generate_is_not_read_as_alternatives() {
    let doc =
        parse_document_from_str("map generate U+00C1 = a-acute\n", "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::MapDecomposed { char_repr, glyph, .. }
            if char_repr == "U+00C1" && glyph.as_deref() == Some("a-acute")),
        "got {:?}",
        doc.items[0],
    );
}

/// An empty token — two backquotes — is a target that says "map nothing". It
/// survives tokenizing, parsing and serializing as an empty string.
#[test]
fn parse_map_with_an_empty_trailing_target() {
    let input = "map A = first ``\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        matches!(&doc.items[0], DocumentItem::Map { char_repr, glyphs, .. }
            if char_repr == "A" && glyphs == &["first", ""]),
        "got {:?}",
        doc.items[0],
    );
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}
