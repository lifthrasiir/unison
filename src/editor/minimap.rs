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
//!
//! # The strip is not the document to scale
//!
//! Because of the two above — and of the reference chart strip, which is over a
//! hundred points of document that this widget draws as nothing at all — the
//! ratio between a document row and its strip row is not the same for every
//! row, and so the two coordinates cannot be converted by scaling the file's
//! two heights against each other. Everything that has to place one in the
//! other — the viewport box, a click, the wheel — goes through [`MinimapMap`],
//! which walks the same rows the strip is drawn from.

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

/// The row-by-row correspondence between the document and the strip.
///
/// The two are not proportional, so no single ratio converts between them. A
/// row is one cell of strip — or one landmark row — whatever height the
/// document gives it, and a reference chart strip is over a hundred points of
/// document that the minimap draws as nothing at all. The viewport box, a
/// click and the wheel all go through this piecewise-linear map instead, built
/// by walking the very rows the strip is drawn from.
struct MinimapMap {
    /// Cumulative document offsets, one per row plus the end.
    doc: Vec<f32>,
    /// Cumulative strip offsets, one per entry in `doc`.
    mm: Vec<f32>,
}

impl MinimapMap {
    /// From `(document height, strip height)` per visual line, in order.
    fn from_rows(rows: impl Iterator<Item = (f32, f32)>) -> Self {
        let (mut doc, mut mm) = (vec![0.0], vec![0.0]);
        let (mut dy, mut my) = (0.0, 0.0);
        for (dh, mh) in rows {
            dy += dh;
            my += mh;
            doc.push(dy);
            mm.push(my);
        }
        Self { doc, mm }
    }

    fn doc_total(&self) -> f32 {
        *self.doc.last().unwrap_or(&0.0)
    }

    fn mm_total(&self) -> f32 {
        *self.mm.last().unwrap_or(&0.0)
    }

    /// The strip height of row `i`.
    fn mm_row(&self, i: usize) -> f32 {
        self.mm[i + 1] - self.mm[i]
    }

    /// `v`, an offset along `from`, read off `to` — linearly inside the row it
    /// falls in. A row with no extent on `from` (a chart strip read off the
    /// strip side) is stepped over whole, which is what makes the strip's blank
    /// band still account for the document it stands for.
    fn convert(from: &[f32], to: &[f32], v: f32) -> f32 {
        let last = *from.last().unwrap_or(&0.0);
        let v = v.clamp(0.0, last);
        let i = from
            .partition_point(|&x| x <= v)
            .saturating_sub(1)
            .min(from.len().saturating_sub(2));
        let (f0, f1) = (from[i], from[i + 1]);
        let (t0, t1) = (to[i], to[i + 1]);
        if f1 > f0 {
            t0 + (v - f0) / (f1 - f0) * (t1 - t0)
        } else {
            t0
        }
    }

    fn doc_to_mm(&self, doc_y: f32) -> f32 {
        Self::convert(&self.doc, &self.mm, doc_y)
    }

    fn mm_to_doc(&self, mm_y: f32) -> f32 {
        Self::convert(&self.mm, &self.doc, mm_y)
    }
}

