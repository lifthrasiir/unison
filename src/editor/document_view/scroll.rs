//! Scrolling: wheel/page handling, physics, grid horizontal scrollbars and
//! drag auto-scroll.

use super::*;
use super::layout::{GridBlock, GridStrip, VLineKind, VisualLine, doc_line_to_y};

const COARSE_SCROLL_COOLDOWN: f64 = 0.05;

/// Wheel step for hover-scroll gestures that arrived via the scroll
/// interceptor: `Some(step)` only when the interceptor captured the gesture
/// and the pointer hovers the target area.
pub(crate) fn interceptor_scroll_step(ctx: &egui::Context, hovering: bool) -> Option<i32> {
    if !hovering {
        return None;
    }
    let on_interceptor = ctx.data(|d| {
        d.get_temp::<bool>(egui::Id::new("scroll_on_interceptor"))
            .unwrap_or(false)
    });
    if !on_interceptor {
        return None;
    }
    debounced_scroll_step(ctx)
}

pub(crate) fn debounced_scroll_step(ctx: &egui::Context) -> Option<i32> {
    let now = ctx.input(|i| i.time);
    ctx.input(|i| i.pointer.hover_pos())?;

    let mut direction: Option<i32> = None;
    ctx.input(|i| {
        for event in &i.events {
            if let egui::Event::MouseWheel { delta, .. } = event {
                if delta.y > 0.0 {
                    direction = Some(-1);
                } else if delta.y < 0.0 {
                    direction = Some(1);
                }
            }
        }
    });

    let dir = direction?;

    let id = egui::Id::new("coarse_scroll_debounce");
    let (last_time, last_dir): (f64, i32) = ctx.data(|d| d.get_temp(id).unwrap_or((0.0, 0)));

    if dir == last_dir && now - last_time < COARSE_SCROLL_COOLDOWN {
        return None;
    }

    ctx.data_mut(|d| d.insert_temp(id, (now, dir)));
    Some(dir)
}

/// Horizontal grid scrollbar: thickness, distance below the grid, and the
/// width of the edge band that triggers auto-scrolling while dragging.
pub(super) const HSCROLL_HEIGHT: f32 = 8.0;

pub(super) const HSCROLL_GAP: f32 = 2.0;

const HSCROLL_EDGE_ZONE: f32 = 24.0;

/// Auto-scroll speed at the outer end of the edge band, in points per second.
const HSCROLL_AUTO_SPEED: f32 = 900.0;

const SCROLL_BASE_MULTIPLIER: f32 = 2.5;

const SCROLL_ACCEL_START: u32 = 3;

const SCROLL_ACCEL_STEP: f32 = 0.8;

const SCROLL_ACCEL_MAX: f32 = 5.0;

const SCROLL_RAPID_THRESHOLD: f64 = 0.12;

const SCROLL_ACCEL_RESET: f64 = 0.20;

const SCROLL_GESTURE_GRACE: f64 = 0.50;

pub(super) fn hscroll_drag_id() -> egui::Id {
    egui::Id::new("grid_hscroll_dragging")
}

/// Track whether this scroll gesture started on an interceptor area
/// (grid, subglyph preview, shape palette). Once a gesture begins, lock
/// in the starting zone so that scrolling the document doesn't
/// accidentally switch to palette selection when the grid passes under
/// the cursor.
pub(super) fn lock_scroll_gesture_zone(ui: &mut egui::Ui, grid_hover: bool) {
    let scroll_on_interceptor = {
        let currently_on = ui.ctx().data(|d| {
            d.get_temp::<bool>(egui::Id::new("subglyph_preview_hover"))
                .unwrap_or(false)
                || d.get_temp::<bool>(egui::Id::new("shape_palette_hover"))
                    .unwrap_or(false)
        }) || grid_hover;

        let has_wheel = ui.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::MouseWheel { .. }))
        });

        let gesture_id = egui::Id::new("scroll_gesture_zone");
        let now = ui.input(|i| i.time);

        if has_wheel {
            let prev: Option<(f64, bool)> = ui.ctx().data(|d| d.get_temp(gesture_id));
            let on = match prev {
                Some((t, was_on)) if now - t < SCROLL_GESTURE_GRACE => was_on,
                _ => currently_on,
            };
            ui.ctx()
                .data_mut(|d| d.insert_temp(gesture_id, (now, on)));
            on
        } else {
            ui.ctx()
                .data(|d| d.get_temp::<(f64, bool)>(gesture_id))
                .is_some_and(|(_, on)| on)
                && ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.01)
        }
    };
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("scroll_on_interceptor"),
            scroll_on_interceptor,
        );
    });
    if scroll_on_interceptor {
        ui.ctx()
            .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }
}

