//! Scrolling and zoom, and the per-instance state two editors must not
//! share.

use super::*;

#[test]
fn scroll_position_survives_zoom_change_across_documents() {
    let mut h = EditorHarness::new(&tall_doc());

    // Scroll to a line well past the viewport.
    h.state.goto_line(100);
    h.frame();
    h.frame();

    let scroll_y_z1 = h.scroll_y();
    assert!(
        scroll_y_z1 > 100.0,
        "should have scrolled down; y = {scroll_y_z1}"
    );

    // --- simulate switching to another document ---
    let mut stashed = std::mem::replace(&mut h.state, crate::editor::EditorState::new());

    // The "other document" scrolls to the top and we change zoom.
    h.frame();
    h.frame();

    h.zoom = 2;
    h.state.notify_zoom_change(1);
    h.frame();
    h.frame();

    // --- switch back to the original document ---
    std::mem::swap(&mut h.state, &mut stashed);
    h.frame();
    h.frame();

    let scroll_y_z2 = h.scroll_y();

    // At zoom=2, grid rows are 2× taller so the same logical position
    // requires a substantially larger pixel offset.  A naïve raw-pixel
    // restore would keep scroll_y ≈ scroll_y_z1; the correct centre-
    // fraction restore scales it up.
    assert!(
        scroll_y_z2 > scroll_y_z1 * 1.3,
        "scroll was not scaled for the new zoom: z1={scroll_y_z1:.1}, z2={scroll_y_z2:.1}"
    );
}

// -- multiple editors in one context -----------------------------------------

/// Two editors alive in the same `egui::Context` and the same frame must keep
/// their view state to themselves. Everything an editor parks in `ctx.data()`
/// is keyed by its [`crate::editor::EditorId`]; when those keys were bare
/// strings instead, the two panes shared one scroll offset, one caret-anchored
/// popup position and one layout snapshot, so whichever painted last won.
#[test]
fn two_editors_do_not_share_view_state() {
    let mut h = EditorHarness::new(&tall_doc());
    h.split(&tall_doc());

    // Scroll only the first pane, well past its viewport.
    h.state.goto_line(100);
    h.frame();
    h.frame();

    let first_y = h.scroll_y();
    let second_state = &h.second.as_ref().unwrap().state;
    let second_y = h.scroll_y_of(second_state);
    assert!(
        first_y > 100.0,
        "first pane should have scrolled; y = {first_y}"
    );
    assert!(
        second_y < 1.0,
        "second pane must stay at the top; y = {second_y} (first = {first_y})"
    );

    // Each pane published its own layout. The scrolled pane's first line sits
    // far above the viewport; the unscrolled one's sits inside it.
    let first_y = h.snap().vlines.first().expect("first pane vlines").y;
    let second_y = h
        .second_snap()
        .vlines
        .first()
        .expect("second pane vlines")
        .y;
    assert!(
        second_y - first_y > 100.0,
        "panes share a layout snapshot: first line y = {first_y} vs {second_y}"
    );

    // Carets move independently: clicking into the second pane leaves the
    // first pane's caret alone.
    let first_cursor = h.state.cursor;
    let second_pos = {
        let snap = h.second_snap();
        let vl = snap
            .vlines
            .iter()
            .find(|vl| matches!(vl.kind, SnapKind::Text { .. }))
            .expect("second pane text line");
        egui::pos2(snap.origin_x + 2.0, vl.y + vl.height * 0.5)
    };
    h.click_at(second_pos);
    assert_eq!(
        h.state.cursor, first_cursor,
        "click leaked into the first pane"
    );
}

// -- subglyph layer interactions ---------------------------------------------

// -- where a jump leaves the target ------------------------------------------

