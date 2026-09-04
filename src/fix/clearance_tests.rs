use super::*;
use crate::document_io::parse_document_from_str;

/// `arrange` and `score` alone, with no source behind them.
mod arithmetic {
    use super::*;

    #[test]
    fn a_total_that_fits_is_laid_out_edges_first() {
        // Three clearances, room for one cell of slack: the parts go out
        // against the box and the slack lands between them.
        assert_eq!(arrange(3, 1, 0, 1), vec![0, 1, 0]);
        assert_eq!(arrange(3, 0, 0, 1), vec![0, 0, 0]);
        // Two cells of slack, one edge has to take one: the near one, by the
        // lexicographic rule.
        assert_eq!(arrange(3, 2, 0, 1), vec![0, 1, 1]);
        assert_eq!(arrange(3, 3, 0, 1), vec![1, 1, 1]);
        for total in -4..=6 {
            assert_eq!(score(&arrange(3, total, 0, 1), 0, 1), least(3, total, 0, 1));
        }
    }

    /// Four clearances: the two inner ones are evened out before the
    /// lexicographic rule ever gets a say.
    #[test]
    fn three_parts_even_out_the_middle() {
        assert_eq!(arrange(4, 2, 0, 1), vec![0, 1, 1, 0]);
        assert_eq!(arrange(4, 3, 0, 1), vec![0, 1, 1, 1]);
        assert_eq!(arrange(4, 1, 0, 1), vec![0, 0, 1, 0]);
        assert_eq!(arrange(4, 4, 0, 1), vec![1, 1, 1, 1]);
    }

    /// Parts too fat for the box: the least the layout can be outside the
    /// range is the shortfall, and no arrangement does better.
    #[test]
    fn parts_that_do_not_fit_are_as_close_as_the_arithmetic_allows() {
        let out = arrange(3, -3, 0, 1);
        assert_eq!(out.iter().sum::<i32>(), -3);
        assert_eq!(score(&out, 0, 1), 3 + 3, "the three plus the total's own");
        // Parts far too thin: the edges take the maximum and the middle swells.
        let out = arrange(3, 9, 0, 1);
        assert_eq!(out, vec![1, 7, 1]);
        assert_eq!(score(&out, 0, 1), 6 + 8);
    }

    /// The exhaustive answer, for ranges and totals across the interesting band.
    #[test]
    fn arrange_is_the_least_and_the_most_even() {
        for n in 3..=4usize {
            for total in -6..=10 {
                for (lo, hi) in [(0, 1), (0, 0), (-1, 1), (1, 2)] {
                    let ours = arrange(n, total, lo, hi);
                    assert_eq!(ours.len(), n);
                    assert_eq!(ours.iter().sum::<i32>(), total);
                    let best = brute_force(n, total, lo, hi);
                    assert_eq!(ours, best, "n={n} total={total} range={lo}..{hi}");
                }
            }
        }
    }

    /// The least cost `n` clearances summing to `total` can have.
    fn least(n: usize, total: i32, lo: i32, hi: i32) -> i32 {
        let n = n as i32;
        let spread = if total < n * lo {
            n * lo - total
        } else if total > n * hi {
            total - n * hi
        } else {
            0
        };
        spread + distance(total, lo, hi)
    }

    /// Every arrangement in a window wide enough to hold the answer, ordered by
    /// the module's rules — the definition `arrange` is a shortcut for.
    fn brute_force(n: usize, total: i32, lo: i32, hi: i32) -> Vec<i32> {
        // Wide enough to hold every answer for the totals tested here; the
        // free clearances are counted off as digits in that window.
        let (from, to) = (-12i32, 12i32);
        let width = (to - from + 1) as usize;
        let free = n - 1;
        let mut best: Option<Vec<i32>> = None;
        // The last clearance is whatever the others leave, so only `free` of
        // them are enumerated.
        for mut counter in 0..width.pow(free as u32) {
            let mut candidate = Vec::with_capacity(n);
            for _ in 0..free {
                candidate.push(from + (counter % width) as i32);
                counter /= width;
            }
            let last = total - candidate.iter().sum::<i32>();
            if last < from || last > to {
                continue;
            }
            candidate.push(last);
            let key = |c: &Vec<i32>| {
                (
                    score(c, lo, hi),
                    c[0] + c[n - 1],
                    if n == 4 { (c[1] - c[2]).abs() } else { 0 },
                    c.clone(),
                )
            };
            if best.as_ref().is_none_or(|b| key(&candidate) < key(b)) {
                best = Some(candidate);
            }
        }
        best.expect("some arrangement is always in the window")
    }
}

/// A source built inline, as every test here is: `font/` is downstream data and
/// no test may read it.
fn plan(src: &str) -> Vec<ClearanceFix> {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    optimize_clearance(&[&doc])
        .into_iter()
        .flat_map(|f| f.fixes)
        .collect()
}

/// Two parts in an 8x4 box, `a` drawn at its left and `b` inset by one, so
/// there is nothing at either edge and a canyon in the middle:
///
/// ```text
/// a:4x4  ##..     a:3x4  #..     b:4x4  .###
/// ```
const TWO_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph a:4x4 4 4
@@@@....
@@@@....
@@@@....
@@@@....

glyph a:3x4 3 4
@@....
@@....
@@....
@@....

glyph b:4x4 4 4
..@@@@@@
..@@@@@@
..@@@@@@
..@@@@@@

glyph test-x 8 4
\u{2FF0} a:4x4 b:4x4
";

