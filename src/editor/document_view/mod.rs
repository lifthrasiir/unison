//! The document view: the `show_document` frame loop and everything it
//! paints, scrolls and edits.

use std::collections::HashMap;

use crate::document::{DocLine, Document, DocumentItem, GlyphPoint, NamePartsMap, PixelGrid};
use crate::document_io::{self, tokenize_with_spans};
use crate::editor::annotations::{AnnotatedText, InlineAnnotation};
use crate::editor::caret::{self, Caret};
use crate::render::ttf_builder::ColorAliasMap;
use crate::editor::doc_input;
use crate::editor::doc_links::{self, LinkSpan, LinkTargetKind, RenameKind};
use crate::editor::grid_render;
use crate::editor::inline_tools;
use crate::editor::minimap;
use crate::editor::pixel_interaction;
use crate::editor::pixel_selection;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};
use crate::editor::visual_lines;
use crate::editor::{EditMode, EditorState, PopupState};
use crate::pixel;

mod changes;
mod keys;
mod layout;
mod paint;
mod popups;
mod scroll;
#[cfg(test)]
mod tests;

use changes::{apply_pending_rederive, line_to_item_idx, source_line_offsets};
use keys::handle_document_keys;
use layout::{ViewCacheKey, ViewData};
use paint::paint_document_area;
use popups::{show_autocomplete_popup, show_error_tooltip, show_rename_popup};
use scroll::{
    handle_page_scroll, lock_scroll_gesture_zone, resolve_scroll_target, scroll_cursor_into_view,
};

// Re-exported so the rest of the editor keeps addressing these as
// `document_view::*`, whichever submodule they now live in.
pub(crate) use changes::flush_document_changes;
pub(crate) use layout::{
    GridExtent, GridStrip, VLineKind, ViewCache, VisualLine, compute_grid_display_extent,
};
#[cfg(test)]
pub(crate) use layout::gutter_line_number;
pub(crate) use scroll::{apply_scroll_physics, debounced_scroll_step, interceptor_scroll_step};

pub(crate) const UNFILLED_OPACITY: f32 = 0.35;

pub(crate) const GRID_CELL: f32 = 14.0;

pub(crate) const LEFT_PAD: f32 = 4.0;

pub(crate) const PREVIEW_SCALE: f32 = 2.0;

const MINIMAP_WIDTH: f32 = 64.0;

const INLINE_PANEL_GAP: f32 = 12.0;

pub(crate) const INLINE_PALETTE_CELL: f32 = 16.0;

use super::colors::Palette;

enum ClickTarget {
    Text(Caret),
    Grid { item_idx: usize },
}

pub struct GotoGlyph {
    pub name: String,
    pub kind: LinkTargetKind,
}

pub struct RenameAction {
    pub old_name: String,
    pub new_name: String,
    pub kind: RenameKind,
}

pub struct DocumentViewResult {
    pub goto: Option<GotoGlyph>,
    pub rename: Option<RenameAction>,
}

#[allow(clippy::too_many_arguments)]
/// The cached-or-rebuilt derived view (composites, visual lines, source
/// offsets) for the current document revision and view parameters.
#[expect(clippy::too_many_arguments)]
fn resolve_view(
    ctx: &egui::Context,
    doc: &Document,
    lines: &[DocLine],
    state: &mut EditorState,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &crate::editor::ref_composite::AlternativesIndex,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
    cache_key: ViewCacheKey,
    editing_item_idx: Option<usize>,
    zoom_level: u32,
    pal: &Palette,
    wrap_width: Option<f32>,
    font_id: &egui::FontId,
) -> std::sync::Arc<ViewData> {
    // An external mutation of `lines` (menu action, rename, …) queues a sync
    // request; the cached view predates that mutation, so rebuild.
    let cache_valid = !state.document_sync_requested
        && state
            .view_cache
            .as_ref()
            .is_some_and(|c| c.key == cache_key);
    if cache_valid {
        return std::sync::Arc::clone(&state.view_cache.as_ref().unwrap().data);
    }
    let composites =
        grid_render::build_composites(doc, named_glyphs, name_parts, alt_index, color_aliases);
    let vlines = visual_lines::build_visual_lines(
        lines,
        doc,
        &doc.item_line_starts,
        &composites,
        named_glyphs,
        name_parts,
        editing_item_idx,
        zoom_level,
        pal,
        wrap_width,
        ctx,
        font_id,
    );
    let source_offsets = source_line_offsets(lines);
    let data = std::sync::Arc::new(ViewData {
        composites,
        vlines,
        source_offsets,
    });
    state.view_cache = Some(ViewCache {
        key: cache_key,
        data: std::sync::Arc::clone(&data),
    });
    data
}

