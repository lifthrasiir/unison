//! Alt + wheel — or Alt + Up/Down — over the editor: step the number at the
//! caret up or down.
//!
//! The gesture is anchored to the *caret*, not to what the pointer is over —
//! the pointer only has to be somewhere over this editor, which is what makes
//! the wheel reach a number the mouse is nowhere near. The wheel step itself
//! is [`debounced_scroll_step`], the same one coarse tick the zoom handler
//! reads, so one physical notch is one increment on every input device. Alt +
//! Up/Down is the keyboard spelling of the same thing, one press per step, and
//! needs no pointer at all.
//!
//! Either input *is* this gesture and has no second meaning, so one that
//! finds no number does nothing at all rather than reverting to what the bare
//! input would have done: a fruitless wheel is swallowed instead of reaching
//! the scroll area ([`alt_wheel_here`]), and a fruitless Alt + Up/Down is
//! consumed instead of moving the caret ([`swallow_alt_arrows`]). An Alt the
//! user is holding for some other reason must never scroll the document or
//! walk the caret out from under itself; the interceptor surfaces are held to
//! the same rule in [`interceptor_scroll_step`].
//!
//! [`interceptor_scroll_step`]: super::interceptor_scroll_step
//!
//! Numbers are integers of unbounded width, so the arithmetic is done on the
//! digit *string* (`[0-8]9*$` carries on increment, `[1-9]0*$` on decrement)
//! rather than through an integer that would cap out at some width. Most are
//! also non-negative, and stepping one down stops at zero: a `-` is only a
//! sign where the grammar reads the number as signed, which
//! [`line_numbers`](crate::editor::line_numbers) knows. There the sign is part
//! of the number — the caret may sit on either side of it, a selection holds
//! it, and zero steps down to `-1` — and anywhere else only the digits are.
//!
//! A split's gap is a signed number that may also be left out for zero
//! ([`GapSlot`]), and there the gesture treats "no gap" *as* the zero rather
//! than writing one: a step that lands on zero takes the gap out, and a step
//! from a slot with none writes `1` or `-1` into it. Stepping down from `1`
//! therefore reads `1`, nothing, `-1`. Digits at the caret still come first —
//! the end of `han-4ebb:5x16` steps the name's `16` as it always did — so a
//! slot is reached from the whitespace between parts, or from the edge of a
//! part that does not end in a digit. A slot that already holds a gap steps
//! that gap rather than writing a second one beside it.
//!
//! Two slots can meet at one caret column: the edge of a nested split is both
//! the line's slot outside the token and the nested split's own slot inside
//! it. The nested one wins, since the line's is also reachable from the part
//! on its other side. Where a gap was taken out, though, the caret alone
//! cannot always say which of the two it was in — `a|b 1 c|d` leaves it at an
//! edge of `c|d` — and it may even end up beside a digit that would claim it
//! (`b:3x4 1`). So the omission remembers its slot ([`OmittedGap`]), and the
//! next tick goes back there for as long as neither the caret nor the line
//! has moved.
//!
//! [`GapSlot`]: crate::editor::line_numbers::GapSlot

use std::ops::Range;

use super::*;
use crate::editor::line_numbers::{GapSlot, gap_slots, signed_numbers};

/// A number the wheel resolved to, ready to be written back. Detection runs
/// before the scroll area consumes the wheel; the edit itself is applied
/// after the paint pass, with the other document edits of the frame.
pub(super) struct NumberBump {
    line: usize,
    /// Character columns being replaced: the number itself, sign included,
    /// or for a gap the whole of its slot's region.
    start: usize,
    end: usize,
    /// What replaces them.
    text: String,
    /// The columns of the stepped number once written, left selected; empty
    /// where a gap stepped to zero was taken out, and the caret goes there.
    number: Range<usize>,
    /// For a gap, whether its slot is a nested split's — what an omission
    /// has to remember besides `start`.
    nested: Option<bool>,
}

