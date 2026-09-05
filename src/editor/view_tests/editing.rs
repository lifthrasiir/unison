//! Typing into the document, and the deferred grid resize a header edit
//! schedules.

use super::*;

#[test]
fn click_then_type_places_text() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 6);
    assert_eq!(h.cursor(), Caret::new(0, 6));
    h.type_text("X");
    assert_eq!(h.text(0), "glyph Xfoo 16 16");
}

#[test]
fn typing_inserts_text_and_marks_document_dirty() {
    let mut h = EditorHarness::new(&sample_doc());
    assert!(!h.doc.dirty);

    h.click_text(0, 0);
    h.type_text("// ");
    assert_eq!(h.text(0), "// glyph foo 16 16");
    assert!(h.doc.dirty);

    undo_all(&mut h);
    assert_eq!(h.text(0), "glyph foo 16 16");
    assert!(!h.doc.dirty);
}

#[test]
fn delete_key_updates_immediately() {
    let mut h = EditorHarness::new(&sample_doc());
    // Place cursor at start of "glyph foo 16 16"
    h.click_text(0, 0);
    h.key(Key::Delete);
    assert_eq!(h.text(0), "lyph foo 16 16");
    assert!(h.doc.dirty);
}

#[test]
fn click_grid_enters_glyph_edit() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit for item_idx 0"
    );
}

#[test]
fn header_height_edit_resizes_grid_when_caret_leaves_line() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // "glyph foo 16 16" -> "glyph foo 16 8"
    // Select trailing "16" by navigating to end, backspace twice.
    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.text(0), "glyph foo 16 8");

    // Grid hasn't changed yet — resize is deferred while the caret is on the
    // header line.
    assert_eq!(
        h.grid_row_count(1),
        16,
        "grid is still 16 rows while deferred"
    );
    assert_eq!(h.gutter_of(3), Some(19), "gutter hasn't changed yet");

    // Move the caret off the header line — now the grid resizes.
    h.key(Key::ArrowDown);
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (16, 8));
    assert!(!grid.get(5, 5).is_clear(), "surviving pixel kept");
    assert_eq!(h.grid_row_count(1), 8, "grid widget shrank to 8 rows");

    // All following lines moved up by 8 source lines.
    assert_eq!(h.gutter_of(2), Some(10));
    assert_eq!(h.gutter_of(3), Some(11));
    assert_eq!(h.gutter_numbers(), (1..=13).collect::<Vec<_>>());

    // Undo everything: header text, grid size, truncated pixels, gutter.
    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_eq!(h.text(0), "glyph foo 16 16");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(
        !h.grid(1).get(12, 12).is_clear(),
        "truncated pixel restored"
    );
    assert_eq!(h.gutter_of(3), Some(19));
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

/// The grid resize is a consequence of the header edit, not a separate user
/// action: one undo has to take the text *and* the grid back together. Undoing
/// only the resize would leave a `18 16` header over an 8-wide grid, which the
/// reparse then renders as an empty 18-wide grid.
#[test]
fn header_dimension_edit_undoes_in_one_step() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // "glyph foo 16 16" -> "glyph foo 18 16"
    h.click_text(0, 12); // just after the width "16"
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("18");
    assert_eq!(h.text(0), "glyph foo 18 16");

    h.key(Key::ArrowDown);
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));

    cmd_z(&mut h);
    assert_eq!(
        h.text(0),
        "glyph foo 16 16",
        "header text restored by one undo"
    );
    assert_eq!(h.lines, original_lines, "grid restored by the same undo");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(
        !h.state.undo.can_undo(),
        "no leftover undo entry for the resize"
    );

    // Redo has to bring both sides back in one step too.
    h.key_mod(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph foo 18 16");
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));
    assert!(
        !h.state.undo.can_redo(),
        "no leftover redo entry for the resize"
    );
}

/// Same as above, but the deferred resize is flushed by the editor losing
/// keyboard focus rather than by caret movement.
#[test]
fn header_height_edit_resizes_grid_on_focus_loss() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.grid_row_count(1), 16, "still deferred while focused");

    h.blur();
    h.frame();
    assert_eq!(h.grid(1).height, 8);
    assert_eq!(h.grid_row_count(1), 8);
    assert_eq!(h.gutter_of(3), Some(11));
}

/// Same as above, but the deferred resize is flushed by clicking straight into
/// the grid: entering a pixel mode leaves the header line just as surely as
/// moving the caret off it does.
#[test]
fn header_height_edit_resizes_grid_on_grid_click() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.grid_row_count(1), 16, "still deferred while editing");

    // Click into the grid without moving the caret off the header line first.
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit for item_idx 0"
    );
    assert_eq!(
        h.grid(1).height,
        8,
        "header edit applied on entering the grid"
    );
    assert_eq!(h.grid_row_count(1), 8);
    assert_eq!(h.gutter_of(3), Some(11));
}

#[test]
fn growing_header_height_expands_grid_and_gutter() {
    let mut h = EditorHarness::new(&sample_doc());

    // "glyph bar 4 2" -> "glyph bar 4 12"
    h.click_text(3, 12); // just before the "2"
    h.type_text("1");
    assert_eq!(h.text(3), "glyph bar 4 12");
    assert_eq!(h.grid_row_count(4), 2, "deferred while editing header");

    h.key(Key::ArrowUp);
    assert_eq!(h.grid(4).height, 12);
    assert_eq!(h.grid_row_count(4), 12);
    // bar's grid rows now cover source lines 20..=31.
    assert_eq!(h.gutter_numbers(), (1..=31).collect::<Vec<_>>());

    undo_all(&mut h);
    assert_eq!(h.grid(4).height, 2);
}

