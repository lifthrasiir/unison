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
    expand_compose("test", parent, 1, compose, dims, None)
}

fn of_severity(issues: &[(Severity, String)], want: Severity) -> Vec<&str> {
    issues
        .iter()
        .filter(|(s, _)| *s == want)
        .map(|(_, m)| m.as_str())
        .collect()
}

fn errors(issues: &[(Severity, String)]) -> Vec<&str> {
    of_severity(issues, Severity::Error)
}

fn todos(issues: &[(Severity, String)]) -> Vec<&str> {
    of_severity(issues, Severity::Todo)
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

/// A component with no `:` has not picked its variant yet, which is where
/// every IDS-populated glyph starts. It is a TODO, and — this is the part that
/// matters — it silences nothing else and is silenced by nothing: no error is
/// reported for the line at all, least of all a sum error about a width nobody
/// has chosen.
#[test]
fn a_part_without_a_variant_suffix_is_a_todo_and_not_an_error() {
    let dims = table(&[("a", (4, 16)), ("b:11x16", (11, 16))]);
    let (refs, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a"), part("b:11x16")]),
        &dims,
    );
    assert_eq!(errors(&issues), Vec::<&str>::new(), "{issues:?}");
    assert!(
        todos(&issues)
            .iter()
            .any(|m| m.contains("no variant picked yet")),
        "{issues:?}"
    );
    // Still placed, so the decided half of the line draws where it will end up.
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[1].offset, Some((4, 0)));
}

/// An undecided component with no glyph behind it at all is the ordinary IDS
/// case (`⿰ han-6c35 han-53ef` before either part exists); it is one TODO and
/// not an "is not defined" error.
#[test]
fn an_undecided_part_that_names_nothing_is_only_a_todo() {
    let dims = table(&[]);
    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a"), part("b")]),
        &dims,
    );
    assert_eq!(errors(&issues), Vec::<&str>::new(), "{issues:?}");
    assert_eq!(todos(&issues).len(), 2, "{issues:?}");
}

