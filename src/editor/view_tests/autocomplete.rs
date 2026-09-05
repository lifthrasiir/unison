//! The completion popup: what it offers, how it filters, and the keys that
//! drive it.

use super::*;

/// DocLines: 0=glyph alpha header, 1=Grid(2x2), 2=glyph beta header,
/// 3=Grid(2x2), 4=blank, 5="ref "
fn ac_doc() -> String {
    "glyph alpha 2 2\n@@@@\n@@..\n\
     glyph beta 2 2\n..@@\n@@@@\n\
     \n\
     ref "
        .to_string()
}

fn ctrl_j(h: &mut EditorHarness) {
    h.key_mod(Key::J, Modifiers::CTRL);
}

fn ctrl_k(h: &mut EditorHarness) {
    h.key_mod(Key::K, Modifiers::CTRL);
}

/// Three variants of one part plus the glyph they compose; `last` is doc line
/// 8, since every declared header carries an empty grid line of its own.
fn family_doc(last: &str) -> String {
    format!(
        "glyph part:4x16 4 16\n\
         glyph part:5x16-l 5 16\n\
         glyph part:5x16-r 5 16\n\
         glyph whole 15 16\n\
         {last}"
    )
}

#[test]
fn autocomplete_trigger_and_dismiss() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    assert!(h.state.autocomplete.is_none());

    ctrl_j(&mut h);
    assert!(h.state.autocomplete.is_some());
    let ac = h.state.autocomplete.as_ref().unwrap();
    assert!(ac.candidates.len() >= 2);

    h.key(Key::Escape);
    assert!(h.state.autocomplete.is_none());
}

#[test]
fn autocomplete_accept_inserts_text() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);

    let ac = h.state.autocomplete.as_ref().unwrap();
    let first_label = ac.candidates[0].label.clone();

    h.key(Key::Enter);
    assert!(h.state.autocomplete.is_none());
    assert_eq!(h.text(5), format!("ref {}", first_label));
}

#[test]
fn autocomplete_filters_as_you_type() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);
    let initial_count = h.state.autocomplete.as_ref().unwrap().candidates.len();

    h.type_text("al");
    if let Some(ac) = &h.state.autocomplete {
        assert!(ac.candidates.len() <= initial_count);
        assert!(ac.candidates.iter().all(|c| c.label.starts_with("al")));
    }
}

#[test]
fn autocomplete_keyword_on_empty_line() {
    // DocLines: 0=header, 1=grid, 2=blank
    let mut h = EditorHarness::new("glyph alpha 2 2\n@@@@\n@@..\n\n");
    h.click_text(2, 0);
    ctrl_j(&mut h);
    if let Some(ac) = &h.state.autocomplete {
        assert!(ac.candidates.iter().any(|c| c.label == "glyph"));
        assert!(ac.candidates.iter().any(|c| c.label == "ref"));
    } else {
        panic!("autocomplete should be active on empty line");
    }
}

#[test]
fn autocomplete_undo_after_accept() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    let original_text = h.text(5).to_string();
    ctrl_j(&mut h);
    h.key(Key::Enter);
    assert_ne!(h.text(5), original_text);

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(5), original_text);
}

/// Triggering inside a name completes the *whole* name: the caret first moves
/// to the name's end, so accepting cannot leave the tail of the old spelling
/// behind.
#[test]
fn autocomplete_trigger_mid_name_completes_whole_name() {
    let mut h = EditorHarness::new(
        "glyph alpha 2 2\n@@@@\n@@..\n\
         glyph beta 2 2\n..@@\n@@@@\n\
         \n\
         ref alp",
    );
    // Caret between `al` and `p`.
    h.click_text(5, 6);
    ctrl_j(&mut h);

    assert_eq!(h.state.cursor.col, 7);
    let ac = h.state.autocomplete.as_ref().unwrap();
    assert_eq!(ac.replace_start, 4);
    assert!(ac.candidates.iter().all(|c| c.label.starts_with("alp")));

    h.key(Key::Enter);
    assert_eq!(h.text(5), "ref alpha");
}

/// Triggering on a name that already carries a variant suffix lists the whole
/// family — that is the choice being made — instead of only what the suffix
/// written so far still matches. On an IDC line the slot the caret is filling
/// orders it: the variants marked for that slot's own direction first
/// (`compose::direction_rank`, D1), the unmarked ones next.
#[test]
fn autocomplete_lists_and_orders_a_variant_family() {
    let mut h = EditorHarness::new(
        "glyph part:4x16 4 16\n\
         glyph part:5x16-l 5 16\n\
         glyph part:5x16-r 5 16\n\
         glyph whole 15 16\n\
         ⿰ part:4x",
    );
    // Every declared header carries an empty grid line, so the IDC line is doc
    // line 8. Caret at the end of `part:4x`, which matches only the 4x16 one.
    h.click_text(8, 9);
    ctrl_j(&mut h);

    let labels: Vec<String> = h
        .state
        .autocomplete
        .as_ref()
        .unwrap()
        .candidates
        .iter()
        .map(|c| c.label.clone())
        .collect();
    assert_eq!(labels, vec!["part:5x16-l", "part:4x16", "part:5x16-r"]);

    // The order is the slot's, but the selection is what is *written*: `part:4x`
    // starts only the 4x16 one, so that is what accepting takes.
    h.key(Key::Enter);
    assert_eq!(h.text(8), "⿰ part:4x16");
}