#[test]
fn a_line_that_warns_is_rewritten_by_its_gaps_alone() {
    let fixes = plan(TWO_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!(fix.glyph, "test-x");
    assert_eq!(fix.old_line, "\u{2FF0} a:4x4 b:4x4");
    // 0/3/0 as written: the middle is 2 outside the range and so is the total.
    assert_eq!(fix.before, Some(4));
    // Both parts move outwards: 1/1/1, and the total — which no arrangement
    // can change — is all that is left to warn about.
    assert_eq!(fix.after, 2);
    assert_eq!(fix.new_line, "\u{2FF0} 1 a:4x4 -2 b:4x4");
}

/// Two flat faces in a box with one cell to spare. Nothing warns about the
/// distance between them — they sit at 0, inside the range — but they run
/// together over the whole of both edges, so `max-contact-run` takes the cell
/// they were leaning on and the optimizer spends the spare one parting them.
const FLAT_FACES: &str = "\
audit ideal-clearance test-* 0 1
audit max-contact-run test-* 2

glyph a:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph b:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 9 4
\u{2FF0} a:4x4 b:4x4
";

#[test]
fn a_long_contact_is_worth_the_spare_cell() {
    let fixes = plan(FLAT_FACES);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    // 0/0/1 as written, and the middle one is reported a cell short because the
    // two faces touch over all 4 lines.
    assert_eq!((fixes[0].before, fixes[0].after), (Some(1), 0));
    assert_eq!(fixes[0].new_line, "\u{2FF0} a:4x4 1 b:4x4");

    // A rule that tolerates the run says nothing, and neither does no rule.
    let slack = FLAT_FACES.replace("max-contact-run test-* 2", "max-contact-run test-* 4");
    assert!(plan(&slack).is_empty(), "4 lines is inside the ideal 4");
    let none = FLAT_FACES.replace("audit max-contact-run test-* 2\n", "");
    assert!(plan(&none).is_empty(), "no rule, nothing measured");
}

#[test]
fn a_line_inside_the_range_is_left_alone() {
    let wide = TWO_PARTS.replace("ideal-clearance test-* 0 1", "ideal-clearance test-* 0 3");
    assert!(plan(&wide).is_empty(), "nothing warns, so nothing is fixed");
    // And so is a glyph no rule reaches.
    assert!(plan(&TWO_PARTS.replace("test-*", "other-*")).is_empty());
}

/// The narrower `a:3x4` leaves a total the gaps cannot rescue; the wider
/// variant is the only thing that changes it, so the search finds it.
#[test]
fn a_wider_variant_is_chosen_when_the_gaps_cannot_do_it() {
    let src = TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:3x4 b:4x4");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (Some(5), 2));
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:4x4 -2 b:4x4");
}

/// A variant drawn for the other side of the glyph is not an alternative, so
/// the same line is fixed by its gaps alone and keeps the part it had.
#[test]
fn a_variant_for_the_wrong_slot_is_not_a_candidate() {
    let src = TWO_PARTS
        .replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:3x4 b:4x4")
        .replace("glyph a:4x4 4 4", "glyph a:4x4-r 4 4");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (Some(5), 4));
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:3x4 -1 b:4x4");
}

/// Two full parts filling an 8x4 box exactly, so every clearance is already
/// 0 and no arrangement of the gaps has anything to say. `p` is drawn twice
/// under the same base, once marked `-l` and once unmarked.
const DIRECTED_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph p:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph p:4x4-l 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph q:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 8 4
\u{2FF0} q:4x4 p:4x4-l
";

/// A component drawn for the other side of the glyph warns even when the
/// clearances are perfect, and the search has to reach it: the score alone says
/// nothing is wrong here.
#[test]
fn a_component_in_the_wrong_slot_is_swapped_for_an_undirected_one() {
    let fixes = plan(DIRECTED_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!(fix.old_line, "\u{2FF0} q:4x4 p:4x4-l");
    assert_eq!(fix.new_line, "\u{2FF0} q:4x4 p:4x4");
    assert_eq!(
        (fix.before, fix.after),
        (Some(0), 0),
        "the clearances were never the problem",
    );
    assert_eq!(fix.mismatched, Some((1, 0)));
}

/// The same line with nothing else to put in the slot: the mismatch stands,
/// since the only answer is the one already written.
#[test]
fn a_wrong_slot_with_no_alternative_keeps_its_warning() {
    let src = DIRECTED_PARTS.replace("glyph p:4x4 4 4", "glyph z:4x4 4 4");
    let fixes = plan(&src);
    assert!(fixes.is_empty(), "{fixes:?}");
}

/// A directed name that *does* suit its slot is what the tie-break prefers, so
/// the search takes it over the unmarked twin.
#[test]
fn a_name_drawn_for_the_slot_is_preferred() {
    let src =
        format!("{DIRECTED_PARTS}\nglyph p:4x4-r 4 4\n@@@@@@@@\n@@@@@@@@\n@@@@@@@@\n@@@@@@@@\n");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].new_line, "\u{2FF0} q:4x4 p:4x4-r");
    assert_eq!(fixes[0].mismatched, Some((1, 0)));
}

/// A component with no variant picked yet is a TODO rather than a measurement,
/// so there is no layout to score it against — but the family it names is still
/// there to be searched, and picking from it is the whole of what the line is
/// waiting for.
#[test]
fn an_undecided_component_picks_a_variant() {
    let src = TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a b:4x4");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].old_line, "\u{2FF0} a b:4x4");
    assert_eq!(fixes[0].before, None, "nothing was measured to begin with");
    // The same answer the decided line reaches: the wider variant, and the
    // gaps that push both parts out against the box.
    assert_eq!(fixes[0].after, 2);
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:4x4 -2 b:4x4");
}

/// An undecided component whose family is empty names nothing that could go in
/// the slot, so the line stays a TODO.
#[test]
fn an_undecided_component_with_no_variants_is_skipped() {
    let src = TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} q b:4x4");
    assert!(plan(&src).is_empty());
}

