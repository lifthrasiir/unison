//! Tests for the enclosing operators: offsets, cavities and their measurement.

use super::clearance::clearance_warnings;
use super::*;

// ---------------------------------------------------------------------------
// Enclosures: `⿴⿵⿶⿷⿸⿹⿺⿼⿽`.
// ---------------------------------------------------------------------------

/// A `gap` item, which an enclosure line reads as an offset rather than a gap.
fn at(v: i16) -> ComposeItem {
    ComposeItem::Gap(v)
}

#[test]
fn every_enclosing_operator_knows_which_sides_it_fills() {
    let walls = |c: char| {
        IdcOp::from_char(c)
            .expect("an IDC")
            .walls()
            .expect("enclosing")
    };
    // ⿴ is walled all round; every other enclosure leaves at least one side open.
    assert_eq!(
        walls('\u{2FF4}'),
        Walls {
            left: true,
            right: true,
            top: true,
            bottom: true
        },
    );
    // ⿷ 匚 opens to the right, ⿼ to the left.
    assert_eq!(walls('\u{2FF7}').along(true), (true, false));
    assert_eq!(walls('\u{2FFC}').along(true), (false, true));
    // ⿵ 冂 opens below, ⿶ 凵 above.
    assert_eq!(walls('\u{2FF5}').along(false), (true, false));
    assert_eq!(walls('\u{2FF6}').along(false), (false, true));
    // The four corners fill exactly two adjacent sides.
    assert_eq!(
        walls('\u{2FF8}'),
        Walls {
            left: true,
            right: false,
            top: true,
            bottom: false
        }
    );
    assert_eq!(
        walls('\u{2FF9}'),
        Walls {
            left: false,
            right: true,
            top: true,
            bottom: false
        }
    );
    assert_eq!(
        walls('\u{2FFA}'),
        Walls {
            left: true,
            right: false,
            top: false,
            bottom: true
        }
    );
    assert_eq!(
        walls('\u{2FFD}'),
        Walls {
            left: false,
            right: true,
            top: false,
            bottom: true
        }
    );
    // Never open on both sides of one axis: an enclosure always has a wall to
    // be measured against, which is what makes the axis's sum a property of
    // the parts alone.
    for c in "\u{2FF4}\u{2FF5}\u{2FF6}\u{2FF7}\u{2FF8}\u{2FF9}\u{2FFA}\u{2FFC}\u{2FFD}".chars() {
        let w = walls(c);
        assert!(w.open_count(true) <= 1 && w.open_count(false) <= 1, "{c}");
        let op = IdcOp::from_char(c).expect("an IDC");
        assert_eq!(op.arity(), 2, "{c}");
        assert!(op.enclosing(), "{c}");
        // No `l`/`r`/`u`/`d` slot: an outer and an inner part are not shares of
        // an axis. See `enclosure_rank`.
        assert_eq!(op.slot_direction(0), None, "{c}");
        assert_eq!(op.slot_direction(1), None, "{c}");
    }
    // `⿻` overlaid, `⿾` mirrored and `⿿` rotated are not compositions this
    // module lays out, so they are not IDCs here at all.
    for c in "\u{2FFB}\u{2FFE}\u{2FFF}".chars() {
        assert!(IdcOp::from_char(c).is_none(), "{c}");
    }
    // A one-dimensional operator has no walls and keeps its axis.
    assert!(IdcOp::LeftRight.walls().is_none());
    assert!(!IdcOp::LeftRight.enclosing());
}

#[test]
fn an_enclosure_places_the_inner_part_at_the_offsets_the_line_writes() {
    let dims = table(&[("o:6x6.4x4", (6, 6)), ("i:2x2", (2, 2))]);
    let (refs, issues) = expand(
        Some((6, 6)),
        &line(
            IdcOp::Surround,
            vec![part("o:6x6.4x4"), part("i:2x2"), at(2), at(3)],
        ),
        &dims,
    );
    assert!(errors(&issues).is_empty(), "{issues:?}");
    // The outer part fills the glyph and so sits at the origin; the inner one
    // sits exactly where the line put it — the numbers are its own top-left
    // offsets, not the room left beside it.
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].offset, Some((0, 0)));
    assert_eq!(refs[1].offset, Some((2, 3)));
}

