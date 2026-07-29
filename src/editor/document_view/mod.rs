//! The document view: the `show_document` frame loop and everything it
//! paints, scrolls and edits.

use std::collections::HashMap;

use crate::document::{
    DocLine, Document, DocumentItem, GlyphBody, GlyphPoint, NamePartsMap, PixelGrid,
};
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
use crate::editor::{EditMode, EditorState, PopupState, Slot};
use crate::pixel;

mod changes;
mod keys;
mod layout;
mod number_scroll;
mod paint;
mod popups;
mod scroll;
#[cfg(test)]
mod tests;

use changes::{apply_pending_rederive, line_to_item_idx, source_line_offsets};
use keys::handle_document_keys;
use layout::{ViewCacheKey, ViewData};
use number_scroll::{apply_number_bump, detect_number_bump, swallow_wheel_delta};
use paint::paint_document_area;
use popups::{show_autocomplete_popup, show_error_tooltip, show_rename_popup};
use scroll::{
    handle_page_scroll, lock_scroll_gesture_zone, resolve_scroll_target, scroll_cursor_into_view,
};

// Re-exported so the rest of the editor keeps addressing these as
// `document_view::*`, whichever submodule they now live in.
pub(crate) use changes::flush_document_changes;
pub(crate) use layout::{
    GlyphMetrics, GridExtent, GridStrip, VLineKind, ViewCache, VisualLine,
    compute_grid_display_extent, glyph_metrics,
};
#[cfg(test)]
pub(crate) use layout::{gutter_line_number, inline_panel_reserved_width};
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

/// Where a followed link led. The two cases differ in who can carry the jump
/// out: a target in this same document is the editor's own business, anything
/// else needs the host, which is the only thing that knows the other files.
pub enum NavTarget {
    /// The target is in this document and the editor has already moved the
    /// caret to `line`.
    Local { line: usize },
    /// The target is not in this document; only the host can find and open it.
    CrossFile(GotoGlyph),
}

/// One Ctrl/Cmd+click on a link, reported so the host can both carry out the
/// cross-file case and record the jump in its go-back/go-forward history.
///
/// `from` is the position of the *link* — not of the caret, which a Ctrl+click
/// deliberately leaves alone. Going back returns there rather than to wherever
/// the caret happened to sit, which is what makes "back" land on the reference
/// the user followed.
pub struct NavRequest {
    pub from: Caret,
    pub target: NavTarget,
}

pub struct DocumentViewResult {
    pub nav: Option<NavRequest>,
    pub rename: Option<RenameAction>,
}

/// Everything an editor reads but never owns: the resolved font data, the
/// name tables and the view parameters the host decides. It is borrowed and
/// `Copy`, so any number of editors can render against one environment in the
/// same frame.
#[derive(Clone, Copy)]
pub struct EditorEnv<'a> {
    pub named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    pub name_parts: &'a NamePartsMap,
    pub alt_index: &'a crate::editor::ref_composite::AlternativesIndex,
    pub color_aliases: &'a ColorAliasMap,
    /// `font-meta`, for the baseline and the height of the metric box.
    pub meta: crate::resolve::FontMeta,
    /// Whether the metric-box overlay is switched on (View menu).
    pub show_metrics: bool,
    /// Generation of the derived data above; bumping it invalidates the
    /// editor's per-frame view cache.
    pub derived_gen: u64,
    /// Generation of the built font, which the view cache also keys on.
    pub font_gen: u64,
    pub zoom_level: u32,
    pub font_id: &'a egui::FontId,
}

/// One editor instance, as a widget.
///
/// The three `&mut` borrows are what an editor owns — the document, the line
/// buffer it edits and its own [`EditorState`] — and everything else is
/// shared through [`EditorEnv`]. Nothing else is instance state: the ids the
/// editor uses inside `egui` are all salted with `state.id()`, so building a
/// second `DocumentEditor` over a second document and state is all it takes
/// to have two live editors in one frame.
pub struct DocumentEditor<'a> {
    doc: &'a mut Document,
    lines: &'a mut Vec<DocLine>,
    state: &'a mut EditorState,
    env: EditorEnv<'a>,
}