/// Where the last tick took a gap out, so that the next one writes it back
/// into the same slot; see the module docs. Stale as soon as the caret or the
/// line differs from what the omission left, which is checked rather than
/// cleared.
#[derive(Clone, Debug)]
pub(crate) struct OmittedGap {
    line: usize,
    col: usize,
    text: String,
    /// The slot, as its region's start (which a write into it never moves)
    /// and whether it is a nested split's.
    region_start: usize,
    nested: bool,
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

/// The number inside an existing selection, which must be exactly
/// `\s*[0-9]+\s*`, or `\s*-?[0-9]+\s*` over one of the line's `signed`
/// numbers — anything else could not be stepped without guessing which part
/// of it is the number, so it is left alone. Also says whether the number
/// found is signed.
fn number_in_selection(
    text: &str,
    signed: &[Range<usize>],
    lo: usize,
    hi: usize,
) -> Option<(usize, usize, bool)> {
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
    let (start, end) = (lo + lead, hi - trail);
    if signed.contains(&(start..end)) {
        return Some((start, end, true));
    }
    let digits = &sel[lead..sel.len() - trail];
    if !digits.iter().all(char::is_ascii_digit) {
        return None;
    }
    Some((start, end, false))
}

/// The number at the caret: one of the line's `signed` numbers the caret is
/// in or next to — so a caret on the sign's side of `-1` finds it too — or
/// else the plain digit run of [`digits_around`]. Also says which it was.
fn number_around(text: &str, signed: &[Range<usize>], col: usize) -> Option<(usize, usize, bool)> {
    // Signed numbers are whole tokens (or whole `..`/`|` pieces), so no two
    // of them touch and at most one can hold the caret.
    if let Some(r) = signed.iter().find(|r| r.start <= col && col <= r.end) {
        return Some((r.start, r.end, true));
    }
    digits_around(text, col).map(|(start, end)| (start, end, false))
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

/// Whether `number` is zero, in whatever width and with whatever sign.
fn is_zero(number: &str) -> bool {
    number.trim_start_matches('-').bytes().all(|b| b == b'0')
}

/// `number` stepped by one in `delta`'s direction. An unsigned number is
/// [`step_digits`] and stops at zero; a `signed` one steps its magnitude away
/// from or toward zero and crosses it, the sign appearing below zero and
/// disappearing at it: `1` → `0` → `-1`, and `-1` → `0` rather than `-0`.
/// Written padding is kept on both sides of zero (`-01` → `00`).
fn step_number(number: &str, delta: i32, signed: bool) -> String {
    let (negative, digits) = match number.strip_prefix('-') {
        Some(digits) if signed => (true, digits),
        _ => (false, number),
    };
    if is_zero(digits) {
        // Zero has no sign to keep, whichever it was written with.
        return if signed && delta < 0 {
            format!("-{}", step_digits(digits, 1))
        } else {
            step_digits(digits, delta)
        };
    }
    let away_from_zero = (delta >= 0) != negative;
    let stepped = step_digits(digits, if away_from_zero { 1 } else { -1 });
    if negative && !is_zero(&stepped) {
        format!("-{stepped}")
    } else {
        stepped
    }
}

/// Alt, and nothing else. A chord that adds Ctrl/Cmd/Shift belongs to whoever
/// else claims it, so it is neither this gesture nor swallowed by it.
fn alt_only(ui: &egui::Ui) -> bool {
    ui.input(|i| {
        let m = i.modifiers;
        m.alt && !m.command && !m.ctrl && !m.shift
    })
}

/// Whether an Alt + wheel gesture over this editor is in progress: Alt alone,
/// a wheel event this frame, and the pointer somewhere over `editor_rect` (a
/// wheel over the other pane is that pane's gesture).
///
/// This is deliberately *not* conditioned on a number being found: the whole
/// point is that Alt + wheel means one thing, so the notch is taken away from
/// the scroll area whether or not the caret had a number to step.
pub(super) fn alt_wheel_here(ui: &egui::Ui, editor_rect: egui::Rect) -> bool {
    if !alt_only(ui) {
        return false;
    }
    ui.input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|p| editor_rect.contains(p))
            && i.events
                .iter()
                .any(|e| matches!(e, egui::Event::MouseWheel { .. }))
    })
}

