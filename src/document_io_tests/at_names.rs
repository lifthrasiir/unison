//! `@` names, headings, and the glyph that declares no grid at all.

use super::*;

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