/// A component nothing defines whose *family* draws nothing either, and one
/// split by a line whose own components have not been decided: neither names a
/// drawing the check can measure and neither has an alternative that would, so
/// there is nothing here to improve. (A part split by a *decided* line is
/// measured like any other — see [`a_part_that_is_itself_split_can_be_chosen`].)
#[test]
fn unmeasurable_lines_are_skipped() {
    for line in ["\u{2FF0} a:4x4 nothing:4x4", "\u{2FF0} a:4x4 split:4x4"] {
        let src = format!(
            "{}\nglyph split:4x4 4 4\n\u{2FF1} b b:4x2\n\nglyph b:4x2 4 2\n..@@@@@@\n..@@@@@@\n",
            TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", line)
        );
        assert!(plan(&src).is_empty(), "{line}");
    }
    // A component whose box does not fill the slot across the axis errors, and
    // its family draws nothing that would fill it instead.
    let src = TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:4x4 short:4x2");
    assert!(plan(&format!("{src}\nglyph short:4x2 4 2\n..@@@@@@\n..@@@@@@\n")).is_empty());
}

/// A line the check *errors* on has no layout either — a component naming a
/// glyph nothing defines is not a measurement that came out wrong — but the
/// family that component names is on hand, and choosing from it is what the
/// error asks for. The three ways a component can be wrong about the glyph it
/// names, and the one answer all of them reach: the variant that is actually
/// drawn, at the gaps the sound line would have chosen.
#[test]
fn a_component_the_check_errors_on_picks_a_variant_that_fits() {
    // `a:5x4` names nothing; `a:4x3` is the wrong height for the slot; `a:9x4`
    // says 9x4 while the glyph it names is 4x4.
    let extra = "\nglyph a:4x3 4 3\n@@@@....\n@@@@....\n@@@@....\n\
                 \nglyph a:9x4 4 4\n@@@@....\n@@@@....\n@@@@....\n@@@@....\n";
    for part in ["a:5x4", "a:4x3", "a:9x4"] {
        let src = format!(
            "{}{extra}",
            TWO_PARTS.replace("\u{2FF0} a:4x4", &format!("\u{2FF0} {part}"))
        );
        let fixes = plan(&src);
        assert_eq!(fixes.len(), 1, "{part}: {fixes:?}");
        assert_eq!(fixes[0].before, None, "{part}: nothing was measured");
        assert!(fixes[0].faulty, "{part}");
        assert_eq!(fixes[0].after, 2, "{part}");
        assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:4x4 -2 b:4x4", "{part}");
    }
}

/// Two variants that measure alike, and the size the erroring name states as
/// the thing that decides between them: a component that names a glyph nothing
/// draws is wrong about the glyph, but the extent its author asked for is still
/// written on it, and an answer that keeps it is the one to prefer.
#[test]
fn an_erroring_name_keeps_the_extent_it_asked_for() {
    let src = "\
audit ideal-clearance test-* 0 1

glyph p:3x4 3 4
@@@@@@
@@@@@@
@@@@@@
@@@@@@

glyph p:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph q:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 8 4
\u{2FF0} p:3x9 q:4x4
";
    // `p:4x4` fills the box exactly and `p:3x4` leaves the one cell the range
    // allows: both score 0, and the 3 the line asked for is the tie-break.
    let fixes = plan(src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (None, 0));
    assert_eq!(
        fixes[0].new_line, "\u{2FF0} p:3x4 1 q:4x4",
        "the spare cell between them"
    );
    // An extent nothing in the family has decides nothing, and the layout that
    // leaves the least at the edges wins as it always does.
    let fixes = plan(&src.replace("p:3x9", "p:9x9"));
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].new_line, "\u{2FF0} p:4x4 q:4x4");
}

/// A part that is a composite draws no pixels of its own, but it does draw: it
/// is flattened and measured like any other, so a radical written as a `ref` to
/// a shared drawing can be chosen and can be scored.
///
/// `c:4x4` is `b:4x4` under another name, so the line through it has to reach
/// exactly the layout the line through `b:4x4` reaches.
#[test]
fn a_part_that_is_a_composite_is_measured() {
    let direct = plan(TWO_PARTS);
    let src = format!(
        "{}\nglyph c:4x4 4 4\nref b:4x4\n",
        TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:4x4 c:4x4")
    );
    let through_ref = plan(&src);
    assert_eq!(direct.len(), 1, "{direct:?}");
    assert_eq!(through_ref.len(), 1, "{through_ref:?}");
    assert_eq!(through_ref[0].before, direct[0].before);
    assert_eq!(through_ref[0].after, direct[0].after);
    assert_eq!(
        through_ref[0].new_line,
        direct[0].new_line.replace("b:4x4", "c:4x4"),
    );
}

/// A `ref` reaching left of the composite's own origin: the flattened grid
/// starts before cell (0, 0), and the ink out there is where it is drawn — the
/// same rule a part's own pixels are read by. `d:4x4` is `b:4x4` placed one
/// declared cell to the left, which is the drawing `bb:4x4` *is*, so the two
/// lines have to be laid out alike.
#[test]
fn a_composite_reaching_left_of_its_origin_is_measured_where_it_draws() {
    let bb = "\nglyph bb:4x4 4 4\n@@@@@@..\n@@@@@@..\n@@@@@@..\n@@@@@@..\n";
    let d = "\nglyph d:4x4 4 4\nref b:4x4 -1 0\n";
    let line = |part: &str| {
        TWO_PARTS.replace(
            "\u{2FF0} a:4x4 b:4x4",
            &format!("\u{2FF0} a:4x4 {part}:4x4"),
        )
    };
    let drawn = plan(&format!("{}{bb}", line("bb")));
    let composed = plan(&format!("{}{d}", line("d")));
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert_eq!(composed.len(), 1, "{composed:?}");
    assert_eq!(composed[0].after, drawn[0].after);
    assert_eq!(composed[0].new_line, drawn[0].new_line.replace("bb", "d"));
}

