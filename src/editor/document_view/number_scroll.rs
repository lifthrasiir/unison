//! Alt + wheel over the editor: step the number at the caret up or down.
//!
//! The gesture is anchored to the *caret*, not to what the pointer is over —
//! the pointer only has to be somewhere over this editor, which is what makes
//! the wheel reach a number the mouse is nowhere near. The wheel step itself
//! is [`debounced_scroll_step`], the same one coarse tick the zoom handler
//! reads, so one physical notch is one increment on every input device.
//!
//! Numbers are non-negative integers of unbounded width, so the arithmetic is
//! done on the digit *string* (`[0-8]9*$` carries on increment, `[1-9]0*$` on
//! decrement) rather than through an integer that would cap out at some width.

use super::*;

/// A number the wheel resolved to, ready to be written back. Detection runs
/// before the scroll area consumes the wheel; the edit itself is applied
/// after the paint pass, with the other document edits of the frame.
pub(super) struct NumberBump {
    line: usize,
    /// Character columns of the digit run being replaced.
    start: usize,
    end: usize,
    /// The stepped digits.
    text: String,
}

/// The digit run the caret is *in or next to*, as character columns. `None`
/// when neither neighbouring character is a digit — the gesture then does
/// nothing at all and the wheel keeps its usual meaning.
fn digits_around(text: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let after = chars.get(col).is_some_and(char::is_ascii_digit);
    let before = col > 0 && chars[col - 1].is_ascii_digit();
    if !after && !before {
        return None;
    }
    let mut start = col;
    while start > 0 && chars[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    Some((start, end))
}

/// The digit run inside an existing selection, which must be exactly
/// `\s*[0-9]+\s*` — anything else could not be stepped without guessing which
/// part of it is the number, so it is left alone.
fn digits_in_selection(text: &str, lo: usize, hi: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if lo >= hi || hi > chars.len() {
        return None;
    }
    let sel = &chars[lo..hi];
    let lead = sel.iter().take_while(|c| c.is_whitespace()).count();
    let trail = sel.iter().rev().take_while(|c| c.is_whitespace()).count();
    if lead + trail >= sel.len() {
        return None;
    }
    let digits = &sel[lead..sel.len() - trail];
    if !digits.iter().all(char::is_ascii_digit) {
        return None;
    }
    Some((lo + lead, hi - trail))
}

/// `digits` stepped by one in `delta`'s direction, on the string rather than
/// through an integer, so the width is unbounded. Decrementing zero stays at
/// zero (numbers here are never negative), and a decrement that would leave a
/// leading zero the input did not have drops it: `10` → `9`, but `007` → `006`.
fn step_digits(digits: &str, delta: i32) -> String {
    let mut d: Vec<u8> = digits.bytes().collect();
    if delta >= 0 {
        match d.iter().rposition(|&c| c != b'9') {
            Some(i) => {
                d[i] += 1;
                d[i + 1..].fill(b'0');
            }
            // All nines: the number gains a digit.
            None => {
                d.fill(b'0');
                d.insert(0, b'1');
            }
        }
    } else {
        let Some(i) = d.iter().rposition(|&c| c != b'0') else {
            // Zero, in whatever width it was written.
            return digits.to_string();
        };
        d[i] -= 1;
        d[i + 1..].fill(b'9');
        if d[0] == b'0' && digits.as_bytes()[0] != b'0' {
            let keep = d.iter().position(|&c| c != b'0').unwrap_or(d.len() - 1);
            d.drain(..keep);
        }
    }
    String::from_utf8(d).expect("digits stay ASCII")
}

/// Reads this frame's Alt + wheel gesture, if it lands on a number.
///
/// Runs *before* the scroll area, so a gesture that resolves to a number can
/// take the wheel delta away from it — otherwise the view would scroll as
/// well. A gesture that resolves to nothing is left untouched and scrolls as
/// usual.
pub(super) fn detect_number_bump(
    ui: &egui::Ui,
    lines: &[DocLine],
    state: &EditorState,
    editor_rect: egui::Rect,
) -> Option<NumberBump> {
    if !state.active
        || !matches!(state.mode, EditMode::Normal)
        || !matches!(state.popup, PopupState::None)
        || state.autocomplete.is_some()
    {
        return None;
    }
    let modifiers_ok = ui.input(|i| {
        let m = i.modifiers;
        m.alt && !m.command && !m.ctrl && !m.shift
    });
    if !modifiers_ok {
        return None;
    }
    // Any point over *this* editor qualifies; a wheel over the other pane is
    // that pane's gesture, not this one's.
    let over_editor = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| editor_rect.contains(p));
    if !over_editor {
        return None;
    }

    let line = state.cursor.line;
    let Some(DocLine::Text(text)) = lines.get(line) else {
        return None;
    };
    let (start, end) = match state.selection_range() {
        Some((lo, hi)) if lo != hi => {
            if lo.line != line || hi.line != line {
                return None;
            }
            digits_in_selection(text, lo.col, hi.col)?
        }
        _ => digits_around(text, state.cursor.col)?,
    };

    // Only now, with a number in hand, is the wheel this gesture's to take.
    let step = debounced_scroll_step(ui.ctx())?;
    let delta = if step < 0 { 1 } else { -1 };
    let digits: String = text.chars().take(end).skip(start).collect();
    Some(NumberBump {
        line,
        start,
        end,
        text: step_digits(&digits, delta),
    })
}

