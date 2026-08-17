use crate::document::{DocLine, Document, DocumentItem, PixelGrid};
use crate::editor::ref_composite::GlyphComposite;
use crate::editor::undo;
use crate::editor::{EditMode, EditorId, EditorState, Slot};
use crate::pixel;

use super::document_view::GridExtent;

fn format_dragged_ref(gref: &crate::document::GlyphRef, col: i16, row: i16) -> String {
    // Always pass an explicit offset — even `0 0` — so the ref doesn't
    // revert to auto-placement.
    gref.format_line(Some((col, row)))
}

/// The `(row, col)` a ref is actually drawn at, offset line or not — what a
/// drag has to continue from when the ref is auto-placed.
pub(crate) fn layer_effective_offset(
    composite: &GlyphComposite,
    ref_idx: usize,
) -> Option<(i16, i16)> {
    composite
        .layers
        .iter()
        .find(|layer| layer.ref_idx == ref_idx)
        .map(|layer| (layer.logical_offset_row, layer.logical_offset_col))
}

/// Did the in-flight pointer gesture start on this glyph's grid?
///
/// Painting reads the raw button state rather than a click, so without this it
/// follows any held button that happens to hover a cell — and a menu drawn over
/// the grid closes on the *press*, leaving the button down over the cell the
/// entry covered for the frames that follow. The answer therefore has to be
/// latched when the button goes down: by the next frame the menu is gone and
/// nothing distinguishes it from a stroke that began on the grid.
///
/// Two things make a press a grid press: it landed inside the drawn grid (of
/// the band, not under a scrollbar), *and* nothing was drawn above the editor
/// there — a popup covering the grid owns its own layer.
#[allow(clippy::too_many_arguments)]
fn grid_gesture(
    ui: &egui::Ui,
    state: &EditorState,
    pixel_row: i16,
    extent: GridExtent,
    strip: &super::document_view::GridStrip,
    grid_x: f32,
    grid_y: f32,
    grid_cell: f32,
) -> bool {
    let gesture_id = state.key(Slot::GridPaintGesture);
    if !ui.input(|i| i.pointer.any_down()) {
        ui.data_mut(|d| d.remove::<bool>(gesture_id));
        return false;
    }
    if ui.input(|i| i.pointer.any_pressed())
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        // `grid_y` is this row's top edge; the rows above and below it belong
        // to the same grid, and a stroke may start on any of them.
        let grid_top = grid_y - (pixel_row - extent.top) as f32 * grid_cell;
        let grid_rect = egui::Rect::from_min_size(
            egui::pos2(grid_x, grid_top),
            egui::vec2(
                extent.display_width(grid_cell),
                (extent.bottom - extent.top) as f32 * grid_cell,
            ),
        )
        .intersect(ui.clip_rect());
        if grid_rect.contains(origin)
            && strip.accepts_pointer(origin)
            && ui.ctx().layer_id_at(origin) == Some(ui.layer_id())
        {
            ui.data_mut(|d| d.insert_temp(gesture_id, true));
        }
    }
    ui.data(|d| d.get_temp::<bool>(gesture_id).unwrap_or(false))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_pixel_painting(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: &mut bool,
    grid_doc_line: usize,
    item_idx: usize,
    pixel_row: i16,
    grid_width: u16,
    grid_height: u16,
    extent: GridExtent,
    strip: &super::document_view::GridStrip,
    grid_x: f32,
    grid_y: f32,
    grid_cell: f32,
) {
    let in_own_row = pixel_row >= 0 && pixel_row < grid_height as i16;
    let mut slant_toggle: Option<pixel::PixelShape> = None;
    if let EditMode::GlyphEdit {
        item_idx: eidx,
        selected_shape,
    } = &state.mode
        && *eidx == item_idx
    {
        if state.suppress_grid_click && !ui.input(|i| i.pointer.primary_down()) {
            state.suppress_grid_click = false;
        }
        let on_grid_gesture = grid_gesture(
            ui, state, pixel_row, extent, strip, grid_x, grid_y, grid_cell,
        );
        let primary =
            on_grid_gesture && !state.suppress_grid_click && ui.input(|i| i.pointer.primary_down());
        let secondary = on_grid_gesture && ui.input(|i| i.pointer.secondary_down());
        let shift_held = ui.input(|i| i.modifiers.shift);

        let slant_last_id = state.key(Slot::SlantToggleLastCell);
        if !primary && !secondary {
            ui.data_mut(|d| d.remove::<(u16, u16)>(slant_last_id));
        }

        if (primary || secondary)
            && let Some(pp) = ui.input(|i| i.pointer.hover_pos())
        {
            let rel_x = pp.x - grid_x;
            let gc = (rel_x / grid_cell) as i32 + extent.left as i32;
            if strip.accepts_pointer(pp)
                && pp.y >= grid_y
                && pp.y < grid_y + grid_cell
                && in_own_row
                && gc >= 0
                && gc < grid_width as i32
            {
                let col = gc as u16;
                let row = pixel_row as u16;
                let last_cell: Option<(u16, u16)> = ui.data(|d| d.get_temp(slant_last_id));
                let on_same_slant_cell =
                    selected_shape.is_slant_pair() && last_cell == Some((row, col));
                let new_shape = if secondary {
                    // Right-click erases; with shift it erases to a hardblank,
                    // the blank that stays in the file.
                    if shift_held {
                        pixel::PixelShape::new(pixel::PX_HARDBLANK, false)
                    } else {
                        pixel::PixelShape::EMPTY
                    }
                } else if shift_held && !selected_shape.is_clear() {
                    selected_shape.with_fill_toggled()
                } else {
                    *selected_shape
                };
                let mut painted = false;
                if on_same_slant_cell {
                    // Already painted this cell with the pre-toggle shape; don't overwrite.
                } else if let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) {
                    let old_shape = grid.get(row, col);
                    if old_shape != new_shape {
                        state.undo.push_pixel(
                            grid_doc_line,
                            undo::PixelChange {
                                row,
                                col,
                                old: old_shape,
                                new: new_shape,
                            },
                            state.cursor,
                            state.cursor,
                        );
                        if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_doc_line) {
                            grid.set(row, col, new_shape);
                        }
                        state.skip_reconcile = true;
                        state.pixel_paint_dirty = Some((item_idx, grid_doc_line));
                        state.suppress_font_rebuild = true;
                        *needs_rederive = true;
                        ui.ctx().request_repaint();
                        painted = true;
                    }
                } else if !new_shape.is_clear() && grid_doc_line > 0 {
                    // Materialize pixel grid for ref-only glyph
                    let header_line = grid_doc_line - 1;
                    if let Some(DocLine::Text(header_text)) = lines.get(header_line) {
                        let trimmed = header_text.trim();
                        if let Ok(tokens) = crate::document_io::tokenize_tokens(trimmed)
                            && tokens.first().is_some_and(|t| t == "glyph")
                            && tokens.len() == 2
                        {
                            let new_header = crate::document_io::append_to_line(
                                trimmed,
                                &format!("{grid_width} {grid_height}"),
                            );
                            let mut new_grid = PixelGrid::new(grid_width, grid_height);
                            new_grid.set(row, col, new_shape);

                            let old_header = header_text.clone();
                            state.undo.break_coalesce();
                            state.undo.push_lines(
                                header_line,
                                vec![DocLine::Text(old_header)],
                                vec![
                                    DocLine::Text(new_header.clone()),
                                    DocLine::Grid(new_grid.clone()),
                                ],
                                state.cursor,
                                state.cursor,
                            );
                            state.undo.break_coalesce();

                            lines[header_line] = DocLine::Text(new_header);
                            lines.insert(header_line + 1, DocLine::Grid(new_grid));
                            state.skip_reconcile = true;
                            *needs_rederive = true;
                            ui.ctx().request_repaint();
                            painted = true;
                        }
                    }
                }
                if painted && !secondary && new_shape.is_slant_pair() {
                    slant_toggle = Some(new_shape.slant_direction_pair());
                    ui.data_mut(|d| d.insert_temp(slant_last_id, (row, col)));
                }
            }
        }
    }
    if let Some(toggled) = slant_toggle
        && let EditMode::GlyphEdit { selected_shape, .. } = &mut state.mode
    {
        *selected_shape = toggled;
        crate::editor::glyph_widget::sync_rotation(toggled, &mut state.shape_rotation);
    }
}

