//! The caret-anchored popups: symbol rename and code-point entry.

use super::*;

/// Cancelling the F2 rename popup with Escape hands keyboard focus back to
/// the editor canvas, with the caret still where it was — otherwise the user
/// has to click back into the document before typing again.
#[test]
fn rename_popup_escape_restores_editor_focus() {
    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\n");
    h.click_text(0, 8);
    assert!(h.editor_has_focus());
    let caret = h.cursor();

    h.key(Key::F2);
    h.frame();
    assert!(!h.editor_has_focus(), "the popup's text field takes focus");

    h.key(Key::Escape);
    h.frame();
    assert!(h.editor_has_focus(), "focus must return to the editor");
    assert_eq!(h.cursor(), caret);
    assert_eq!(h.text(0), "glyph foo 2 1");
}

/// Confirming the rename popup with Enter likewise returns focus to the
/// editor.
#[test]
fn rename_popup_confirm_restores_editor_focus() {
    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\n");
    h.click_text(0, 8);
    h.key(Key::F2);
    h.frame();
    h.type_text("bar");
    h.key(Key::Enter);
    h.frame();
    assert!(h.editor_has_focus(), "focus must return to the editor");
}

/// F2 on a slice qualifier opens the rename popup for the *slice*, not for
/// whatever the rest of the line names.
#[test]
fn rename_popup_opens_for_a_slice_qualifier() {
    use crate::editor::PopupState;
    use crate::editor::doc_links::RenameKind;

    let mut h = EditorHarness::new("glyph a 2 1\n@@..\nmap narrow : A = a\n");
    h.click_text(2, 6);
    h.key(Key::F2);
    h.frame();
    match &h.state.popup {
        PopupState::Rename {
            original_name,
            kind,
            ..
        } => {
            assert_eq!(original_name, "narrow");
            assert_eq!(*kind, RenameKind::Slice);
        }
        other => panic!("F2 opened no rename popup: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Ctrl+K: the code point popup
// ---------------------------------------------------------------------------

/// Ctrl+K opens the code point popup, whose text field takes focus just like
/// the rename popup's.  Nothing is inserted until it is confirmed.
#[test]
fn codepoint_popup_opens_on_ctrl_k() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    assert!(h.editor_has_focus());

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::Codepoint(_)),
        "Ctrl+K opened no code point popup: {:?}",
        h.state.popup
    );
    assert!(!h.editor_has_focus(), "the popup's text field takes focus");
    assert_eq!(h.text(0), "meta name Test");
}

/// On Windows and Linux the backend sets `command` to the same value as `ctrl`
/// (only macOS keeps them apart), so the chord must be recognized with both
/// flags set.  Rejecting `command` there is what made Ctrl+K a no-op.
#[test]
fn codepoint_popup_opens_on_ctrl_k_with_command_alias() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);

    // What winit reports for Ctrl+K off the Mac.
    let win_ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    h.key_mod(Key::K, win_ctrl);
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::Codepoint(_)),
        "Ctrl+K opened no code point popup: {:?}",
        h.state.popup
    );
}

/// Cmd+K on macOS (`mac_cmd` + `command`, no `ctrl`) is not the chord.
#[test]
fn codepoint_popup_ignores_mac_cmd_k() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);

    let mac_cmd = Modifiers {
        mac_cmd: true,
        command: true,
        ..Default::default()
    };
    h.key_mod(Key::K, mac_cmd);
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::None),
        "Cmd+K must not open the code point popup: {:?}",
        h.state.popup
    );
}

/// The popup is anchored under the caret from the frame it opens, not only
/// once the digits decode to something.  The canvas is unfocused while the
/// popup owns the keyboard, and an empty preedit used to fall back to the
/// start of the line, which put the popup at the left margin until the first
/// valid digit jumped it back to the caret.
#[test]
fn codepoint_popup_anchors_at_the_caret_while_still_empty() {
    use crate::editor::document_view::popups::caret_anchor_pos;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    let caret_x = h.text_pos(0, 14).x;

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(h.state.preedit, "", "no digits typed yet");
    let empty_x = caret_anchor_pos(&h.ctx, &h.state).x;
    assert!(
        (empty_x - caret_x).abs() < 2.0,
        "empty popup anchored at {empty_x}, not at the caret {caret_x}"
    );

    // And it stays there once the digits do decode.
    h.type_text("41");
    h.frame();
    let filled_x = caret_anchor_pos(&h.ctx, &h.state).x;
    assert!(
        (filled_x - caret_x).abs() < 2.0,
        "popup with a preedit anchored at {filled_x}, not at the caret {caret_x}"
    );
}