/// PageUp/PageDown: scroll so last/first fully visible line becomes
/// first/last; move the caret by the same number of visual lines.
/// A "sticky vline index" survives across presses so that landing
/// inside a multi-row grid doesn't snap to the grid top and drift.
#[expect(clippy::too_many_arguments)]
pub(super) fn handle_page_scroll(
    ui: &egui::Ui,
    lines: &[DocLine],
    state: &mut EditorState,
    vlines: &[VisualLine],
    row_height: f32,
    grid_cell: f32,
    prev_scroll_y: f32,
    prev_viewport_h: f32,
) {
    // Clear page-scroll sticky state when cursor moved by non-page means.
    {
        let id = egui::Id::new("page_last_cursor");
        let prev: Option<(usize, usize)> =
            ui.ctx().data(|d| d.get_temp(id));
        if prev.is_some_and(|(l, c)| l != state.cursor.line || c != state.cursor.col) {
            ui.ctx().data_mut(|d| {
                d.remove::<usize>(egui::Id::new("page_sticky_vi"));
                d.remove::<(usize, usize)>(id);
            });
        }
    }

    let page_id = egui::Id::new("page_scroll_request");
    let Some((dir, shift)) = ui.ctx().data(|d| d.get_temp::<(i32, bool)>(page_id)) else {
        return;
    };
    ui.ctx().data_mut(|d| d.remove::<(i32, bool)>(page_id));
    if shift {
        if state.selection_anchor.is_none() {
            state.selection_anchor = Some(state.cursor);
        }
    } else {
        state.selection_anchor = None;
    }

    let eps = 0.5;

    let mut vline_y: Vec<f32> = Vec::with_capacity(vlines.len());
    let mut y_acc = 0.0f32;
    for vl in vlines {
        vline_y.push(y_acc);
        y_acc += vl.height(row_height, grid_cell);
    }

    let mut first_vis: Option<usize> = None;
    let mut last_vis: Option<usize> = None;
    for i in 0..vlines.len() {
        let top = vline_y[i];
        let h = vlines[i].height(row_height, grid_cell);
        if top >= prev_scroll_y - eps
            && top + h <= prev_scroll_y + prev_viewport_h + eps
        {
            if first_vis.is_none() {
                first_vis = Some(i);
            }
            last_vis = Some(i);
        }
    }

    let sticky_vi_id = egui::Id::new("page_sticky_vi");
    let cursor_vi = ui
        .ctx()
        .data(|d| d.get_temp::<usize>(sticky_vi_id))
        .map(|vi| vi.min(vlines.len().saturating_sub(1)))
        .unwrap_or_else(|| {
            let mut result = 0usize;
            let mut in_target = false;
            for (i, vl) in vlines.iter().enumerate() {
                if vl.doc_line != state.cursor.line {
                    if in_target {
                        break;
                    }
                    continue;
                }
                in_target = true;
                result = i;
                match &vl.kind {
                    VLineKind::Text(text) => {
                        let seg_len = text.chars().count();
                        if state.cursor.col >= vl.col_offset
                            && state.cursor.col <= vl.col_offset + seg_len
                        {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            result
        });

    if let (Some(fv), Some(lv)) = (first_vis, last_vis) {
        let page_vlines = lv - fv;
        if page_vlines > 0 {
            let (new_scroll, new_cvi) = if dir > 0 {
                (vline_y[lv], (cursor_vi + page_vlines).min(vlines.len() - 1))
            } else {
                let fh = vlines[fv].height(row_height, grid_cell);
                let ns = (vline_y[fv] + fh - prev_viewport_h).max(0.0);
                (ns, cursor_vi.saturating_sub(page_vlines))
            };

            let new_line = vlines[new_cvi].doc_line;
            state.cursor = Caret::new(
                new_line,
                state.cursor.col.min(caret::line_char_len(lines, new_line)),
            );

            ui.ctx().data_mut(|d| {
                d.insert_temp(sticky_vi_id, new_cvi);
                d.insert_temp(
                    egui::Id::new("page_last_cursor"),
                    (state.cursor.line, state.cursor.col),
                );
                d.insert_temp(egui::Id::new("goto_scroll_target"), new_scroll);
            });
        }
    }
}

/// Where the scroll area should jump this frame, if anywhere: minimap click,
/// pending goto, scroll-to-cursor request, zoom recentering, or restoring
/// the saved position — in that priority order.
#[expect(clippy::too_many_arguments)]
pub(super) fn resolve_scroll_target(
    ui: &egui::Ui,
    state: &mut EditorState,
    vlines: &[VisualLine],
    row_height: f32,
    grid_cell: f32,
    zoom_level: u32,
    prev_scroll_y: f32,
    viewport_h: f32,
    total_height: f32,
    minimap_scroll_target: Option<f32>,
) -> Option<f32> {
    // When zoom level changes, adjust scroll so the content under the mouse
    // pointer stays at the same screen position.
    let zoom_scroll: Option<f32> = {
        let old_zoom: Option<u32> = state.take_zoom_change();
        if let Some(old_z) = old_zoom {
            let scale = zoom_level as f32 / old_z as f32;
            let pointer_y = ui.ctx().input(|i| i.pointer.hover_pos().map(|p| p.y));
            if let Some(py) = pointer_y {
                let viewport_top = ui.max_rect().top();
                let pvo = py - viewport_top;
                let old_doc_y = prev_scroll_y + pvo;
                Some((old_doc_y * scale - pvo).max(0.0))
            } else {
                let new_caret_y =
                    doc_line_to_y(vlines, row_height, grid_cell, state.cursor.line);
                let old_caret_y = new_caret_y / scale;
                let visual_offset = (old_caret_y - prev_scroll_y).max(0.0);
                Some((new_caret_y - visual_offset).max(0.0))
            }
        } else {
            None
        }
    };

    let goto_scroll_id = egui::Id::new("goto_scroll_target");
    let goto_scroll: Option<f32> = ui.ctx().data(|d| d.get_temp::<f32>(goto_scroll_id));
    if goto_scroll.is_some() {
        ui.ctx().data_mut(|d| d.remove::<f32>(goto_scroll_id));
    }
    let scroll_to_cursor = state.take_scroll_to_cursor();
    let cursor_scroll = if scroll_to_cursor {
        let target_y = doc_line_to_y(vlines, row_height, grid_cell, state.cursor.line);
        Some((target_y - viewport_h / 3.0).max(0.0))
    } else {
        None
    };
    let saved_scroll_y = (state.saved_scroll_frac * total_height - viewport_h / 2.0).max(0.0);
    let restore_scroll = if minimap_scroll_target.is_none()
        && goto_scroll.is_none()
        && cursor_scroll.is_none()
        && zoom_scroll.is_none()
        && (saved_scroll_y - prev_scroll_y).abs() > 1.0
    {
        Some(saved_scroll_y)
    } else {
        None
    };
    minimap_scroll_target
        .or(goto_scroll)
        .or(cursor_scroll)
        .or(zoom_scroll)
        .or(restore_scroll)
}

/// Queues a scroll so a cursor that moved this frame stays inside the
/// viewport (with a half-row margin; over-tall lines align their top).
#[expect(clippy::too_many_arguments)]
pub(super) fn scroll_cursor_into_view(
    ui: &egui::Ui,
    state: &EditorState,
    vlines: &[VisualLine],
    row_height: f32,
    grid_cell: f32,
    prev_cursor: Caret,
    scroll_y: f32,
    viewport_h: f32,
) {
    if state.cursor != prev_cursor {
        let cursor_y = doc_line_to_y(vlines, row_height, grid_cell, state.cursor.line);
        let cursor_h: f32 = vlines
            .iter()
            .filter(|vl| vl.doc_line == state.cursor.line)
            .map(|vl| vl.height(row_height, grid_cell))
            .sum();
        let cursor_h = if cursor_h > 0.0 { cursor_h } else { row_height };
        let margin = row_height * 0.5;
        if cursor_h + margin * 2.0 <= viewport_h {
            if cursor_y < scroll_y + margin {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("goto_scroll_target"),
                        (cursor_y - margin).max(0.0),
                    );
                });
            } else if cursor_y + cursor_h > scroll_y + viewport_h - margin {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("goto_scroll_target"),
                        (cursor_y + cursor_h - viewport_h + margin).max(0.0),
                    );
                });
            }
        } else if cursor_y < scroll_y + margin
            || cursor_y + cursor_h > scroll_y + viewport_h - margin
        {
            // Cursor line taller than the viewport: align its top.
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("goto_scroll_target"),
                    (cursor_y - margin).max(0.0),
                );
            });
        }
    }
}

