//! Ctrl/Cmd+click over a name: what it resolves to, what it hands to the
//! host, and the tokens a soft wrap splits.

use super::*;

fn link_doc(body: &str) -> String {
    format!("glyph a 2 2\n@@..\n..@@\nglyph b\n{body}\n")
}

/// The jump also records *where on the page* the link was, which is what lets
/// Go Back restore the view rather than merely the line. Reported by the editor
/// because only it knows the layout — and reported for the link, not the caret,
/// for the same reason `from` is.
#[test]
fn following_a_link_reports_the_page_the_link_was_seen_on() {
    let mut src = String::from("glyph a 2 2\n@@..\n..@@\n");
    for i in 0..30 {
        src.push_str(&format!("glyph filler{i} 2 2\n....\n....\n"));
    }
    src.push_str("glyph b\nref a 0 0\n");

    let mut h = EditorHarness::new(&src);
    h.viewport_height = Some(300.0);
    h.frame();
    let ref_line = text_line_at(&h, "ref a");

    // Bring the link on screen somewhere other than the top, so an offset
    // taken from anywhere but the link itself would read differently.
    h.state.goto_line(ref_line);
    h.frame();
    h.frame();
    let seen_at = view_offset_of(&h, ref_line);
    assert!(
        seen_at > 1.0 && seen_at < 300.0,
        "the link should be on screen, at {seen_at}"
    );

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, 4), Modifiers::COMMAND);
    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert!(
        (nav.from_offset - seen_at).abs() < 4.0,
        "reported {} for a link drawn at {seen_at}",
        nav.from_offset
    );
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
        NavTarget::Local { line, .. } => assert_eq!(line, def_line),
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
        NavTarget::Local { line, .. } => assert_eq!(*line, slice_line),
        _ => panic!("expected the slice declaration"),
    }

    h.last_nav = None;
    h.click_at_mod(h.text_pos(map_line, 17), Modifiers::COMMAND);
    match &h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line, .. } => assert_eq!(*line, glyph_line),
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

/// A `$-N` on a `ref` line names a group of the *header* above it, and nothing
/// on its own line says which. Ctrl/Cmd+clicking it goes to that group — the
/// only place the name it stands for is written.
#[test]
fn clicking_a_back_reference_goes_to_the_group_it_names() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(
        "glyph part-a 2 2\n\
         @@..\n\
         ..@@\n\
         glyph whole-(a|b)\n\
         ref part-($-1) 0 0\n",
    );
    let ref_line = text_line_at(&h, "ref part-");
    let header_line = text_line_at(&h, "glyph whole-");

    h.last_nav = None;
    // Inside the `$-1`, which is its own link inside the ref name.
    h.click_at_mod(h.text_pos(ref_line, 11), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    match nav.target {
        NavTarget::Local { line, .. } => assert_eq!(line, header_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("the group `$-1` names is on the header above it")
        }
    }
    // On the group itself, not merely on its line: `whole-(a|b)` starts at
    // column 6, so its one group opens at column 12.
    assert_eq!(h.state.cursor, Caret::new(header_line, 12));
}

/// The same for a `($N)` under an `exists`: the group is on the search line,
/// and `$0` — the whole match — is the search itself.
#[test]
fn clicking_a_search_capture_goes_to_the_search() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(
        "glyph han-4e00:2x2 2 2\n\
         @@..\n\
         ..@@\n\
         exists han-([0-9a-f]{4}):2x2\n\
         glyph han-($1) 2 2\n\
         ref ($0) 0 0\n",
    );
    let exists_line = text_line_at(&h, "exists ");
    let header_line = text_line_at(&h, "glyph han-($1)");
    let ref_line = text_line_at(&h, "ref ($0)");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(header_line, 12), Modifiers::COMMAND);
    match h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line, .. } => assert_eq!(line, exists_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("the group `$1` names is on the `exists` line above")
        }
    }
    // `exists ` is seven columns, so the pattern's one group opens at 11.
    assert_eq!(h.state.cursor, Caret::new(exists_line, 11));

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, 6), Modifiers::COMMAND);
    match h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line, .. } => assert_eq!(line, exists_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`$0` is the whole search")
        }
    }
    // `$0` is the match itself, so it lands on the pattern rather than on a
    // group of it.
    assert_eq!(h.state.cursor, Caret::new(exists_line, 7));
}