/// A pattern block stands for a family whose parts are sized one by one, so
/// only what the family *shares* can be rewritten: the gaps. Two glyphs whose
/// parts are drawn differently, and the one pair of gaps that puts both of them
/// inside the range.
///
/// ```text
/// l1:4x4 ####   r1:4x4 ####      l2:5x4 ####.  r2:3x4 ###
/// ```
const PATTERN_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph l1:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r1:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph l2:5x4 5 4
@@@@@@@@..
@@@@@@@@..
@@@@@@@@..
@@@@@@@@..

glyph r2:3x4 3 4
@@@@@@
@@@@@@
@@@@@@
@@@@@@

glyph test-(x|y) 8 4
\u{2FF0} 1 (l1:4x4|l2:5x4) 1 (r1:4x4|r2:3x4)
";

#[test]
fn a_pattern_line_is_optimized_by_the_gaps_its_glyphs_share() {
    let fixes = plan(PATTERN_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!(fix.glyph, "test-(x|y)", "the block, not one of its glyphs");
    // 1/1/-2 and 1/2/-2 as written: both glyphs warn. One pair of gaps puts
    // both inside — 0/0/0 and 0/1/0 — though neither glyph could have chosen
    // it alone.
    assert_eq!((fix.before, fix.after), (Some(5), 0));
    assert_eq!(fix.glyphs_warning, Some((2, 0)));
    assert_eq!(
        fix.new_line, "\u{2FF0} (l1:4x4|l2:5x4) (r1:4x4|r2:3x4)",
        "the components are the block's own; only the gaps are the fix",
    );
}

/// Three glyphs, one of which the gaps can bring inside the range while the
/// other two are pushed further out. Fewer glyphs warning is what the command
/// is for, so it takes that trade even though the summed score is worse.
#[test]
fn fewer_warning_glyphs_beats_a_lower_score() {
    let src = "\
audit ideal-clearance test-* 0 1

glyph la:5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph ra:4x4 4 4
....@@@@
....@@@@
....@@@@
....@@@@

glyph lb:4x4 4 4
..@@@@@@
..@@@@@@
..@@@@@@
..@@@@@@

glyph rb:4x4 4 4
@@@@@@..
@@@@@@..
@@@@@@..
@@@@@@..

glyph test-(x|y|z) 8 4
\u{2FF0} (la:5x4|lb:4x4|lb:4x4) (ra:4x4|rb:4x4|rb:4x4)
";
    let fixes = plan(src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    // As written every glyph warns, and the total of the three scores is 4.
    // The one gap that clears `test-x` costs the other two 2 each: three
    // warnings at 4, or two at 6, and the count decides.
    assert_eq!((fix.before, fix.after), (Some(4), 6));
    assert_eq!(fix.glyphs_warning, Some((3, 2)));
    assert_eq!(
        fix.new_line,
        "\u{2FF0} (la:5x4|lb:4x4|lb:4x4) -1 (ra:4x4|rb:4x4|rb:4x4)",
    );
}

/// A glyph of the family whose parts cannot be measured — one of them names
/// nothing — has no layout in the answer, and a family none of whose glyphs
/// can be measured is left alone. The glyph is still *counted*, since a name
/// nothing defines is a thing the line is reported for; it is the label the
/// slot shares that could put it right, and here the slot's label is the
/// block's own pattern and there is nothing to move.
#[test]
fn a_glyph_the_line_cannot_measure_is_left_out_of_the_answer() {
    let src = PATTERN_PARTS.replace("(r1:4x4|r2:3x4)", "(r1:4x4|nothing:3x4)");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    // `test-x` alone decides now: 1/1/-2 to 0/0/0, and `test-y` stays where it
    // is, counted among the glyphs that warn on both sides.
    assert_eq!((fixes[0].before, fixes[0].after), (Some(2), 0));
    assert_eq!(fixes[0].glyphs_warning, Some((2, 1)));

    let none = PATTERN_PARTS
        .replace("(r1:4x4|r2:3x4)", "(nope:4x4|nothing:3x4)")
        .replace("glyph r1:4x4 4 4", "glyph unused-r1:4x4 4 4");
    assert!(plan(&none).is_empty());
}

/// A pattern line one of whose glyphs the check errors on: the label the slot
/// shares is what can put it right, and the search asks every glyph's family
/// for it — the erroring one included, which is the whole point. `r2:4x4` is
/// not drawn, so the one label both families offer is `3x4`.
#[test]
fn a_shared_label_answers_a_glyph_the_check_errors_on() {
    let src = format!(
        "{}\nglyph r1:3x4 3 4\n@@@@@@\n@@@@@@\n@@@@@@\n@@@@@@\n",
        PATTERN_PARTS.replace(
            "\u{2FF0} 1 (l1:4x4|l2:5x4) 1 (r1:4x4|r2:3x4)",
            "\u{2FF0} (l1:4x4|l2:5x4) (r1|r2):4x4",
        )
    );
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert!(fixes[0].faulty);
    assert_eq!(
        fixes[0].new_line, "\u{2FF0} (l1:4x4|l2:5x4) (r1|r2):3x4",
        "the one label every glyph of the family draws",
    );
    assert_eq!(fixes[0].glyphs_warning, Some((1, 0)));
}

/// A pattern line whose gaps already suit every glyph it stands for is not
/// touched, and neither is one no rule reaches.
#[test]
fn a_pattern_line_that_warns_about_nothing_is_left_alone() {
    let good = PATTERN_PARTS.replace(
        "\u{2FF0} 1 (l1:4x4|l2:5x4) 1 (r1:4x4|r2:3x4)",
        "\u{2FF0} (l1:4x4|l2:5x4) (r1:4x4|r2:3x4)",
    );
    assert!(plan(&good).is_empty());
    assert!(plan(&PATTERN_PARTS.replace("test-*", "other-*")).is_empty());
}

/// Three parts, each drawing only its first column of three, in a 9x3 box.
/// The total is 6 and nothing can change it, but the arrangement can still be
/// brought from 0/2/2/2 to the evenest layout the range allows.
#[test]
fn three_parts_are_spread_evenly() {
    let src = "\
audit ideal-clearance test-* 0 1

glyph p:3x3 3 3
@@....
@@....
@@....

glyph test-y 9 3
\u{2FF2} p:3x3 p:3x3 p:3x3
";
    let fixes = plan(src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (Some(8), 7));
    // A gap of zero is not written, and neither is a trailing one.
    assert_eq!(fixes[0].new_line, "\u{2FF2} 1 p:3x3 p:3x3 p:3x3");
}

/// A vertical split measures columns instead of rows.
#[test]
fn a_vertical_split_is_optimized_along_its_own_axis() {
    let src = "\
audit ideal-clearance test-* 0 1

glyph u:4x2 4 2
@@@@@@@@
........

glyph d:4x2 4 2
........
@@@@@@@@

glyph test-z 4 4
\u{2FF1} u:4x2 d:4x2
";
    let fixes = plan(src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (Some(2), 1));
    assert_eq!(fixes[0].new_line, "\u{2FF1} u:4x2 -1 d:4x2");
}

/// The comment on the line survives the rewrite.
#[test]
fn a_rewritten_line_keeps_its_comment() {
    let src = TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:4x4 b:4x4  // as drawn");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:4x4 -2 b:4x4 // as drawn");
}

/// The clearance warnings the real check reports for a source.
fn clearance_warnings(src: &str) -> Vec<String> {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    crate::issues::collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.severity == crate::issues::Severity::Warning && i.message.contains("leaves"))
        .map(|i| i.message)
        .collect()
}

/// The source with every planned fix applied, through the same two helpers the
/// `fix` subcommand uses.
fn fixed(src: &str) -> String {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let lines: Vec<&str> = src.split('\n').collect();
    let edits: Vec<(usize, String)> = optimize_clearance(&[&doc])
        .into_iter()
        .flat_map(|f| f.fixes)
        .map(|f| {
            let line = crate::fix::compose_file_line(&doc, f.item_idx, f.compose_idx, &lines)
                .expect("the planned line is findable");
            (line, f.new_line)
        })
        .collect();
    crate::fix::rewrite_lines(src, &edits)
}

/// The end of the whole thing: what the optimizer scores has to be what the
/// check warns about, so applying a plan must actually remove warnings.
///
/// The two are separate code — the check formats messages while this solves
/// for a layout — and this is the test that holds them to the same numbers.
#[test]
fn applying_a_plan_removes_the_warnings_it_was_scored_on() {
    for src in [
        TWO_PARTS.to_string(),
        TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:3x4 b:4x4"),
    ] {
        let before = clearance_warnings(&src);
        let after = clearance_warnings(&fixed(&src));
        // Only the total is left, and it is the one no arrangement can move.
        assert_eq!(before.len(), 2, "{before:?}");
        assert_eq!(after.len(), 1, "{after:?}");
        assert!(after[0].contains("in total"), "{after:?}");
    }
    // A second run is a no-op: what it would rewrite, it already did.
    let once = fixed(TWO_PARTS);
    assert_eq!(fixed(&once), once);
}

/// The same, for a line that stands for a family: the count the plan claims to
/// have brought down is the count the check reports.
#[test]
fn applying_a_pattern_plan_removes_the_warnings_of_every_glyph_it_cleared() {
    let before = clearance_warnings(PATTERN_PARTS);
    assert_eq!(before.len(), 3, "both glyphs warn: {before:?}");
    let once = fixed(PATTERN_PARTS);
    assert!(clearance_warnings(&once).is_empty(), "{once}");
    assert_eq!(fixed(&once), once, "a second run is a no-op");
}

/// A pattern line whose components carry a variant label the block's own
/// pattern does not reach: `(rx|ry):5x4` says the same `5x4` for every glyph
/// of the family, so the label is the family's answer and not one glyph's.
///
/// ```text
/// l:4x4 ####   rx:5x4/ry:5x4 #####   rx:4x4/ry:4x4 ####
/// ```
const PATTERN_LABELS: &str = "\
audit ideal-clearance test-* 0 1

glyph l:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph l:5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph rx:5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph ry:5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph rx:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph ry:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-(x|y) 8 4
\u{2FF0} l:4x4 (rx|ry):5x4
";

/// The parts are 4 + 5 wide in an 8-wide box, so no set of gaps can bring the
/// family inside the range — but every glyph's own family offers a `4x4`, and
/// the label is spelled out on the line, so it is the family's to choose.
#[test]
fn a_pattern_component_label_is_searched_when_the_pattern_does_not_reach_it() {
    let fixes = plan(PATTERN_LABELS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!(fix.glyph, "test-(x|y)");
    assert_eq!((fix.before, fix.after), (Some(4), 0));
    assert_eq!(fix.glyphs_warning, Some((2, 0)));
    assert_eq!(fix.new_line, "\u{2FF0} l:4x4 (rx|ry):4x4");
}

/// A component of a pattern line that is spelled out entirely is the same
/// glyph in every member of the family, so its variants are searched too.
#[test]
fn a_spelled_out_component_of_a_pattern_line_is_searched() {
    let src = PATTERN_LABELS.replace("\u{2FF0} l:4x4 (rx|ry):5x4", "\u{2FF0} l:5x4 (rx|ry):4x4");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (Some(4), 0));
    assert_eq!(fixes[0].new_line, "\u{2FF0} l:4x4 (rx|ry):4x4");
}

/// A label the block's pattern *does* reach is one glyph's answer and not the
/// family's, so it is left exactly as written.
#[test]
fn a_label_the_pattern_reaches_is_not_searched() {
    let src = PATTERN_LABELS.replace("\u{2FF0} l:4x4 (rx|ry):5x4", "\u{2FF0} l:4x4 rx:(4|5)x4");
    assert!(plan(&src).is_empty(), "{:?}", plan(&src));
}

/// The parts of a real Han source are written as one block per size — a
/// pattern block declaring a whole family of names with one drawing — so a
/// variant search that only knew spelled-out blocks would find almost nothing.
#[test]
fn a_variant_declared_by_a_pattern_block_is_a_candidate() {
    let src = PATTERN_LABELS
        .replace("glyph rx:5x4 5 4", "glyph (rx|ry):5x4 5 4")
        .replace("glyph rx:4x4 4 4", "glyph (rx|ry):4x4 4 4")
        // The blocks the two names above now cover, gone.
        .replace("glyph ry:5x4 5 4", "glyph spare-a:5x4 5 4")
        .replace("glyph ry:4x4 4 4", "glyph spare-b:4x4 4 4");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (Some(4), 0));
    assert_eq!(
        fixes[0].glyphs_warning,
        Some((2, 0)),
        "both glyphs measured"
    );
    assert_eq!(fixes[0].new_line, "\u{2FF0} l:4x4 (rx|ry):4x4");
}

