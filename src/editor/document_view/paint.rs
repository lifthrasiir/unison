//! Painting the document area: text, pixel grids, selection, the edit border
//! and color backgrounds.

use super::changes::{apply_edit_action_to_editor, apply_inline_action};
use super::layout::{
    GridStrip, VLineKind, VisualLine, collect_grid_blocks, doc_line_to_y, gutter_line_number,
    inline_panel_reserved_width, visible_grid_rect,
};
use super::scroll::{
    HSCROLL_GAP, HSCROLL_HEIGHT, auto_scroll_grid_on_drag, draw_grid_hscrollbars, hscroll_drag_id,
    interceptor_scroll_step,
};
use super::*;

/// The scrollable document canvas: allocates the full-height area, paints
/// every visual line (text, grids, selection, links, caret) and handles all
/// pointer interaction inside it — clicks, drags, wheel gestures, the inline
/// tool panel, layer-move drags and the context menu.
#[expect(clippy::too_many_arguments)]
pub(super) fn paint_document_area(
    ui: &mut egui::Ui,
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    env: EditorEnv<'_>,
    vlines: &[VisualLine],
    composites: &HashMap<usize, GlyphComposite>,
    shadow: Option<&(usize, AnchorShadow)>,
    source_offsets: &[usize],
    pal: &Palette,
    row_height: f32,
    grid_cell: f32,
    gutter_width: f32,
    total_height: f32,
    viewport_h: f32,
    cursor_color: egui::Color32,
    inline_panel_edit_idx: Option<usize>,
    needs_rederive: &mut bool,
) {
    let EditorEnv {
        named_glyphs,
        name_parts,
        color_aliases,
        zoom_level,
        font_id,
        menu_open,
        ..
    } = env;
    let avail_w = ui.available_width();
    let desired = egui::vec2(avail_w, total_height.max(row_height));
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());

    let wid = response.id;
    state.canvas_id = Some(wid);
    if response.clicked() || response.drag_started() || std::mem::take(&mut state.pending_focus) {
        ui.memory_mut(|m| m.request_focus(wid));
    }
    let has_focus = ui.memory(|m| m.has_focus(wid));
    state.active = has_focus;

    *needs_rederive |= pixel_selection::reconcile(doc, lines, state, menu_open);

    // A resize is a modal gesture over one glyph: the moment the editor is no
    // longer the surface being typed into, it is off. `menu_open` is the one
    // exception every modal state here makes — the focus went to a menu button
    // drawn over this editor, not to another document.
    if matches!(state.mode, EditMode::GlyphResize { .. }) {
        if !crate::editor::glyph_resize::still_valid(doc, state) {
            crate::editor::glyph_resize::abandon(state);
        } else if !has_focus && !menu_open {
            *needs_rederive |= crate::editor::glyph_resize::cancel(lines, state);
        }
    } else if state.resize.is_some() {
        // The mode was switched from outside the resize (a host action, an
        // undo that restored a mode). The preview goes with it.
        *needs_rederive |= crate::editor::glyph_resize::cancel(lines, state);
    }

    if has_focus {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                wid,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                    tab: state.autocomplete.is_some(),
                },
            );
        });
        ui.output_mut(|o| o.mutable_text_under_cursor = true);
    }

    let painter = ui.painter_at(rect);
    let gutter_x = rect.min.x;
    let origin = egui::pos2(rect.min.x + gutter_width, rect.min.y);
    let sel = state.selection_range();

    // Grid band: the full editor width, less the space the inline tool
    // panel takes *while a glyph is being edited* — outside editing there
    // is no panel, so reserving for one only narrows the band and scrolls
    // grids that would otherwise fit. Grids wider than the band scroll
    // inside it.
    let blocks = collect_grid_blocks(vlines, row_height, grid_cell);
    let mut strip = {
        let x = origin.x + LEFT_PAD;
        let reserved = if inline_panel_edit_idx.is_some() {
            inline_panel_reserved_width(zoom_level as f32)
        } else {
            0.0
        };
        let w = (rect.max.x - x - reserved).max(grid_cell * 2.0);
        let max_overflow = blocks
            .iter()
            .fold(0.0f32, |acc, b| acc.max(b.content_w - w));
        state.grid_scroll_x = state.grid_scroll_x.clamp(0.0, max_overflow.max(0.0));
        let captured = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(hscroll_drag_id(state.id())))
            .unwrap_or(false)
            && ui.input(|i| i.pointer.primary_down());
        GridStrip {
            x,
            w,
            scroll: state.grid_scroll_x,
            bars: Vec::new(),
            captured,
        }
    };
    // The scrollbar belongs to the glyph being edited only: it sits in
    // the line below the grid, which for any other glyph still holds its
    // `ref`/`anchor` lines. Normally it goes just under the block; a
    // block taller than the viewport would put it out of sight, so in
    // that case it is pulled back in — covering grid rows is the lesser
    // evil there.
    let hbars: Vec<(usize, egui::Rect)> = {
        let view = ui.clip_rect();
        let zoom = zoom_level as f32;
        let bar_h = HSCROLL_HEIGHT * zoom;
        let gap = HSCROLL_GAP * zoom;
        let lo = view.min.y + gap;
        let hi = view.max.y - bar_h - gap;
        blocks
            .iter()
            .enumerate()
            .filter(|_| lo <= hi)
            .filter(|(_, b)| inline_panel_edit_idx == Some(b.item_idx))
            .filter(|(_, b)| strip.overflow(b.content_w) > 0.0)
            .filter(|(_, b)| origin.y + b.y1 >= view.min.y && origin.y + b.y0 <= view.max.y)
            .map(|(i, b)| {
                let bar_y = (origin.y + b.y1 + gap).clamp(lo, hi);
                (
                    i,
                    egui::Rect::from_min_size(
                        egui::pos2(strip.x, bar_y),
                        egui::vec2(strip.w, bar_h),
                    ),
                )
            })
            .collect()
    };
    strip.bars = hbars.iter().map(|(_, r)| *r).collect();

    // Publish the frame's layout for the in-crate GUI test harness.
    #[cfg(test)]
    crate::editor::harness::capture_snapshot(
        ui.ctx(),
        state.id(),
        vlines,
        lines,
        source_offsets,
        origin,
        row_height,
        grid_cell,
        wid,
        &strip,
    );
    let grid_painter = painter.with_clip_rect(
        egui::Rect::from_min_max(
            egui::pos2(strip.x, rect.min.y),
            egui::pos2(strip.right(), rect.max.y),
        )
        .intersect(painter.clip_rect()),
    );

    let is_double = response.double_clicked();
    let is_triple = response.triple_clicked();
    // Only track drag positions for text selection in Normal mode. In edit
    // modes (GlyphEdit/PixelSelect/LayerMove), drag tracking is handled
    // directly via pointer.hover_pos()/delta() by each mode's handler;
    // letting click_pos resolve here would hit a text line and force a
    // mode change back to Normal mid-drag. That goes for the drag's *first*
    // frame too: a layer dragged past the edge of its glyph's own columns
    // (which is the whole point of dragging it) puts the pointer outside
    // the grid on the very frame the drag starts.
    let normal_mode = matches!(state.mode, EditMode::Normal);
    let click_pos =
        if response.clicked() || is_double || is_triple || (response.drag_started() && normal_mode)
        {
            response.interact_pointer_pos()
        } else if response.dragged() && normal_mode {
            ui.input(|i| i.pointer.hover_pos())
        } else {
            None
        };

    let mut click_result: Option<ClickTarget> = None;
    let mut cursor_screen: Option<egui::Pos2> = None;
    let mut error_tooltip: Option<(egui::Pos2, String)> = None;
    let mut goto_glyph_name: Option<String> = None;
    let mut goto_glyph_kind: Option<LinkTargetKind> = None;
    let mut goto_link_pos: Option<Caret> = None;
    let mut goto_is_def = false;
    let mut inline_panel_origin: Option<(f32, f32, f32)> = None; // (x, y, grid_display_width)
    let mut edit_grid_rect: Option<egui::Rect> = None;

    let cmd_held = ui.input(|i| i.modifiers.command);
    let hover_pos = ui.input(|i| i.pointer.hover_pos());

    let clip = ui.clip_rect();
    let vis_top = clip.min.y - origin.y;
    let vis_bottom = clip.max.y - origin.y;

    // The edit and grid-caret borders span the whole glyph but are drawn once,
    // from the first *visible* grid row of their item — the top row may well be
    // scrolled out of view (culled), and drawing only from it lost the border.
    let mut edit_border_drawn = false;
    let mut grid_caret_drawn = false;
    // The glyph boundary a resize drags, once it has been painted.
    let mut resize_rect: Option<egui::Rect> = None;

    let mut y = 0.0f32;
    for vl in vlines {
        let h = vl.height(row_height, grid_cell);

        if y + h < vis_top || y > vis_bottom {
            if matches!(state.mode, EditMode::Normal) && state.cursor.line == vl.doc_line {
                cursor_screen = Some(egui::pos2(origin.x + LEFT_PAD, origin.y + y));
            }
            if inline_panel_origin.is_none()
                && let VLineKind::GridRow {
                    item_idx, extent, ..
                } = &vl.kind
                && inline_panel_edit_idx == Some(*item_idx)
                && vl.kind_row() == Some(extent.top)
            {
                let content_w = extent.display_width(grid_cell);
                let gx = strip.grid_x(content_w);
                let gy = origin.y + y;
                inline_panel_origin = Some((
                    (gx + content_w).min(strip.right()) + INLINE_PANEL_GAP * zoom_level as f32,
                    gy,
                    content_w,
                ));
                edit_grid_rect = visible_grid_rect(
                    &strip,
                    gx,
                    gy,
                    content_w,
                    (extent.bottom - extent.top) as f32 * grid_cell,
                );
            }
            y += h;
            continue;
        }

        let src_line = gutter_line_number(vl, lines, source_offsets);
        if let Some(num) = src_line {
            let num_text = format!("{num:>5} ");
            painter.text(
                egui::pos2(gutter_x, origin.y + y),
                egui::Align2::LEFT_TOP,
                &num_text,
                font_id.clone(),
                pal.line_num,
            );
        }

        if let Some((sel_lo, sel_hi)) = sel {
            draw_selection(
                &painter,
                &grid_painter,
                ui,
                font_id,
                vl,
                origin,
                y,
                h,
                sel_lo,
                sel_hi,
                &strip,
                grid_cell,
            );
        }

        match &vl.kind {
            VLineKind::Text(text) => {
                let atext = AnnotatedText::new(text, &vl.annotations);
                atext.paint(
                    &painter,
                    ui,
                    font_id,
                    egui::pos2(origin.x + LEFT_PAD, origin.y + y),
                    vl.color,
                    vl.comment_col.map(|c| (c, pal.text_comment)),
                );

                // Color background for color tokens in color/ref-fill lines
                let color_spans = paint_color_backgrounds(
                    &painter,
                    ui,
                    font_id,
                    &atext,
                    doc_line_text(lines, vl, text),
                    vl.col_offset,
                    origin.x + LEFT_PAD,
                    origin.y + y,
                    h,
                    color_aliases,
                );
                #[cfg(test)]
                crate::editor::harness::capture_color_spans(
                    ui.ctx(),
                    state.id(),
                    vl.doc_line,
                    &color_spans,
                );
                #[cfg(not(test))]
                let _ = color_spans;

                if !vl.error_spans.is_empty() {
                    let error_color = pal.error;
                    for (col_start, col_end, _msg) in &vl.error_spans {
                        let col_start = *col_start;
                        let col_end = *col_end;
                        let x0 = atext.x_pos(ui, font_id, col_start);
                        let x1 = atext.x_pos(ui, font_id, col_end);
                        let name_text: String = text
                            .chars()
                            .skip(col_start)
                            .take(col_end - col_start)
                            .collect();
                        let name_x0 = origin.x + LEFT_PAD + x0;
                        let name_x1 = origin.x + LEFT_PAD + x1;
                        let name_y0 = origin.y + y;
                        let name_y1 = name_y0 + h;
                        painter.text(
                            egui::pos2(name_x0, name_y0),
                            egui::Align2::LEFT_TOP,
                            &name_text,
                            font_id.clone(),
                            error_color,
                        );
                        painter.line_segment(
                            [
                                egui::pos2(name_x0, name_y1 - 1.0),
                                egui::pos2(name_x1, name_y1 - 1.0),
                            ],
                            egui::Stroke::new(1.0, error_color),
                        );
                    }
                }

                if cmd_held {
                    // Off the whole line, not this segment: a name cut by
                    // the wrap would otherwise name only its own half.
                    let links = doc_links::extract_line_links(
                        doc_line_text(lines, vl, text),
                        crate::document::at_base_at_line(lines, vl.doc_line).as_deref(),
                    );
                    // Where a link falls on *this* segment, clipped to it —
                    // `None` for one that lies entirely on another segment.
                    let seg_len = text.chars().count();
                    let seg_span = |link: &LinkSpan| {
                        let lo = vl.col_offset;
                        let hi = lo + seg_len;
                        let s = link.col_start.clamp(lo, hi) - lo;
                        let e = link.col_end.clamp(lo, hi) - lo;
                        (s < e).then_some((s, e))
                    };
                    if !links.is_empty() {
                        let link_color = pal.link;
                        let name_y0 = origin.y + y;
                        let name_y1 = name_y0 + h;

                        // Find the hovered link (prefer shortest span on overlap)
                        let hovered_link = hover_pos.and_then(|hp| {
                            if hp.y < name_y0 || hp.y >= name_y1 {
                                return None;
                            }
                            let mut best: Option<&LinkSpan> = None;
                            for link in &links {
                                let Some((adj_start, adj_end)) = seg_span(link) else {
                                    continue;
                                };
                                let lx0 = origin.x + LEFT_PAD + atext.x_pos(ui, font_id, adj_start);
                                let lx1 = origin.x + LEFT_PAD + atext.x_pos(ui, font_id, adj_end);
                                if hp.x >= lx0 && hp.x < lx1 {
                                    let span_len = link.col_end - link.col_start;
                                    if best.is_none_or(|b| span_len < b.col_end - b.col_start) {
                                        best = Some(link);
                                    }
                                }
                            }
                            best
                        });

                        if let Some(link) = hovered_link
                            && let Some((adj_start, adj_end)) = seg_span(link)
                        {
                            let lx0 = origin.x + LEFT_PAD + atext.x_pos(ui, font_id, adj_start);
                            let lx1 = origin.x + LEFT_PAD + atext.x_pos(ui, font_id, adj_end);
                            let link_text: String = text
                                .chars()
                                .skip(adj_start)
                                .take(adj_end - adj_start)
                                .collect();
                            painter.text(
                                egui::pos2(lx0, name_y0),
                                egui::Align2::LEFT_TOP,
                                &link_text,
                                font_id.clone(),
                                link_color,
                            );
                            painter.line_segment(
                                [
                                    egui::pos2(lx0, name_y1 - 1.0),
                                    egui::pos2(lx1, name_y1 - 1.0),
                                ],
                                egui::Stroke::new(1.0, link_color),
                            );
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            ui.ctx().request_repaint();

                            if response.clicked() {
                                goto_glyph_name = Some(link.target.clone());
                                goto_glyph_kind = Some(link.kind);
                                goto_is_def = link.is_def;
                                goto_link_pos = Some(Caret::new(vl.doc_line, link.col_start));
                            }
                        }
                    }
                }

                if let Some(cp) = click_pos
                    && cp.y >= origin.y + y
                    && cp.y < origin.y + y + h
                {
                    let rel_x = (cp.x - origin.x - LEFT_PAD).max(0.0);
                    let col = vl.col_offset + atext.x_to_col(ui, font_id, rel_x);
                    click_result = Some(ClickTarget::Text(Caret::new(vl.doc_line, col)));
                }

                // Cursor drawing for text lines
                let text_char_count = text.chars().count();
                if matches!(state.mode, EditMode::Normal)
                    && state.cursor.line == vl.doc_line
                    && state.cursor.col >= vl.col_offset
                    && state.cursor.col <= vl.col_offset + text_char_count
                {
                    let local_col = state.cursor.col - vl.col_offset;
                    let cx = origin.x + LEFT_PAD + atext.x_pos(ui, font_id, local_col);
                    let cy = origin.y + y;

                    // The preedit paints whether or not the canvas is
                    // focused: the Ctrl+K code point popup owns the
                    // keyboard while previewing through it.
                    if !state.preedit.is_empty() {
                        {
                            let preedit_w = ui.fonts(|f| {
                                f.layout_no_wrap(
                                    state.preedit.clone(),
                                    font_id.clone(),
                                    egui::Color32::WHITE,
                                )
                                .rect
                                .width()
                            });
                            let bg_color = cursor_color;
                            let fg_color = ui.visuals().panel_fill;
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(cx, cy),
                                    egui::vec2(preedit_w, h),
                                ),
                                0.0,
                                bg_color,
                            );
                            painter.text(
                                egui::pos2(cx, cy),
                                egui::Align2::LEFT_TOP,
                                &state.preedit,
                                font_id.clone(),
                                fg_color,
                            );
                            // An IME's candidate window follows its
                            // preedit, but the popup that anchors here
                            // must not walk sideways as digits are typed,
                            // so an unfocused canvas keeps the caret.
                            cursor_screen = Some(if has_focus {
                                egui::pos2(cx + preedit_w, cy)
                            } else {
                                egui::pos2(cx, cy)
                            });
                        }
                    } else if has_focus {
                        {
                            painter.line_segment(
                                [egui::pos2(cx, cy), egui::pos2(cx, cy + h)],
                                egui::Stroke::new(2.0, cursor_color),
                            );
                            cursor_screen = Some(egui::pos2(cx, cy));
                        }
                    } else {
                        // No caret is painted without focus, but the caret's
                        // column is still where a popup belongs: the Ctrl+K
                        // popup takes focus away and opens before anything is
                        // typed, so falling back to the start of the line put
                        // it at the left margin until the first digit decoded.
                        cursor_screen = Some(egui::pos2(cx, cy));
                    }

                    // Check if caret is inside an error span
                    for (s, e, msg) in &vl.error_spans {
                        if local_col >= *s && local_col < *e {
                            let span_x = origin.x + LEFT_PAD + atext.x_pos(ui, font_id, *s);
                            error_tooltip = Some((egui::pos2(span_x, cy + h + 2.0), msg.clone()));
                            break;
                        }
                    }
                }
            }
            VLineKind::GridRow {
                item_idx,
                row,
                own_width,
                own_height,
                grid_doc_line,
                extent,
                metrics,
            } => {
                let content_w = extent.display_width(grid_cell);
                let grid_x = strip.grid_x(content_w);
                let grid_y = origin.y + y;

                grid_render::render_grid_row(
                    &grid_painter,
                    grid_x,
                    grid_y,
                    doc,
                    *item_idx,
                    *row,
                    *own_width,
                    *own_height,
                    *extent,
                    metrics.as_ref(),
                    composites.get(item_idx),
                    shadow.filter(|(idx, _)| idx == item_idx).map(|(_, s)| s),
                    &state.mode,
                    grid_cell,
                    pal,
                );

                grid_render::handle_grid_hover_preview(
                    ui,
                    &grid_painter,
                    &strip,
                    &state.mode,
                    *item_idx,
                    *own_width,
                    *own_height,
                    *row,
                    *extent,
                    grid_x,
                    grid_y,
                    grid_cell,
                );

                if inline_panel_origin.is_none()
                    && inline_panel_edit_idx == Some(*item_idx)
                    && *row == extent.top
                {
                    inline_panel_origin = Some((
                        (grid_x + content_w).min(strip.right())
                            + INLINE_PANEL_GAP * zoom_level as f32,
                        grid_y,
                        content_w,
                    ));
                    edit_grid_rect = visible_grid_rect(
                        &strip,
                        grid_x,
                        grid_y,
                        content_w,
                        (extent.bottom - extent.top) as f32 * grid_cell,
                    );
                }

                if let Some(cp) = click_pos
                    && cp.y >= grid_y
                    && cp.y < grid_y + grid_cell
                {
                    let rel_x = cp.x - grid_x;
                    let gc = (rel_x / grid_cell) as i32 + extent.left as i32;
                    if strip.accepts_pointer(cp)
                        && gc >= extent.left as i32
                        && gc < extent.right as i32
                    {
                        click_result = Some(ClickTarget::Grid {
                            item_idx: *item_idx,
                        });
                    } else {
                        click_result = Some(ClickTarget::Text(Caret::new(vl.doc_line, 0)));
                    }
                }

                if !matches!(state.mode, EditMode::PixelSelect { item_idx: eidx } if eidx == *item_idx)
                {
                    pixel_interaction::handle_pixel_painting(
                        ui,
                        lines,
                        state,
                        needs_rederive,
                        *grid_doc_line,
                        *item_idx,
                        *row,
                        *own_width,
                        *own_height,
                        *extent,
                        &strip,
                        grid_x,
                        grid_y,
                        grid_cell,
                    );
                }

                // Pixel selection overlay + interaction
                if let Some(sel) = state
                    .pixel_selection
                    .as_ref()
                    .filter(|s| s.item_idx == *item_idx)
                {
                    grid_render::render_pixel_selection_overlay(
                        &grid_painter,
                        grid_x,
                        grid_y,
                        *row,
                        *extent,
                        grid_cell,
                        sel,
                        pal,
                    );
                }
                pixel_selection::handle_pixel_select_interaction(
                    ui,
                    doc,
                    lines,
                    state,
                    needs_rederive,
                    *grid_doc_line,
                    *item_idx,
                    composites.get(item_idx),
                    *row,
                    *own_width,
                    *own_height,
                    *extent,
                    &strip,
                    grid_x,
                    grid_y,
                    grid_cell,
                );

                // Grid caret. Drawn once, from the first visible row of the
                // grid — the top row may be culled by the scroll position, and
                // the border spans the whole glyph regardless of which row
                // computes it (`grid_y` is this row's y, `row` rows below the
                // glyph's own row 0).
                if matches!(state.mode, EditMode::Normal)
                    && state.cursor.line == *grid_doc_line
                    && !grid_caret_drawn
                    && has_focus
                {
                    grid_caret_drawn = true;
                    let own_x = grid_x + (-extent.left) as f32 * grid_cell;
                    let own_y = grid_y - *row as f32 * grid_cell;
                    let border_rect = egui::Rect::from_min_size(
                        egui::pos2(own_x, own_y),
                        egui::vec2(
                            *own_width as f32 * grid_cell,
                            *own_height as f32 * grid_cell,
                        ),
                    );
                    grid_painter.rect_stroke(
                        border_rect,
                        0.0,
                        egui::Stroke::new(2.0, pal.grid_border),
                        egui::epaint::StrokeKind::Outside,
                    );
                    cursor_screen = Some(egui::pos2(own_x, own_y));
                }
            }
        }

        if !edit_border_drawn
            && let Some(border_rect) = draw_edit_border(
                &grid_painter,
                &state.mode,
                vl,
                doc,
                origin,
                y,
                composites,
                &strip,
                grid_cell,
                pal,
            )
        {
            edit_border_drawn = true;
            resize_rect = matches!(state.mode, EditMode::GlyphResize { .. }).then_some(border_rect);
            #[cfg(test)]
            crate::editor::harness::capture_edit_border(ui.ctx(), state.id(), border_rect);
            #[cfg(not(test))]
            let _ = border_rect;
        }

        y += h;
    }

    draw_grid_hscrollbars(
        ui, &painter, state, &strip, &blocks, &hbars, zoom_level, pal,
    );
    auto_scroll_grid_on_drag(ui, state, &strip, &blocks, origin, zoom_level);

    // Inline tools panel to the right of the grid. A resize replaces it
    // wholesale — neither the layer row nor the shape palette acts on
    // anything while the glyph's own box is what is being edited.
    if let (Some(_), Some((panel_x, panel_y, _))) = (inline_panel_edit_idx, inline_panel_origin)
        && matches!(state.mode, EditMode::GlyphResize { .. })
    {
        let (action, consumed) = crate::editor::glyph_resize::draw_panel(
            ui,
            &painter,
            panel_x,
            panel_y,
            state,
            click_pos,
            zoom_level as f32,
        );
        if consumed {
            click_result = None;
        }
        match action {
            Some(crate::editor::glyph_resize::PanelAction::Apply) => {
                state.pending_resize = crate::editor::glyph_resize::finish(doc, lines, state);
                *needs_rederive = true;
            }
            Some(crate::editor::glyph_resize::PanelAction::Cancel) => {
                *needs_rederive |= crate::editor::glyph_resize::cancel(lines, state);
            }
            None => {}
        }
    } else if let (Some(edit_idx), Some((panel_x, panel_y, _grid_w))) =
        (inline_panel_edit_idx, inline_panel_origin)
    {
        let panel_result = inline_tools::draw_inline_tools_panel(
            ui,
            &painter,
            panel_x,
            panel_y,
            doc,
            state,
            edit_idx,
            composites,
            named_glyphs,
            name_parts,
            shadow.filter(|(idx, _)| *idx == edit_idx).map(|(_, s)| s),
            click_pos,
            zoom_level,
        );
        if panel_result.click_consumed {
            click_result = None;
        }
        if let Some((ref_idx, action)) = panel_result.inline_ref {
            refocus_after_menu(ui, wid);
            if apply_inline_action(
                action,
                lines,
                doc,
                state,
                edit_idx,
                ref_idx,
                composites.get(&edit_idx),
                named_glyphs,
                name_parts,
            ) {
                *needs_rederive = true;
            }
        }
    }

    // Resize overlay and drag: the boundary is painted here, after every grid
    // row, and one of its edges follows the pointer.
    if matches!(state.mode, EditMode::GlyphResize { .. })
        && let Some(rect) = resize_rect
    {
        crate::editor::glyph_resize::draw_overlay(&grid_painter, rect, zoom_level as f32, pal);
        crate::editor::glyph_resize::handle_drag(
            ui,
            lines,
            state,
            needs_rederive,
            rect,
            grid_cell,
            zoom_level as f32,
        );
    }

    // Layer move drag handling (refs and points)
    if let EditMode::LayerMove {
        item_idx: eidx,
        layer_idx,
    } = state.mode
    {
        pixel_interaction::handle_layer_drag(
            ui,
            lines,
            state,
            needs_rederive,
            doc,
            eidx,
            layer_idx,
            &doc.item_line_starts,
            composites.get(&eidx),
            grid_cell,
        );
    }

    // Wheel scroll on grid: change subpixel shape or layer
    if inline_panel_edit_idx.is_none() {
        state.grid_hover = false;
    }
    if let (Some(edit_idx), Some(grid_rect)) = (inline_panel_edit_idx, edit_grid_rect) {
        let body = match doc.items.get(edit_idx) {
            Some(DocumentItem::Glyph { body, .. }) => Some(body),
            _ => None,
        };
        if let Some(body) = body {
            let on_grid = ui.input(|i| {
                i.pointer
                    .hover_pos()
                    .is_some_and(|hp| grid_rect.contains(hp))
            });
            state.grid_hover = on_grid;

            {
                let ctrl_held = ui.input(|i| i.modifiers.command);
                let shift_held = ui.input(|i| i.modifiers.shift);
                if let Some(step) =
                    interceptor_scroll_step(ui.ctx(), state.id(), on_grid).filter(|_| {
                        // A resize owns the glyph; neither layer cycling nor
                        // the shape palette may switch the mode under it.
                        !matches!(state.mode, EditMode::GlyphResize { .. })
                    })
                {
                    if ctrl_held {
                        // Ctrl+wheel on grid: cycle layers (same as layer palette)
                        let inherited_count = composites
                            .get(&edit_idx)
                            .map_or(0, |c| c.inherited_anchors.len());
                        crate::editor::inline_tools::cycle_layer_mode(
                            state,
                            body,
                            edit_idx,
                            inherited_count,
                            step,
                        );
                    } else if matches!(state.mode, EditMode::GlyphEdit { item_idx, .. } if item_idx == edit_idx)
                    {
                        // Wheel on grid in pixel layer: rotate the palette,
                        // or with shift held pick another shape from it —
                        // exactly as over the palette itself.
                        if let EditMode::GlyphEdit { selected_shape, .. } = &mut state.mode {
                            crate::editor::glyph_widget::wheel_step_shape(
                                selected_shape,
                                &mut state.shape_rotation,
                                step,
                                shift_held,
                            );
                        }
                    }
                }
            }
        }
    }

    // Ctrl/Cmd+click goto
    if let Some(ref target_name) = goto_glyph_name {
        let kind = goto_glyph_kind.unwrap_or(LinkTargetKind::Glyph);
        // The link's own position, so "go back" returns to the reference
        // rather than to the untouched caret.
        let from = goto_link_pos.unwrap_or(state.cursor);
        // A declaration would "navigate" to the line the click was already
        // on, so it never looks for one — it asks for the search instead.
        let local = (!goto_is_def)
            .then(|| doc_links::find_link_target_in_doc(lines, target_name, &kind))
            .flatten();
        let target = if let Some(line_idx) = local {
            state.mode = EditMode::Normal;
            state.selection_anchor = None;
            state.cursor = Caret::new(line_idx, 0);
            let target_y = doc_line_to_y(vlines, row_height, grid_cell, line_idx);
            let centered = (target_y - viewport_h / 3.0).max(0.0);
            ui.ctx().data_mut(|d| {
                d.insert_temp(state.key(Slot::ScrollTarget), centered);
            });
            NavTarget::Local { line: line_idx }
        } else {
            let goto = GotoGlyph {
                name: target_name.clone(),
                kind,
            };
            if goto_is_def {
                NavTarget::Search(goto)
            } else {
                NavTarget::CrossFile(goto)
            }
        };
        state.pending_nav = Some(NavRequest { from, target });
    }

    // Process click.  A click on the canvas while the rename popup is
    // open cancels it (the popup's field loses focus), and the caret
    // moves to where the user clicked — so the click is *not* swallowed.
    if goto_glyph_name.is_none()
        && let Some(target) = click_result
    {
        state.autocomplete = None;
        let shift = ui.input(|i| i.modifiers.shift);
        match target {
            ClickTarget::Text(caret_pos) => {
                if !matches!(state.mode, EditMode::Normal) {
                    state.mode = EditMode::Normal;
                }
                let caret_pos = caret::clamp(lines, caret_pos);
                if is_triple {
                    state.selection_anchor = Some(Caret::new(caret_pos.line, 0));
                    state.cursor =
                        Caret::new(caret_pos.line, caret::line_char_len(lines, caret_pos.line));
                } else if is_double {
                    let (lo, hi) = caret::word_bounds_at(lines, caret_pos);
                    state.selection_anchor = Some(lo);
                    state.cursor = hi;
                } else if !shift && !response.dragged() {
                    state.selection_anchor = None;
                    state.cursor = caret_pos;
                } else if !shift && response.drag_started() {
                    state.selection_anchor = Some(caret_pos);
                    state.cursor = caret_pos;
                } else {
                    if state.selection_anchor.is_none() {
                        state.selection_anchor = Some(state.cursor);
                    }
                    state.cursor = caret_pos;
                }
            }
            ClickTarget::Grid { item_idx } => {
                state.selection_anchor = None;
                if matches!(state.mode, EditMode::GlyphResize { .. }) {
                    // The glyph is being resized; a press on it grabbed an
                    // edge, not a pixel.
                } else if !matches!(
                    state.mode,
                    EditMode::GlyphEdit { item_idx: eidx, .. } if eidx == item_idx
                ) && !matches!(
                    state.mode,
                    EditMode::PixelSelect { item_idx: eidx } if eidx == item_idx
                ) && !matches!(
                    state.mode,
                    EditMode::LayerMove { item_idx: eidx, .. } if eidx == item_idx
                ) {
                    let has_pixels = matches!(
                        doc.items.get(item_idx),
                        Some(DocumentItem::Glyph { body, .. }) if body.pixels.is_some()
                    );
                    if has_pixels {
                        state.mode = EditMode::GlyphEdit {
                            item_idx,
                            selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
                        };
                        state.suppress_grid_click = true;
                    } else if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx)
                        && !body.refs.is_empty()
                    {
                        state.mode = EditMode::GlyphEdit {
                            item_idx,
                            selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
                        };
                        state.suppress_grid_click = true;
                    }
                }
            }
        }
    }

    // IME
    if has_focus && let Some(cpos) = cursor_screen {
        let ime_rect = egui::Rect::from_min_size(cpos, egui::vec2(2.0, row_height));
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                rect: ime_rect,
                cursor_rect: ime_rect,
            });
        });
    }

    // Store cursor screen position for popup use
    if let Some(cpos) = cursor_screen {
        ui.ctx().data_mut(|d| {
            d.insert_temp(state.key(Slot::CursorScreenPos), cpos);
            d.insert_temp(state.key(Slot::CursorRowHeight), row_height);
        });
    }

    // Store error tooltip for display outside scroll area
    ui.ctx().data_mut(|d| {
        d.insert_temp(state.key(Slot::ErrorTooltipData), error_tooltip);
    });

    // Right-clicking the grid while a ref layer is selected offers the same
    // subglyph menu as right-clicking that layer's thumbnail in the inline
    // tools panel. Whether the click landed on the grid has to be latched:
    // `context_menu` is re-evaluated every frame while the menu is open, and
    // by then the pointer sits on the menu itself.
    let grid_ctx_id = state.key(Slot::GridSubglyphCtxOnGrid);
    if response.secondary_clicked() {
        let on_grid = response
            .interact_pointer_pos()
            .zip(edit_grid_rect)
            .is_some_and(|(p, r)| r.contains(p));
        ui.ctx().data_mut(|d| d.insert_temp(grid_ctx_id, on_grid));
    }
    let grid_subglyph_ref = match state.mode {
        EditMode::LayerMove {
            item_idx,
            layer_idx,
        } if ui
            .ctx()
            .data(|d| d.get_temp::<bool>(grid_ctx_id).unwrap_or(false)) =>
        {
            matches!(doc.items.get(item_idx),
                    Some(DocumentItem::Glyph { body, .. }) if layer_idx < body.refs.len())
            .then_some((item_idx, layer_idx))
        }
        _ => None,
    };

    // Context menu (only in Normal mode; edit modes use right-click for erasing)
    let ctx_mode_normal = matches!(state.mode, EditMode::Normal);
    if let Some((edit_idx, ref_idx)) = grid_subglyph_ref {
        let mut inline = None;
        response.context_menu(|ui| {
            inline = inline_tools::subglyph_context_menu(ui);
        });
        if let Some(action) = inline {
            refocus_after_menu(ui, wid);
            if apply_inline_action(
                action,
                lines,
                doc,
                state,
                edit_idx,
                ref_idx,
                composites.get(&edit_idx),
                named_glyphs,
                name_parts,
            ) {
                *needs_rederive = true;
            }
        }
    } else if ctx_mode_normal {
        let mut acted = false;
        response.context_menu(|ui| {
            let caps = crate::edit_menu::EditMenuCaps {
                can_undo: state.undo.can_undo(),
                can_redo: state.undo.can_redo(),
                has_selection: state.selection_range().is_some(),
                can_edit: ctx_mode_normal,
            };
            let action = crate::edit_menu::show_edit_menu_items(ui, &caps, false);
            acted |= action != crate::edit_menu::EditAction::None;
            if apply_edit_action_to_editor(action, doc, lines, state, ui.ctx()) {
                *needs_rederive = true;
            }
        });
        if acted {
            refocus_after_menu(ui, wid);
        }
    }
}

