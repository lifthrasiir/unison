//! Logical pixels of a `scale N` grid whose cells disagree on what the bitmap
//! build makes of them.

use super::*;

const HEADER: &str = "meta height 4\nmeta ascent 3\nmeta descent 1\n\n";

/// The findings on a `scale 2` glyph `g` of `width`×2 pixels drawn as `rows`.
fn scaled(width: u16, rows: &str) -> Vec<Issue> {
    let issues = issues_for(&format!(
        "{HEADER}glyph g {width} 2 scale 2\n{rows}\n\nmap A = g\n"
    ));
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "the grid has to parse: {issues:?}"
    );
    issues
}

fn ambiguous(issues: &[Issue]) -> Vec<&Issue> {
    issues
        .iter()
        .filter(|i| i.message.contains("ambiguous in the bitmap"))
        .collect()
}

#[test]
fn lit_and_unlit_cells_in_one_pixel_are_ambiguous() {
    let issues = scaled(2, "@@88....\n........\n........\n........");
    let found = ambiguous(&issues);
    assert_eq!(found.len(), 1, "{issues:?}");
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(found[0].message.contains("lit and unlit"), "{found:?}");
    assert!(found[0].message.contains("row 0, column 0"), "{found:?}");
}

#[test]
fn blank_and_hardblank_cells_in_one_pixel_are_ambiguous() {
    let issues = scaled(2, "........\n....$$..\n........\n........");
    let found = ambiguous(&issues);
    assert_eq!(found.len(), 1, "{issues:?}");
    assert!(
        found[0].message.contains("blank and hardblank"),
        "{found:?}"
    );
    assert!(found[0].message.contains("row 0, column 1"), "{found:?}");
}

/// Every other mix decides itself: lit wins over blank, unlit cells beside a
/// claim are not lit, and a lit pixel is never a hardblank, so a `$$` in one
/// is only a spelling.
#[test]
fn unambiguous_mixes_are_quiet() {
    for rows in [
        "@@..@@$$\n........\n8888$$88\n88888888",
        "@@@@$$$$\n@@@@$$$$\n........\n........",
    ] {
        let issues = scaled(2, rows);
        assert!(ambiguous(&issues).is_empty(), "{rows}: {issues:?}");
    }
}

/// One finding per glyph however many pixels are at fault, naming each kind
/// once and only the first few pixels of each.
#[test]
fn many_faulty_pixels_make_one_summarized_warning() {
    let issues = scaled(
        8,
        "@@88@@88@@88@@88@@88@@88@@88@@88\n\
         ................................\n\
         $$..............................\n\
         ................................",
    );
    let found = ambiguous(&issues);
    assert_eq!(found.len(), 1, "{issues:?}");
    let message = &found[0].message;
    assert!(message.contains("8 pixels mix lit and unlit"), "{message}");
    assert!(
        message.contains("1 pixel mixes blank and hardblank"),
        "{message}"
    );
    assert!(message.contains("and 5 more"), "{message}");
    assert!(!message.contains("column 7"), "{message}");
}

/// A grid at `scale 1` has no cells below the pixel to disagree.
#[test]
fn an_unscaled_glyph_is_never_ambiguous() {
    let issues = issues_for(&format!("{HEADER}glyph g 2 2\n@@88\n..$$\n\nmap A = g\n"));
    assert!(ambiguous(&issues).is_empty(), "{issues:?}");
}

/// What a composite settles into depends on where its parts land, which no
/// source line states, so only a glyph's own grid is checked.
#[test]
fn a_composite_is_not_checked_across_its_components() {
    let issues = issues_for(&format!(
        "{HEADER}glyph lit 1 1 scale 2\n@@..\n....\n\n\
         glyph unlit 1 1 scale 2\n..88\n....\n\n\
         glyph g 1 1 scale 2\nref lit\nref unlit\n\nmap A = g\n"
    ));
    assert!(ambiguous(&issues).is_empty(), "{issues:?}");
}

/// A `vectoronly` glyph is drawn as its vector geometry in the bitmap build
/// too, so its cells are never squared off into pixels to disagree.
#[test]
fn a_vectoronly_glyph_is_never_ambiguous() {
    let issues = issues_for(&format!(
        "{HEADER}glyph g 2 2 scale 2 vectoronly\n@@88....\n....$$..\n........\n........\n\nmap A = g\n"
    ));
    assert!(ambiguous(&issues).is_empty(), "{issues:?}");
}
