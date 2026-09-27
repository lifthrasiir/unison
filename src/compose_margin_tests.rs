//! Tests for a part's preferred margin (`margin-x L|R`, `margin-y T|B`): the
//! header that states it, and the padding an IDC line gives a part whose box
//! falls short of its slot across the axis by exactly that much.

use crate::document::DocumentItem;
use crate::document_io::parse_document_from_str;
use crate::issues::Severity;
use crate::render::ttf_builder::Expansion;

/// `mouth` is 4 wide and wants 2 cells either side, so it fills an 8-wide slot
/// of a `⿱`; `side` is 3 tall and wants 1 above and 2 below, so it fills a
/// 6-tall slot of a `⿰`. `wide` inks the two columns on its left in its top
/// row and nothing else there, which is what tells a part placed across the
/// axis from one that is not.
const PARTS: &str = "\
glyph mouth:4x2 4 2 margin-x 2
@@@@@@@@
@@@@@@@@
glyph wide:8x3 8 3
@@@@............
................
@@@@@@@@@@@@@@@@
glyph plank:10x3 10 3
@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@
glyph side:2x3 2 3 margin-y 1|2
@@@@
@@@@
@@@@
glyph tall:3x6 3 6
@@@@@@
@@@@@@
@@@@@@
@@@@@@
@@@@@@
@@@@@@
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

/// Everything short of a chore: what a line that lays out cleanly says nothing of.
fn problems(expansion: &Expansion) -> Vec<&str> {
    [Severity::Error, Severity::Warning, Severity::Todo]
        .into_iter()
        .flat_map(|s| of(expansion, s))
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

fn serialized(src: &str) -> String {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let mut out = Vec::new();
    crate::document_io::serialize_document(&doc, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

// ------------------------------------------------------------------ the header

/// One value is both sides; two are written with a `|` so that the flag always
/// takes one token, whatever stands after it. A margin equal on both sides
/// writes back as one value.
#[test]
fn a_margin_is_read_and_written_back() {
    for src in [
        "glyph a 2 1 margin-x 2|3 margin-y 1\n@@@@\n",
        "glyph a margin-y 0|4 2 1\n@@@@\n",
    ] {
        assert!(
            parse_document_from_str(src, "test.unf".into()).is_ok(),
            "{src}"
        );
    }
    assert_eq!(
        serialized("glyph a 2 1 margin-x 2|3 margin-y 1|1\n@@@@\n"),
        "glyph a 2 1 margin-x 2|3 margin-y 1\n@@@@\n"
    );
    assert_eq!(
        serialized("glyph a margin-y 0|4 2 1\n@@@@\n"),
        "glyph a 2 1 margin-y 0|4\n@@@@\n"
    );
}

#[test]
fn a_malformed_margin_does_not_parse() {
    for flag in [
        "margin-x",
        "margin-x a",
        "margin-x 1|",
        "margin-x |1",
        "margin-x 1|2|3",
        "margin-x -1",
        "margin-x 1 margin-x 2",
    ] {
        let src = format!("glyph a 2 1 {flag}\n@@@@\n");
        assert!(
            parse_document_from_str(&src, "test.unf".into()).is_err(),
            "{flag}"
        );
    }
}

// ------------------------------------------------------------------ the layout

/// The whole point: a part short of the slot across the axis by exactly its
/// margin is written by its own name and lands padded, with nothing to say
/// about it.
#[test]
fn a_part_short_of_its_slot_by_its_margin_is_padded() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g:8x5 8 5
\u{2FF1} mouth:4x2 wide:8x3
glyph h:5x6 5 6
\u{2FF0} side:2x3 tall:3x6
"
    ));
    assert!(
        problems(&expansion).is_empty(),
        "{:?}",
        problems(&expansion)
    );
    assert_eq!(
        placed(&expansion, "g:8x5"),
        vec![("mouth:4x2", (2, 0)), ("wide:8x3", (0, 2))]
    );
    // Asymmetric, and on the other axis: 1 above, 2 below.
    assert_eq!(
        placed(&expansion, "h:5x6"),
        vec![("side:2x3", (0, 1)), ("tall:3x6", (2, 0))]
    );
}

