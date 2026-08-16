//! The minimap: the whole document as one strip of pixels beside the editor.
//!
//! # It is always dark
//!
//! The strip's ground, its grid and its viewport box are one set of colors in
//! both themes, so everything drawn on it comes from
//! [`Palette::dark`] — including the syntax colors, which are repeated from
//! visual lines that resolved them against the *reader's* theme. In light mode
//! those are dark inks, and a dark ink on this strip is nothing at all; hence
//! [`Palette::dark_equivalent`]. Which theme the document is read in is the
//! document's business, not this widget's.
//!
//! # Landmarks
//!
//! A row is one cell — the pane's zoom level in pixels — of texture: two
//! characters of text, or one pixel of a glyph's grid. A `#`/`##` heading is
//! the exception. It is drawn as readable text at a fixed size in a row as tall
//! as that text, so the file can be navigated by section; which level it is, is
//! left to the `#` prefix the line already carries, because the point of the
//! fixed size is that both levels stay legible. `###` is not one of these —
//! three tiers of landmark is texture again.

use std::collections::HashMap;

use crate::document::{Document, DocumentItem, GlyphBody};
use crate::editor::colors::Palette;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};

use super::document_view::{VLineKind, VisualLine};
use super::grid_render::{PreviewGeom, apply_opacity, blit_preview};

/// The deepest heading the minimap marks. See `draw_minimap`.
const MINIMAP_HEADING_LEVELS: u8 = 2;

/// Type size of a minimap landmark, in points and independent of the pane's
/// zoom: the minimap is a fixed-width strip, so a label that scaled with the
/// zoom would only run out of it sooner.
const MINIMAP_HEADING_SIZE: f32 = 16.0;

