use std::collections::HashMap;

use crate::document::{Document, DocumentItem, NamePartsMap};
use crate::editor::colors::Palette;
use crate::editor::document_view::{INLINE_PALETTE_CELL, PREVIEW_SCALE, UNFILLED_OPACITY};
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
    inherited_count: usize,
    step: i32,
) {
    let layer_count = body.refs.len() + body.points.len() + inherited_count;
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

// Painting parameters plus the resolution tables the tools read.
#[expect(clippy::too_many_arguments)]
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
    let ppp = ui.ctx().pixels_per_point();
    let preview_scale = PREVIEW_SCALE * zoom;
    let palette_cell = INLINE_PALETTE_CELL * zoom;
    let composite = composites.get(&edit_idx);
    let max_ph = preview_max_height(body, composite, named_glyphs, name_parts);
    let prh = preview_row_height(zoom_level, max_ph);

    // Compute panel bounding rect to detect if click is consumed
    let palette_rows = crate::editor::glyph_widget::palette_rows();
    let palette_height = palette_rows as f32 * palette_cell;
    let panel_total_height = prh + 4.0 + palette_height;
    let palette_cols = crate::editor::glyph_widget::palette_cols();
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
    ) {
        let inherited_count = composite.map_or(0, |c| c.inherited_anchors.len());
        cycle_layer_mode(state, body, edit_idx, inherited_count, step);
    }

    // --- Row 0-1: 2x pixelated previews (composite + subglyphs) ---
    let mut px = panel_x;

    let is_pixel_mode =
        matches!(state.mode, EditMode::GlyphEdit { item_idx, .. } if item_idx == edit_idx);

    // Full composite preview (always at exact preview_scale per *logical*
    // pixel — the composite and the own grid are both in this glyph's own
    // `scale N` subcells).
    let own_cell = preview_scale / body.scale.max(1) as f32;
    let full_preview_size = if let Some(comp) = composite {
        egui::vec2(comp.width as f32 * own_cell, comp.height as f32 * own_cell)
    } else if let Some(grid) = &body.pixels {
        egui::vec2(grid.width as f32 * own_cell, grid.height as f32 * own_cell)
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
        ppp,
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
            ref_composite::resolve_ref_name_for_view(&gref.name, named_glyphs, name_parts);
        // A `scale N` glyph's grid is N times finer than its logical size, so
        // the thumbnail is sized from the logical extent and its subcells are
        // drawn at a correspondingly smaller cell size — otherwise the subglyph
        // shows up N times larger than the glyph it is a part of.
        let ref_cell = resolved.map_or(preview_scale, |rg| preview_scale / rg.scale.max(1) as f32);
        let ref_size = if let Some(rg) = resolved {
            egui::vec2(
                (rg.grid.width as f32 * ref_cell).max(pixel_preview_w),
                (rg.grid.height as f32 * ref_cell).max(pixel_preview_h),
            )
        } else {
            egui::vec2(pixel_preview_w, pixel_preview_h)
        };

        let ref_rect = egui::Rect::from_min_size(egui::pos2(px, panel_y), ref_size);

        // Publish this thumbnail's rect for the in-crate GUI test harness, so
        // tests can click it without hand-replicating the layout math above.
        #[cfg(test)]
        crate::editor::harness::capture_ref_rect(ui.ctx(), state.id(), edit_idx, ref_idx, ref_rect);

        let is_active = matches!(
            state.mode,
            EditMode::LayerMove { item_idx, layer_idx } if item_idx == edit_idx && layer_idx == ref_idx
        );

        let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, ref_idx);
        painter.rect_filled(ref_rect, 0.0, pal.grid_bg);
        if let Some(rg) = resolved {
            let geom = grid_render::PreviewGeom {
                rect: ref_rect,
                cell_w: ref_cell,
                cell_h: ref_cell,
                ppp,
            };
            grid_render::blit_preview(painter, &geom, &rg.grid, 0, 0, color);
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

    // Anchors exposed through `inherit` refs: listed after the declared
    // points and selectable like them (label, anchor shadow), but colored
    // like the subglyph they come from, and not movable — they have no
    // document line of their own.
    let inherited: &[(crate::document::GlyphPoint, usize)] =
        composite.map_or(&[], |c| &c.inherited_anchors);
    let num_points = body.points.len();
    for (ii, (_point, src_ref)) in inherited.iter().enumerate() {
        let layer_idx = num_refs + num_points + ii;
        let point_size = egui::vec2(palette_cell, palette_cell);
        let point_rect = egui::Rect::from_min_size(egui::pos2(px, panel_y), point_size);

        let is_active = matches!(
            state.mode,
            EditMode::LayerMove { item_idx, layer_idx: li } if item_idx == edit_idx && li == layer_idx
        );

        let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, *src_ref);
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
            &mut state.shape_rotation,
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
                && let Some(layer) = comp.layers.iter().find(|l| l.ref_idx == *layer_idx)
            {
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
        } else if let Some((point, src_ref)) = composite.and_then(|c| {
            c.inherited_anchors
                .get(layer_idx - num_refs - body.points.len())
        }) {
            // An inherited anchor names its source alongside the position, in
            // the source subglyph's color.
            let source = body
                .refs
                .get(*src_ref)
                .map_or("", |gref| gref.name.as_str());
            let label = match shadow {
                Some(s) => format!("{} ({source}) \u{00d7}{}", point.position, s.count),
                None => format!("{} ({source})", point.position),
            };
            painter.text(
                egui::pos2(panel_x, label_y),
                egui::Align2::LEFT_TOP,
                &label,
                egui::FontId::monospace(16.0_f32.max(palette_cell * 0.8)),
                ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, *src_ref),
            );
        }
    }

    InlineToolsResult {
        click_consumed,
        inline_ref: inline_ref_action,
    }
}