/// The margin is what a part asks for when the slot is larger than it, not a
/// size it always is: a slot exactly its own box takes it unpadded.
#[test]
fn a_part_as_wide_as_its_slot_is_not_padded() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g:4x5 4 5
\u{2FF1} mouth:4x2 1 mouth:4x2
"
    ));
    assert!(
        problems(&expansion).is_empty(),
        "{:?}",
        problems(&expansion)
    );
    assert_eq!(
        placed(&expansion, "g:4x5"),
        vec![("mouth:4x2", (0, 0)), ("mouth:4x2", (0, 3))]
    );
}

/// A margin that does not make the part up to the slot is no help, and the
/// error says what it would have made it.
#[test]
fn a_margin_short_of_the_slot_is_still_an_error() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g:10x5 10 5
\u{2FF1} mouth:4x2 plank:10x3
"
    ));
    let errors = of(&expansion, Severity::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("'mouth:4x2' is wide 4, not the glyph's 10")
            && errors[0].contains("8 with its `margin-x 2`"),
        "{errors:?}"
    );
}

/// A part's margin is across the slot and only across it: `margin-x` pads a
/// `⿱` part, and says nothing about a `⿰` one, whose height it is not.
#[test]
fn a_margin_pads_only_across_its_own_axis() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g:7x6 7 6
\u{2FF0} mouth:4x2 tall:3x6
"
    ));
    let errors = of(&expansion, Severity::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("'mouth:4x2' is tall 2, not the glyph's 6"),
        "{errors:?}"
    );
}

/// A nested split is the spelled-out form, and its members are taken at their
/// boxes: `1|mouth:4x2|1` is 6 wide, whatever `mouth` would ask for on its own.
#[test]
fn a_nested_split_is_written_out_and_takes_no_margin() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g:8x5 8 5
\u{2FF1} 1|mouth:4x2|1 wide:8x3
"
    ));
    let errors = of(&expansion, Severity::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("adds up to 6 wide, not the glyph's 8"),
        "{errors:?}"
    );
}

/// The padded part is exactly the one-part nested split it abbreviates: the
/// same drawing, and the same clearances. `wide` inks its top-left two columns
/// only, so a part measured as if it sat at column 0 — rather than at the 2 its
/// margin puts it at — would read 0 between the two where there are 2.
#[test]
fn a_padded_part_is_the_nested_split_it_abbreviates() {
    let src = |slot: &str| {
        format!(
            "audit ideal-clearance test-* 0 1\n{PARTS}\
glyph test-g 8 5
\u{2FF1} {slot} wide:8x3
"
        )
    };
    let short = expand(&src("mouth:4x2"));
    let long = expand(&src("2|mouth:4x2|2"));
    assert!(problems(&short).is_empty(), "{:?}", problems(&short));

    let chores = |e: &Expansion| -> Vec<String> {
        of(e, Severity::Chore)
            .into_iter()
            .map(|m| m.replace("2|mouth:4x2|2", "mouth:4x2"))
            .collect()
    };
    assert_eq!(chores(&short), chores(&long));
    assert!(
        chores(&short)
            .iter()
            .any(|m| m.contains("leaves 2 between 'mouth:4x2' and 'wide:8x3'")),
        "{:?}",
        chores(&short)
    );

    let resolve = |src: &str| {
        let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (resolved, _) = crate::ref_composite::resolve_expansion(
            crate::render::ttf_builder::expand_documents(&docs, &name_parts),
            &name_parts,
            &crate::cancel::CancelToken::never(),
        );
        let g = &resolved["test-g"];
        (g.grid.clone(), g.origin_col, g.origin_row)
    };
    assert!(resolve(&src("mouth:4x2")) == resolve(&src("2|mouth:4x2|2")));
}