/// Reads this frame's Alt + wheel or Alt + Up/Down gesture, if it lands on a
/// number.
///
/// Runs *before* the scroll area and before [`handle_document_keys`], so a
/// gesture that resolves to a number can take its input away from them —
/// otherwise the view would scroll, or the caret would change line, as well.
/// A gesture that resolves to nothing takes its input away all the same: the
/// caller swallows the wheel notch and the arrow press, since neither chord
/// has another meaning to fall back to.
///
/// [`handle_document_keys`]: super::keys::handle_document_keys
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
    if !alt_only(ui) {
        return None;
    }
    // Which input is asking. An arrow goes wherever the keyboard focus is,
    // which `state.active` already settled; a wheel has to be over *this*
    // editor, since a wheel over the other pane is that pane's gesture. Any
    // point over it qualifies — the caret is what the gesture is anchored to.
    let key = ui.input(|i| {
        [egui::Key::ArrowUp, egui::Key::ArrowDown]
            .into_iter()
            .find(|&k| i.key_pressed(k))
    });
    if key.is_none() && !alt_wheel_here(ui, editor_rect) {
        return None;
    }

    let line = state.cursor.line;
    let Some(DocLine::Text(text)) = lines.get(line) else {
        return None;
    };
    let col = state.cursor.col;
    let signed = signed_numbers(text);
    let slots = gap_slots(text);
    let target = match state.selection_range() {
        Some((lo, hi)) if lo != hi => {
            if lo.line != line || hi.line != line {
                return None;
            }
            Target::Number(number_in_selection(text, &signed, lo.col, hi.col)?)
        }
        _ => {
            let remembered = state
                .omitted_gap
                .as_ref()
                .filter(|m| m.line == line && m.col == col && *text == *m.text);
            match remembered {
                Some(m) => Target::Slot(
                    slots
                        .iter()
                        .find(|s| s.region.start == m.region_start && s.nested == m.nested)?,
                ),
                None => match number_around(text, &signed, col) {
                    Some(number) => Target::Number(number),
                    None => Target::Slot(slot_at(&slots, col)?),
                },
            }
        }
    };

    // Only now, with a number in hand, is the input this gesture's to take.
    let delta = match key {
        // Consuming the press is what keeps the caret from also moving a line.
        Some(k) => {
            if !ui.input_mut(|i| i.consume_key(egui::Modifiers::ALT, k)) {
                return None;
            }
            if k == egui::Key::ArrowUp { 1 } else { -1 }
        }
        None => {
            let step = debounced_scroll_step(ui.ctx())?;
            if step < 0 { 1 } else { -1 }
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let (slot, gap) = match target {
        Target::Number((start, end, signed)) => {
            match slots.iter().find(|s| s.gaps.contains(&(start..end))) {
                Some(slot) => (slot, Some(start..end)),
                None => {
                    let stepped =
                        step_number(&chars[start..end].iter().collect::<String>(), delta, signed);
                    return Some(NumberBump {
                        line,
                        start,
                        end,
                        number: start..start + stepped.chars().count(),
                        text: stepped,
                        nested: None,
                    });
                }
            }
        }
        Target::Slot(slot) => (slot, slot.gaps.first().cloned()),
    };
    Some(step_gap(&chars, line, slot, gap, col, delta))
}

/// What the gesture resolved to: a number already written — its columns and
/// whether it is signed — or a gap slot with none at the caret.
enum Target<'a> {
    Number((usize, usize, bool)),
    Slot(&'a GapSlot),
}

/// The slot the caret is in, touching either end of its region counting. A
/// nested split's slot wins over the line's where the two meet at the token's
/// edge; see the module docs.
fn slot_at(slots: &[GapSlot], col: usize) -> Option<&GapSlot> {
    let mut at = slots
        .iter()
        .filter(|s| s.region.start <= col && col <= s.region.end);
    let first = at.next()?;
    Some(at.find(|s| s.nested).unwrap_or(first))
}

/// Steps the `gap` written in `slot`, or with none there the zero it leaves
/// out, as one rewrite of the slot's whole region. Always the whole region,
/// starting where the slot does, so that each tick replaces exactly what the
/// last one wrote at the same column — the undo stack's replace chain, which
/// is what folds writing, stepping and omitting a gap into one undo.
///
/// A gap that steps to zero is taken out with one separator beside it, the
/// one after where there is one, so `a 1 b` becomes `a b` and a trailing
/// `b 1` becomes `b`. One written from nothing goes in at the caret with a
/// separator on whichever side needs one to stay a token of its own.
fn step_gap(
    chars: &[char],
    line: usize,
    slot: &GapSlot,
    gap: Option<Range<usize>>,
    col: usize,
    delta: i32,
) -> NumberBump {
    let is_sep = |c: char| {
        if slot.nested {
            c == '|'
        } else {
            c.is_whitespace()
        }
    };
    let region = slot.region.clone();
    // What is cut out of the line, what goes in its place, and where the
    // stepped number ends up.
    let (cut, insert, number) = match gap {
        Some(gap) => {
            let stepped = step_number(&chars[gap.clone()].iter().collect::<String>(), delta, true);
            let cut = if !is_zero(&stepped) {
                let number = gap.start..gap.start + stepped.chars().count();
                return rewrite_region(chars, line, slot, gap, &stepped, number);
            } else if gap.end < region.end && is_sep(chars[gap.end]) {
                gap.start..gap.end + 1
            } else if gap.start > region.start && is_sep(chars[gap.start - 1]) {
                gap.start - 1..gap.end
            } else {
                gap
            };
            (cut.clone(), String::new(), cut.start..cut.start)
        }
        None => {
            let at = col.clamp(region.start, region.end);
            // A neighbour that is neither a separator nor outside the token
            // (whitespace, for a nested split) needs a separator between.
            let needs_sep = |c: Option<&char>| c.is_some_and(|&c| !is_sep(c) && !c.is_whitespace());
            let sep = if slot.nested { '|' } else { ' ' };
            let stepped = step_number("0", delta, true);
            let lead = needs_sep(at.checked_sub(1).and_then(|i| chars.get(i)));
            let number = at + usize::from(lead)..at + usize::from(lead) + stepped.len();
            let mut insert = String::new();
            insert.extend(lead.then_some(sep));
            insert.push_str(&stepped);
            insert.extend(needs_sep(chars.get(at)).then_some(sep));
            (at..at, insert, number)
        }
    };
    rewrite_region(chars, line, slot, cut, &insert, number)
}

/// The [`NumberBump`] that rewrites `slot`'s whole region with `cut` replaced
/// by `insert`; see [`step_gap`] for why the whole region.
fn rewrite_region(
    chars: &[char],
    line: usize,
    slot: &GapSlot,
    cut: Range<usize>,
    insert: &str,
    number: Range<usize>,
) -> NumberBump {
    let region = slot.region.clone();
    let mut text: String = chars[region.start..cut.start].iter().collect();
    text.push_str(insert);
    text.extend(&chars[cut.end..region.end]);
    NumberBump {
        line,
        start: region.start,
        end: region.end,
        text,
        number,
        nested: Some(slot.nested),
    }
}

/// Keeps the wheel away from the scroll area for as long as the gesture's
/// delta is still arriving. Called every frame, with `claimed` set on the
/// frames an Alt + wheel notch arrived over this editor — whether it stepped
/// a number or found none to step.
///
/// One notch cannot be swallowed in a single frame: egui pushes a discrete
/// wheel event into its private `unprocessed_scroll_delta` and drips it into
/// `smooth_scroll_delta` over the following frames, so zeroing that delta on
/// the gesture's own frame stops only the first slice of the notch and the
/// rest still scrolls the view. There is no way to clear the reservoir, so the
/// editor instead keeps zeroing what comes out of it until it runs dry.
pub(super) fn swallow_wheel_delta(ui: &egui::Ui, state: &EditorState, claimed: bool) {
    let id = state.key(Slot::ScrollSwallow);
    let armed = claimed || ui.ctx().data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    if !armed {
        return;
    }
    let residual = ui.input(|i| i.smooth_scroll_delta.y.abs());
    ui.ctx()
        .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    // Repaint while it drains: without further input no frame would run, and
    // the reservoir would empty into whatever frame comes next instead.
    let draining = claimed || residual > 0.1;
    ui.ctx().data_mut(|d| d.insert_temp(id, draining));
    if draining {
        ui.ctx().request_repaint();
    }
}

/// Eats an Alt + Up/Down press that [`detect_number_bump`] did not claim, so
/// a fruitless gesture stays fruitless instead of moving the caret, nudging a
/// resize or stepping whatever else the bare arrow drives.
///
/// Guarded on `state.active` alone — the pointer's counterpart in
/// [`alt_wheel_here`] — and so deliberately blind to the mode and to any
/// popup: Alt + arrow means this one thing everywhere inside the focused
/// editor. Calling it after `detect_number_bump` is what makes the successful
/// case a no-op here: that press is already consumed.
pub(super) fn swallow_alt_arrows(ui: &egui::Ui, state: &EditorState) {
    if !state.active || !alt_only(ui) {
        return;
    }
    for key in [egui::Key::ArrowUp, egui::Key::ArrowDown] {
        ui.input_mut(|i| i.consume_key(egui::Modifiers::ALT, key));
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
        number,
        nested,
    } = bump;
    crate::editor::editing::replace_in_line(
        lines,
        &mut state.undo,
        line,
        start,
        end,
        &text,
        state.cursor,
    );
    state.cursor = Caret::new(line, number.end);
    state.selection_anchor = (!number.is_empty()).then_some(Caret::new(line, number.start));
    state.omitted_gap = match (nested, lines.get(line)) {
        (Some(nested), Some(DocLine::Text(after))) if number.is_empty() => Some(OmittedGap {
            line,
            col: number.start,
            text: after.to_string(),
            region_start: start,
            nested,
        }),
        _ => None,
    };
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
        assert_eq!(
            number_in_selection("a 12 b", &[], 1, 5),
            Some((2, 4, false))
        );
        assert_eq!(
            number_in_selection("a 12 b", &[], 2, 4),
            Some((2, 4, false))
        );
        assert_eq!(number_in_selection("a 12 b", &[], 0, 4), None);
        assert_eq!(number_in_selection("a 12 b", &[], 2, 6), None);
        assert_eq!(number_in_selection("a 12 b", &[], 1, 2), None);
        assert_eq!(number_in_selection("a 12 b", &[], 3, 3), None);
    }

    #[test]
    fn a_selection_over_a_signed_number_takes_its_sign() {
        let signed = [2..4, 7..8];
        assert_eq!(
            number_in_selection("a -3 b", &signed, 1, 5),
            Some((2, 4, true))
        );
        // Unsigned, the `-` is not part of a number.
        assert_eq!(number_in_selection("a -3 b", &[], 1, 5), None);
        assert_eq!(
            number_in_selection("a -3 b", &signed, 3, 4),
            Some((3, 4, false))
        );
    }

    #[test]
    fn a_caret_on_either_side_of_the_sign_finds_a_signed_number() {
        let signed = [2..4, 7..8];
        assert_eq!(number_around("a -3 b", &signed, 2), Some((2, 4, true)));
        assert_eq!(number_around("a -3 b", &signed, 4), Some((2, 4, true)));
        assert_eq!(number_around("a -3 b", &[], 2), None);
        assert_eq!(number_around("a -3 b", &[], 3), Some((3, 4, false)));
    }

    #[test]
    fn a_signed_number_steps_through_zero() {
        assert_eq!(step_number("1", -1, true), "0");
        assert_eq!(step_number("0", -1, true), "-1");
        assert_eq!(step_number("-1", -1, true), "-2");
        assert_eq!(step_number("-1", 1, true), "0");
        assert_eq!(step_number("-10", 1, true), "-9");
        assert_eq!(step_number("-99", -1, true), "-100");
        // Zero has no sign; padding survives the crossing.
        assert_eq!(step_number("-0", 1, true), "1");
        assert_eq!(step_number("-0", -1, true), "-1");
        assert_eq!(step_number("00", -1, true), "-01");
        assert_eq!(step_number("-01", 1, true), "00");
        // Unsigned, zero is the floor and a stray `-` is not a sign.
        assert_eq!(step_number("0", -1, false), "0");
        assert_eq!(step_number("5", 1, false), "6");
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