/// One label has to serve the whole family: a label some glyph of it does not
/// draw is no answer, however well it suits the others.
#[test]
fn a_label_only_part_of_the_family_draws_is_not_a_candidate() {
    let src = PATTERN_LABELS.replace("glyph ry:4x4 4 4", "glyph unused-ry:4x4 4 4");
    assert!(plan(&src).is_empty(), "{:?}", plan(&src));
}

/// A relabel writes the name the *line* spells, so a component written as an
/// alias may be relabelled only where the alias itself goes on saying what it
/// says. An alias that exists at one label alone is not a family to search.
#[test]
fn an_alias_that_exists_at_one_label_only_is_not_relabelled() {
    let src = format!(
        "{}\nglyph ry:5x4 = ry2:5x4\n",
        PATTERN_LABELS
            .replace("glyph ry:5x4 5 4", "glyph ry2:5x4 5 4")
            .replace("glyph ry:4x4 4 4", "glyph ry2:4x4 4 4"),
    );
    assert!(plan(&src).is_empty(), "{:?}", plan(&src));
}

/// Parts far too thin for their box, with a variant of `a` as wide as the
/// whole glyph on hand. Filling the box with one part leaves a *negative*
/// total, which the score likes as much as one that is too large — so without a
/// bound on the axis the search would write a layout whose parts overlap.
const OVERSIZED_VARIANT: &str = "\
audit ideal-clearance test-* 0 1