/// A component fills its slot across the whole of the parent, so a variant of
/// the wrong size *across* the split axis is not a choice at all — `compose`
/// calls it an error — and the listing leaves it out entirely, rather than
/// offering it last the way a variant drawn for the other side is offered.
#[test]
fn autocomplete_drops_variants_that_do_not_fit_the_slot() {
    let labels = |h: &EditorHarness| -> Vec<String> {
        h.state
            .autocomplete
            .as_ref()
            .unwrap()
            .candidates
            .iter()
            .map(|c| c.label.clone())
            .collect()
    };

    // ⿰ splits the width, so every part is the parent's full 16 tall.
    let mut h = EditorHarness::new(
        "glyph part:4x16 4 16\n\
         glyph part:4x10 4 10\n\
         glyph whole 15 16\n\
         ⿰ part:4x",
    );
    h.click_text(6, 9);
    ctrl_j(&mut h);
    assert_eq!(labels(&h), vec!["part:4x16"]);

    // ⿱ splits the height, so the other side of the box is the one that has
    // to match.
    let mut h = EditorHarness::new(
        "glyph part:15x4 15 4\n\
         glyph part:10x4 10 4\n\
         glyph whole 15 16\n\
         ⿱ part:1",
    );
    h.click_text(6, 8);
    ctrl_j(&mut h);
    assert_eq!(labels(&h), vec!["part:15x4"]);
}

/// Ctrl+J/Ctrl+K walk the open popup like Down/Up. The trigger itself is the
/// first step down from a virtual item before the list, so the popup opens on
/// item 0 and Ctrl+K there stays put rather than closing it — there is nothing
/// above to step back to. Ctrl+K must not reach the code-point popup either.
#[test]
fn autocomplete_ctrl_j_k_navigate() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);

    ctrl_j(&mut h);
    assert!(h.state.autocomplete.as_ref().unwrap().candidates.len() >= 2);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);

    // Nothing above item 0, and the popup survives.
    ctrl_k(&mut h);
    assert!(h.state.autocomplete.is_some());
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);
    assert!(matches!(h.state.popup, crate::editor::PopupState::None));

    ctrl_j(&mut h);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 1);
    ctrl_k(&mut h);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);

    // Arrow keys keep working alongside them.
    h.key(Key::ArrowDown);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 1);
    h.key(Key::ArrowUp);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);
}

/// A family listing opens on the variant that is *already written*: the
/// selection is placed by where the text sorts (`a <= text < b` picks `a`), and
/// an exactly written name is that `a`.
#[test]
fn autocomplete_opens_on_the_variant_already_written() {
    let mut h = EditorHarness::new(&family_doc("⿰ part:5x16-r"));
    h.click_text(8, 13);
    ctrl_j(&mut h);

    let ac = h.state.autocomplete.as_ref().unwrap();
    assert_eq!(ac.candidates.len(), 3);
    assert_eq!(ac.candidates[ac.selected].label, "part:5x16-r");
}

/// Typing on with no key having walked the list keeps re-placing the selection
/// by what is written, however the listing itself is ordered.
#[test]
fn autocomplete_selection_follows_what_is_typed() {
    let mut h = EditorHarness::new(&family_doc("ref part:"));
    h.click_text(8, 9);
    ctrl_j(&mut h);
    assert_eq!(
        h.state.autocomplete.as_ref().unwrap().candidates[0].label,
        "part:4x16"
    );

    h.type_text("5x16-r");
    let ac = h.state.autocomplete.as_ref().unwrap();
    // The whole family stays listed — that is the choice being made — and the
    // selection moved to what has been written.
    assert_eq!(ac.candidates.len(), 3);
    assert_eq!(ac.candidates[ac.selected].label, "part:5x16-r");
}

/// Walking the list is choosing a name, so the next character typed continues
/// the *selected* one rather than what the line still says.
#[test]
fn autocomplete_typing_after_walking_the_list_continues_the_selection() {
    let mut h = EditorHarness::new(
        "glyph glide 2 2\n\
         glyph graph 2 2\n\
         glyph graphic 2 2\n\
         ref g",
    );
    h.click_text(6, 5);
    ctrl_j(&mut h);
    assert_eq!(
        h.state.autocomplete.as_ref().unwrap().candidates[0].label,
        "glide"
    );

    h.key(Key::ArrowDown);
    assert_eq!(
        h.state.autocomplete.as_ref().unwrap().candidates
            [h.state.autocomplete.as_ref().unwrap().selected]
            .label,
        "graph"
    );

    h.type_text("ic");
    assert_eq!(h.text(6), "ref graphic");
    let ac = h.state.autocomplete.as_ref().unwrap();
    assert_eq!(ac.candidates[ac.selected].label, "graphic");

    // And the selection follows the text again from there.
    h.key(Key::Backspace);
    assert_eq!(h.text(6), "ref graphi");
}