#[test]
fn an_enclosure_offset_is_scaled_like_every_other_derived_ref() {
    let dims = table(&[("o:6x6.4x4", (6, 6)), ("i:2x2", (2, 2))]);
    let compose = line(
        IdcOp::Surround,
        vec![part("o:6x6.4x4"), part("i:2x2"), at(2), at(3)],
    );
    let (refs, _) = expand_compose("test", Some((6, 6)), scaled(2), &compose, &dims, None, None);
    assert_eq!(refs[1].offset, Some((4, 6)));
}

#[test]
fn an_enclosure_outer_part_must_be_exactly_the_glyph() {
    let dims = table(&[("o:5x6.3x4", (5, 6)), ("i:2x2", (2, 2))]);
    let issues = expand(
        Some((6, 6)),
        &line(
            IdcOp::Surround,
            vec![part("o:5x6.3x4"), part("i:2x2"), at(1), at(1)],
        ),
        &dims,
    )
    .1;
    // Not `fits_axis`'s "shorter than the glyph" but an equality: an enclosure
    // whose outer part is smaller than the glyph has walls that are not the
    // glyph's, and the cavity it offers is measured against the wrong box.
    assert_eq!(errors(&issues).len(), 1, "{issues:?}");
    assert!(errors(&issues)[0].contains("5x6"), "{issues:?}");
}

#[test]
fn an_enclosure_inner_part_must_fit_the_glyph() {
    let dims = table(&[("o:6x6.4x4", (6, 6)), ("i:7x2", (7, 2))]);
    let issues = expand(
        Some((6, 6)),
        &line(
            IdcOp::Surround,
            vec![part("o:6x6.4x4"), part("i:7x2"), at(0), at(1)],
        ),
        &dims,
    )
    .1;
    assert_eq!(errors(&issues).len(), 1, "{issues:?}");
    assert!(errors(&issues)[0].contains("7x2"), "{issues:?}");
}

#[test]
fn an_enclosure_line_with_no_offsets_is_a_todo() {
    let dims = table(&[("o:6x6.4x4", (6, 6)), ("i:2x2", (2, 2))]);
    // Nothing has been decided about where the inner part goes, which is the
    // ordinary state of a line populated from IDS — not a placement of (0, 0),
    // which would wedge the inner part into the corner of the walls.
    let issues = expand(
        Some((6, 6)),
        &line(IdcOp::Surround, vec![part("o:6x6.4x4"), part("i:2x2")]),
        &dims,
    )
    .1;
    assert!(errors(&issues).is_empty(), "{issues:?}");
    assert_eq!(todos(&issues).len(), 1, "{issues:?}");
    // Half a placement is not a placement.
    let issues = expand(
        Some((6, 6)),
        &line(
            IdcOp::Surround,
            vec![part("o:6x6.4x4"), part("i:2x2"), at(1)],
        ),
        &dims,
    )
    .1;
    assert_eq!(errors(&issues).len(), 1, "{issues:?}");
}

#[test]
fn an_enclosure_warns_when_a_part_is_drawn_for_the_other_slot() {
    let dims = table(&[("o:6x6.4x4", (6, 6)), ("i:6x6.2x2", (6, 6))]);
    // Both names promise a cavity, so the inner slot holds a drawing made to
    // enclose. That is a warning and not an error, exactly as a `-r` variant
    // in a `⿰`'s left slot is: it may still be what the author wanted.
    let issues = expand(
        Some((6, 6)),
        &line(
            IdcOp::Surround,
            vec![part("o:6x6.4x4"), part("i:6x6.2x2"), at(0), at(0)],
        ),
        &dims,
    )
    .1;
    let warnings = of_severity(&issues, Severity::Warning);
    assert!(
        warnings.iter().any(|w| w.contains("i:6x6.2x2")),
        "{issues:?}"
    );
}

