//! Painting a glyph's pixel grid: the cells themselves, the shadow under them
//! ([`crate::editor::shadow`], either kind), and the metrics overlay over them.
//!
//! # The metrics overlay (View ▸ Show glyph metrics)
//!
//! Each glyph's metric box is drawn over its grid.
//! [`crate::editor::document_view::GlyphMetrics`] computes where it sits — and
//! documents why `left`/`top` put it at `-left`/`-top`, why `bottom` is derived
//! rather than written, and when the baseline pair exists at all. Inside the box,
//! a second left/right-open box runs from the ascent line (the box's own top)
//! down to the baseline, and `GridExtent::include_metrics` widens the drawn area
//! to the whole box, which is what makes a two-row mark like `dia-below` show
//! where on the line it actually lands.
//!
//! Each metric line is three 1 px strokes — a `grid_bg` band between two
//! `grid_on` ones, the baseline pair additionally dashed. See
//! [`METRICS_STROKES`] for why they are inset inward and sized in points rather
//! than scaled with the zoom, and the ring comment in `draw_metrics_box` for why
//! the outer box is drawn as three *closed rectangles* instead of four edges.

use std::collections::HashMap;

use crate::document::{Document, DocumentItem, NamePartsMap, PixelGrid};
use crate::editor::EditMode;
use crate::editor::colors::Palette;
use crate::editor::document_view::{GlyphMetrics, GridExtent, UNFILLED_OPACITY};
use crate::editor::glyph_widget;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};
use crate::editor::shadow::{Shadow, ShadowKind};

/// Where a preview bitmap goes and how big one grid cell is on it.
///
/// A cell is *not* a logical pixel: a `scale N` glyph's grid counts subcells,
/// so at [`PREVIEW_SCALE`](crate::editor::document_view::PREVIEW_SCALE) a cell
/// is a fraction of a point wide and lands wherever it lands. `ppp` is what
/// lets [`blit_preview`] resolve that against the device pixel grid.
#[derive(Clone, Copy)]
pub(crate) struct PreviewGeom {
    pub(crate) rect: egui::Rect,
    pub(crate) cell_w: f32,
    pub(crate) cell_h: f32,
    pub(crate) ppp: f32,
}

/// Per-cell coverage of the device pixels a cell axis spans: for cell `i`,
/// the device pixel indices it touches (relative to `origin`) and how much of
/// each it covers.
fn axis_coverage(n: u16, off: i16, cell: f32, origin: f32, limit: usize) -> Vec<Vec<(usize, f32)>> {
    (0..n)
        .map(|i| {
            let a = origin + (off as f32 + i as f32) * cell;
            let b = a + cell;
            let mut spans = Vec::new();
            let first = a.floor() as i64;
            let last = (b.ceil() as i64 - 1).max(first);
            for d in first..=last {
                let overlap = b.min(d as f32 + 1.0) - a.max(d as f32);
                if overlap > 0.0 && d >= 0 && (d as usize) < limit {
                    spans.push((d as usize, overlap));
                }
            }
            spans
        })
        .collect()
}

