//! The choice a Ctrl/Cmd+click on a *pattern* reference sometimes has to offer.
//!
//! A reference written as a pattern names many glyphs, and they need not be
//! declared in one place: `ref han-xxxx-($-1):15x16` under a seven-region
//! header goes to one block if the font declares all seven together and to two
//! if it declares them `(g|h|t)` and `(j|k|p|v)`. The host resolves that — see
//! [`crate::app::goto_pattern`], which is also where the grouping and the
//! "there is only one place, just jump" case live — and hands what is left
//! here: one row per place, the name a jump to it is made with, and how many
//! further names of the pattern land there.
//!
//! The rows are a listing to be walked, so the walk is
//! [`crate::editor::list_popup`]'s, the same one autocompletion uses. What is
//! *not* shared is when it closes: this popup narrows nothing, so there is no
//! typing that could refine it and anything but its own keys dismisses it.

use crate::editor::caret::Caret;
use crate::editor::list_popup::{ListMove, ListNav, read_move};

/// One place a pattern's expansions lead.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GotoChoice {
    /// The name the jump is carried out with — the first expansion that landed
    /// here, which stands for the rest.
    pub name: String,
    /// How many *further* names land in the same place; `0` for a group of one.
    pub extra: usize,
    /// `file.unf:12`, the place itself, for the row to show.
    pub location: String,
}

/// The open popup.
///
/// `from` and `from_offset` are the click's, carried across the frame the popup
/// is up so that the jump it eventually makes is recorded from where the *link*
/// was — a Go Back has to return to the reference, not to the row that was
/// picked. They are the same two fields a
/// [`super::document_view::NavRequest`] carries, because the popup ends in one.
pub(crate) struct GotoChoicePopup {
    pub choices: Vec<GotoChoice>,
    pub nav: ListNav,
    /// Where on screen the followed link was drawn, so the popup opens under it
    /// rather than under a caret the click deliberately left alone.
    pub anchor: egui::Pos2,
    pub from: Caret,
    pub from_offset: f32,
}

impl GotoChoicePopup {
    pub(crate) fn new(
        choices: Vec<GotoChoice>,
        anchor: egui::Pos2,
        from: Caret,
        from_offset: f32,
    ) -> Self {
        let nav = ListNav::new(0, choices.len());
        GotoChoicePopup {
            choices,
            nav,
            anchor,
            from,
            from_offset,
        }
    }

    /// The rows as `(name and count, location)`, with the first column padded
    /// so the locations line up under each other.
    pub(crate) fn rows(&self) -> Vec<(String, String)> {
        let labels: Vec<String> = self
            .choices
            .iter()
            .map(|c| match c.extra {
                0 => c.name.clone(),
                n => format!("{}  [+{n}]", c.name),
            })
            .collect();
        let width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        labels
            .into_iter()
            .zip(&self.choices)
            .map(|(label, choice)| {
                let pad = width - label.chars().count();
                (
                    format!("{label}{:pad$}", "", pad = pad),
                    choice.location.clone(),
                )
            })
            .collect()
    }
}

/// What one frame's input did to the popup.
pub(crate) enum GotoKeys {
    /// Nothing of the popup's happened; it stays open and the key is the
    /// editor's.
    Idle,
    /// A key the popup owns. The caller must not go on handling it.
    Consumed,
    /// A row was accepted. The popup is closed and the jump is already pending.
    Chosen,
    /// Anything else at all: the popup is closed, and the key still belongs to
    /// the editor.
    Dismissed,
}

/// Reads this frame's keys for the popup.
///
/// Escape and the accept keys close it; the arrows and the four jump keys walk
/// it; Left and Right are swallowed, since one column has no sideways. Every
/// other key press dismisses it *without* consuming the key — the popup is an
/// offer, and typing on is a refusal of it, not a keystroke to be eaten.
pub(crate) fn handle_keys(ui: &egui::Ui, state: &mut super::EditorState) -> GotoKeys {
    let Some(popup) = &state.goto_choice else {
        return GotoKeys::Idle;
    };
    let (len, selected) = (popup.choices.len(), popup.nav.selected);
    let (escape, accept, step, any_key) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Tab),
            read_move(i, selected, len),
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { pressed: true, .. } | egui::Event::Text(_)
                )
            }),
        )
    });

    if escape {
        state.goto_choice = None;
        return GotoKeys::Consumed;
    }
    if let Some(step) = step {
        let popup = state.goto_choice.as_mut().expect("checked above");
        match step {
            ListMove::Prev => popup.nav.move_to(selected.saturating_sub(1), len),
            ListMove::Next => popup.nav.move_to(selected + 1, len),
            ListMove::To(to) => popup.nav.move_to(to, len),
            // One column wide: a sideways step means nothing, and letting it
            // through would only move the caret out from under the popup.
            ListMove::Sideways => {}
        }
        return GotoKeys::Consumed;
    }
    if accept {
        choose(state, selected);
        return GotoKeys::Chosen;
    }
    if any_key {
        state.goto_choice = None;
        return GotoKeys::Dismissed;
    }
    GotoKeys::Idle
}

/// Takes row `index` and turns it into the pending jump, closing the popup.
///
/// The jump goes out as a cross-file request even when the target is in this
/// same document: the host is what finds a declaration by name, and it is what
/// records the hop either way.
pub(crate) fn choose(state: &mut super::EditorState, index: usize) {
    let Some(popup) = state.goto_choice.take() else {
        return;
    };
    let Some(choice) = popup.choices.get(index) else {
        return;
    };
    state.pending_nav = Some(super::document_view::NavRequest {
        from: popup.from,
        from_offset: popup.from_offset,
        target: super::document_view::NavTarget::CrossFile(super::document_view::GotoGlyph {
            name: choice.name.clone(),
            kind: super::doc_links::LinkTargetKind::Glyph,
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popup(rows: &[(&str, usize)]) -> GotoChoicePopup {
        let choices = rows
            .iter()
            .map(|(name, extra)| GotoChoice {
                name: (*name).to_string(),
                extra: *extra,
                location: "han-0038.unf:1013".to_string(),
            })
            .collect();
        GotoChoicePopup::new(choices, egui::Pos2::ZERO, Caret::new(0, 0), 0.0)
    }

    /// What the reader compares between rows is where each one goes, so the
    /// locations line up under each other whatever the names are.
    #[test]
    fn the_rows_pad_the_names_into_a_column() {
        let popup = popup(&[("han-xxxx-g:15x16", 2), ("han-x:15x16", 3)]);
        let rows = popup.rows();
        assert_eq!(rows[0].0, "han-xxxx-g:15x16  [+2]");
        assert_eq!(rows[1].0, "han-x:15x16  [+3]     ");
        assert_eq!(rows[0].0.chars().count(), rows[1].0.chars().count());
        assert_eq!(rows[0].1, "han-0038.unf:1013");
    }

    /// A group of one says so by saying nothing: there is no `[+0]`.
    #[test]
    fn a_group_of_one_carries_no_count() {
        assert_eq!(popup(&[("foo", 0)]).rows()[0].0, "foo");
    }
}
