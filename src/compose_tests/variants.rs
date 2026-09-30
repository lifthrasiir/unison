//! Tests for the variant name rule: size, direction and slot ranking.

use super::*;

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
            inner: None,
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
    // A name with no `.` promises no cavity, which is what an ordinary part is.
    assert_eq!(VariantSpec::parse("x:4x16-l").inner, None);
}

#[test]
fn variant_spec_reads_the_cavity_an_enclosure_promises() {
    let spec = VariantSpec::parse("han-5e7f:15x16.11x12");
    assert_eq!(spec.size, Some((15, 16)));
    assert_eq!(spec.inner, Some((11, 12)));
    // The cavity rides on the size token, so a direction still parses beside it.
    let spec = VariantSpec::parse("x:15x16.11x12-l");
    assert_eq!(spec.inner, Some((11, 12)));
    assert_eq!(spec.direction, Some(Direction::Left));
    // Half a cavity is not a size token at all: a name that cannot be read
    // claims nothing rather than claiming the outer box alone.
    assert_eq!(VariantSpec::parse("x:15x16.").size, None);
    assert_eq!(VariantSpec::parse("x:.11x12").size, None);
    assert_eq!(VariantSpec::parse("x:15x16.11").size, None);
    // One spelling per cavity, exactly as for the box.
    assert_eq!(VariantSpec::parse("x:15x16.011x12").size, None);
}

#[test]
fn enclosure_rank_reads_the_cavity_instead_of_a_direction() {
    // A drawing that promises a cavity was made to hold something.
    assert_eq!(enclosure_rank("a:15x16.11x12", true), 0);
    assert_eq!(enclosure_rank("a:15x16.11x12", false), 2);
    // One that does not was made to be held.
    assert_eq!(enclosure_rank("a:11x12", false), 0);
    assert_eq!(enclosure_rank("a:11x12", true), 2);
    // A component that has picked nothing claims nothing, on either slot.
    assert_eq!(enclosure_rank("a", true), 1);
    assert_eq!(enclosure_rank("a", false), 1);
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
    // The middle of a three-part split claims nothing: no Han character has a
    // form that only appears there, so a name written for either side is right.
    assert_eq!(LeftMiddleRight.slot_direction(1), None);
    assert_eq!(AboveMiddleBelow.slot_direction(1), None);
    assert_eq!(AboveBelow.slot_direction(0), Some(Direction::Up));
    assert_eq!(AboveMiddleBelow.slot_direction(2), Some(Direction::Down));
    assert_eq!(IdcOp::from_token("\u{2FF0}"), Some(LeftRight));
    assert_eq!(IdcOp::from_token("\u{2FF0}x"), None);
    assert_eq!(IdcOp::from_token("ref"), None);
}
