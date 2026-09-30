//! Tests through the real pipeline: parse, expand, resolve, report, and the assumed line.

use super::*;

// ---------------------------------------------------------------------------
// Through the real pipeline: parse → expand → resolve → report
// ---------------------------------------------------------------------------

/// Two 2×4 halves and the 4×4 glyph they split, with `$SPLIT` standing in for
/// the IDC line under test.
fn source(split: &str) -> String {
    format!(
        "\
glyph part:2x4-l 2 4
@@..
@@..
@@..
@@..

glyph part:2x4-r 2 4
..@@
..@@
..@@
..@@

glyph whole 4 4
{split}
map U+4E00 = whole
"
    )
}

fn parse(split: &str) -> crate::document::Document {
    crate::document_io::parse_document_from_str(&source(split), "test.unf".into()).unwrap()
}

fn messages(doc: &crate::document::Document) -> Vec<String> {
    crate::issues::collect_issues(&[doc])
        .into_iter()
        .map(|i| format!("{:?}: {}", i.severity, i.message))
        .collect()
}

#[test]
fn an_idc_line_resolves_to_the_composed_glyph() {
    let doc = parse("\u{2FF0} part:2x4-l part:2x4-r");
    let msgs = messages(&doc);
    assert!(
        !msgs.iter().any(|m| m.starts_with("Error")),
        "clean source: {msgs:?}"
    );

    let (resolved, _) = crate::ref_composite::resolve_named_glyphs_with_parts(
        &[&doc],
        &crate::document::NamePartsMap::default(),
    );
    let whole = resolved.get("whole").expect("whole should resolve");
    assert_eq!((whole.grid.width, whole.grid.height), (4, 4));
    // Each half draws one of its own two columns: the left one column 0, the
    // right one column 1. So ink at 0 and at 3 is the derived offset of 2 —
    // the number nothing in the file wrote.
    for row in 0..4 {
        let on: Vec<u16> = (0..4)
            .filter(|c| whole.grid.get(row, *c).is_bitmap_filled())
            .collect();
        assert_eq!(on, vec![0, 3], "row {row}");
    }
}

#[test]
fn the_live_view_places_the_parts_where_the_font_does() {
    // The editor composes from the document body, which still holds the IDC
    // line; the build composes from the expansion, which no longer does. The
    // two must land in the same place — a glyph drawn one way on screen and
    // another in the font is the failure this derivation exists to avoid.
    let doc = parse("\u{2FF0} part:2x4-l part:2x4-r");
    let name_parts = crate::document::NamePartsMap::default();
    let (resolved, alt_index) =
        crate::ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
    let body = doc
        .items
        .iter()
        .find_map(|item| match item {
            crate::document::DocumentItem::Glyph { name, body } if name.display() == "whole" => {
                Some(body)
            }
            _ => None,
        })
        .expect("whole");
    let composite = crate::ref_composite::compute_composite(
        body,
        &resolved,
        &name_parts,
        &alt_index,
        &Default::default(),
        &Default::default(),
    )
    .expect("an IDC line alone is a composite");
    assert_eq!(composite.layers.len(), 2);
    assert_eq!(composite.layers[0].logical_offset_col, 0);
    assert_eq!(composite.layers[1].logical_offset_col, 2);
}

#[test]
fn a_component_is_a_use_of_the_glyph() {
    // Nothing `ref`s the halves; only the IDC line names them, and that has to
    // count or every part of every composed glyph reads as unused.
    let doc = parse("\u{2FF0} part:2x4-l part:2x4-r");
    let msgs = messages(&doc);
    assert!(
        !msgs.iter().any(|m| m.contains("unused")),
        "a component is not unused: {msgs:?}"
    );
}

#[test]
fn two_idc_lines_in_one_glyph_are_an_error() {
    let doc = parse("\u{2FF0} part:2x4-l part:2x4-r\n\u{2FF1} part:2x4-l part:2x4-r");
    let msgs = messages(&doc);
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("Error") && m.contains("has 2 IDC lines")),
        "{msgs:?}"
    );
}