#[test]
fn an_enclosure_reads_the_cavity_promise_off_the_name_as_written() {
    // `i:6x6` is an alias of a drawing made to enclose, and the line names the
    // alias: the author asked for a part that promises nothing, and what the
    // alias happens to point at is not a claim they made. Exactly as with the
    // `-l`/`-r` position on a split's name, the promise is read off the name as
    // written, so there is nothing here to warn about.
    let dims = table(&[("o:6x6.4x4", (6, 6)), ("i:6x6.2x2", (6, 6))]);
    let issues = expand(
        Some((6, 6)),
        &line(
            IdcOp::Surround,
            vec![
                part("o:6x6.4x4"),
                aliased("i:6x6", "i:6x6.2x2"),
                at(0),
                at(0),
            ],
        ),
        &dims,
    )
    .1;
    assert!(
        of_severity(&issues, Severity::Warning).is_empty(),
        "{issues:?}"
    );
}

/// `⿷` 匚: three walls and an opening to the right. The horizontal axis has one
/// clearance to the wall's inner face and one to the glyph's own edge; the
/// vertical axis has two inner ones. Four in all, and each axis's sum is a
/// property of the parts, not of where the line puts them.
#[test]
fn an_enclosure_is_measured_against_the_walls_and_the_open_edges() {
    // A 6x6 匚 with a one-cell wall: the cavity is columns 1..5, rows 1..4.
    let outer = whole(
        &grid(&["######", "#.....", "#.....", "#.....", "#.....", "######"]),
        1,
    );
    let inner = whole(&grid(&["##", "##"]), 1);
    let measure = |at| {
        crate::compose::measure_enclosure_clearances(
            IdcOp::SurroundLeft.walls().expect("enclosing"),
            (6, 6),
            ("o", &outer, (0, 0)),
            ("i", &inner),
            at,
            None,
        )
        .expect("both parts draw")
    };
    let values = |at| measure(at).iter().map(|c| c.value).collect::<Vec<_>>();
    // Placed at (2, 2): 1 from the left wall's inner face, 2 to the right edge,
    // 1 from the top wall and 1 to the bottom one.
    assert_eq!(values((2, 2)), vec![1, 2, 1, 1]);
    // One cell right and one down moves each axis's pair in opposite
    // directions, and leaves both sums where they were.
    assert_eq!(values((3, 3)), vec![2, 1, 2, 0]);
    let sums = |at: (i32, i32)| {
        let c = measure(at);
        let axis = |h: bool| {
            c.iter()
                .filter(|c| c.horizontal == h)
                .map(|c| c.value)
                .sum::<i32>()
        };
        (axis(true), axis(false))
    };
    assert_eq!(sums((2, 2)), sums((3, 3)));
    assert_eq!(sums((2, 2)), sums((1, 1)));

    // Which of the four touch the glyph's own boundary: only the open side.
    let at_edge: Vec<bool> = measure((2, 2)).iter().map(|c| c.at_edge).collect();
    assert_eq!(at_edge, vec![false, true, false, false]);
}

/// `⿴` 囗 is walled all round, so none of its four clearances touches the
/// glyph's boundary and every one of them is measured against the ring.
#[test]
fn a_full_surround_measures_every_side_against_the_ring() {
    let outer = whole(
        &grid(&["######", "#....#", "#....#", "#....#", "#....#", "######"]),
        1,
    );
    let inner = whole(&grid(&["##", "##"]), 1);
    let c = crate::compose::measure_enclosure_clearances(
        IdcOp::Surround.walls().expect("enclosing"),
        (6, 6),
        ("o", &outer, (0, 0)),
        ("i", &inner),
        (2, 2),
        None,
    )
    .expect("both parts draw");
    assert_eq!(
        c.iter().map(|c| c.value).collect::<Vec<_>>(),
        vec![1, 1, 1, 1]
    );
    assert!(c.iter().all(|c| !c.at_edge));
}

