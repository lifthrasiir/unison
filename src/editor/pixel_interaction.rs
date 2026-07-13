use std::collections::HashMap;

use crate::document::{DocLine, Document, DocumentItem, NamePartsMap, PixelGrid};
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};
use crate::editor::undo;
use crate::editor::{EditMode, EditorState};
use crate::pixel;

use super::document_view::GridExtent;

fn format_dragged_ref(gref: &crate::document::GlyphRef, col: i16, row: i16) -> String {
    // A drag is an explicit placement even when it lands at the origin.
    // Omitting `0 0` would turn it back into an auto-offset ref.
    if gref.negated {
        format!("ref {} {} {} negated", gref.name, col, row)
    } else {
        format!("ref {} {} {}", gref.name, col, row)
    }
}

fn layer_effective_offset(composite: &GlyphComposite, ref_idx: usize) -> Option<(i16, i16)> {
    composite
        .layers
        .iter()
        .find(|layer| layer.ref_idx == ref_idx)
        .map(|layer| (layer.logical_offset_row, layer.logical_offset_col))
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
    grid_x: f32,
    grid_y: f32,
    grid_cell: f32,
) {
    let in_own_row = pixel_row >= 0 && pixel_row < grid_height as i16;
    if let EditMode::GlyphEdit {
        item_idx: eidx,
        selected_shape,
    } = &state.mode
        && *eidx == item_idx
    {
        if state.suppress_grid_click && !ui.input(|i| i.pointer.primary_down()) {
            state.suppress_grid_click = false;
        }
        let primary = !state.suppress_grid_click && ui.input(|i| i.pointer.primary_down());
        let secondary = ui.input(|i| i.pointer.secondary_down());
        if (primary || secondary)
            && let Some(pp) = ui.input(|i| i.pointer.hover_pos())
        {
            let rel_x = pp.x - grid_x;
            let gc = (rel_x / grid_cell) as i32 + extent.left as i32;
            if pp.y >= grid_y
                && pp.y < grid_y + grid_cell
                && in_own_row
                && gc >= 0
                && gc < grid_width as i32
            {
                let col = gc as u16;
                let row = pixel_row as u16;
                let new_shape = if secondary {
                    pixel::PixelShape::EMPTY
                } else {
                    *selected_shape
                };
                if let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) {
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
                        *needs_rederive = true;
                        ui.ctx().request_repaint();
                    }
                } else if !new_shape.is_empty() && grid_doc_line > 0 {
                    // Materialize pixel grid for ref-only glyph
                    let header_line = grid_doc_line - 1;
                    if let Some(DocLine::Text(header_text)) = lines.get(header_line) {
                        let trimmed = header_text.trim();
                        if let Ok(tokens) = crate::document_io::tokenize_tokens(trimmed) {
                            if tokens.first().is_some_and(|t| t == "glyph") && tokens.len() == 2 {
                                let new_header =
                                    format!("{} {} {}", trimmed, grid_width, grid_height);
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
                            }
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_layer_drag(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: &mut bool,
    doc: &Document,
    item_idx: usize,
    layer_idx: usize,
    item_line_starts: &[usize],
    composite: Option<&GlyphComposite>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    grid_cell: f32,
) {
    let body = match doc.items.get(item_idx) {
        Some(DocumentItem::Glyph { body, .. }) => body,
        _ => return,
    };

    let num_refs = body.refs.len();
    if layer_idx < num_refs {
        handle_ref_drag_inner(
            ui,
            lines,
            state,
            needs_rederive,
            body,
            item_idx,
            layer_idx,
            item_line_starts,
            composite,
            named_glyphs,
            name_parts,
            grid_cell,
        );
    } else {
        let point_idx = layer_idx - num_refs;
        handle_point_drag_inner(
            ui,
            lines,
            state,
            needs_rederive,
            body,
            item_idx,
            point_idx,
            item_line_starts,
            grid_cell,
        );
    }
}

#[allow(clippy::too_many_arguments, clippy::ptr_arg)]
fn handle_ref_drag_inner(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: &mut bool,
    body: &crate::document::GlyphBody,
    item_idx: usize,
    ref_idx: usize,
    item_line_starts: &[usize],
    composite: Option<&GlyphComposite>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    grid_cell: f32,
) {
    let gref = match body.refs.get(ref_idx) {
        Some(r) => r,
        None => return,
    };
    let ref_resolved =
        match ref_composite::resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts) {
            Some(g) => g,
            None => return,
        };
    let comp = match composite {
        Some(c) => c,
        None => return,
    };

    let dragging = ui.input(|i| i.pointer.primary_down());
    let drag_id = egui::Id::new("layer_drag_accum");
    if !dragging {
        ui.data_mut(|d| d.remove::<egui::Vec2>(drag_id));
        return;
    }

    let drag_delta = ui.input(|i| i.pointer.delta());
    if drag_delta.x.abs() < 0.5 && drag_delta.y.abs() < 0.5 {
        return;
    }

    let mut accum = ui.data_mut(|d| d.get_temp::<egui::Vec2>(drag_id).unwrap_or_default());
    accum += drag_delta;

    let dcol = (accum.x / grid_cell).round() as i16;
    let drow = (accum.y / grid_cell).round() as i16;

    if dcol == 0 && drow == 0 {
        ui.data_mut(|d| d.insert_temp(drag_id, accum));
        return;
    }

    let (current_row, current_col) =
        layer_effective_offset(comp, ref_idx).unwrap_or_else(|| (gref.row(), gref.col()));
    let new_row = current_row + drow;
    let new_col = current_col + dcol;

    // Overlap constraint
    let ref_w = ref_resolved.grid.width as i16;
    let ref_h = ref_resolved.grid.height as i16;

    let mut other_min_r: Option<i16> = None;
    let mut other_min_c: Option<i16> = None;
    let mut other_max_r: Option<i16> = None;
    let mut other_max_c: Option<i16> = None;

    let mut update_bounds = |r0: i16, c0: i16, r1: i16, c1: i16| {
        other_min_r = Some(other_min_r.map_or(r0, |v: i16| v.min(r0)));
        other_min_c = Some(other_min_c.map_or(c0, |v: i16| v.min(c0)));
        other_max_r = Some(other_max_r.map_or(r1, |v: i16| v.max(r1)));
        other_max_c = Some(other_max_c.map_or(c1, |v: i16| v.max(c1)));
    };

    if let Some(grid) = &body.pixels {
        update_bounds(0, 0, grid.height as i16, grid.width as i16);
    }
    for layer in &comp.layers {
        if layer.ref_idx == ref_idx {
            continue;
        }
        let eff_row = layer.offset_row - comp.own_offset_row;
        let eff_col = layer.offset_col - comp.own_offset_col;
        update_bounds(
            eff_row,
            eff_col,
            eff_row + layer.grid.height as i16,
            eff_col + layer.grid.width as i16,
        );
    }

    if let (Some(omin_r), Some(omin_c), Some(omax_r), Some(omax_c)) =
        (other_min_r, other_min_c, other_max_r, other_max_c)
        && (new_col + ref_w <= omin_c
            || new_col >= omax_c
            || new_row + ref_h <= omin_r
            || new_row >= omax_r)
    {
        ui.data_mut(|d| d.insert_temp(drag_id, accum));
        return;
    }

    accum.x -= dcol as f32 * grid_cell;
    accum.y -= drow as f32 * grid_cell;
    ui.data_mut(|d| d.insert_temp(drag_id, accum));

    let header_line = item_line_starts.get(item_idx).copied().unwrap_or(0);
    let ref_line = header_line + 1 + if body.pixels.is_some() { 1 } else { 0 } + ref_idx;

    if let Some(DocLine::Text(old_text)) = lines.get(ref_line) {
        let old_text = old_text.clone();
        let new_text = format_dragged_ref(gref, new_col, new_row);
        if old_text != new_text {
            state.undo.push_text(
                ref_line,
                0,
                old_text,
                new_text.clone(),
                state.cursor,
                state.cursor,
            );
            lines[ref_line] = DocLine::Text(new_text);
            *needs_rederive = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::GlyphRef;
    use crate::editor::ref_composite::CompositeLayer;

    #[test]
    fn dragged_ref_at_origin_stays_explicit() {
        let normal = GlyphRef {
            name: "part".into(),
            offset: Some((1, 0)),
            negated: false,
        };
        assert_eq!(format_dragged_ref(&normal, 0, 0), "ref part 0 0");

        let negated = GlyphRef {
            negated: true,
            ..normal
        };
        assert_eq!(format_dragged_ref(&negated, 0, 0), "ref part 0 0 negated");
    }

    #[test]
    fn drag_starts_from_derived_composite_offset() {
        let composite = GlyphComposite {
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
            }],
        };
        assert_eq!(layer_effective_offset(&composite, 7), Some((4, 5)));
        assert_eq!(layer_effective_offset(&composite, 6), None);
    }
}

#[allow(clippy::too_many_arguments, clippy::ptr_arg)]
fn handle_point_drag_inner(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: &mut bool,
    body: &crate::document::GlyphBody,
    item_idx: usize,
    point_idx: usize,
    item_line_starts: &[usize],
    grid_cell: f32,
) {
    let point = match body.points.get(point_idx) {
        Some(p) => p,
        None => return,
    };

    let dragging = ui.input(|i| i.pointer.primary_down());
    let drag_id = egui::Id::new("layer_drag_accum");
    if !dragging {
        ui.data_mut(|d| d.remove::<egui::Vec2>(drag_id));
        return;
    }

    let drag_delta = ui.input(|i| i.pointer.delta());
    if drag_delta.x.abs() < 0.5 && drag_delta.y.abs() < 0.5 {
        return;
    }

    let mut accum = ui.data_mut(|d| d.get_temp::<egui::Vec2>(drag_id).unwrap_or_default());
    accum += drag_delta;

    let dcol = (accum.x / grid_cell).round() as i16;
    let drow = (accum.y / grid_cell).round() as i16;

    if dcol == 0 && drow == 0 {
        ui.data_mut(|d| d.insert_temp(drag_id, accum));
        return;
    }

    let new_col = point.col + dcol;
    let new_row = point.row + drow;
    let new_col_end = point.col_end + dcol;
    let new_row_end = point.row_end + drow;

    accum.x -= dcol as f32 * grid_cell;
    accum.y -= drow as f32 * grid_cell;
    ui.data_mut(|d| d.insert_temp(drag_id, accum));

    let header_line = item_line_starts.get(item_idx).copied().unwrap_or(0);
    let point_line =
        header_line + 1 + if body.pixels.is_some() { 1 } else { 0 } + body.refs.len() + point_idx;

    if let Some(DocLine::Text(old_text)) = lines.get(point_line) {
        let old_text = old_text.clone();
        let col_s = if new_col == new_col_end {
            format!("{}", new_col)
        } else {
            format!("{}..{}", new_col, new_col_end)
        };
        let row_s = if new_row == new_row_end {
            format!("{}", new_row)
        } else {
            format!("{}..{}", new_row, new_row_end)
        };
        let new_text = format!("anchor {} {} {}", point.position, col_s, row_s);
        if old_text != new_text {
            state.undo.push_text(
                point_line,
                0,
                old_text,
                new_text.clone(),
                state.cursor,
                state.cursor,
            );
            lines[point_line] = DocLine::Text(new_text);
            *needs_rederive = true;
        }
    }
}