glyph a:2x4 2 4
@@@@
@@@@
@@@@
@@@@

glyph a:8x4 8 4
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph b:1x4 1 4
@@
@@
@@
@@

glyph test-x:8x4 8 4
\u{2FF0} a:2x4 b:1x4
";

/// A part that would fill the glyph's whole axis on its own is no candidate:
/// whatever sits beside it has nowhere to go.
#[test]
fn a_part_as_long_as_the_glyph_is_not_a_candidate() {
    let fixes = plan(OVERSIZED_VARIANT);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert!(
        !fixes[0].new_line.contains("a:8x4"),
        "the 8-wide variant fills the 8-wide box: {}",
        fixes[0].new_line,
    );
    // Only the gaps are left to work with, and they cannot mend a total of 5:
    // the edges take their maximum and the middle swells with the rest.
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:2x4 3 b:1x4");
}

/// The same trap over a family: the label `8x4` fills the 8-wide box on its
/// own, and it is the label that scores best if nothing bounds it.
const OVERSIZED_LABEL: &str = "\
audit ideal-clearance test-* 0 1

glyph l:1x4 1 4
@@
@@
@@
@@

glyph (rx|ry):2x4 2 4
@@@@
@@@@
@@@@
@@@@

glyph (rx|ry):8x4 8 4
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph test-(x|y) 8 4
\u{2FF0} l:1x4 (rx|ry):2x4
";

/// A label no glyph of the family has room for is no answer for it either, so
/// the pattern line is left with its gaps alone.
#[test]
fn a_label_as_long_as_the_glyph_is_not_a_candidate() {
    let fixes = plan(OVERSIZED_LABEL);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 l:1x4 2 (rx|ry):2x4");
}

/// A family whose only drawing for a slot is reachable through a `glyph A = B`
/// alias: `r:4x4-r` is drawn, and `r:4x4-l` is a second *name* for it. The
/// name is what carries the direction, so without the alias the left slot has
/// no candidate at all — every name the family declares outright ranks as the
/// wrong direction there.
const ALIASED_VARIANT: &str = "\
audit ideal-clearance test-* 0 1

glyph r:4x4-r 4 4
@@@@@@..
@@@@@@..
@@@@@@..
@@@@@@..

glyph r:4x4-l = r:4x4-r

glyph b:4x4 4 4
..@@@@@@
..@@@@@@
..@@@@@@
..@@@@@@

glyph test-x 8 4
\u{2FF0} r b:4x4
";