/// A "go to symbol" jump centres its target: the line it lands on is what the
/// user asked to see, and the context above it is as interesting as the context
/// below. It used to be parked a third of a viewport from the top, which buries
/// what precedes a definition under the header of the pane.
#[test]
fn a_goto_centres_its_target_line() {
    let mut h = EditorHarness::new(&tall_doc());
    h.viewport_height = Some(600.0);
    h.frame();

    // A header well past the first screenful, so the view has to move.
    let target = text_line_at(&h, "glyph tall12");
    h.state.goto_line(target);
    h.frame();
    h.frame();

    let vl = h
        .snap()
        .vlines
        .iter()
        .find(|vl| vl.doc_line == target)
        .cloned()
        .expect("the target line is on screen");
    // The first visual line sits at the top of the *content*, which is the
    // viewport's top less however far the view has scrolled.
    let viewport_top = h.snap().vlines[0].y + h.scroll_y();
    let middle = viewport_top + 300.0;
    assert!(
        (vl.y + vl.height * 0.5 - middle).abs() < vl.height,
        "target sits at y = {} (height {}), viewport middle is {middle}",
        vl.y,
        vl.height
    );
}

/// Going back is not a jump: the line returned to has to come back to the place
/// on the page it was left at, because that page — not the line alone — is what
/// the reader is asking for. Centring it instead was the bug.
#[test]
fn a_remembered_offset_puts_the_line_back_where_it_was_seen() {
    let mut h = EditorHarness::new(&tall_doc());
    h.viewport_height = Some(600.0);
    h.frame();

    // Near the bottom of the page, which no centring could produce.
    let line = text_line_at(&h, "glyph tall5");
    h.state
        .goto_caret_with(None, line, 0, ScrollIntent::Offset(520.0));
    h.frame();
    h.frame();
    assert!(
        (view_offset_of(&h, line) - 520.0).abs() < 4.0,
        "asked for 520, got {}",
        view_offset_of(&h, line)
    );
    // And that is the offset the host reads back when the user leaves from
    // here, so the round trip is closed.
    assert!(
        (h.state.caret_view_offset - 520.0).abs() < 4.0,
        "the caret's offset was published as {}",
        h.state.caret_view_offset
    );

    // Wander off, then return with the offset that was recorded.
    let elsewhere = text_line_at(&h, "glyph tall15");
    h.state.goto_line(elsewhere);
    h.frame();
    h.frame();
    assert!(
        view_offset_of(&h, line) < 0.0,
        "the line is still on screen"
    );

    h.state
        .goto_caret_with(None, line, 0, ScrollIntent::Offset(520.0));
    h.frame();
    h.frame();
    assert!(
        (view_offset_of(&h, line) - 520.0).abs() < 4.0,
        "the page was not restored: offset {}",
        view_offset_of(&h, line)
    );
}

/// Typing on the document's *last* line, with the view already scrolled to the
/// bottom, must not move the page. `scroll_cursor_into_view` asks for a
/// half-row margin below the caret, and below the last line there is none — so
/// the target it queues sits past the end of the scroll range. `egui` clamps
/// such an offset, but only after laying the frame out, so an unclamped target
/// painted one frame a half row too high and snapped back on the next.
#[test]
fn typing_on_the_last_line_does_not_jog_the_page() {
    let mut src = String::new();
    for i in 0..80 {
        use std::fmt::Write;
        writeln!(src, "// line {i}").unwrap();
    }
    let mut h = EditorHarness::new(&src);
    h.viewport_height = Some(300.0);
    h.frame();
    h.focus();

    let last = h.lines.len() - 1;
    h.state.goto_line(last);
    h.frame();
    h.frame();
    h.state.cursor = Caret::new(last, h.text(last).chars().count());
    h.frame();
    h.frame();

    let last_line_y = |h: &EditorHarness| h.snap().vlines.last().unwrap().y;
    let settled = (h.scroll_y(), last_line_y(&h));

    for _ in 0..3 {
        h.type_text("x");
        assert_eq!(
            (h.scroll_y(), last_line_y(&h)),
            settled,
            "typing on the last line moved the page"
        );
        h.frame();
        assert_eq!(
            (h.scroll_y(), last_line_y(&h)),
            settled,
            "the page did not come back the frame after"
        );
    }
}
