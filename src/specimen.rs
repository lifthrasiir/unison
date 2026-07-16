use std::collections::BTreeMap;

use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use crate::document::{Document, DocumentItem, NamePartsMap, substitute_name_parts};
use crate::preview::rasterizer::GlyphCache;
use crate::render::ttf_builder::{expand_map_pairs, parse_map_char};

pub struct SpecimenState {
    entries: Vec<(u32, String)>,
    cached_gen: u64,
    glyph_cache: GlyphCache,
    pub hover_status: Option<String>,
}

impl SpecimenState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            cached_gen: u64::MAX,
            glyph_cache: GlyphCache::new(),
            hover_status: None,
        }
    }

    pub fn needs_rebuild(&self, font_gen: u64) -> bool {
        self.cached_gen != font_gen
    }

    pub fn rebuild_if_needed(
        &mut self,
        docs: &[&Document],
        name_parts: &NamePartsMap,
        font_gen: u64,
    ) {
        if self.cached_gen == font_gen {
            return;
        }
        self.cached_gen = font_gen;
        self.glyph_cache.invalidate_if_changed(font_gen);

        let mut map: BTreeMap<u32, String> = BTreeMap::new();
        for doc in docs {
            for item in &doc.items {
                match item {
                    DocumentItem::Map { char_repr, glyph } => {
                        let subst_glyph = substitute_name_parts(glyph, name_parts);
                        let pairs = expand_map_pairs(char_repr, &subst_glyph);
                        for (cp, glyph_name) in pairs {
                            map.entry(cp).or_insert(glyph_name);
                        }
                    }
                    DocumentItem::MapDecomposed { char_repr } => {
                        if let Some(cp) = parse_map_char(char_repr) {
                            map.entry(cp).or_insert_with(|| format!("uni{cp:04X}"));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.entries = map.into_iter().collect();
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        font_data: Option<&(Vec<u8>, Vec<u8>)>,
    ) -> Option<String> {
        self.hover_status = None;

        if self.entries.is_empty() {
            ui.label("No cmap entries.");
            return None;
        }

        let mut clicked_glyph = None;
        let cell_w = 64.0_f32;
        let cell_h = 80.0_f32;
        let avail_width = ui.available_width();
        let cols = (avail_width / cell_w).floor().max(1.0) as usize;
        let num_rows = self.entries.len().div_ceil(cols);
        let total_height = num_rows as f32 * cell_h;
        let grid_width = cols as f32 * cell_w;

        let label_font = crate::app::uniform_font_id(ui.ctx(), 16.0);
        let label_color = egui::Color32::from_gray(180);
        let glyph_color = egui::Color32::BLACK;
        let bg_color = egui::Color32::WHITE;
        let border_stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
        let px_size = 48.0_f32;

        let raster_font = font_data.map(|p| &p.1);

        crate::editor::document_view::apply_scroll_physics(ui, 1, "specimen");

        let hover_pointer = ui.input(|i| i.pointer.hover_pos());
        let ctrl_c = ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.modifiers.command && i.key_pressed(egui::Key::C))
        });

        let inner = egui::ScrollArea::vertical()
            .id_salt("specimen_scroll")
            .show(ui, |ui| {
                let (response, painter) = ui.allocate_painter(
                    egui::vec2(grid_width, total_height),
                    egui::Sense::click(),
                );
                let origin = response.rect.min;

                let clip = painter.clip_rect();
                let first_row =
                    ((clip.top() - origin.y) / cell_h).floor().max(0.0) as usize;
                let last_row =
                    ((clip.bottom() - origin.y) / cell_h).ceil().max(0.0) as usize;
                let last_row = last_row.min(num_rows);

                let vis_rect = egui::Rect::from_min_max(
                    egui::pos2(origin.x, origin.y + first_row as f32 * cell_h),
                    egui::pos2(
                        origin.x + grid_width,
                        origin.y + last_row as f32 * cell_h,
                    ),
                );
                painter.rect_filled(vis_rect, 0.0, bg_color);

                for col in 0..=cols {
                    let x = origin.x + col as f32 * cell_w;
                    painter.line_segment(
                        [
                            egui::pos2(x, vis_rect.top()),
                            egui::pos2(x, vis_rect.bottom()),
                        ],
                        border_stroke,
                    );
                }
                for row in first_row..=last_row {
                    let y = origin.y + row as f32 * cell_h;
                    painter.line_segment(
                        [
                            egui::pos2(origin.x, y),
                            egui::pos2(origin.x + grid_width, y),
                        ],
                        border_stroke,
                    );
                }

                let hovered_idx = hover_pointer.and_then(|pos| {
                    if !response.rect.contains(pos) {
                        return None;
                    }
                    let col = ((pos.x - origin.x) / cell_w).floor() as usize;
                    let row = ((pos.y - origin.y) / cell_h).floor() as usize;
                    if col >= cols {
                        return None;
                    }
                    let idx = row * cols + col;
                    if idx < self.entries.len() {
                        Some(idx)
                    } else {
                        None
                    }
                });

                struct DeferredHover {
                    cell_min: egui::Pos2,
                    cp: u32,
                    glyph_name: String,
                }
                let mut deferred_hover: Option<DeferredHover> = None;

                for row in first_row..last_row {
                    for col in 0..cols {
                        let idx = row * cols + col;
                        if idx >= self.entries.len() {
                            break;
                        }
                        let (cp, glyph_name) = &self.entries[idx];
                        let cp = *cp;
                        let is_hovered = hovered_idx == Some(idx);
                        let cell_min = egui::pos2(
                            origin.x + col as f32 * cell_w,
                            origin.y + row as f32 * cell_h,
                        );
                        let cell_rect = egui::Rect::from_min_size(
                            cell_min,
                            egui::vec2(cell_w, cell_h),
                        );

                        if is_hovered {
                            deferred_hover = Some(DeferredHover {
                                cell_min,
                                cp,
                                glyph_name: glyph_name.clone(),
                            });
                            continue;
                        }

                        self.draw_cell(
                            &painter,
                            cell_min,
                            cell_rect,
                            cell_w,
                            cell_h,
                            cp,
                            px_size,
                            false,
                            &label_font,
                            label_color,
                            glyph_color,
                            raster_font,
                            ui.ctx(),
                        );
                    }
                }

                if let Some(hover) = &deferred_hover {
                    let cell_rect = egui::Rect::from_min_size(
                        hover.cell_min,
                        egui::vec2(cell_w, cell_h),
                    );
                    painter.rect_filled(cell_rect, 0.0, egui::Color32::BLACK);

                    if let Some(gr) = self.compute_glyph_rect(
                        hover.cell_min, cell_w, cell_h, hover.cp, px_size,
                        raster_font, ui.ctx(),
                    ) {
                        let padded = gr.expand(4.0);
                        painter.rect_filled(padded, 0.0, egui::Color32::BLACK);
                    }

                    self.draw_cell(
                        &painter,
                        hover.cell_min,
                        cell_rect,
                        cell_w,
                        cell_h,
                        hover.cp,
                        px_size,
                        true,
                        &label_font,
                        label_color,
                        egui::Color32::WHITE,
                        raster_font,
                        ui.ctx(),
                    );
                }

                if response.clicked()
                    && let Some(pos) = response.interact_pointer_pos() {
                        let col =
                            ((pos.x - origin.x) / cell_w).floor() as usize;
                        let row =
                            ((pos.y - origin.y) / cell_h).floor() as usize;
                        let idx = row * cols + col;
                        if col < cols && idx < self.entries.len() {
                            clicked_glyph = Some(self.entries[idx].1.clone());
                        }
                    }

                if let Some(hover) = deferred_hover {
                    let ch = char::from_u32(hover.cp);
                    let char_str = ch.map(|c| c.to_string()).unwrap_or_default();
                    let char_name = ch
                        .and_then(unicode_names2::name)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    self.hover_status = Some(format!(
                        "U+{:04X} {} {} ({})",
                        hover.cp, char_str, char_name, hover.glyph_name,
                    ));

                    if ctrl_c
                        && let Some(ch) = ch {
                            ui.ctx().copy_text(ch.to_string());
                        }
                }
            });
        let _ = inner;

        clicked_glyph
    }

    fn draw_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        cell_rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
        cp: u32,
        px_size: f32,
        is_hovered: bool,
        label_font: &egui::FontId,
        label_color: egui::Color32,
        glyph_color: egui::Color32,
        raster_font: Option<&Vec<u8>>,
        ctx: &egui::Context,
    ) {
        let hex = format!("{cp:04X}");
        let label_galley = painter.layout_no_wrap(
            hex,
            label_font.clone(),
            label_color,
        );
        painter.galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            label_color,
        );

        let Some(ch) = char::from_u32(cp) else { return };

        let mut drawn_via_rasterizer = false;

        if let Some(font_bytes) = raster_font
            && let Ok(font) = FontRef::new(font_bytes) {
                let charmap = font.charmap();
                if let Some(gid) = charmap.map(ch) {
                    let glyph_id = gid.to_u32() as u16;
                    if let Some(cached) = self.glyph_cache.get_or_rasterize(
                        ctx,
                        font_bytes,
                        glyph_id,
                        px_size,
                        true,
                        glyph_color,
                    ) {
                        let center_x = cell_min.x + cell_w / 2.0;
                        let center_y = cell_min.y + cell_h / 2.0 + 8.0;

                        let font_metrics = font.metrics(Size::new(px_size), LocationRef::default());
                        let glyph_metrics = font.glyph_metrics(Size::new(px_size), LocationRef::default());
                        let advance_w = glyph_metrics.advance_width(gid).unwrap_or(cached.width);
                        let ascent = font_metrics.ascent;
                        let descent = font_metrics.descent;

                        let baseline_y = center_y + (ascent + descent) / 2.0;
                        let pen_x = center_x - advance_w / 2.0;

                        let draw_x = pen_x + cached.bearing_x;
                        let draw_y = baseline_y - cached.bearing_y;

                        let draw_rect = egui::Rect::from_min_size(
                            egui::pos2(draw_x, draw_y),
                            egui::vec2(cached.width, cached.height),
                        );

                        let tint = if cached.is_color {
                            egui::Color32::WHITE
                        } else {
                            glyph_color
                        };

                        if is_hovered {
                            painter.image(
                                cached.texture.id(),
                                draw_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                tint,
                            );
                        } else {
                            let sub = painter.with_clip_rect(cell_rect);
                            sub.image(
                                cached.texture.id(),
                                draw_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                tint,
                            );
                        }
                        drawn_via_rasterizer = true;
                    }
                }
            }

        if !drawn_via_rasterizer {
            let glyph_font = crate::app::uniform_font_id(ctx, px_size);
            let glyph_galley = painter.layout_no_wrap(
                ch.to_string(),
                glyph_font,
                glyph_color,
            );
            let glyph_size = glyph_galley.size();
            let center_x = cell_min.x + cell_w / 2.0;
            let center_y = cell_min.y + cell_h / 2.0 + 8.0;
            let pos = egui::pos2(
                center_x - glyph_size.x / 2.0,
                center_y - glyph_size.y / 2.0,
            );

            if is_hovered {
                painter.galley(pos, glyph_galley, glyph_color);
            } else {
                let sub = painter.with_clip_rect(cell_rect);
                sub.galley(pos, glyph_galley, glyph_color);
            }
        }
    }

    fn compute_glyph_rect(
        &mut self,
        cell_min: egui::Pos2,
        cell_w: f32,
        cell_h: f32,
        cp: u32,
        px_size: f32,
        raster_font: Option<&Vec<u8>>,
        ctx: &egui::Context,
    ) -> Option<egui::Rect> {
        let ch = char::from_u32(cp)?;
        let center_x = cell_min.x + cell_w / 2.0;
        let center_y = cell_min.y + cell_h / 2.0 + 8.0;

        if let Some(font_bytes) = raster_font
            && let Ok(font) = FontRef::new(font_bytes) {
                let charmap = font.charmap();
                if let Some(gid) = charmap.map(ch) {
                    let font_metrics = font.metrics(Size::new(px_size), LocationRef::default());
                    let glyph_metrics = font.glyph_metrics(Size::new(px_size), LocationRef::default());
                    let advance_w = glyph_metrics.advance_width(gid).unwrap_or(0.0);
                    let ascent = font_metrics.ascent;
                    let descent = font_metrics.descent;
                    let extent_h = ascent - descent;

                    let baseline_y = center_y + (ascent + descent) / 2.0;
                    let pen_x = center_x - advance_w / 2.0;

                    return Some(egui::Rect::from_min_size(
                        egui::pos2(pen_x, baseline_y - ascent),
                        egui::vec2(advance_w, extent_h),
                    ));
                }
            }

        let glyph_font = crate::app::uniform_font_id(ctx, px_size);
        let galley = ctx.fonts(|f| f.layout_no_wrap(ch.to_string(), glyph_font, egui::Color32::WHITE));
        let size = galley.size();
        Some(egui::Rect::from_min_size(
            egui::pos2(center_x - size.x / 2.0, center_y - size.y / 2.0),
            size,
        ))
    }
}