/// The decided half of an undecided line is still fully checked: what stands
/// down is the clearance check and the undecided component's own claims, not
/// the whole line.
#[test]
fn an_undecided_line_still_checks_its_decided_parts() {
    let dims = table(&[("a", (4, 16)), ("b:11x16", (11, 17))]);
    let (_, issues) = expand(
        Some((15, 16)),
        &line(IdcOp::LeftRight, vec![part("a"), part("b:11x16")]),
        &dims,
    );
    assert!(
        errors(&issues).iter().any(|m| m.contains("is tall 17")),
        "{issues:?}"
    );
    assert!(
        errors(&issues).iter().any(|m| m.contains("names 11x16")),
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
        None,
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(refs[1].offset, Some((8, 0)));
}

// ---------------------------------------------------------------- clearance

/// A grid from a picture: `#` is ink, `$` a hardblank, anything else nothing.
fn grid(rows: &[&str]) -> PixelGrid {
    let mut g = PixelGrid::new(rows[0].len() as u16, rows.len() as u16);
    for (r, row) in rows.iter().enumerate() {
        for (c, ch) in row.chars().enumerate() {
            let shape = match ch {
                '#' => crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
                '$' => crate::pixel::PixelShape::new(crate::pixel::PX_HARDBLANK, false),
                _ => continue,
            };
            g.set(r as u16, c as u16, shape);
        }
    }
    g
}

/// The profile of a grid that is its own declared box, which is what a part
/// with no `origin`/`extent` of its own is.
fn whole(g: &PixelGrid, scale: u8) -> InkProfile {
    let s = scale.max(1) as u16;
    InkProfile::of(g, scale, (0, 0), (g.width / s, g.height / s))
}

/// A part is measured on its *declared box*, not on its grid: the clearance an
/// IDC line checks is the room the box leaves, and a part whose grid is bigger
/// than what it claims to fill would otherwise measure its own margin as ink
/// standing in the way.
///
/// Along the axis, though, what is outside the box is measured where it is
/// drawn rather than folded onto the box's edge: it can only take room away,
/// never invent it — the part is drawing (or claiming) where it said it would
/// not, and the neighbour has to be told.
#[test]
fn a_clearance_is_measured_on_the_declared_box() {
    // Grid 6 wide, box the middle 4: the ink at grid column 2 is the box's
    // column 1, and the empty column 0 of the grid is outside the box entirely.
    let g = grid(&["..##..", "..#..."]);
    let p = InkProfile::of(&g, 1, (1, 0), (4, 2));
    assert_eq!(p.rows[0].expect("row 0 is occupied").near, 1);
    assert_eq!(p.rows[0].expect("row 0 is occupied").far, 2);
    assert_eq!(p.cols.len(), 4, "the profile is the box's width");

    // The same grid measured as itself, for contrast.
    let as_grid = InkProfile::of(&g, 1, (0, 0), (6, 2));
    assert_eq!(as_grid.rows[0].expect("row 0 is occupied").near, 2);

    // Ink before the box's own corner keeps the coordinate it is drawn at.
    let escaping = InkProfile::of(&grid(&["#....."]), 1, (2, 0), (4, 1));
    assert_eq!(escaping.rows[0].expect("row 0 is occupied").near, -2);
}

/// A claim survives composition, and a negated claim releases it — measured
/// where it is actually observable, at the clearance frontier.
///
/// A part that claims the space to its right holds the frontier out there, so a
/// facing part cannot slide in. Overlaying a hardblank-only glyph negated is
/// how a composite takes that claim back; the frontier then falls to where the
/// ink stops. Both halves used to be lost in [`PixelGrid::blit`], which sent
/// every pair through the region layer, where a claim is indistinguishable from
/// the nothing it draws.
#[test]
fn a_negated_hardblank_releases_a_claim_at_the_frontier() {
    let claimed = grid(&["#$$"]);
    let line = whole(&claimed, 1).rows[0].expect("the row is occupied");
    assert_eq!(
        (line.near, line.far, line.far_hardblanks),
        (0, 2, 2),
        "the claim holds the frontier out past the ink"
    );

    // Blitting the same claim over itself must not annihilate it.
    let mut doubled = claimed.clone();
    doubled.blit(&grid(&[".$$"]), 0, 0, false);
    assert_eq!(
        whole(&doubled, 1).rows[0],
        Some(line),
        "a claim over a claim is one claim"
    );

    let mut released = claimed.clone();
    released.blit(&grid(&[".$$"]), 0, 0, true);
    let line = whole(&released, 1).rows[0].expect("the ink is still there");
    assert_eq!(
        (line.near, line.far, line.far_hardblanks),
        (0, 0, 0),
        "with the claim released the frontier falls back to the ink"
    );
}

fn profiles(entries: &[(&str, &[&str])]) -> std::collections::HashMap<String, InkProfile> {
    entries
        .iter()
        .map(|(name, rows)| (name.to_string(), whole(&grid(rows), 1)))
        .collect()
}

/// `expand`, holding the line to `min..max`.
fn with_clearance(
    parent: (u16, u16),
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    profiles: &std::collections::HashMap<String, InkProfile>,
    min: i16,
    max: i16,
) -> Vec<String> {
    let ink = |name: &str| profiles.get(name);
    let rule = ClearanceRule {
        written: "test*",
        min,
        max,
        ink: &ink,
    };
    let (_, issues) = expand_compose("test", Some(parent), 1, compose, dims, Some(&rule));
    assert!(errors(&issues).is_empty(), "{issues:?}");
    of_severity(&issues, Severity::Warning)
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[test]
fn ink_profile_reads_both_frontiers_and_counts_a_hardblank() {
    let ink = |near, far| {
        Some(InkLine {
            near,
            far,
            near_hardblanks: 0,
            far_hardblanks: 0,
        })
    };
    let p = whole(&grid(&[".##.", "....", "$..#"]), 1);
    assert_eq!(
        p.rows,
        vec![
            ink(1, 2),
            None,
            // The lone `$` is the near frontier, and a run of one.
            Some(InkLine {
                near: 0,
                far: 3,
                near_hardblanks: 1,
                far_hardblanks: 0,
            }),
        ],
    );
    assert_eq!(
        p.cols,
        vec![
            Some(InkLine {
                near: 2,
                far: 2,
                near_hardblanks: 1,
                far_hardblanks: 1,
            }),
            ink(0, 0),
            ink(0, 0),
            ink(2, 2),
        ],
    );
    // Declared units, so a scale-2 grid measures like the 1-unit glyph it is,
    // and a declared cell with any ink in it is ink.
    let doubled = whole(&grid(&["..####..", "..##$$..", "........", "........"]), 2);
    assert_eq!(doubled.rows, vec![ink(1, 2), None]);
}

#[test]
fn facing_hardblanks_overlap_as_far_as_both_reach() {
    // Two facing hardblanks meet, so a unit of each pair is shared: without
    // them the frontiers would face at -4 on every row.
    let a = whole(&grid(&["##$$", "###$"]), 1);
    let b = whole(&grid(&["$###", "$$##"]), 1);
    assert_eq!(facing_offset(&a, &b, true), Some(-3));
    // Only the shared part counts: a row whose other side has none is measured
    // as before, and one row is enough to hold the whole line back.
    let plain = whole(&grid(&["####", "$$##"]), 1);
    assert_eq!(facing_offset(&a, &plain, true), Some(-4));
    // The reach is the whole facing run, not one cell of it.
    let deep_a = whole(&grid(&["##$$", "#$$$"]), 1);
    let deep_b = whole(&grid(&["$$##", "$$$#"]), 1);
    assert_eq!(facing_offset(&deep_a, &deep_b, true), Some(-2));
    // A hardblank pointing the other way is not on this side.
    let away = whole(&grid(&["###$", "###$"]), 1);
    assert_eq!(facing_offset(&a, &away, true), Some(-4));
}

#[test]
fn an_edge_swallows_the_hardblanks_facing_it() {
    // The edge is all the hardblank anyone could want, so a part's own facing
    // run collapses into it whole: 2 in from the left, 1 in from the right.
    let p = whole(&grid(&["$$#$", "$##$"]), 1);
    let f = p.frontier(true).unwrap();
    assert_eq!((f.near, f.far), (1, 2));
    // A row of nothing but hardblanks constrains neither edge.
    let all = whole(&grid(&["$$$$", "$###"]), 1);
    let f = all.frontier(true).unwrap();
    assert_eq!((f.near, f.far), (1, 3));
    // Down the other axis the runs are read the same way.
    let f = p.frontier(false).unwrap();
    assert_eq!((f.near, f.far), (0, 1));
}

#[test]
fn clearance_is_measured_between_frontiers_and_the_edges() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:4x4", (4, 4))]);
    let ink = profiles(&[
        ("a:4x4", &["##..", "##..", "##..", "##.."]),
        ("b:4x4", &[".###", ".###", ".###", ".###"]),
    ]);
    let compose = line(IdcOp::LeftRight, vec![part("a:4x4"), part("b:4x4")]);
    // 0 at each edge, 3 down the middle: only the middle and the total are out.
    let warnings = with_clearance((8, 4), &compose, &dims, &ink, 0, 1);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("leaves 3 between 'a:4x4' and 'b:4x4', outside the ideal 0..1"),
        "{warnings:?}",
    );
    assert!(warnings[1].contains("leaves 3 in total"), "{warnings:?}");
    // A range that admits it says nothing at all.
    assert!(with_clearance((8, 4), &compose, &dims, &ink, 0, 3).is_empty());
}