/// Smallest thumb we will draw, so a very wide grid still leaves something
/// to grab.
const HSCROLL_MIN_THUMB: f32 = 24.0;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_grid_hscrollbars(
    ui: &egui::Ui,
    painter: &egui::Painter,
    state: &mut EditorState,
    strip: &GridStrip,
    blocks: &[GridBlock],
    hbars: &[(usize, egui::Rect)],
    zoom_level: u32,
    pal: &Palette,
) {
    let zoom = zoom_level as f32;
    let mut dragging = false;
    for (block_idx, bar) in hbars {
        let Some(block) = blocks.get(*block_idx) else {
            continue;
        };
        let overflow = strip.overflow(block.content_w);
        if overflow <= 0.0 {
            continue;
        }

        let track_w = bar.width();
        let thumb_w = (track_w * strip.w / block.content_w)
            .clamp((HSCROLL_MIN_THUMB * zoom).min(track_w), track_w);
        let travel = track_w - thumb_w;

        let resp = ui.interact(
            *bar,
            egui::Id::new(("grid_hscroll", block.item_idx)),
            egui::Sense::click_and_drag(),
        );
        dragging |= resp.is_pointer_button_down_on() || resp.dragged();

        let scroll = state.grid_scroll_x.min(overflow);
        let thumb_x = bar.min.x + if travel > 0.5 { travel * scroll / overflow } else { 0.0 };

        // A press outside the thumb jumps to that position first; the drag
        // then continues from there.
        let jump_to = |px: f32| {
            (((px - bar.min.x - thumb_w * 0.5) / travel.max(1.0)).clamp(0.0, 1.0)) * overflow
        };
        let mut new_scroll = scroll;
        if (resp.drag_started() || resp.clicked())
            && let Some(p) = resp.interact_pointer_pos()
            && (p.x < thumb_x || p.x > thumb_x + thumb_w)
        {
            new_scroll = jump_to(p.x);
        }
        if resp.dragged() && travel > 0.5 {
            new_scroll += resp.drag_delta().x * overflow / travel;
        }
        let new_scroll = new_scroll.clamp(0.0, overflow);
        if (new_scroll - state.grid_scroll_x).abs() > 0.01 {
            state.grid_scroll_x = new_scroll;
            ui.ctx().request_repaint();
        }

        let radius = bar.height() * 0.5;
        painter.rect_filled(*bar, radius, pal.hscroll_track);
        let thumb = egui::Rect::from_min_size(
            egui::pos2(
                bar.min.x + if travel > 0.5 { travel * new_scroll / overflow } else { 0.0 },
                bar.min.y,
            ),
            egui::vec2(thumb_w, bar.height()),
        );
        let color = if resp.dragged() || resp.hovered() {
            pal.hscroll_thumb_active
        } else {
            pal.hscroll_thumb
        };
        painter.rect_filled(thumb, radius, color);
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(hscroll_drag_id(), dragging));
}

