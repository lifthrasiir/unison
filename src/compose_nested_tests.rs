//! Tests for a nested split (`1|foo|1|bar|1`) in an IDC line: the parse, and the
//! glyph it stands for seen end to end, through the expansion and the
//! resolution the font is built from.

use crate::document::{ComposeItem, DocumentItem};
use crate::document_io::parse_document_from_str;
use crate::issues::Severity;
use crate::render::ttf_builder::Expansion;

/// Three filled parts, the two stacked ones of which are 4 wide and 3 and 2
/// tall, and the part beside them 4 by 8.
const PARTS: &str = "\
glyph foo:4x3 4 3
@@@@@@@@
@@@@@@@@
@@@@@@@@
glyph bar:4x2 4 2
@@@@@@@@
@@@@@@@@
glyph baz:4x8 4 8
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
";

fn expand(src: &str) -> Expansion {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    crate::render::ttf_builder::expand_documents(&docs, &name_parts)
}

fn of(expansion: &Expansion, severity: Severity) -> Vec<&str> {
    expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .map(|d| d.message.as_str())
        .collect()
}

/// Every ref of `glyph`, with the offset the line derived for it.
fn placed<'a>(expansion: &'a Expansion, glyph: &str) -> Vec<(&'a str, (i16, i16))> {
    expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == glyph => Some(
                body.refs
                    .iter()
                    .map(|r| (r.name.as_str(), r.offset.unwrap_or_default())),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

fn compose_items(src: &str) -> Vec<ComposeItem> {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    doc.items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { body, .. } => body.compose.first().map(|c| c.items.clone()),
            _ => None,
        })
        .expect("an IDC line")
}

fn part(name: &str) -> ComposeItem {
    ComposeItem::Part {
        name: name.to_string(),
        raw_name: None,
    }
}

/// A `|` outside parentheses makes a token a nested split, read piece by piece as
/// the line is; one inside them is a pattern's, as it always was. The line
/// writes back as it was written.
#[test]
fn a_top_level_pipe_makes_a_nested_split_and_a_parenthesized_one_does_not() {
    let items = compose_items("glyph g 10 8\n\u{2FF0} 2 1|foo|-1|bar|1 (a|b):4x8\n");
    assert_eq!(
        items,
        vec![
            ComposeItem::Gap(2),
            ComposeItem::Nested(vec![
                ComposeItem::Gap(1),
                part("foo"),
                ComposeItem::Gap(-1),
                part("bar"),
                ComposeItem::Gap(1),
            ]),
            part("(a|b):4x8"),
        ]
    );
    // A nested split with no gaps is still one.
    assert_eq!(
        compose_items("glyph g 10 8\n\u{2FF0} foo|bar baz\n")[0],
        ComposeItem::Nested(vec![part("foo"), part("bar")])
    );

    let doc = parse_document_from_str(
        "glyph g 10 8\n\u{2FF0} 2 1|@x|1|bar|1 baz\n",
        "test.unf".into(),
    )
    .unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("a glyph");
    };
    assert_eq!(body.compose[0].format_line(), "\u{2FF0} 2 1|@x|1|bar|1 baz");
}

/// An empty piece is the one thing a nested split cannot be read with, and the line
/// is then unread, as a `ref` that does not parse is.
#[test]
fn a_group_with_an_empty_piece_is_not_read() {
    let doc = parse_document_from_str("glyph g 10 8\n\u{2FF0} 1|foo||bar baz\n", "test.unf".into())
        .unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("a glyph");
    };
    assert!(body.compose.is_empty());
}

/// The whole point: `⿰ 1 1|foo|1|bar|1 1 baz` is the glyph `⿱ 1 foo 1 bar 1`
/// standing in the slot. The parts land where that glyph's would, and the
/// glyph resolves to the same drawing, cell for cell.
#[test]
fn a_group_is_the_glyph_its_line_would_be() {
    let src = format!(
        "{PARTS}\
glyph g:10x8 10 8
\u{2FF0} 1 1|foo:4x3|1|bar:4x2|1 1 baz:4x8
glyph grp:4x8 4 8
\u{2FF1} 1 foo:4x3 1 bar:4x2 1
glyph h:10x8 10 8
\u{2FF0} 1 grp:4x8 1 baz:4x8
"
    );
    let expansion = expand(&src);
    let problems: Vec<&str> = [Severity::Error, Severity::Warning, Severity::Todo]
        .into_iter()
        .flat_map(|s| of(&expansion, s))
        .collect();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        placed(&expansion, "g:10x8"),
        vec![
            ("foo:4x3", (1, 1)),
            ("bar:4x2", (1, 5)),
            ("baz:4x8", (6, 0))
        ]
    );

    let doc = parse_document_from_str(&src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _) = crate::ref_composite::resolve_expansion(
        crate::render::ttf_builder::expand_documents(&docs, &name_parts),
        &name_parts,
        &crate::cancel::CancelToken::never(),
    );
    let (g, h) = (&resolved["g:10x8"], &resolved["h:10x8"]);
    assert_eq!(g.grid, h.grid);
    assert_eq!((g.origin_col, g.origin_row), (h.origin_col, h.origin_row));
}