/// Clicking a context-menu item hands egui's keyboard focus to that menu
/// button, and the menu is gone by the next frame — so the editor is left with
/// no focus and swallows the very next keystroke. Any menu item that acts on
/// this editor has to take focus back.
fn refocus_after_menu(ui: &egui::Ui, wid: egui::Id) {
    ui.memory_mut(|m| m.request_focus(wid));
}

#[allow(clippy::too_many_arguments)]
fn draw_selection(
    painter: &egui::Painter,
    grid_painter: &egui::Painter,
    ui: &egui::Ui,
    font_id: &egui::FontId,
    vl: &VisualLine,
    origin: egui::Pos2,
    y: f32,
    h: f32,
    sel_lo: Caret,
    sel_hi: Caret,
    strip: &GridStrip,
    grid_cell: f32,
) {
    let dl = vl.doc_line;
    if dl < sel_lo.line || dl > sel_hi.line {
        return;
    }
    let sel_color = Palette::get(ui).selection;
    match &vl.kind {
        VLineKind::Text(text) => {
            let seg_len = text.chars().count();
            let seg_start = vl.col_offset;
            let seg_end = seg_start + seg_len;
            let doc_lo = if dl == sel_lo.line { sel_lo.col } else { 0 };
            let doc_hi = if dl == sel_hi.line {
                sel_hi.col
            } else {
                seg_end
            };
            let col_lo = doc_lo.max(seg_start).saturating_sub(seg_start).min(seg_len);
            let col_hi = doc_hi.max(seg_start).saturating_sub(seg_start).min(seg_len);
            if col_lo >= col_hi {
                return;
            }
            let atext = AnnotatedText::new(text, &vl.annotations);
            let x0 = atext.x_pos(ui, font_id, col_lo);
            let x1 = atext.x_pos(ui, font_id, col_hi);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(origin.x + LEFT_PAD + x0, origin.y + y),
                    egui::pos2(origin.x + LEFT_PAD + x1, origin.y + y + h),
                ),
                0.0,
                sel_color,
            );
        }
        VLineKind::GridRow { extent, .. } => {
            let content_w = extent.display_width(grid_cell);
            let gx = strip.grid_x(content_w);
            if let Some((x0, x1)) = strip.clip_span(gx, gx + content_w) {
                grid_painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, origin.y + y),
                        egui::pos2(x1, origin.y + y + h),
                    ),
                    0.0,
                    sel_color,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_edit_border(
    painter: &egui::Painter,
    mode: &EditMode,
    vl: &VisualLine,
    _doc: &Document,
    origin: egui::Pos2,
    y: f32,
    _composites: &HashMap<usize, GlyphComposite>,
    strip: &GridStrip,
    grid_cell: f32,
    pal: &Palette,
) -> Option<egui::Rect> {
    let editing_idx = match mode {
        EditMode::GlyphEdit { item_idx, .. } => Some(*item_idx),
        EditMode::LayerMove { item_idx, .. } => Some(*item_idx),
        EditMode::GlyphResize { item_idx } => Some(*item_idx),
        _ => return None,
    };
    let eidx = editing_idx?;

    match &vl.kind {
        VLineKind::GridRow {
            item_idx,
            row,
            own_width,
            own_height,
            extent,
            ..
        } if *item_idx == eidx => {
            let own_x =
                strip.grid_x(extent.display_width(grid_cell)) + (-extent.left) as f32 * grid_cell;
            // `y` belongs to whichever row of the item is being painted (the
            // first visible one — the caller draws once per frame); the
            // border top is the glyph's own row 0, `row` rows above it.
            let border_rect = egui::Rect::from_min_size(
                egui::pos2(own_x, origin.y + y - *row as f32 * grid_cell),
                egui::vec2(
                    *own_width as f32 * grid_cell,
                    *own_height as f32 * grid_cell,
                ),
            );
            // While resizing, this rectangle *is* the thing being dragged, and
            // its overlay is drawn inside the box — so it cannot go out from
            // here, in the middle of the glyph's rows, or the rows below this
            // one would paint straight over it. The caller draws it once every
            // row is down; all this does is work out where.
            if !matches!(mode, EditMode::GlyphResize { .. }) {
                painter.rect_stroke(
                    border_rect,
                    0.0,
                    egui::Stroke::new(2.0, pal.cursor_border),
                    egui::epaint::StrokeKind::Outside,
                );
            }
            Some(border_rect)
        }
        _ => None,
    }
}

