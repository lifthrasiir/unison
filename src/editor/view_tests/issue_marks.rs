//! Line highlights for reported issues: the tint, and the message after the
//! line.

use super::*;
use crate::issues::Severity;

/// The message is drawn once, after the line it belongs to, in the severity
/// the line was reported at.
#[test]
fn a_reported_line_carries_its_message() {
    let mut h = EditorHarness::new("meta ascent 8\nglyph a 2 2\n..\n..\n");
    h.set_line_issues([(1, Severity::Warning, "no `map` names this glyph")]);
    h.frame();

    assert_eq!(
        h.issue_lines(),
        vec![(
            1,
            Severity::Warning,
            "no `map` names this glyph".to_string()
        )]
    );
}

/// Nothing is drawn on the lines nothing was said about — including the grid
/// rows of the very glyph the finding is about, which carry the `glyph`
/// line's own `doc_line` and would otherwise be tinted along with it.
#[test]
fn a_quiet_document_draws_nothing() {
    let mut h = EditorHarness::new("meta ascent 8\nglyph a 2 2\n..\n..\n");
    h.frame();
    assert!(h.issue_lines().is_empty());

    h.set_line_issues([(1, Severity::Error, "unresolved ref")]);
    h.frame();
    let drawn: Vec<usize> = h.issue_lines().iter().map(|(line, ..)| *line).collect();
    assert_eq!(drawn, vec![1], "only the `glyph` line itself");
}

/// A line long enough to soft-wrap is still one finding: the message goes on
/// the last visual segment and is not repeated on each of them.
#[test]
fn a_wrapped_line_says_it_once() {
    // Far wider than the harness viewport, so the comment wraps.
    let long = "// ".to_string() + &"wrapping ".repeat(40);
    let mut h = EditorHarness::new(&format!("{long}\nglyph a 2 2\n..\n..\n"));
    h.set_line_issues([(0, Severity::Note, "said once")]);
    h.frame();

    let segments = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == 0 && matches!(vl.kind, SnapKind::Text { .. }))
        .count();
    assert!(
        segments > 1,
        "the line has to wrap for this to test anything"
    );
    assert_eq!(
        h.issue_lines(),
        vec![(0, Severity::Note, "said once".to_string())]
    );
}

/// The message is painted, not spliced into the line: the caret cannot walk
/// into it, and clicking where it is drawn lands at the end of the text.
#[test]
fn the_message_is_not_part_of_the_line() {
    let mut h = EditorHarness::new("meta ascent 8\nglyph a 2 2\n..\n..\n");
    h.set_line_issues([(0, Severity::Error, "a message long enough to click into")]);
    h.frame();

    let end = h.lines[0].as_text().unwrap().chars().count();
    h.click_text(0, end);
    assert_eq!(h.state.cursor, Caret::new(0, end));

    // Well past the text, where the message is drawn.
    let pos = h.text_pos(0, end);
    h.click_at(egui::pos2(pos.x + 200.0, pos.y));
    assert_eq!(
        h.state.cursor,
        Caret::new(0, end),
        "a click on the message belongs to the end of the line"
    );

    h.key(Key::End);
    assert_eq!(h.state.cursor, Caret::new(0, end));
}