/// A component with no variant picked has one that fits once its margin is
/// counted, so the line is waiting for a decision (a todo), and not one that
/// cannot be made (a warning naming what the family draws).
#[test]
fn an_undecided_family_fits_with_its_margin() {
    let expansion = expand(&format!(
        "{PARTS}\
glyph g:8x5 8 5
\u{2FF1} mouth wide:8x3
"
    ));
    assert!(
        of(&expansion, Severity::Warning).is_empty(),
        "{:?}",
        of(&expansion, Severity::Warning)
    );
    assert_eq!(of(&expansion, Severity::Todo).len(), 1);
}

/// The editor draws from the document body, which still holds the line, and
/// has to land the part where the font does.
#[test]
fn the_live_view_pads_the_part_as_the_font_does() {
    let src = format!(
        "{PARTS}\
glyph g:8x5 8 5
\u{2FF1} mouth:4x2 wide:8x3
"
    );
    let doc = parse_document_from_str(&src, "test.unf".into()).unwrap();
    let name_parts = crate::document::NamePartsMap::default();
    let (resolved, alt_index) =
        crate::ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
    let body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "g:8x5" => Some(body),
            _ => None,
        })
        .expect("g:8x5");
    let composite = crate::ref_composite::compute_composite(
        body,
        &resolved,
        &name_parts,
        &alt_index,
        &Default::default(),
        &Default::default(),
    )
    .expect("an IDC line alone is a composite");
    let at: Vec<(i16, i16)> = composite
        .layers
        .iter()
        .map(|l| (l.logical_offset_col, l.logical_offset_row))
        .collect();
    assert_eq!(at, vec![(2, 0), (0, 2)]);
}

// ------------------------------------------------------------------ enclosures

/// 凵 drawn tight, and the same drawing with its margin drawn in. `cup` is 5x4
/// and asks for a cell either side and one above, which makes it the 7x5 glyph
/// exactly; the cell above is on the side `⿶` opens on, so the room it offers
/// once placed runs through it and is 3x4 — the cavity its name promises. In
/// its own box it would be 3x3.
const CUPS: &str = "\
audit ideal-clearance test-* 0 1
glyph cup:5x4.3x4 5 4 margin-x 1 margin-y 1|0
@@......@@
@@......@@
@@......@@
@@@@@@@@@@
glyph cupp:7x5.3x4 7 5
..............
..@@......@@..
..@@......@@..
..@@......@@..
..@@@@@@@@@@..
glyph dot:3x2 3 2
@@@@@@
@@@@@@
glyph wall:7x5.3x3 7 5
@@@@@@@@@@@@@@
@@@@......@@@@
@@@@......@@@@
@@@@......@@@@
@@@@@@@@@@@@@@
";

fn enclosed(outer: &str, rest: &str) -> String {
    format!("{CUPS}glyph test-g 7 5\n\u{2FF6} {outer} {rest}\n")
}

/// An outer part its margin makes up to the glyph is placed by it, promises the
/// cavity it leaves where it is placed, and is measured there: the same
/// drawing and the same clearances as the part with its margin drawn in.
#[test]
fn an_outer_part_is_padded_by_its_margin() {
    let short = expand(&enclosed("cup:5x4.3x4", "dot:3x2 2 1"));
    let long = expand(&enclosed("cupp:7x5.3x4", "dot:3x2 2 1"));
    assert!(problems(&short).is_empty(), "{:?}", problems(&short));
    assert_eq!(
        placed(&short, "test-g"),
        vec![("cup:5x4.3x4", (1, 1)), ("dot:3x2", (2, 1))]
    );
    let chores = |e: &Expansion| -> Vec<String> {
        of(e, Severity::Chore)
            .into_iter()
            .map(|m| m.replace("cupp:7x5.3x4", "cup:5x4.3x4"))
            .collect()
    };
    assert!(!chores(&long).is_empty(), "the layout is measured at all");
    assert_eq!(chores(&short), chores(&long));

    let resolve = |src: &str| {
        let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (resolved, _) = crate::ref_composite::resolve_expansion(
            crate::render::ttf_builder::expand_documents(&docs, &name_parts),
            &name_parts,
            &crate::cancel::CancelToken::never(),
        );
        let g = &resolved["test-g"];
        (g.grid.clone(), g.origin_col, g.origin_row)
    };
    assert!(
        resolve(&enclosed("cup:5x4.3x4", "dot:3x2 2 1"))
            == resolve(&enclosed("cupp:7x5.3x4", "dot:3x2 2 1"))
    );
}

