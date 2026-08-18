//! The empty band below the last line, and where a click there lands.

use super::*;

/// A document too short to fill the editor leaves an empty band below its last
/// line. A click there used to land on nothing at all: the canvas was allocated
/// exactly as tall as its content, so the band was outside the response.
fn short_doc() -> String {
    String::from("glyph foo 4 2\n@@......\n......@@\n// trailing note")
}

#[test]
fn click_below_the_last_line_lands_on_it() {
    let mut h = EditorHarness::new(&short_doc());
    let last = h.lines.len() - 1;
    assert_eq!(h.text(last), "// trailing note");

    let on_line = h.text_pos(last, 6);
    let below = egui::pos2(on_line.x, h.content_bottom() + 40.0);
    h.click_at(below);
    assert_eq!(h.cursor(), Caret::new(last, 6));
}

#[test]
fn dragging_into_the_empty_band_selects_to_the_last_line() {
    let mut h = EditorHarness::new(&short_doc());
    let last = h.lines.len() - 1;

    let start = h.text_pos(last, 2);
    let end_x = h.text_pos(last, 9).x;
    let below = egui::pos2(end_x, h.content_bottom() + 40.0);
    h.press_at(start);
    // Straight down first, past egui's drag threshold: the drag starts inside
    // the empty band at the same x, which is what column 2 is anchored on.
    h.move_pointer(egui::pos2(start.x, start.y + 12.0));
    h.move_pointer(below);
    h.release_at(below);
    h.frame();

    assert_eq!(
        h.state.selection_range(),
        Some((Caret::new(last, 2), Caret::new(last, 9)))
    );
}

#[test]
fn right_click_in_the_empty_band_opens_the_edit_menu() {
    let mut h = EditorHarness::new(&short_doc());
    h.click_text(0, 0);
    h.type_text("X");
    assert_eq!(h.text(0), "Xglyph foo 4 2");

    let below = egui::pos2(h.text_pos(0, 0).x, h.content_bottom() + 40.0);
    h.right_click_at(below);
    h.frame();

    // The first item of the edit menu is Undo; egui lays it out just inside
    // the menu frame at the click position.
    let item = below + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    assert_eq!(
        h.text(0),
        "glyph foo 4 2",
        "the menu's Undo should have run"
    );
}

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------
