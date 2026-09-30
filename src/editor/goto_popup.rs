//! The choice a Ctrl/Cmd+click on a *pattern* reference sometimes has to offer.
//!
//! A reference written as a pattern names many glyphs, and they need not be
//! declared in one place: `ref han-xxxx-($-1):15x16` under a seven-region
//! header goes to one block if the font declares all seven together and to two
//! if it declares them `(g|h|t)` and `(j|k|p|v)`. The host resolves that — see
//! [`crate::app::goto_pattern`], which is also where the grouping and the
//! "there is only one place, just jump" case live — and hands what is left
//! here: one row per place and the names of the pattern that land there, the
//! first of which is the one a jump to it is made with.
//!
//! The rows are a listing to be walked, so the walk is
//! [`crate::editor::list_popup`]'s, the same one autocompletion uses. What is
//! *not* shared is when it closes: this popup narrows nothing, so there is no
//! typing that could refine it and anything but its own keys dismisses it.

use crate::editor::caret::Caret;
use crate::editor::list_popup::{ListNav, read_move};

/// One place a pattern's expansions lead.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GotoChoice {
    /// Every expansion that landed here, in the pattern's order. The first is
    /// the name the jump is carried out with, and stands for the rest.
    pub names: Vec<String>,
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
        let labels: Vec<String> = self.choices.iter().map(|c| group_label(&c.names)).collect();
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

/// A row's first column: the group written as the pattern it spells, with
/// its size, when that is short enough to read —
/// `han-xxxx-(j|k|p|v):15x16  [4]` — and otherwise the name the jump is made with and how many *more*
/// names land there, `han-xxxx-j:15x16  [+3]`. The two counts differ on
/// purpose: the pattern already shows every name, so its count is a total, and
/// the lone name shows one, so its count is the rest.
///
/// Short enough is no longer than twice the longest name. A group whose names
/// share little has a pattern about as long as all of them together, and a row
/// that long is a list, not a label.
fn group_label(names: &[String]) -> String {
    let [first, ..] = names else {
        return String::new();
    };
    if names.len() == 1 {
        return first.clone();
    }
    let longest = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    // A name the pattern grammar would read as syntax has no pattern to
    // stand for it.
    let spellable = names.iter().all(|n| !n.contains(['(', ')', '|', '*', '$']));
    if spellable {
        let pattern = common_pattern(names);
        if pattern.chars().count() <= 2 * longest {
            return format!("{pattern}  [{}]", names.len());
        }
    }
    format!("{first}  [+{}]", names.len() - 1)
}

