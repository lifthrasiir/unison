//! F2 over a grid and the canvas drag under the backreference shadow: the
//! two rectangles a resize may take.

use super::*;

/// Enter box-resize mode the way a user does: click into the grid, press F2.
fn resize_harness() -> EditorHarness {
    let mut h = EditorHarness::new(RESIZE_SRC);
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "clicking the grid edits it: {:?}",
        h.state.mode
    );
    h.key(Key::F2);
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { item_idx: 0 }),
        "F2 over a grid resizes the glyph: {:?}",
        h.state.mode
    );
    h
}

/// The canvas is resized from under the backreference shadow: click into the
/// grid, `` ` `` twice, then drag an edge. No session exists until the drag has
/// a whole pixel to show for itself, so this leaves the harness *in* the shadow
/// with the pointer ready.
fn canvas_harness() -> EditorHarness {
    let mut h = EditorHarness::new(RESIZE_SRC);
    h.click_grid_cell(1, 0, 0);
    h.key(Key::Backtick);
    h.frame();
    h.key(Key::Backtick);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 0,
                backrefs: true
            }
        ),
        "two backticks put the backreference shadow up: {:?}",
        h.state.mode
    );
    h
}

/// Drag `side` of the glyph's canvas by `cells` logical pixels, starting the
/// canvas session on the way.
fn drag_canvas_edge(h: &mut EditorHarness, cells: f32, from_right: bool) {
    let rect = h.edit_border_rect().expect("the grid's boundary is known");
    let cell = h.snap().grid_cell;
    let grab = if from_right {
        egui::pos2(rect.right(), rect.center().y)
    } else {
        egui::pos2(rect.left(), rect.center().y)
    };
    h.press_at(grab);
    let to = egui::pos2(grab.x + cell * cells, grab.y);
    h.move_pointer(to);
    h.release_at(to);
}

/// An arrow moves the boundary the way it points: `Left` grows the *box*
/// leftwards, which is a left bearing and a wider advance. The drawing does not
/// move — that is the whole difference between this and a canvas resize.
#[test]
fn resize_arrow_grows_the_box_the_way_it_points() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    assert_eq!(h.text(0), "glyph dot 2 2 origin -1 0 advance 3");
    assert_eq!(h.grid(1).width, 2, "the canvas is untouched");
    assert!(
        h.grid(1).get(0, 0).is_bitmap_filled(),
        "and so is the ink in it"
    );
    assert_view_consistent(&h);
}

/// The right edge is the advance alone: no bearing, since the box's corner has
/// not moved.
#[test]
fn resize_arrow_on_the_far_edge_is_the_advance_alone() {
    let mut h = resize_harness();
    h.key(Key::ArrowRight);
    assert_eq!(h.text(0), "glyph dot 2 2 advance 3");
}

/// A vertical drag states the height, which the box otherwise takes from the em
/// box — so it is `extent` that gets written, and the width comes along.
#[test]
fn resize_vertical_states_the_boxs_height() {
    let mut h = resize_harness();
    h.key(Key::ArrowUp);
    assert_eq!(h.text(0), "glyph dot 2 2 origin 0 -1 extent 2 17");
}

/// Shift moves the *far* edge, so the boundary still travels the way the key
/// points and the box shrinks instead of growing.
#[test]
fn resize_shift_arrow_moves_the_far_edge() {
    let mut h = resize_harness();
    h.key_mod(Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(
        h.text(0),
        "glyph dot 2 2 extent 2 15",
        "the bottom edge came up, and the box's corner stayed"
    );
    assert_eq!(h.grid(1).height, 2, "the canvas is untouched");
}

/// A box may be empty — `advance 0` is what every combining mark says — but it
/// may not be inside out.
#[test]
fn a_box_may_shrink_to_nothing_and_no_further() {
    let mut h = resize_harness();
    // Shift+Left pulls the *right* edge in, so the box narrows from the side
    // its corner is not on.
    h.key_mod(Key::ArrowLeft, Modifiers::SHIFT);
    h.key_mod(Key::ArrowLeft, Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph dot 2 2 advance 0");
    h.key_mod(Key::ArrowLeft, Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph dot 2 2 advance 0", "and no further");
}

/// Escape puts the document back exactly as it was, in one step, and leaves
/// the mode it was entered from.
#[test]
fn resize_escape_restores_the_glyph() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowUp);
    assert_eq!(h.text(0), "glyph dot 2 2 origin -1 -1 extent 3 17");
    h.key(Key::Escape);
    assert_eq!(h.text(0), "glyph dot 2 2");
    assert_eq!(h.grid(1).width, 2);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "cancelling goes back to the mode F2 was pressed in: {:?}",
        h.state.mode
    );
    assert!(
        h.take_resize().is_none(),
        "a cancelled resize asks for nothing"
    );
    assert_view_consistent(&h);
}