/// Paint the filled cells of `grid` into `geom`, offset by `off_r`/`off_c`
/// cells.
///
/// One `rect_filled` per cell only works while a cell is a whole number of
/// device pixels. It is not for a `scale N` glyph, and a fractional cell rect
/// is antialiased against its neighbour instead of tiling with it — which drew
/// scaled subglyph thumbnails as a grid of seams, or dropped the cells thinner
/// than a pixel entirely. So the cells are rasterized to per-device-pixel
/// coverage first and emitted as a single mesh, merging horizontal runs of
/// equal coverage: no seams, correct antialiasing, and fewer quads than the
/// one-per-cell version it replaces.
pub(crate) fn blit_preview(
    painter: &egui::Painter,
    geom: &PreviewGeom,
    grid: &PixelGrid,
    off_r: i16,
    off_c: i16,
    color: egui::Color32,
) {
    let PreviewGeom {
        rect,
        cell_w,
        cell_h,
        ppp,
    } = *geom;
    if grid.width == 0 || grid.height == 0 || cell_w <= 0.0 || cell_h <= 0.0 || ppp <= 0.0 {
        return;
    }

    // The destination in device pixels. Its origin is rounded, so a thumbnail
    // whose rect starts mid-pixel still lays its cells on the pixel grid.
    // Both edges are rounded, not the origin plus a rounded size: a thumbnail
    // sized to its grid must not lose its last column to the two roundings
    // disagreeing.
    let x0 = (rect.min.x * ppp).round();
    let y0 = (rect.min.y * ppp).round();
    let dw_f = (rect.max.x * ppp).round() - x0;
    let dh_f = (rect.max.y * ppp).round() - y0;
    if dw_f < 1.0 || dh_f < 1.0 {
        return;
    }
    let (dw, dh) = (dw_f as usize, dh_f as usize);

    let cols = axis_coverage(grid.width, off_c, cell_w * ppp, 0.0, dw);
    let rows = axis_coverage(grid.height, off_r, cell_h * ppp, 0.0, dh);

    let mut cov = vec![0.0f32; dw * dh];
    for r in 0..grid.height {
        let row_spans = &rows[r as usize];
        if row_spans.is_empty() {
            continue;
        }
        for c in 0..grid.width {
            if !grid.get(r, c).is_filled() {
                continue;
            }
            for &(dy, wy) in row_spans {
                let base = dy * dw;
                for &(dx, wx) in &cols[c as usize] {
                    cov[base + dx] += wy * wx;
                }
            }
        }
    }

    let [cr, cg, cb, ca] = color.to_array();
    let mut mesh = egui::Mesh::default();
    for dy in 0..dh {
        let mut dx = 0;
        while dx < dw {
            let alpha = ((cov[dy * dw + dx].clamp(0.0, 1.0) * ca as f32).round()) as u8;
            if alpha == 0 {
                dx += 1;
                continue;
            }
            // Merge the run of equal coverage into one quad.
            let start = dx;
            while dx < dw
                && ((cov[dy * dw + dx].clamp(0.0, 1.0) * ca as f32).round()) as u8 == alpha
            {
                dx += 1;
            }
            let quad = egui::Rect::from_min_max(
                egui::pos2((x0 + start as f32) / ppp, (y0 + dy as f32) / ppp),
                egui::pos2((x0 + dx as f32) / ppp, (y0 + dy as f32 + 1.0) / ppp),
            );
            mesh.add_colored_rect(
                quad,
                egui::Color32::from_rgba_unmultiplied(cr, cg, cb, alpha),
            );
        }
    }
    if !mesh.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
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
            && let Some(comp) = ref_composite::compute_composite(
                body,
                named_glyphs,
                name_parts,
                alt_index,
                color_aliases,
            )
        {
            composites.insert(idx, comp);
        }
    }
    composites
}

#[allow(clippy::too_many_arguments)]
/// Color of the anchor layer `pi` (points-then-inherited index): a declared
/// point is colored by its own layer index, an inherited anchor by the ref it
/// came from.
fn anchor_layer_color(
    pal: &Palette,
    body: &crate::document::GlyphBody,
    composite: Option<&GlyphComposite>,
    pi: usize,
) -> egui::Color32 {
    if let Some(ii) = pi.checked_sub(body.points.len())
        && let Some((_, src_ref)) = composite.and_then(|c| c.inherited_anchors.get(ii))
    {
        return ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, *src_ref);
    }
    ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, body.refs.len() + pi)
}