impl<'a> DocumentEditor<'a> {
    pub fn new(
        doc: &'a mut Document,
        lines: &'a mut Vec<DocLine>,
        state: &'a mut EditorState,
        env: EditorEnv<'a>,
    ) -> Self {
        Self { doc, lines, state, env }
    }

    /// Renders this editor into `ui` and reports the actions only the host can
    /// carry out (following a link into another file, applying a rename).
    pub fn show(self, ui: &mut egui::Ui) -> DocumentViewResult {
        let Self { doc, lines, state, env } = self;
        // Salt every *auto-generated* id inside — the canvas widget, the
        // scroll area, each interaction rect — with this editor's namespace.
        // The explicitly-named ids (areas, panels, temp slots) carry the same
        // salt via `Slot`; between the two, no id an editor creates depends on
        // being the only editor in the context.
        let salt = state.id().egui_id();
        ui.push_id(salt, |ui| show_document(ui, doc, lines, state, env))
            .inner
    }
}

/// The cached-or-rebuilt derived view (composites, visual lines, source
/// offsets) for the current document revision and view parameters.
fn resolve_view(
    ctx: &egui::Context,
    doc: &Document,
    lines: &[DocLine],
    state: &mut EditorState,
    env: EditorEnv<'_>,
    cache_key: ViewCacheKey,
    editing_item_idx: Option<usize>,
    pal: &Palette,
    wrap_width: Option<f32>,
) -> std::sync::Arc<ViewData> {
    let EditorEnv {
        named_glyphs,
        name_parts,
        alt_index,
        color_aliases,
        meta,
        show_metrics,
        zoom_level,
        font_id,
        ..
    } = env;
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
        meta,
        show_metrics,
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

/// The editor's frame loop. Reached through [`DocumentEditor::show`], which
/// is also what establishes the instance's id namespace.
fn show_document(
    ui: &mut egui::Ui,
    doc: &mut Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    env: EditorEnv<'_>,
) -> DocumentViewResult {
    let EditorEnv {
        named_glyphs,
        name_parts,
        derived_gen,
        font_gen,
        zoom_level,
        font_id,
        ..
    } = env;
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
        show_metrics: env.show_metrics,
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
        env,
        cache_key,
        editing_item_idx,
        &pal,
        wrap_width,
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

    // Alt + wheel steps the number at the caret. Resolved here, before the
    // scroll area sees the wheel — a gesture that lands on a number takes the
    // delta with it — but written back below with the frame's other edits, so
    // the view being painted still matches `lines`.
    let number_bump = detect_number_bump(ui, lines, state, ui.max_rect());
    swallow_wheel_delta(ui, state, number_bump.is_some());

    let scroll_y_id = state.key(Slot::ScrollY);
    let viewport_h_id = state.key(Slot::ViewportH);
    let prev_scroll_y = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(scroll_y_id))
        .unwrap_or(0.0);
    let prev_viewport_h = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(viewport_h_id))
        .unwrap_or(200.0);

    apply_scroll_physics(ui, zoom_level, state.key(Slot::ScrollAccel));

    let mut minimap_scroll_target: Option<f32> = None;
    egui::SidePanel::right(state.key(Slot::MinimapPanel))
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

    lock_scroll_gesture_zone(ui, state);

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
            env,
            vlines,
            composites,
            source_offsets,
            &pal,
            row_height,
            grid_cell,
            gutter_width,
            total_height,
            viewport_h,
            cursor_color,
            inline_panel_edit_idx,
            &mut needs_rederive,
        );
    });

    if total_height > 0.0 {
        state.saved_scroll_frac = (scroll_output.state.offset.y + viewport_h / 2.0) / total_height;
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(scroll_y_id, scroll_output.state.offset.y);
        d.insert_temp(viewport_h_id, viewport_h);
    });

    let prev_cursor = state.cursor;
    handle_document_keys(
        ui, doc, lines, state, named_glyphs, name_parts, prev_cursor, &mut needs_rederive,
    );

    if let Some(bump) = number_bump {
        needs_rederive |= apply_number_bump(lines, state, bump);
    }

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

    DocumentViewResult { nav: state.pending_nav.take(), rename: rename_result }
}
