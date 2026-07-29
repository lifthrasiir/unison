use std::collections::HashMap;

use crate::document::{Document, DocumentItem, NamePartsMap};
use crate::editor::colors::Palette;
use crate::editor::document_view::{
    INLINE_PALETTE_CELL, PREVIEW_SCALE, UNFILLED_OPACITY,
};
use crate::editor::grid_render;
use crate::editor::minimap;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};
use crate::editor::visual_lines::{preview_max_height, preview_row_height};
use crate::editor::{EditMode, EditorId, EditorState, Slot};
use crate::pixel;

pub(crate) struct InlineToolsResult {
    pub click_consumed: bool,
    pub inline_ref: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
/// Step the layer selection (0 = pixel layer, 1.. = ref/point layers) by
/// `step`, switching the edit mode accordingly. Used by both scroll-wheel
/// layer cycling on the grid and on the inline tools preview row.
pub(crate) fn cycle_layer_mode(
    state: &mut EditorState,
    body: &crate::document::GlyphBody,
    edit_idx: usize,
    step: i32,
) {
    let layer_count = body.refs.len() + body.points.len();
    let total = 1 + layer_count as i32;
    let current = match &state.mode {
        EditMode::GlyphEdit { .. } | EditMode::PixelSelect { .. } => 0,
        EditMode::LayerMove { layer_idx, .. } => *layer_idx as i32 + 1,
        _ => 0,
    };
    let next = (current + step).clamp(0, total - 1);
    if next != current {
        if next == 0 {
            state.mode = EditMode::GlyphEdit {
                item_idx: edit_idx,
                selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
            };
        } else {
            state.mode = EditMode::LayerMove {
                item_idx: edit_idx,
                layer_idx: (next - 1) as usize,
            };
        }
    }
}

/// Items of the subglyph (ref layer) context menu. Shared by the ref thumbnail
/// in the inline tools panel and by right-clicking the grid while that layer is
/// the selected one, so both entry points offer the same actions.
/// Returns true when "Inline to pixels" was chosen.
pub(crate) fn subglyph_context_menu(ui: &mut egui::Ui) -> bool {
    if ui.button("Inline to pixels").clicked() {
        ui.close_menu();
        return true;
    }
    false
}

pub(crate) fn draw_inline_tools_panel(
    ui: &egui::Ui,
    painter: &egui::Painter,
    panel_x: f32,
    panel_y: f32,
    doc: &Document,
    state: &mut EditorState,
    edit_idx: usize,
    composites: &HashMap<usize, GlyphComposite>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    shadow: Option<&crate::editor::anchor_shadow::AnchorShadow>,
    click_pos: Option<egui::Pos2>,
    zoom_level: u32,
) -> InlineToolsResult {
    let no_action = InlineToolsResult {
        click_consumed: false,
        inline_ref: None,
    };
    let body = match doc.items.get(edit_idx) {
        Some(DocumentItem::Glyph { body, .. }) => body,
        _ => return no_action,
    };

    let zoom = zoom_level as f32;
    let preview_scale = PREVIEW_SCALE * zoom;
    let palette_cell = INLINE_PALETTE_CELL * zoom;
    let composite = composites.get(&edit_idx);
    let max_ph = preview_max_height(body, composite, named_glyphs, name_parts);
    let prh = preview_row_height(zoom_level, max_ph);

    // Compute panel bounding rect to detect if click is consumed
    let palette_rows = crate::editor::glyph_widget::palette_rows();
    let palette_height = palette_rows as f32 * palette_cell;
    let panel_total_height = prh + 4.0 + palette_height;
    let palette_cols = crate::editor::glyph_widget::PALETTE_COLS;
    let panel_width = palette_cols as f32 * palette_cell;
    let panel_rect = egui::Rect::from_min_size(
        egui::pos2(panel_x, panel_y),
        egui::vec2(panel_width, panel_total_height),
    );

    let click_consumed = click_pos.is_some_and(|cp| panel_rect.contains(cp));

    // Wheel scroll on preview row to cycle subglyphs
    let preview_row_rect =
        egui::Rect::from_min_size(egui::pos2(panel_x, panel_y), egui::vec2(panel_width, prh));
    let hover_on_preview_row = ui.input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|hp| preview_row_rect.contains(hp))
    });
    let editor = state.id();
    ui.ctx().data_mut(|d| {
        d.insert_temp(editor.key(Slot::SubglyphPreviewHover), hover_on_preview_row);
    });
    if let Some(step) = crate::editor::document_view::interceptor_scroll_step(
        ui.ctx(),
        editor,
        hover_on_preview_row,
    )
    {
        cycle_layer_mode(state, body, edit_idx, step);
    }

    // --- Row 0-1: 2x pixelated previews (composite + subglyphs) ---
    let mut px = panel_x;

    let is_pixel_mode =
        matches!(state.mode, EditMode::GlyphEdit { item_idx, .. } if item_idx == edit_idx);

    // Full composite preview (always at exact preview_scale per pixel)
    let full_preview_size = if let Some(comp) = composite {
        egui::vec2(
            comp.width as f32 * preview_scale,
            comp.height as f32 * preview_scale,
        )
    } else if let Some(grid) = &body.pixels {
        egui::vec2(
            grid.width as f32 * preview_scale,
            grid.height as f32 * preview_scale,
        )
    } else {
        egui::vec2(16.0 * zoom, 16.0 * zoom)
    };

    let full_rect = egui::Rect::from_min_size(egui::pos2(px, panel_y), full_preview_size);
    let pal = Palette::get(ui);
    minimap::draw_preview_bitmap(
        painter,
        full_rect,
        body,
        composite,
        named_glyphs,
        None,
        &pal,
    );
    if is_pixel_mode {
        painter.rect_stroke(
            full_rect,
            0.0,
            egui::Stroke::new(2.0, pal.cursor_border),
            egui::epaint::StrokeKind::Outside,
        );
    }

    if let Some(cp) = click_pos
        && full_rect.contains(cp)
    {
        state.mode = EditMode::GlyphEdit {
            item_idx: edit_idx,
            selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
        };
    }

    px += full_preview_size.x + 4.0 * zoom;

    // Individual ref previews (always at exact preview_scale per pixel,
    // but at least as large as the pixel layer)
    let pixel_preview_w = full_preview_size.x;
    let pixel_preview_h = full_preview_size.y;
    let mut inline_ref_action: Option<usize> = None;
    for (ref_idx, gref) in body.refs.iter().enumerate() {
        let resolved =
            ref_composite::resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts);
        let ref_size = if let Some(rg) = resolved {
            egui::vec2(
                (rg.grid.width as f32 * preview_scale).max(pixel_preview_w),
                (rg.grid.height as f32 * preview_scale).max(pixel_preview_h),
            )
        } else {
            egui::vec2(pixel_preview_w, pixel_preview_h)
        };

        let ref_rect = egui::Rect::from_min_size(egui::pos2(px, panel_y), ref_size);

        // Publish this thumbnail's rect for the in-crate GUI test harness, so
        // tests can click it without hand-replicating the layout math above.
        #[cfg(test)]
        crate::editor::harness::capture_ref_rect(
            ui.ctx(),
            state.id(),
            edit_idx,
            ref_idx,
            ref_rect,
        );

        let is_active = matches!(
            state.mode,
            EditMode::LayerMove { item_idx, layer_idx } if item_idx == edit_idx && layer_idx == ref_idx
        );

        let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, ref_idx);
        painter.rect_filled(ref_rect, 0.0, pal.grid_bg);
        if let Some(rg) = resolved {
            for r in 0..rg.grid.height {
                for c in 0..rg.grid.width {
                    if rg.grid.get(r, c).is_filled() {
                        let px_rect = egui::Rect::from_min_size(
                            egui::pos2(
                                ref_rect.min.x + c as f32 * preview_scale,
                                ref_rect.min.y + r as f32 * preview_scale,
                            ),
                            egui::vec2(preview_scale, preview_scale),
                        );
                        painter.rect_filled(px_rect, 0.0, color);
                    }
                }
            }
        }

        if is_active {
            painter.rect_stroke(
                ref_rect,
                0.0,
                egui::Stroke::new(2.0, color),
                egui::epaint::StrokeKind::Outside,
            );
        }

        // Context menu on right-click; also use this response for left-click
        // layer selection — ui.interact() consumes the click from the parent
        // response, so click_pos would be None for ref previews.
        let interact_id = state.keyed(Slot::RefLayerCtx, (edit_idx, ref_idx));
        let ref_response = ui.interact(ref_rect, interact_id, egui::Sense::click());

        if ref_response.clicked() {
            state.mode = EditMode::LayerMove {
                item_idx: edit_idx,
                layer_idx: ref_idx,
            };
        }

        ref_response.context_menu(|ui| {
            if subglyph_context_menu(ui) {
                inline_ref_action = Some(ref_idx);
            }
        });

        px += ref_size.x + 4.0 * zoom;
    }

    // Individual point previews (X marks at same preview scale)
    let num_refs = body.refs.len();
    for (pi, _point) in body.points.iter().enumerate() {
        let layer_idx = num_refs + pi;
        let point_size = egui::vec2(palette_cell, palette_cell);
        let point_rect = egui::Rect::from_min_size(egui::pos2(px, panel_y), point_size);

        let is_active = matches!(
            state.mode,
            EditMode::LayerMove { item_idx, layer_idx: li } if item_idx == edit_idx && li == layer_idx
        );

        let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer_idx);
        painter.rect_filled(point_rect, 0.0, pal.grid_bg);
        grid_render::draw_point_x_mark(painter, point_rect, color, pal.grid_bg);

        if is_active {
            painter.rect_stroke(
                point_rect,
                0.0,
                egui::Stroke::new(2.0, color),
                egui::epaint::StrokeKind::Outside,
            );
        }

        if let Some(cp) = click_pos
            && point_rect.contains(cp)
        {
            state.mode = EditMode::LayerMove {
                item_idx: edit_idx,
                layer_idx,
            };
        }

        px += point_size.x + 4.0 * zoom;
    }

    // --- Row 2+: Shape palette or point name ---
    if let EditMode::GlyphEdit { selected_shape, .. } = &mut state.mode {
        let palette_y = panel_y + prh + 4.0;
        let shift_held = ui.input(|i| i.modifiers.shift);
        draw_inline_palette(
            ui,
            painter,
            editor,
            panel_x,
            palette_y,
            selected_shape,
            click_pos,
            palette_cell,
            &pal,
            shift_held,
        );
    } else if let EditMode::LayerMove { layer_idx, .. } = &state.mode {
        let num_refs = body.refs.len();
        let label_y = panel_y + prh + 4.0;
        let layer_color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, *layer_idx);
        if *layer_idx < num_refs {
            // Show the resolved alternative name if it differs from the source ref.
            if let Some(comp) = composite
                && let Some(layer) = comp.layers.iter().find(|l| l.ref_idx == *layer_idx) {
                    let source_name = &body.refs[*layer_idx].name;
                    if layer.resolved_name != *source_name {
                        painter.text(
                            egui::pos2(panel_x, label_y),
                            egui::Align2::LEFT_TOP,
                            &layer.resolved_name,
                            egui::FontId::monospace(16.0_f32.max(palette_cell * 0.8)),
                            layer_color,
                        );
                    }
                }
        } else if let Some(point) = body.points.get(layer_idx - num_refs) {
            // How many glyphs the shadow behind the grid is made of — the label
            // is the only place that says the dim shape is more than one glyph.
            let label = match shadow {
                Some(s) => format!("{} \u{00d7}{}", point.position, s.count),
                None => point.position.clone(),
            };
            painter.text(
                egui::pos2(panel_x, label_y),
                egui::Align2::LEFT_TOP,
                &label,
                egui::FontId::monospace(16.0_f32.max(palette_cell * 0.8)),
                layer_color,
            );
        }
    }

    InlineToolsResult {
        click_consumed,
        inline_ref: inline_ref_action,
    }
}