#[test]
fn overlapping_ink_is_a_negative_clearance() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:4x4", (4, 4))]);
    let ink = profiles(&[
        ("a:4x4", &["####", "####", "####", "####"]),
        ("b:4x4", &["####", "####", "####", "####"]),
    ]);
    // The overlap term the boxes are allowed: 4 + (-1) + 4 == 7, and the ink
    // that fills both boxes therefore shares a column.
    let warnings = with_clearance(
        (7, 4),
        &line(
            IdcOp::LeftRight,
            vec![part("a:4x4"), ComposeItem::Gap(-1), part("b:4x4")],
        ),
        &dims,
        &ink,
        0,
        1,
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("leaves -1 between 'a:4x4' and 'b:4x4'")),
        "{warnings:?}",
    );
}

/// A hardblank drawn *outside* the declared box is a claim on the neighbour's
/// space, and it is measured where it is drawn.
///
/// This is how the Han parts write their side bearings: the box is the cells
/// the part fills and the column beyond it is the space it wants kept clear, so
/// two parts that each claim one column and are placed box-to-box are
/// overlapping — each one's claim sits on the other's ink. Folding the escaping
/// cell into the box's edge lost exactly that, since the edge is already ink.
#[test]
fn a_hardblank_outside_the_box_claims_the_neighbours_cell() {
    let dims = table(&[("a:3x1", (3, 1)), ("b:3x1", (3, 1))]);
    // `a` is 3 wide from grid column 0 and claims the column past it; `b` is 3
    // wide from grid column 1 and claims the column before it.
    let ink: std::collections::HashMap<String, InkProfile> = [
        (
            "a:3x1".to_string(),
            InkProfile::of(&grid(&["$##$"]), 1, (0, 0), (3, 1)),
        ),
        (
            "b:3x1".to_string(),
            InkProfile::of(&grid(&["$###$"]), 1, (1, 0), (3, 1)),
        ),
    ]
    .into_iter()
    .collect();
    let compose = line(IdcOp::LeftRight, vec![part("a:3x1"), part("b:3x1")]);
    // Box to box in a 6-wide parent: the two claims interlock, and the pair
    // needs the one column of gap the parent has room for.
    let tight = with_clearance((6, 1), &compose, &dims, &ink, 0, 1);
    assert!(
        tight
            .iter()
            .any(|w| w.contains("leaves -1 between 'a:3x1' and 'b:3x1'")),
        "{tight:?}",
    );
    // With the gap the claims coincide, which is one space and not two.
    let spaced = with_clearance(
        (7, 1),
        &line(
            IdcOp::LeftRight,
            vec![part("a:3x1"), ComposeItem::Gap(1), part("b:3x1")],
        ),
        &dims,
        &ink,
        0,
        1,
    );
    assert!(
        !spaced
            .iter()
            .any(|w| w.contains("between 'a:3x1' and 'b:3x1'")),
        "{spaced:?}",
    );
}

