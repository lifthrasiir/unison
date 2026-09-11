//! The scrolling-list half of a popup, shared by the three popups that offer a
//! list to walk: autocompletion ([`super::autocomplete`]), the goto choice
//! ([`super::goto_popup`]) and the host's palette (`crate::app::palette`).
//!
//! What is shared is the *walk*, not the list: a selection index, the window it
//! is kept visible in, the keys that move it, and the loop that draws the
//! window. Every one of them is read the same way — a short window onto a
//! possibly long list, walked with the arrows and the four jump keys — and none
//! had any business spelling that out again.
//!
//! The two popups that narrow a list by typing — completion and the palette —
//! also agree on the Ctrl+J/K aliases and on Enter/Tab accepting
//! ([`read_typed_list_key`]). The goto choice narrows nothing, so it keeps the
//! plain walk and dismisses on anything else.
//!
//! What is deliberately **not** here is what a popup does with the choice, how
//! it narrows (completion by prefix, the palette by subsequence) and when it
//! closes. Those differ, and each popup keeps them.

/// How many rows a popup shows at once. Also the step `PageUp`/`PageDown` take,
/// so a page moves by exactly a window.
pub(crate) const MAX_VISIBLE: usize = 10;

/// A selection into a list, plus the offset of the window it is shown through.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ListNav {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl ListNav {
    /// A walk starting on `selected`, with the window placed around it.
    pub(crate) fn new(selected: usize, len: usize) -> Self {
        let mut nav = ListNav {
            selected,
            scroll_offset: 0,
        };
        nav.reveal(len);
        nav
    }

    /// Bring the selected item into the visible window, and never scroll past
    /// the end of a list that fits in it.
    pub(crate) fn reveal(&mut self, len: usize) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + MAX_VISIBLE {
            self.scroll_offset = self.selected + 1 - MAX_VISIBLE;
        }
        self.scroll_offset = self.scroll_offset.min(len.saturating_sub(MAX_VISIBLE));
    }

    /// Move the selection to `to`, clamped to the list, and reveal it.
    pub(crate) fn move_to(&mut self, to: usize, len: usize) {
        self.selected = to.min(len.saturating_sub(1));
        self.reveal(len);
    }

    /// The rows a popup should draw, as indices into a list of `len`.
    pub(crate) fn visible(&self, len: usize) -> std::ops::Range<usize> {
        self.scroll_offset..len.min(self.scroll_offset + MAX_VISIBLE)
    }
}

/// What one key press asks of a list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ListMove {
    Prev,
    Next,
    /// One of the four jump keys, already resolved to an index.
    To(usize),
    /// Left or Right. The listing is one column wide, so a sideways step means
    /// nothing in it; it is reported so a popup can swallow it rather than let
    /// it move the caret out from under the popup.
    Sideways,
}

/// A bare key press: no modifier that would make it mean something else.
pub(crate) fn plain(i: &egui::InputState) -> bool {
    !i.modifiers.shift && !i.modifiers.command && !i.modifiers.alt
}

/// Reads this frame's input as a movement in a list of `len`, currently on
/// `selected`.
///
/// Only the keys every list popup agrees on. `Escape`, the accept keys and any
/// alias a popup adds are its own to read, before or after this.
pub(crate) fn read_move(i: &egui::InputState, selected: usize, len: usize) -> Option<ListMove> {
    if !plain(i) {
        return None;
    }
    let last = len.saturating_sub(1);
    if i.key_pressed(egui::Key::ArrowUp) {
        Some(ListMove::Prev)
    } else if i.key_pressed(egui::Key::ArrowDown) {
        Some(ListMove::Next)
    } else if i.key_pressed(egui::Key::Home) {
        Some(ListMove::To(0))
    } else if i.key_pressed(egui::Key::End) {
        Some(ListMove::To(last))
    } else if i.key_pressed(egui::Key::PageUp) {
        Some(ListMove::To(selected.saturating_sub(MAX_VISIBLE)))
    } else if i.key_pressed(egui::Key::PageDown) {
        Some(ListMove::To((selected + MAX_VISIBLE).min(last)))
    } else if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowRight) {
        Some(ListMove::Sideways)
    } else {
        None
    }
}

