//! Painting the selection highlight and the edit-mode border around a pixel grid.

use super::super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_selection(
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
        // Nothing of the strip is selectable: it is not text, and the line it
        // introduces paints its own selection one row down.
        VLineKind::RefImage { .. } => {}
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
pub(super) fn draw_edit_border(
    painter: &egui::Painter,
    mode: &EditMode,
    box_drag: bool,
    vl: &VisualLine,
    _doc: &Document,
    origin: egui::Pos2,
    y: f32,
    _composites: &crate::editor::grid_render::Composites,
    strip: &GridStrip,
    grid_cell: f32,
    pal: &Palette,
) -> Option<egui::Rect> {
    let editing_idx = match mode {
        EditMode::GlyphEdit { item_idx, .. } => Some(*item_idx),
        EditMode::LayerMove { item_idx, .. } => Some(*item_idx),
        EditMode::GlyphResize { item_idx } => Some(*item_idx),
        // Not an editing border: the caller wants the grid's rectangle so the
        // canvas can be dragged from under the backreference shadow, which is
        // where a canvas resize starts now that `F2` drags the box.
        EditMode::PixelSelect {
            item_idx,
            backrefs: true,
        } => Some(*item_idx),
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
            metrics,
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
            if !matches!(
                mode,
                EditMode::GlyphResize { .. } | EditMode::PixelSelect { .. }
            ) {
                painter.rect_stroke(
                    border_rect,
                    0.0,
                    egui::Stroke::new(2.0, pal.cursor_border),
                    egui::epaint::StrokeKind::Outside,
                );
                return Some(border_rect);
            }
            if matches!(mode, EditMode::PixelSelect { .. }) {
                return Some(border_rect);
            }
            // A box drag grabs the *metric box*, which is the rectangle it
            // moves; a canvas drag grabs the grid. The two coincide for a
            // glyph that declares nothing, which is why only one of them was
            // ever needed before.
            if box_drag && let Some(m) = metrics {
                let gx = |c: i16| own_x + c as f32 * grid_cell;
                let gy = |r: i16| origin.y + y + (r - *row) as f32 * grid_cell;
                return Some(egui::Rect::from_min_max(
                    egui::pos2(gx(m.left), gy(m.top)),
                    egui::pos2(gx(m.right), gy(m.bottom)),
                ));
            }
            Some(border_rect)
        }
        _ => None,
    }
}