#[test]
fn the_sum_does_not_move_when_a_gap_does() {
    let dims = table(&[("a:3x4", (3, 4)), ("b:4x4", (4, 4))]);
    let ink = profiles(&[
        ("a:3x4", &["##.", "##.", "##.", "##."]),
        ("b:4x4", &[".###", ".###", ".###", ".###"]),
    ]);
    let sum = |compose: &GlyphCompose| {
        let warnings = with_clearance((8, 4), compose, &dims, &ink, 0, 0);
        warnings
            .iter()
            .find(|w| w.contains("in total"))
            .expect("a total that is not 0..0")
            .clone()
    };
    // The gap after the first part, then before it: the individual clearances
    // differ and the total cannot.
    let after = sum(&line(
        IdcOp::LeftRight,
        vec![part("a:3x4"), ComposeItem::Gap(1), part("b:4x4")],
    ));
    let before = sum(&line(
        IdcOp::LeftRight,
        vec![ComposeItem::Gap(1), part("a:3x4"), part("b:4x4")],
    ));
    assert!(after.contains("leaves 3 in total"), "{after}");
    assert!(before.contains("leaves 3 in total"), "{before}");
    assert!(after.contains("3 between 'a:3x4' and 'b:4x4'"), "{after}");
    assert!(before.contains("2 between 'a:3x4' and 'b:4x4'"), "{before}");
}

#[test]
fn a_vertical_split_measures_the_same_way_downward() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:4x4", (4, 4))]);
    let ink = profiles(&[
        ("a:4x4", &["####", "####", "....", "...."]),
        ("b:4x4", &["....", "####", "####", "####"]),
    ]);
    // a stops at row 1, b starts at row 4 + 1: 3 between them, 0 at the top
    // and 0 at the bottom.
    let warnings = with_clearance(
        (4, 8),
        &line(IdcOp::AboveBelow, vec![part("a:4x4"), part("b:4x4")]),
        &dims,
        &ink,
        0,
        1,
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("leaves 3 between 'a:4x4' and 'b:4x4'"),
        "{warnings:?}"
    );
    assert!(warnings[1].contains("leaves 3 in total"), "{warnings:?}");
}

