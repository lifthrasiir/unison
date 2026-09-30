//! Drawing a laid-out cell: its tint, glyph, label and metric marks.

use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use crate::glyph_flags::GlyphFlag;

use super::status::uvs_label;
use super::{
    CELL_H, CELL_W, DIM_ALPHA, ERROR_BG, ERROR_BG_HOVER, Item, SpecimenState, WARNING_BG,
    WARNING_BG_HOVER,
};

pub(super) fn flag_bg(flag: GlyphFlag, is_hovered: bool) -> egui::Color32 {
    match (flag, is_hovered) {
        (GlyphFlag::Warning, false) => WARNING_BG,
        (GlyphFlag::Error, false) => ERROR_BG,
        (GlyphFlag::Warning, true) => WARNING_BG_HOVER,
        (GlyphFlag::Error, true) => ERROR_BG_HOVER,
    }
}

impl SpecimenState {
    pub(super) fn draw_item(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        item: Item,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        // Painted under everything the cell draws. A hovered cell has already
        // had its (dark) background filled in by `show`, which owns the
        // overflow past the cell edge as well.
        if !is_hovered && let Some(flag) = self.flag_for(item) {
            // Inset by the border stroke, which is drawn before the cells and
            // is the grid the tint sits inside rather than over.
            painter.rect_filled(
                style.cell_rect(cell_min).shrink(1.0),
                0.0,
                flag_bg(flag, false),
            );
        }
        match item {
            Item::Char(i) => {
                let cp = self.entries[i].cp;
                let declared = self.entries[i].glyph_name.is_some();
                self.draw_cell(
                    painter,
                    cell_min,
                    cp,
                    declared,
                    is_hovered,
                    glyph_color,
                    style,
                );
            }
            Item::Uvs(i) => {
                self.draw_uvs_cell(painter, cell_min, i, is_hovered, glyph_color, style)
            }
            Item::Remap(ri) => {
                self.draw_remap_cell(painter, cell_min, ri, is_hovered, glyph_color, style)
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn draw_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        cp: u32,
        declared: bool,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let px_size = style.px_size;
        let ctx = style.ctx;

        let font = style.raster_font.and_then(|b| FontRef::new(b).ok());
        // Whether the *built font* has anything to show here, which is what
        // dims the cell. Before the first build there is no font to ask, so
        // what the source says stands in for it.
        let has_metrics = match &font {
            Some(font) => cp_has_metrics(font, cp),
            None => declared,
        };

        let hex = format!("{cp:04X}");
        let label_color = if has_metrics {
            style.label_color
        } else {
            style.dim_label_color
        };
        let label_galley = painter.layout_no_wrap(hex, style.label_font.clone(), label_color);
        painter.galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            label_color,
        );

        let Some(ch) = char::from_u32(cp) else { return };
        let center = style.cell_center(cell_min);

        let mut drawn_via_rasterizer = false;
        // Whether the built font answers for this character at all. Where it
        // does, its answer stands even when it is a glyph with no outline: a
        // blank the font maps is drawn blank, not in the UI font's shape.
        let mut font_maps = false;

        if let Some(font_bytes) = style.raster_font
            && let Some(font) = &font
            && let Some(gid) = font.charmap().map(ch)
        {
            font_maps = true;
            drawn_via_rasterizer = self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                font,
                font_bytes,
                gid,
                is_hovered,
                glyph_color,
                style,
            );
        }

        // The fallback draws the character in the *editor's* UI font, which is
        // a reasonable stand-in for a glyph the build has not caught up with
        // (one the font does not map yet) —
        // but for a character the source declares nothing about it would read
        // as coverage the font does not have, so an undeclared cell stays empty.
        // It is the font's glyph, not this one, that the cell claims to show,
        // so a dimmed cell dims it too.
        if !drawn_via_rasterizer && !font_maps && declared {
            let color = if has_metrics {
                glyph_color
            } else {
                glyph_color.gamma_multiply(DIM_ALPHA)
            };
            let glyph_font = crate::app::uniform_font_id(ctx, px_size);
            let glyph_galley = painter.layout_no_wrap(ch.to_string(), glyph_font, color);
            let glyph_size = glyph_galley.size();
            let pos = egui::pos2(center.0 - glyph_size.x / 2.0, center.1 - glyph_size.y / 2.0);
            cell_painter(painter, cell_rect, is_hovered).galley(pos, glyph_galley, color);
        }
    }

