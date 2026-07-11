use std::collections::HashMap;

use crate::document::{Document, DocumentItem, GlyphBody};
use crate::editor::colors::Palette;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};

use super::document_view::{VLineKind, VisualLine};
use super::grid_render::{apply_opacity, blit_preview};

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_minimap(
    ui: &mut egui::Ui,
    vlines: &[VisualLine],
    doc: &Document,
    composites: &HashMap<usize, GlyphComposite>,
    total_height: f32,
    scroll_y: f32,
    viewport_height: f32,
    zoom_level: u32,
) -> Option<f32> {
    let available = ui.available_rect_before_wrap();
    let minimap_h = available.height();
    let minimap_w = available.width();

    if total_height <= 0.0 || minimap_h <= 0.0 || vlines.is_empty() {
        return None;
    }

    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let cell = snap(zoom_level as f32).max(zoom_level as f32 / ppp);

    let mm_total = vlines.len() as f32 * cell;
    let max_doc_scroll = (total_height - viewport_height).max(0.0);
    let scroll_frac = if max_doc_scroll > 0.0 {
        (scroll_y / max_doc_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mm_scroll = if mm_total > minimap_h {
        scroll_frac * (mm_total - minimap_h)
    } else {
        0.0
    };

    let response = ui.allocate_rect(available, egui::Sense::click_and_drag());
    let painter = ui.painter_at(available);
    let pal = Palette::get(ui);
    painter.rect_filled(available, 0.0, pal.minimap_bg);

    let mut mesh = egui::Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    let emit = |mesh: &mut egui::Mesh, x: f32, y: f32, w: f32, h: f32, c: egui::Color32| {
        let idx = mesh.vertices.len() as u32;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(x, y),
            uv,
            color: c,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(x + w, y),
            uv,
            color: c,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(x + w, y + h),
            uv,
            color: c,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(x, y + h),
            uv,
            color: c,
        });
        mesh.indices
            .extend_from_slice(&[idx, idx + 1, idx + 2, idx, idx + 2, idx + 3]);
    };

    let x0 = snap(available.min.x + 1.0);
    let y0 = available.min.y - mm_scroll;

    for (i, vl) in vlines.iter().enumerate() {
        let sy = snap(y0 + i as f32 * cell);
        if sy + cell <= available.min.y || sy >= available.max.y {
            continue;
        }

        match &vl.kind {
            VLineKind::Text(text) => {
                let chars: Vec<char> = text.chars().collect();
                for (j, pair) in chars.chunks(2).enumerate() {
                    let x = snap(x0 + j as f32 * cell);
                    if x >= available.max.x {
                        break;
                    }
                    let a = pair.first().is_some_and(|c| !c.is_whitespace());
                    let b = pair.get(1).is_some_and(|c| !c.is_whitespace());
                    if a || b {
                        let alpha: u8 = if a && b { 180 } else { 90 };
                        let [r, g, b, _] = vl.color.to_array();
                        emit(
                            &mut mesh,
                            x,
                            sy,
                            cell,
                            cell,
                            egui::Color32::from_rgba_unmultiplied(r, g, b, alpha),
                        );
                    }
                }
            }
            VLineKind::GridRow {
                item_idx,
                row,
                own_width,
                own_height,
                extent,
                ..
            } => {
                let grid = match doc.items.get(*item_idx) {
                    Some(DocumentItem::Glyph { body, .. }) => body.pixels.as_ref(),
                    _ => None,
                };
                let comp = composites.get(item_idx);
                let in_own_row = *row >= 0 && *row < *own_height as i16;
                for dc in extent.left..extent.right {
                    let disp_c = (dc - extent.left) as f32;
                    let x = snap(x0 + disp_c * cell);
                    if x >= available.max.x {
                        break;
                    }
                    let in_own_col = dc >= 0 && dc < *own_width as i16;
                    let own_filled = in_own_row
                        && in_own_col
                        && grid.is_some_and(|g| g.get(*row as u16, dc as u16).is_filled());
                    let ref_filled = !own_filled
                        && comp.is_some_and(|comp| {
                            comp.any_layer_filled_at(
                                comp.own_offset_row + *row,
                                comp.own_offset_col + dc,
                            )
                        });
                    let in_own = in_own_row && in_own_col;
                    let color = if own_filled || ref_filled {
                        pal.grid_on
                    } else if in_own {
                        pal.grid_off
                    } else {
                        pal.grid_ext_off
                    };
                    emit(&mut mesh, x, sy, cell, cell, color);
                }
            }
        }
    }

    painter.add(egui::Shape::mesh(mesh));

    let vp_mm_top = scroll_y / total_height.max(1.0) * mm_total;
    let vp_mm_h = viewport_height / total_height.max(1.0) * mm_total;
    let vp_sy = snap(available.min.y + vp_mm_top - mm_scroll);
    let vp_sh = snap(vp_mm_h.max(4.0));
    let vp_rect = egui::Rect::from_min_size(
        egui::pos2(available.min.x, vp_sy),
        egui::vec2(minimap_w, vp_sh),
    )
    .intersect(available);

    if vp_rect.is_positive() {
        painter.rect_filled(vp_rect, 0.0, pal.minimap_viewport_fill);
        painter.rect_stroke(
            vp_rect,
            0.0,
            egui::Stroke::new(1.0, pal.minimap_viewport_stroke),
            egui::epaint::StrokeKind::Inside,
        );
    }

    if (response.clicked() || response.dragged())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let mm_content_y = ((pos.y - available.min.y) + mm_scroll).clamp(0.0, mm_total);
        let doc_y = mm_content_y / mm_total.max(1.0) * total_height;
        let max_scroll = (total_height - viewport_height).max(0.0);
        let target = (doc_y - viewport_height / 2.0).clamp(0.0, max_scroll);
        return Some(target);
    }

    if response.hovered() {
        let delta_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if delta_y.abs() > 0.1 {
            ui.ctx().input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            let scale = total_height / mm_total.max(1.0);
            let max_scroll = (total_height - viewport_height).max(0.0);
            let target = (scroll_y - delta_y * scale).clamp(0.0, max_scroll);
            return Some(target);
        }
    }

    None
}

