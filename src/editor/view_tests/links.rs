//! Ctrl/Cmd+click over a name: what it resolves to, what it hands to the
//! host, and the tokens a soft wrap splits.

use super::*;

fn link_doc(body: &str) -> String {
    format!("glyph a 2 2\n@@..\n..@@\nglyph b\n{body}\n")
}

/// Doc-line index of the first text line starting with `prefix`.
#[track_caller]
fn text_line_at(h: &EditorHarness, prefix: &str) -> usize {
    h.lines
        .iter()
        .position(|l| matches!(l, DocLine::Text(s) if s.trim_start().starts_with(prefix)))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?}"))
}

/// Ctrl/Cmd+clicking a link reports the jump, and reports it as starting at
/// the *link* — not at the caret, which the click deliberately leaves where it
/// was. Go Back relies on that position to return to the reference.
#[test]
fn following_a_link_reports_the_link_position_not_the_caret() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let ref_line = text_line_at(&h, "ref a");
    let def_line = text_line_at(&h, "glyph a");

    // Park the caret somewhere unrelated, so a `from` taken from the caret
    // would be visibly wrong.
    h.click_text(text_line_at(&h, "glyph b"), 2);
    assert_eq!(h.state.cursor.line, text_line_at(&h, "glyph b"));

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, 4), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert_eq!(nav.from, Caret::new(ref_line, 4));
    match nav.target {
        NavTarget::Local { line } => assert_eq!(line, def_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`a` is defined in this document")
        }
    }
    // The editor carries the local jump out itself.
    assert_eq!(h.state.cursor.line, def_line);
}

/// A link whose target is in another file cannot be resolved by the editor, so
/// it is handed to the host — still carrying the link position to come back to.
#[test]
fn a_link_to_another_file_is_handed_to_the_host() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref elsewhere 0 0"));
    let ref_line = text_line_at(&h, "ref elsewhere");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, 4), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert_eq!(nav.from, Caret::new(ref_line, 4));
    match &nav.target {
        NavTarget::CrossFile(goto) => assert_eq!(goto.name, "elsewhere"),
        NavTarget::Local { .. } | NavTarget::Search(_) => {
            panic!("a reference is not a definition, and `elsewhere` is not in this document")
        }
    }
    // Nothing moved: only the host can follow it.
    assert_ne!(h.state.cursor.line, ref_line + 1);
}

/// A jump the *host* carries out — a cross-file link, a search hit, an issue
/// click — moves the caret while egui's focus still sits wherever the gesture
/// started. The caret only paints while the editor has focus, so a jump that
/// left focus behind moved an invisible caret.
#[test]
fn a_host_jump_takes_focus_so_the_caret_shows() {
    let mut h = EditorHarness::new(&tall_doc());
    h.blur();
    assert!(!h.editor_has_focus(), "precondition: focus is elsewhere");

    h.state.goto_line(100);
    h.frame();

    assert!(
        h.editor_has_focus(),
        "a host-driven jump must take focus back, or its caret is invisible"
    );
}

/// Ctrl/Cmd+clicking the *definition* of a name asks the host to list its
/// appearances. Navigating would land on the line the click was already on, so
/// the gesture means "find references" here rather than "go to definition" —
/// and the editor must not move the caret itself.
#[test]
fn clicking_a_definition_asks_for_a_search() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let def_line = text_line_at(&h, "glyph a");

    h.click_text(text_line_at(&h, "glyph b"), 2);
    let parked = h.state.cursor.line;

    h.last_nav = None;
    h.click_at_mod(h.text_pos(def_line, 6), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    match &nav.target {
        NavTarget::Search(goto) => assert_eq!(goto.name, "a"),
        NavTarget::Local { .. } | NavTarget::CrossFile(_) => {
            panic!("a definition has nowhere to go")
        }
    }
    assert_eq!(h.state.cursor.line, parked, "the caret must not move");
}

/// An anchor is matched by name across glyphs and declared nowhere in
/// particular, so a click on one can only ever search — and searches for the
/// bare name, since `+above` and `-above` are two sides of one anchor.
#[test]
fn clicking_an_anchor_searches_for_it_without_its_sign() {
    use crate::editor::doc_links::LinkTargetKind;
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("anchor +above 1 0"));
    let anchor_line = text_line_at(&h, "anchor");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(anchor_line, 9), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    match &nav.target {
        NavTarget::Search(goto) => {
            assert_eq!(goto.name, "above");
            assert_eq!(goto.kind, LinkTargetKind::Anchor);
        }
        NavTarget::Local { .. } | NavTarget::CrossFile(_) => {
            panic!("an anchor has no definition to go to")
        }
    }
}

