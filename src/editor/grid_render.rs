use std::collections::HashMap;

use crate::document::{Document, DocumentItem, NamePartsMap, PixelGrid};
use crate::editor::EditMode;
use crate::editor::colors::Palette;
use crate::editor::document_view::{GridExtent, UNFILLED_OPACITY};
use crate::editor::glyph_widget;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};

#[allow(clippy::too_many_arguments)]
pub(crate) fn blit_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    grid: &PixelGrid,
    off_r: i16,
    off_c: i16,
    color: egui::Color32,
    cell_w: f32,
    cell_h: f32,
) {
    for r in 0..grid.height {
        for c in 0..grid.width {
            if grid.get(r, c).is_filled() {
                let dr = off_r as f32 + r as f32;
                let dc = off_c as f32 + c as f32;
                if dr >= 0.0 && dc >= 0.0 {
                    let px_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + dc * cell_w, rect.min.y + dr * cell_h),
                        egui::vec2(cell_w, cell_h),
                    );
                    painter.rect_filled(px_rect, 0.0, color);
                }
            }
        }
    }
}

pub(crate) fn apply_opacity(color: egui::Color32, opacity: f32) -> egui::Color32 {
    let [r, g, b, _] = color.to_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, (255.0 * opacity) as u8)
}

pub(crate) fn build_composites(
    doc: &Document,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &ref_composite::AlternativesIndex,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> HashMap<usize, GlyphComposite> {
    let mut composites = HashMap::new();
    for (idx, item) in doc.items.iter().enumerate() {
        if let DocumentItem::Glyph { body, .. } = item
            && let Some(comp) = ref_composite::compute_composite(body, named_glyphs, name_parts, alt_index, color_aliases)
        {
            composites.insert(idx, comp);
        }
    }
    composites
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_grid_row(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    doc: &Document,
    item_idx: usize,
    row: i16,
    own_width: u16,
    own_height: u16,
    extent: GridExtent,
    composite: Option<&GlyphComposite>,
    mode: &EditMode,
    grid_cell: f32,
    pal: &Palette,
) {
    let grid = match doc.items.get(item_idx) {
        Some(DocumentItem::Glyph { body, .. }) => body.pixels.as_ref(),
        _ => None,
    };

    let cs = grid_cell;
    let display_w = extent.display_width(grid_cell);
    let in_own_row = row >= 0 && row < own_height as i16;

    painter.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(display_w, cs)),
        0.0,
        pal.grid_bg,
    );

    let is_layer_mode =
        matches!(mode, EditMode::LayerMove { item_idx: eidx, .. } if *eidx == item_idx);
    let active_ref = match mode {
        EditMode::LayerMove {
            item_idx: eidx,
            layer_idx,
        } if *eidx == item_idx => {
            if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) {
                if *layer_idx < body.refs.len() {
                    Some(*layer_idx)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };
    let active_point = match mode {
        EditMode::LayerMove {
            item_idx: eidx,
            layer_idx,
        } if *eidx == item_idx => {
            if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) {
                if *layer_idx >= body.refs.len() {
                    Some(*layer_idx - body.refs.len())
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(comp) = composite {
        for layer in &comp.layers {
            if Some(layer.ref_idx) == active_ref {
                continue;
            }
            let color = layer.fill_color.unwrap_or_else(||
                ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx)
            );
            let opacity = if is_layer_mode { 0.35 } else { 1.0 };
            let color = if opacity < 1.0 {
                apply_opacity(color, opacity)
            } else {
                color
            };

            let lr_in_layer = comp.own_offset_row + row - layer.offset_row;
            if lr_in_layer >= 0 && lr_in_layer < layer.grid.height as i16 {
                for dc in extent.left..extent.right {
                    let lc_in_layer = comp.own_offset_col + dc - layer.offset_col;
                    if lc_in_layer < 0 || lc_in_layer >= layer.grid.width as i16 {
                        continue;
                    }
                    let shape = layer.grid.get(lr_in_layer as u16, lc_in_layer as u16);
                    if !shape.is_empty() {
                        let cell_rect = egui::Rect::from_min_size(
                            egui::pos2(x + (dc - extent.left) as f32 * cs, y),
                            egui::vec2(cs, cs),
                        );
                        if layer.negated {
                            glyph_widget::draw_pixel_cell_colored(
                                painter,
                                cell_rect,
                                shape,
                                Some(pal.grid_bg),
                            );
                        } else {
                            let px_color = if shape.is_filled() {
                                color
                            } else {
                                apply_opacity(color, UNFILLED_OPACITY)
                            };
                            glyph_widget::draw_pixel_cell_colored(
                                painter,
                                cell_rect,
                                shape,
                                Some(px_color),
                            );
                        }
                    }
                }
            }
        }

        let disp_row = (row - extent.top) as u16;
        let disp_w = (extent.right - extent.left) as u16;
        draw_ref_bounding_boxes_offset(
            painter,
            x,
            y,
            disp_row,
            disp_w,
            comp,
            cs,
            comp.own_offset_row + extent.top,
            comp.own_offset_col + extent.left,
            pal,
        );
    }

    let own_opacity = if is_layer_mode { 0.35 } else { 1.0 };
    let has_ref_pixel = |dc: i16| -> bool {
        composite.is_some_and(|comp| {
            let mut visible = false;
            for layer in &comp.layers {
                let lr = comp.own_offset_row + row - layer.offset_row;
                let lc = comp.own_offset_col + dc - layer.offset_col;
                if lr >= 0
                    && lr < layer.grid.height as i16
                    && lc >= 0
                    && lc < layer.grid.width as i16
                {
                    let shape = layer.grid.get(lr as u16, lc as u16);
                    if !shape.is_empty() {
                        if layer.negated {
                            if shape.shape_id() == 0 {
                                visible = false;
                            }
                        } else {
                            visible = true;
                        }
                    }
                }
            }
            visible
        })
    };
    for dc in extent.left..extent.right {
        let disp_c = (dc - extent.left) as f32;
        let cell_rect =
            egui::Rect::from_min_size(egui::pos2(x + disp_c * cs, y), egui::vec2(cs, cs));
        let in_own_col = dc >= 0 && dc < own_width as i16;
        let in_own = in_own_row && in_own_col;

        if in_own {
            let own_shape = grid.map(|g| g.get(row as u16, dc as u16));
            let is_occupied = own_shape.is_some_and(|s| !s.is_empty());
            if is_occupied {
                let shape = own_shape.unwrap();
                let base_color = pal.pixel_filled;
                let mut opacity = own_opacity;
                if !shape.is_filled() {
                    opacity *= UNFILLED_OPACITY;
                }
                let color = if opacity < 1.0 {
                    apply_opacity(base_color, opacity)
                } else {
                    base_color
                };
                glyph_widget::draw_pixel_cell_colored(painter, cell_rect, shape, Some(color));
            } else if !has_ref_pixel(dc) {
                painter.rect_filled(cell_rect, 0.0, pal.grid_off);
            }
        } else if !has_ref_pixel(dc) {
            painter.rect_filled(cell_rect, 0.0, pal.grid_ext_off);
        }
    }

    if let Some(active) = active_ref
        && let Some(comp) = composite
        && let Some(layer) = comp.layers.iter().find(|l| l.ref_idx == active)
    {
        let color = layer.fill_color.unwrap_or_else(||
            ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx)
        );
        for dc in extent.left..extent.right {
            let lr_in_layer = comp.own_offset_row + row - layer.offset_row;
            let lc_in_layer = comp.own_offset_col + dc - layer.offset_col;
            if lr_in_layer >= 0
                && lr_in_layer < layer.grid.height as i16
                && lc_in_layer >= 0
                && lc_in_layer < layer.grid.width as i16
            {
                let shape = layer.grid.get(lr_in_layer as u16, lc_in_layer as u16);
                if !shape.is_empty() {
                    let cell_rect = egui::Rect::from_min_size(
                        egui::pos2(x + (dc - extent.left) as f32 * cs, y),
                        egui::vec2(cs, cs),
                    );
                    let px_color = if shape.is_filled() {
                        color
                    } else {
                        apply_opacity(color, UNFILLED_OPACITY)
                    };
                    glyph_widget::draw_pixel_cell_colored(
                        painter,
                        cell_rect,
                        shape,
                        Some(px_color),
                    );
                }
            }
        }
    }

    // Draw anchor markers
    if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) {
        let num_refs = body.refs.len();
        for (pi, point) in body.points.iter().enumerate() {
            if row < point.row || row > point.row_end {
                continue;
            }
            if point.col_end < extent.left || point.col >= extent.right {
                continue;
            }
            let layer_idx = num_refs + pi;
            let is_active_point = active_point == Some(pi);
            let opacity = if is_layer_mode && !is_active_point {
                0.35
            } else {
                1.0
            };
            let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer_idx);
            let color = if opacity < 1.0 {
                apply_opacity(color, opacity)
            } else {
                color
            };
            if point.is_single_cell() {
                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(x + (point.col - extent.left) as f32 * cs, y),
                    egui::vec2(cs, cs),
                );
                draw_point_x_mark(painter, cell_rect, color, pal.grid_bg);
            } else {
                let anchor_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        x + (point.col - extent.left) as f32 * cs,
                        y + (point.row - row) as f32 * cs,
                    ),
                    egui::vec2(point.width() as f32 * cs, point.height() as f32 * cs),
                );
                draw_anchor_region(painter, anchor_rect, row, point, cs, color, pal.grid_bg);
            }
        }
    }

    draw_grid_lines_with_extent(
        painter, x, y, row, own_width, own_height, extent, grid, grid_cell, pal,
    );
}