#[test]
fn an_alias_is_a_candidate_for_the_slot_its_name_states() {
    let fixes = plan(ALIASED_VARIANT);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].glyph, "test-x");
    // Undecided as written, so there is no score to compare against.
    assert_eq!(fixes[0].before, None);
    assert!(
        fixes[0].new_line.contains("r:4x4-l"),
        "{}",
        fixes[0].new_line
    );

    // Drop the alias and the slot is empty again: nothing to plan.
    let without = ALIASED_VARIANT.replace("glyph r:4x4-l = r:4x4-r\n\n", "");
    assert!(plan(&without).is_empty(), "no name the left slot can take");
}

/// A part that is itself split by an IDC line (`⿱艹林`, where 林 is `⿰木木`)
/// is a candidate like any other: its own line is derived and the drawing it
/// stands for is measured, so the outer line can be optimized.
///
/// `nested:4x4` is `⿰ in:2x4-l in:2x4-r`, which inks its columns 1..3; beside
/// `left:4x4`, which inks 0..1, that leaves a canyon of 3 the gaps can spread.
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
fn a_part_that_is_itself_split_can_be_chosen() {
    let fixes = plan(NESTED);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].glyph, "test-x");
    // 0/3/0 as written: the middle is 2 outside the range and so is the total.
    // The total is the parts' own and cannot move, so 1/1/1 is the best there
    // is and the total's own 2 is all that is left.
    assert_eq!((fixes[0].before, fixes[0].after), (Some(4), 2));
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 left:4x4 -2 nested:4x4");
}

// ------------------------------------------------------------------ enclosures

/// A 6x6 ring with a one-cell wall and a 2x2 seed, held to `1..2` as an
/// enclosure and `0..1` as a split. The only layout that satisfies the ring is
/// the centred one.
const RING: &str = "\
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
\u{2FF4} ring:6x6.4x4 seed:2x2 1 1
";

/// A `⿴` has no side the glyph opens on, so the tie-break that pushes a split's
/// parts out against the box has nothing to say and the evenness of the two
/// inner clearances is what decides. Without it the lexicographic rule would
/// wedge the seed into a corner of the ring and call that an answer.
#[test]
fn a_full_surround_centres_what_it_holds() {
    let fixes = plan(RING);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!(fix.old_line, "\u{2FF4} ring:6x6.4x4 seed:2x2 1 1");
    // As written the seed touches the top-left of the ring: two clearances of
    // 0 against a band of 1..2.
    assert_eq!(fix.before, Some(2));
    assert_eq!(fix.after, 0);
    assert_eq!(fix.new_line, "\u{2FF4} ring:6x6.4x4 seed:2x2 2 2");
}

/// A line already in the range is a decision its author made.
#[test]
fn a_centred_surround_is_left_alone() {
    let src = RING.replace(
        "\u{2FF4} ring:6x6.4x4 seed:2x2 1 1",
        "\u{2FF4} ring:6x6.4x4 seed:2x2 2 2",
    );
    assert!(plan(&src).is_empty());
}

/// `⿸` 广: walls left and top, open right and below. Both open sides get a
/// clearance to the glyph's own edge, and minimizing those is what puts the
/// inner part into the corner the operator opens on.
const GUANG: &str = "\
audit ideal-clearance test-* 0 1 0 1

glyph guang:6x6.5x5 6 6
@@@@@@@@@@@@
@@..........
@@..........
@@..........
@@..........
@@..........

glyph seed:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 6 6
\u{2FF8} guang:6x6.5x5 seed:4x4 0 0
";

#[test]
fn a_corner_enclosure_pushes_what_it_holds_into_the_corner_it_opens_on() {
    let fixes = plan(GUANG);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    // At (0, 0) the seed is under the top bar and inside the left stroke, so
    // each axis reads -6 against the wall and 2 to the open edge, and each
    // axis's total is -4. Wrong in every one of those six numbers.
    assert_eq!(fixes[0].before, Some(22));
    // Each axis leaves 1 in total whatever the placement, so the only question
    // is where it goes — and the tie-break puts the seed against the sides the
    // operator opens on, leaving the cell beside the walls.
    assert_eq!(fixes[0].new_line, "\u{2FF8} guang:6x6.5x5 seed:4x4 2 2");
    assert_eq!(fixes[0].after, 0);
}

/// An enclosure line with no offsets has decided nothing, exactly as a
/// component with no variant has: there is no layout to have measured badly, so
/// `before` is `None` and any decision is more than none.
#[test]
fn an_unplaced_enclosure_is_planned_like_a_todo() {
    let src = RING.replace(
        "\u{2FF4} ring:6x6.4x4 seed:2x2 1 1",
        "\u{2FF4} ring:6x6.4x4 seed:2x2",
    );
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].before, None);
    assert_eq!(fixes[0].new_line, "\u{2FF4} ring:6x6.4x4 seed:2x2 2 2");
}

