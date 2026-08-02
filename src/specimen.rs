//! The specimen panel: every mapped character and every remap-only glyph of the
//! built font, rasterized through the shared glyph cache.
//!
//! It draws the *built font bytes* against names the background pipeline
//! resolved, so its cached cell list is keyed on the generations of those two
//! background **results** — never on the build request, which is bumped the
//! moment a document changes. `SpecimenState::cached_gen` carries the whole
//! story; get that key wrong and a remap cell draws a stale gid against fresh
//! font bytes, i.e. simply the wrong glyph.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use crate::document::{
    Document, DocumentItem, NamePartsMap, expand_name_element, substitute_name_parts,
};
use crate::editor::doc_links::LinkTargetKind;
use crate::preview::rasterizer::GlyphCache;
use crate::render::ttf_builder::{decomposed_map_pairs, expand_map_pairs};

struct RemapEntry {
    label: String,
    glyph_name: String,
    feature: String,
    gid: u16,
    cp_sequence: Option<Vec<u32>>,
}

pub struct SpecimenClick {
    pub name: String,
    pub kind: LinkTargetKind,
}

pub struct SpecimenState {
    entries: Vec<(u32, String)>,
    remap_entries: Vec<RemapEntry>,
    /// `(font_data_gen, derived_gen)` — the generations of the *two* background
    /// results the rebuild reads, never the generation of the build *request*.
    /// A remap-only glyph is listed only if `name_to_gid` knows its (name-part
    /// expanded) name, so a rebuild keyed on the request would drop nearly all
    /// of them whenever the specimen is opened while a build is in flight — or
    /// at startup, where `name_parts` is empty until the first derive lands —
    /// and would then never run again to fix it.
    cached_gen: Option<(u64, u64)>,
    glyph_cache: GlyphCache,
    pub hover_status: Option<String>,
}