/// How far the strip itself has scrolled inside its panel: a strip taller than
/// the panel is walked end to end as the document is, so the last row is
/// reachable.
fn strip_scroll(
    scroll_y: f32,
    minimap_h: f32,
    mm_total: f32,
    total_height: f32,
    viewport_height: f32,
) -> f32 {
    let max_doc_scroll = (total_height - viewport_height).max(0.0);
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_minimap(
    ui: &mut egui::Ui,
    vlines: &[VisualLine],
    doc: &Document,
    composites: &HashMap<usize, GlyphComposite>,
    row_height: f32,
    grid_cell: f32,
    scroll_y: f32,
    viewport_height: f32,
    zoom_level: u32,
) -> Option<f32> {
    let available = ui.available_rect_before_wrap();
    let minimap_h = available.height();
    let minimap_w = available.width();

    if minimap_h <= 0.0 || vlines.is_empty() {
        return None;
    }

    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let cell = snap(zoom_level as f32).max(zoom_level as f32 / ppp);

    // A landmark's row is as tall as the text drawn in it, so the label sits in
    // space of its own rather than over the rows below it — a `#` at 1× is
    // sixteen ordinary rows' worth of strip, which is the prominence it is for.
    let heading_row = snap(MINIMAP_HEADING_SIZE).max(cell);
    let map = MinimapMap::from_rows(vlines.iter().map(|vl| {
        let mm = match vl.kind {
            // A chart strip is not the document, so the minimap leaves its
            // band blank rather than inventing a mark for it.
            VLineKind::RefImage { .. } => 0.0,
            _ if is_landmark(vl) => heading_row,
            _ => cell,
        };
        (vl.height(row_height, grid_cell), mm)
    }));
    let total_height = map.doc_total();
    let mm_total = map.mm_total();
    if total_height <= 0.0 {
        return None;
    }
    let mm_scroll = strip_scroll(scroll_y, minimap_h, mm_total, total_height, viewport_height);

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

    for (i, vl) in vlines.iter().enumerate() {
        let h = map.mm_row(i);
        let sy = snap(y0 + map.mm[i]);
        if h <= 0.0 || sy + h <= available.min.y || sy >= available.max.y {
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
            VLineKind::RefImage { .. } => {}
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
                        && grid.is_some_and(|g| g.get(*row as u16, dc as u16).is_bitmap_filled());
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

    let vp_mm_top = map.doc_to_mm(scroll_y);
    let vp_mm_h = map.doc_to_mm(scroll_y + viewport_height) - vp_mm_top;
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
            &map,
            total_height,
            viewport_height,
        ));
    }

    if response.hovered() {
        let delta_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if delta_y.abs() > 0.1 {
            ui.ctx().input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            // The wheel moves the strip under the pointer by `delta_y`, and
            // the document by however much document that is — which is what
            // the map answers, and is not one factor over the whole file.
            let max_scroll = (total_height - viewport_height).max(0.0);
            let target = map.mm_to_doc(map.doc_to_mm(scroll_y) - delta_y);
            return Some(target.clamp(0.0, max_scroll));
        }
    }

    None
}