/// The inner slot picks from the family the same way a split's slot does, and a
/// drawing that promises a cavity is not offered for it.
#[test]
fn an_enclosure_picks_the_inner_variant_that_fits() {
    let src = "\
audit ideal-clearance test-* 0 1 1 2

glyph ring:6x6.4x4 6 6
@@@@@@@@@@@@
@@........@@
@@........@@
@@........@@
@@........@@
@@@@@@@@@@@@

glyph seed:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph seed:2x2 2 2
@@@@
@@@@

glyph test-x 6 6
\u{2FF4} ring:6x6.4x4 seed:4x4 1 1
";
    let fixes = plan(src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    // `seed:4x4` fills the ring's cavity edge to edge — every clearance is 0
    // and no placement helps — so the 2x2 is chosen and centred.
    assert_eq!(fixes[0].new_line, "\u{2FF4} ring:6x6.4x4 seed:2x2 2 2");
    assert_eq!(fixes[0].after, 0);
}

/// A pattern block that encloses is not planned: what a family shares on a
/// split is its gaps, and an offset is only meaningful against one glyph's own
/// walls.
#[test]
fn a_pattern_enclosure_line_is_left_to_its_author() {
    let src = "\
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

glyph test-(x|y) 6 6
\u{2FF4} ring:6x6.4x4 seed:2x2 1 1
";
    assert!(plan(src).is_empty());
}

/// The same, for an enclosure: the four numbers the placement search scores are
/// the four the check warns about, and applying the plan clears them.
#[test]
fn applying_an_enclosure_plan_removes_the_warnings_it_was_scored_on() {
    for src in [RING.to_string(), GUANG.to_string()] {
        let before = clearance_warnings(&src);
        assert!(!before.is_empty(), "{before:?}");
        let after = clearance_warnings(&fixed(&src));
        assert!(after.is_empty(), "{after:?}");
        // A second run is a no-op: what it would rewrite, it already did.
        let once = fixed(&src);
        assert_eq!(fixed(&once), once);
    }
}

/// A pattern line whose component has picked no label at all: the TODO case,
/// which the plain path has always planned and this one used to skip whole.
/// The name is the family's answer wherever the block's own pattern does not
/// reach it, so the label the component is missing is exactly what one
/// rewrite can supply.
#[test]
fn an_undecided_component_of_a_pattern_line_is_decided() {
    let src = PATTERN_LABELS.replace("\u{2FF0} l:4x4 (rx|ry):5x4", "\u{2FF0} l:4x4 (rx|ry)");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    // No layout as written, so nothing was measured to report against — the
    // same `None` the plain path's TODO carries — and no error either.
    assert_eq!((fix.before, fix.after), (None, 0));
    assert!(
        !fix.faulty,
        "an undecided component is a TODO, not an error"
    );
    assert_eq!(fix.glyphs_warning, Some((2, 0)));
    assert_eq!(fix.new_line, "\u{2FF0} l:4x4 (rx|ry):4x4");
}

/// The same, with the component written as a *pattern* rather than spelled
/// out: only the label is the line's to choose, so everything the source wrote
/// before the `:` comes back verbatim — the back-reference included.
const BACKREF_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph l:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r-(x|y):5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph r-(x|y):4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-(x|y) 8 4
\u{2FF0} l:4x4 r-($-1)
";

#[test]
fn an_undecided_component_keeps_the_pattern_it_is_written_with() {
    let fixes = plan(BACKREF_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (None, 0));
    assert_eq!(fixes[0].new_line, "\u{2FF0} l:4x4 r-($-1):4x4");
}

/// A component whose family is reached only through an `exists`-scoped alias:
/// `glyph r-(x|y):($1) = ($0)` is how a source says that two names draw one
/// shape, and a fixer that read only the unscoped aliases found no family
/// there at all — neither for a label to move nor for one to be supplied.
const EXISTS_ALIASED_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph l:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r0:5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph r0:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

exists r0:([0-9]+x[0-9]+)
glyph r-(x|y):($1) = ($0)

glyph test-(x|y) 8 4
\u{2FF0} l:4x4 r-($-1):5x4
";

#[test]
fn a_family_reached_through_an_exists_scoped_alias_is_searched() {
    let fixes = plan(EXISTS_ALIASED_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!((fix.before, fix.after), (Some(4), 0));
    assert_eq!(fix.new_line, "\u{2FF0} l:4x4 r-($-1):4x4");
}

/// The two together, which is the shape a Han source actually writes: a
/// component that has picked nothing, in a family only the searches name.
#[test]
fn an_undecided_component_of_an_exists_aliased_family_is_decided() {
    let src = EXISTS_ALIASED_PARTS.replace("r-($-1):5x4", "r-($-1)");
    let fixes = plan(&src);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!((fixes[0].before, fixes[0].after), (None, 0));
    assert_eq!(fixes[0].new_line, "\u{2FF0} l:4x4 r-($-1):4x4");
}

/// Parts that cannot be made to fit however they are chosen: `a`'s narrowest
/// drawing beside `b` overruns the box by one. Every decision still warns, so
/// the "must lower the score" rule has nothing to offer — but the line has no
/// layout at all as written, and a decided layout that warns is still more
/// than a TODO.
const TIGHT_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph a:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph a:3x4 3 4
@@@@@@
@@@@@@
@@@@@@
@@@@@@

glyph b:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 6 4
\u{2FF0} a b
";

#[test]
fn an_undecided_line_no_choice_can_clear_is_still_decided() {
    let fixes = plan(TIGHT_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].before, None, "nothing was measured to begin with");
    assert!(fixes[0].after > 0, "no choice clears the warning");
    assert_eq!(fixes[0].new_line, "\u{2FF0} -1 a:3x4 b:4x4");
}

/// The same over a family, the shape a Han source writes: a `($-1)` component
/// that has picked nothing, in a box no choice of labels can clear. The line
/// used to be left undecided, because staying a TODO scores zero and so beat
/// every layout that warns.
const TIGHT_BACKREF_PARTS: &str = "\
audit ideal-clearance test-* 0 1

glyph l:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r-(x|y):5x4 5 4
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@
@@@@@@@@@@

glyph r-(x|y):4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-(x|y) 7 4
\u{2FF0} l:4x4 r-($-1)
";

#[test]
fn an_undecided_pattern_line_no_label_can_clear_is_still_decided() {
    let fixes = plan(TIGHT_BACKREF_PARTS);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let fix = &fixes[0];
    assert_eq!(fix.before, None, "nothing was measured to begin with");
    assert!(fix.after > 0, "no label clears the warning");
    assert_eq!(fix.new_line, "\u{2FF0} -1 l:4x4 r-($-1):4x4");
}
