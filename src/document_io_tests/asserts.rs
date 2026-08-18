//! The `assert` directives: `shape`, `same` and `distinct`.

use super::*;

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
