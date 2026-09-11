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

// ---------------------------------------------------------------------------
// Following edits made after the report
// ---------------------------------------------------------------------------

const THREE: &str = "// one\n// two\n// three\n";

/// A line opened above a reported one carries its highlight down with it:
/// the report named the line, not the row it happened to be on.
#[test]
fn a_line_opened_above_carries_the_mark_down() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(2, Severity::Warning, "three")]);
    h.frame();

    h.click_text(0, 6);
    h.key(Key::Enter);
    h.frame();

    assert_eq!(h.text(3), "// three");
    assert_eq!(
        h.issue_lines(),
        vec![(3, Severity::Warning, "three".to_string())]
    );
}

/// A reported line joined into the one above is gone, and its finding with
/// it; the line below moves up and keeps its own. Undo puts both back.
#[test]
fn a_line_joined_away_takes_its_mark_and_undo_restores_it() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(1, Severity::Error, "two"), (2, Severity::Warning, "three")]);
    h.frame();

    h.click_text(1, 0);
    h.key(Key::Backspace);
    h.frame();

    assert_eq!(h.text(0), "// one// two");
    assert_eq!(
        h.issue_lines(),
        vec![(1, Severity::Warning, "three".to_string())]
    );

    h.key_mod(Key::Z, Modifiers::COMMAND);
    h.frame();
    assert_eq!(h.text(1), "// two");
    assert_eq!(
        h.issue_lines(),
        vec![
            (1, Severity::Error, "two".to_string()),
            (2, Severity::Warning, "three".to_string())
        ]
    );
}

/// Enter inside a reported line splits it, and the first half is the line
/// continued: the mark stays where the reader is still looking.
#[test]
fn a_split_line_keeps_its_mark_on_the_first_half() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(1, Severity::Error, "two"), (2, Severity::Warning, "three")]);
    h.frame();

    h.click_text(1, 5);
    h.key(Key::Enter);
    h.frame();

    assert_eq!((h.text(1), h.text(2)), ("// tw", "o"));
    assert_eq!(
        h.issue_lines(),
        vec![
            (1, Severity::Error, "two".to_string()),
            (3, Severity::Warning, "three".to_string())
        ]
    );
}

/// Pulling the next line up into a reported one keeps the reported line: it
/// is the one that grew.
#[test]
fn a_line_joined_into_a_reported_one_keeps_that_mark() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(1, Severity::Error, "two"), (2, Severity::Warning, "three")]);
    h.frame();

    h.click_text(2, 0);
    h.key(Key::Backspace);
    h.frame();

    assert_eq!(h.text(1), "// two// three");
    assert_eq!(
        h.issue_lines(),
        vec![(1, Severity::Error, "two".to_string())]
    );
}

/// Enter at the very start of a reported line opens a blank line *above* it:
/// the text moved down, so the mark does too, and the new blank line is not
/// the reported one.
#[test]
fn enter_at_the_start_of_a_reported_line_pushes_it_down() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(1, Severity::Error, "two")]);
    h.frame();

    h.click_text(1, 0);
    h.key(Key::Enter);
    h.frame();

    assert_eq!((h.text(1), h.text(2)), ("", "// two"));
    assert_eq!(
        h.issue_lines(),
        vec![(2, Severity::Error, "two".to_string())]
    );
}

/// Backspace at the start of a reported line under a blank one deletes the
/// blank line, not the reported one.
#[test]
fn a_blank_line_deleted_above_keeps_the_mark_below() {
    let mut h = EditorHarness::new("// one\n\n// three\n");
    h.report_issues([(2, Severity::Warning, "three")]);
    h.frame();

    h.click_text(2, 0);
    h.key(Key::Backspace);
    h.frame();

    assert_eq!(h.text(1), "// three");
    assert_eq!(
        h.issue_lines(),
        vec![(1, Severity::Warning, "three".to_string())]
    );
}

/// A selection from the start of one line to the start of a reported one
/// deletes everything before the reported text; the text that is left is
/// still the reported line.
#[test]
fn a_selection_deleted_up_to_a_reported_line_keeps_it() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(0, Severity::Error, "one"), (2, Severity::Warning, "three")]);
    h.frame();

    h.click_text(0, 0);
    h.click_at_mod(h.text_pos(2, 0), Modifiers::SHIFT);
    h.key(Key::Backspace);
    h.frame();

    assert_eq!(h.text(0), "// three");
    assert_eq!(
        h.issue_lines(),
        vec![(0, Severity::Warning, "three".to_string())]
    );
}

/// Lines pasted at the start of a reported line land above its text, and the
/// mark stays with the text.
#[test]
fn lines_pasted_before_a_reported_line_keep_it_on_its_text() {
    let mut h = EditorHarness::new(THREE);
    h.report_issues([(1, Severity::Error, "two")]);
    h.frame();

    h.click_text(1, 0);
    h.paste("// a\n// b\n");
    h.frame();

    assert_eq!(h.text(3), "// two");
    assert_eq!(
        h.issue_lines(),
        vec![(3, Severity::Error, "two".to_string())]
    );
}

/// Commenting a reported line out rewrites it, but it is the same line: the
/// mark stays until the next build says otherwise.
#[test]
fn commenting_a_reported_line_keeps_its_mark() {
    let mut h = EditorHarness::new("meta a 1\nmeta b 2\nmeta c 3\n");
    h.report_issues([(1, Severity::Error, "b")]);
    h.frame();

    h.click_text(1, 0);
    h.key_mod(Key::Slash, Modifiers::COMMAND);
    h.frame();

    assert!(h.text(1).starts_with("//"), "{:?}", h.text(1));
    assert_eq!(h.issue_lines(), vec![(1, Severity::Error, "b".to_string())]);
}