/// Enter hands the resize to the host and rolls the preview back: the host is
/// the only thing that can move the `ref`s in the other files, so it redoes
/// the whole edit as the one entry it records.
#[test]
fn resize_enter_hands_the_action_to_the_host() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    let action = h.take_resize().expect("the resize was applied");
    assert_eq!(action.glyph_name, "dot");
    assert_eq!(action.deltas.left, 2);
    assert_eq!(action.deltas.bottom, 1);
    assert_eq!(action.deltas.right, 0);
    assert_eq!(action.deltas.top, 0);
    assert_eq!(
        h.text(0),
        "glyph dot 2 2",
        "the editor leaves the document untouched for the host to edit once"
    );
    assert!(matches!(
        h.state.mode,
        EditMode::GlyphEdit { item_idx: 0, .. }
    ));
}

/// The panel beside the grid is Apply and Cancel and nothing else while a
/// resize is live; clicking Cancel is Escape.
#[test]
fn resize_panel_cancel_button_restores_the_glyph() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    assert_eq!(h.text(0), "glyph dot 2 2 origin -1 0 advance 3");
    let pos = h.resize_button_pos(crate::editor::glyph_resize::PanelAction::Cancel);
    h.click_at(pos);
    assert_eq!(h.text(0), "glyph dot 2 2");
    assert!(h.take_resize().is_none());
}

/// ...and clicking Apply is Enter.
#[test]
fn resize_panel_apply_button_hands_over_the_action() {
    let mut h = resize_harness();
    h.key(Key::ArrowRight);
    let pos = h.resize_button_pos(crate::editor::glyph_resize::PanelAction::Apply);
    h.click_at(pos);
    let action = h.take_resize().expect("the resize was applied");
    assert_eq!(action.deltas.right, 1);
    assert_eq!(h.text(0), "glyph dot 2 2");
}

/// Dragging the boundary moves it a whole logical pixel at a time.
#[test]
fn resize_drag_moves_the_grabbed_edge() {
    let mut h = resize_harness();
    let rect = h.edit_border_rect().expect("the boundary is painted");
    let cell = h.snap().grid_cell;
    let grab = egui::pos2(rect.right(), rect.center().y);
    h.press_at(grab);
    h.move_pointer(egui::pos2(grab.x + cell * 2.0, grab.y));
    h.release_at(egui::pos2(grab.x + cell * 2.0, grab.y));
    assert_eq!(
        h.text(0),
        "glyph dot 2 2 advance 4",
        "the box's right edge followed the pointer"
    );
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { .. }),
        "a press on the boundary grabs an edge rather than painting a pixel",
    );
}

/// The canvas is what the backreference shadow resizes, and the mode switches
/// when the drag has something to show — not when the pointer goes down.
#[test]
fn a_drag_under_the_backref_shadow_resizes_the_canvas() {
    let mut h = canvas_harness();
    drag_canvas_edge(&mut h, 2.0, true);
    assert_eq!(
        h.text(0),
        "glyph dot 4 2 advance 2",
        "the grid grew to the right, and the box it would have widened is pinned"
    );
    assert_eq!(h.grid(1).width, 4);
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { item_idx: 0 }),
        "the drag took the mode with it: {:?}",
        h.state.mode
    );
    assert!(
        h.state.pixel_selection.is_none(),
        "the press that grabbed the edge did not also leave a selection behind"
    );
    // Growing to the left moves the ink inside the grid, which is the canvas
    // resize's own signature.
    h.key(Key::Escape);
    let mut h = canvas_harness();
    drag_canvas_edge(&mut h, -1.0, false);
    assert_eq!(
        h.text(0),
        "glyph dot 3 2 origin 1 0 advance 2",
        "growing to the left states the origin that keeps the ink where it was"
    );
    assert!(
        h.grid(1).get(0, 1).is_bitmap_filled() && !h.grid(1).get(0, 0).is_bitmap_filled(),
        "the ink moved right with the new column, not into it"
    );
}