    /// A variation-sequence cell: the `+VS17` label, and whatever the built
    /// font's cmap format 14 maps the pair to.
    ///
    /// The glyph is looked up through the *font*, not through the source's
    /// glyph name, so the cell shows what a shaper would actually pick — a
    /// sequence the build dropped draws nothing and dims its label, exactly as
    /// a character whose glyph never made it does.
    fn draw_uvs_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        uvs_idx: usize,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let entry = &self.uvs_entries[uvs_idx];
        let label_text = uvs_label(&self.char_props, entry);
        let (base, selector) = (entry.base, entry.selector);

        let font = style.raster_font.and_then(|b| FontRef::new(b).ok());
        let gid = font.as_ref().and_then(|f| variant_gid(f, base, selector));
        let label_color = if gid.is_some() {
            style.label_color
        } else {
            style.dim_label_color
        };
        let label_galley =
            painter.layout_no_wrap(label_text, style.label_font.clone(), label_color);
        cell_painter(painter, cell_rect, is_hovered).galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            label_color,
        );

        if let (Some(font_bytes), Some(font), Some(gid)) = (style.raster_font, &font, gid) {
            let center = style.cell_center(cell_min);
            self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                font,
                font_bytes,
                gid,
                is_hovered,
                glyph_color,
                style,
            );
        }
    }

    fn draw_remap_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        remap_idx: usize,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let entry = &self.remap_entries[remap_idx];
        let gid = entry.gid;
        let label_text = entry.label.clone();

        let label_galley =
            painter.layout_no_wrap(label_text, style.label_font.clone(), style.label_color);
        cell_painter(painter, cell_rect, is_hovered).galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            style.label_color,
        );

        if let Some(font_bytes) = style.raster_font
            && let Ok(font) = FontRef::new(font_bytes)
        {
            let center = style.cell_center(cell_min);
            self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                &font,
                font_bytes,
                skrifa::GlyphId::new(gid as u32),
                is_hovered,
                glyph_color,
                style,
            );
        }
    }

    /// Rasterizes `gid` and paints it centered on the cell baseline; returns
    /// false when the rasterizer produced nothing so the caller can fall back
    /// to text rendering.
    #[expect(clippy::too_many_arguments)]
    fn draw_rasterized_glyph(
        &mut self,
        painter: &egui::Painter,
        cell_rect: egui::Rect,
        center: (f32, f32),
        font: &FontRef,
        font_bytes: &[u8],
        gid: skrifa::GlyphId,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) -> bool {
        let px_size = style.px_size;
        let Some(cached) = self.glyph_cache.get_or_rasterize(
            style.ctx,
            font_bytes,
            gid.to_u32() as u16,
            px_size,
            true,
            glyph_color,
        ) else {
            return false;
        };

        let m = cell_glyph_metrics(font, gid, px_size, center, cached.width);
        if style.show_metric_marks {
            draw_metric_marks(painter, cell_rect, &m, is_hovered, glyph_color);
        }
        let draw_rect = egui::Rect::from_min_size(
            egui::pos2(m.pen_x + cached.bearing_x, m.baseline_y - cached.bearing_y),
            egui::vec2(cached.width, cached.height),
        );
        let tint = if cached.is_color {
            egui::Color32::WHITE
        } else {
            glyph_color
        };
        cell_painter(painter, cell_rect, is_hovered).image(
            cached.texture.id(),
            draw_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
        true
    }

    /// The rect the hovered cell's glyph occupies, which the hover paints its
    /// black background over — `None` when the cell draws no glyph at all.
    pub(super) fn compute_glyph_rect(
        &self,
        cell_min: egui::Pos2,
        item: Item,
        style: &CellStyle<'_>,
    ) -> Option<egui::Rect> {
        let center = style.cell_center(cell_min);
        let font = style.raster_font.and_then(|b| FontRef::new(b).ok());
        match item {
            Item::Char(i) => {
                let ch = char::from_u32(self.entries[i].cp)?;
                if let Some(font) = &font
                    && let Some(gid) = font.charmap().map(ch)
                {
                    return Some(raster_glyph_rect(font, gid, style.px_size, center));
                }
                // Whatever `draw_cell`'s UI-font fallback will draw — nothing,
                // for an undeclared character.
                self.entries[i].glyph_name.as_ref()?;
                let glyph_font = crate::app::uniform_font_id(style.ctx, style.px_size);
                let size = style
                    .ctx
                    .fonts(|f| f.layout_no_wrap(ch.to_string(), glyph_font, egui::Color32::WHITE))
                    .size();
                Some(egui::Rect::from_min_size(
                    egui::pos2(center.0 - size.x / 2.0, center.1 - size.y / 2.0),
                    size,
                ))
            }
            Item::Uvs(i) => {
                let entry = &self.uvs_entries[i];
                let font = font?;
                let gid = variant_gid(&font, entry.base, entry.selector)?;
                Some(raster_glyph_rect(&font, gid, style.px_size, center))
            }
            Item::Remap(ri) => Some(raster_glyph_rect(
                &font?,
                skrifa::GlyphId::new(self.remap_entries[ri].gid as u32),
                style.px_size,
                center,
            )),
        }
    }
}