impl SpecimenState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            remap_entries: Vec::new(),
            cached_gen: None,
            glyph_cache: GlyphCache::new(),
            hover_status: None,
        }
    }

    pub fn needs_rebuild(&self, font_data_gen: u64, derived_gen: u64) -> bool {
        self.cached_gen != Some((font_data_gen, derived_gen))
    }

    pub fn rebuild_if_needed(
        &mut self,
        docs: &[&Document],
        name_parts: &NamePartsMap,
        name_to_gid: &HashMap<String, u16>,
        font_data_gen: u64,
        derived_gen: u64,
    ) {
        if !self.needs_rebuild(font_data_gen, derived_gen) {
            return;
        }
        self.cached_gen = Some((font_data_gen, derived_gen));

        let mut map: BTreeMap<u32, String> = BTreeMap::new();
        let mut mapped_glyphs: HashSet<String> = HashSet::new();
        for doc in docs {
            for item in &doc.items {
                match item {
                    DocumentItem::Map {
                        char_repr, glyph, ..
                    } => {
                        let subst_glyph = substitute_name_parts(glyph, name_parts);
                        let pairs = expand_map_pairs(char_repr, &subst_glyph);
                        for (cp, glyph_name) in pairs {
                            mapped_glyphs.insert(glyph_name.clone());
                            map.entry(cp).or_insert(glyph_name);
                        }
                    }
                    DocumentItem::MapDecomposed {
                        char_repr, glyph, ..
                    } => {
                        let subst = glyph.as_ref().map(|g| substitute_name_parts(g, name_parts));
                        for (cp, name) in decomposed_map_pairs(char_repr, subst.as_deref()) {
                            mapped_glyphs.insert(name.clone());
                            map.entry(cp).or_insert(name);
                        }
                    }
                    _ => {}
                }
            }
        }
        self.entries = map.into_iter().collect();

        // Build reverse map: glyph_name → smallest codepoint.
        let mut glyph_to_cp: HashMap<&str, u32> = HashMap::new();
        for (cp, glyph_name) in &self.entries {
            glyph_to_cp.entry(glyph_name.as_str()).or_insert(*cp);
        }

        // Collect remap-only glyph names and their originating feature.
        let mut remap_only: BTreeSet<String> = BTreeSet::new();

        // Context-free remap rules (no lookbehind/lookahead) are eligible
        // for codepoint-sequence labels.
        struct RemapRule {
            source: Vec<String>,
            target: Vec<String>,
        }
        let mut ligature_rules: Vec<RemapRule> = Vec::new();
        // feature name for each remap-only glyph (first seen wins).
        let mut glyph_feature: HashMap<String, String> = HashMap::new();

        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Remap {
                    feature,
                    source,
                    target,
                    lookbehind,
                    lookahead,
                    ..
                } = item
                {
                    let tgt_expanded: Vec<Vec<String>> = target
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();
                    let src_expanded: Vec<Vec<String>> = source
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();

                    let max_len = src_expanded
                        .iter()
                        .chain(tgt_expanded.iter())
                        .map(|v| v.len())
                        .max()
                        .unwrap_or(1);

                    let has_context = !lookbehind.is_empty() || !lookahead.is_empty();

                    for i in 0..max_len {
                        let tgt: Vec<String> = tgt_expanded
                            .iter()
                            .map(|v| v[i % v.len()].clone())
                            .collect();
                        for name in &tgt {
                            if !mapped_glyphs.contains(name) {
                                remap_only.insert(name.clone());
                                glyph_feature
                                    .entry(name.clone())
                                    .or_insert_with(|| feature.clone());
                            }
                        }
                        if !has_context {
                            let src: Vec<String> = src_expanded
                                .iter()
                                .map(|v| v[i % v.len()].clone())
                                .collect();
                            ligature_rules.push(RemapRule {
                                source: src,
                                target: tgt,
                            });
                        }
                    }
                }
            }
        }

        // Build remap entries, trying to compute codepoint sequences.
        let mut with_cp: Vec<RemapEntry> = Vec::new();
        let mut without_cp: Vec<RemapEntry> = Vec::new();

        for glyph_name in remap_only {
            let Some(&gid) = name_to_gid.get(&glyph_name) else {
                continue;
            };
            let feature = glyph_feature.get(&glyph_name).cloned().unwrap_or_default();

            // Find a context-free remap rule where this glyph appears in
            // the target and all source glyphs have direct cmap mappings.
            let cp_seq = ligature_rules.iter().find_map(|rule| {
                if !rule.target.contains(&glyph_name) {
                    return None;
                }
                rule.source
                    .iter()
                    .map(|s| glyph_to_cp.get(s.as_str()).copied())
                    .collect::<Option<Vec<u32>>>()
            });

            let label = if let Some(ref cps) = cp_seq {
                let hex = cps
                    .iter()
                    .map(|cp| format!("{cp:04X}"))
                    .collect::<Vec<_>>()
                    .join("+");
                format!("{hex} ({glyph_name})")
            } else {
                glyph_name.clone()
            };

            let entry = RemapEntry {
                label,
                glyph_name,
                feature,
                gid,
                cp_sequence: cp_seq.clone(),
            };
            if cp_seq.is_some() {
                with_cp.push(entry);
            } else {
                without_cp.push(entry);
            }
        }

        // Sort ligature remaps by codepoint sequence, then append others
        // (already sorted by glyph name via BTreeSet).
        with_cp.sort_by(|a, b| a.cp_sequence.cmp(&b.cp_sequence));
        self.remap_entries = with_cp;
        self.remap_entries.append(&mut without_cp);
    }

    #[cfg(test)]
    fn remap_glyph_names(&self) -> Vec<&str> {
        self.remap_entries
            .iter()
            .map(|e| e.glyph_name.as_str())
            .collect()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        font_data: Option<&(Vec<u8>, Vec<u8>)>,
        font_data_gen: u64,
    ) -> Option<SpecimenClick> {
        self.glyph_cache.invalidate_if_changed(font_data_gen);
        self.hover_status = None;

        let total_count = self.entries.len() + self.remap_entries.len();
        if total_count == 0 {
            ui.label("No cmap entries.");
            return None;
        }

        let mut clicked: Option<SpecimenClick> = None;
        let cell_w = 64.0_f32;
        let cell_h = 80.0_f32;
        let avail_width = ui.available_width();
        let cols = (avail_width / cell_w).floor().max(1.0) as usize;
        let num_rows = total_count.div_ceil(cols);
        let total_height = num_rows as f32 * cell_h;
        let grid_width = cols as f32 * cell_w;

        let label_font = crate::app::uniform_font_id(ui.ctx(), 16.0);
        let label_color = egui::Color32::from_gray(180);
        let glyph_color = egui::Color32::BLACK;
        let bg_color = egui::Color32::WHITE;
        let border_stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
        let px_size = 48.0_f32;

        let raster_font = font_data.map(|p| &p.1);

        crate::editor::document_view::apply_scroll_physics(
            ui,
            1,
            egui::Id::new("specimen_scroll_accel"),
        );

        let hover_pointer = ui.input(|i| i.pointer.hover_pos());
        let ctrl_c = ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.modifiers.command && i.key_pressed(egui::Key::C))
        });

        let cmap_count = self.entries.len();

        let inner = egui::ScrollArea::vertical()
            .id_salt("specimen_scroll")
            .show(ui, |ui| {
                // Own the full width even though only `cols` boxes fit: the
                // painter's clip rect is the allocated area, and a hovered cell
                // deliberately overflows its neighbors, so a rect ending at
                // `grid_width` would cut the rightmost column's overflow off.
                // The slack to the right is filled with the same background.
                let (response, painter) = ui.allocate_painter(
                    egui::vec2(avail_width.max(grid_width), total_height),
                    egui::Sense::click(),
                );
                let origin = response.rect.min;

                let clip = painter.clip_rect();
                let first_row = ((clip.top() - origin.y) / cell_h).floor().max(0.0) as usize;
                let last_row = ((clip.bottom() - origin.y) / cell_h).ceil().max(0.0) as usize;
                let last_row = last_row.min(num_rows);

                let vis_rect = egui::Rect::from_min_max(
                    egui::pos2(origin.x, origin.y + first_row as f32 * cell_h),
                    egui::pos2(response.rect.right(), origin.y + last_row as f32 * cell_h),
                );
                painter.rect_filled(vis_rect, 0.0, bg_color);

                // A border stroke is centred on its line, so an outermost one
                // sitting exactly on the allocated rect's edge loses half its
                // width to the clip; inset those (and only those) inward.
                let half = border_stroke.width / 2.0;
                let clamp_x =
                    |x: f32| x.clamp(response.rect.left() + half, response.rect.right() - half);
                let clamp_y =
                    |y: f32| y.clamp(response.rect.top() + half, response.rect.bottom() - half);
                let line_right = clamp_x(origin.x + grid_width);
                for col in 0..=cols {
                    let x = clamp_x(origin.x + col as f32 * cell_w);
                    painter.line_segment(
                        [
                            egui::pos2(x, clamp_y(vis_rect.top())),
                            egui::pos2(x, clamp_y(vis_rect.bottom())),
                        ],
                        border_stroke,
                    );
                }
                for row in first_row..=last_row {
                    let y = clamp_y(origin.y + row as f32 * cell_h);
                    painter.line_segment(
                        [egui::pos2(clamp_x(origin.x), y), egui::pos2(line_right, y)],
                        border_stroke,
                    );
                }

                // `response.rect` is the *content* rect, which extends past the
                // scroll viewport on both sides once the grid is scrolled, so
                // it contains points that are over the editor above instead.
                // `contains_pointer` respects the clip rect and the layer
                // order, so the cell under the pointer is the one on screen.
                let hovered_idx = hover_pointer
                    .filter(|_| response.contains_pointer())
                    .and_then(|pos| {
                        if !response.rect.contains(pos) {
                            return None;
                        }
                        let col = ((pos.x - origin.x) / cell_w).floor() as usize;
                        let row = ((pos.y - origin.y) / cell_h).floor() as usize;
                        if col >= cols {
                            return None;
                        }
                        let idx = row * cols + col;
                        if idx < total_count { Some(idx) } else { None }
                    });

                enum DeferredHover {
                    Cmap {
                        cell_min: egui::Pos2,
                        cp: u32,
                        glyph_name: String,
                    },
                    Remap {
                        cell_min: egui::Pos2,
                        remap_idx: usize,
                    },
                }
                let mut deferred_hover: Option<DeferredHover> = None;
                let style = CellStyle {
                    cell_w,
                    cell_h,
                    px_size,
                    label_font: &label_font,
                    label_color,
                    raster_font,
                    ctx: ui.ctx(),
                };

                for row in first_row..last_row {
                    for col in 0..cols {
                        let idx = row * cols + col;
                        if idx >= total_count {
                            break;
                        }
                        let is_hovered = hovered_idx == Some(idx);
                        let cell_min = egui::pos2(
                            origin.x + col as f32 * cell_w,
                            origin.y + row as f32 * cell_h,
                        );
                        if idx < cmap_count {
                            let (cp, glyph_name) = &self.entries[idx];
                            let cp = *cp;
                            if is_hovered {
                                deferred_hover = Some(DeferredHover::Cmap {
                                    cell_min,
                                    cp,
                                    glyph_name: glyph_name.clone(),
                                });
                                continue;
                            }
                            self.draw_cell(&painter, cell_min, cp, false, glyph_color, &style);
                        } else {
                            let ri = idx - cmap_count;
                            if is_hovered {
                                deferred_hover = Some(DeferredHover::Remap {
                                    cell_min,
                                    remap_idx: ri,
                                });
                                continue;
                            }
                            self.draw_remap_cell(
                                &painter,
                                cell_min,
                                ri,
                                false,
                                glyph_color,
                                &style,
                            );
                        }
                    }
                }

                if let Some(hover) = &deferred_hover {
                    match hover {
                        DeferredHover::Cmap { cell_min, cp, .. } => {
                            painter.rect_filled(
                                style.cell_rect(*cell_min),
                                0.0,
                                egui::Color32::BLACK,
                            );
                            if let Some(gr) = self.compute_glyph_rect(*cell_min, *cp, &style) {
                                painter.rect_filled(gr.expand(4.0), 0.0, egui::Color32::BLACK);
                            }
                            self.draw_cell(
                                &painter,
                                *cell_min,
                                *cp,
                                true,
                                egui::Color32::WHITE,
                                &style,
                            );
                        }
                        DeferredHover::Remap {
                            cell_min,
                            remap_idx,
                        } => {
                            let ri = *remap_idx;
                            painter.rect_filled(
                                style.cell_rect(*cell_min),
                                0.0,
                                egui::Color32::BLACK,
                            );
                            if let Some(gr) = self.compute_remap_glyph_rect(*cell_min, ri, &style) {
                                painter.rect_filled(gr.expand(4.0), 0.0, egui::Color32::BLACK);
                            }
                            // Expand black background for the label too.
                            let label_text = &self.remap_entries[ri].label;
                            let label_galley = painter.layout_no_wrap(
                                label_text.clone(),
                                label_font.clone(),
                                label_color,
                            );
                            let lw = label_galley.size().x + 4.0;
                            if lw > cell_w {
                                let label_bg = egui::Rect::from_min_size(
                                    egui::pos2(cell_min.x, cell_min.y),
                                    egui::vec2(lw, label_galley.size().y + 2.0),
                                );
                                painter.rect_filled(label_bg, 0.0, egui::Color32::BLACK);
                            }
                            self.draw_remap_cell(
                                &painter,
                                *cell_min,
                                ri,
                                true,
                                egui::Color32::WHITE,
                                &style,
                            );
                        }
                    }
                }

                if response.clicked()
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    let col = ((pos.x - origin.x) / cell_w).floor() as usize;
                    let row = ((pos.y - origin.y) / cell_h).floor() as usize;
                    let idx = row * cols + col;
                    if col < cols && idx < total_count {
                        if idx < cmap_count {
                            clicked = Some(SpecimenClick {
                                name: self.entries[idx].1.clone(),
                                kind: LinkTargetKind::Glyph,
                            });
                        } else {
                            let entry = &self.remap_entries[idx - cmap_count];
                            clicked = Some(SpecimenClick {
                                name: entry.feature.clone(),
                                kind: LinkTargetKind::Remap,
                            });
                        }
                    }
                }

                if let Some(hover) = deferred_hover {
                    match hover {
                        DeferredHover::Cmap { cp, glyph_name, .. } => {
                            let ch = char::from_u32(cp);
                            let char_str = ch.map(|c| c.to_string()).unwrap_or_default();
                            let char_name = ch
                                .and_then(unicode_names2::name)
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "<unknown>".to_string());
                            self.hover_status = Some(format!(
                                "U+{:04X} {} {} ({})",
                                cp, char_str, char_name, glyph_name,
                            ));
                            if ctrl_c && let Some(ch) = ch {
                                ui.ctx().copy_text(ch.to_string());
                            }
                        }
                        DeferredHover::Remap { remap_idx, .. } => {
                            let entry = &self.remap_entries[remap_idx];
                            if let Some(ref cps) = entry.cp_sequence {
                                let parts: Vec<String> = cps
                                    .iter()
                                    .map(|cp| {
                                        let ch = char::from_u32(*cp);
                                        let char_name = ch
                                            .and_then(unicode_names2::name)
                                            .map(|n| n.to_string())
                                            .unwrap_or_else(|| "<unknown>".to_string());
                                        format!("U+{cp:04X} {char_name}")
                                    })
                                    .collect();
                                self.hover_status =
                                    Some(format!("{} ({})", parts.join(" + "), entry.glyph_name,));
                            } else {
                                self.hover_status =
                                    Some(format!("{} (remap-only)", entry.glyph_name,));
                            }
                        }
                    }
                }
            });
        let _ = inner;

        clicked
    }

    fn draw_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        cp: u32,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let px_size = style.px_size;
        let ctx = style.ctx;
        let hex = format!("{cp:04X}");
        let label_galley = painter.layout_no_wrap(hex, style.label_font.clone(), style.label_color);
        painter.galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            style.label_color,
        );

        let Some(ch) = char::from_u32(cp) else { return };
        let center = style.cell_center(cell_min);

        let mut drawn_via_rasterizer = false;

        if let Some(font_bytes) = style.raster_font
            && let Ok(font) = FontRef::new(font_bytes)
            && let Some(gid) = font.charmap().map(ch)
        {
            drawn_via_rasterizer = self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                &font,
                font_bytes,
                gid,
                px_size,
                is_hovered,
                glyph_color,
                ctx,
            );
        }

        if !drawn_via_rasterizer {
            let glyph_font = crate::app::uniform_font_id(ctx, px_size);
            let glyph_galley = painter.layout_no_wrap(ch.to_string(), glyph_font, glyph_color);
            let glyph_size = glyph_galley.size();
            let pos = egui::pos2(center.0 - glyph_size.x / 2.0, center.1 - glyph_size.y / 2.0);
            cell_painter(painter, cell_rect, is_hovered).galley(pos, glyph_galley, glyph_color);
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
                style.px_size,
                is_hovered,
                glyph_color,
                style.ctx,
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
        px_size: f32,
        is_hovered: bool,
        glyph_color: egui::Color32,
        ctx: &egui::Context,
    ) -> bool {
        let Some(cached) = self.glyph_cache.get_or_rasterize(
            ctx,
            font_bytes,
            gid.to_u32() as u16,
            px_size,
            true,
            glyph_color,
        ) else {
            return false;
        };

        let m = cell_glyph_metrics(font, gid, px_size, center, cached.width);
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

    fn compute_glyph_rect(
        &mut self,
        cell_min: egui::Pos2,
        cp: u32,
        style: &CellStyle<'_>,
    ) -> Option<egui::Rect> {
        let ch = char::from_u32(cp)?;
        let center = style.cell_center(cell_min);

        if let Some(font_bytes) = style.raster_font
            && let Ok(font) = FontRef::new(font_bytes)
            && let Some(gid) = font.charmap().map(ch)
        {
            return Some(raster_glyph_rect(&font, gid, style.px_size, center));
        }

        let ctx = style.ctx;
        let glyph_font = crate::app::uniform_font_id(ctx, style.px_size);
        let galley =
            ctx.fonts(|f| f.layout_no_wrap(ch.to_string(), glyph_font, egui::Color32::WHITE));
        let size = galley.size();
        Some(egui::Rect::from_min_size(
            egui::pos2(center.0 - size.x / 2.0, center.1 - size.y / 2.0),
            size,
        ))
    }

    fn compute_remap_glyph_rect(
        &self,
        cell_min: egui::Pos2,
        remap_idx: usize,
        style: &CellStyle<'_>,
    ) -> Option<egui::Rect> {
        let gid = self.remap_entries[remap_idx].gid;
        let font = FontRef::new(style.raster_font?).ok()?;
        Some(raster_glyph_rect(
            &font,
            skrifa::GlyphId::new(gid as u32),
            style.px_size,
            style.cell_center(cell_min),
        ))
    }
}