#[test]
fn ink_before_the_parent_edge_is_a_negative_edge_clearance() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:5x4", (5, 4))]);
    let ink = profiles(&[
        ("a:4x4", &["####", "####", "####", "####"]),
        ("b:5x4", &["#####", "#####", "#####", "#####"]),
    ]);
    // A gap before the first part is a bearing, and a negative one hangs the
    // part off the left of the box: -1 + 4 + 5 == 8.
    let warnings = with_clearance(
        (8, 4),
        &line(
            IdcOp::LeftRight,
            vec![ComposeItem::Gap(-1), part("a:4x4"), part("b:5x4")],
        ),
        &dims,
        &ink,
        0,
        1,
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("leaves -1 between the left edge and 'a:4x4'")),
        "{warnings:?}",
    );
}

/// Boxes that do not fill the parent are caught by the ink they leave against
/// its edges, in both directions: too much room at the far edge, and ink past
/// it when the parts are too wide for the box.
#[test]
fn parts_that_misfit_the_box_show_up_at_its_edges() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:4x4", (4, 4))]);
    let ink = profiles(&[
        ("a:4x4", &["####", "####", "####", "####"]),
        ("b:4x4", &["####", "####", "####", "####"]),
    ]);
    let compose = line(IdcOp::LeftRight, vec![part("a:4x4"), part("b:4x4")]);
    let short = with_clearance((10, 4), &compose, &dims, &ink, 0, 1);
    assert!(
        short
            .iter()
            .any(|w| w.contains("leaves 2 between 'b:4x4' and the right edge")),
        "{short:?}",
    );
    let over = with_clearance((7, 4), &compose, &dims, &ink, 0, 1);
    assert!(
        over.iter()
            .any(|w| w.contains("leaves -1 between 'b:4x4' and the right edge")),
        "{over:?}",
    );
}

#[test]
fn a_part_with_nothing_to_measure_stands_the_check_down() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:4x4", (4, 4))]);
    let compose = line(IdcOp::LeftRight, vec![part("a:4x4"), part("b:4x4")]);
    // `b` is drawn but empty…
    let blank = profiles(&[
        ("a:4x4", &["##..", "##..", "##..", "##.."]),
        ("b:4x4", &["....", "....", "....", "...."]),
    ]);
    assert!(with_clearance((8, 4), &compose, &dims, &blank, 0, 0).is_empty());
    // …and here `b` has no profile at all (a composite, say).
    let missing = profiles(&[("a:4x4", &["##..", "##..", "##..", "##.."])]);
    assert!(with_clearance((8, 4), &compose, &dims, &missing, 0, 0).is_empty());
    // Two parts that share no line where both draw cannot be measured either.
    let disjoint = profiles(&[
        ("a:4x4", &["##..", "##..", "....", "...."]),
        ("b:4x4", &["....", "....", ".###", ".###"]),
    ]);
    assert!(with_clearance((8, 4), &compose, &dims, &disjoint, 0, 0).is_empty());
}

#[test]
fn an_undecided_line_is_not_measured() {
    // The width the slot will be filled with is not chosen yet, so neither is
    // where anything sits: one Todo, and no clearance warning over a layout
    // nobody meant.
    let dims = table(&[("a", (4, 4)), ("b:4x4", (4, 4))]);
    let ink = profiles(&[
        ("a", &["##..", "##..", "##..", "##.."]),
        ("b:4x4", &[".###", ".###", ".###", ".###"]),
    ]);
    let ink_fn = |name: &str| ink.get(name);
    let (_, issues) = expand_compose(
        "test",
        Some((8, 4)),
        1,
        &line(IdcOp::LeftRight, vec![part("a"), part("b:4x4")]),
        &dims,
        Some(&ClearanceRule {
            written: "test*",
            min: 0,
            max: 1,
            ink: &ink_fn,
        }),
    );
    assert_eq!(todos(&issues).len(), 1, "{issues:?}");
    assert!(
        of_severity(&issues, Severity::Warning).is_empty(),
        "{issues:?}"
    );
}