/// The glyph the built font maps `base` + `selector` to, following cmap format
/// 14's "use default" entries back to the base's own glyph.
fn variant_gid(font: &FontRef, base: u32, selector: u32) -> Option<skrifa::GlyphId> {
    let ch = char::from_u32(base)?;
    match font.charmap().map_variant(ch, char::from_u32(selector)?)? {
        skrifa::charmap::MapVariant::UseDefault => font.charmap().map(ch),
        skrifa::charmap::MapVariant::Variant(gid) => Some(gid),
    }
}

/// Whether the built font has metrics for `cp` — an advance to occupy, or an
/// outline to draw. False for a character the font has no glyph for at all
/// (undeclared, or `map`ped to a glyph that does not exist) and for one whose
/// glyph is an empty grid.
///
/// A blank glyph *with* an advance — a space — has metrics: it is a character
/// the font has, and the cell says so.
pub(super) fn cp_has_metrics(font: &FontRef, cp: u32) -> bool {
    let Some(gid) = char::from_u32(cp).and_then(|ch| font.charmap().map(ch)) else {
        return false;
    };
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    metrics.advance_width(gid).is_some_and(|w| w > 0.0)
        || metrics
            .bounds(gid)
            .is_some_and(|b| b.x_max > b.x_min && b.y_max > b.y_min)
}

/// The glyph anchor point of a specimen cell: horizontally centered, nudged
/// below center to leave room for the codepoint label.
fn cell_center(cell_min: egui::Pos2) -> (f32, f32) {
    (cell_min.x + CELL_W / 2.0, cell_min.y + CELL_H / 2.0 + 8.0)
}

/// Everything a specimen cell is drawn with that is the same for every cell of
/// one grid: glyph size, label style, whether the metric marks are on, and the
/// font to raster from.
pub(super) struct CellStyle<'a> {
    pub(super) px_size: f32,
    pub(super) label_font: &'a egui::FontId,
    pub(super) label_color: egui::Color32,
    /// The label color of a cell the built font has no metrics for — an empty
    /// glyph, a `map` to a glyph that does not exist, or a character the source
    /// declares nothing about. Dimmer, since such a cell is there to show the
    /// *hole*, not the character.
    pub(super) dim_label_color: egui::Color32,
    pub(super) show_metric_marks: bool,
    pub(super) raster_font: Option<&'a Vec<u8>>,
    pub(super) ctx: &'a egui::Context,
}