/// A split fills the glyph's *box*, and a box with an `origin` does not start at
/// the grid's corner: the parts start where the box does, exactly as the
/// grid's own ink and every written `ref` are measured from the grid.
#[test]
fn an_idc_line_fills_the_box_where_the_origin_puts_it() {
    let src = source("\u{2FF0} part:2x4-l part:2x4-r\nref part:2x4-l 0 0").replace(
        "glyph whole 4 4",
        "glyph whole 5 4 origin 1 0 extent 4 4\n..........\n..........\n..........\n@@........",
    );
    let doc = crate::document_io::parse_document_from_str(&src, "test.unf".into()).unwrap();
    let msgs = messages(&doc);
    assert!(!msgs.iter().any(|m| m.starts_with("Error")), "{msgs:?}");
    let (resolved, _) = crate::ref_composite::resolve_named_glyphs_with_parts(
        &[&doc],
        &crate::document::NamePartsMap::default(),
    );
    let whole = resolved.get("whole").expect("whole should resolve");
    let on = |row: u16| -> Vec<u16> {
        (0..whole.grid.width)
            .filter(|c| whole.grid.get(row, *c).is_bitmap_filled())
            .collect()
    };
    // The written `ref` and the grid's own cell sit at grid column 0; the
    // split's halves draw box columns 0 and 3, which are grid columns 1 and 4.
    assert_eq!(on(0), vec![0, 1, 4]);
    assert_eq!(on(3), vec![0, 1, 4]);
}

#[test]
fn an_idc_line_round_trips_through_the_serializer() {
    let input = source("\u{2FF0} part:2x4-l -1 part:2x4-r // a note");
    let doc = crate::document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let mut out = Vec::new();
    crate::document_io::serialize_document(&doc, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), input);
}

#[test]
fn an_assumed_idc_line_round_trips_through_the_serializer() {
    let input = source("assume \u{2FF0} part:2x4-l -1 part:2x4-r // a note");
    let doc = crate::document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let mut out = Vec::new();
    crate::document_io::serialize_document(&doc, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), input);
}

/// [`source`] with `line` in it, held to a band of `1..1` that its halves,
/// ink flush against both edges, do not meet.
fn assumed_source(line: &str) -> String {
    format!("audit ideal-clearance whole 1 1\n{}", source(line))
}

fn findings(src: &str) -> Vec<(Severity, String)> {
    let doc = crate::document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    crate::issues::collect_issues(&[&doc])
        .into_iter()
        .map(|i| (i.severity, i.message))
        .collect()
}

/// `assume` takes the clearances on trust and nothing else: the chores go, and
/// the glyph is built exactly as the plain line builds it.
#[test]
fn an_assumed_line_drops_its_clearance_chores() {
    let plain = assumed_source("\u{2FF0} part:2x4-l part:2x4-r");
    let loud = findings(&plain);
    assert!(loud.iter().any(|(s, _)| *s == Severity::Chore), "{loud:?}");

    let assumed = assumed_source("assume \u{2FF0} part:2x4-l part:2x4-r");
    let quiet = findings(&assumed);
    assert!(quiet.is_empty(), "{quiet:?}");

    let name_parts = crate::document::NamePartsMap::default();
    let ink = |src: &str| {
        let doc = crate::document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
        let (resolved, _) =
            crate::ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        let whole = resolved.get("whole").expect("whole should resolve");
        (0..4)
            .filter(|c| whole.grid.get(0, *c).is_bitmap_filled())
            .collect::<Vec<u16>>()
    };
    assert_eq!(ink(&assumed), vec![0, 3]);
    assert_eq!(ink(&assumed), ink(&plain));
}

