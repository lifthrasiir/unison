//! The backtick-quoting tokenizer.

use super::*;

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