// Painting parameters, each independent of the others.
#[expect(clippy::too_many_arguments)]
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
    metrics: Option<&GlyphMetrics>,
    composite: Option<&GlyphComposite>,
    shadow: Option<&Shadow>,
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
    // LayerMove indexes refs first, then points; split the active layer index
    // into whichever of the two it denotes.
    let (active_ref, active_point) = match mode {
        EditMode::LayerMove {
            item_idx: eidx,
            layer_idx,
        } if *eidx == item_idx => match doc.items.get(item_idx) {
            Some(DocumentItem::Glyph { body, .. }) if *layer_idx < body.refs.len() => {
                (Some(*layer_idx), None)
            }
            Some(DocumentItem::Glyph { body, .. }) => (None, Some(*layer_idx - body.refs.len())),
            _ => (None, None),
        },
        _ => (None, None),
    };

    // Under everything else: a shadow is context for the glyph — what the
    // selected anchor *could* carry, or what carries this glyph elsewhere —
    // and never part of it. Each kind is only drawn in the mode that asks for
    // it, since the view keeps the last one it built.
    let shadow = shadow.filter(|s| match s.kind {
        ShadowKind::Anchor => active_point.is_some(),
        ShadowKind::Backref => {
            matches!(mode, EditMode::PixelSelect { item_idx: eidx, backrefs: true } if *eidx == item_idx)
        }
    });
    if let Some(s) = shadow {
        let body = match doc.items.get(item_idx) {
            Some(DocumentItem::Glyph { body, .. }) => Some(body),
            _ => None,
        };
        let color = shadow_color(pal, s.kind, body, composite, active_point);
        draw_shadow_row(painter, x, y, row, extent, s, color, cs);
    }
    let shadow_inked = |dc: i16| -> bool {
        shadow.is_some_and(|s| {
            let (sr, sc) = (row - s.row, dc - s.col);
            sr >= 0
                && sr < s.grid.height as i16
                && sc >= 0
                && sc < s.grid.width as i16
                && !s.grid.get(sr as u16, sc as u16).is_blank()
        })
    };

    if let Some(comp) = composite {
        for layer in &comp.layers {
            if Some(layer.ref_idx) == active_ref {
                continue;
            }
            let color = layer.fill_color.unwrap_or_else(|| {
                ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx)
            });
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
                            glyph_widget::draw_grid_cell_colored(
                                painter,
                                cell_rect,
                                &layer.grid,
                                lr_in_layer as u16,
                                lc_in_layer as u16,
                                pal.grid_bg,
                            );
                        } else {
                            let px_color = if shape.is_filled() {
                                color
                            } else {
                                apply_opacity(color, UNFILLED_OPACITY)
                            };
                            glyph_widget::draw_grid_cell_colored(
                                painter,
                                cell_rect,
                                &layer.grid,
                                lr_in_layer as u16,
                                lc_in_layer as u16,
                                px_color,
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
                glyph_widget::draw_pixel_cell_colored(painter, cell_rect, shape, color);
            } else if !has_ref_pixel(dc) && !shadow_inked(dc) {
                painter.rect_filled(cell_rect, 0.0, pal.grid_off);
            }
        } else if !has_ref_pixel(dc) && !shadow_inked(dc) {
            painter.rect_filled(cell_rect, 0.0, pal.grid_ext_off);
        }
    }

    if let Some(active) = active_ref
        && let Some(comp) = composite
        && let Some(layer) = comp.layers.iter().find(|l| l.ref_idx == active)
    {
        let color = layer.fill_color.unwrap_or_else(|| {
            ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, layer.ref_idx)
        });
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
                    glyph_widget::draw_grid_cell_colored(
                        painter,
                        cell_rect,
                        &layer.grid,
                        lr_in_layer as u16,
                        lc_in_layer as u16,
                        px_color,
                    );
                }
            }
        }
    }

    // Draw anchor markers: the declared points first, then the anchors
    // inherited through `inherit` refs, each in its source subglyph's color.
    if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) {
        let num_refs = body.refs.len();
        let inherited = composite.map_or(&[][..], |c| c.inherited_anchors.as_slice());
        let declared = body.points.iter().enumerate().map(|(pi, point)| {
            let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, num_refs + pi);
            (pi, point, color)
        });
        let inherited = inherited.iter().enumerate().map(|(ii, (point, src_ref))| {
            let color = ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, *src_ref);
            (body.points.len() + ii, point, color)
        });
        for (pi, point, color) in declared.chain(inherited) {
            if row < point.row || row > point.row_end {
                continue;
            }
            if point.col_end < extent.left || point.col >= extent.right {
                continue;
            }
            let is_active_point = active_point == Some(pi);
            let opacity = if is_layer_mode && !is_active_point {
                0.35
            } else {
                1.0
            };
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

    if let Some(m) = metrics {
        draw_metrics_box(painter, x, y, row, extent, m, grid_cell, pal);
    }
}