/// Where a glyph and a nested split part ways. A glyph declares its box, so
/// `⿱ 1 foo 1 bar` in an 8-tall header passes however little of it the line
/// spells out; a nested split's box is what its members add up to, so the same
/// members leave the nested split 7 tall in an 8-tall slot, and that is an error.
#[test]
fn a_group_that_does_not_add_up_to_the_slot_is_an_error() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph grp:4x8 4 8
\u{2FF1} 1 foo:4x3 1 bar:4x2
glyph h:10x8 10 8
\u{2FF0} 1 grp:4x8 1 baz:4x8
glyph g:10x8 10 8
\u{2FF0} 1 1|foo:4x3|1|bar:4x2 1 baz:4x8
"
    ));
    assert_eq!(
        of(&expansion, Severity::Error),
        vec![
            "glyph 'g:10x8': `\u{2FF0}` nested split '1|foo:4x3|1|bar:4x2' adds up to 7 tall, not the glyph's 8"
        ]
    );
}

/// Across its own axis a nested split is as wide as its members, and they have to
/// agree about it exactly as the parts of a glyph's line agree with its
/// header. The first member is what the nested split is taken to be.
#[test]
fn the_members_of_a_nested_split_are_as_wide_as_each_other() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph wide:5x2 5 2
@@@@@@@@@@
@@@@@@@@@@
glyph g:11x8 11 8
\u{2FF0} 1 1|foo:4x3|1|wide:5x2|1 1 baz:4x8
"
    ));
    assert_eq!(
        of(&expansion, Severity::Error),
        vec![
            "glyph 'g:11x8': `\u{2FF0}` nested split '1|foo:4x3|1|wide:5x2|1': `\u{2FF1}` component \
             'wide:5x2' is wide 5, not the nested split's 4"
        ]
    );
}

/// A nested split is a ⿱ of one or two parts or a ⿳ of three inside a ⿰ (and
/// the other way round inside a ⿱). Three is laid out as ⿳ lays it out, and
/// four or more is an error.
#[test]
fn a_nested_split_takes_one_to_three_members() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g3:9x8 9 8
\u{2FF0} foo:4x3|bar:4x2|1|bar:4x2 1 baz:4x8
glyph g4:9x8 9 8
\u{2FF0} bar:4x2|bar:4x2|bar:4x2|bar:4x2 1 baz:4x8
"
    ));
    assert_eq!(
        placed(&expansion, "g3:9x8"),
        vec![
            ("foo:4x3", (0, 0)),
            ("bar:4x2", (0, 3)),
            ("bar:4x2", (0, 6)),
            ("baz:4x8", (5, 0))
        ]
    );
    assert_eq!(
        of(&expansion, Severity::Error),
        vec![
            "glyph 'g4:9x8': `\u{2FF0}` nested split 'bar:4x2|bar:4x2|bar:4x2|bar:4x2': \
             `\u{2FF1}` takes 1 to 3 components, not 4"
        ]
    );
}

/// One member is a part padded across the axis: `⿱ 1|foo|1 wide` sits `foo`
/// one cell in from each side of a slot two cells wider than it. The lone
/// member is at both ends of its axis at once and claims neither, so a name
/// drawn for either side sits there without a warning; and it is measured
/// like any other nested split, from outside only — the padding is what the
/// source wrote, and nothing would ever move it.
#[test]
fn a_nested_split_of_one_member_pads_it() {
    let src = format!(
        "{PARTS}\
glyph wide:6x2 6 2
@@@@@@@@@@@@
@@@@@@@@@@@@
glyph foo:4x3-r 4 3
@@@@@@@@
@@@@@@@@
@@@@@@@@
glyph v:6x6 6 6
\u{2FF1} 1|foo:4x3-r|1 1 wide:6x2
"
    );
    let expansion = expand(&src);
    let problems: Vec<&str> = [Severity::Error, Severity::Warning, Severity::Todo]
        .into_iter()
        .flat_map(|s| of(&expansion, s))
        .collect();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        placed(&expansion, "v:6x6"),
        vec![("foo:4x3-r", (1, 0)), ("wide:6x2", (0, 4))]
    );

    let expansion = expand(&format!("audit ideal-clearance v* 0 0\n{src}"));
    let chores = of(&expansion, Severity::Chore);
    assert!(
        chores
            .iter()
            .any(|m| m.contains("`\u{2FF1}` leaves 1 between '1|foo:4x3-r|1' and 'wide:6x2'")),
        "{chores:?}"
    );
    assert!(
        !chores.iter().any(|m| m.contains("nested split")),
        "{chores:?}"
    );
}