/// While the hex digits are being typed the decoded character shows as the
/// editor's preedit — the popup drives the same preview an IME would — and
/// the document itself is untouched.
#[test]
fn codepoint_popup_previews_as_preedit() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();

    h.type_text("41");
    assert_eq!(h.state.preedit, "A", "the preview should track the digits");
    assert_eq!(h.text(0), "meta name Test", "nothing committed yet");

    h.type_text("0");
    assert_eq!(
        h.state.preedit, "\u{410}",
        "U+0410 CYRILLIC CAPITAL LETTER A"
    );
    assert_eq!(h.text(0), "meta name Test");
}

/// Enter commits the preedit at the caret, exactly as an IME commit would,
/// and hands focus back to the editor.
#[test]
fn codepoint_popup_enter_commits_the_preedit() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2603");
    h.key(Key::Enter);
    h.frame();

    assert_eq!(h.text(0), "meta name Test\u{2603}");
    assert_eq!(h.cursor(), Caret { line: 0, col: 15 });
    assert!(
        h.state.preedit.is_empty(),
        "the preedit is consumed by the commit"
    );
    assert!(matches!(h.state.popup, PopupState::None));
    assert!(h.editor_has_focus(), "focus must return to the editor");
}

/// Escape rolls the preedit back: no text, no leftover preview, focus back in
/// the editor with the caret where it was.
#[test]
fn codepoint_popup_escape_rolls_back() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    let caret = h.cursor();
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2603");
    assert_eq!(h.state.preedit, "\u{2603}");

    h.key(Key::Escape);
    h.frame();
    assert_eq!(h.text(0), "meta name Test");
    assert!(
        h.state.preedit.is_empty(),
        "the preview must not survive a cancel"
    );
    assert!(matches!(h.state.popup, PopupState::None));
    assert_eq!(h.cursor(), caret);
    assert!(h.editor_has_focus());
}

/// A code point that decodes to nothing — a lone surrogate, or no digits at
/// all — previews as nothing and commits nothing.
#[test]
fn codepoint_popup_rejects_a_surrogate() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("D800");
    assert_eq!(h.state.preedit, "", "a surrogate is not a character");

    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name Test");
    assert!(matches!(h.state.popup, PopupState::None));
}

/// Non-hex characters never reach the field, so a stray keystroke cannot
/// silently turn `2603` into something else.
#[test]
fn codepoint_popup_keeps_only_hex_digits() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();

    h.type_text("2x6g0 3");
    assert_eq!(h.state.preedit, "\u{2603}");
    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name Test\u{2603}");
}

/// Typed digits replace a selection, like any other insertion.
#[test]
fn codepoint_popup_commit_replaces_the_selection() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 10);
    h.key_mod(Key::End, Modifiers::SHIFT);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("41");
    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name A");
}

/// The status bar reads the popup's label off the editor state: the code
/// point as typed, plus the Unicode name and properties that tell the user
/// they got the one they meant.
#[test]
fn codepoint_popup_reports_the_unicode_name() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+")
    );

    h.type_text("41");
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+0041  LATIN CAPITAL LETTER A {gc=Lu eaw=Na}")
    );

    h.type_text("0000");
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+410000  (not a code point)")
    );
}

/// Every popup after the first opens on the code point *after* the last one
/// committed — and pre-selected, so the first keystroke replaces it instead of
/// appending to it. A commit that jumped elsewhere does not make the next guess
/// jump too.
#[test]
fn codepoint_popup_predicts_the_next_code_point() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);

    // The first popup guesses nothing.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+")
    );
    h.type_text("2600");
    h.key(Key::Enter);
    h.frame();

    // With one code point recorded the guess is the one after it.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2601  CLOUD {gc=So eaw=N}")
    );
    // Typing replaces the pre-selected guess rather than appending to it.
    h.type_text("2604");
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2604  COMET {gc=So eaw=N}")
    );
    h.key(Key::Enter);
    h.frame();

    // The jump from U+2600 to U+2604 is not extrapolated: the next guess is
    // still just one past the last commit.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2605  BLACK STAR {gc=So eaw=A}")
    );
    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name Test\u{2600}\u{2604}\u{2605}");
}