/// What colour a shadow is drawn in.
///
/// An anchor shadow borrows the selected anchor's own layer colour, so it reads
/// as belonging to that anchor. A backreference belongs to no layer, so it
/// takes the body text colour — but through
/// [`Palette::dark_equivalent`](crate::editor::colors::Palette::dark_equivalent),
/// for the same reason the minimap does: the grid is painted on a dark ground
/// in *both* themes, while `text_default` follows the reader's theme and is
/// near-black in the light one. Painted straight it would be invisible.
fn shadow_color(
    pal: &Palette,
    kind: ShadowKind,
    body: Option<&crate::document::GlyphBody>,
    composite: Option<&GlyphComposite>,
    active_point: Option<usize>,
) -> egui::Color32 {
    match (kind, body) {
        (ShadowKind::Backref, _) => pal.dark_equivalent(pal.text_default),
        (ShadowKind::Anchor, Some(body)) => {
            anchor_layer_color(pal, body, composite, active_point.unwrap_or(0))
        }
        (ShadowKind::Anchor, None) => ref_composite::ref_color_sv(pal.ref_hsv_s, pal.ref_hsv_v, 0),
    }
}

/// How strongly a shadow is drawn. Well under a real layer's opacity:
/// it is context for the glyph being edited, and every candidate at once is a
/// lot of ink — the union of a mark's bases covers most of the em box.
const SHADOW_OPACITY: f32 = 0.3;

/// One row of a shadow, in the colour the caller picked for its kind. Cells any
/// candidate inks are drawn solid, sub-pixel-only ones at the usual unfilled
/// opacity; the exact geometry comes from the shadow grid itself, custom
/// details included.
#[allow(clippy::too_many_arguments)]
fn draw_shadow_row(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    row: i16,
    extent: GridExtent,
    shadow: &Shadow,
    color: egui::Color32,
    cs: f32,
) {
    let sr = row - shadow.row;
    if sr < 0 || sr >= shadow.grid.height as i16 {
        return;
    }
    for dc in extent.left..extent.right {
        let sc = dc - shadow.col;
        if sc < 0 || sc >= shadow.grid.width as i16 {
            continue;
        }
        let shape = shadow.grid.get(sr as u16, sc as u16);
        if shape.is_empty() {
            continue;
        }
        let opacity = if shape.is_filled() {
            SHADOW_OPACITY
        } else {
            SHADOW_OPACITY * UNFILLED_OPACITY
        };
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x + (dc - extent.left) as f32 * cs, y),
            egui::vec2(cs, cs),
        );
        glyph_widget::draw_grid_cell_colored(
            painter,
            cell_rect,
            &shadow.grid,
            sr as u16,
            sc as u16,
            apply_opacity(color, opacity),
        );
    }
}

/// Widths of the three strokes a metric line is made of, in points: a
/// background-coloured band with a border-coloured stroke on either side.
const METRICS_BAND: f32 = 1.0;
const METRICS_EDGE: f32 = 1.0;
/// Dash length on the baseline/ascent pair; the gap is the same.
const METRICS_DASH: f32 = 3.0;

/// How far each of the three strokes is inset from the metric line it marks —
/// centres, so the whole stack occupies `0 .. 2 * EDGE + BAND` inside the box.
///
/// The stack is inset rather than centred on the line because the drawn area is
/// widened to exactly the metric box (`GridExtent::include_metrics`), so a
/// centred stack would lose its outer half to the clip on every flush edge —
/// which is all four of them for a glyph with no metric flags.
///
/// Every width here is deliberately **not** scaled by the zoom level. The band
/// is not a feature of the glyph, and at 6x zoom one that grew with the cells
/// would read as another sub-pixel shape.
const METRICS_STROKES: [(f32, f32); 3] = [
    (METRICS_EDGE * 0.5, METRICS_EDGE),
    (METRICS_EDGE + METRICS_BAND * 0.5, METRICS_BAND),
    (
        METRICS_EDGE + METRICS_BAND + METRICS_EDGE * 0.5,
        METRICS_EDGE,
    ),
];
/// Index of the background band in [`METRICS_STROKES`] — the one the dashes go
/// back over, and the one that must be laid down before either neighbour.
const METRICS_BAND_IDX: usize = 1;