#[test]
fn a_cavity_must_be_flush_with_the_sides_the_operator_opens() {
    let walls = |c: char| {
        IdcOp::from_char(c)
            .expect("an IDC")
            .walls()
            .expect("enclosing")
    };
    // 广: a top bar and a stroke down the left, opening right and below.
    let guang = whole(
        &grid(&["######", "#.....", "#.....", "#.....", "#.....", "#....."]),
        1,
    );
    // The cavity is the 5x5 block at the bottom right, so anything up to that
    // fits — flush against both open sides.
    assert!(cavity_fits(
        &guang,
        walls('\u{2FF8}'),
        (6, 6),
        (5, 5),
        (0, 0)
    ));
    assert!(cavity_fits(
        &guang,
        walls('\u{2FF8}'),
        (6, 6),
        (3, 2),
        (0, 0)
    ));
    // One cell wider or taller than the drawing leaves, and it does not.
    assert!(!cavity_fits(
        &guang,
        walls('\u{2FF8}'),
        (6, 6),
        (6, 5),
        (0, 0)
    ));
    assert!(!cavity_fits(
        &guang,
        walls('\u{2FF8}'),
        (6, 6),
        (5, 6),
        (0, 0)
    ));

    // 匚: walled top and bottom, open right. The rectangle is flush right but
    // free to sit anywhere down the axis, so a 5x4 fits where a 5x5 does not.
    let fang = whole(
        &grid(&["######", "#.....", "#.....", "#.....", "#.....", "######"]),
        1,
    );
    assert!(cavity_fits(
        &fang,
        walls('\u{2FF7}'),
        (6, 6),
        (5, 4),
        (0, 0)
    ));
    assert!(!cavity_fits(
        &fang,
        walls('\u{2FF7}'),
        (6, 6),
        (5, 5),
        (0, 0)
    ));

    // 囗: walled all round, so the rectangle is free both ways — and bounded
    // both ways.
    let wei = whole(
        &grid(&["######", "#....#", "#....#", "#....#", "#....#", "######"]),
        1,
    );
    assert!(cavity_fits(&wei, walls('\u{2FF4}'), (6, 6), (4, 4), (0, 0)));
    assert!(!cavity_fits(
        &wei,
        walls('\u{2FF4}'),
        (6, 6),
        (5, 4),
        (0, 0)
    ));

    // A hardblank is wall: it is space the source keeps clear of whatever goes
    // inside, so it takes room out of the cavity exactly as ink does.
    let claimed = whole(
        &grid(&["######", "#$....", "#$....", "#$....", "#$....", "#$...."]),
        1,
    );
    assert!(cavity_fits(
        &claimed,
        walls('\u{2FF8}'),
        (6, 6),
        (4, 5),
        (0, 0)
    ));
    assert!(!cavity_fits(
        &claimed,
        walls('\u{2FF8}'),
        (6, 6),
        (5, 5),
        (0, 0)
    ));
}

/// The cavity a name promises is a *lower bound*: a drawing more generous than
/// its name is fine, one that cannot keep the promise is a warning.
#[test]
fn an_outer_part_that_cannot_keep_its_cavity_promise_warns() {
    let dims = table(&[("o:6x6.5x5", (6, 6)), ("i:2x2", (2, 2))]);
    let profiles = profiles(&[
        // A 广 whose left stroke is two cells wide leaves only 4 columns.
        (
            "o:6x6.5x5",
            &["######", "##....", "##....", "##....", "##....", "##...."],
        ),
        ("i:2x2", &["##", "##"]),
    ]);
    let ink = |name: &str| profiles.get(name);
    let band = band(0, 2);
    let rule = ClearanceRule {
        written: "test*",
        band: &band,
        ink: &ink,
        max_contact_run: None,
        contact_written: "test*",
    };
    let (_, issues) = expand_compose(
        "test",
        Some((6, 6)),
        UNIT,
        &line(
            IdcOp::SurroundUpperLeft,
            vec![part("o:6x6.5x5"), part("i:2x2"), at(3), at(3)],
        ),
        &dims,
        None,
        Some(&rule),
    );
    assert!(errors(&issues).is_empty(), "{issues:?}");
    let warnings = of_severity(&issues, Severity::Warning);
    assert!(
        warnings.iter().any(|w| w.contains("5x5 cavity")),
        "{issues:?}"
    );
}