fn resolve_color_for_display(token: &str, aliases: &ColorAliasMap) -> Option<egui::Color32> {
    if token == "fg" {
        return None;
    }
    if token.starts_with('#') {
        let rgba = crate::render::ttf_builder::parse_hex_color(token)?;
        return Some(egui::Color32::from_rgba_unmultiplied(
            rgba.r, rgba.g, rgba.b, rgba.a,
        ));
    }
    let (rgba, _) = aliases.get(token)?;
    Some(egui::Color32::from_rgba_unmultiplied(
        rgba.r, rgba.g, rgba.b, rgba.a,
    ))
}

fn contrast_text_color(bg: egui::Color32) -> egui::Color32 {
    let [r, g, b, _] = bg.to_array();
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 128.0 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

/// The whole document line a visual line came from — a soft wrap makes the
/// visual line's own text a *segment* of it, and anything that parses the line
/// (links, color tokens) has to read all of it: half a line classifies as
/// different fields entirely. Falls back to the segment when the line is not
/// text, which cannot happen for a `VLineKind::Text`.
fn doc_line_text<'a>(lines: &'a [DocLine], vl: &VisualLine, segment: &'a str) -> &'a str {
    match lines.get(vl.doc_line) {
        Some(DocLine::Text(s)) => s.as_str(),
        _ => segment,
    }
}

/// Paints the color swatches `line` calls for onto the segment `atext` draws,
/// and returns the spans it painted in absolute document columns. A swatch
/// whose token the wrap put on another segment belongs to that segment.
#[allow(clippy::too_many_arguments)]
fn paint_color_backgrounds(
    painter: &egui::Painter,
    ui: &egui::Ui,
    font_id: &egui::FontId,
    atext: &AnnotatedText<'_>,
    line: &str,
    col_offset: usize,
    base_x: f32,
    base_y: f32,
    row_h: f32,
    aliases: &ColorAliasMap,
) -> Vec<(usize, usize)> {
    let text = atext.text();
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();
    let spans = match tokenize_with_spans(trimmed) {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };
    let keyword = spans[0].value.as_str();
    let rest = &spans[1..];

    let mut color_spans: Vec<(usize, usize, egui::Color32)> = Vec::new();

    match keyword {
        "color" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                let val_span = &rest[2];
                if let Some(color) = resolve_color_for_display(&val_span.value, aliases) {
                    color_spans.push((
                        leading + val_span.raw_start,
                        leading + val_span.raw_end,
                        color,
                    ));
                }
            }
        }
        "ref" => {
            if let Some(fill_pos) = rest.iter().position(|s| s.value == "fill")
                && let Some(color_span) = rest.get(fill_pos + 1)
                && let Some(color) = resolve_color_for_display(&color_span.value, aliases)
            {
                color_spans.push((
                    leading + color_span.raw_start,
                    leading + color_span.raw_end,
                    color,
                ));
            }
        }
        _ => {}
    }

    let seg_len = text.chars().count();
    let mut painted = Vec::new();
    for (col_start, col_end, bg_color) in &color_spans {
        // Clipped to this segment: a token the wrap put wholly on another one
        // has nothing to draw here.
        let adj_start = (*col_start).clamp(col_offset, col_offset + seg_len) - col_offset;
        let adj_end = (*col_end).clamp(col_offset, col_offset + seg_len) - col_offset;
        if adj_start >= adj_end {
            continue;
        }
        painted.push((col_offset + adj_start, col_offset + adj_end));
        let x0 = base_x + atext.x_pos(ui, font_id, adj_start);
        let x1 = base_x + atext.x_pos(ui, font_id, adj_end);
        let rect = egui::Rect::from_min_size(egui::pos2(x0, base_y), egui::vec2(x1 - x0, row_h));
        painter.rect_filled(rect, 0.0, *bg_color);
        let token_text: String = text
            .chars()
            .skip(adj_start)
            .take(adj_end - adj_start)
            .collect();
        let fg = contrast_text_color(*bg_color);
        painter.text(
            egui::pos2(x0, base_y),
            egui::Align2::LEFT_TOP,
            &token_text,
            font_id.clone(),
            fg,
        );
    }
    painted
}
