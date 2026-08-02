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
/// exactly as the rename popup does).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CodepointPopup {
    hex: String,
    focus_set: bool,
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

impl CodepointPopup {
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

    /// The status-bar line: the code point as typed, and its Unicode name so
    /// a wrong digit is visible before it is committed.
    pub(crate) fn status_label(&self) -> String {
        if self.hex.is_empty() {
            return "U+".to_string();
        }
        let name = match self.character() {
            Some(ch) => unicode_names2::name(ch)
                .map(|n| n.to_string())
                .unwrap_or_else(|| "(unnamed)".to_string()),
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

        let confirmed = area
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_min_width(200.0);
                        ui.label("Type code point");
                        let resp = ui
                            .horizontal(|ui| {
                                ui.label("U+");
                                // No `char_limit`: it would truncate the raw
                                // keystrokes, so a rejected character could
                                // still push a digit off the end. The filter
                                // below is the only thing that bounds the
                                // field, and it counts digits.
                                let te =
                                    egui::TextEdit::singleline(&mut self.hex).desired_width(180.0);
                                ui.add(te)
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
                            self.focus_set = true;
                        }
                        if resp.lost_focus() {
                            // Enter confirms; anything else that took focus
                            // away (Escape, a click elsewhere) cancels.
                            return Some(ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        }
                        None
                    })
                    .inner
            })
            .inner;

        match confirmed {
            Some(true) => CodepointOutcome::Commit(self.preedit()),
            Some(false) => CodepointOutcome::Cancel,
            None => CodepointOutcome::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_hex(hex: &str) -> CodepointPopup {
        CodepointPopup {
            hex: hex.to_string(),
            focus_set: true,
        }
    }

    #[test]
    fn a_named_code_point_reports_its_name() {
        assert_eq!(
            with_hex("41").status_label(),
            "U+0041  LATIN CAPITAL LETTER A"
        );
        assert_eq!(with_hex("2603").status_label(), "U+2603  SNOWMAN");
    }

    /// Short input is padded to the conventional four digits, but a code point
    /// that genuinely needs five or six keeps them.
    #[test]
    fn the_code_point_is_padded_to_four_digits_but_not_truncated() {
        assert_eq!(with_hex("A").status_label(), "U+000A  (unnamed)");
        assert_eq!(with_hex("1F600").status_label(), "U+1F600  GRINNING FACE");
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
            with_hex("D800").status_label(),
            "U+D800  (not a code point)"
        );
        assert_eq!(with_hex("").status_label(), "U+");
    }

    /// A code point with no name at all — a private-use character — still
    /// gets a status line, so the field never goes blank mid-typing.
    #[test]
    fn an_unnamed_code_point_still_gets_a_label() {
        assert_eq!(with_hex("E000").status_label(), "U+E000  (unnamed)");
    }
}