/// Pull both ends of a span inward by `by`, collapsing to the midpoint when
/// there is not enough room — an `advance 0` box has no room at all.
fn inset_span(a: f32, b: f32, by: f32) -> (f32, f32) {
    if b - a >= by * 2.0 {
        (a + by, b - by)
    } else {
        let mid = (a + b) * 0.5;
        (mid, mid)
    }
}

/// The colour a metric line is drawn in. Same as an ordinary grid line: the box
/// is told apart by its shape and its background core, not by a hue of its own.
fn metrics_stroke_color(pal: &Palette) -> egui::Color32 {
    pal.grid_on
}

/// The three strokes of one horizontal metric line, `sign` telling which way is
/// into the shape the line bounds. Dashed for the baseline/ascent pair.
fn metrics_hline(
    painter: &egui::Painter,
    x0: f32,
    x1: f32,
    y: f32,
    sign: f32,
    dashed: bool,
    pal: &Palette,
) {
    if x1 <= x0 {
        return;
    }
    for (i, (off, width)) in METRICS_STROKES.iter().enumerate() {
        let ly = y + sign * off;
        let color = if i == METRICS_BAND_IDX {
            pal.grid_bg
        } else {
            metrics_stroke_color(pal)
        };
        painter.line_segment(
            [egui::pos2(x0, ly), egui::pos2(x1, ly)],
            egui::Stroke::new(*width, color),
        );
    }
    if dashed {
        let ly = y + sign * METRICS_STROKES[METRICS_BAND_IDX].0;
        let mut t = x0;
        while t < x1 {
            let e = (t + METRICS_DASH).min(x1);
            painter.line_segment(
                [egui::pos2(t, ly), egui::pos2(e, ly)],
                egui::Stroke::new(METRICS_BAND, metrics_stroke_color(pal)),
            );
            t += METRICS_DASH * 2.0;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_metrics_box(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    row: i16,
    extent: GridExtent,
    m: &GlyphMetrics,
    cs: f32,
    pal: &Palette,
) {
    // The box spans many rows but is painted a row at a time, so each row draws
    // the whole thing behind its own clip. Cheaper than working out which
    // pieces fall in this row, and it keeps the geometry in one expression.
    let row_rect =
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(extent.display_width(cs), cs));
    let clip = painter.clip_rect().intersect(row_rect);
    if clip.width() <= 0.0 || clip.height() <= 0.0 {
        return;
    }
    let p = painter.with_clip_rect(clip);

    let gx = |c: i16| x + (c - extent.left) as f32 * cs;
    let gy = |r: i16| y + (r - row) as f32 * cs;
    let (x0, x1) = (gx(m.left), gx(m.right));
    let (y0, y1) = (gy(m.top), gy(m.bottom));

    // The baseline/ascent pair goes down first: its ascent line lands exactly on
    // the big box's top edge whenever `height == ascent + descent`, and letting
    // the big box paint over it is what settles that without special-casing
    // either. `advance 0` (every combining mark) leaves the pair no width to run
    // along, so it falls back to a cell's worth of tick centred on the box.
    if let Some(baseline) = m.baseline {
        let (hx0, hx1) = if x1 - x0 < cs {
            let mid = (x0 + x1) * 0.5;
            (mid - cs * 0.5, mid + cs * 0.5)
        } else {
            (x0, x1)
        };
        metrics_hline(&p, hx0, hx1, y0, 1.0, true, pal);
        metrics_hline(&p, hx0, hx1, gy(baseline), -1.0, true, pal);
    }

    // Each of the three strokes is one closed rectangle, not four segments:
    // drawn edge by edge, whichever of the two meeting at a corner comes second
    // lays its background band over the other's border stroke and breaks it.
    // Stacking whole rings also fixes the order between them — the background
    // ring is complete before either border ring starts.
    let ring = |off: f32, width: f32, color: egui::Color32| {
        let (rx0, rx1) = inset_span(x0, x1, off);
        let (ry0, ry1) = inset_span(y0, y1, off);
        p.add(egui::Shape::closed_line(
            vec![
                egui::pos2(rx0, ry0),
                egui::pos2(rx1, ry0),
                egui::pos2(rx1, ry1),
                egui::pos2(rx0, ry1),
            ],
            egui::Stroke::new(width, color),
        ));
    };
    let (band_off, band_w) = METRICS_STROKES[METRICS_BAND_IDX];
    ring(band_off, band_w, pal.grid_bg);
    for (i, (off, width)) in METRICS_STROKES.iter().enumerate() {
        if i != METRICS_BAND_IDX {
            ring(*off, *width, metrics_stroke_color(pal));
        }
    }
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
    strip: &crate::editor::document_view::GridStrip,
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
        && strip.accepts_pointer(hp)
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
            let shift_held = ui.input(|i| i.modifiers.shift);
            let preview_shape = if shift_held && !selected_shape.is_empty() {
                selected_shape.with_fill_toggled()
            } else {
                *selected_shape
            };
            if preview_shape.is_empty() {
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
                    preview_shape,
                    preview_color,
                );
            }
            ui.ctx().request_repaint();
        }
    }
}