/// The cavity is read where the part is placed, so a promise larger than that
/// room is still broken.
#[test]
fn a_padded_outer_part_still_keeps_its_promise() {
    let src =
        enclosed("cup:5x4.3x5", "dot:3x2 2 1").replace("glyph cup:5x4.3x4", "glyph cup:5x4.3x5");
    let warnings = of(&expand(&src), Severity::Warning)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|m| m.contains("promises a 3x5 cavity")),
        "{warnings:?}"
    );
}

/// A margin that does not make the part up to the glyph is no help.
#[test]
fn an_outer_part_short_of_the_glyph_even_with_its_margin_is_an_error() {
    let src = enclosed("cup:5x4.3x4", "dot:3x2 2 1").replace("margin-y 1|0", "margin-y 0");
    let errors = of(&expand(&src), Severity::Error)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("is 5x4, not the glyph's 7x5")
            && errors[0].contains("7x4 with its `margin-x 1` `margin-y 0`"),
        "{errors:?}"
    );
}

/// The inner part is placed by the offsets the line writes, so its own margin
/// says nothing there: a part as large as the glyph is still too large to sit
/// inside it however its margin reads.
#[test]
fn an_inner_part_takes_no_margin() {
    let src = format!(
        "{CUPS}glyph big:7x5 7 5 margin-x 1\n{}glyph test-g 7 5\n\u{2FF4} wall:7x5.3x3 big:7x5 0 0\n",
        "@@@@@@@@@@@@@@\n".repeat(5)
    );
    let expansion = expand(&src);
    assert!(
        problems(&expansion).is_empty(),
        "{:?}",
        problems(&expansion)
    );
    assert_eq!(
        placed(&expansion, "test-g"),
        vec![("wall:7x5.3x3", (0, 0)), ("big:7x5", (0, 0))]
    );
}

/// An undecided outer part whose family fits once its margin is counted is a
/// decision waiting to be made, not one that cannot be.
#[test]
fn an_undecided_outer_family_fits_with_its_margin() {
    let expansion = expand(&enclosed("cup", "dot:3x2 2 1"));
    assert!(
        of(&expansion, Severity::Warning).is_empty(),
        "{:?}",
        of(&expansion, Severity::Warning)
    );
    assert_eq!(of(&expansion, Severity::Todo).len(), 1);
}

/// Across a side the operator opens on, the cavity is flush against the glyph's
/// edge, so where the drawing sits across it decides how much room there is.
/// 匚 drawn tight and padded one cell on its left leaves 3 columns up to the
/// right edge, not the 4 its own box would.
#[test]
fn a_padded_outer_part_offers_the_room_where_it_sits() {
    let src = |cavity: &str| {
        format!(
            "audit ideal-clearance g 0 9
glyph box:4x5.{cavity} 4 5 margin-x 1|0
@@@@@@@@
@@......
@@......
@@......
@@@@@@@@
glyph dot:1x1 1 1
@@
glyph g 5 5
\u{2FF7} box:4x5.{cavity} dot:1x1 3 2
"
        )
    };
    assert!(problems(&expand(&src("3x3"))).is_empty());
    let warnings: Vec<String> = of(&expand(&src("4x3")), Severity::Warning)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        warnings.iter().any(|m| m.contains("promises a 4x3 cavity")),
        "{warnings:?}"
    );
}