/// While dragging inside a grid, holding the pointer near (or past) either
/// edge of the band scrolls it, so a selection or a layer can be dragged
/// beyond the visible columns.
pub(super) fn auto_scroll_grid_on_drag(
    ui: &egui::Ui,
    state: &mut EditorState,
    strip: &GridStrip,
    blocks: &[GridBlock],
    origin: egui::Pos2,
    zoom_level: u32,
) {
    if matches!(state.mode, EditMode::Normal) || !ui.input(|i| i.pointer.primary_down()) {
        return;
    }
    let Some(hp) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    if strip.captured || strip.bars.iter().any(|r| r.contains(hp)) {
        return;
    }
    // Only a gesture that started on the grid itself scrolls it. The inline
    // tool panel sits just past the band's right edge, i.e. inside the edge
    // zone, so a press there would otherwise be read as a drag to the edge.
    let started_on_grid = ui
        .input(|i| i.pointer.press_origin())
        .is_some_and(|p| strip.contains_x(p.x));
    if !started_on_grid {
        return;
    }
    let Some(block) = blocks.iter().find(|b| {
        hp.y >= origin.y + b.y0 && hp.y < origin.y + b.y1 && strip.overflow(b.content_w) > 0.0
    }) else {
        return;
    };

    let zoom = zoom_level as f32;
    let edge = HSCROLL_EDGE_ZONE * zoom;
    let past_right = hp.x - (strip.right() - edge);
    let past_left = (strip.x + edge) - hp.x;
    let ratio = if past_right > 0.0 {
        (past_right / edge).min(1.0)
    } else if past_left > 0.0 {
        -(past_left / edge).min(1.0)
    } else {
        return;
    };

    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let overflow = strip.overflow(block.content_w);
    let next =
        (state.grid_scroll_x + ratio * HSCROLL_AUTO_SPEED * zoom * dt).clamp(0.0, overflow);
    if (next - state.grid_scroll_x).abs() > 0.01 {
        state.grid_scroll_x = next;
    }
    ui.ctx().request_repaint();
}