/// A `SLICE :` qualifier is a link of its own, and it does not swallow the
/// links that follow it: the same line still names a glyph two tokens later.
#[test]
fn clicking_a_slice_qualifier_goes_to_the_slice() {
    use crate::editor::document_view::NavTarget;

    let doc = "slice narrow\nglyph a 2 2\n@@..\n..@@\nglyph b\nmap narrow : A = a\n";
    let mut h = EditorHarness::new(doc);
    let slice_line = text_line_at(&h, "slice narrow");
    let glyph_line = text_line_at(&h, "glyph a");
    let map_line = text_line_at(&h, "map narrow");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(map_line, 6), Modifiers::COMMAND);
    match &h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line } => assert_eq!(*line, slice_line),
        _ => panic!("expected the slice declaration"),
    }

    h.last_nav = None;
    h.click_at_mod(h.text_pos(map_line, 17), Modifiers::COMMAND);
    match &h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line } => assert_eq!(*line, glyph_line),
        _ => panic!("expected the glyph"),
    }
}

/// An ordinary click on a link is just a click — no jump, and nothing recorded.
#[test]
fn clicking_a_link_without_the_modifier_reports_nothing() {
    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let ref_line = text_line_at(&h, "ref a");

    h.last_nav = None;
    h.click_text(ref_line, 4);

    assert!(h.last_nav.is_none());
    assert_eq!(h.state.cursor, Caret::new(ref_line, 4));
}

/// A soft wrap is a drawing decision, not a change to the line: a name split
/// across one still links to the whole name, from either half. Reading the
/// links off the wrapped *segment* used to hand the host whatever half was
/// clicked — and the half that no longer started with `ref` linked nothing.
#[test]
fn a_link_split_by_a_soft_wrap_still_names_the_whole_symbol() {
    use crate::editor::document_view::NavTarget;

    // Long enough to wrap at any plausible editor width.
    let long: String = std::iter::repeat_n("very-long-glyph-name", 12)
        .collect::<Vec<_>>()
        .join("-");
    let mut h = EditorHarness::new(&link_doc(&format!("ref {long} 0 0")));
    let ref_line = text_line_at(&h, "ref ");

    let wrap_col = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == ref_line)
        .filter_map(|vl| match &vl.kind {
            SnapKind::Text { col_offset, .. } => Some(*col_offset),
            SnapKind::GridRow { .. } => None,
        })
        .find(|c| *c > 0)
        .expect("the line must wrap for this test to mean anything");
    let name_start = 4;
    let name_end = name_start + long.chars().count();
    assert!(
        name_start < wrap_col && wrap_col < name_end,
        "the wrap must fall inside the name, not between tokens",
    );

    // Both halves of the name are the same link, and both name it in full.
    for col in [wrap_col - 2, wrap_col + 2] {
        h.last_nav = None;
        h.click_at_mod(h.text_pos(ref_line, col), Modifiers::COMMAND);
        let nav = h
            .last_nav
            .as_ref()
            .unwrap_or_else(|| panic!("no navigation reported for a click at col {col}"));
        match &nav.target {
            NavTarget::CrossFile(goto) => assert_eq!(goto.name, long, "at col {col}"),
            NavTarget::Local { .. } | NavTarget::Search(_) => {
                panic!("`{long}` is not in this document")
            }
        }
    }
}

/// The same goes for a color swatch: a `fill` pushed onto a later segment by a
/// soft wrap is still a `ref` line's fill, and still paints its swatch. Read
/// off the segment alone, the tail no longer starts with `ref` and the token
/// simply vanished.
#[test]
fn a_color_token_pushed_past_a_soft_wrap_still_paints_its_swatch() {
    const FILL: &str = "#00ff00";
    let long: String = std::iter::repeat_n("very-long-glyph-name", 12)
        .collect::<Vec<_>>()
        .join("-");
    let line = format!("ref {long} 0 0 fill {FILL}");
    let mut h = EditorHarness::new(&link_doc(&line));
    let ref_line = text_line_at(&h, "ref ");
    h.frame();

    let wrapped = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == ref_line)
        .filter(|vl| matches!(vl.kind, SnapKind::Text { col_offset, .. } if col_offset > 0))
        .count();
    assert!(
        wrapped > 0,
        "the line must wrap for this test to mean anything"
    );

    let col_start = line.chars().count() - FILL.len();
    assert!(
        h.color_backgrounds()
            .contains(&(ref_line, col_start, col_start + FILL.len())),
        "no swatch for the fill at col {col_start}: {:?}",
        h.color_backgrounds(),
    );
}

// ---------------------------------------------------------------------------
// Glyph metrics overlay
// ---------------------------------------------------------------------------