/// Cmd+Up/Down are the macOS way to reach the ends of a document. The same
/// physical chord elsewhere is Ctrl+Up/Down, where it means nothing and only
/// gets in the way — Ctrl+Home/End is the shortcut there. So the jump is the
/// Command chord alone, told apart by `mac_cmd` rather than by the
/// platform-folded `command`, and a Ctrl+Up/Down is swallowed rather than
/// falling back to a plain Up/Down.
#[test]
fn only_the_command_chord_jumps_to_the_ends_of_the_document_with_arrows() {
    let mac_cmd = Modifiers::MAC_CMD | Modifiers::COMMAND;
    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };

    let mut h = EditorHarness::new("// one\n// two\n// three\n");
    h.click_text(1, 0);
    let start = h.cursor();

    h.key_mod(Key::ArrowUp, mac_cmd);
    assert_eq!(h.cursor(), Caret::zero(), "Cmd+Up goes to the top");
    h.key_mod(Key::ArrowDown, mac_cmd);
    assert!(h.cursor().line > start.line, "Cmd+Down goes to the end");

    h.click_text(1, 0);
    h.key_mod(Key::ArrowUp, ctrl);
    assert_eq!(h.cursor(), start, "Ctrl+Up does nothing at all");
    h.key_mod(Key::ArrowDown, ctrl);
    assert_eq!(h.cursor(), start, "Ctrl+Down does nothing at all");

    // Ctrl/Cmd+Home/End stay the portable way to the ends.
    h.key_mod(Key::Home, ctrl);
    assert_eq!(h.cursor(), Caret::zero());
    h.key_mod(Key::End, ctrl);
    assert!(h.cursor().line > start.line);
}

// ---------------------------------------------------------------------------
// Right-click and the caret
// ---------------------------------------------------------------------------

/// The context menu acts on the line the caret is on (Inline once and the
/// flatten beside it read it through `inline_target_at_line`), so a right-click
/// has to put the caret where it landed before the menu opens.
#[test]
fn right_click_moves_the_caret_first() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 0);

    let line = 3;
    assert_eq!(h.text(line), "glyph bar 4 2");
    let pos = h.text_pos(line, 4);
    h.right_click_at(pos);

    assert_eq!(h.cursor(), Caret::new(line, 4));
    assert_eq!(h.state.selection_range(), None);
}

/// A right-click inside the selection is how a menu acts *on* that selection
/// (Cut, Copy), so it must leave the selection alone.
#[test]
fn right_click_inside_a_selection_keeps_it() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 2);
    for _ in 0..7 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    }
    let sel = h.state.selection_range();
    assert_eq!(sel, Some((Caret::new(0, 2), Caret::new(0, 9))));

    let pos = h.text_pos(0, 5);
    h.right_click_at(pos);

    assert_eq!(h.state.selection_range(), sel);
}

/// A right-click on an unfocused editor is what makes it the focused one: the
/// caret it just moved is only visible — and only typed into — with focus.
#[test]
fn right_click_takes_focus_first() {
    let mut h = EditorHarness::new(&sample_doc());
    h.blur();
    assert!(!h.editor_has_focus(), "precondition: focus is elsewhere");

    let pos = h.text_pos(3, 4);
    h.right_click_at(pos);

    assert!(h.editor_has_focus());
    assert_eq!(h.cursor(), Caret::new(3, 4));
}

/// Commenting a line back out has to reach the rebuild. The gate compares the
/// items the *rebuild* reads, and a line on its way to `//` passes through an
/// unrecognized directive — an item the font ignores but `issues` reports. When
/// the gate asked only whether the font would change, the last keystroke turned
/// that directive into a comment with neither one affecting the font, no
/// `content_gen` bump, and so an issue list still faulting the half-typed line.
#[test]
fn commenting_a_line_back_out_advances_content_gen() {
    let mut h = EditorHarness::new("map A = foo\n");
    h.click_text(0, 0);

    let before = h.doc.content_gen;
    h.type_text("/");
    assert_eq!(h.text(0), "/map A = foo");
    let half_typed = h.doc.content_gen;
    assert!(
        half_typed > before,
        "a `map` line becoming an unrecognized directive is a change"
    );

    h.type_text("/");
    assert_eq!(h.text(0), "//map A = foo");
    assert!(
        h.doc.content_gen > half_typed,
        "an unrecognized directive becoming a comment is a change too"
    );
}

/// The other half of the same gate: typing *inside* a comment rebuilds nothing.
/// A comment's text is read by nothing the rebuild produces, and a font source
/// is largely comments.
#[test]
fn typing_inside_a_comment_does_not_advance_content_gen() {
    let mut h = EditorHarness::new("// note\nmap A = foo\n");
    h.click_text(0, 7);

    let before = h.doc.content_gen;
    h.type_text("s");
    assert_eq!(h.text(0), "// notes");
    assert_eq!(h.doc.content_gen, before);
}
