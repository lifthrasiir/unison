//! Scrolling and zoom, and the per-instance state two editors must not
//! share.

use super::*;
use crate::editor::harness::SnapLine;

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
    h.state.notify_zoom_change();
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

// -- what a zoom keeps in place ----------------------------------------------

/// A page mixing every kind of visual line whose height does *not* follow the
/// zoom linearly: reference strips (fixed height), headings (stepped sizes),
/// comments long enough to wrap differently at each level, and grids.
fn mixed_zoom_doc() -> (String, Vec<u32>) {
    use std::fmt::Write;
    let mut s = String::new();
    let mut cps = Vec::new();
    for i in 0..30u32 {
        if i % 8 == 0 {
            writeln!(s, "## section {i}").unwrap();
        }
        writeln!(s, "// {}", "wrap me please ".repeat(12)).unwrap();
        let cp = 0x4e00 + i;
        cps.push(cp);
        writeln!(s, "glyph han-{cp:x} 16 16").unwrap();
        for _ in 0..16 {
            s.push_str("@@..............................\n");
        }
        s.push('\n');
    }
    (s, cps)
}

fn mixed_zoom_harness() -> EditorHarness {
    let (src, cps) = mixed_zoom_doc();
    let mut h = EditorHarness::new(&src);
    h.ref_images = Some(crate::editor::ref_images::RefImages::for_test(
        cps.into_iter().collect(),
    ));
    h.viewport_height = Some(600.0);
    h.frame();
    h
}

/// Changes the zoom the way the app does: the font follows the level, and the
/// editor is told which level it came from.
fn zoom_to(h: &mut EditorHarness, level: u32) {
    h.zoom = level;
    h.font_id = egui::FontId::monospace(16.0 * level as f32);
    h.state.notify_zoom_change();
    h.frame();
}

/// What a point on the page is *of*: a grid row, a strip row, or somewhere
/// down a (possibly wrapped) text line, as a fraction of that thing's height.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PageSpot {
    Grid(usize, i16, f32),
    Strip(usize, f32),
    Text(usize, f32),
}

fn spot_at(h: &EditorHarness, y: f32) -> PageSpot {
    let snap = h.snap();
    let vl = snap
        .vlines
        .iter()
        .find(|vl| vl.y <= y && y < vl.y + vl.height)
        .unwrap_or_else(|| panic!("nothing is laid out at y = {y}"));
    let frac = (y - vl.y) / vl.height;
    match &vl.kind {
        SnapKind::GridRow { row, .. } => PageSpot::Grid(vl.doc_line, *row, frac),
        SnapKind::RefImage { .. } => PageSpot::Strip(vl.doc_line, frac),
        SnapKind::Text { .. } => {
            let block: Vec<_> = snap
                .vlines
                .iter()
                .filter(|v| v.doc_line == vl.doc_line && matches!(v.kind, SnapKind::Text { .. }))
                .collect();
            let top = block[0].y;
            let h: f32 = block.iter().map(|v| v.height).sum();
            PageSpot::Text(vl.doc_line, (y - top) / h)
        }
    }
}

fn y_of_spot(h: &EditorHarness, spot: PageSpot) -> f32 {
    let snap = h.snap();
    let find = |pred: &dyn Fn(&SnapLine) -> bool| -> Vec<&SnapLine> {
        snap.vlines.iter().filter(|v| pred(v)).collect()
    };
    let (block, frac) = match spot {
        PageSpot::Grid(line, row, f) => (
            find(&|v| {
                v.doc_line == line && matches!(v.kind, SnapKind::GridRow { row: r, .. } if r == row)
            }),
            f,
        ),
        PageSpot::Strip(line, f) => (
            find(&|v| v.doc_line == line && matches!(v.kind, SnapKind::RefImage { .. })),
            f,
        ),
        PageSpot::Text(line, f) => (
            find(&|v| v.doc_line == line && matches!(v.kind, SnapKind::Text { .. })),
            f,
        ),
    };
    assert!(!block.is_empty(), "{spot:?} is not laid out");
    let h: f32 = block.iter().map(|v| v.height).sum();
    block[0].y + frac * h
}

/// Where the caret's own segment sits: the vertical middle of the visual line
/// it is drawn on.
fn caret_mid_y(h: &EditorHarness) -> f32 {
    let c = h.state.cursor;
    let vl = h
        .snap()
        .vlines
        .iter()
        .find(|vl| match &vl.kind {
            SnapKind::Text {
                text, col_offset, ..
            } => {
                vl.doc_line == c.line
                    && c.col >= *col_offset
                    && c.col <= col_offset + text.chars().count()
            }
            _ => false,
        })
        .expect("the caret's segment is laid out");
    vl.y + vl.height * 0.5
}