/// The whole pipeline over an inline enclosure source: the line parses, derives
/// its two `ref`s, and is measured — which is what says the enclosure reaches
/// the build the same way a split does.
#[test]
fn an_enclosure_line_survives_the_whole_pipeline() {
    const SRC: &str = "\
audit ideal-clearance test-* 0 1 1 2

glyph ring:6x6.4x4 6 6
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
\u{2FF4} ring:6x6.4x4 seed:2x2 2 2
";
    let doc = crate::document_io::parse_document_from_str(SRC, "test.unf".into()).unwrap();
    let r = crate::resolve::Resolution::compute(&[&doc]);
    let hard: Vec<&str> = r
        .expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.as_str())
        .collect();
    assert!(hard.is_empty(), "{hard:?}");
    // Every clearance is 1, which the enclosure band `1 2` admits, so the line
    // is silent — and the *linear* band `0 1` it would have been held to under
    // one pair is not what was read.
    assert_eq!(clearance_warnings(SRC), Vec::<String>::new());

    // Wedged into the corner instead: the two clearances the inner part leaves
    // behind it go to 0 and the two ahead of it to 2, so both axes warn twice.
    // The IDC line's own offsets, not the `glyph` header that reads alike.
    let wedged = SRC.replace(
        "\u{2FF4} ring:6x6.4x4 seed:2x2 2 2",
        "\u{2FF4} ring:6x6.4x4 seed:2x2 1 1",
    );
    let warnings = clearance_warnings(&wedged);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings.iter().all(|w| w.contains("leaves 0 between")),
        "{warnings:?}"
    );
}

/// A wall is the run *nearest the middle of the box*, not the line's first run.
///
/// Every han part writes its side bearing as a detached hardblank column at the
/// box's edge, so the first run of nearly every line is a bearing and not a
/// wall. Measuring a cavity against it swallows the wall itself: the `⿸` below
/// read 0 where the inner part is a cell into the wall, and a fixer would have
/// placed it there.
#[test]
fn a_wall_is_the_run_beside_the_cavity_and_not_the_side_bearing() {
    // A 广 drawn the way the source draws every part: a hardblank bearing at
    // cell 0, and the wall itself at cell 2.
    let guang = whole(
        &grid(&["$#####", "$.#...", "$.#...", "$.#...", "$.#...", "$.#..."]),
        1,
    );
    let row = guang.rows[3].expect("the row draws");
    assert_eq!(row.near, 0, "the bearing is still the line's near end");
    assert_eq!(
        row.low_wall.expect("a wall below the middle").at,
        2,
        "but the wall a cavity sees is the stroke, not the bearing",
    );

    let seed = whole(&grid(&["##", "##"]), 1);
    let walls = IdcOp::SurroundUpperLeft.walls().expect("enclosing");
    let measure = |at| {
        crate::compose::measure_enclosure_clearances(
            walls,
            (6, 6),
            ("o", &guang, (0, 0)),
            ("i", &seed),
            at,
            None,
        )
        .expect("both parts draw")
        .iter()
        .map(|c| c.value)
        .collect::<Vec<_>>()
    };
    // At (1, 1) the seed's left column is the wall's own: an overlap, not room.
    assert_eq!(measure((1, 1))[0], -2);
    // Clear of it at (3, 1): one cell from the wall, two to the open edge.
    assert_eq!(measure((3, 1))[0], 0);

    // The cavity a name may promise is bounded the same way, so the promise and
    // the measurement cannot disagree: three columns clear, not five.
    assert!(cavity_fits(&guang, walls, (6, 6), (3, 5), (0, 0)));
    assert!(!cavity_fits(&guang, walls, (6, 6), (4, 5), (0, 0)));
}
