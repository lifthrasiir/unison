//! Tests for the split operators: placing parts, gaps, sizes and the diagnostics on a line.

use super::*;

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

/// A family whose sizes come from a table, as `table` does for `dims`.
fn family_of<'a>(
    entries: &'a [(&'a str, &'a [(u16, u16)])],
) -> impl Fn(&str) -> Vec<((u16, u16), Margin)> + 'a {
    move |name: &str| {
        entries
            .iter()
            .find(|(n, _)| *n == name)
            .map_or_else(Vec::new, |(_, sizes)| {
                sizes.iter().map(|&s| (s, Margin::default())).collect()
            })
    }
}

/// 址 (`⿰土止`) as an IDS-populated source has it: 止 is drawn at one size, and
/// that size fills the whole 15-wide glyph. No decision can lay this line out,
/// so it is not a decision waiting to be made — the drawing it waits for does
/// not exist.
#[test]
fn an_undecided_part_whose_family_cannot_fit_the_slot_is_a_warning() {
    let dims = table(&[("a", (4, 16))]);
    let family = family_of(&[("b", &[(15, 16)])]);
    let (_, issues) = expand_compose(
        "test",
        Some((15, 16)),
        UNIT,
        &line(IdcOp::LeftRight, vec![part("a"), part("b")]),
        &dims,
        Some(&family),
        None,
    );
    assert_eq!(errors(&issues), Vec::<&str>::new(), "{issues:?}");
    let warnings = of_severity(&issues, Severity::Warning);
    assert_eq!(
        warnings,
        vec![
            "glyph 'test': `\u{2FF0}` component 'b' has no variant that fits a 15-wide slot; \
             its family draws 15x16"
        ],
    );
    // And it is the *warning*, not a TODO beside it: one line, one thing to do.
    assert_eq!(
        todos(&issues).len(),
        1,
        "only 'a', whose family was not offered: {issues:?}"
    );
}

/// The same line where the family does draw something that fits: the decision
/// is there to be made, which is exactly what a TODO says.
#[test]
fn an_undecided_part_whose_family_fits_is_still_a_todo() {
    let dims = table(&[("a", (4, 16))]);
    for sizes in [&[(11, 16)][..], &[(15, 16), (11, 16)][..]] {
        let entries = [("b", sizes)];
        let family = family_of(&entries);
        let (_, issues) = expand_compose(
            "test",
            Some((15, 16)),
            UNIT,
            &line(IdcOp::LeftRight, vec![part("a"), part("b")]),
            &dims,
            Some(&family),
            None,
        );
        assert_eq!(of_severity(&issues, Severity::Warning), Vec::<&str>::new());
        assert_eq!(todos(&issues).len(), 2, "{issues:?}");
    }
    // A variant of the right length but the wrong height fills no slot either.
    let family = family_of(&[("b", &[(11, 15)])]);
    let (_, issues) = expand_compose(
        "test",
        Some((15, 16)),
        UNIT,
        &line(IdcOp::LeftRight, vec![part("a"), part("b")]),
        &dims,
        Some(&family),
        None,
    );
    assert_eq!(
        of_severity(&issues, Severity::Warning).len(),
        1,
        "{issues:?}"
    );
}

/// A vertical split asks the same question of the other axis.
#[test]
fn a_vertical_split_measures_the_family_along_its_own_axis() {
    let dims = table(&[("a", (16, 4))]);
    let family = family_of(&[("b", &[(16, 16)])]);
    let (_, issues) = expand_compose(
        "test",
        Some((16, 16)),
        UNIT,
        &line(IdcOp::AboveBelow, vec![part("a"), part("b")]),
        &dims,
        Some(&family),
        None,
    );
    assert_eq!(
        of_severity(&issues, Severity::Warning),
        vec![
            "glyph 'test': `\u{2FF1}` component 'b' has no variant that fits a 16-tall slot; \
             its family draws 16x16"
        ],
    );
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
        "b:11x16" => PartDims::Size(11, 16, Margin::default()),
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

/// A middle slot takes a part drawn for either side without a word, because the
/// side it borrows from is a fact about the character and not about the slot:
/// 阝 in the middle of a ⿲ is the right-hand 邑 when that is what it descends
/// from, and 匕 there is the left-hand form.
#[test]
fn the_middle_slot_accepts_a_part_drawn_for_either_side() {
    let dims = table(&[
        ("a:4x16-l", (4, 16)),
        ("b:4x16-r", (4, 16)),
        ("c:4x16-l", (4, 16)),
    ]);
    for middle in ["b:4x16-r", "c:4x16-l"] {
        let (refs, issues) = expand(
            Some((12, 16)),
            &line(
                IdcOp::LeftMiddleRight,
                vec![part("a:4x16-l"), part(middle), part("b:4x16-r")],
            ),
            &dims,
        );
        assert_eq!(refs.len(), 3, "{middle}: {issues:?}");
        assert!(
            !issues.iter().any(|(_, m)| m.contains("sits in the")),
            "{middle}: {issues:?}"
        );
    }
    // The ends of the same line are checked as strictly as ever.
    let (_, issues) = expand(
        Some((12, 16)),
        &line(
            IdcOp::LeftMiddleRight,
            vec![part("b:4x16-r"), part("a:4x16-l"), part("a:4x16-l")],
        ),
        &dims,
    );
    let placed: Vec<&str> = issues
        .iter()
        .filter(|(s, m)| *s == Severity::Warning && m.contains("sits in the"))
        .map(|(_, m)| m.as_str())
        .collect();
    assert_eq!(placed.len(), 2, "{issues:?}");
}

/// Nothing ranks a middle slot's candidates by side, since neither side is the
/// wrong one there — and `fix::clearance` refuses only a last-ranked candidate.
#[test]
fn a_middle_slot_ranks_both_sides_alike() {
    let slot = IdcOp::LeftMiddleRight.slot_direction(1);
    assert_eq!(direction_rank("a:4x16-l", slot), 1);
    assert_eq!(direction_rank("a:4x16-r", slot), 1);
    assert_eq!(direction_rank("a:4x16", slot), 1);
}
