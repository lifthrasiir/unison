//! The scrolling-list half of a caret-anchored popup, shared by the two popups
//! that offer a list to walk: autocompletion ([`super::autocomplete`]) and the
//! goto choice ([`super::goto_popup`]).
//!
//! What is shared is the *walk*, not the list: a selection index, the window it
//! is kept visible in, and the keys that move it. Both popups are read the same
//! way — a short window onto a possibly long list, walked with the arrows and
//! the four jump keys — and the two had no business spelling that out twice.
//!
//! What is deliberately **not** here is what a popup does with the choice, what
//! else its keys mean (autocompletion aliases Ctrl+J/K onto the arrows and
//! rewrites the line as it walks) and when it closes. Those differ, and each
//! popup keeps them.

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
