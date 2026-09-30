//! Tests for clearance measurement over ink profiles, contact runs and nested parts.

use super::*;

// ---------------------------------------------------------------- clearance

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
    let p = InkProfile::of(&g, 1, (0, 0), (1, 0), (4, 2));
    assert_eq!(p.rows[0].expect("row 0 is occupied").near, 1);
    assert_eq!(p.rows[0].expect("row 0 is occupied").far, 2);
    assert_eq!(p.cols.len(), 4, "the profile is the box's width");

    // The same grid measured as itself, for contrast.
    let as_grid = InkProfile::of(&g, 1, (0, 0), (0, 0), (6, 2));
    assert_eq!(as_grid.rows[0].expect("row 0 is occupied").near, 2);

    // Ink before the box's own corner keeps the coordinate it is drawn at.
    let escaping = InkProfile::of(&grid(&["#....."]), 1, (0, 0), (2, 0), (4, 1));
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

/// `expand`, holding the line to `min..max`.
fn with_clearance(
    parent: (u16, u16),
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    profiles: &crate::hash::HashMap<String, InkProfile>,
    min: i16,
    max: i16,
) -> Vec<String> {
    let ink = |name: &str| profiles.get(name);
    let band = band(min, max);
    let rule = ClearanceRule {
        written: "test*",
        band: &band,
        ink: &ink,
        max_contact_run: None,
        contact_written: "test*",
    };
    let (_, issues) = expand_compose("test", Some(parent), UNIT, compose, dims, None, Some(&rule));
    assert!(errors(&issues).is_empty(), "{issues:?}");
    // A clearance finding is a `Severity::Chore`, not a warning: it says the
    // same thing, and it is only a build's *log* that is spared it.
    of_severity(&issues, Severity::Chore)
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[test]
fn ink_profile_reads_both_frontiers_and_counts_a_hardblank() {
    let ink = |near, far| Some(one_run(near, far, 0, 0));
    let p = whole(&grid(&[".##.", "....", "$..#"]), 1);
    assert_eq!(
        p.rows,
        vec![
            ink(1, 2),
            None,
            // The lone `$` is the near frontier, and a run of one — so this
            // is a line of *two* runs, and its inner faces are the `$`'s far
            // side and the `#`'s near side rather than the line's own ends.
            InkLine::from_runs(&[(0, 0, 1, 1), (3, 3, 0, 0)], 2),
        ],
    );
    // Read down instead, the box is 3 tall and the pivot is row 1, so a run
    // that does not reach it is a wall on one side only: there is no drawing
    // on the other side of the cavity for anything to be measured against.
    let col = |runs: &[(i32, i32, u16, u16)]| InkLine::from_runs(runs, 1);
    assert_eq!(
        p.cols,
        vec![
            col(&[(2, 2, 1, 1)]),
            col(&[(0, 0, 0, 0)]),
            col(&[(0, 0, 0, 0)]),
            col(&[(2, 2, 0, 0)]),
        ],
    );
    assert_eq!(p.cols[0].unwrap().low_wall, None);
    assert_eq!(
        p.cols[0].unwrap().high_wall,
        Some(WallFace {
            at: 2,
            hardblanks: 1,
            run: 1
        }),
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
    assert_eq!(facing(&a, &b, true), Some(-3));
    // Only the shared part counts: a row whose other side has none is measured
    // as before, and one row is enough to hold the whole line back.
    let plain = whole(&grid(&["####", "$$##"]), 1);
    assert_eq!(facing(&a, &plain, true), Some(-4));
    // The reach is the whole facing run, not one cell of it.
    let deep_a = whole(&grid(&["##$$", "#$$$"]), 1);
    let deep_b = whole(&grid(&["$$##", "$$$#"]), 1);
    assert_eq!(facing(&deep_a, &deep_b, true), Some(-2));
    // A hardblank pointing the other way is not on this side.
    let away = whole(&grid(&["###$", "###$"]), 1);
    assert_eq!(facing(&a, &away, true), Some(-4));
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
    let ink: crate::hash::HashMap<String, InkProfile> = [
        (
            "a:3x1".to_string(),
            InkProfile::of(&grid(&["$##$"]), 1, (0, 0), (0, 0), (3, 1)),
        ),
        (
            "b:3x1".to_string(),
            InkProfile::of(&grid(&["$###$"]), 1, (0, 0), (1, 0), (3, 1)),
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

/// What [`contact_run`] measures, and what [`contact_demand`] makes of it.
#[test]
fn a_contact_run_is_the_longest_seam_two_edges_share() {
    // Flat faces: they run together over every line they share.
    let flat = whole(&grid(&["####", "####", "####"]), 1);
    let facing = whole(&grid(&["####", "####", "####"]), 1);
    assert_eq!(contact(&flat, &facing, true, 4), 3);
    assert_eq!(contact(&flat, &facing, true, 5), 0, "drawn apart");
    assert_eq!(contact(&flat, &facing, true, 3), 3, "overlapping");

    // A tip: the seam is one line long however flat the other side is.
    let tip = whole(&grid(&["...#", "####", "...#"]), 1);
    assert_eq!(contact(&flat, &tip, true, 4), 1);

    // Two lines that touch with a third between them are two seams, not one.
    let notched = whole(&grid(&["####", "...#", "####"]), 1);
    let reversed = whole(&grid(&["####", "#...", "####"]), 1);
    assert_eq!(contact(&reversed, &notched, true, 4), 1);

    // A line one of them draws nothing on has no contact on it.
    let gapped = whole(&grid(&["####", "....", "####"]), 1);
    assert_eq!(contact(&flat, &gapped, true, 4), 1);

    // A hardblank holds the ink back, so the same edges no longer meet.
    let claimed = whole(&grid(&["###$", "###$", "###$"]), 1);
    assert_eq!(contact(&claimed, &facing, true, 4), 0);
    assert_eq!(contact(&claimed, &facing, true, 3), 3);

    // The demand is read where the ink *would* meet, so it is the same however
    // the line places the parts — and a hardblank that already holds them apart
    // is the rule's answer rather than a second claim on top of it.
    let demand = |a: &InkProfile, b: &InkProfile, max| {
        contact_demand(GapSide::linear(a), GapSide::linear(b), true, max)
            .expect("both draw on some line")
    };
    assert_eq!(demand(&flat, &facing, 2), ContactDemand { run: 3, owed: 1 });
    assert_eq!(demand(&flat, &facing, 3), ContactDemand { run: 3, owed: 0 });
    assert_eq!(demand(&flat, &tip, 2), ContactDemand { run: 1, owed: 0 });
    assert_eq!(
        demand(&claimed, &facing, 2),
        ContactDemand { run: 3, owed: 0 }
    );
}

/// A contact is between two *contours*, not two cells: a frontier cell whose
/// ink never reaches the side facing the neighbour touches nothing there, and
/// two that reach opposite halves of the boundary they share pass each other.
#[test]
fn a_contact_is_where_the_ink_meets_and_not_where_the_cells_do() {
    let flat = whole(&grid(&["####", "####", "####"]), 1);
    let facing = whole(&grid(&["####", "####", "####"]), 1);
    assert_eq!(contact(&flat, &facing, true, 4), 3);

    // `/` inks its cell and covers none of its right edge, so the cells abut
    // and the ink does not.
    let tapered = whole(&grid(&["###/", "###/", "###/"]), 1);
    assert_eq!(contact(&tapered, &facing, true, 4), 0);
    // The cells are still what an overlap is measured by.
    assert_eq!(contact(&tapered, &facing, true, 3), 3);

    // Opposite halves of the same boundary: both cells are inked, the two
    // frontiers meet, and the contours miss each other.
    let upper = whole(&grid(&["###^", "###^", "###^"]), 1);
    let lower = whole(&grid(&["v###", "v###", "v###"]), 1);
    assert_eq!(contact(&upper, &lower, true, 4), 0);
    // Against a flat edge each of them shares half the boundary, which is a
    // seam like any other.
    assert_eq!(contact(&upper, &facing, true, 4), 3);
    assert_eq!(contact(&flat, &lower, true, 4), 3);

    // And so the demand follows the ink: no cell is owed for a seam that is
    // not there.
    assert_eq!(demand(&tapered, &facing, true, 2).map(|d| d.owed), Some(0),);
    assert_eq!(demand(&flat, &facing, true, 2).map(|d| d.owed), Some(1),);
}

/// `expand`, holding the line to `min..max` and to a contact-run rule.
fn with_contact_run(
    parent: (u16, u16),
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    profiles: &crate::hash::HashMap<String, InkProfile>,
    max_contact_run: Option<u16>,
) -> Vec<String> {
    let ink = |name: &str| profiles.get(name);
    let band = band(0, 1);
    let rule = ClearanceRule {
        written: "test*",
        band: &band,
        ink: &ink,
        max_contact_run,
        contact_written: "test*",
    };
    let (_, issues) = expand_compose("test", Some(parent), UNIT, compose, dims, None, Some(&rule));
    assert!(errors(&issues).is_empty(), "{issues:?}");
    of_severity(&issues, Severity::Chore)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The whole of the contact rule: two parts that touch over a long run are held
/// a cell apart, two that touch over a short one are not, and a hardblank that
/// has already parted them says nothing more.
#[test]
fn a_long_contact_run_costs_a_clearance() {
    let dims = table(&[("a:4x4", (4, 4)), ("b:4x4", (4, 4)), ("c:4x4", (4, 4))]);
    let ink = profiles(&[
        // Flat right edge on every row.
        ("a:4x4", &["####", "####", "####", "####"]),
        // Flat left edge: the two meet over all 4 rows.
        ("b:4x4", &["####", "####", "####", "####"]),
        // Only the top two rows reach the left edge.
        ("c:4x4", &["####", "####", "..##", "..##"]),
        // Like `a`, but the last column is a hardblank rather than ink.
        ("a$:4x4", &["###$", "###$", "###$", "###$"]),
    ]);
    let dims = |name: &str| match name {
        "a$:4x4" => PartDims::Size(4, 4, Margin::default()),
        other => dims(other),
    };
    let touching = line(IdcOp::LeftRight, vec![part("a:4x4"), part("b:4x4")]);
    let grazing = line(IdcOp::LeftRight, vec![part("a:4x4"), part("c:4x4")]);
    let parted = line(IdcOp::LeftRight, vec![part("a$:4x4"), part("b:4x4")]);

    // With no rule stated, nothing about contact is measured at all.
    assert!(with_contact_run((8, 4), &touching, &dims, &ink, None).is_empty());

    // 4 rows of contact, over the ideal 2: the clearance is one less than the
    // ink says, which puts it under the ideal 0..1.
    let warnings = with_contact_run((8, 4), &touching, &dims, &ink, Some(2));
    assert_eq!(warnings.len(), 2, "{warnings:?}"); // the clearance and the total
    assert!(
        warnings[0].contains("run together over 4") && warnings[0].contains("leaves -1"),
        "{warnings:?}"
    );

    // 2 rows is inside the ideal, so the parts may sit against each other.
    assert!(with_contact_run((8, 4), &grazing, &dims, &ink, Some(2)).is_empty());

    // The hardblank has already parted them, so the rule asks for nothing more.
    assert!(with_contact_run((8, 4), &parted, &dims, &ink, Some(2)).is_empty());

    // A line that has *given* the junction its cell still reads 0 there: the
    // space the rule asked for is not room the glyph has left to spend. Written
    // a cell apart in a 10-wide box, that reads 0 at the junction and 1 at the
    // right edge — a total of 1, inside the ideal.
    let given = GlyphCompose {
        op: IdcOp::LeftRight,
        items: vec![
            ComposeItem::Part {
                name: "a:4x4".to_string(),
                raw_name: None,
            },
            ComposeItem::Gap(1),
            ComposeItem::Part {
                name: "b:4x4".to_string(),
                raw_name: None,
            },
        ],
        assumed: false,
        comment: None,
    };
    assert!(with_contact_run((10, 4), &given, &dims, &ink, Some(2)).is_empty());
    // Without the rule that same cell is a cell of slack, and the total says so.
    let loose = with_contact_run((10, 4), &given, &dims, &ink, None);
    assert_eq!(loose.len(), 1, "{loose:?}");
    assert!(loose[0].contains("leaves 2 in total"), "{loose:?}");
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
        UNIT,
        &line(IdcOp::LeftRight, vec![part("a"), part("b:4x4")]),
        &dims,
        None,
        Some(&ClearanceRule {
            written: "test*",
            band: &band(0, 1),
            ink: &ink_fn,
            max_contact_run: None,
            contact_written: "test*",
        }),
    );
    assert_eq!(todos(&issues).len(), 1, "{issues:?}");
    assert!(
        of_severity(&issues, Severity::Warning).is_empty(),
        "{issues:?}"
    );
}

// ------------------------------------------ a part that is itself an IDC line

/// The whole pipeline over an inline source, for the two tests below: the
/// clearance findings, in order.
pub(super) fn clearance_warnings(src: &str) -> Vec<String> {
    let doc = crate::document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let r = crate::resolve::Resolution::compute(&[&doc]);
    r.expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Chore && d.message.contains("outside the ideal"))
        .map(|d| d.message.clone())
        .collect()
}

/// `test-x` is `⿰ left:4x4 nested:4x4`, and `nested:4x4` is itself split by an
/// IDC line. Its drawing exists all the same — the line derives it — so the
/// outer line's clearances are measurable, and the canyon between the two
/// parts is a warning like any other.
///
/// It used to be silently unmeasurable: a part split by a line of its own had
/// no ink profile, and the check stands down for the whole line when one part
/// cannot be measured. So a `⿱艹林` said nothing at all, which reads exactly
/// like a layout nobody need look at.
const NESTED: &str = "\
audit ideal-clearance test-* 0 1

glyph in:2x4-l 2 4
..@@
..@@
..@@
..@@

glyph in:2x4-r 2 4
@@@@
@@@@
@@@@
@@@@

glyph left:4x4 4 4
@@@@....
@@@@....
@@@@....
@@@@....

glyph nested:4x4 4 4
\u{2FF0} in:2x4-l in:2x4-r

glyph test-x 8 4
\u{2FF0} left:4x4 nested:4x4
";

#[test]
fn a_part_that_is_itself_split_is_measured() {
    let warnings = clearance_warnings(NESTED);
    // `left:4x4` inks columns 0..1 and `nested:4x4`, placed at 4, inks 5..7:
    // three cells between them, and the same three in total.
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("leaves 3 between 'left:4x4' and 'nested:4x4'"),
        "{warnings:?}"
    );
    assert!(warnings[1].contains("leaves 3 in total"), "{warnings:?}");
}

/// One part of the nested line has not picked its variant, so the nested
/// drawing is not one the source has decided on — and half a layout measured is
/// worse than none. The outer line stands down, as it does for any part it
/// cannot measure.
#[test]
fn an_undecided_nested_part_is_still_not_measured() {
    let src = NESTED.replace(
        "\u{2FF0} in:2x4-l in:2x4-r",
        "\u{2FF0} in-a in:2x4-r\n\nglyph in-a 2 4\n..@@\n..@@\n..@@\n..@@",
    );
    assert!(clearance_warnings(&src).is_empty(), "{src}");
}
