//! Tests for the IDC line and the variant name rule.

use super::*;
use crate::document::{ComposeItem, GlyphCompose};

fn part(name: &str) -> ComposeItem {
    ComposeItem::Part {
        name: name.to_string(),
        raw_name: None,
    }
}

fn line(op: IdcOp, items: Vec<ComposeItem>) -> GlyphCompose {
    GlyphCompose {
        op,
        items,
        comment: None,
    }
}

/// `dims` over a table, with everything else unknown.
fn table<'a>(entries: &'a [(&'a str, (u16, u16))]) -> impl Fn(&str) -> PartDims + 'a {
    move |name: &str| {
        entries
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(PartDims::Unknown, |(_, (w, h))| PartDims::Size(*w, *h))
    }
}

fn expand(
    parent: Option<(u16, u16)>,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
) -> (Vec<GlyphRef>, Vec<(Severity, String)>) {
    expand_compose("test", parent, 1, compose, dims)
}

fn errors(issues: &[(Severity, String)]) -> Vec<&str> {
    issues
        .iter()
        .filter(|(s, _)| *s == Severity::Error)
        .map(|(_, m)| m.as_str())
        .collect()
}

#[test]
fn variant_spec_reads_size_and_direction() {
    let spec = VariantSpec::parse("han-6c35:4x16-l");
    assert_eq!(spec.size, Some((4, 16)));
    assert_eq!(spec.direction, Some(Direction::Left));

    // Order within the suffix does not matter, and either half may be absent.
    assert_eq!(
        VariantSpec::parse("x:r-5x16"),
        VariantSpec {
            size: Some((5, 16)),
            direction: Some(Direction::Right),
        }
    );
    assert_eq!(VariantSpec::parse("x:compressed"), VariantSpec::default());
    assert_eq!(VariantSpec::parse("x").size, None);
    // Only the first of each kind counts.
    assert_eq!(VariantSpec::parse("x:4x16-8x16").size, Some((4, 16)));
    assert_eq!(VariantSpec::parse("x:l-r").direction, Some(Direction::Left));
    // A name whose *base* looks like a size says nothing: the rule reads the
    // suffix, so `4x16` stays the on-demand rectangle it always was.
    assert_eq!(VariantSpec::parse("4x16").size, None);
    // One spelling per size.
    assert_eq!(VariantSpec::parse("x:04x16").size, None);
}

#[test]
fn direction_rank_prefers_the_slot_then_the_unmarked() {
    let slot = Some(Direction::Left);
    assert_eq!(direction_rank("a:4x16-l", slot), 0);
    assert_eq!(direction_rank("a:4x16", slot), 1);
    assert_eq!(direction_rank("a:4x16-r", slot), 2);
    // A ranking is only ever a sort key, and equal ranks keep their order.
    let mut names = vec!["a:4x16-r", "b:4x16", "c:4x16-l", "d:4x16"];
    names.sort_by_key(|n| direction_rank(n, slot));
    assert_eq!(names, vec!["c:4x16-l", "b:4x16", "d:4x16", "a:4x16-r"]);
}

#[test]
fn slot_directions_follow_the_operator() {
    use IdcOp::*;
    assert_eq!(LeftRight.slot_direction(0), Some(Direction::Left));
    assert_eq!(LeftRight.slot_direction(1), Some(Direction::Right));
    assert_eq!(LeftRight.slot_direction(2), None);
    assert_eq!(LeftMiddleRight.slot_direction(1), Some(Direction::Center));
    assert_eq!(AboveBelow.slot_direction(0), Some(Direction::Up));
    assert_eq!(AboveMiddleBelow.slot_direction(2), Some(Direction::Down));
    assert_eq!(IdcOp::from_token("\u{2FF0}"), Some(LeftRight));
    assert_eq!(IdcOp::from_token("\u{2FF0}x"), None);
    assert_eq!(IdcOp::from_token("ref"), None);
}

#[test]
fn horizontal_split_places_parts_left_to_right() {
    let dims = table(&[("a:4x16", (4, 16)), ("b:11x16", (11, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:11x16")]),
        &dims,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].offset, Some((0, 0)));
    assert_eq!(refs[1].offset, Some((4, 0)));
}

#[test]
fn vertical_split_places_parts_top_to_bottom() {
    let dims = table(&[("a:15x8", (15, 8)), ("b:15x8", (15, 8))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::AboveBelow, vec![part("a:15x8"), part("b:15x8")]),
        &dims,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[0].offset, Some((0, 0)));
    assert_eq!(refs[1].offset, Some((0, 8)));
}

#[test]
fn a_gap_moves_the_cursor_and_counts_towards_the_sum() {
    let dims = table(&[("a:4x16", (4, 16)), ("b:12x16", (12, 16))]);
    // 4 + (-1) + 12 == 15: the overlap the design calls for.
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(
            IdcOp::LeftRight,
            vec![part("a:4x16"), ComposeItem::Gap(-1), part("b:12x16")],
        ),
        &dims,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[1].offset, Some((3, 0)));

    // A leading gap is a bearing inside the box, and it counts too.
    let dims = table(&[("a:4x16", (4, 16)), ("b:10x16", (10, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(
            IdcOp::LeftRight,
            vec![ComposeItem::Gap(1), part("a:4x16"), part("b:10x16")],
        ),
        &dims,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[0].offset, Some((1, 0)));
    assert_eq!(refs[1].offset, Some((5, 0)));
}

#[test]
fn three_way_splits_take_three_parts() {
    let dims = table(&[("a:5x16", (5, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(
            IdcOp::LeftMiddleRight,
            vec![part("a:5x16"), part("a:5x16"), part("a:5x16")],
        ),
        &dims,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[2].offset, Some((10, 0)));

    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftMiddleRight, vec![part("a:5x16"), part("a:5x16")]),
        &dims,
    );
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("takes 3 components")),
        "{issues:?}"
    );
}

#[test]
fn parts_that_do_not_add_up_are_an_error() {
    let dims = table(&[("a:4x16", (4, 16)), ("b:10x16", (10, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:10x16")]),
        &dims,
    );
    // Still laid out — the report is what makes the mistake visible, and the
    // editor has to draw the glyph being fixed.
    assert_eq!(refs.len(), 2);
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("add up to 14 across the glyph's 15")),
        "{issues:?}"
    );
}

#[test]
fn a_part_must_span_the_other_axis() {
    let dims = table(&[("a:4x14", (4, 14)), ("b:11x16", (11, 16))]);
    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a:4x14"), part("b:11x16")]),
        &dims,
    );
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("is tall 14, not the glyph's 16")),
        "{issues:?}"
    );
}

#[test]
fn a_name_that_lies_about_its_size_is_an_error() {
    let dims = table(&[("a:4x16", (5, 16)), ("b:11x16", (11, 16))]);
    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:11x16")]),
        &dims,
    );
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("names 4x16 but the glyph is 5x16")),
        "{issues:?}"
    );
}