// Painting parameters, each independent of the others.
#[expect(clippy::too_many_arguments)]
pub(crate) fn render_pixel_selection_overlay(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    row: i16,
    extent: GridExtent,
    grid_cell: f32,
    sel: &crate::editor::pixel_selection::PixelSelection,
    pal: &Palette,
) {
    let cs = grid_cell;
    let sel_top = sel.row;
    let sel_bottom = sel.row + sel.height as i16;
    let sel_left = sel.col;
    let sel_right = sel.col + sel.width as i16;

    if row < sel_top || row >= sel_bottom {
        return;
    }

    // 1. Render floating pixels
    if sel.is_floating() {
        let float = sel.float_pixels.as_ref().unwrap();
        let fr = (row - sel.row) as u16;
        for dc in extent.left..extent.right {
            if dc < sel_left || dc >= sel_right {
                continue;
            }
            let fc = (dc - sel.col) as u16;
            let shape = float.get(fr, fc);
            if !shape.is_empty() {
                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(x + (dc - extent.left) as f32 * cs, y),
                    egui::vec2(cs, cs),
                );
                let color = if shape.is_filled() {
                    pal.pixel_filled
                } else {
                    apply_opacity(pal.pixel_filled, UNFILLED_OPACITY)
                };
                glyph_widget::draw_pixel_cell_colored(painter, cell_rect, shape, color);
            }
        }
    }

    // 2. Blue selection overlay
    let vis_left = sel_left.max(extent.left);
    let vis_right = sel_right.min(extent.right);
    if vis_left >= vis_right {
        return;
    }
    let x0 = x + (vis_left - extent.left) as f32 * cs;
    let x1 = x + (vis_right - extent.left) as f32 * cs;

    // Fill
    let band_rect = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(x1 - x0, cs));
    painter.rect_filled(band_rect, 0.0, apply_opacity(pal.pixel_selection, 0.25));

    let border_color = apply_opacity(pal.pixel_selection, 0.8);
    let stroke = egui::Stroke::new(1.5, border_color);

    // Top border
    if row == sel_top {
        painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke);
    }
    // Bottom border
    if row + 1 == sel_bottom {
        painter.line_segment([egui::pos2(x0, y + cs), egui::pos2(x1, y + cs)], stroke);
    }
    // Left border
    if sel_left >= extent.left {
        let lx = x + (sel_left - extent.left) as f32 * cs;
        painter.line_segment([egui::pos2(lx, y), egui::pos2(lx, y + cs)], stroke);
    }
    // Right border
    if sel_right <= extent.right {
        let rx = x + (sel_right - extent.left) as f32 * cs;
        painter.line_segment([egui::pos2(rx, y), egui::pos2(rx, y + cs)], stroke);
    }
}