pub(crate) fn draw_point_x_mark(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill_color: egui::Color32,
    border_color: egui::Color32,
) {
    let stroke_w = (rect.width() * 0.15).max(1.5);
    let border_w = stroke_w + 2.0;
    let m = rect.width() * 0.1;
    let tl = egui::pos2(rect.min.x + m, rect.min.y + m);
    let tr = egui::pos2(rect.max.x - m, rect.min.y + m);
    let bl = egui::pos2(rect.min.x + m, rect.max.y - m);
    let br = egui::pos2(rect.max.x - m, rect.max.y - m);
    painter.line_segment([tl, br], egui::Stroke::new(border_w, border_color));
    painter.line_segment([tr, bl], egui::Stroke::new(border_w, border_color));
    painter.line_segment([tl, br], egui::Stroke::new(stroke_w, fill_color));
    painter.line_segment([tr, bl], egui::Stroke::new(stroke_w, fill_color));
}

/// Draw a multi-cell anchor mark. The shape is an inset rectangle at
/// `(x1+0.5, y1+0.5)–(x2-0.5, y2-0.5)` in cell coordinates, plus four
/// diagonal line segments from each outer corner to the corresponding
/// inner corner. When width or height is 1 the rectangle collapses to a
/// line; when both are 1 it collapses to a point and the diagonals form
/// the same X mark as `draw_point_x_mark`.
fn draw_anchor_region(
    painter: &egui::Painter,
    full_rect: egui::Rect,
    _current_row: i16,
    _point: &crate::document::GlyphPoint,
    cs: f32,
    fill_color: egui::Color32,
    border_color: egui::Color32,
) {
    let stroke_w = (cs * 0.15).max(1.5);
    let border_w = stroke_w + 2.0;
    let m = cs * 0.1;
    let half = cs * 0.5;

    // Outer corners (same inset as the single-cell X mark).
    let otl = egui::pos2(full_rect.min.x + m, full_rect.min.y + m);
    let otr = egui::pos2(full_rect.max.x - m, full_rect.min.y + m);
    let obl = egui::pos2(full_rect.min.x + m, full_rect.max.y - m);
    let obr = egui::pos2(full_rect.max.x - m, full_rect.max.y - m);

    // Inner corners — half a cell inward from each edge.
    let itl = egui::pos2(full_rect.min.x + half, full_rect.min.y + half);
    let itr = egui::pos2(full_rect.max.x - half, full_rect.min.y + half);
    let ibl = egui::pos2(full_rect.min.x + half, full_rect.max.y - half);
    let ibr = egui::pos2(full_rect.max.x - half, full_rect.max.y - half);

    // Inner rectangle (collapses to line or point when dimension is 1).
    let inner_segments = [[itl, itr], [itr, ibr], [ibr, ibl], [ibl, itl]];
    // Diagonal stubs from outer corners to inner corners.
    let diag_segments = [[otl, itl], [otr, itr], [obl, ibl], [obr, ibr]];

    for segs in [&inner_segments[..], &diag_segments[..]] {
        for [a, b] in segs {
            if (a.x - b.x).abs() > 0.5 || (a.y - b.y).abs() > 0.5 {
                painter.line_segment([*a, *b], egui::Stroke::new(border_w, border_color));
            }
        }
    }
    for segs in [&inner_segments[..], &diag_segments[..]] {
        for [a, b] in segs {
            if (a.x - b.x).abs() > 0.5 || (a.y - b.y).abs() > 0.5 {
                painter.line_segment([*a, *b], egui::Stroke::new(stroke_w, fill_color));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ref_bounding_boxes_offset(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    display_row: u16,
    grid_width: u16,
    comp: &GlyphComposite,
    cs: f32,
    row_offset: i16,
    col_offset: i16,
    pal: &Palette,
) {
    for layer in &comp.layers {
        let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx);
        let border_color = apply_opacity(color, 0.6);

        let box_top = layer.offset_row - row_offset;
        let box_bottom = layer.offset_row + layer.grid.height as i16 - row_offset;
        let box_left = layer.offset_col - col_offset;
        let box_right = layer.offset_col + layer.grid.width as i16 - col_offset;
        let row = display_row as i16;

        if row == box_top {
            let c0 = (box_left.max(0) as u16).min(grid_width);
            let c1 = (box_right.max(0) as u16).min(grid_width);
            if c0 < c1 {
                painter.line_segment(
                    [
                        egui::pos2(x + c0 as f32 * cs, y),
                        egui::pos2(x + c1 as f32 * cs, y),
                    ],
                    egui::Stroke::new(1.5, border_color),
                );
            }
        }
        if row + 1 == box_bottom {
            let c0 = (box_left.max(0) as u16).min(grid_width);
            let c1 = (box_right.max(0) as u16).min(grid_width);
            if c0 < c1 {
                painter.line_segment(
                    [
                        egui::pos2(x + c0 as f32 * cs, y + cs),
                        egui::pos2(x + c1 as f32 * cs, y + cs),
                    ],
                    egui::Stroke::new(1.5, border_color),
                );
            }
        }
        if row >= box_top && row < box_bottom {
            for box_x in [box_left, box_right] {
                if box_x >= 0 && (box_x as u16) <= grid_width {
                    painter.line_segment(
                        [
                            egui::pos2(x + box_x as f32 * cs, y),
                            egui::pos2(x + box_x as f32 * cs, y + cs),
                        ],
                        egui::Stroke::new(1.5, border_color),
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_grid_lines(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    pixel_row: u16,
    grid_width: u16,
    grid: &PixelGrid,
    grid_cell: f32,
    pal: &Palette,
) {
    use crate::pixel::{self, EdgeInterval};

    let cs = grid_cell;
    let r = pixel_row;
    let w = grid_width;

    let dark_stroke = egui::Stroke::new(1.0, pal.grid_bg);
    let light_stroke = egui::Stroke::new(1.0, pal.grid_on);

    // Vertical boundaries (between columns)
    for c in 0..=w {
        let overlap = if c == 0 || c == w {
            EdgeInterval::EMPTY
        } else {
            let left_shape = grid.get(r, c - 1);
            let right_shape = grid.get(r, c);
            let left_right = if left_shape.is_empty() {
                EdgeInterval::EMPTY
            } else {
                pixel::edge_coverage(left_shape.shape_id()).right
            };
            let right_left = if right_shape.is_empty() {
                EdgeInterval::EMPTY
            } else {
                pixel::edge_coverage(right_shape.shape_id()).left
            };
            left_right.intersect(right_left)
        };

        let lx = x + c as f32 * cs;
        if overlap.is_empty() {
            painter.line_segment([egui::pos2(lx, y), egui::pos2(lx, y + cs)], light_stroke);
        } else {
            let dark_y0 = y + overlap.start * cs;
            let dark_y1 = y + overlap.end * cs;
            if overlap.start > 1e-6 {
                painter.line_segment([egui::pos2(lx, y), egui::pos2(lx, dark_y0)], light_stroke);
            }
            painter.line_segment(
                [egui::pos2(lx, dark_y0), egui::pos2(lx, dark_y1)],
                dark_stroke,
            );
            if overlap.end < 1.0 - 1e-6 {
                painter.line_segment(
                    [egui::pos2(lx, dark_y1), egui::pos2(lx, y + cs)],
                    light_stroke,
                );
            }
        }
    }

    // Horizontal boundary at the top of this row
    for c in 0..w {
        let overlap = if r == 0 {
            EdgeInterval::EMPTY
        } else {
            let above_shape = grid.get(r - 1, c);
            let below_shape = grid.get(r, c);
            let above_bottom = if above_shape.is_empty() {
                EdgeInterval::EMPTY
            } else {
                pixel::edge_coverage(above_shape.shape_id()).bottom
            };
            let below_top = if below_shape.is_empty() {
                EdgeInterval::EMPTY
            } else {
                pixel::edge_coverage(below_shape.shape_id()).top
            };
            above_bottom.intersect(below_top)
        };

        let lx = x + c as f32 * cs;
        if overlap.is_empty() {
            painter.line_segment([egui::pos2(lx, y), egui::pos2(lx + cs, y)], light_stroke);
        } else {
            let dark_x0 = lx + overlap.start * cs;
            let dark_x1 = lx + overlap.end * cs;
            if overlap.start > 1e-6 {
                painter.line_segment([egui::pos2(lx, y), egui::pos2(dark_x0, y)], light_stroke);
            }
            painter.line_segment(
                [egui::pos2(dark_x0, y), egui::pos2(dark_x1, y)],
                dark_stroke,
            );
            if overlap.end < 1.0 - 1e-6 {
                painter.line_segment(
                    [egui::pos2(dark_x1, y), egui::pos2(lx + cs, y)],
                    light_stroke,
                );
            }
        }
    }

    // Horizontal boundary at the bottom of the last row
    if r + 1 == grid.height {
        for c in 0..w {
            painter.line_segment(
                [
                    egui::pos2(x + c as f32 * cs, y + cs),
                    egui::pos2(x + (c as f32 + 1.0) * cs, y + cs),
                ],
                light_stroke,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_grid_lines_with_extent(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    pixel_row: i16,
    own_width: u16,
    own_height: u16,
    extent: GridExtent,
    grid: Option<&PixelGrid>,
    grid_cell: f32,
    pal: &Palette,
) {
    let cs = grid_cell;
    let in_own_row = pixel_row >= 0 && pixel_row < own_height as i16;
    let has_extension = extent.left < 0
        || extent.right > own_width as i16
        || extent.top < 0
        || extent.bottom > own_height as i16;

    if !has_extension {
        if in_own_row {
            if let Some(grid) = grid {
                draw_grid_lines(
                    painter,
                    x,
                    y,
                    pixel_row as u16,
                    own_width,
                    grid,
                    grid_cell,
                    pal,
                );
            } else {
                draw_simple_grid_lines(
                    painter, x, y, pixel_row, own_width, own_height, grid_cell, pal,
                );
            }
        }
        return;
    }

    let ext_stroke = egui::Stroke::new(1.0, pal.grid_ext_grid);

    // Vertical lines in extended region
    for dc in extent.left..=extent.right {
        let in_own_left = in_own_row && dc > 0 && dc <= own_width as i16;
        let in_own_right = in_own_row && dc >= 0 && dc < own_width as i16;
        if in_own_left && in_own_right {
            continue;
        }
        if (dc == 0 || dc == own_width as i16) && in_own_row {
            continue;
        }
        let lx = x + (dc - extent.left) as f32 * cs;
        painter.line_segment([egui::pos2(lx, y), egui::pos2(lx, y + cs)], ext_stroke);
    }

    // Horizontal lines at top in extended region
    for dc in extent.left..extent.right {
        let in_own_col = dc >= 0 && dc < own_width as i16;
        if in_own_row && in_own_col {
            continue;
        }
        let lx = x + (dc - extent.left) as f32 * cs;
        painter.line_segment([egui::pos2(lx, y), egui::pos2(lx + cs, y)], ext_stroke);
    }

    // Bottom of last displayed row
    if pixel_row + 1 == extent.bottom {
        for dc in extent.left..extent.right {
            let in_own_col = dc >= 0 && dc < own_width as i16;
            let is_own_bottom = pixel_row + 1 == own_height as i16;
            if in_own_col && is_own_bottom {
                continue;
            }
            let lx = x + (dc - extent.left) as f32 * cs;
            painter.line_segment(
                [egui::pos2(lx, y + cs), egui::pos2(lx + cs, y + cs)],
                ext_stroke,
            );
        }
    }

    // Own area grid lines on top
    if in_own_row {
        let own_x = x + (-extent.left) as f32 * cs;
        if let Some(grid) = grid {
            draw_grid_lines(
                painter,
                own_x,
                y,
                pixel_row as u16,
                own_width,
                grid,
                grid_cell,
                pal,
            );
        } else {
            draw_simple_grid_lines(
                painter, own_x, y, pixel_row, own_width, own_height, grid_cell, pal,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_simple_grid_lines(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    row: i16,
    width: u16,
    height: u16,
    grid_cell: f32,
    pal: &Palette,
) {
    let cs = grid_cell;
    let stroke = egui::Stroke::new(1.0, pal.grid_on);
    for c in 0..=width {
        painter.line_segment(
            [
                egui::pos2(x + c as f32 * cs, y),
                egui::pos2(x + c as f32 * cs, y + cs),
            ],
            stroke,
        );
    }
    for c in 0..width {
        painter.line_segment(
            [
                egui::pos2(x + c as f32 * cs, y),
                egui::pos2(x + (c + 1) as f32 * cs, y),
            ],
            stroke,
        );
    }
    if row + 1 == height as i16 {
        for c in 0..width {
            painter.line_segment(
                [
                    egui::pos2(x + c as f32 * cs, y + cs),
                    egui::pos2(x + (c + 1) as f32 * cs, y + cs),
                ],
                stroke,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_grid_hover_preview(
    ui: &egui::Ui,
    painter: &egui::Painter,
    mode: &EditMode,
    item_idx: usize,
    grid_width: u16,
    grid_height: u16,
    pixel_row: i16,
    extent: GridExtent,
    grid_x: f32,
    grid_y: f32,
    grid_cell: f32,
) {
    if let EditMode::GlyphEdit {
        item_idx: eidx,
        selected_shape,
    } = mode
        && *eidx == item_idx
        && let Some(hp) = ui.input(|i| i.pointer.hover_pos())
        && hp.y >= grid_y
        && hp.y < grid_y + grid_cell
    {
        let rel_x = hp.x - grid_x;
        let gc = (rel_x / grid_cell) as i32 + extent.left as i32;
        let in_own =
            pixel_row >= 0 && pixel_row < grid_height as i16 && gc >= 0 && gc < grid_width as i32;
        if in_own {
            let cell_rect = egui::Rect::from_min_size(
                egui::pos2(
                    grid_x + (gc - extent.left as i32) as f32 * grid_cell,
                    grid_y,
                ),
                egui::vec2(grid_cell, grid_cell),
            );
            let preview_color = Palette::get(ui).glyph_edit_preview;
            if selected_shape.is_empty() {
                painter.rect_stroke(
                    cell_rect.shrink(1.0),
                    0.0,
                    egui::Stroke::new(1.0, preview_color),
                    egui::epaint::StrokeKind::Inside,
                );
            } else {
                glyph_widget::draw_pixel_cell_colored(
                    painter,
                    cell_rect,
                    *selected_shape,
                    Some(preview_color),
                );
            }
            ui.ctx().request_repaint();
        }
    }
}

pub(crate) fn char_x_pos(
    ui: &egui::Ui,
    font_id: &egui::FontId,
    text: &str,
    char_col: usize,
) -> f32 {
    if char_col == 0 || text.is_empty() {
        return 0.0;
    }
    let prefix: String = text.chars().take(char_col).collect();
    ui.fonts(|f| {
        let galley = f.layout_no_wrap(prefix, font_id.clone(), egui::Color32::WHITE);
        galley.rect.width()
    })
}

pub(crate) fn x_to_char_col(ui: &egui::Ui, font_id: &egui::FontId, text: &str, x: f32) -> usize {
    if text.is_empty() || x <= 0.0 {
        return 0;
    }
    let char_count = text.chars().count();
    for col in 0..=char_count {
        let cx = char_x_pos(ui, font_id, text, col);
        if cx > x {
            return if col > 0 { col - 1 } else { 0 };
        }
    }
    char_count
}