/// The canvas being grabbable has to be *visible*: the backreference shadow is
/// where a resize starts, and a boundary with nothing on it says only that the
/// glyph ends there. So the four handles are drawn as soon as the shadow is up,
/// before any pointer goes near them.
#[test]
fn the_backref_shadow_marks_the_canvas_as_grabbable() {
    let h = canvas_harness();
    let border = h.edit_border_rect().expect("the grid's boundary is known");
    // Dimmed: the shadow is up to be looked at, so the marker is quieter than
    // the overlay a live drag paints.
    let color = crate::editor::colors::Palette::dark()
        .pixel_selection
        .gamma_multiply(0.55);
    let handles: Vec<_> = h
        .painted_rects()
        .into_iter()
        .filter(|p| {
            p.fill == color && p.rect.area() > 0.0 && border.expand(1.0).contains_rect(p.rect)
        })
        .collect();
    assert_eq!(
        handles.len(),
        4,
        "one handle per edge, got {handles:?} over {border:?}"
    );

    // With the shadow off there is nothing to grab, so nothing is marked.
    let mut plain = EditorHarness::new(RESIZE_SRC);
    plain.click_grid_cell(1, 0, 0);
    plain.key(Key::Backtick);
    plain.frame();
    assert!(
        matches!(
            plain.state.mode,
            EditMode::PixelSelect {
                backrefs: false,
                ..
            }
        ),
        "one backtick is selection without the shadow: {:?}",
        plain.state.mode
    );
    let marked = plain
        .painted_rects()
        .into_iter()
        .filter(|p| p.fill == color && p.rect.area() > 0.0)
        .count();
    assert_eq!(marked, 0, "nothing is grabbable, so nothing is marked");
}

/// The shadow stays up for the drag it started. It is what the new size is
/// being judged against, and it is wider than the canvas nearly always — so
/// dropping it at the mode switch would shrink the drawn area out from under
/// the pointer, mid-gesture.
#[test]
fn the_shadow_survives_the_resize_it_starts() {
    let mut h = canvas_harness();
    let grid_line = first_grid_line(&h);
    let with_shadow = grid_extent_x(&h, grid_line);
    assert_eq!(
        with_shadow,
        (-1, 3),
        "`user` places `dot` at (1, 1) and is four wide, so the shadow reaches \
         one column before this glyph and three past its own two"
    );

    drag_canvas_edge(&mut h, 1.0, true);
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { item_idx: 0 }),
        "the drag started a session: {:?}",
        h.state.mode
    );
    assert_eq!(
        grid_extent_x(&h, grid_line),
        with_shadow,
        "the drawn area kept the shadow it was measured against"
    );

    // Cancelling goes back to the shadow the drag was started from, so the
    // drawn area does not move then either. The shadow goes when it is asked
    // to, which is the third `` ` ``.
    h.key(Key::Escape);
    h.frame();
    assert!(matches!(
        h.state.mode,
        EditMode::PixelSelect {
            item_idx: 0,
            backrefs: true
        }
    ));
    assert_eq!(grid_extent_x(&h, grid_line), with_shadow);
    h.key(Key::Backtick);
    h.frame();
    assert_eq!(grid_extent_x(&h, grid_line), (0, 2));
}

/// The shadow follows the document it is drawn from. Stepping through history
/// changes where every parent places this glyph, so the shadow has to move in
/// the frame the step happens in — leaving it to the next pointer event shows a
/// placement that is no longer true, which is exactly the state an undo is
/// supposed to leave nothing in.
#[test]
fn the_backref_shadow_follows_a_step_through_history() {
    let mut h = EditorHarness::new(
        "\
glyph dot 2 2 origin 1 0
@@..
..@@

glyph user 4 4
ref dot 3 1
",
    );
    // An edit to the box, made before the shadow is up so the click that makes
    // it does not leave the mode.
    h.click_text(0, "glyph dot 2 2 origin 1".chars().count());
    h.key(Key::Backspace);
    h.type_text("2");
    h.frame();
    assert_eq!(h.text(0), "glyph dot 2 2 origin 2 0");

    h.click_grid_cell(1, 0, 0);
    h.key(Key::Backtick);
    h.frame();
    h.key(Key::Backtick);
    h.frame();
    let grid_line = first_grid_line(&h);
    let with_two = grid_extent_x(&h, grid_line);
    assert_eq!(
        with_two,
        (-1, 3),
        "`user` places the box's corner at column 3, so its own grid starts one \
         column before this one"
    );

    h.key_mod(Key::Z, Modifiers::COMMAND);
    h.frame();
    assert_eq!(h.text(0), "glyph dot 2 2 origin 1 0");
    assert_eq!(
        grid_extent_x(&h, grid_line),
        (-2, 2),
        "the shadow moved with the box the undo restored, in the same frame"
    );
}

