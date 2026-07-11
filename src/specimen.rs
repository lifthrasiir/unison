use std::collections::BTreeMap;

use crate::document::{Document, DocumentItem, NamePartsMap, substitute_name_parts};
use crate::render::ttf_builder::expand_map_pairs;

pub struct SpecimenState {
    entries: Vec<(u32, String)>,
    cached_gen: u64,
}

impl SpecimenState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            cached_gen: u64::MAX,
        }
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

        let mut map: BTreeMap<u32, String> = BTreeMap::new();
        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Map { char_repr, glyph } = item {
                    let subst_glyph = substitute_name_parts(glyph, name_parts);
                    let pairs = expand_map_pairs(char_repr, &subst_glyph);
                    for (cp, glyph_name) in pairs {
                        map.entry(cp).or_insert(glyph_name);
                    }
                }
            }
        }
        self.entries = map.into_iter().collect();
    }

    pub fn show(&self, ui: &mut egui::Ui) -> Option<String> {
        if self.entries.is_empty() {
            ui.label("No cmap entries.");
            return None;
        }

        let mut clicked_glyph = None;
        let cell_w = 64.0_f32;
        let cell_h = 80.0_f32;
        let avail_width = ui.available_width();
        let cols = (avail_width / cell_w).floor().max(1.0) as usize;
        let num_rows = (self.entries.len() + cols - 1) / cols;
        let total_height = num_rows as f32 * cell_h;
        let grid_width = cols as f32 * cell_w;

        let label_font = crate::app::uniform_font_id(ui.ctx(), 16.0);
        let glyph_font = crate::app::uniform_font_id(ui.ctx(), 48.0);
        let label_color = egui::Color32::from_gray(180);
        let glyph_color = egui::Color32::BLACK;
        let bg_color = egui::Color32::WHITE;
        let border_stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);

        crate::editor::document_view::apply_scroll_physics(ui, 1, "specimen");

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

                for row in first_row..last_row {
                    for col in 0..cols {
                        let idx = row * cols + col;
                        if idx >= self.entries.len() {
                            break;
                        }
                        let (cp, _) = &self.entries[idx];
                        let cp = *cp;
                        let cell_min = egui::pos2(
                            origin.x + col as f32 * cell_w,
                            origin.y + row as f32 * cell_h,
                        );

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

                        if let Some(ch) = char::from_u32(cp) {
                            let glyph_galley = painter.layout_no_wrap(
                                ch.to_string(),
                                glyph_font.clone(),
                                glyph_color,
                            );
                            let glyph_size = glyph_galley.size();
                            let center_x = cell_min.x + cell_w / 2.0;
                            let center_y = cell_min.y + cell_h / 2.0 + 8.0;
                            painter.galley(
                                egui::pos2(
                                    center_x - glyph_size.x / 2.0,
                                    center_y - glyph_size.y / 2.0,
                                ),
                                glyph_galley,
                                glyph_color,
                            );
                        }
                    }
                }

                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let col =
                            ((pos.x - origin.x) / cell_w).floor() as usize;
                        let row =
                            ((pos.y - origin.y) / cell_h).floor() as usize;
                        let idx = row * cols + col;
                        if col < cols && idx < self.entries.len() {
                            clicked_glyph =
                                Some(self.entries[idx].1.clone());
                        }
                    }
                }
            });
        let _ = inner;

        clicked_glyph
    }
}