/// An enclosure hands out no share of an axis for a nested split to divide.
#[test]
fn an_enclosure_takes_no_nested_split() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph e:10x8 10 8
\u{2FF4} 1|foo:4x3|1|bar:4x2|1 baz:4x8 0 0
"
    ));
    let errors = of(&expansion, Severity::Error);
    assert!(
        errors.iter().any(
            |m| m.contains("writes the nested split '1|foo:4x3|1|bar:4x2|1', but only a split")
        ),
        "{errors:?}"
    );
}

/// The line around a nested split measures it by the ink of what it stands
/// for — the same numbers the glyph it would be gives — and its own line is not
/// measured: `uniform fix` does not go inside one, so a finding there would be
/// one nothing answers.
#[test]
fn a_nested_split_is_measured_from_outside_only() {
    let with_nested = expand(&format!(
        "audit ideal-clearance g* 0 0\n{PARTS}\
glyph g:10x8 10 8
\u{2FF0} 1 1|foo:4x3|1|bar:4x2|1 1 baz:4x8
"
    ));
    let chores = of(&with_nested, Severity::Chore);
    assert!(
        !chores.iter().any(|m| m.contains("nested split")),
        "{chores:?}"
    );
    assert!(
        chores.iter().any(
            |m| m.contains("`\u{2FF0}` leaves 1 between '1|foo:4x3|1|bar:4x2|1' and 'baz:4x8'")
        ),
        "{chores:?}"
    );

    let with_glyph = expand(&format!(
        "audit ideal-clearance g* 0 0\n{PARTS}\
glyph g:10x8 10 8
\u{2FF0} 1 grp:4x8 1 baz:4x8
glyph grp:4x8 4 8
\u{2FF1} 1 foo:4x3 1 bar:4x2 1
"
    ));
    let outer = |chores: Vec<&str>| -> Vec<String> {
        chores
            .into_iter()
            .filter(|m| m.starts_with("glyph 'g:10x8': `\u{2FF0}` leaves"))
            .map(|m| m.replace("1|foo:4x3|1|bar:4x2|1", "grp:4x8"))
            .collect()
    };
    assert_eq!(
        outer(chores.clone()),
        outer(of(&with_glyph, Severity::Chore))
    );
    assert_eq!(outer(chores).len(), 3);
}

/// An unpicked member is the TODO it is anywhere else, and it stands down the
/// clearance check of the line around the nested split as well as the nested split's own.
#[test]
fn an_undecided_member_is_a_todo_and_nothing_is_measured() {
    let expansion = expand(&format!(
        "audit ideal-clearance g* 0 0\n{PARTS}\
glyph g:10x8 10 8
\u{2FF0} 1 1|foo:4x3|1|bar|1 1 baz:4x8
"
    ));
    assert!(of(&expansion, Severity::Error).is_empty());
    assert!(of(&expansion, Severity::Chore).is_empty());
    assert_eq!(
        of(&expansion, Severity::Todo),
        vec![
            "glyph 'g:10x8': `\u{2FF0}` nested split '1|foo:4x3|1|bar|1': `\u{2FF1}` component 'bar' \
             has no variant picked yet; a component names the sized variant it wants, as in \
             `bar:WxH`"
        ]
    );
}

/// A pattern block expands a nested split's members in lock-step with its name, as
/// it does every other component.
#[test]
fn a_pattern_block_expands_the_members_of_a_nested_split() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph bar2:4x2 4 2
@@@@@@@@
@@@@@@@@
glyph g-(1|2):10x8 10 8
\u{2FF0} 1 1|foo:4x3|1|(bar|bar2):4x2|1 1 baz:4x8
"
    ));
    assert!(of(&expansion, Severity::Error).is_empty());
    assert_eq!(placed(&expansion, "g-1:10x8")[1], ("bar:4x2", (1, 5)));
    assert_eq!(placed(&expansion, "g-2:10x8")[1], ("bar2:4x2", (1, 5)));
}