impl CellStyle<'_> {
    pub(super) fn cell_rect(&self, cell_min: egui::Pos2) -> egui::Rect {
        egui::Rect::from_min_size(cell_min, egui::vec2(CELL_W, CELL_H))
    }

    fn cell_center(&self, cell_min: egui::Pos2) -> (f32, f32) {
        cell_center(cell_min)
    }
}

/// A painter that clips to the cell unless the cell is hovered (hovered
/// cells intentionally overflow their neighbors).
fn cell_painter(painter: &egui::Painter, cell_rect: egui::Rect, is_hovered: bool) -> egui::Painter {
    if is_hovered {
        painter.clone()
    } else {
        painter.with_clip_rect(cell_rect)
    }
}

/// Length of one arm of a metric corner mark, in points.
const METRIC_MARK_LEN: f32 = 5.0;

/// Paints the corners of a cell's metric box — the advance width by the
/// ascent-to-descent band — as crop marks: two segments per corner, each
/// pointing *away* from the box, so the marks say where the metrics are
/// without drawing a frame over the glyph.
fn draw_metric_marks(
    painter: &egui::Painter,
    cell_rect: egui::Rect,
    m: &CellGlyphMetrics,
    is_hovered: bool,
    glyph_color: egui::Color32,
) {
    let left = m.pen_x;
    let right = m.pen_x + m.advance_w;
    let top = m.baseline_y - m.ascent;
    let bottom = m.baseline_y - m.descent;
    if !(left.is_finite() && right.is_finite() && top.is_finite() && bottom.is_finite()) {
        return;
    }

    let stroke = egui::Stroke::new(1.0, glyph_color.gamma_multiply(0.3));
    let painter = cell_painter(painter, cell_rect, is_hovered);
    // (x, y, x-arm direction, y-arm direction) per corner; a zero-advance glyph
    // collapses the two columns onto each other, which is what it looks like.
    for (x, y, dx, dy) in [
        (left, top, -1.0, -1.0),
        (right, top, 1.0, -1.0),
        (left, bottom, -1.0, 1.0),
        (right, bottom, 1.0, 1.0),
    ] {
        let corner = egui::pos2(x, y);
        painter.line_segment(
            [corner, corner + egui::vec2(dx * METRIC_MARK_LEN, 0.0)],
            stroke,
        );
        painter.line_segment(
            [corner, corner + egui::vec2(0.0, dy * METRIC_MARK_LEN)],
            stroke,
        );
    }
}

struct CellGlyphMetrics {
    advance_w: f32,
    ascent: f32,
    descent: f32,
    baseline_y: f32,
    pen_x: f32,
}

/// Baseline/pen placement centering a glyph's advance in a cell.
fn cell_glyph_metrics(
    font: &FontRef,
    gid: skrifa::GlyphId,
    px_size: f32,
    center: (f32, f32),
    fallback_advance: f32,
) -> CellGlyphMetrics {
    let font_metrics = font.metrics(Size::new(px_size), LocationRef::default());
    let glyph_metrics = font.glyph_metrics(Size::new(px_size), LocationRef::default());
    let advance_w = glyph_metrics.advance_width(gid).unwrap_or(fallback_advance);
    let ascent = font_metrics.ascent;
    let descent = font_metrics.descent;
    CellGlyphMetrics {
        advance_w,
        ascent,
        descent,
        baseline_y: center.1 + (ascent + descent) / 2.0,
        pen_x: center.0 - advance_w / 2.0,
    }
}

/// The rect a rasterized glyph's advance/extent occupies in a cell.
fn raster_glyph_rect(
    font: &FontRef,
    gid: skrifa::GlyphId,
    px_size: f32,
    center: (f32, f32),
) -> egui::Rect {
    let m = cell_glyph_metrics(font, gid, px_size, center, 0.0);
    egui::Rect::from_min_size(
        egui::pos2(m.pen_x, m.baseline_y - m.ascent),
        egui::vec2(m.advance_w, m.ascent - m.descent),
    )
}