/// A bare Ctrl chord on a letter key. `ctrl` and not `command`: off the Mac
/// `command` mirrors `ctrl`, so testing `command` would reject every Ctrl
/// chord, and `mac_cmd` is what rules the Cmd variant out on the Mac.
pub(crate) fn ctrl_letter(i: &egui::InputState, key: egui::Key) -> bool {
    i.modifiers.ctrl
        && !i.modifiers.mac_cmd
        && !i.modifiers.alt
        && !i.modifiers.shift
        && i.key_pressed(key)
}

/// What one frame's keys ask of a list that is narrowed by typing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TypedListKey {
    /// Escape: give up on the listing.
    Dismiss,
    /// Enter or Tab: take the selected item.
    Accept,
    /// A step through the list, already resolved against its length. `Prev` at
    /// the first item and `Next` at the last stay where they are — a list that
    /// is being narrowed has no wrap-around to discover.
    Move(ListMove),
}

/// Reads this frame's input for a popup narrowed by typing: Escape, the walk of
/// [`read_move`], Ctrl+J and Ctrl+K as Down and Up, and Enter or Tab to accept.
///
/// Escape first, then the walk, then accepting, which is the order a frame that
/// somehow carries two of them resolves in.
pub(crate) fn read_typed_list_key(
    i: &egui::InputState,
    selected: usize,
    len: usize,
) -> Option<TypedListKey> {
    if i.key_pressed(egui::Key::Escape) {
        return Some(TypedListKey::Dismiss);
    }
    if let Some(step) = read_move(i, selected, len) {
        return Some(TypedListKey::Move(step));
    }
    let arrow = |key| i.key_pressed(key) && !i.modifiers.shift && !i.modifiers.command;
    if arrow(egui::Key::ArrowUp) || ctrl_letter(i, egui::Key::K) {
        return Some(TypedListKey::Move(ListMove::Prev));
    }
    if arrow(egui::Key::ArrowDown) || ctrl_letter(i, egui::Key::J) {
        return Some(TypedListKey::Move(ListMove::Next));
    }
    if i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Tab) {
        return Some(TypedListKey::Accept);
    }
    None
}

impl ListNav {
    /// Applies a step read off the keys. `Sideways` moves nothing; whether it is
    /// swallowed is the popup's call.
    pub(crate) fn step(&mut self, step: ListMove, len: usize) {
        match step {
            ListMove::Prev => self.move_to(self.selected.saturating_sub(1), len),
            ListMove::Next => self.move_to(self.selected + 1, len),
            ListMove::To(to) => self.move_to(to, len),
            ListMove::Sideways => {}
        }
    }
}

/// Draws the window `nav` shows onto a list of `len` rows, one `row` call per
/// visible index (`row(ui, index, selected)`), and the `n/m` counter beneath a
/// list longer than the window. Returns the row that was clicked, if any.
///
/// The row's own look is the popup's: completion prefixes a kind letter, the
/// goto choice lines its locations up, the palette right-aligns a detail.
pub(crate) fn show_window(
    ui: &mut egui::Ui,
    nav: &ListNav,
    len: usize,
    mut row: impl FnMut(&mut egui::Ui, usize, bool) -> egui::Response,
) -> Option<usize> {
    let mut clicked = None;
    for i in nav.visible(len) {
        if row(ui, i, i == nav.selected).clicked() {
            clicked = Some(i);
        }
    }
    if len > MAX_VISIBLE {
        ui.label(format!("{}/{len}", nav.selected + 1));
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_follows_the_selection_and_stops_at_the_end() {
        let len = 25;
        let mut nav = ListNav::new(0, len);
        assert_eq!(nav.visible(len), 0..MAX_VISIBLE);
        nav.move_to(MAX_VISIBLE, len);
        assert_eq!(nav.scroll_offset, 1);
        nav.move_to(len - 1, len);
        assert_eq!(nav.scroll_offset, len - MAX_VISIBLE);
        // Clamped to the list rather than scrolling past its end.
        nav.move_to(len + 5, len);
        assert_eq!(nav.selected, len - 1);
        assert_eq!(nav.scroll_offset, len - MAX_VISIBLE);
        nav.move_to(0, len);
        assert_eq!(nav.scroll_offset, 0);
    }

    /// A list shorter than the window never scrolls at all.
    #[test]
    fn a_short_list_keeps_its_window_at_the_top() {
        let len = 3;
        let mut nav = ListNav::new(2, len);
        assert_eq!(nav.scroll_offset, 0);
        assert_eq!(nav.visible(len), 0..3);
        nav.move_to(0, len);
        assert_eq!(nav.scroll_offset, 0);
    }
}