fn viewport_mid_y(h: &EditorHarness) -> f32 {
    // The content's top is the viewport's top less the scroll offset.
    h.snap().vlines[0].y + h.scroll_y() + 300.0
}

const ZOOM_WALK: [u32; 9] = [2, 3, 4, 3, 1, 5, 8, 2, 1];

/// With the pointer over the editor, what is under the pointer stays under it,
/// grids, strips, headings and rewrapped lines above it notwithstanding.
#[test]
fn a_zoom_keeps_what_is_under_the_pointer_under_it() {
    let mut h = mixed_zoom_harness();
    let target = text_line_at(&h, "glyph han-4e0f");
    h.state.goto_line(target);
    h.frame();
    h.frame();
    let pointer = egui::pos2(300.0, 180.0);
    h.move_pointer(pointer);
    for level in ZOOM_WALK {
        let spot = spot_at(&h, pointer.y);
        let from = h.zoom;
        zoom_to(&mut h, level);
        let drift = y_of_spot(&h, spot) - pointer.y;
        assert!(
            drift.abs() < 1.5,
            "{from}x -> {level}x moved {spot:?} by {drift:.1} from under the pointer"
        );
        h.frame();
        h.frame();
        let drift = y_of_spot(&h, spot) - pointer.y;
        assert!(
            drift.abs() < 1.5,
            "{from}x -> {level}x: {spot:?} drifted {drift:.1} on the frames after"
        );
    }
}

/// With the pointer outside the window and the caret on screen, the caret's
/// segment keeps its place on the screen.
#[test]
fn a_zoom_without_the_pointer_keeps_a_visible_caret_in_place() {
    let mut h = mixed_zoom_harness();
    // Well into a wrapped comment, so the segment it is on changes with zoom.
    let line = (0..h.lines.len())
        .filter(|&l| matches!(&h.lines[l], DocLine::Text(t) if t.starts_with("// wrap me")))
        .nth(12)
        .unwrap();
    h.state
        .goto_caret_with(None, line, 100, ScrollIntent::Offset(150.0));
    h.frame();
    h.frame();
    h.frame_with(vec![egui::Event::PointerGone], egui::Modifiers::NONE);
    for level in ZOOM_WALK {
        let before = caret_mid_y(&h);
        let from = h.zoom;
        zoom_to(&mut h, level);
        for settle in 0..3 {
            let drift = caret_mid_y(&h) - before;
            assert!(
                drift.abs() < 1.5,
                "{from}x -> {level}x moved the caret by {drift:.1} (frame {settle})"
            );
            h.frame();
        }
    }
}

/// With the pointer outside the editor and the caret off screen, the middle of
/// the viewport stays the middle.
#[test]
fn a_zoom_without_the_pointer_or_a_visible_caret_keeps_the_middle() {
    let mut h = mixed_zoom_harness();
    let target = text_line_at(&h, "glyph han-4e0f");
    h.state.goto_line(target);
    h.frame();
    h.frame();
    // The caret goes back to the top without the view following it.
    h.state.cursor = Caret::new(0, 0);
    h.frame();
    // Inside the window, below the editor's band: not over the editor.
    h.move_pointer(egui::pos2(300.0, 900.0));
    for level in ZOOM_WALK {
        let mid = viewport_mid_y(&h);
        let spot = spot_at(&h, mid);
        let from = h.zoom;
        zoom_to(&mut h, level);
        for settle in 0..3 {
            let drift = y_of_spot(&h, spot) - viewport_mid_y(&h);
            assert!(
                drift.abs() < 1.5,
                "{from}x -> {level}x moved {spot:?} off the middle by {drift:.1} (frame {settle})"
            );
            h.frame();
        }
    }
}

/// The minimap is part of the editor: a click on it scrolls the view and takes
/// the focus, so the keys go to the document just scrolled to.
#[test]
fn clicking_the_minimap_focuses_the_editor() {
    let src: String = (0..200).map(|i| format!("// line {i}\n")).collect();
    let mut h = EditorHarness::new(&src);
    h.blur();
    let screen = h.ctx.screen_rect();
    // The central panel's margin is 8 points; the minimap is flush with it.
    h.click_at(egui::pos2(screen.right() - 8.0 - 3.0, screen.center().y));
    assert!(h.editor_has_focus());
}