/// A press that goes nowhere is not a resize: the mode it was made in survives.
#[test]
fn a_press_on_the_border_that_does_not_move_changes_no_mode() {
    let mut h = canvas_harness();
    let rect = h.edit_border_rect().expect("the grid's boundary is known");
    let grab = egui::pos2(rect.right(), rect.center().y);
    h.press_at(grab);
    h.move_pointer(egui::pos2(grab.x + 2.0, grab.y));
    h.release_at(egui::pos2(grab.x + 2.0, grab.y));
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 0,
                backrefs: true
            }
        ),
        "half a pixel of travel is not a resize: {:?}",
        h.state.mode
    );
    assert_eq!(h.text(0), "glyph dot 2 2");
}

/// The mode is over the moment the editor is not the surface being typed
/// into: a resize nobody can see must not stay half-applied in the buffer.
#[test]
fn resize_is_cancelled_by_losing_the_focus() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    h.blur();
    assert_eq!(h.text(0), "glyph dot 2 2");
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));
    assert!(h.take_resize().is_none());
}

/// F2 with the caret merely sitting on a grid line resizes too — no pixel mode
/// needed — and cancelling goes back to that plain caret.
#[test]
fn resize_starts_from_a_caret_on_the_grid_line() {
    let mut h = EditorHarness::new(RESIZE_SRC);
    h.click_text(0, 0);
    h.state.cursor = Caret::new(1, 0);
    h.frame();
    h.key(Key::F2);
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { item_idx: 0 }),
        "{:?}",
        h.state.mode
    );
    h.key(Key::Escape);
    assert!(
        matches!(h.state.mode, EditMode::Normal),
        "{:?}",
        h.state.mode
    );
}

/// The panel is chrome, not glyph rendering, so it has to follow the theme.
/// The editor's own palette deliberately keeps its panel colours dark in both
/// themes — they sit behind glyph swatches over the dark grid — and reusing
/// them here left dark text on a dark button in light mode.
#[test]
fn resize_buttons_stay_legible_in_both_themes() {
    use crate::editor::glyph_resize::PanelAction;

    /// Perceived brightness, 0..1, of a colour painted over the given theme.
    fn luma(c: egui::Color32) -> f32 {
        (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
    }

    let mut h = resize_harness();
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        h.set_theme(theme);
        for action in [PanelAction::Apply, PanelAction::Cancel] {
            let (fill, text) = h.resize_button_colors(action);
            assert!(
                (luma(fill) - luma(text)).abs() > 0.25,
                "{action:?} is unreadable in {theme:?}: text {text:?} on {fill:?}",
            );
        }
    }
}

/// The four handles have to be *inside* the glyph's box, because the grid band
/// is clipped to exactly where the grid starts: a handle centred on the left
/// edge of a glyph at column 0 falls in the half that is clipped away, so it
/// was invisible even though the band around it still grabbed the pointer.
#[test]
fn resize_handles_are_inside_the_drawn_band() {
    let h = resize_harness();
    let rect = h.edit_border_rect().expect("the boundary is painted");
    let strip = &h.snap().strip;
    for (side, handle) in crate::editor::glyph_resize::handle_rects(rect, 1.0) {
        assert!(
            handle.left() >= strip.x && handle.right() <= strip.right(),
            "the {side:?} handle is outside the clipped band: {handle:?} vs {}..{}",
            strip.x,
            strip.right(),
        );
        assert!(
            rect.contains_rect(handle),
            "the {side:?} handle is outside the glyph's own box: {handle:?} vs {rect:?}",
        );
    }
}

/// The overlay has to be painted *over* the glyph, not under it. It used to go
/// out with the first visible grid row, so every row below that one painted
/// over it: with the handles moved inside the box, all that survived was the
/// top border and one cell's worth of the sides.
#[test]
fn resize_overlay_is_painted_over_the_grid() {
    let h = resize_harness();
    let border = h.edit_border_rect().expect("the boundary is painted");
    let color = crate::editor::colors::Palette::dark().pixel_selection;
    let painted = h.painted_rects();
    // The outline and its four handles are all painted in the overlay colour;
    // what must not follow them is anything of the grid.
    let last_overlay = painted
        .iter()
        .rposition(|p| p.fill == color || p.stroke.color == color)
        .expect("the overlay is among the painted rects");
    assert!(
        painted[last_overlay].rect.expand(0.5).intersects(border),
        "the overlay belongs to this glyph's box",
    );
    for later in &painted[last_overlay + 1..] {
        assert!(
            !(later.fill.a() > 0
                && later
                    .rect
                    .intersect(later.clip)
                    .intersects(border.shrink(1.0))),
            "{later:?} is painted over the resize overlay {border:?}",
        );
    }
}