/// Whether this visual line is drawn as a landmark — readable text in a row of
/// its own — rather than as texture. See `draw_minimap`.
fn is_landmark(vl: &VisualLine) -> bool {
    matches!(vl.kind, VLineKind::Text(_))
        && vl.col_offset == 0
        && vl
            .heading
            .is_some_and(|h| h.level <= MINIMAP_HEADING_LEVELS)
}

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

    // A landmark's row is as tall as the text drawn in it, so the label sits in
    // space of its own rather than over the rows below it — a `#` at 1× is
    // sixteen ordinary rows' worth of strip, which is the prominence it is for.
    let heading_row = snap(MINIMAP_HEADING_SIZE).max(cell);
    let row_height = |vl: &VisualLine| {
        if is_landmark(vl) { heading_row } else { cell }
    };
    let mm_total: f32 = vlines.iter().map(row_height).sum();
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
    // Dark in both themes; see the module docs.
    let themed = Palette::get(ui);
    let pal = Palette::dark();
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

    // Landmark text, collected as the rows are walked; see the module docs.
    let mut labels: Vec<(egui::Pos2, &str)> = Vec::new();

    let mut y = y0;
    for vl in vlines {
        let h = row_height(vl);
        let sy = snap(y);
        y += h;
        if sy + h <= available.min.y || sy >= available.max.y {
            continue;
        }

        match &vl.kind {
            VLineKind::Text(text) if is_landmark(vl) => {
                labels.push((egui::pos2(x0, snap(sy + h * 0.5)), text.as_str()));
            }
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
                        let [r, g, b, _] = themed.dark_equivalent(vl.color).to_array();
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

    // After the mesh, which is one batched shape covering every other line.
    for (pos, text) in labels {
        painter.text(
            pos,
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(MINIMAP_HEADING_SIZE),
            pal.text_heading,
        );
    }

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
        return Some(pointer_scroll_target(
            pos.y - available.min.y,
            minimap_h,
            mm_total,
            total_height,
            viewport_height,
        ));
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

/// Where a click or drag at `pointer_y` — an offset from the top of the strip —
/// asks the document to scroll to.
///
/// A strip taller than its panel scrolls with the document (`mm_scroll` in
/// `draw_minimap`), so *which* content row sits under a fixed pointer depends on
/// where the document already is. Answering from the scroll of the frame the
/// pointer arrived in therefore makes every drag event one step towards the row
/// asked for rather than the row itself: the pointer holds still, the strip
/// slides under it, and the view creeps in over as many frames as the mouse
/// happens to send events. So the scroll is *solved for* instead — the one whose
/// own strip offset puts `pointer_y` on the very row that scroll centers, which
/// is a fixed point of that iteration and so is reached in one event.
///
/// The solution exists as long as the strip's window shows more of the document
/// than the viewport does, which is what a compressed strip is; when it does not
/// (the denominator vanishes), the un-solved mapping is the answer, and it is
/// exact there anyway because such a strip barely scrolls.
fn pointer_scroll_target(
    pointer_y: f32,
    minimap_h: f32,
    mm_total: f32,
    total_height: f32,
    viewport_height: f32,
) -> f32 {
    let p = pointer_y.clamp(0.0, minimap_h);
    let mm = mm_total.max(1.0);
    let max_scroll = (total_height - viewport_height).max(0.0);
    let strip_range = (mm_total - minimap_h).max(0.0);

    // target = (p + target/max_scroll * strip_range) / mm * total - viewport/2
    let numer = p / mm * total_height - viewport_height / 2.0;
    let denom = if max_scroll > 0.0 {
        1.0 - strip_range * total_height / (max_scroll * mm)
    } else {
        1.0
    };
    let target = if denom > 1e-4 { numer / denom } else { numer };
    target.clamp(0.0, max_scroll)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_preview_bitmap(
    painter: &egui::Painter,
    rect: egui::Rect,
    body: &GlyphBody,
    composite: Option<&GlyphComposite>,
    _named_glyphs: &HashMap<String, ResolvedGlyph>,
    highlight_ref: Option<usize>,
    pal: &Palette,
    ppp: f32,
) {
    painter.rect_filled(rect, 0.0, pal.grid_bg);

    let (total_w, total_h) = if let Some(comp) = composite {
        (comp.width as f32, comp.height as f32)
    } else if let Some(grid) = &body.pixels {
        (grid.width as f32, grid.height as f32)
    } else {
        return;
    };
    let geom = PreviewGeom {
        rect,
        cell_w: rect.width() / total_w,
        cell_h: rect.height() / total_h,
        ppp,
    };

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
                    &geom,
                    &layer.grid,
                    layer.offset_row,
                    layer.offset_col,
                    color,
                );
            }
        };

        if let Some(hi_ref) = highlight_ref {
            render_layers(painter, Some(hi_ref));
            if let Some(grid) = &body.pixels {
                blit_preview(
                    painter,
                    &geom,
                    grid,
                    own_off_r,
                    own_off_c,
                    apply_opacity(pal.grid_on, 0.4),
                );
            }
            if let Some(layer) = comp.layers.iter().find(|l| l.ref_idx == hi_ref) {
                let color =
                    ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx);
                blit_preview(
                    painter,
                    &geom,
                    &layer.grid,
                    layer.offset_row,
                    layer.offset_col,
                    color,
                );
            }
        } else {
            render_layers(painter, None);
            if let Some(grid) = &body.pixels {
                blit_preview(painter, &geom, grid, own_off_r, own_off_c, pal.grid_on);
            }
        }
    } else if let Some(grid) = &body.pixels {
        blit_preview(painter, &geom, grid, 0, 0, pal.grid_on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip offset `draw_minimap` computes for a given document scroll.
    fn mm_scroll(scroll_y: f32, minimap_h: f32, mm_total: f32, total: f32, viewport: f32) -> f32 {
        let max_doc_scroll = (total - viewport).max(0.0);
        let frac = if max_doc_scroll > 0.0 {
            (scroll_y / max_doc_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if mm_total > minimap_h {
            frac * (mm_total - minimap_h)
        } else {
            0.0
        }
    }

    /// Reading the pointer against the scroll the frame happens to be at: the
    /// step this used to take once per drag event.
    fn one_step(
        pointer_y: f32,
        scroll_y: f32,
        minimap_h: f32,
        mm_total: f32,
        total: f32,
        viewport: f32,
    ) -> f32 {
        let off = mm_scroll(scroll_y, minimap_h, mm_total, total, viewport);
        let content_y = (pointer_y + off).clamp(0.0, mm_total);
        let doc_y = content_y / mm_total.max(1.0) * total;
        (doc_y - viewport / 2.0).clamp(0.0, (total - viewport).max(0.0))
    }

    // A tall document: 4000 visual rows of strip against a 600 pt panel, so the
    // strip scrolls and the reading feeds back on itself.
    const MM_H: f32 = 600.0;
    const MM_TOTAL: f32 = 4000.0;
    const TOTAL: f32 = 96000.0;
    const VIEWPORT: f32 = 800.0;

    #[test]
    fn drag_target_is_a_fixed_point() {
        for pointer_y in [0.0, 37.0, 150.0, 300.0, 455.0, 599.0] {
            let target = pointer_scroll_target(pointer_y, MM_H, MM_TOTAL, TOTAL, VIEWPORT);
            // Once there, the same pointer must ask for the same place — that is
            // what makes one drag event enough.
            let again = one_step(pointer_y, target, MM_H, MM_TOTAL, TOTAL, VIEWPORT);
            assert!(
                (again - target).abs() <= 1.0,
                "pointer {pointer_y}: settled at {target} but re-reads as {again}"
            );
        }
    }

    #[test]
    fn drag_lands_in_one_event_not_many() {
        // Dragging from the top of the strip down to two thirds of it, with the
        // pointer then held still: the answer must not depend on how many events
        // the mouse sends.
        let pointer_y = 400.0;
        let one = pointer_scroll_target(pointer_y, MM_H, MM_TOTAL, TOTAL, VIEWPORT);
        // The step contracts by only ~0.86 per event here, which is the crawl
        // itself: hundreds of events to arrive where one should have.
        let mut scroll = 0.0;
        for _ in 0..500 {
            scroll = one_step(pointer_y, scroll, MM_H, MM_TOTAL, TOTAL, VIEWPORT);
        }
        assert!(
            (one - scroll).abs() <= 1.0,
            "one event gives {one}, the old iteration converges to {scroll}"
        );
    }

    #[test]
    fn a_strip_that_fits_is_read_directly() {
        // No strip scroll, so there is nothing to solve and the mapping is the
        // plain one.
        let (mm_h, mm_total, total, viewport) = (600.0, 400.0, 8000.0, 800.0);
        let target = pointer_scroll_target(200.0, mm_h, mm_total, total, viewport);
        let plain = one_step(200.0, 0.0, mm_h, mm_total, total, viewport);
        assert!((target - plain).abs() <= 0.01);
    }

    #[test]
    fn the_ends_of_the_strip_reach_the_ends_of_the_document() {
        let top = pointer_scroll_target(0.0, MM_H, MM_TOTAL, TOTAL, VIEWPORT);
        let bottom = pointer_scroll_target(MM_H, MM_H, MM_TOTAL, TOTAL, VIEWPORT);
        assert_eq!(top, 0.0);
        assert_eq!(bottom, TOTAL - VIEWPORT);
    }
}