pub(crate) fn apply_scroll_physics(ui: &egui::Ui, zoom_level: u32, salt: &str) {
    let cmd_held = ui.input(|i| i.modifiers.command);
    if cmd_held {
        return;
    }

    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pos| ui.max_rect().contains(pos));
    if !hovered {
        return;
    }

    let accel_id = egui::Id::new(("scroll_accel_state", salt));
    let now = ui.input(|i| i.time);

    let has_line_scroll = ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    ..
                }
            )
        })
    });
    let has_point_scroll = ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    ..
                }
            )
        })
    });

    let (mut last_tick_time, mut consecutive_ticks): (f64, u32) = ui
        .ctx()
        .data(|d| d.get_temp(accel_id).unwrap_or((0.0f64, 0u32)));

    if has_line_scroll {
        if now - last_tick_time < SCROLL_RAPID_THRESHOLD {
            consecutive_ticks += 1;
        } else {
            consecutive_ticks = 1;
        }
        last_tick_time = now;
    } else if now - last_tick_time > SCROLL_ACCEL_RESET {
        consecutive_ticks = 0;
    }

    ui.ctx()
        .data_mut(|d| d.insert_temp(accel_id, (last_tick_time, consecutive_ticks)));

    let accel = if consecutive_ticks > SCROLL_ACCEL_START {
        (1.0 + (consecutive_ticks - SCROLL_ACCEL_START) as f32 * SCROLL_ACCEL_STEP)
            .min(SCROLL_ACCEL_MAX)
    } else {
        1.0
    };

    let multiplier = SCROLL_BASE_MULTIPLIER * accel * zoom_level as f32;
    let in_discrete_tail = !has_line_scroll
        && !has_point_scroll
        && consecutive_ticks > 0
        && ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.1);

    if has_line_scroll || in_discrete_tail {
        ui.ctx()
            .input_mut(|i| i.smooth_scroll_delta.y *= multiplier);
    }
}