/// Keeps the wheel away from the scroll area for as long as the gesture's
/// delta is still arriving. Called every frame, with `bumped` set on the
/// frames a tick was actually consumed.
///
/// One notch cannot be swallowed in a single frame: egui pushes a discrete
/// wheel event into its private `unprocessed_scroll_delta` and drips it into
/// `smooth_scroll_delta` over the following frames, so zeroing that delta on
/// the gesture's own frame stops only the first slice of the notch and the
/// rest still scrolls the view. There is no way to clear the reservoir, so the
/// editor instead keeps zeroing what comes out of it until it runs dry.
pub(super) fn swallow_wheel_delta(ui: &egui::Ui, state: &EditorState, bumped: bool) {
    let id = state.key(Slot::ScrollSwallow);
    let armed = bumped || ui.ctx().data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    if !armed {
        return;
    }
    let residual = ui.input(|i| i.smooth_scroll_delta.y.abs());
    ui.ctx()
        .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    // Repaint while it drains: without further input no frame would run, and
    // the reservoir would empty into whatever frame comes next instead.
    let draining = bumped || residual > 0.1;
    ui.ctx().data_mut(|d| d.insert_temp(id, draining));
    if draining {
        ui.ctx().request_repaint();
    }
}

/// Writes a detected bump back and leaves the new number selected, so the
/// next tick of the same gesture steps it again.
///
/// The write is one span replacement, which is what lets a run of ticks
/// coalesce into a single undo entry: each tick's `old` is the digits the
/// previous tick wrote, so [`UndoStack::push_text`] folds them together for as
/// long as the ticks keep coming inside the coalesce window.
///
/// [`UndoStack::push_text`]: crate::editor::undo::UndoStack::push_text
pub(super) fn apply_number_bump(
    lines: &mut [DocLine],
    state: &mut EditorState,
    bump: NumberBump,
) -> bool {
    let NumberBump {
        line,
        start,
        end,
        text,
    } = bump;
    let anchor = Caret::new(line, start);
    state.cursor = crate::editor::editing::replace_in_line(
        lines,
        &mut state.undo,
        line,
        start,
        end,
        &text,
        state.cursor,
    );
    state.selection_anchor = Some(anchor);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_found_inside_and_beside_a_run() {
        // Inside, at either edge, and nowhere near.
        assert_eq!(digits_around("ref sp 0 12", 10), Some((9, 11)));
        assert_eq!(digits_around("ref sp 0 12", 9), Some((9, 11)));
        assert_eq!(digits_around("ref sp 0 12", 11), Some((9, 11)));
        // Beside the lone "0" a step later on, and away from every digit.
        assert_eq!(digits_around("ref sp 0 12", 8), Some((7, 8)));
        assert_eq!(digits_around("ref sp 0 12", 6), None);
        assert_eq!(digits_around("ref sp 0 12", 2), None);
        // A caret between two runs takes the one it is already inside.
        assert_eq!(digits_around("12 34", 2), Some((0, 2)));
        assert_eq!(digits_around("12 34", 3), Some((3, 5)));
    }

    #[test]
    fn a_selection_is_a_number_only_when_it_holds_nothing_else() {
        assert_eq!(digits_in_selection("a 12 b", 1, 5), Some((2, 4)));
        assert_eq!(digits_in_selection("a 12 b", 2, 4), Some((2, 4)));
        assert_eq!(digits_in_selection("a 12 b", 0, 4), None);
        assert_eq!(digits_in_selection("a 12 b", 2, 6), None);
        assert_eq!(digits_in_selection("a 12 b", 1, 2), None);
        assert_eq!(digits_in_selection("a 12 b", 3, 3), None);
    }

    #[test]
    fn stepping_carries_across_any_number_of_digits() {
        assert_eq!(step_digits("0", 1), "1");
        assert_eq!(step_digits("8", 1), "9");
        assert_eq!(step_digits("9", 1), "10");
        assert_eq!(step_digits("199", 1), "200");
        assert_eq!(step_digits("999", 1), "1000");
        // Wider than any integer type this could have been parsed into.
        let huge = "9".repeat(64);
        assert_eq!(step_digits(&huge, 1), format!("1{}", "0".repeat(64)));
        assert_eq!(
            step_digits(&format!("1{}", "0".repeat(64)), -1),
            "9".repeat(64)
        );
    }

    #[test]
    fn stepping_down_stops_at_zero_and_keeps_written_padding() {
        assert_eq!(step_digits("1", -1), "0");
        assert_eq!(step_digits("0", -1), "0");
        assert_eq!(step_digits("00", -1), "00");
        assert_eq!(step_digits("10", -1), "9");
        assert_eq!(step_digits("100", -1), "99");
        // Zero-padded input keeps its width.
        assert_eq!(step_digits("007", -1), "006");
        assert_eq!(step_digits("010", -1), "009");
        assert_eq!(step_digits("009", 1), "010");
    }
}
