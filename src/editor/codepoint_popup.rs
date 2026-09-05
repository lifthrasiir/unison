//! Typing a character by its code point.
//!
//! Ctrl+K opens a small caret-anchored popup — the rename popup's shape — with
//! a `U+` field that takes hex digits. What the digits decode to is shown as
//! the host's *preedit*, not as document text: Enter commits it and Escape
//! rolls it back, so the interaction is the one every IME already teaches.
//! That is also why this module produces a string rather than editing
//! anything itself; both hosts (the document editor and the shaped-preview
//! field) already render a preedit and already know how to commit one, so
//! they need no new insertion path.
//!
//! Two rules exist to stop a mistyped code point from being committed
//! unnoticed, which is what the previous Alt+hex chord made easy:
//!
//! - the field accepts hex digits only, so a stray keystroke cannot land in
//!   the middle of a number and change it silently;
//! - [`CodepointPopup::status_label`] names the code point, and the status bar
//!   shows that name while the digits are being typed.
//!
//! The field does not open empty once anything has been typed through it:
//! [`CodepointPrediction`] offers the code point after the last one committed,
//! and the popup opens on that guess with it selected, so Enter takes it and
//! any digit replaces it. A selection of exactly one character outranks that
//! guess ([`CodepointPopup::for_selection`]): the one thing Ctrl+K can be
//! asked over a character is what it is, and a commit replaces the selection
//! anyway, so opening on it costs nothing and answers that question. Any
//! wider selection names no single code point and a bare caret names none at
//! all, so both keep the prediction.
//!
//! The predecessor was Alt (Option on macOS) held down over hex keys. It could
//! not survive macOS: with an IME allowed, `Option+E` is a dead key, so AppKit
//! consumes the keystroke into a pending composition and winit never emits a
//! `KeyboardInput` event for it at all. The chord was also invisible while
//! being typed. Nothing here uses Alt.
//!
//! Both hosts detect the chord as `ctrl && !mac_cmd && !alt`. Excluding Cmd
//! has to go through `mac_cmd`: egui sets `Modifiers::command` to `ctrl` on
//! every platform but macOS, so a `!command` test silently rejects every
//! Ctrl+K on Windows and Linux.

/// The state of one open code point popup: the digits typed so far, and
/// whether its text field has been given focus yet (the first frame does it,
/// exactly as the rename popup does, selecting whatever is in the field so a
/// seeded guess is replaced by the first keystroke).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CodepointPopup {
    hex: String,
    focus_set: bool,
}

/// The guess the *next* popup opens with. Entering code points is usually
/// walking a block, so the guess is simply the one after the last committed —
/// one popup's worth of state, kept by the host beside the buffer it types
/// into, not globally.
///
/// The guess is deliberately not extrapolated from a longer history: a step
/// inferred from two earlier commits is right often enough to be trusted and
/// wrong often enough to mislead, and a wrong seed costs more than an empty
/// field. Two rules follow:
///
/// - with nothing committed yet there is no guess, so the field starts empty;
/// - a successor that is not a code point — a surrogate, or past `U+10FFFF` —
///   is no guess either, so the field starts empty rather than skipping to a
///   nearby value.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CodepointPrediction {
    last: Option<u32>,
}

impl CodepointPrediction {
    /// What the next code point is guessed to be, if anything.
    pub(crate) fn predicted(&self) -> Option<char> {
        // `char::from_u32` is the whole range check: it rejects surrogates and
        // anything past U+10FFFF.
        char::from_u32(self.last? + 1)
    }

    /// Records a committed code point. Only a commit moves the sequence on: a
    /// cancelled popup leaves the next one seeded exactly as this one was.
    pub(crate) fn record(&mut self, ch: char) {
        self.last = Some(ch as u32);
    }
}

/// What one frame of the popup asks its host to do.
pub(crate) enum CodepointOutcome {
    /// Still open; `preedit` is what the host should show at its caret.
    Open,
    /// Confirmed: the host should commit this string as it would an IME
    /// commit, and close the popup.
    Commit(String),
    /// Dismissed: the host should drop the preedit and close the popup.
    Cancel,
}

/// The longest hex string that can name a code point (`10FFFF`). Longer input
/// could only ever be out of range, so the field simply does not take it.
const MAX_HEX_LEN: usize = 6;

