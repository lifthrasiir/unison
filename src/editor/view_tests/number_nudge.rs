//! Alt+wheel and Alt+arrows over a number: what they take, what they leave
//! alone, and how the ticks fold into one undo.

use super::*;

/// Alt + wheel bumps the number the caret sits in, and it does so with the
/// *pointer* anywhere over the editor — the gesture is anchored to the caret,
/// not to what is under the mouse.
#[test]
fn alt_wheel_increments_the_number_at_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 13); // between the digits of "16"
    let elsewhere = h.text_pos(1, 2);

    h.alt_wheel_at(elsewhere, true);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
    // The bumped number is left selected, so the next tick repeats on it.
    assert_eq!(h.cursor(), Caret { line: 0, col: 14 });
    assert_eq!(h.state.selection_anchor, Some(Caret { line: 0, col: 12 }));

    h.alt_wheel_at(elsewhere, false);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    h.alt_wheel_at(elsewhere, false);
    assert_eq!(h.text(0), "meta height 15 // and a comment");
}

/// Alt + Up/Down is the keyboard spelling of the same gesture: Up steps the
/// number up, Down steps it down, and the caret stays on the line instead of
/// moving as a bare arrow would.
#[test]
fn alt_arrows_step_the_number_at_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 13); // between the digits of "16"

    h.key_mod(Key::ArrowUp, Modifiers::ALT);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
    assert_eq!(h.cursor(), Caret { line: 0, col: 14 });
    assert_eq!(h.state.selection_anchor, Some(Caret { line: 0, col: 12 }));

    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(0), "meta height 15 // and a comment");
    assert_eq!(h.cursor().line, 0, "the caret never left the line");
}

/// With no number to step, Alt + Up/Down does nothing at all: like Alt +
/// wheel it is the number gesture and never falls back to the arrow's usual
/// meaning, so the caret stays put.
#[test]
fn alt_arrows_away_from_a_digit_do_nothing() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(1, 3); // inside "meta" on the second line
    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(1), "meta ascent 12");
    assert_eq!(h.cursor(), Caret { line: 1, col: 3 });

    h.key_mod(Key::ArrowUp, Modifiers::ALT);
    assert_eq!(h.cursor(), Caret { line: 1, col: 3 });
}

/// The caret only has to be *adjacent* to a digit run: at its right edge the
/// preceding digits are what gets bumped.
#[test]
fn alt_wheel_takes_the_digits_before_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 14); // right after "16"
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
}

/// With no digit anywhere around the caret the gesture does nothing at all.
#[test]
fn alt_wheel_away_from_any_digit_does_nothing() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 3); // inside "meta"
    let pos = h.text_pos(0, 3);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    assert_eq!(h.state.selection_anchor, None);
}

/// A selection that is not a bare number is left alone: bumping it would have
/// to guess which of its parts is the number.
#[test]
fn alt_wheel_ignores_a_selection_that_is_not_a_number() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 12);
    for _ in 0..4 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT); // "16 /"
    }
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
}

/// A selection of one number padded with spaces is a number: the digits move
/// and the padding stays.
#[test]
fn alt_wheel_accepts_a_number_selection_with_surrounding_space() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 11); // the space before "16"
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    }
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
}

/// Numbers are non-negative: wheeling down at zero leaves it at zero.
#[test]
fn alt_wheel_down_at_zero_stays_at_zero() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(2, 13); // the "0" of "descent 0"
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, false);
    assert_eq!(h.text(2), "meta descent 0");
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(2), "meta descent 1");
}

/// A run of ticks is one edit: the numbers scroll past several values, and a
/// single undo takes the whole run back — as typing does within its coalesce
/// window.
#[test]
fn alt_wheel_ticks_coalesce_into_one_undo() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 13);
    let pos = h.text_pos(0, 0);
    for _ in 0..3 {
        h.alt_wheel_at(pos, true);
    }
    assert_eq!(h.text(0), "meta height 19 // and a comment");

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    assert_eq!(
        h.cursor(),
        Caret { line: 0, col: 13 },
        "back to the pre-gesture caret"
    );
    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        h.text(0),
        "meta height 16 // and a comment",
        "nothing left to undo"
    );
}

/// The wheel notch belongs to the number, not to the view. egui spreads one
/// discrete notch over several frames, so a gesture that only consumed its own
/// frame left the rest of the notch to scroll the document under the caret.
#[test]
fn alt_wheel_does_not_also_scroll_the_view() {
    let src = format!("meta height 16\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 13);
    let pos = h.text_pos(0, 2);
    assert_eq!(h.scroll_y(), 0.0);

    // Wheel *down*: the direction that would scroll away from the top.
    h.alt_wheel_at(pos, false);
    for _ in 0..20 {
        h.frame();
    }
    assert_eq!(h.text(0), "meta height 15");
    assert!(
        h.scroll_y() < 0.01,
        "the view scrolled too: y = {}",
        h.scroll_y()
    );
}

/// Alt + wheel is the number gesture and nothing else: with no number to
/// step it does *not* fall back to scrolling the view, so a stray Alt never
/// moves the document under the caret.
#[test]
fn alt_wheel_with_no_number_does_not_scroll_the_view() {
    let src = format!("meta height 16\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 3); // inside "meta": no digit anywhere around the caret
    let pos = h.text_pos(0, 2);
    assert_eq!(h.scroll_y(), 0.0);

    h.alt_wheel_at(pos, false);
    for _ in 0..20 {
        h.frame();
    }
    assert_eq!(h.text(0), "meta height 16");
    assert!(
        h.scroll_y() < 0.01,
        "the view scrolled on a fruitless Alt gesture: y = {}",
        h.scroll_y()
    );
}

/// Swallowing the notch must not latch: once the gesture's delta has drained,
/// an ordinary wheel scrolls the view as before.
#[test]
fn a_plain_wheel_still_scrolls_after_an_alt_gesture() {
    let src = format!("meta height 16\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 13);
    let pos = h.text_pos(0, 2);

    h.alt_wheel_at(pos, false);
    for _ in 0..20 {
        h.frame();
    }
    assert!(h.scroll_y() < 0.01);

    h.frame_with(
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                modifiers: Modifiers::NONE,
            },
        ],
        Modifiers::NONE,
    );
    for _ in 0..20 {
        h.frame();
    }
    assert!(
        h.scroll_y() > 1.0,
        "the view no longer scrolls; y = {}",
        h.scroll_y()
    );
}

// ---------------------------------------------------------------------------
// Following links (the input side of go back / go forward)
// ---------------------------------------------------------------------------