/// A word inside a `// …` comment links when — and only when — it names a
/// glyph the font actually has. Prose says nothing about which of its words is
/// a name, so existence is the whole test, and a word that names nothing stays
/// plain text rather than becoming a link to a search.
#[test]
fn a_comment_word_that_names_a_glyph_is_a_link() {
    use crate::editor::document_view::NavTarget;

    let body = "ref a 0 0 // like a but not zzz";
    let mut h = EditorHarness::new(&link_doc(body));
    let ref_line = text_line_at(&h, "ref a");
    let def_line = text_line_at(&h, "glyph a");
    let a_col = body.rfind(" a ").unwrap() + 1;
    let zzz_col = body.find("zzz").unwrap();

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, a_col), Modifiers::COMMAND);
    match h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line, .. } => assert_eq!(line, def_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`a` is defined in this document")
        }
    }

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, zzz_col), Modifiers::COMMAND);
    assert!(
        h.last_nav.is_none(),
        "`zzz` names no glyph, so it is not a link"
    );
}

/// Ctrl/Cmd+`]` is the keyboard form of the same gesture: it follows whatever
/// link the caret is sitting on, on a directive or in a comment alike.
#[test]
fn the_goto_key_follows_the_link_under_the_caret() {
    use crate::editor::document_view::NavTarget;

    let body = "ref a 0 0 // see a";
    let mut h = EditorHarness::new(&link_doc(body));
    let ref_line = text_line_at(&h, "ref a");
    let def_line = text_line_at(&h, "glyph a");

    // On the `ref`'s own target.
    h.click_text(ref_line, 4);
    h.last_nav = None;
    h.key_mod(Key::CloseBracket, Modifiers::COMMAND);
    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert_eq!(nav.from, Caret::new(ref_line, 4));
    match nav.target {
        NavTarget::Local { line, .. } => assert_eq!(line, def_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`a` is defined in this document")
        }
    }

    // And on the same name written in the comment.
    h.click_text(ref_line, body.rfind('a').unwrap());
    h.last_nav = None;
    h.key_mod(Key::CloseBracket, Modifiers::COMMAND);
    match h
        .last_nav
        .as_ref()
        .expect("no navigation reported for the comment word")
        .target
    {
        NavTarget::Local { line, .. } => assert_eq!(line, def_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`a` is defined in this document")
        }
    }
}

/// The key reports nothing where the caret is on no link at all — a bare word
/// of prose included, so the gesture stays as quiet as the click is.
#[test]
fn the_goto_key_on_no_link_reports_nothing() {
    let body = "ref a 0 0 // nothing here";
    let mut h = EditorHarness::new(&link_doc(body));
    let ref_line = text_line_at(&h, "ref a");

    h.click_text(ref_line, body.find("nothing").unwrap() + 2);
    h.last_nav = None;
    h.key_mod(Key::CloseBracket, Modifiers::COMMAND);
    assert!(h.last_nav.is_none(), "no link sits under the caret");
}

/// Edit ▸ Go to symbol asks for the same jump from outside the frame that
/// carries it out, so the request has to survive to the next paint pass.
#[test]
fn the_menu_request_follows_the_link_under_the_caret() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let ref_line = text_line_at(&h, "ref a");
    let def_line = text_line_at(&h, "glyph a");

    h.click_text(ref_line, 4);
    h.last_nav = None;
    h.state.request_goto_symbol();
    h.frame();

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert_eq!(nav.from, Caret::new(ref_line, 4));
    match nav.target {
        NavTarget::Local { line, .. } => assert_eq!(line, def_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`a` is defined in this document")
        }
    }
}