/// A position a name claims is not a clearance: a part drawn for the other side
/// still warns on an `assume`d line.
#[test]
fn an_assumed_line_still_warns_about_positions() {
    let found = findings(&assumed_source("assume \u{2FF0} part:2x4-r part:2x4-l"));
    assert_eq!(
        found
            .iter()
            .filter(|(s, m)| *s == Severity::Warning && m.contains("sits in the"))
            .count(),
        2,
        "{found:?}"
    );
    assert!(
        !found.iter().any(|(s, _)| *s == Severity::Chore),
        "{found:?}"
    );
}

/// Whether the parts fit is not a matter of taste: a part that does not span
/// the other axis is as much an error as ever.
#[test]
fn an_assumed_line_still_reports_its_errors() {
    let src = source("assume \u{2FF0} part:2x4-l part:2x4-r")
        .replace("glyph whole 4 4", "glyph whole 4 5");
    let found = findings(&src);
    assert!(
        found
            .iter()
            .any(|(s, m)| *s == Severity::Error && m.contains("not the glyph's 5")),
        "{found:?}"
    );
}

/// On an enclosure too, only the chores go: the cavity an outer part does not
/// promise is still a warning, and an unwritten placement still a todo.
#[test]
fn an_assumed_enclosure_drops_only_its_chores() {
    const SRC: &str = "\
audit ideal-clearance test-* 0 0

glyph ring:6x6 6 6
@@@@@@@@@@@@
@@........@@
@@........@@
@@........@@
@@........@@
@@@@@@@@@@@@

glyph seed:2x2 2 2
@@@@
@@@@

glyph test-x 6 6
$LINE
";
    let found = |line: &str| findings(&SRC.replace("$LINE", line));
    let promises_nothing = |f: &[(Severity, String)]| {
        f.iter()
            .any(|(s, m)| *s == Severity::Warning && m.contains("promises no cavity"))
    };

    let loud = found("\u{2FF4} ring:6x6 seed:2x2 1 1");
    assert!(promises_nothing(&loud), "{loud:?}");
    assert!(loud.iter().any(|(s, _)| *s == Severity::Chore), "{loud:?}");

    let quiet = found("assume \u{2FF4} ring:6x6 seed:2x2 1 1");
    assert!(promises_nothing(&quiet), "{quiet:?}");
    assert!(
        !quiet.iter().any(|(s, _)| *s == Severity::Chore),
        "{quiet:?}"
    );

    let unplaced = found("assume \u{2FF4} ring:6x6 seed:2x2");
    assert!(
        unplaced.iter().any(|(s, _)| *s == Severity::Todo),
        "{unplaced:?}"
    );
}

#[test]
fn a_component_may_be_written_with_an_at_name() {
    let input = "\
glyph whole 4 4
\u{2FF0} @-l:2x4-l @-r:2x4-r

glyph @-l:2x4-l 2 4
@@..
@@..
@@..
@@..

glyph @-r:2x4-r 2 4
..@@
..@@
..@@
..@@
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let msgs = messages(&doc);
    assert!(
        !msgs.iter().any(|m| m.starts_with("Error")),
        "`@` expands like a ref's: {msgs:?}"
    );
    // …and the written form is what comes back out.
    let mut out = Vec::new();
    crate::document_io::serialize_document(&doc, &mut out).unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("\u{2FF0} @-l:2x4-l @-r:2x4-r"),
        "the `@` form is kept",
    );
}

#[test]
fn offsets_leave_in_the_parents_raster_units() {
    // Everything above is declared units; only the offsets are multiplied, so
    // a scale-2 parent places its second part at 2 * 4.
    let dims = table(&[("a:4x16", (4, 16)), ("b:11x16", (11, 16))]);
    let (refs, issues) = expand_compose(
        "test",
        Some((15, 16)),
        scaled(2),
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:11x16")]),
        &dims,
        None,
        None,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[1].offset, Some((8, 0)));
}