/// Escape leaves whatever has been typed on the line, including the name a
/// walk of the list rewrote it to.
#[test]
fn autocomplete_escape_keeps_what_was_typed() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);
    h.type_text("al");
    h.key(Key::Escape);
    assert!(h.state.autocomplete.is_none());
    assert_eq!(h.text(5), "ref al");

    // A walk that nothing was typed after leaves the line as it stood.
    ctrl_j(&mut h);
    h.key(Key::ArrowDown);
    h.key(Key::Escape);
    assert_eq!(h.text(5), "ref al");
}

/// Home/End/PageUp/PageDown walk the *listing* while it is open: nothing else
/// reaches an item a long list keeps off-screen, and moving the caret instead
/// would only dismiss the popup.
#[test]
fn autocomplete_page_home_and_end_walk_the_listing() {
    use crate::editor::autocomplete::MAX_VISIBLE;

    let mut doc = String::new();
    for i in 0..12 {
        doc.push_str(&format!("glyph a{i:02} 2 2\n"));
    }
    doc.push_str("ref a");
    let mut h = EditorHarness::new(&doc);
    h.click_text(24, 5);
    ctrl_j(&mut h);

    let selected = |h: &EditorHarness| h.state.autocomplete.as_ref().unwrap().selected;
    assert_eq!(h.state.autocomplete.as_ref().unwrap().candidates.len(), 12);
    assert_eq!(selected(&h), 0);

    h.key(Key::PageDown);
    assert_eq!(selected(&h), MAX_VISIBLE);
    h.key(Key::End);
    assert_eq!(selected(&h), 11);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().scroll_offset, 2);
    h.key(Key::PageUp);
    assert_eq!(selected(&h), 11 - MAX_VISIBLE);
    h.key(Key::Home);
    assert_eq!(selected(&h), 0);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().scroll_offset, 0);
    // The caret never left the word being completed.
    assert_eq!(h.state.cursor.col, 5);
}

/// Left and Right are the popup's to swallow: the listing is narrowed by the
/// word the caret sits at the end of, and a step off it would only dismiss the
/// popup or re-filter against half a name.
#[test]
fn autocomplete_ignores_left_and_right() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);
    h.type_text("al");
    let col = h.state.cursor.col;

    h.key(Key::ArrowLeft);
    assert_eq!(h.state.cursor.col, col);
    assert!(h.state.autocomplete.is_some());
    h.key(Key::ArrowRight);
    assert_eq!(h.state.cursor.col, col);
    assert!(h.state.autocomplete.is_some());
}

/// A caret resting immediately *before* a name is writing that name: the whole
/// of it is what completes, rather than the popup falling back to the empty
/// prefix and offering everything.
#[test]
fn autocomplete_before_a_name_completes_that_name() {
    let mut h = EditorHarness::new(
        "glyph alpha 2 2\n@@@@\n@@..\n\
         glyph beta 2 2\n..@@\n@@@@\n\
         \n\
         ref alpha",
    );
    h.click_text(5, 4);
    ctrl_j(&mut h);
    assert_eq!(h.state.cursor.col, 9);
    let ac = h.state.autocomplete.as_ref().unwrap();
    assert_eq!(ac.replace_start, 4);
    let labels: Vec<&str> = ac.candidates.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, vec!["alpha"]);

    // The same at the head of the line, where the first word is the keyword.
    h.key(Key::Escape);
    h.click_text(5, 0);
    ctrl_j(&mut h);
    assert_eq!(h.state.cursor.col, 3);
    let ac = h.state.autocomplete.as_ref().unwrap();
    let labels: Vec<&str> = ac.candidates.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, vec!["ref"]);
}

/// What is written wins over where it sorts: a candidate the text is a prefix
/// of is the one meant, and only when there is none does the selection fall
/// back to where the text sorts.
#[test]
fn autocomplete_prefers_a_candidate_the_text_starts() {
    let mut h = EditorHarness::new(&family_doc("ref part:5x"));
    h.click_text(8, 11);
    ctrl_j(&mut h);

    let ac = h.state.autocomplete.as_ref().unwrap();
    assert_eq!(ac.candidates.len(), 3);
    assert_eq!(ac.candidates[ac.selected].label, "part:5x16-l");
}

/// Walking the list and accepting writes one undo entry, not one per step: the
/// line is only rewritten when a *character* is typed on from the selection.
#[test]
fn autocomplete_walking_the_list_costs_one_undo_entry() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    assert_eq!(h.text(5), "ref beta");

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(5), "ref ");
}

/// Ctrl+K with no popup open still starts code-point entry.
#[test]
fn ctrl_k_without_autocomplete_opens_codepoint_entry() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_k(&mut h);
    assert!(h.state.autocomplete.is_none());
    assert!(!matches!(h.state.popup, crate::editor::PopupState::None));
}

// -- visual line <-> logical line reconciliation --------------------------
