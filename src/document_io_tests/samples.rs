//! `sample` and the `||` continuation lines it takes.

use super::*;

fn sample_of(doc: &Document, idx: usize) -> (&str, Option<&str>, Vec<&str>, Vec<&str>) {
    match &doc.items[idx] {
        DocumentItem::Sample {
            label,
            sublabel,
            mode,
            text,
            ..
        } => (
            label.as_str(),
            sublabel.as_deref(),
            mode.iter().map(|m| m.as_str()).collect(),
            text.iter().map(|t| t.as_str()).collect(),
        ),
        other => panic!("expected Sample, got {other:?}"),
    }
}

#[test]
fn a_sample_takes_its_continuations_as_one_text() {
    let input = "sample Latin `English pangram`\n\
                 || The quick brown fox jumps over the lazy dog.\n\
                 || Mr Jock, TV quiz PhD, bags few lynx.\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(
        doc.items.len(),
        1,
        "the continuations are not items of their own"
    );
    assert_eq!(
        sample_of(&doc, 0),
        (
            "Latin",
            Some("English pangram"),
            vec![],
            vec![
                "The quick brown fox jumps over the lazy dog.",
                "Mr Jock, TV quiz PhD, bags few lynx.",
            ]
        )
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn a_sample_may_have_no_sublabel_and_a_reserved_mode() {
    let input = "sample Pangram : vertical\n|| Sphinx of black quartz.\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(
        sample_of(&doc, 0),
        (
            "Pangram",
            None,
            vec!["vertical"],
            vec!["Sphinx of black quartz."]
        )
    );

    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// Only the whitespace *every* line shares comes off, so a line indented past
/// its neighbours keeps the difference — and a blank line does not cap the
/// prefix at nothing.
#[test]
fn a_continuation_loses_only_the_shared_indent() {
    let input = "sample Verse\n||   one\n||\n||     two\n||   three\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(
        sample_of(&doc, 0).3,
        vec!["one", "", "  two", "three"],
        "the two-space prefix is punctuation; the extra two are content"
    );

    // Written back, the model's own text is what a re-parse gives again.
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    let written = String::from_utf8(output).unwrap();
    assert_eq!(written, "sample Verse\n|| one\n||\n||   two\n|| three\n");
    let again = parse_document_from_str(&written, "test.unf".into()).unwrap();
    assert_eq!(sample_of(&again, 0).3, sample_of(&doc, 0).3);
}

/// A continuation is prose: it is never tokenized, so a backtick, a `//` and a
/// `||` inside one are all just text. Before this, an unterminated backtick on
/// a sample line failed the whole file.
#[test]
fn a_continuation_is_raw_text() {
    let input = "sample Odd\n|| a ` backtick, a // slash and a || bar\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(
        sample_of(&doc, 0).3,
        vec!["a ` backtick, a // slash and a || bar"]
    );
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

#[test]
fn a_sample_header_keeps_its_comment() {
    let input = "sample Latin // why this one\n|| text\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let DocumentItem::Sample { comment, .. } = &doc.items[0] else {
        panic!("expected Sample");
    };
    assert_eq!(comment.as_deref(), Some("why this one"));
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A continuation with nothing above it to continue is its own line again, and
/// classifies as one so `issues` can name it.
#[test]
fn an_orphan_continuation_stays_a_line_of_its_own() {
    let input = "// nothing above\n|| stranded\nmeta family Foo\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 3);
    let DocumentItem::Directive(text) = &doc.items[1] else {
        panic!("expected Directive, got {:?}", doc.items[1]);
    };
    assert_eq!(
        crate::document::classify_directive(text),
        crate::document::Directive::OrphanContinuation
    );
    let mut output = Vec::new();
    serialize_document(&doc, &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), input);
}

/// A header the grammar cannot read claims nothing: it stays a line, and so do
/// the continuations under it, so both halves of the mistake get reported.
#[test]
fn a_malformed_sample_header_claims_no_continuations() {
    let input = "sample a b c\n|| text\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    assert!(matches!(doc.items[0], DocumentItem::Directive(_)));
    let DocumentItem::Directive(text) = &doc.items[1] else {
        panic!("expected Directive");
    };
    assert_eq!(
        crate::document::classify_directive(text),
        crate::document::Directive::OrphanContinuation
    );
}

/// A `sample` line with nothing under it parses — with an empty text, which is
/// what `issues` reports on. The parser does not fault a half-written line.
#[test]
fn a_sample_with_no_continuation_parses_empty() {
    let doc = parse_document_from_str("sample Latin\n", "test.unf".into()).unwrap();
    assert!(sample_of(&doc, 0).3.is_empty());
}

/// `sample` ends the glyph block above it like every other item keyword.
#[test]
fn a_sample_ends_the_block_above_it() {
    assert!(crate::document_io::starts_item("sample"));
    let input = "glyph a 1 1\n@@\nsample Latin\n|| text\n";
    let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    assert!(matches!(doc.items[0], DocumentItem::Glyph { .. }));
    assert!(matches!(doc.items[1], DocumentItem::Sample { .. }));
}