/// The glyph anchor point of a specimen cell: horizontally centered, nudged
/// below center to leave room for the codepoint label.
fn cell_center(cell_min: egui::Pos2, cell_w: f32, cell_h: f32) -> (f32, f32) {
    (cell_min.x + cell_w / 2.0, cell_min.y + cell_h / 2.0 + 8.0)
}

/// Everything a specimen cell is drawn with that is the same for every cell of
/// one grid: cell size, glyph size, label style and the font to raster from.
struct CellStyle<'a> {
    cell_w: f32,
    cell_h: f32,
    px_size: f32,
    label_font: &'a egui::FontId,
    label_color: egui::Color32,
    raster_font: Option<&'a Vec<u8>>,
    ctx: &'a egui::Context,
}

impl CellStyle<'_> {
    fn cell_rect(&self, cell_min: egui::Pos2) -> egui::Rect {
        egui::Rect::from_min_size(cell_min, egui::vec2(self.cell_w, self.cell_h))
    }

    fn cell_center(&self, cell_min: egui::Pos2) -> (f32, f32) {
        cell_center(cell_min, self.cell_w, self.cell_h)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Document {
        crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap()
    }

    const SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
name-parts $l = a b
glyph sq 1 1
@@
glyph a-lig
ref sq
glyph b-lig
ref sq
map U+0061 = sq
remap liga : sq -> ($l)-lig
";

    /// The gid map and `name_parts` arrive from *background* work, so the
    /// specimen can be opened while both are still the previous build's (or,
    /// at startup, empty). Keying its cache on the build request would then
    /// freeze that half-built state in place forever.
    #[test]
    fn rebuilds_when_name_parts_and_gids_arrive_late() {
        let d = doc(SRC);
        let docs = [&d];
        let name_parts = crate::document::collect_name_parts(&docs);
        let gids: HashMap<String, u16> = [
            ("sq".to_string(), 1u16),
            ("a-lig".to_string(), 2),
            ("b-lig".to_string(), 3),
        ]
        .into_iter()
        .collect();

        let mut state = SpecimenState::new();

        // Frame 1: opened before the background build landed — no name parts,
        // no gid map yet.
        assert!(state.needs_rebuild(0, 0));
        state.rebuild_if_needed(&docs, &NamePartsMap::new(), &HashMap::new(), 0, 0);
        assert!(state.remap_glyph_names().is_empty());

        // Frame 2: the build and the derived data have landed.
        assert!(state.needs_rebuild(1, 1));
        state.rebuild_if_needed(&docs, &name_parts, &gids, 1, 1);
        assert_eq!(state.remap_glyph_names(), vec!["a-lig", "b-lig"]);

        // Nothing new: the cache holds.
        assert!(!state.needs_rebuild(1, 1));
    }
}
