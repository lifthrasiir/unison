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

    h.key(Key::Enter);
    assert_eq!(h.text(8), "⿰ part:5x16-l");
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