/// Where a click or drag at `pointer_y` — an offset from the top of the strip —
/// asks the document to scroll to.
///
/// A strip taller than its panel scrolls with the document ([`strip_scroll`]),
/// so *which* content row sits under a fixed pointer depends on where the
/// document already is. Answering from the scroll of the frame the pointer
/// arrived in therefore makes every drag event one step towards the row asked
/// for rather than the row itself: the pointer holds still, the strip slides
/// under it, and the view creeps in over as many frames as the mouse happens to
/// send events. So the scroll is *solved for* instead — the one whose own strip
/// offset puts `pointer_y` on the very row that scroll centers, which is a
/// fixed point of that iteration and so is reached in one event.
///
/// The map is piecewise, so the fixed point is found rather than derived: the
/// step function is monotone in the scroll, and bisecting it to the precision
/// of the float is a handful of binary searches on a click.
fn pointer_scroll_target(
    pointer_y: f32,
    minimap_h: f32,
    map: &MinimapMap,
    total_height: f32,
    viewport_height: f32,
) -> f32 {
    let p = pointer_y.clamp(0.0, minimap_h);
    let max_scroll = (total_height - viewport_height).max(0.0);
    let mm_total = map.mm_total();
    // Where a document at `s` says the pointer is pointing.
    let step = |s: f32| {
        let off = strip_scroll(s, minimap_h, mm_total, total_height, viewport_height);
        let doc_y = map.mm_to_doc(p + off);
        (doc_y - viewport_height / 2.0).clamp(0.0, max_scroll)
    };

    let (mut lo, mut hi) = (0.0f32, max_scroll);
    for _ in 0..40 {
        let mid = lo + (hi - lo) * 0.5;
        if step(mid) >= mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // `lo` and not the midpoint: forty halvings leave the two adjacent, and the
    // low side is the one that reaches the ends of the document exactly.
    lo
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

    /// A strip whose rows are all one cell, for the scroll arithmetic that does
    /// not care which rows they are.
    fn uniform(mm_total: f32, total: f32) -> MinimapMap {
        let n = mm_total as usize;
        MinimapMap::from_rows((0..n).map(move |_| (total / n as f32, 1.0)))
    }

    /// Reading the pointer against the scroll the frame happens to be at: the
    /// step this used to take once per drag event.
    fn one_step(
        pointer_y: f32,
        scroll_y: f32,
        minimap_h: f32,
        map: &MinimapMap,
        total: f32,
        viewport: f32,
    ) -> f32 {
        let off = strip_scroll(scroll_y, minimap_h, map.mm_total(), total, viewport);
        let doc_y = map.mm_to_doc(pointer_y + off);
        (doc_y - viewport / 2.0).clamp(0.0, (total - viewport).max(0.0))
    }

    // A tall document: 4000 visual rows of strip against a 600 pt panel, so the
    // strip scrolls and the reading feeds back on itself.
    const MM_H: f32 = 600.0;
    const MM_TOTAL: f32 = 4000.0;
    const TOTAL: f32 = 96000.0;
    const VIEWPORT: f32 = 800.0;

    /// 40 text rows of 16 pt drawn as one 2 pt cell each, with a 110 pt chart
    /// strip — blank on the minimap — before every fourth of the first ten.
    fn with_charts() -> (MinimapMap, Vec<(f32, f32)>) {
        let mut rows = Vec::new();
        for i in 0..40 {
            if i < 10 && i % 4 == 0 {
                rows.push((crate::editor::ref_images::REF_IMAGE_ROW, 0.0));
            }
            rows.push((16.0, 2.0));
        }
        (MinimapMap::from_rows(rows.iter().copied()), rows)
    }

    #[test]
    fn the_viewport_box_sits_on_the_row_it_covers() {
        let (map, rows) = with_charts();
        // Every row boundary, not just a convenient one: a chart strip is over
        // a hundred points of document and no strip at all, so scaling the
        // whole file by one ratio puts the box rows away from its content.
        let (mut doc_y, mut mm_y) = (0.0, 0.0);
        for &(dh, mh) in &rows {
            assert!(
                (map.doc_to_mm(doc_y) - mm_y).abs() <= 0.01,
                "document {doc_y} reads as strip {} not {mm_y}",
                map.doc_to_mm(doc_y)
            );
            doc_y += dh;
            mm_y += mh;
        }
        assert!((map.doc_to_mm(doc_y) - mm_y).abs() <= 0.01);
    }

    #[test]
    fn a_blank_chart_band_is_still_the_document_it_stands_for() {
        let (map, rows) = with_charts();
        // The first chart strip: no strip height at all, so the whole of it
        // reads as the one strip offset its rows share...
        let chart_doc = rows[0].0;
        assert!((map.doc_to_mm(chart_doc * 0.5) - 0.0).abs() <= 0.01);
        // ...and reading back from that offset lands past it, on the text row
        // the strip actually draws there.
        assert!((map.mm_to_doc(0.0) - chart_doc).abs() <= 0.01);
        assert!((map.mm_to_doc(2.0) - (chart_doc + 16.0)).abs() <= 0.01);
    }

    #[test]
    fn drag_target_is_a_fixed_point() {
        let map = uniform(MM_TOTAL, TOTAL);
        for pointer_y in [0.0, 37.0, 150.0, 300.0, 455.0, 599.0] {
            let target = pointer_scroll_target(pointer_y, MM_H, &map, TOTAL, VIEWPORT);
            // Once there, the same pointer must ask for the same place — that is
            // what makes one drag event enough.
            let again = one_step(pointer_y, target, MM_H, &map, TOTAL, VIEWPORT);
            assert!(
                (again - target).abs() <= 1.0,
                "pointer {pointer_y}: settled at {target} but re-reads as {again}"
            );
        }
    }

    #[test]
    fn drag_target_is_a_fixed_point_over_chart_strips() {
        let (map, rows) = with_charts();
        let total: f32 = rows.iter().map(|r| r.0).sum();
        let (mm_h, viewport) = (40.0, 200.0);
        for pointer_y in [0.0, 7.0, 19.0, 33.0, 39.0] {
            let target = pointer_scroll_target(pointer_y, mm_h, &map, total, viewport);
            let again = one_step(pointer_y, target, mm_h, &map, total, viewport);
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
        let map = uniform(MM_TOTAL, TOTAL);
        let pointer_y = 400.0;
        let one = pointer_scroll_target(pointer_y, MM_H, &map, TOTAL, VIEWPORT);
        // The step contracts by only ~0.86 per event here, which is the crawl
        // itself: hundreds of events to arrive where one should have.
        let mut scroll = 0.0;
        for _ in 0..500 {
            scroll = one_step(pointer_y, scroll, MM_H, &map, TOTAL, VIEWPORT);
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
        let (mm_h, total, viewport) = (600.0, 8000.0, 800.0);
        let map = uniform(400.0, total);
        let target = pointer_scroll_target(200.0, mm_h, &map, total, viewport);
        let plain = one_step(200.0, 0.0, mm_h, &map, total, viewport);
        assert!((target - plain).abs() <= 0.01);
    }

    #[test]
    fn the_ends_of_the_strip_reach_the_ends_of_the_document() {
        let map = uniform(MM_TOTAL, TOTAL);
        let top = pointer_scroll_target(0.0, MM_H, &map, TOTAL, VIEWPORT);
        let bottom = pointer_scroll_target(MM_H, MM_H, &map, TOTAL, VIEWPORT);
        assert_eq!(top, 0.0);
        assert_eq!(bottom, TOTAL - VIEWPORT);
    }
}