/// DocLine index of the body line backing layer `layer_idx`.
///
/// Layer indices run over the refs first and the points after, but the *lines*
/// need not: the parser accepts `ref` and `anchor` lines in any order, so the
/// n-th layer's line is found by scanning the body block (which starts after
/// the header and the glyph's single `DocLine::Grid`, when it owns one) for
/// the n-th line of that layer's kind. `layer_idx == refs + points` addresses
/// one past the block's last line. Falls back to plain offset arithmetic when
/// `lines` no longer matches `body` (a transient edit mid-frame).
pub(crate) fn layer_doc_line(
    lines: &[DocLine],
    body: &crate::document::GlyphBody,
    header_line: usize,
    layer_idx: usize,
) -> usize {
    let base = header_line + 1 + usize::from(body.pixels.is_some());
    let total = body.refs.len() + body.points.len();
    let (want_ref, ordinal) = if layer_idx < body.refs.len() {
        (true, layer_idx)
    } else {
        (false, layer_idx - body.refs.len())
    };
    let mut seen = 0usize;
    for i in 0..total {
        let is_ref = match lines.get(base + i) {
            Some(DocLine::Text(t)) => match t.split_whitespace().next() {
                Some("ref") => true,
                Some("anchor") => false,
                _ => return base + layer_idx,
            },
            _ => return base + layer_idx,
        };
        if is_ref == want_ref && layer_idx < total {
            if seen == ordinal {
                return base + i;
            }
            seen += 1;
        }
    }
    base + total
}