// Text column<->x geometry lives in `annotations::AnnotatedText`, which
// also accounts for inline annotations.

#[cfg(test)]
mod tests {
    use super::{Palette, axis_coverage, shadow_color};
    use crate::editor::shadow::ShadowKind;

    /// Relative luminance, for "can this be seen against that".
    fn luma(c: egui::Color32) -> f32 {
        let [r, g, b, _] = c.to_array();
        (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
    }

    /// The grid is drawn on a dark ground in *both* themes (`Palette::light`
    /// copies the grid colours from `dark`), so a shadow painted in the
    /// reader's text colour is near-black on near-black in the light theme —
    /// which is how the backreference shadow first shipped.
    #[test]
    fn the_backref_shadow_is_visible_on_the_grid_in_either_theme() {
        for (name, pal) in [("dark", Palette::dark()), ("light", Palette::light())] {
            let c = shadow_color(&pal, ShadowKind::Backref, None, None, None);
            assert!(
                luma(c) - luma(pal.grid_bg) > 0.4,
                "{name}: shadow colour {c:?} is invisible on the grid ground {:?}",
                pal.grid_bg
            );
        }
    }

    fn total(spans: &[Vec<(usize, f32)>], device_px: usize) -> f32 {
        spans
            .iter()
            .flatten()
            .filter(|(d, _)| *d == device_px)
            .map(|(_, w)| *w)
            .sum()
    }

    /// Whole-device-pixel cells (a `scale 1` glyph at 2x on a 1x display) land
    /// one per pixel, with nothing spilling into the neighbours.
    #[test]
    fn whole_pixel_cells_tile_one_to_one() {
        let cov = axis_coverage(4, 0, 1.0, 0.0, 4);
        for (i, spans) in cov.iter().enumerate() {
            assert_eq!(spans.as_slice(), &[(i, 1.0)], "cell {i}");
        }
    }

    /// The case that used to break: a `scale 2` glyph's cells are half a device
    /// pixel wide, so no cell owns a pixel outright. Each must still contribute
    /// its half — drawing them as fractional rects instead left seams, and the
    /// cells that rounded away vanished.
    #[test]
    fn subpixel_cells_split_a_pixel_instead_of_vanishing() {
        let cov = axis_coverage(4, 0, 0.5, 0.0, 2);
        for (i, spans) in cov.iter().enumerate() {
            assert!(!spans.is_empty(), "cell {i} covers nothing");
        }
        assert_eq!(cov[0], vec![(0, 0.5)]);
        assert_eq!(cov[1], vec![(0, 0.5)]);
        assert_eq!(total(&cov, 0), 1.0, "pixel 0 must end up fully covered");
        assert_eq!(total(&cov, 1), 1.0, "pixel 1 must end up fully covered");
    }

    /// A cell straddling a pixel boundary covers both sides, and the two halves
    /// still add up to exactly one cell — no gap at the seam, no double ink.
    #[test]
    fn a_straddling_cell_is_split_without_loss() {
        let cov = axis_coverage(2, 0, 1.0, 0.5, 3);
        assert_eq!(cov[0], vec![(0, 0.5), (1, 0.5)]);
        assert_eq!(cov[1], vec![(1, 0.5), (2, 0.5)]);
        assert_eq!(total(&cov, 1), 1.0, "the shared pixel is covered once");
    }

    /// Cells placed outside the destination are dropped, not wrapped around.
    #[test]
    fn cells_outside_the_destination_are_dropped() {
        let cov = axis_coverage(3, -2, 1.0, 0.0, 3);
        assert!(cov[0].is_empty());
        assert!(cov[1].is_empty());
        assert_eq!(cov[2], vec![(0, 1.0)]);
    }
}
