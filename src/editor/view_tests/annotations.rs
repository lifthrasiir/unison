//! Inline annotations and comment highlighting.

use super::*;

/// `map` spells out the codepoint of a literally written character as a
/// dimmed inline annotation. It is display-only: the document line is
/// untouched, and the caret treats the character plus its annotation as one.
#[test]
fn map_literal_char_renders_codepoint_annotation() {
    let mut h = EditorHarness::new("map 가 = hangul-ga\nglyph hangul-ga 2 2\n....\n....\n");
    assert_view_consistent(&h);

    let vl = &h.snap().vlines[0];
    match &vl.kind {
        SnapKind::Text { text, display, .. } => {
            assert_eq!(text, "map 가 = hangul-ga", "the document line is unchanged");
            assert_eq!(display, "map 가 U+AC00 = hangul-ga");
        }
        other => panic!("expected a text visual line, got {other:?}"),
    }

    // Clicking on either side of the annotated character lands on the
    // document column, not inside the annotation.
    h.click_text(0, 4);
    assert_eq!(h.state.cursor, Caret::new(0, 4));
    h.click_text(0, 5);
    assert_eq!(h.state.cursor, Caret::new(0, 5));

    // Nothing in the span the annotation occupies resolves to a column
    // between the two: the pair is a single caret step.
    let x0 = h.text_pos(0, 4);
    let x1 = h.text_pos(0, 5);
    let steps = 12;
    for i in 1..steps {
        let t = i as f32 / steps as f32;
        h.click_at(egui::pos2(x0.x + (x1.x - x0.x) * t, x0.y));
        let col = h.state.cursor.col;
        assert!(
            col == 4 || col == 5,
            "caret landed inside the annotation at col {col}"
        );
    }

    // Typing still edits the document, annotation and all.
    h.click_text(0, 5);
    h.key(Key::ArrowRight);
    assert_eq!(h.state.cursor, Caret::new(0, 6));
}

/// An annotation too long for one visual line wraps by itself, exactly as the
/// same text written in the document would. It used to be unbreakable, so a
/// long one dragged the character it trails onto the next line and then
/// painted past the right edge anyway.
#[test]
fn a_long_annotation_wraps_across_visual_lines() {
    // 30 characters, so the ` U+XXXX` spelling is far wider than any editor.
    let text = "안녕하세요반갑습니다어서오세요고맙습니다또또오세요건강하세요";
    let line = format!("assert shape {text} : greeting");
    let mut h = EditorHarness::new(&format!("{line}\nglyph greeting 2 2\n....\n....\n"));
    assert_view_consistent(&h);

    let segments: Vec<(String, usize, String)> = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == 0)
        .filter_map(|vl| match &vl.kind {
            SnapKind::Text {
                text,
                col_offset,
                display,
                ..
            } => Some((text.clone(), *col_offset, display.clone())),
            SnapKind::GridRow { .. } => None,
        })
        .collect();
    assert!(
        segments.len() > 1,
        "the annotation must wrap for this test to mean anything"
    );

    // The rendered segments reassemble the rendered line: the annotation is
    // split across them, not dropped, duplicated or held back.
    let joined_display: String = segments.iter().map(|(_, _, d)| d.as_str()).collect();
    let expected: String = format!(
        "assert shape {text}{} : greeting",
        text.chars()
            .map(|c| format!(" U+{:04X}", c as u32))
            .collect::<String>()
    );
    assert_eq!(joined_display, expected);
    let joined_text: String = segments.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(joined_text, line, "the document line is unchanged");

    // At least one segment carries only annotation and no document column of
    // its own — the wrap fell inside the annotation twice over.
    assert!(
        segments
            .iter()
            .any(|(t, _, display)| t.is_empty() && !display.is_empty()),
        "expected an annotation-only segment: {segments:?}"
    );

    // The caret still walks document columns: the columns on either side of
    // the wrapped annotation are reachable and one step apart.
    let after = "assert shape ".chars().count() + text.chars().count();
    h.click_text(0, after);
    assert_eq!(h.state.cursor, Caret::new(0, after));
    h.key(Key::ArrowRight);
    assert_eq!(h.state.cursor, Caret::new(0, after + 1));
}

/// A `map` already written as `U+XXXX` needs no annotation.
#[test]
fn map_explicit_codepoint_is_not_annotated() {
    let h = EditorHarness::new("map U+AC00 = hangul-ga\nglyph hangul-ga 2 2\n....\n....\n");
    match &h.snap().vlines[0].kind {
        SnapKind::Text { text, display, .. } => assert_eq!(display, text),
        other => panic!("expected a text visual line, got {other:?}"),
    }
}

/// A `// …` comment is highlighted as a comment wherever it sits, and only
/// from its `//` on. The pixel rows below it keep their `//` as pixels.
#[test]
fn inline_comment_is_highlighted_from_its_marker() {
    let h = EditorHarness::new("glyph slash 2 1 // a note\n0//1\n");
    assert_view_consistent(&h);
    match &h.snap().vlines[0].kind {
        SnapKind::Text {
            text, comment_col, ..
        } => {
            assert_eq!(*comment_col, Some(text.find("//").unwrap()));
        }
        other => panic!("expected a text visual line, got {other:?}"),
    }
    // The pixel row is still a grid, not a commented-out text line.
    assert!(matches!(h.snap().vlines[1].kind, SnapKind::GridRow { .. }));
    assert_eq!(h.grid(1).width, 2);
}

/// A line without a comment has nothing highlighted, and a quoted `//` is an
/// ordinary token rather than a comment marker.
#[test]
fn quoted_double_slash_is_not_a_comment() {
    let h = EditorHarness::new("map `//` = solidus-double\nglyph solidus-double 2 1\n@@..\n");
    match &h.snap().vlines[0].kind {
        SnapKind::Text { comment_col, .. } => assert_eq!(*comment_col, None),
        other => panic!("expected a text visual line, got {other:?}"),
    }
}

/// Typing a comment onto a glyph header must not cost the header its grid:
/// the comment is not part of the `W H` grammar.
#[test]
fn typing_a_comment_on_a_header_keeps_the_grid() {
    let mut h = EditorHarness::new("glyph foo 4 2\n@@......\n......@@\n");
    h.click_text(0, 13);
    h.type_text(" // a note");
    h.key(Key::ArrowDown);
    assert_eq!(h.text(0), "glyph foo 4 2 // a note");
    assert_view_consistent(&h);
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (4, 2));
    assert!(
        !grid.get(0, 0).is_clear(),
        "pixels survived the header edit"
    );
    match &h.snap().vlines[0].kind {
        SnapKind::Text { comment_col, .. } => assert_eq!(*comment_col, Some(14)),
        other => panic!("expected a text visual line, got {other:?}"),
    }
}
