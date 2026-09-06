//! `DocLine` round-trips: the line-level model the editor edits.

use super::*;

fn docline_roundtrip(input: &str) {
    let lines = parse_doclines(input);
    let mut output = Vec::new();
    serialize_doclines(&lines, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(input, output_str, "serialize_doclines did not round-trip");
}

#[test]
fn docline_roundtrip_simple() {
    let input = "\
// test comment
meta height 16
meta ascent 14
meta descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph uni0041
ref test-glyph 2 0

assume unused test-glyph
";
    docline_roundtrip(input);

    let lines = parse_doclines(input);
    // comment, 3x meta, blank, glyph-header, Grid, blank, glyph-header, ref, blank, directive
    assert_eq!(lines.len(), 12);
    assert!(matches!(lines[0], DocLine::Text(ref s) if s.starts_with("//")));
    assert!(matches!(lines[1], DocLine::Text(ref s) if s.starts_with("meta ")));
    assert!(matches!(lines[5], DocLine::Text(ref s) if s.starts_with("glyph test-glyph")));
    assert!(matches!(lines[6], DocLine::Grid(_)));
    assert!(matches!(lines[8], DocLine::Text(ref s) if s.starts_with("glyph uni0041")));
    assert!(matches!(lines[9], DocLine::Text(ref s) if s.starts_with("ref ")));
}

#[test]
fn docline_roundtrip_alias() {
    docline_roundtrip("glyph uni0041 = test-glyph\n");
    let lines = parse_doclines("glyph uni0041 = test-glyph\n");
    assert_eq!(lines.len(), 1);
    assert!(matches!(lines[0], DocLine::Text(_)));
}

#[test]
fn docline_roundtrip_ref_only_glyph() {
    let input = "\
glyph composite
ref part-a 0 0
ref part-b 4 2
";
    docline_roundtrip(input);
    let lines = parse_doclines(input);
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|l| matches!(l, DocLine::Text(_))));
}

#[test]
fn docline_roundtrip_glyph_with_pixels_and_refs() {
    let input = "\
glyph mixed 2 2
..@@
@@..
ref other 1 1
";
    docline_roundtrip(input);
    let lines = parse_doclines(input);
    assert_eq!(lines.len(), 3);
    assert!(matches!(lines[0], DocLine::Text(_)));
    assert!(matches!(lines[1], DocLine::Grid(_)));
    assert!(matches!(lines[2], DocLine::Text(ref s) if s.starts_with("ref ")));
}

// -----------------------------------------------------------------------
// derive_document equivalence tests
// -----------------------------------------------------------------------

/// The two ways a file is divided into lines are one walk, and the file lines
/// the walk reports are the ones the units actually start at.
///
/// What rests on it: the search pane counts a hit's ordinal over an unopened
/// file's raw text and re-finds it over the same file's doclines once it opens
/// (`crate::app::search`). A row either walk skipped and the other did not
/// would land every later hit in that file one off.
#[test]
fn the_source_walk_and_the_doclines_divide_a_file_the_same_way() {
    let input = "\
meta height 2
glyph a 2 2
@@..
..@@
// after the grid
glyph b 2 2
not a pixel row
glyph c = a
";
    let mut units: Vec<(usize, bool)> = Vec::new();
    walk_source_lines(input, |file_line, unit| {
        units.push((file_line, matches!(unit, SourceLine::Text(_))));
    });

    // One unit per docline, of the same kind and in the same order.
    let doclines = parse_doclines(input);
    assert_eq!(
        units.iter().map(|&(_, t)| t).collect::<Vec<_>>(),
        doclines
            .iter()
            .map(|l| matches!(l, DocLine::Text(_)))
            .collect::<Vec<_>>(),
    );

    // `glyph b` declares two rows and is followed by a line that is not one,
    // so its grid is empty and the text after it keeps its own file line.
    assert_eq!(
        units,
        vec![
            (1, true),  // meta height 2
            (2, true),  // glyph a 2 2
            (3, false), // its two pixel rows
            (5, true),  // // after the grid
            (6, true),  // glyph b 2 2
            (7, false), // an empty grid: the next line is no pixel row
            (7, true),  // not a pixel row
            (8, true),  // glyph c = a
        ],
    );
}