pub fn show_document(
    ui: &mut egui::Ui,
    doc: &mut Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &crate::editor::ref_composite::AlternativesIndex,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
    derived_gen: u64,
    font_gen: u64,
    zoom_level: u32,
    font_id: &egui::FontId,
) -> DocumentViewResult {
    Palette::store(ui.ctx());
    let pal = Palette::get(ui);

    if state.suppress_font_rebuild
        && !ui.input(|i| i.pointer.primary_down() || i.pointer.secondary_down())
    {
        state.suppress_font_rebuild = false;
    }

    let grid_cell = GRID_CELL * zoom_level as f32;
    let font_id = font_id.clone();
    let row_height = ui.fonts(|f| f.row_height(&font_id));
    let text_color = ui.visuals().text_color();
    let cursor_color = text_color;

    let editing_item_idx = match &state.mode {
        EditMode::GlyphEdit { item_idx, .. } => Some(*item_idx),
        EditMode::PixelSelect { item_idx } => Some(*item_idx),
        EditMode::LayerMove { item_idx, .. } => Some(*item_idx),
        EditMode::Normal => None,
    };

    let gutter_width = ui.fonts(|f| {
        f.layout_no_wrap("88888 ".to_string(), font_id.clone(), egui::Color32::WHITE)
            .rect
            .width()
    });

    let wrap_width = {
        let minimap_w = MINIMAP_WIDTH * zoom_level as f32;
        let text_area = ui.available_width() - minimap_w - gutter_width - LEFT_PAD - 16.0;
        if text_area > 0.0 {
            Some(text_area)
        } else {
            None
        }
    };

    let cache_key = ViewCacheKey {
        edit_gen: doc.edit_gen,
        derived_gen,
        font_gen,
        zoom_level,
        editing_item_idx,
        wrap_width_bits: wrap_width.map(f32::to_bits),
        font_id: font_id.clone(),
        dark_mode: ui.ctx().theme() == egui::Theme::Dark,
        ppp_bits: ui.ctx().pixels_per_point().to_bits(),
    };
    let view = resolve_view(
        ui.ctx(),
        doc,
        lines,
        state,
        named_glyphs,
        name_parts,
        alt_index,
        color_aliases,
        cache_key,
        editing_item_idx,
        zoom_level,
        &pal,
        wrap_width,
        &font_id,
    );
    let composites = &view.composites;
    let vlines: &[VisualLine] = &view.vlines;
    let source_offsets: &[usize] = &view.source_offsets;

    let total_height: f32 = vlines
        .iter()
        .map(|vl| vl.height(row_height, grid_cell))
        .sum();

    let inline_panel_edit_idx = editing_item_idx;

    // Menu actions can mutate `lines` after this view was rendered in the
    // preceding frame. Consume their queued synchronization here so the
    // derived document cannot remain stale.
    let mut needs_rederive = state.take_document_sync_request();

    let prev_scroll_y = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(egui::Id::new("doc_scroll_y")))
        .unwrap_or(0.0);
    let prev_viewport_h = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(egui::Id::new("doc_viewport_h")))
        .unwrap_or(200.0);

    apply_scroll_physics(ui, zoom_level, "editor");

    let mut minimap_scroll_target: Option<f32> = None;
    egui::SidePanel::right("minimap")
        .exact_width(MINIMAP_WIDTH * zoom_level as f32)
        .resizable(false)
        .show_inside(ui, |ui| {
            minimap_scroll_target = minimap::draw_minimap(
                ui,
                vlines,
                doc,
                composites,
                total_height,
                prev_scroll_y,
                prev_viewport_h,
                zoom_level,
            );
        });

    lock_scroll_gesture_zone(ui, state.grid_hover);

    let viewport_h = ui.available_height();
    let mut scroll_area_builder = egui::ScrollArea::vertical().auto_shrink([false, false]);

    handle_page_scroll(
        ui, lines, state, vlines, row_height, grid_cell, prev_scroll_y, prev_viewport_h,
    );

    if let Some(target) = resolve_scroll_target(
        ui,
        state,
        vlines,
        row_height,
        grid_cell,
        zoom_level,
        prev_scroll_y,
        viewport_h,
        total_height,
        minimap_scroll_target,
    ) {
        scroll_area_builder = scroll_area_builder.vertical_scroll_offset(target);
    }

    let scroll_output = scroll_area_builder.show(ui, |ui| {
        paint_document_area(
            ui,
            doc,
            lines,
            state,
            vlines,
            composites,
            source_offsets,
            named_glyphs,
            name_parts,
            color_aliases,
            &pal,
            &font_id,
            row_height,
            grid_cell,
            gutter_width,
            total_height,
            viewport_h,
            zoom_level,
            cursor_color,
            inline_panel_edit_idx,
            &mut needs_rederive,
        );
    });

    if total_height > 0.0 {
        state.saved_scroll_frac = (scroll_output.state.offset.y + viewport_h / 2.0) / total_height;
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new("doc_scroll_y"), scroll_output.state.offset.y);
        d.insert_temp(egui::Id::new("doc_viewport_h"), viewport_h);
    });

    let prev_cursor = state.cursor;
    handle_document_keys(
        ui, doc, lines, state, named_glyphs, name_parts, prev_cursor, &mut needs_rederive,
    );

    let rename_result = show_rename_popup(ui, state);
    show_autocomplete_popup(ui, lines, state, &mut needs_rederive);
    show_error_tooltip(ui, state, &pal);
    scroll_cursor_into_view(
        ui,
        state,
        vlines,
        row_height,
        grid_cell,
        prev_cursor,
        scroll_output.state.offset.y,
        viewport_h,
    );
    apply_pending_rederive(doc, lines, state, needs_rederive);

    state.cursor = caret::clamp(lines, state.cursor);
    state.cursor_item = line_to_item_idx(&doc.item_line_starts, state.cursor.line);
    state.cursor_source_line = source_offsets
        .get(state.cursor.line)
        .map(|&off| off + 1)
        .unwrap_or(1);

    let cross_file_id = egui::Id::new("goto_cross_file");
    let goto_request: Option<String> = ui.ctx().data(|d| d.get_temp(cross_file_id));
    if let Some(name) = goto_request {
        let kind_u8: u8 = ui
            .ctx()
            .data(|d| d.get_temp(egui::Id::new("goto_cross_file_kind")).unwrap_or(0));
        let kind = match kind_u8 {
            1 => LinkTargetKind::NameParts,
            2 => LinkTargetKind::Remap,
            _ => LinkTargetKind::Glyph,
        };
        ui.ctx().data_mut(|d| {
            d.remove::<String>(cross_file_id);
            d.remove::<u8>(egui::Id::new("goto_cross_file_kind"));
        });
        return DocumentViewResult {
            goto: Some(GotoGlyph { name, kind }),
            rename: rename_result,
        };
    }
    DocumentViewResult { goto: None, rename: rename_result }
}