#[expect(clippy::too_many_arguments)]
fn draw_inline_palette(
    ui: &egui::Ui,
    painter: &egui::Painter,
    editor: EditorId,
    x: f32,
    y: f32,
    selected_shape: &mut pixel::PixelShape,
    click_pos: Option<egui::Pos2>,
    cell_size: f32,
    pal: &Palette,
    shift_held: bool,
) {
    use crate::editor::glyph_widget::{
        all_valid_shapes, draw_pixel_cell_colored, palette_row_col, palette_rows, PALETTE_COLS,
    };

    let shapes = all_valid_shapes();
    let cell = cell_size;
    let num_rows = palette_rows();

    let palette_rect = egui::Rect::from_min_size(
        egui::pos2(x, y),
        egui::vec2(PALETTE_COLS as f32 * cell, num_rows as f32 * cell),
    );
    let hover_on_palette = ui.input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|hp| palette_rect.contains(hp))
    });
    if let Some(step) =
        crate::editor::document_view::interceptor_scroll_step(ui.ctx(), editor, hover_on_palette)
            && let Some(cur_idx) = shapes.iter().position(|s| s.shape_id() == selected_shape.shape_id()) {
                let next = (cur_idx as i32 + step).clamp(0, shapes.len() as i32 - 1) as usize;
                let next_shape = shapes[next];
                *selected_shape = pixel::PixelShape::new(next_shape.shape_id(), next_shape.is_filled());
            }
    ui.ctx().data_mut(|d| {
        d.insert_temp(editor.key(Slot::ShapePaletteHover), hover_on_palette);
    });

    for (i, shape) in shapes.iter().enumerate() {
        let (row, col) = palette_row_col(i);
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x + col as f32 * cell, y + row as f32 * cell),
            egui::vec2(cell, cell),
        );

        let is_selected = shape.shape_id() == selected_shape.shape_id();
        let bg = if is_selected {
            pal.shape_palette_selected_bg
        } else {
            pal.shape_palette_bg
        };
        painter.rect_filled(cell_rect, 1.0, bg);
        let apply_shift = shift_held && !shape.is_empty();
        let display_filled = shape.is_filled() ^ apply_shift;
        let px_color = if display_filled {
            pal.pixel_filled
        } else {
            grid_render::apply_opacity(pal.pixel_filled, UNFILLED_OPACITY)
        };
        draw_pixel_cell_colored(painter, cell_rect.shrink(2.0), *shape, px_color);

        if is_selected {
            painter.rect_stroke(
                cell_rect,
                1.0,
                egui::Stroke::new(1.5, pal.shape_palette_selected_stroke),
                egui::epaint::StrokeKind::Outside,
            );
        }

        if let Some(cp) = click_pos
            && cell_rect.contains(cp)
        {
            if shape.is_slant_pair() && selected_shape.shape_id() == shape.shape_id() {
                let pair = shape.slant_direction_pair();
                *selected_shape = if shift_held { pair.with_fill_toggled() } else { pair };
            } else if shape.is_slant_pair()
                && selected_shape.shape_id() == shape.slant_direction_pair().shape_id()
            {
                let target = if shift_held { shape.with_fill_toggled() } else { *shape };
                *selected_shape = target;
            } else {
                let new_fill = shape.is_filled() ^ (shift_held && !shape.is_empty());
                *selected_shape = pixel::PixelShape::new(shape.shape_id(), new_fill);
            }
        }
    }
}