#[test]
fn a_part_without_a_variant_suffix_is_an_error() {
    let dims = table(&[("a", (4, 16)), ("b:11x16", (11, 16))]);
    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a"), part("b:11x16")]),
        &dims,
    );
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("names no variant")),
        "{issues:?}"
    );
}

#[test]
fn an_undefined_part_is_an_error_but_the_rest_still_lands() {
    let dims = table(&[("b:11x16", (11, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("gone:4x16"), part("b:11x16")]),
        &dims,
    );
    assert_eq!(refs.len(), 2);
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("'gone:4x16' is not defined")),
        "{issues:?}"
    );
}

#[test]
fn a_part_with_no_declared_box_is_an_error() {
    let dims = |name: &str| match name {
        "a:4x16" => PartDims::Undeclared,
        "b:11x16" => PartDims::Size(11, 16),
        _ => PartDims::Unknown,
    };
    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:11x16")]),
        &dims,
    );
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("declares no `W H`")),
        "{issues:?}"
    );
}

#[test]
fn a_parent_without_a_box_cannot_be_split() {
    let dims = table(&[("a:4x16", (4, 16)), ("b:11x16", (11, 16))]);
    let (refs, issues) = expand(
        None,
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:11x16")]),
        &dims,
    );
    assert!(refs.is_empty());
    assert!(
        errors(&issues)
            .iter()
            .any(|m| m.contains("needs the enclosing `glyph` header")),
        "{issues:?}"
    );
}

#[test]
fn a_part_drawn_for_the_other_side_is_only_a_warning() {
    let dims = table(&[("a:4x16-r", (4, 16)), ("b:11x16", (11, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a:4x16-r"), part("b:11x16")]),
        &dims,
    );
    assert_eq!(refs.len(), 2, "the glyph is still built");
    assert!(errors(&issues).is_empty(), "{issues:?}");
    assert!(
        issues
            .iter()
            .any(|(s, m)| *s == Severity::Warning && m.contains("sits in the `-l` slot")),
        "{issues:?}"
    );
}

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
        &crate::document::NamePartsMap::new(),
    );
    let whole = resolved.get("whole").expect("whole should resolve");
    assert_eq!((whole.grid.width, whole.grid.height), (4, 4));
    // Each half draws one of its own two columns: the left one column 0, the
    // right one column 1. So ink at 0 and at 3 is the derived offset of 2 —
    // the number nothing in the file wrote.
    for row in 0..4 {
        let on: Vec<u16> = (0..4)
            .filter(|c| whole.grid.get(row, *c).is_filled())
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
    let name_parts = crate::document::NamePartsMap::new();
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
    )
    .expect("an IDC line alone is a composite");
    assert_eq!(composite.layers.len(), 2);
    assert_eq!(composite.layers[0].logical_offset_col, 0);
    assert_eq!(composite.layers[1].logical_offset_col, 2);
}

#[test]
fn a_split_that_does_not_add_up_is_reported_against_its_line() {
    // Both halves on the left: 2 + 2 == 4 is satisfied, so swap in a part that
    // is used twice and a parent that is one wider.
    let doc = crate::document_io::parse_document_from_str(
        &source("\u{2FF0} part:2x4-l part:2x4-r").replace("glyph whole 4 4", "glyph whole 5 4"),
        "test.unf".into(),
    )
    .unwrap();
    let msgs = messages(&doc);
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("Error") && m.contains("add up to 4 across the glyph's 5")),
        "{msgs:?}"
    );
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

#[test]
fn an_idc_line_round_trips_through_the_serializer() {
    let input = source("\u{2FF0} part:2x4-l -1 part:2x4-r // a note");
    let doc = crate::document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let mut out = Vec::new();
    crate::document_io::serialize_document(&doc, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), input);
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
        2,
        &line(IdcOp::LeftRight, vec![part("a:4x16"), part("b:11x16")]),
        &dims,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[1].offset, Some((8, 0)));
}