/// Drag a `ref` or `anchor` layer by whole grid cells, rewriting its body line.
///
/// Both layer kinds and both kinds of glyph (own pixel grid or ref-only) take
/// the same path: nothing here may depend on the glyph having a grid of its
/// own. A layer used to be pinned to whatever else the glyph draws — it could
/// only be dropped where it still overlapped the pixel grid or another layer —
/// but that test was evaluated at the *destination*, so a layer that already
/// sat clear of everything else could not be dragged at all. Ref-only glyphs
/// made of side-by-side parts are exactly that case, and they were immovable
/// in every direction; a glyph with a pixel grid was, in practice, never
/// constrained at all. Points were never constrained either. So the rule is
/// gone rather than repaired: a layer follows the pointer, and undo takes it
/// back.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_layer_drag(
    ui: &egui::Ui,
    lines: &mut [DocLine],
    state: &mut EditorState,
    needs_rederive: &mut bool,
    doc: &Document,
    item_idx: usize,
    layer_idx: usize,
    item_line_starts: &[usize],
    composite: Option<&GlyphComposite>,
    grid_cell: f32,
) {
    let body = match doc.items.get(item_idx) {
        Some(DocumentItem::Glyph { body, .. }) => body,
        _ => return,
    };

    let Some((dcol, drow)) = drag_cell_step(ui, state.id(), grid_cell) else {
        return;
    };

    let new_text = if let Some(gref) = body.refs.get(layer_idx) {
        // Start from the placement the composite actually derived, so dragging
        // an auto-placed ref continues from where it is drawn.
        let (row, col) = composite
            .and_then(|comp| layer_effective_offset(comp, layer_idx))
            .unwrap_or_else(|| (gref.row(), gref.col()));
        format_dragged_ref(gref, col + dcol, row + drow)
    } else if let Some(point) = body.points.get(layer_idx - body.refs.len()) {
        point.shifted(dcol, drow).format_line()
    } else {
        return;
    };

    let header_line = item_line_starts.get(item_idx).copied().unwrap_or(0);
    let layer_line = layer_doc_line(lines, body, header_line, layer_idx);

    if let Some(DocLine::Text(old_text)) = lines.get(layer_line)
        && *old_text != new_text
    {
        let old_text = old_text.clone();
        state.undo.push_text(
            layer_line,
            0,
            old_text,
            new_text.clone(),
            state.cursor,
            state.cursor,
        );
        lines[layer_line] = DocLine::Text(new_text);
        *needs_rederive = true;
    }
}

/// Accumulate the pointer drag and convert it to a whole-cell step.
/// Returns `(dcol, drow)` once the accumulated drag reaches at least one cell,
/// keeping the sub-cell remainder for the next frame; returns `None` (updating
/// or clearing the stored accumulator as appropriate) while the drag is still
/// sub-cell or the button is released.
fn drag_cell_step(ui: &egui::Ui, editor: EditorId, grid_cell: f32) -> Option<(i16, i16)> {
    let dragging = ui.input(|i| i.pointer.primary_down());
    let drag_id = editor.key(Slot::LayerDragAccum);
    if !dragging {
        ui.data_mut(|d| d.remove::<egui::Vec2>(drag_id));
        return None;
    }

    let drag_delta = ui.input(|i| i.pointer.delta());
    if drag_delta.x.abs() < 0.5 && drag_delta.y.abs() < 0.5 {
        return None;
    }

    let mut accum = ui.data_mut(|d| d.get_temp::<egui::Vec2>(drag_id).unwrap_or_default());
    accum += drag_delta;

    let dcol = (accum.x / grid_cell).round() as i16;
    let drow = (accum.y / grid_cell).round() as i16;

    accum.x -= dcol as f32 * grid_cell;
    accum.y -= drow as f32 * grid_cell;
    ui.data_mut(|d| d.insert_temp(drag_id, accum));

    if dcol == 0 && drow == 0 {
        return None;
    }
    Some((dcol, drow))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::GlyphRef;
    use crate::editor::ref_composite::CompositeLayer;

    #[test]
    fn dragged_ref_at_origin_stays_explicit() {
        let normal = GlyphRef {
            raw_name: None,
            comment: None,
            name: "part".into(),
            offset: Some((1, 0)),
            negated: false,
            inherit: false,
            if_exists: false,
            fill: None,
            visibility: None,
        };
        assert_eq!(format_dragged_ref(&normal, 0, 0), "ref part 0 0");

        let negated = GlyphRef {
            raw_name: None,
            negated: true,
            ..normal
        };
        assert_eq!(format_dragged_ref(&negated, 0, 0), "ref part 0 0 negated");
    }

    #[test]
    fn drag_starts_from_derived_composite_offset() {
        let composite = GlyphComposite {
            inherited_anchors: Vec::new(),
            width: 4,
            height: 2,
            own_offset_row: 2,
            own_offset_col: 3,
            layers: vec![CompositeLayer {
                ref_idx: 7,
                resolved_name: String::new(),
                grid: PixelGrid::new(1, 1),
                offset_row: 6,
                offset_col: 8,
                logical_offset_row: 4,
                logical_offset_col: 5,
                negated: false,
                fill_color: None,
            }],
        };
        assert_eq!(layer_effective_offset(&composite, 7), Some((4, 5)));
        assert_eq!(layer_effective_offset(&composite, 6), None);
    }
}