/// `prefix(a|b|c)suffix` over the names' longest common prefix and suffix, the
/// suffix taken from what the prefix leaves of the shortest name so the two
/// never overlap. A name that is all prefix and suffix is an empty
/// alternative, which the grammar reads back as it is.
fn common_pattern(names: &[String]) -> String {
    let chars: Vec<Vec<char>> = names.iter().map(|n| n.chars().collect()).collect();
    let shortest = chars.iter().map(Vec::len).min().unwrap_or(0);
    let agree = |at: &dyn Fn(&[char]) -> char| chars.iter().all(|c| at(c) == at(&chars[0]));
    let prefix = (0..shortest).take_while(|&i| agree(&|c| c[i])).count();
    let suffix = (0..shortest - prefix)
        .take_while(|&i| agree(&|c| c[c.len() - 1 - i]))
        .count();
    let middles: Vec<String> = chars
        .iter()
        .map(|c| c[prefix..c.len() - suffix].iter().collect())
        .collect();
    format!(
        "{}({}){}",
        chars[0][..prefix].iter().collect::<String>(),
        middles.join("|"),
        chars[0][chars[0].len() - suffix..]
            .iter()
            .collect::<String>(),
    )
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
        // A sideways step moves nothing, and is consumed all the same: letting
        // it through would only move the caret out from under the popup.
        let popup = state.goto_choice.as_mut().expect("checked above");
        popup.nav.step(step, len);
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
            name: choice.names[0].clone(),
            kind: super::doc_links::LinkTargetKind::Glyph,
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popup(rows: &[&[&str]]) -> GotoChoicePopup {
        let choices = rows
            .iter()
            .map(|names| GotoChoice {
                names: names.iter().map(|n| n.to_string()).collect(),
                location: "han-0038.unf:1013".to_string(),
            })
            .collect();
        GotoChoicePopup::new(choices, egui::Pos2::ZERO, Caret::new(0, 0), 0.0)
    }

    /// What the reader compares between rows is where each one goes, so the
    /// locations line up under each other whatever the names are.
    #[test]
    fn the_rows_pad_the_names_into_a_column() {
        let popup = popup(&[
            &["han-xxxx-g:15x16", "han-xxxx-h:15x16", "han-xxxx-t:15x16"],
            &["foo-alpha", "bar-beta", "baz-gamma", "qux-delta"],
        ]);
        let rows = popup.rows();
        assert_eq!(rows[0].0, "han-xxxx-(g|h|t):15x16  [3]");
        assert_eq!(rows[1].0, "foo-alpha  [+3]            ");
        assert_eq!(rows[0].0.chars().count(), rows[1].0.chars().count());
        assert_eq!(rows[0].1, "han-0038.unf:1013");
    }

    /// A group of one says so by saying nothing: there is no `[+0]` or `[1]`.
    #[test]
    fn a_group_of_one_carries_no_count() {
        assert_eq!(popup(&[&["foo"]]).rows()[0].0, "foo");
    }

    /// A group is written as the pattern it spells when that stays short —
    /// no longer than twice its longest name — and the count is then the
    /// group's size, since every name is on the row.
    #[test]
    fn a_short_pattern_stands_for_the_group() {
        let names = [
            "han-xxxx-j:15x16",
            "han-xxxx-k:15x16",
            "han-xxxx-p:15x16",
            "han-xxxx-v:15x16",
        ];
        assert_eq!(
            popup(&[&names]).rows()[0].0,
            "han-xxxx-(j|k|p|v):15x16  [4]"
        );
    }

    /// The bound is inclusive: a pattern of exactly twice the longest name is
    /// still short enough, and one character more is not.
    #[test]
    fn the_pattern_may_be_up_to_twice_the_longest_name() {
        // `ab-(x|y)` is 8 characters against a longest name of 4.
        assert_eq!(group_label(&strings(&["ab-x", "ab-y"])), "ab-(x|y)  [2]");
        // `a-(x|y)` is 7 against 3.
        assert_eq!(group_label(&strings(&["a-x", "a-y"])), "a-x  [+1]");
    }

    /// The prefix and the suffix never overlap, so a name that is all prefix
    /// becomes an empty alternative — which the pattern grammar reads back.
    #[test]
    fn a_name_that_is_all_prefix_is_an_empty_alternative() {
        let names = strings(&["foo-bar", "foo-bar-2", "foo-bar-3"]);
        assert_eq!(common_pattern(&names), "foo-bar(|-2|-3)");
        let names = strings(&["aa", "aaa"]);
        assert_eq!(common_pattern(&names), "aa(|a)");
    }

    /// Whatever the pattern shown, it denotes exactly the group, in order.
    #[test]
    fn the_pattern_reads_back_as_the_names() {
        for names in [
            &["han-xxxx-g:15x16", "han-xxxx-h:15x16", "han-xxxx-t:15x16"][..],
            &["foo-bar", "foo-bar-2", "foo-bar-3"],
            &["aa", "aaa"],
            &["a-x", "b-x"],
            &["가-a", "가-b"],
        ] {
            let names = strings(names);
            let pattern = common_pattern(&names);
            let back = crate::pattern::NamePattern::parse_element(&pattern)
                .unwrap()
                .into_vec();
            assert_eq!(back, names, "{pattern}");
        }
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }
}