/// Parses an accumulated hex string into a scalar value, rejecting
/// surrogates and out-of-range code points.
pub(crate) fn validate_hex_codepoint(hex: &str) -> Option<char> {
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// One frame of a caret-anchored popup's text field, as its host saw it.
pub(crate) struct FieldFrame {
    /// The field's widget id, so a click that landed elsewhere in the panel
    /// can be handed straight back to it.
    pub id: egui::Id,
    /// The field no longer holds focus as of this frame.
    pub lost_focus: bool,
    /// Enter in the field, or the panel's own commit button — the two ways of
    /// confirming what has been typed.
    pub confirmed: bool,
}

/// What [`resolve_field`] tells the host to do with the popup.
pub(crate) enum FieldOutcome {
    /// Leave it open.
    Open,
    /// Confirmed: commit the field's contents and close.
    Commit,
    /// Dismissed: drop what was typed and close.
    Cancel,
}

/// Decides a popup's fate from one frame of its field.
///
/// The field surrenders focus to anything pressed outside it, its own panel
/// included, so `lost_focus` alone cannot say whether the popup was dismissed:
/// pressing the panel's label or the padding around the field would close it,
/// which is what made these popups feel hair-triggered. Only a press *outside*
/// `panel` dismisses; a press inside it hands focus back to the field, so the
/// panel can be clicked anywhere without losing what is being typed. Escape
/// drops focus with no press at all, and so still cancels.
///
/// The commit button is the one thing in the panel that does not bounce focus
/// back — it confirms instead, and `confirmed` is therefore tested first.
pub(crate) fn resolve_field(
    ctx: &egui::Context,
    frame: &FieldFrame,
    panel: egui::Rect,
) -> FieldOutcome {
    if frame.confirmed {
        // Whatever holds focus at this point — the field itself, or the
        // commit button that was just clicked — disappears with the popup, so
        // the focus is dropped here and `restore_host_focus` can hand the
        // keyboard back to the host. Enter's own path already clears it; a
        // button click does not.
        ctx.memory_mut(|m| m.stop_text_input());
        return FieldOutcome::Commit;
    }
    if !frame.lost_focus {
        return FieldOutcome::Open;
    }
    // A press is what takes focus away (`Context::interact_with_hovered`), so
    // it is the press position — not a mere hover — that says where the click
    // that closed the field landed.
    let pressed_inside = ctx.input(|i| {
        (i.pointer.any_pressed() || i.pointer.any_click())
            && i.pointer.interact_pos().is_some_and(|p| panel.contains(p))
    });
    if pressed_inside {
        ctx.memory_mut(|m| m.request_focus(frame.id));
        FieldOutcome::Open
    } else {
        FieldOutcome::Cancel
    }
}

/// Hands keyboard focus back to `host` after the popup closes, so typing
/// continues where it was going before Ctrl+K instead of going nowhere. The
/// popup's text field surrenders focus to no one, so without this every host
/// is left unfocused. A widget that already claimed focus this frame (the host
/// itself when the popup was dismissed by clicking back into it, or something
/// outside it) keeps it.
pub(crate) fn restore_host_focus(ctx: &egui::Context, host: egui::Id) {
    let focused = ctx.memory(|m| m.focused());
    if focused.is_none_or(|id| id == host) {
        ctx.memory_mut(|m| m.request_focus(host));
    }
}

impl CodepointPopup {
    /// A popup pre-filled with `seed`, the guess from
    /// [`CodepointPrediction::predicted`]. `None` opens it empty.
    pub(crate) fn seeded(seed: Option<char>) -> Self {
        Self {
            hex: seed
                .map(|ch| format!("{:04X}", ch as u32))
                .unwrap_or_default(),
            focus_set: false,
        }
    }

    /// The popup a host opens over `selected` — the text its selection covers,
    /// which is `None` or empty when nothing is selected. Exactly one
    /// character seeds the field with that character; anything else falls back
    /// to `prediction`.
    pub(crate) fn for_selection(selected: Option<&str>, prediction: &CodepointPrediction) -> Self {
        let one = selected.and_then(|text| {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(ch),
                _ => None,
            }
        });
        Self::seeded(one.or_else(|| prediction.predicted()))
    }

    /// The character the digits currently name, if they name one.
    pub(crate) fn character(&self) -> Option<char> {
        validate_hex_codepoint(&self.hex)
    }

    /// What the host should show at its caret this frame — empty while the
    /// digits do not (yet) name a character, so an in-progress `D8` shows
    /// nothing rather than something wrong.
    pub(crate) fn preedit(&self) -> String {
        self.character().map(String::from).unwrap_or_default()
    }

    /// The status-bar line: the code point as typed, its Unicode name and its
    /// properties ([`crate::ucd::CharProps::property_summary`]), so a wrong
    /// digit is visible before it is committed. Digits that name no character
    /// get the name slot alone — there are no properties to report for a
    /// non-character.
    ///
    /// `char_props` is what the source's `prop` lines state, so a Private Use
    /// character this font defines is named here as the font names it rather
    /// than as `(unnamed)`.
    pub(crate) fn status_label(&self, char_props: &crate::ucd::CharProps) -> String {
        if self.hex.is_empty() {
            return "U+".to_string();
        }
        let name = match self.character() {
            Some(ch) => {
                let name = char_props
                    .name(ch as u32)
                    .unwrap_or_else(|| "(unnamed)".to_string());
                format!("{name} {}", char_props.property_summary(ch))
            }
            None => "(not a code point)".to_string(),
        };
        format!("U+{:0>4}  {name}", self.hex)
    }

    /// Draws the popup at `pos` and reports what the host should do. `area_id`
    /// is the host's per-instance id for this popup, so two editors showing
    /// one at once do not collide.
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        area_id: egui::Id,
        pos: egui::Pos2,
    ) -> CodepointOutcome {
        let area = egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(pos);

        let area_resp = area.show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .show(ui, |ui| {
                    ui.set_min_width(200.0);
                    ui.label("Type code point");
                    let (resp, commit_clicked) = ui
                        .horizontal(|ui| {
                            ui.label("U+");
                            // No `char_limit`: it would truncate the raw
                            // keystrokes, so a rejected character could
                            // still push a digit off the end. The filter
                            // below is the only thing that bounds the
                            // field, and it counts digits.
                            let te = egui::TextEdit::singleline(&mut self.hex).desired_width(140.0);
                            let resp = ui.add(te);
                            // The pointer's way of pressing Enter, for anyone
                            // who reached the field with the mouse.
                            let button = ui.small_button("Input");
                            #[cfg(test)]
                            crate::editor::harness::capture_popup_rect(
                                ui.ctx(),
                                area_id,
                                "commit",
                                button.rect,
                            );
                            (resp, button.clicked())
                        })
                        .inner;

                    // Hex only, and upper case, so what the field shows and
                    // what the status bar names are the same text. Typing
                    // is filtered rather than rejected: the digits that are
                    // valid still land, as they would in any numeric field.
                    let filtered: String = self
                        .hex
                        .chars()
                        .filter(|c| c.is_ascii_hexdigit())
                        .map(|c| c.to_ascii_uppercase())
                        .take(MAX_HEX_LEN)
                        .collect();
                    if filtered != self.hex {
                        self.hex = filtered;
                    }

                    if !self.focus_set {
                        resp.request_focus();
                        // Select what is there, so a seeded guess is
                        // replaced by the first digit typed instead of
                        // being appended to. Harmless on an empty field.
                        if let Some(mut te_state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                            te_state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(self.hex.chars().count()),
                                )));
                            te_state.store(ui.ctx(), resp.id);
                        }
                        self.focus_set = true;
                    }
                    FieldFrame {
                        id: resp.id,
                        lost_focus: resp.lost_focus(),
                        confirmed: commit_clicked
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))),
                    }
                })
                .inner
        });

        #[cfg(test)]
        crate::editor::harness::capture_popup_rect(ctx, area_id, "panel", area_resp.response.rect);

        match resolve_field(ctx, &area_resp.inner, area_resp.response.rect) {
            FieldOutcome::Commit => CodepointOutcome::Commit(self.preedit()),
            FieldOutcome::Cancel => CodepointOutcome::Cancel,
            FieldOutcome::Open => CodepointOutcome::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ucd::CharProps;

    fn with_hex(hex: &str) -> CodepointPopup {
        CodepointPopup {
            hex: hex.to_string(),
            focus_set: true,
        }
    }

    #[test]
    fn a_named_code_point_reports_its_name() {
        assert_eq!(
            with_hex("41").status_label(&CharProps::default()),
            "U+0041  LATIN CAPITAL LETTER A {gc=Lu eaw=Na}"
        );
        assert_eq!(
            with_hex("2603").status_label(&CharProps::default()),
            "U+2603  SNOWMAN {gc=So eaw=N}"
        );
    }

    /// Short input is padded to the conventional four digits, but a code point
    /// that genuinely needs five or six keeps them.
    #[test]
    fn the_code_point_is_padded_to_four_digits_but_not_truncated() {
        assert_eq!(
            with_hex("A").status_label(&CharProps::default()),
            "U+000A  (unnamed) {gc=Cc eaw=N}"
        );
        assert_eq!(
            with_hex("1F600").status_label(&CharProps::default()),
            "U+1F600  GRINNING FACE {gc=So eaw=W}"
        );
    }

    /// The two ways a hex string names nothing: a surrogate, and past the end
    /// of the code space. Both preview as nothing, so neither can be
    /// committed by accident.
    #[test]
    fn a_non_character_previews_as_nothing() {
        for hex in ["D800", "110000", ""] {
            let p = with_hex(hex);
            assert_eq!(p.character(), None, "{hex} should name no character");
            assert_eq!(p.preedit(), "");
        }
        assert_eq!(
            with_hex("D800").status_label(&CharProps::default()),
            "U+D800  (not a code point)"
        );
        assert_eq!(with_hex("").status_label(&CharProps::default()), "U+");
    }

    fn after(committed: &[char]) -> Option<char> {
        let mut p = CodepointPrediction::default();
        for &ch in committed {
            p.record(ch);
        }
        p.predicted()
    }

    /// Nothing committed guesses nothing; otherwise the guess is always the
    /// code point after the last one committed, however that one was reached.
    #[test]
    fn the_prediction_is_the_code_point_after_the_last_commit() {
        assert_eq!(after(&[]), None);
        assert_eq!(after(&['\u{2600}']), Some('\u{2601}'));
        assert_eq!(after(&['\u{2600}', '\u{2604}']), Some('\u{2605}'));
        assert_eq!(after(&['\u{41}', '\u{2610}', '\u{2608}']), Some('\u{2609}'));
    }

    /// A seeded popup starts on its guess, padded the way the status line pads
    /// it, and an unseeded one starts empty.
    #[test]
    fn a_seeded_popup_starts_on_its_guess() {
        assert_eq!(CodepointPopup::seeded(Some('\u{2601}')).hex, "2601");
        assert_eq!(CodepointPopup::seeded(Some('\u{41}')).hex, "0041");
        assert_eq!(CodepointPopup::seeded(Some('\u{1F600}')).hex, "1F600");
        assert_eq!(CodepointPopup::seeded(None).hex, "");
    }

    /// The two ways the next code point is not one: the last one before the
    /// surrogate block, and the last one there is. Both guess nothing rather
    /// than skipping to a nearby value.
    #[test]
    fn an_impossible_prediction_guesses_nothing() {
        assert_eq!(after(&['\u{D7FF}']), None);
        assert_eq!(after(&['\u{10FFFF}']), None);
        // Guessing nothing once says nothing about the next commit.
        assert_eq!(after(&['\u{10FFFF}', '\u{2600}']), Some('\u{2601}'));
    }

    /// A code point with no name at all — a private-use character — still
    /// gets a status line, so the field never goes blank mid-typing.
    #[test]
    fn an_unnamed_code_point_still_gets_a_label() {
        assert_eq!(
            with_hex("E000").status_label(&CharProps::default()),
            "U+E000  (unnamed) {gc=Co eaw=A}"
        );
    }

    /// …and one the source named through a `prop` line reads as that name,
    /// with the properties the same line stated.
    #[test]
    fn a_prop_line_names_a_private_use_code_point() {
        let doc = crate::document_io::parse_document_from_str(
            "prop U+E000 = `UNISON LOGO` gc So eaw W\n",
            "t.unf".into(),
        )
        .unwrap();
        let props = CharProps::collect(&[&doc]);
        assert_eq!(
            with_hex("E000").status_label(&props),
            "U+E000  UNISON LOGO {gc=So eaw=W}"
        );
    }
}