pub(crate) fn draw_preview_bitmap(
    painter: &egui::Painter,
    rect: egui::Rect,
    body: &GlyphBody,
    composite: Option<&GlyphComposite>,
    _named_glyphs: &HashMap<String, ResolvedGlyph>,
    highlight_ref: Option<usize>,
    pal: &Palette,
) {
    painter.rect_filled(rect, 0.0, pal.grid_bg);

    let (total_w, total_h) = if let Some(comp) = composite {
        (comp.width as f32, comp.height as f32)
    } else if let Some(grid) = &body.pixels {
        (grid.width as f32, grid.height as f32)
    } else {
        return;
    };
    let cell_w = rect.width() / total_w;
    let cell_h = rect.height() / total_h;

    if let Some(comp) = composite {
        let own_off_r = comp.own_offset_row;
        let own_off_c = comp.own_offset_col;

        let render_layers = |painter: &egui::Painter, skip_ref: Option<usize>| {
            for layer in &comp.layers {
                if Some(layer.ref_idx) == skip_ref {
                    continue;
                }
                let color =
                    ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx);
                let opacity = if highlight_ref.is_some() && Some(layer.ref_idx) != highlight_ref {
                    0.4
                } else {
                    1.0
                };
                let color = apply_opacity(color, opacity);
                blit_preview(
                    painter,
                    rect,
                    &layer.grid,
                    layer.offset_row,
                    layer.offset_col,
                    color,
                    cell_w,
                    cell_h,
                );
            }
        };

        if let Some(hi_ref) = highlight_ref {
            render_layers(painter, Some(hi_ref));
            if let Some(grid) = &body.pixels {
                blit_preview(
                    painter,
                    rect,
                    grid,
                    own_off_r,
                    own_off_c,
                    apply_opacity(pal.grid_on, 0.4),
                    cell_w,
                    cell_h,
                );
            }
            if let Some(layer) = comp.layers.iter().find(|l| l.ref_idx == hi_ref) {
                let color =
                    ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx);
                blit_preview(
                    painter,
                    rect,
                    &layer.grid,
                    layer.offset_row,
                    layer.offset_col,
                    color,
                    cell_w,
                    cell_h,
                );
            }
        } else {
            render_layers(painter, None);
            if let Some(grid) = &body.pixels {
                blit_preview(
                    painter,
                    rect,
                    grid,
                    own_off_r,
                    own_off_c,
                    pal.grid_on,
                    cell_w,
                    cell_h,
                );
            }
        }
    } else if let Some(grid) = &body.pixels {
        blit_preview(painter, rect, grid, 0, 0, pal.grid_on, cell_w, cell_h);
    }
}
