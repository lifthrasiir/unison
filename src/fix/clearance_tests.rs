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
    assert_eq!(fix.before, 4);
    // Both parts move outwards: 1/1/1, and the total — which no arrangement
    // can change — is all that is left to warn about.
    assert_eq!(fix.after, 2);
    assert_eq!(fix.new_line, "\u{2FF0} 1 a:4x4 -2 b:4x4");
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
    assert_eq!((fixes[0].before, fixes[0].after), (5, 2));
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
    assert_eq!((fixes[0].before, fixes[0].after), (5, 4));
    assert_eq!(fixes[0].new_line, "\u{2FF0} 1 a:3x4 -1 b:4x4");
}

/// An undecided component, a component nothing defines, and a part that is
/// itself a composite: all three are lines the check does not measure, so
/// there is nothing here to improve.
#[test]
fn unmeasurable_lines_are_skipped() {
    for line in [
        "\u{2FF0} a b:4x4",
        "\u{2FF0} a:4x4 nothing:4x4",
        "\u{2FF0} a:4x4 c:4x4",
    ] {
        let src = format!(
            "{}\nglyph c:4x4 4 4\nref b:4x4\n",
            TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", line)
        );
        assert!(plan(&src).is_empty(), "{line}");
    }
    // A component whose box does not fill the slot across the axis is an
    // error the source has to answer first.
    let src = TWO_PARTS.replace("\u{2FF0} a:4x4 b:4x4", "\u{2FF0} a:4x4 short:4x2");
    assert!(plan(&format!("{src}\nglyph short:4x2 4 2\n..@@@@@@\n..@@@@@@\n")).is_empty());
}

/// A pattern block stands for a family whose parts are sized one by one, so no
/// single line could be right for it.
#[test]
fn a_pattern_named_glyph_is_skipped() {
    let src = TWO_PARTS.replace("glyph test-x 8 4", "glyph test-(x|y) 8 4");
    assert!(plan(&src).is_empty());
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
    assert_eq!((fixes[0].before, fixes[0].after), (8, 7));
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
    assert_eq!((fixes[0].before, fixes[0].after), (2, 1));
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