/// A cancelled popup records nothing, so the guess it was seeded with is still
/// the guess the next one gets.
#[test]
fn codepoint_popup_cancel_does_not_move_the_prediction() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2600");
    h.key(Key::Enter);
    h.frame();

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.key(Key::Escape);
    h.frame();

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2601  CLOUD {gc=So eaw=N}")
    );
}

/// A prediction that would land outside the code space puts the popup back to
/// guessing nothing at all.
#[test]
fn codepoint_popup_drops_a_prediction_off_the_end() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("10FFFF");
    h.key(Key::Enter);
    h.frame();

    // The next code point would be U+110000, past the last one there is.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+")
    );
}

/// Clicking elsewhere in the document also cancels the rename popup, and the
/// click keeps its usual effect: the caret moves to where it landed and the
/// editor has focus again.
#[test]
fn rename_popup_click_cancels_and_moves_the_caret() {
    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\nmap A = foo\n");
    h.click_text(2, 9);
    h.key(Key::F2);
    h.frame();
    assert!(!h.editor_has_focus());

    // Press and release inside one frame, so the click is processed while
    // the popup is still open — the path that used to swallow it.
    let pos = h.text_pos(0, 3);
    h.click_at_same_frame(pos);
    assert!(h.editor_has_focus(), "focus must return to the editor");
    assert_eq!(h.cursor(), Caret { line: 0, col: 3 });
    assert_eq!(h.text(2), "map A = foo", "the rename was cancelled");
}

/// A click on the popup's own chrome — its label, or the padding around the
/// field — must not close it: the field takes focus straight back, so what has
/// been typed survives and typing continues into the field.
#[test]
fn rename_popup_survives_a_click_on_its_own_chrome() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\n");
    h.click_text(0, 8);
    h.key(Key::F2);
    h.frame();
    h.type_text("bar");

    let panel = h.popup_rect("panel");
    h.click_at(panel.left_top() + egui::vec2(6.0, 6.0));
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::Rename { .. }),
        "clicking the panel itself must not dismiss it"
    );

    // The field, not the editor, still owns the keyboard.
    h.type_text("2");
    match &h.state.popup {
        PopupState::Rename { new_name, .. } => assert_eq!(new_name, "bar2"),
        other => panic!("the rename popup closed: {other:?}"),
    }
    assert_eq!(h.text(0), "glyph foo 2 1", "nothing committed yet");
}

/// The Rename button is the pointer's Enter: it confirms instead of handing
/// focus back to the field.
#[test]
fn rename_popup_button_confirms_the_rename() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\nmap A = foo\n");
    h.click_text(0, 8);
    h.key(Key::F2);
    h.frame();
    h.type_text("bar");

    let button = h.popup_rect("commit");
    h.click_at(button.center());
    h.frame();
    assert!(matches!(h.state.popup, PopupState::None));
    let action = h.take_rename().expect("the button must confirm the rename");
    assert_eq!(action.old_name, "foo");
    assert_eq!(action.new_name, "bar");
    assert!(h.editor_has_focus(), "focus must return to the editor");
}

/// The same two rules for the code point popup: its chrome is not a dismiss
/// target, and its Input button commits what the digits name.
#[test]
fn codepoint_popup_survives_a_click_on_its_own_chrome() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("260");

    let panel = h.popup_rect("panel");
    h.click_at(panel.left_top() + egui::vec2(6.0, 6.0));
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::Codepoint(_)),
        "clicking the panel itself must not dismiss it"
    );

    h.type_text("3");
    assert_eq!(
        h.state.preedit, "\u{2603}",
        "the field must still own the keyboard"
    );
    assert_eq!(h.text(0), "meta name Test", "nothing committed yet");
}

#[test]
fn codepoint_popup_button_commits_the_preedit() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2603");

    let button = h.popup_rect("commit");
    h.click_at(button.center());
    h.frame();
    assert!(matches!(h.state.popup, PopupState::None));
    assert_eq!(h.text(0), "meta name Test\u{2603}");
    assert!(h.state.preedit.is_empty());
    assert!(h.editor_has_focus(), "focus must return to the editor");
}

// ---------------------------------------------------------------------------
// Alt + wheel over the editor bumps the number at the caret
// ---------------------------------------------------------------------------
