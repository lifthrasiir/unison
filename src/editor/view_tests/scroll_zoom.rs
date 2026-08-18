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