/// The shape palette: one cell per rotation orbit, every cell drawn at the
/// editor's current rotation. A plain wheel notch turns the whole palette (and
/// with it the shape under the cursor); shift+wheel walks the cells, which is
/// why the rotation lives in [`EditorState`] and not in the selected shape.
#[expect(clippy::too_many_arguments)]
fn draw_inline_palette(
    ui: &egui::Ui,
    painter: &egui::Painter,
    editor: EditorId,
    x: f32,
    y: f32,
    selected_shape: &mut pixel::PixelShape,
    rotation: &mut u32,
    click_pos: Option<egui::Pos2>,
    cell_size: f32,
    pal: &Palette,
    shift_held: bool,
) {
    use crate::editor::glyph_widget::{
        draw_pixel_cell_colored, palette_cols, palette_row_col, palette_rows, palette_shapes,
        rotate_shape, shape_orbit, wheel_step_shape,
    };

    let shapes = palette_shapes();
    let cell = cell_size;
    let num_rows = palette_rows();

    let palette_rect = egui::Rect::from_min_size(
        egui::pos2(x, y),
        egui::vec2(palette_cols() as f32 * cell, num_rows as f32 * cell),
    );
    let hover_on_palette = ui.input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|hp| palette_rect.contains(hp))
    });
    if let Some(step) =
        crate::editor::document_view::interceptor_scroll_step(ui.ctx(), editor, hover_on_palette)
    {
        wheel_step_shape(selected_shape, rotation, step, shift_held);
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(editor.key(Slot::ShapePaletteHover), hover_on_palette);
    });

    let selected_cell = shape_orbit(*selected_shape).map(|(idx, _)| idx);
    for (i, rep) in shapes.iter().enumerate() {
        let (row, col) = palette_row_col(i);
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x + col as f32 * cell, y + row as f32 * cell),
            egui::vec2(cell, cell),
        );

        let shape = &rotate_shape(*rep, *rotation as i32);
        let is_selected = selected_cell == Some(i);
        let bg = if is_selected {
            pal.shape_palette_selected_bg
        } else {
            pal.shape_palette_bg
        };
        painter.rect_filled(cell_rect, 1.0, bg);
        let apply_shift = shift_held && !shape.is_empty();
        // The fill is per-cell palette data, *except* on the selected cell,
        // which shows the selection's own fill — that is what makes clicking it
        // again (see below) visibly alternate between solid and dimmed.
        let cell_filled = if is_selected {
            selected_shape.is_filled()
        } else {
            shape.is_filled()
        };
        let display_filled = cell_filled ^ apply_shift;

        #[cfg(test)]
        crate::editor::harness::capture_palette_rect(
            ui.ctx(),
            editor,
            i,
            cell_rect,
            display_filled,
        );

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
            // Clicking the cell that is already selected flips the fill instead
            // of re-selecting it: a shape and its complement are then one click
            // apart, without holding shift. Any other cell arrives with the
            // fill the palette draws it with (shift inverts that, as on the grid).
            let new_fill = if selected_shape.shape_id() == shape.shape_id() {
                !selected_shape.is_filled()
            } else {
                shape.is_filled() ^ (shift_held && !shape.is_empty())
            };
            *selected_shape = pixel::PixelShape::new(shape.shape_id(), new_fill);
        }
    }
}
