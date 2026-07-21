use std::collections::HashMap;

use crate::document::{DocLine, Document, DocumentItem, GlyphPoint, NamePartsMap, PixelGrid};
use crate::document_io::{self, tokenize_with_spans};
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

const COARSE_SCROLL_COOLDOWN: f64 = 0.05;
pub(crate) const UNFILLED_OPACITY: f32 = 0.35;

pub(crate) fn debounced_scroll_step(ctx: &egui::Context) -> Option<i32> {
    let now = ctx.input(|i| i.time);
    ctx.input(|i| i.pointer.hover_pos())?;

    let mut direction: Option<i32> = None;
    ctx.input(|i| {
        for event in &i.events {
            if let egui::Event::MouseWheel { delta, .. } = event {
                if delta.y > 0.0 {
                    direction = Some(-1);
                } else if delta.y < 0.0 {
                    direction = Some(1);
                }
            }
        }
    });

    let dir = direction?;

    let id = egui::Id::new("coarse_scroll_debounce");
    let (last_time, last_dir): (f64, i32) = ctx.data(|d| d.get_temp(id).unwrap_or((0.0, 0)));

    if dir == last_dir && now - last_time < COARSE_SCROLL_COOLDOWN {
        return None;
    }

    ctx.data_mut(|d| d.insert_temp(id, (now, dir)));
    Some(dir)
}

pub(crate) const GRID_CELL: f32 = 14.0;
pub(crate) const LEFT_PAD: f32 = 4.0;
pub(crate) const PREVIEW_SCALE: f32 = 2.0;
const MINIMAP_WIDTH: f32 = 64.0;
const INLINE_PANEL_GAP: f32 = 12.0;
pub(crate) const INLINE_PALETTE_CELL: f32 = 16.0;

const SCROLL_BASE_MULTIPLIER: f32 = 2.5;
const SCROLL_ACCEL_START: u32 = 3;
const SCROLL_ACCEL_STEP: f32 = 0.8;
const SCROLL_ACCEL_MAX: f32 = 5.0;
const SCROLL_RAPID_THRESHOLD: f64 = 0.12;
const SCROLL_ACCEL_RESET: f64 = 0.20;
const SCROLL_GESTURE_GRACE: f64 = 0.50;

use super::colors::Palette;

#[derive(Clone, Copy)]
pub(crate) struct GridExtent {
    pub(crate) top: i16,
    pub(crate) left: i16,
    pub(crate) bottom: i16,
    pub(crate) right: i16,
}

impl GridExtent {
    pub(crate) fn own_area(width: u16, height: u16) -> Self {
        Self {
            top: 0,
            left: 0,
            bottom: height as i16,
            right: width as i16,
        }
    }

    pub(crate) fn display_width(&self, grid_cell: f32) -> f32 {
        (self.right - self.left) as f32 * grid_cell
    }
}

pub(crate) fn compute_grid_display_extent(
    pixels: Option<&PixelGrid>,
    composite: Option<&GlyphComposite>,
    points: &[GlyphPoint],
) -> (u16, u16, GridExtent) {
    let (own_w, own_h, mut extent) = if let Some(grid) = pixels {
        let own_w = grid.width;
        let own_h = grid.height;
        if let Some(comp) = composite {
            let extent = GridExtent {
                top: (-comp.own_offset_row).min(0),
                left: (-comp.own_offset_col).min(0),
                bottom: (comp.height as i16 - comp.own_offset_row).max(own_h as i16),
                right: (comp.width as i16 - comp.own_offset_col).max(own_w as i16),
            };
            (own_w, own_h, extent)
        } else {
            (own_w, own_h, GridExtent::own_area(own_w, own_h))
        }
    } else if let Some(comp) = composite {
        let own_w = (comp.width as i16 - comp.own_offset_col) as u16;
        let own_h = (comp.height as i16 - comp.own_offset_row) as u16;
        let extent = GridExtent {
            top: (-comp.own_offset_row).min(0),
            left: (-comp.own_offset_col).min(0),
            bottom: own_h as i16,
            right: own_w as i16,
        };
        (own_w, own_h, extent)
    } else {
        (
            0,
            0,
            GridExtent {
                top: 0,
                left: 0,
                bottom: 0,
                right: 0,
            },
        )
    };

    for pt in points {
        extent.top = extent.top.min(pt.row);
        extent.left = extent.left.min(pt.col);
        extent.bottom = extent.bottom.max(pt.row_end + 1);
        extent.right = extent.right.max(pt.col_end + 1);
    }

    (own_w, own_h, extent)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub(crate) struct VisualLine {
    pub(crate) doc_line: usize,
    pub(crate) kind: VLineKind,
    pub(crate) color: egui::Color32,
    pub(crate) error_spans: Vec<(usize, usize, String)>,
    pub(crate) col_offset: usize,
}

#[derive(Clone)]
pub(crate) enum VLineKind {
    Text(String),
    GridRow {
        item_idx: usize,
        row: i16,
        own_width: u16,
        own_height: u16,
        grid_doc_line: usize,
        extent: GridExtent,
    },
}

/// Everything derivable from the document/line buffers that `show_document`
/// needs each frame. Rebuilding it is O(document); the cache below keeps the
/// last result so idle frames (no edits, no layout change) skip the rebuild.
pub(crate) struct ViewData {
    pub(crate) composites: HashMap<usize, GlyphComposite>,
    pub(crate) vlines: Vec<VisualLine>,
    pub(crate) source_offsets: Vec<usize>,
}

/// Inputs `ViewData` was computed from. `edit_gen` stands in for the document
/// contents, so anything that mutates `lines` without an immediate rederive
/// must drop the cache instead (see the `needs_rederive` handling below).
#[derive(PartialEq)]
struct ViewCacheKey {
    edit_gen: u64,
    derived_gen: u64,
    font_gen: u64,
    zoom_level: u32,
    editing_item_idx: Option<usize>,
    wrap_width_bits: Option<u32>,
    font_id: egui::FontId,
    dark_mode: bool,
    ppp_bits: u32,
}

pub(crate) struct ViewCache {
    key: ViewCacheKey,
    data: std::sync::Arc<ViewData>,
}

#[cfg(test)]
impl ViewCache {
    pub(crate) fn data_ptr(&self) -> *const ViewData {
        std::sync::Arc::as_ptr(&self.data)
    }
}

impl VisualLine {
    pub(crate) fn height(&self, row_h: f32, grid_cell: f32) -> f32 {
        match &self.kind {
            VLineKind::Text(_) => row_h,
            VLineKind::GridRow { .. } => grid_cell,
        }
    }

    fn kind_row(&self) -> Option<i16> {
        match &self.kind {
            VLineKind::GridRow { row, .. } => Some(*row),
            _ => None,
        }
    }
}

/// Vertical offset (in pixels) of the first visual line belonging to
/// `target_doc_line`, i.e. the sum of heights of all visual lines before it.
fn doc_line_to_y(
    vlines: &[VisualLine],
    row_height: f32,
    grid_cell: f32,
    target_doc_line: usize,
) -> f32 {
    let mut y = 0.0f32;
    for vl in vlines {
        if vl.doc_line >= target_doc_line {
            break;
        }
        y += vl.height(row_height, grid_cell);
    }
    y
}

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

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
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
    // An external mutation of `lines` (menu action, rename, …) queues a sync
    // request; the cached view predates that mutation, so rebuild.
    let cache_valid = !state.document_sync_requested
        && state
            .view_cache
            .as_ref()
            .is_some_and(|c| c.key == cache_key);
    let view = if cache_valid {
        std::sync::Arc::clone(&state.view_cache.as_ref().unwrap().data)
    } else {
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
            &pal,
            wrap_width,
            ui.ctx(),
            &font_id,
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
    };
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

    // Track whether this scroll gesture started on an interceptor area
    // (grid, subglyph preview, shape palette). Once a gesture begins, lock
    // in the starting zone so that scrolling the document doesn't
    // accidentally switch to palette selection when the grid passes under
    // the cursor.
    let scroll_on_interceptor = {
        let currently_on = ui.ctx().data(|d| {
            d.get_temp::<bool>(egui::Id::new("subglyph_preview_hover"))
                .unwrap_or(false)
                || d.get_temp::<bool>(egui::Id::new("shape_palette_hover"))
                    .unwrap_or(false)
        }) || state.grid_hover;

        let has_wheel = ui.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::MouseWheel { .. }))
        });

        let gesture_id = egui::Id::new("scroll_gesture_zone");
        let now = ui.input(|i| i.time);

        if has_wheel {
            let prev: Option<(f64, bool)> = ui.ctx().data(|d| d.get_temp(gesture_id));
            let on = match prev {
                Some((t, was_on)) if now - t < SCROLL_GESTURE_GRACE => was_on,
                _ => currently_on,
            };
            ui.ctx()
                .data_mut(|d| d.insert_temp(gesture_id, (now, on)));
            on
        } else {
            ui.ctx()
                .data(|d| d.get_temp::<(f64, bool)>(gesture_id))
                .is_some_and(|(_, on)| on)
                && ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.01)
        }
    };
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("scroll_on_interceptor"),
            scroll_on_interceptor,
        );
    });
    if scroll_on_interceptor {
        ui.ctx()
            .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }

    let viewport_h = ui.available_height();
    let mut scroll_area_builder = egui::ScrollArea::vertical().auto_shrink([false, false]);

    // Clear page-scroll sticky state when cursor moved by non-page means.
    {
        let id = egui::Id::new("page_last_cursor");
        let prev: Option<(usize, usize)> =
            ui.ctx().data(|d| d.get_temp(id));
        if prev.is_some_and(|(l, c)| l != state.cursor.line || c != state.cursor.col) {
            ui.ctx().data_mut(|d| {
                d.remove::<usize>(egui::Id::new("page_sticky_vi"));
                d.remove::<(usize, usize)>(id);
            });
        }
    }

    // PageUp/PageDown: scroll so last/first fully visible line becomes
    // first/last; move the caret by the same number of visual lines.
    // A "sticky vline index" survives across presses so that landing
    // inside a multi-row grid doesn't snap to the grid top and drift.
    {
        let page_id = egui::Id::new("page_scroll_request");
        if let Some((dir, shift)) = ui.ctx().data(|d| d.get_temp::<(i32, bool)>(page_id)) {
            ui.ctx().data_mut(|d| d.remove::<(i32, bool)>(page_id));
            if shift {
                if state.selection_anchor.is_none() {
                    state.selection_anchor = Some(state.cursor);
                }
            } else {
                state.selection_anchor = None;
            }

            let eps = 0.5;

            let mut vline_y: Vec<f32> = Vec::with_capacity(vlines.len());
            let mut y_acc = 0.0f32;
            for vl in vlines {
                vline_y.push(y_acc);
                y_acc += vl.height(row_height, grid_cell);
            }

            let mut first_vis: Option<usize> = None;
            let mut last_vis: Option<usize> = None;
            for i in 0..vlines.len() {
                let top = vline_y[i];
                let h = vlines[i].height(row_height, grid_cell);
                if top >= prev_scroll_y - eps
                    && top + h <= prev_scroll_y + prev_viewport_h + eps
                {
                    if first_vis.is_none() {
                        first_vis = Some(i);
                    }
                    last_vis = Some(i);
                }
            }

            let sticky_vi_id = egui::Id::new("page_sticky_vi");
            let cursor_vi = ui
                .ctx()
                .data(|d| d.get_temp::<usize>(sticky_vi_id))
                .map(|vi| vi.min(vlines.len().saturating_sub(1)))
                .unwrap_or_else(|| {
                    let mut result = 0usize;
                    let mut in_target = false;
                    for (i, vl) in vlines.iter().enumerate() {
                        if vl.doc_line != state.cursor.line {
                            if in_target {
                                break;
                            }
                            continue;
                        }
                        in_target = true;
                        result = i;
                        match &vl.kind {
                            VLineKind::Text(text) => {
                                let seg_len = text.chars().count();
                                if state.cursor.col >= vl.col_offset
                                    && state.cursor.col <= vl.col_offset + seg_len
                                {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    result
                });

            if let (Some(fv), Some(lv)) = (first_vis, last_vis) {
                let page_vlines = lv - fv;
                if page_vlines > 0 {
                    let (new_scroll, new_cvi) = if dir > 0 {
                        (vline_y[lv], (cursor_vi + page_vlines).min(vlines.len() - 1))
                    } else {
                        let fh = vlines[fv].height(row_height, grid_cell);
                        let ns = (vline_y[fv] + fh - prev_viewport_h).max(0.0);
                        (ns, cursor_vi.saturating_sub(page_vlines))
                    };

                    let new_line = vlines[new_cvi].doc_line;
                    state.cursor = Caret::new(
                        new_line,
                        state.cursor.col.min(caret::line_char_len(lines, new_line)),
                    );

                    ui.ctx().data_mut(|d| {
                        d.insert_temp(sticky_vi_id, new_cvi);
                        d.insert_temp(
                            egui::Id::new("page_last_cursor"),
                            (state.cursor.line, state.cursor.col),
                        );
                        d.insert_temp(egui::Id::new("goto_scroll_target"), new_scroll);
                    });
                }
            }
        }
    }

    // When zoom level changes, adjust scroll so the content under the mouse
    // pointer stays at the same screen position.
    let zoom_scroll: Option<f32> = {
        let old_zoom: Option<u32> = state.take_zoom_change();
        if let Some(old_z) = old_zoom {
            let scale = zoom_level as f32 / old_z as f32;
            let pointer_y = ui.ctx().input(|i| i.pointer.hover_pos().map(|p| p.y));
            if let Some(py) = pointer_y {
                let viewport_top = ui.max_rect().top();
                let pvo = py - viewport_top;
                let old_doc_y = prev_scroll_y + pvo;
                Some((old_doc_y * scale - pvo).max(0.0))
            } else {
                let new_caret_y =
                    doc_line_to_y(vlines, row_height, grid_cell, state.cursor.line);
                let old_caret_y = new_caret_y / scale;
                let visual_offset = (old_caret_y - prev_scroll_y).max(0.0);
                Some((new_caret_y - visual_offset).max(0.0))
            }
        } else {
            None
        }
    };

    let goto_scroll_id = egui::Id::new("goto_scroll_target");
    let goto_scroll: Option<f32> = ui.ctx().data(|d| d.get_temp::<f32>(goto_scroll_id));
    if goto_scroll.is_some() {
        ui.ctx().data_mut(|d| d.remove::<f32>(goto_scroll_id));
    }
    let scroll_to_cursor = state.take_scroll_to_cursor();
    let cursor_scroll = if scroll_to_cursor {
        let target_y = doc_line_to_y(vlines, row_height, grid_cell, state.cursor.line);
        Some((target_y - viewport_h / 3.0).max(0.0))
    } else {
        None
    };
    let saved_scroll_y = (state.saved_scroll_frac * total_height - viewport_h / 2.0).max(0.0);
    let restore_scroll = if minimap_scroll_target.is_none()
        && goto_scroll.is_none()
        && cursor_scroll.is_none()
        && zoom_scroll.is_none()
        && (saved_scroll_y - prev_scroll_y).abs() > 1.0
    {
        Some(saved_scroll_y)
    } else {
        None
    };
    if let Some(target) = minimap_scroll_target.or(goto_scroll).or(cursor_scroll).or(zoom_scroll).or(restore_scroll) {
        scroll_area_builder = scroll_area_builder.vertical_scroll_offset(target);
    }

    let scroll_output = scroll_area_builder.show(ui, |ui| {
        let avail_w = ui.available_width();
        let desired = egui::vec2(avail_w, total_height.max(row_height));
        let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());

        let wid = response.id;
        if response.clicked() || response.drag_started() {
            ui.memory_mut(|m| m.request_focus(wid));
        }
        let has_focus = ui.memory(|m| m.has_focus(wid));
        state.active = has_focus;

        needs_rederive |= pixel_selection::reconcile(doc, lines, state);

        if has_focus {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(wid, egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                    tab: state.autocomplete.is_some(),
                });
            });
            ui.output_mut(|o| o.mutable_text_under_cursor = true);
        }

        let painter = ui.painter_at(rect);
        let gutter_x = rect.min.x;
        let origin = egui::pos2(rect.min.x + gutter_width, rect.min.y);
        let sel = state.selection_range();

        // Publish the frame's layout for the in-crate GUI test harness.
        #[cfg(test)]
        crate::editor::harness::capture_snapshot(
            ui.ctx(),
            vlines,
            lines,
            source_offsets,
            origin,
            row_height,
            grid_cell,
            wid,
        );

        let is_double = response.double_clicked();
        let is_triple = response.triple_clicked();
        let click_pos = if response.clicked() || response.drag_started() || is_double || is_triple {
            response.interact_pointer_pos()
        } else if response.dragged() && matches!(state.mode, EditMode::Normal) {
            // Only track drag position for text selection in Normal mode.
            // In edit modes (GlyphEdit/PixelSelect/LayerMove), drag tracking
            // is handled directly via pointer.hover_pos()/delta() by each
            // mode's handler; letting click_pos resolve here would hit a text
            // line and force a mode change back to Normal mid-drag.
            ui.input(|i| i.pointer.hover_pos())
        } else {
            None
        };

        let mut click_result: Option<ClickTarget> = None;
        let mut cursor_screen: Option<egui::Pos2> = None;
        let mut error_tooltip: Option<(egui::Pos2, String)> = None;
        let mut goto_glyph_name: Option<String> = None;
        let mut goto_glyph_kind: Option<LinkTargetKind> = None;
        let mut inline_panel_origin: Option<(f32, f32, f32)> = None; // (x, y, grid_display_width)
        let mut edit_grid_rect: Option<egui::Rect> = None;

        let cmd_held = ui.input(|i| i.modifiers.command);
        let hover_pos = ui.input(|i| i.pointer.hover_pos());

        let clip = ui.clip_rect();
        let vis_top = clip.min.y - origin.y;
        let vis_bottom = clip.max.y - origin.y;

        let mut y = 0.0f32;
        for vl in vlines {
            let h = vl.height(row_height, grid_cell);

            if y + h < vis_top || y > vis_bottom {
                if matches!(state.mode, EditMode::Normal)
                    && state.cursor.line == vl.doc_line
                {
                    cursor_screen = Some(egui::pos2(origin.x + LEFT_PAD, origin.y + y));
                }
                if inline_panel_origin.is_none()
                    && let VLineKind::GridRow { item_idx, extent, .. } = &vl.kind
                        && inline_panel_edit_idx == Some(*item_idx)
                            && vl.kind_row() == Some(extent.top)
                        {
                            let gx = origin.x + LEFT_PAD;
                            let gy = origin.y + y;
                            inline_panel_origin = Some((
                                gx + extent.display_width(grid_cell)
                                    + INLINE_PANEL_GAP * zoom_level as f32,
                                gy,
                                extent.display_width(grid_cell),
                            ));
                            edit_grid_rect = Some(egui::Rect::from_min_size(
                                egui::pos2(gx, gy),
                                egui::vec2(
                                    extent.display_width(grid_cell),
                                    (extent.bottom - extent.top) as f32 * grid_cell,
                                ),
                            ));
                        }
                y += h;
                continue;
            }

            let src_line = gutter_line_number(vl, lines, source_offsets);
            if let Some(num) = src_line {
                let num_text = format!("{num:>5} ");
                painter.text(
                    egui::pos2(gutter_x, origin.y + y),
                    egui::Align2::LEFT_TOP,
                    &num_text,
                    font_id.clone(),
                    pal.line_num,
                );
            }

            if let Some((sel_lo, sel_hi)) = sel {
                draw_selection(
                    &painter, ui, &font_id, vl, origin, y, h, sel_lo, sel_hi, grid_cell,
                );
            }

            match &vl.kind {
                VLineKind::Text(text) => {
                    painter.text(
                        egui::pos2(origin.x + LEFT_PAD, origin.y + y),
                        egui::Align2::LEFT_TOP,
                        text,
                        font_id.clone(),
                        vl.color,
                    );

                    // Color background for color tokens in color/ref-fill lines
                    paint_color_backgrounds(
                        &painter, ui, &font_id, text, vl.col_offset,
                        origin.x + LEFT_PAD, origin.y + y, h, color_aliases,
                    );

                    if !vl.error_spans.is_empty() {
                        let error_color = pal.error;
                        for (col_start, col_end, _msg) in &vl.error_spans {
                            let col_start = *col_start;
                            let col_end = *col_end;
                            let x0 = grid_render::char_x_pos(ui, &font_id, text, col_start);
                            let x1 = grid_render::char_x_pos(ui, &font_id, text, col_end);
                            let name_text: String = text
                                .chars()
                                .skip(col_start)
                                .take(col_end - col_start)
                                .collect();
                            let name_x0 = origin.x + LEFT_PAD + x0;
                            let name_x1 = origin.x + LEFT_PAD + x1;
                            let name_y0 = origin.y + y;
                            let name_y1 = name_y0 + h;
                            painter.text(
                                egui::pos2(name_x0, name_y0),
                                egui::Align2::LEFT_TOP,
                                &name_text,
                                font_id.clone(),
                                error_color,
                            );
                            painter.line_segment(
                                [
                                    egui::pos2(name_x0, name_y1 - 1.0),
                                    egui::pos2(name_x1, name_y1 - 1.0),
                                ],
                                egui::Stroke::new(1.0, error_color),
                            );
                        }
                    }

                    if cmd_held {
                        let links = doc_links::extract_line_links(text);
                        if !links.is_empty() {
                            let link_color = pal.link;
                            let name_y0 = origin.y + y;
                            let name_y1 = name_y0 + h;

                            // Find the hovered link (prefer shortest span on overlap)
                            let hovered_link = hover_pos.and_then(|hp| {
                                if hp.y < name_y0 || hp.y >= name_y1 {
                                    return None;
                                }
                                let mut best: Option<&LinkSpan> = None;
                                for link in &links {
                                    let adj_start =
                                        link.col_start.saturating_sub(vl.col_offset);
                                    let adj_end =
                                        link.col_end.saturating_sub(vl.col_offset);
                                    let lx0 =
                                        origin.x + LEFT_PAD + grid_render::char_x_pos(ui, &font_id, text, adj_start);
                                    let lx1 =
                                        origin.x + LEFT_PAD + grid_render::char_x_pos(ui, &font_id, text, adj_end);
                                    if hp.x >= lx0 && hp.x < lx1 {
                                        let span_len = link.col_end - link.col_start;
                                        if best.is_none_or(|b| {
                                            span_len < b.col_end - b.col_start
                                        }) {
                                            best = Some(link);
                                        }
                                    }
                                }
                                best
                            });

                            if let Some(link) = hovered_link {
                                let adj_start =
                                    link.col_start.saturating_sub(vl.col_offset);
                                let adj_end =
                                    link.col_end.saturating_sub(vl.col_offset);
                                let lx0 = origin.x
                                    + LEFT_PAD
                                    + grid_render::char_x_pos(ui, &font_id, text, adj_start);
                                let lx1 = origin.x
                                    + LEFT_PAD
                                    + grid_render::char_x_pos(ui, &font_id, text, adj_end);
                                let link_text: String = text
                                    .chars()
                                    .skip(adj_start)
                                    .take(adj_end - adj_start)
                                    .collect();
                                painter.text(
                                    egui::pos2(lx0, name_y0),
                                    egui::Align2::LEFT_TOP,
                                    &link_text,
                                    font_id.clone(),
                                    link_color,
                                );
                                painter.line_segment(
                                    [
                                        egui::pos2(lx0, name_y1 - 1.0),
                                        egui::pos2(lx1, name_y1 - 1.0),
                                    ],
                                    egui::Stroke::new(1.0, link_color),
                                );
                                ui.ctx()
                                    .set_cursor_icon(egui::CursorIcon::PointingHand);
                                ui.ctx().request_repaint();

                                if response.clicked() {
                                    goto_glyph_name =
                                        Some(link.target.clone());
                                    goto_glyph_kind =
                                        Some(link.kind.clone());
                                }
                            }
                        }
                    }

                    if let Some(cp) = click_pos
                        && cp.y >= origin.y + y && cp.y < origin.y + y + h {
                            let rel_x = (cp.x - origin.x - LEFT_PAD).max(0.0);
                            let col = vl.col_offset + grid_render::x_to_char_col(ui, &font_id, text, rel_x);
                            click_result = Some(ClickTarget::Text(Caret::new(vl.doc_line, col)));
                        }

                    // Cursor drawing for text lines
                    let text_char_count = text.chars().count();
                    if matches!(state.mode, EditMode::Normal)
                        && state.cursor.line == vl.doc_line
                        && state.cursor.col >= vl.col_offset
                        && state.cursor.col <= vl.col_offset + text_char_count
                    {
                        let local_col = state.cursor.col - vl.col_offset;
                        let cx =
                            origin.x + LEFT_PAD + grid_render::char_x_pos(ui, &font_id, text, local_col);
                        let cy = origin.y + y;

                        if has_focus {
                            if !state.preedit.is_empty() {
                                let preedit_w = ui.fonts(|f| {
                                    f.layout_no_wrap(
                                        state.preedit.clone(),
                                        font_id.clone(),
                                        egui::Color32::WHITE,
                                    )
                                    .rect
                                    .width()
                                });
                                let bg_color = cursor_color;
                                let fg_color = ui.visuals().panel_fill;
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(cx, cy),
                                        egui::vec2(preedit_w, h),
                                    ),
                                    0.0,
                                    bg_color,
                                );
                                painter.text(
                                    egui::pos2(cx, cy),
                                    egui::Align2::LEFT_TOP,
                                    &state.preedit,
                                    font_id.clone(),
                                    fg_color,
                                );
                                cursor_screen = Some(egui::pos2(cx + preedit_w, cy));
                            } else {
                                painter.line_segment(
                                    [egui::pos2(cx, cy), egui::pos2(cx, cy + h)],
                                    egui::Stroke::new(2.0, cursor_color),
                                );
                                cursor_screen = Some(egui::pos2(cx, cy));
                            }
                        } else {
                            cursor_screen = Some(egui::pos2(origin.x + LEFT_PAD, origin.y + y));
                        }

                        // Check if caret is inside an error span
                        for (s, e, msg) in &vl.error_spans {
                            if local_col >= *s && local_col < *e {
                                let span_x = origin.x + LEFT_PAD
                                    + grid_render::char_x_pos(ui, &font_id, text, *s);
                                error_tooltip = Some((
                                    egui::pos2(span_x, cy + h + 2.0),
                                    msg.clone(),
                                ));
                                break;
                            }
                        }
                    }
                }
                VLineKind::GridRow {
                    item_idx,
                    row,
                    own_width,
                    own_height,
                    grid_doc_line,
                    extent,
                } => {
                    let grid_x = origin.x + LEFT_PAD;
                    let grid_y = origin.y + y;

                    grid_render::render_grid_row(
                        &painter,
                        grid_x,
                        grid_y,
                        doc,
                        *item_idx,
                        *row,
                        *own_width,
                        *own_height,
                        *extent,
                        composites.get(item_idx),
                        &state.mode,
                        grid_cell,
                        &pal,
                    );

                    grid_render::handle_grid_hover_preview(
                        ui,
                        &painter,
                        &state.mode,
                        *item_idx,
                        *own_width,
                        *own_height,
                        *row,
                        *extent,
                        grid_x,
                        grid_y,
                        grid_cell,
                    );

                    if inline_panel_origin.is_none()
                        && inline_panel_edit_idx == Some(*item_idx)
                        && *row == extent.top
                    {
                        inline_panel_origin = Some((
                            grid_x
                                + extent.display_width(grid_cell)
                                + INLINE_PANEL_GAP * zoom_level as f32,
                            grid_y,
                            extent.display_width(grid_cell),
                        ));
                        edit_grid_rect = Some(egui::Rect::from_min_size(
                            egui::pos2(grid_x, grid_y),
                            egui::vec2(
                                extent.display_width(grid_cell),
                                (extent.bottom - extent.top) as f32 * grid_cell,
                            ),
                        ));
                    }

                    if let Some(cp) = click_pos
                        && cp.y >= grid_y && cp.y < grid_y + grid_cell {
                            let rel_x = cp.x - grid_x;
                            let gc = (rel_x / grid_cell) as i32 + extent.left as i32;
                            if gc >= extent.left as i32 && gc < extent.right as i32 {
                                click_result = Some(ClickTarget::Grid {
                                    item_idx: *item_idx,
                                });
                            } else {
                                click_result = Some(ClickTarget::Text(Caret::new(vl.doc_line, 0)));
                            }
                        }

                    if !matches!(state.mode, EditMode::PixelSelect { item_idx: eidx } if eidx == *item_idx)
                    {
                        pixel_interaction::handle_pixel_painting(
                            ui,
                            lines,
                            state,
                            &mut needs_rederive,
                            *grid_doc_line,
                            *item_idx,
                            *row,
                            *own_width,
                            *own_height,
                            *extent,
                            grid_x,
                            grid_y,
                            grid_cell,
                        );
                    }

                    // Pixel selection overlay + interaction
                    if let Some(sel) = state.pixel_selection.as_ref()
                        .filter(|s| s.item_idx == *item_idx)
                    {
                        grid_render::render_pixel_selection_overlay(
                            &painter, grid_x, grid_y, *row, *extent, grid_cell, sel, &pal,
                        );
                    }
                    pixel_selection::handle_pixel_select_interaction(
                        ui, doc, lines, state, &mut needs_rederive,
                        *grid_doc_line, *item_idx, *row, *own_width, *own_height,
                        *extent, grid_x, grid_y, grid_cell,
                    );

                    // Grid caret
                    if matches!(state.mode, EditMode::Normal)
                        && state.cursor.line == *grid_doc_line
                        && *row == extent.top
                        && has_focus
                    {
                        let own_x = grid_x + (-extent.left) as f32 * grid_cell;
                        let border_rect = egui::Rect::from_min_size(
                            egui::pos2(own_x, grid_y + (-extent.top) as f32 * grid_cell),
                            egui::vec2(
                                *own_width as f32 * grid_cell,
                                *own_height as f32 * grid_cell,
                            ),
                        );
                        painter.rect_stroke(
                            border_rect,
                            0.0,
                            egui::Stroke::new(2.0, pal.grid_border),
                            egui::epaint::StrokeKind::Outside,
                        );
                        cursor_screen =
                            Some(egui::pos2(own_x, grid_y + (-extent.top) as f32 * grid_cell));
                    }
                }
            }

            draw_edit_border(
                &painter,
                &state.mode,
                vl,
                doc,
                origin,
                y,
                composites,
                grid_cell,
                &pal,
            );

            y += h;
        }

        // Inline tools panel (preview + palette) to the right of the grid
        if let (Some(edit_idx), Some((panel_x, panel_y, _grid_w))) =
            (inline_panel_edit_idx, inline_panel_origin)
        {
            let panel_result = inline_tools::draw_inline_tools_panel(
                ui,
                &painter,
                panel_x,
                panel_y,
                doc,
                state,
                edit_idx,
                composites,
                named_glyphs,
                name_parts,
                click_pos,
                zoom_level,
            );
            if panel_result.click_consumed {
                click_result = None;
            }
            if let Some(ref_idx) = panel_result.inline_ref
                && inline_ref_to_pixels(
                    lines,
                    doc,
                    state,
                    edit_idx,
                    ref_idx,
                    named_glyphs,
                    name_parts,
                ) {
                    needs_rederive = true;
                }
        }

        // Layer move drag handling (refs and points)
        if let EditMode::LayerMove {
            item_idx: eidx,
            layer_idx,
        } = state.mode
        {
            pixel_interaction::handle_layer_drag(
                ui,
                lines,
                state,
                &mut needs_rederive,
                doc,
                eidx,
                layer_idx,
                &doc.item_line_starts,
                composites.get(&eidx),
                named_glyphs,
                name_parts,
                grid_cell,
            );
        }

        // Wheel scroll on grid: change subpixel shape or layer
        if inline_panel_edit_idx.is_none() {
            state.grid_hover = false;
        }
        if let (Some(edit_idx), Some(grid_rect)) =
            (inline_panel_edit_idx, edit_grid_rect)
        {
            let body = match doc.items.get(edit_idx) {
                Some(DocumentItem::Glyph { body, .. }) => Some(body),
                _ => None,
            };
            if let Some(body) = body {
                let on_grid = ui.input(|i| {
                    i.pointer.hover_pos().is_some_and(|hp| grid_rect.contains(hp))
                });
                state.grid_hover = on_grid;

                let gesture_on_interceptor = ui.ctx().data(|d| {
                    d.get_temp::<bool>(egui::Id::new("scroll_on_interceptor"))
                        .unwrap_or(false)
                });
                if gesture_on_interceptor && on_grid {
                    let ctrl_held = ui.input(|i| i.modifiers.command);
                    if let Some(step) = debounced_scroll_step(ui.ctx()) {
                        if ctrl_held {
                            // Ctrl+wheel on grid: cycle layers (same as layer palette)
                            crate::editor::inline_tools::cycle_layer_mode(
                                state, body, edit_idx, step,
                            );
                        } else if matches!(state.mode, EditMode::GlyphEdit { item_idx, .. } if item_idx == edit_idx)
                        {
                            // Wheel on grid in pixel layer: cycle subpixel shapes
                            if let EditMode::GlyphEdit { selected_shape, .. } = &mut state.mode {
                                let shapes = crate::editor::glyph_widget::all_valid_shapes();
                                if let Some(cur_idx) =
                                    shapes.iter().position(|s| s.shape_id() == selected_shape.shape_id())
                                {
                                    let next_idx = (cur_idx as i32 + step)
                                        .clamp(0, shapes.len() as i32 - 1)
                                        as usize;
                                    *selected_shape = shapes[next_idx];
                                }
                            }
                        }
                    }
                }
            }
        }

        // Ctrl/Cmd+click goto
        if let Some(ref target_name) = goto_glyph_name {
            let kind = goto_glyph_kind.as_ref().unwrap_or(&LinkTargetKind::Glyph);
            if let Some(line_idx) = doc_links::find_link_target_in_doc(lines, target_name, kind) {
                state.mode = EditMode::Normal;
                state.selection_anchor = None;
                state.cursor = Caret::new(line_idx, 0);
                let target_y = doc_line_to_y(vlines, row_height, grid_cell, line_idx);
                let centered = (target_y - viewport_h / 3.0).max(0.0);
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("goto_scroll_target"), centered);
                });
            } else {
                let kind_u8: u8 = match kind {
                    LinkTargetKind::Glyph => 0,
                    LinkTargetKind::NameParts => 1,
                    LinkTargetKind::Remap => 2,
                    LinkTargetKind::Color => 3,
                };
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("goto_cross_file"), target_name.clone());
                    d.insert_temp(egui::Id::new("goto_cross_file_kind"), kind_u8);
                });
            }
        }

        // Process click (skip while rename popup is open)
        if goto_glyph_name.is_none() && !matches!(state.popup, PopupState::Rename { .. })
            && let Some(target) = click_result {
                state.autocomplete = None;
                let shift = ui.input(|i| i.modifiers.shift);
                match target {
                    ClickTarget::Text(caret_pos) => {
                        if !matches!(state.mode, EditMode::Normal) {
                            state.mode = EditMode::Normal;
                        }
                        let caret_pos = caret::clamp(lines, caret_pos);
                        if is_triple {
                            state.selection_anchor = Some(Caret::new(caret_pos.line, 0));
                            state.cursor = Caret::new(
                                caret_pos.line,
                                caret::line_char_len(lines, caret_pos.line),
                            );
                        } else if is_double {
                            let (lo, hi) = caret::word_bounds_at(lines, caret_pos);
                            state.selection_anchor = Some(lo);
                            state.cursor = hi;
                        } else if !shift && !response.dragged() {
                            state.selection_anchor = None;
                            state.cursor = caret_pos;
                        } else if !shift && response.drag_started() {
                            state.selection_anchor = Some(caret_pos);
                            state.cursor = caret_pos;
                        } else {
                            if state.selection_anchor.is_none() {
                                state.selection_anchor = Some(state.cursor);
                            }
                            state.cursor = caret_pos;
                        }
                    }
                    ClickTarget::Grid { item_idx } => {
                        state.selection_anchor = None;
                        if !matches!(
                            state.mode,
                            EditMode::GlyphEdit { item_idx: eidx, .. } if eidx == item_idx
                        ) && !matches!(
                            state.mode,
                            EditMode::PixelSelect { item_idx: eidx } if eidx == item_idx
                        ) && !matches!(
                            state.mode,
                            EditMode::LayerMove { item_idx: eidx, .. } if eidx == item_idx
                        ) {
                            let has_pixels = matches!(
                                doc.items.get(item_idx),
                                Some(DocumentItem::Glyph { body, .. }) if body.pixels.is_some()
                            );
                            if has_pixels {
                                state.mode = EditMode::GlyphEdit {
                                    item_idx,
                                    selected_shape: pixel::PixelShape::new(
                                        pixel::PX_ALMOSTFULL,
                                        true,
                                    ),
                                };
                                state.suppress_grid_click = true;
                            } else if let Some(DocumentItem::Glyph { body, .. }) =
                                doc.items.get(item_idx)
                                && !body.refs.is_empty() {
                                    state.mode = EditMode::GlyphEdit {
                                        item_idx,
                                        selected_shape: pixel::PixelShape::new(
                                            pixel::PX_ALMOSTFULL,
                                            true,
                                        ),
                                    };
                                    state.suppress_grid_click = true;
                                }
                        }
                    }
                }
            }

        // IME
        if has_focus
            && let Some(cpos) = cursor_screen {
                let ime_rect = egui::Rect::from_min_size(cpos, egui::vec2(2.0, row_height));
                ui.ctx().output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        rect: ime_rect,
                        cursor_rect: ime_rect,
                    });
                });
            }

        // Store cursor screen position for popup use
        if let Some(cpos) = cursor_screen {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("cursor_screen_pos"), cpos);
                d.insert_temp(egui::Id::new("cursor_row_height"), row_height);
            });
        }

        // Store error tooltip for display outside scroll area
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("error_tooltip_data"), error_tooltip);
        });

        // Context menu (only in Normal mode; edit modes use right-click for erasing)
        let ctx_mode_normal = matches!(state.mode, EditMode::Normal);
        if ctx_mode_normal {
        response.context_menu(|ui| {
            let caps = crate::edit_menu::EditMenuCaps {
                can_undo: state.undo.can_undo(),
                can_redo: state.undo.can_redo(),
                has_selection: state.selection_range().is_some(),
                can_edit: ctx_mode_normal,
            };
            let action = crate::edit_menu::show_edit_menu_items(ui, &caps, false);
            if apply_edit_action_to_editor(action, lines, state, ui.ctx()) {
                needs_rederive = true;
            }
        });
        }
    });

    if total_height > 0.0 {
        state.saved_scroll_frac = (scroll_output.state.offset.y + viewport_h / 2.0) / total_height;
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new("doc_scroll_y"), scroll_output.state.offset.y);
        d.insert_temp(egui::Id::new("doc_viewport_h"), viewport_h);
    });

    // Keyboard handling
    let prev_cursor = state.cursor;
    let mut rename_result: Option<RenameAction> = None;
    if state.active {
        // Autocomplete key handling takes priority
        let ac_result = crate::editor::autocomplete::handle_keys(ui, lines, state);
        if matches!(ac_result, crate::editor::autocomplete::HandleResult::TextChanged) {
            needs_rederive = true;
        }

        if matches!(ac_result, crate::editor::autocomplete::HandleResult::NotConsumed) {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                if matches!(state.popup, PopupState::Rename { .. }) {
                    state.popup = PopupState::None;
                } else if !matches!(state.mode, EditMode::Normal) {
                    state.mode = EditMode::Normal;
                }
            }

            // F2: rename symbol at caret
            if matches!(state.mode, EditMode::Normal)
                && matches!(state.popup, PopupState::None)
                && ui.input(|i| i.key_pressed(egui::Key::F2))
                && let Some(DocLine::Text(line_text)) = lines.get(state.cursor.line)
                    && let Some(target) = doc_links::find_renameable_at_caret(line_text, state.cursor.col) {
                        state.popup = PopupState::Rename {
                            original_name: target.name.clone(),
                            new_name: target.name,
                            kind: target.kind,
                            focus_set: false,
                        };
                    }

            // Undo/redo in GlyphEdit/LayerMove modes (Normal mode handles it via doc_input::handle_keys)
            if !matches!(state.mode, EditMode::Normal)
                && !matches!(state.popup, PopupState::Rename { .. })
            {
                let undo_pressed = ui.input(|i| {
                    i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z)
                });
                let redo_pressed = ui.input(|i| {
                    (i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z))
                        || (i.modifiers.command && i.key_pressed(egui::Key::Y))
                });
                if undo_pressed {
                    let sel_ctx = Some(crate::editor::undo::SelectionUndoCtx {
                        mode: &mut state.mode,
                        pixel_selection: &mut state.pixel_selection,
                    });
                    if let Some(c) = state.undo.undo_with_sel(lines, sel_ctx) {
                        state.cursor = caret::clamp(lines, c);
                        state.selection_anchor = None;
                        state.skip_reconcile = true;
                        needs_rederive = true;
                    }
                } else if redo_pressed {
                    let sel_ctx = Some(crate::editor::undo::SelectionUndoCtx {
                        mode: &mut state.mode,
                        pixel_selection: &mut state.pixel_selection,
                    });
                    if let Some(c) = state.undo.redo_with_sel(lines, sel_ctx) {
                        state.cursor = caret::clamp(lines, c);
                        state.selection_anchor = None;
                        state.skip_reconcile = true;
                        needs_rederive = true;
                    }
                }
            }

            // Backtick: GlyphEdit → PixelSelect
            if let EditMode::GlyphEdit { item_idx, .. } = &state.mode {
                if ui.input(|i| {
                    i.key_pressed(egui::Key::Backtick)
                        && !i.modifiers.command
                        && !i.modifiers.alt
                }) {
                    state.mode = EditMode::PixelSelect {
                        item_idx: *item_idx,
                    };
                }
            }

            // PixelSelect key handling
            if let EditMode::PixelSelect { item_idx } = state.mode {
                // 1: back to GlyphEdit
                if ui.input(|i| {
                    i.key_pressed(egui::Key::Num1)
                        && !i.modifiers.command
                        && !i.modifiers.alt
                }) {
                    // Reconciliation will commit any floating selection
                    state.mode = EditMode::GlyphEdit {
                        item_idx,
                        selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
                    };
                }

                // Delete/Backspace: delete selection
                if ui.input(|i| {
                    (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                        && !i.modifiers.command
                }) && state.pixel_selection.is_some()
                {
                    pixel_selection::handle_delete_selection(doc, lines, state);
                    needs_rederive = true;
                }

                // Copy/Cut/Paste via egui events
                let mut sel_clipboard_out: Option<String> = None;
                let mut sel_do_cut = false;
                let mut sel_paste_text: Option<String> = None;
                ui.input(|input| {
                    for event in &input.events {
                        match event {
                            egui::Event::Copy => {
                                if let Some(sel) = &state.pixel_selection {
                                    sel_clipboard_out =
                                        pixel_selection::copy_selection(doc, lines, sel);
                                }
                            }
                            egui::Event::Cut => {
                                if let Some(sel) = &state.pixel_selection {
                                    sel_clipboard_out =
                                        pixel_selection::copy_selection(doc, lines, sel);
                                    sel_do_cut = true;
                                }
                            }
                            egui::Event::Paste(text) if !text.is_empty() => {
                                sel_paste_text = Some(text.clone());
                            }
                            _ => {}
                        }
                    }
                });
                if let Some(text) = sel_clipboard_out {
                    ui.ctx().copy_text(text);
                }
                if sel_do_cut {
                    pixel_selection::handle_delete_selection(doc, lines, state);
                    needs_rederive = true;
                }
                if let Some(text) = sel_paste_text {
                    if pixel_selection::paste_selection(doc, lines, state, &text) {
                        needs_rederive = true;
                    }
                }
            }

            // Paste in GlyphEdit mode: check for pixel grid paste
            if matches!(state.mode, EditMode::GlyphEdit { .. }) {
                let mut paste_text: Option<String> = None;
                ui.input(|input| {
                    for event in &input.events {
                        if let egui::Event::Paste(text) = event {
                            if !text.is_empty() {
                                paste_text = Some(text.clone());
                            }
                        }
                    }
                });
                if let Some(text) = paste_text {
                    if pixel_selection::paste_selection(doc, lines, state, &text) {
                        needs_rederive = true;
                    }
                }
            }

            // Selection transforms (Ctrl+M/I/O/J/K/L) in GlyphEdit/PixelSelect
            if matches!(
                state.mode,
                EditMode::GlyphEdit { .. } | EditMode::PixelSelect { .. }
            ) {
                use pixel_selection::SelectionTransform;
                let transform = ui.input(|i| {
                    if i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift {
                        if i.key_pressed(egui::Key::M) {
                            Some(SelectionTransform::MirrorH)
                        } else if i.key_pressed(egui::Key::I) {
                            Some(SelectionTransform::FlipV)
                        } else if i.key_pressed(egui::Key::O) {
                            Some(SelectionTransform::Opposite)
                        } else if i.key_pressed(egui::Key::J) {
                            Some(SelectionTransform::RotateCCW)
                        } else if i.key_pressed(egui::Key::K) {
                            Some(SelectionTransform::Rotate180)
                        } else if i.key_pressed(egui::Key::L) {
                            Some(SelectionTransform::RotateCW)
                        } else {
                            None
                        }
                    } else if i.modifiers.command && !i.modifiers.alt && i.modifiers.shift {
                        if i.key_pressed(egui::Key::O) {
                            Some(SelectionTransform::OppositeBitmap)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                if let Some(t) = transform {
                    if pixel_selection::can_transform(doc, state, t) {
                        if pixel_selection::handle_transform_selection(
                            doc, lines, state, t,
                        ) {
                            needs_rederive = true;
                        }
                    }
                }
            }

            // Subpixel shape shortcuts in GlyphEdit mode
            if let EditMode::GlyphEdit { selected_shape, .. } = &mut state.mode {
                handle_shape_shortcuts(ui, selected_shape);
            }

            if matches!(state.mode, EditMode::Normal)
                && !matches!(state.popup, PopupState::Rename { .. })
            {
                let text_changed = doc_input::handle_keys(ui, lines, state);
                needs_rederive |= text_changed;

                // Update autocomplete candidates after text changes
                if text_changed && state.autocomplete.is_some() {
                    crate::editor::autocomplete::update_after_edit(lines, state);
                }
            }
        }

        // Ctrl+Space (or Cmd+Space on macOS) to trigger autocomplete
        let trigger_ac = ui.input(|i| {
            let ctrl_space = i.modifiers.ctrl && i.key_pressed(egui::Key::Space);
            let cmd_period = cfg!(target_os = "macos")
                && i.modifiers.command
                && i.key_pressed(egui::Key::Period);
            ctrl_space || cmd_period
        });
        if trigger_ac
            && state.autocomplete.is_none()
            && matches!(state.mode, EditMode::Normal)
            && matches!(state.popup, PopupState::None)
        {
            let source = crate::editor::autocomplete::CompletionSource {
                named_glyphs,
                name_parts,
                doc,
            };
            crate::editor::autocomplete::trigger(lines, state, &source);
        }

        // Dismiss autocomplete when cursor moves inappropriately
        if let Some(ac) = &state.autocomplete
            && (state.cursor.line != ac.line || state.cursor.col < ac.replace_start) {
                state.autocomplete = None;
            }
        // Also re-filter if cursor moved within the token but no text changed
        if state.autocomplete.is_some() && state.cursor != prev_cursor && !needs_rederive {
            crate::editor::autocomplete::update_after_edit(lines, state);
        }
    }

    // Rename popup
    if matches!(state.popup, PopupState::Rename { .. }) {
        let popup_id = egui::Id::new("rename_popup");
        let stored_pos: Option<egui::Pos2> = ui.ctx().data(|d| d.get_temp(egui::Id::new("cursor_screen_pos")));
        let stored_rh: f32 = ui.ctx().data(|d| d.get_temp(egui::Id::new("cursor_row_height")).unwrap_or(16.0));
        let popup_pos = stored_pos.unwrap_or(egui::pos2(100.0, 100.0));
        let popup_pos = egui::pos2(popup_pos.x, popup_pos.y + stored_rh + 2.0);

        let area = egui::Area::new(popup_id)
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos);

        let area_resp = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                ui.horizontal(|ui| {
                    let kind_label = match &state.popup {
                        PopupState::Rename { kind, .. } => match kind {
                            RenameKind::Glyph => "Rename glyph",
                            RenameKind::NameParts => "Rename name-parts",
                            RenameKind::Point => "Rename point",
                            RenameKind::Color => "Rename color",
                        },
                        _ => "Rename",
                    };
                    ui.label(kind_label);
                });
                if let PopupState::Rename { new_name, focus_set, .. } = &mut state.popup {
                    let te = egui::TextEdit::singleline(new_name)
                        .desired_width(200.0);
                    let resp = ui.add(te);
                    if !*focus_set {
                        resp.request_focus();
                        if let Some(mut te_state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                            te_state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(new_name.chars().count()),
                            )));
                            te_state.store(ui.ctx(), resp.id);
                        }
                        *focus_set = true;
                    }
                    if resp.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            return Some(true); // confirm
                        }
                        return Some(false); // cancel (Escape, click outside, etc.)
                    }
                }
                None
            }).inner
        });

        match area_resp.inner {
            Some(true) => {
                // Confirmed
                if let PopupState::Rename { original_name, new_name, kind, .. } =
                    std::mem::replace(&mut state.popup, PopupState::None)
                {
                    let new_name = new_name.trim().to_string();
                    if !new_name.is_empty() && new_name != original_name {
                        rename_result = Some(RenameAction {
                            old_name: original_name,
                            new_name,
                            kind,
                        });
                    }
                }
            }
            Some(false) => {
                // Cancelled
                state.popup = PopupState::None;
            }
            None => {}
        }
    }

    // Autocomplete popup
    if state.autocomplete.is_some() {
        let popup_id = egui::Id::new("autocomplete_popup");
        let stored_pos: Option<egui::Pos2> =
            ui.ctx().data(|d| d.get_temp(egui::Id::new("cursor_screen_pos")));
        let stored_rh: f32 = ui
            .ctx()
            .data(|d| d.get_temp(egui::Id::new("cursor_row_height")).unwrap_or(16.0));
        let popup_pos = stored_pos.unwrap_or(egui::pos2(100.0, 100.0));
        let popup_pos = egui::pos2(popup_pos.x, popup_pos.y + stored_rh + 2.0);

        let ac_area = egui::Area::new(popup_id)
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let ac = state.autocomplete.as_ref().unwrap();
                    let end = ac
                        .candidates
                        .len()
                        .min(ac.scroll_offset + crate::editor::autocomplete::MAX_VISIBLE);
                    ui.set_min_width(180.0);
                    let mut clicked_idx: Option<usize> = None;
                    for i in ac.scroll_offset..end {
                        let selected = i == ac.selected;
                        let candidate = &ac.candidates[i];
                        let kind_char = match candidate.kind {
                            crate::editor::autocomplete::CompletionKind::Glyph => "G",
                            crate::editor::autocomplete::CompletionKind::NameParts => "$",
                            crate::editor::autocomplete::CompletionKind::Point => "P",
                            crate::editor::autocomplete::CompletionKind::Keyword => "K",
                            crate::editor::autocomplete::CompletionKind::GlyphFlag => "F",
                            crate::editor::autocomplete::CompletionKind::Color => "C",
                        };
                        let text = format!("{kind_char}  {}", candidate.label);
                        if ui.selectable_label(selected, &text).clicked() {
                            clicked_idx = Some(i);
                        }
                    }
                    if ac.candidates.len() > crate::editor::autocomplete::MAX_VISIBLE {
                        ui.label(format!(
                            "{}/{}",
                            ac.selected + 1,
                            ac.candidates.len()
                        ));
                    }
                    clicked_idx
                })
            });
        if let Some(clicked) = ac_area.inner.inner {
            if let Some(ac) = &mut state.autocomplete {
                ac.selected = clicked;
            }
            crate::editor::autocomplete::apply_completion(lines, state);
            needs_rederive = true;
        }
    }

    // Error tooltip: show when caret is inside an error span
    if state.active
        && matches!(state.popup, PopupState::None)
        && state.autocomplete.is_none()
    {
        let tooltip_data: Option<Option<(egui::Pos2, String)>> =
            ui.ctx().data(|d| d.get_temp(egui::Id::new("error_tooltip_data")));
        if let Some(Some((pos, msg))) = tooltip_data {
            egui::Area::new(egui::Id::new("error_tooltip"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.colored_label(pal.error, msg);
                    });
                });
        }
    }

    if state.cursor != prev_cursor {
        let cursor_y = doc_line_to_y(vlines, row_height, grid_cell, state.cursor.line);
        let cursor_h: f32 = vlines
            .iter()
            .filter(|vl| vl.doc_line == state.cursor.line)
            .map(|vl| vl.height(row_height, grid_cell))
            .sum();
        let cursor_h = if cursor_h > 0.0 { cursor_h } else { row_height };
        let scroll_y = scroll_output.state.offset.y;
        let margin = row_height * 0.5;
        if cursor_h + margin * 2.0 <= viewport_h {
            if cursor_y < scroll_y + margin {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("goto_scroll_target"),
                        (cursor_y - margin).max(0.0),
                    );
                });
            } else if cursor_y + cursor_h > scroll_y + viewport_h - margin {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("goto_scroll_target"),
                        (cursor_y + cursor_h - viewport_h + margin).max(0.0),
                    );
                });
            }
        } else if cursor_y < scroll_y + margin
            || cursor_y + cursor_h > scroll_y + viewport_h - margin
        {
            // Cursor line taller than the viewport: align its top.
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("goto_scroll_target"),
                    (cursor_y - margin).max(0.0),
                );
            });
        }
    }

    if needs_rederive {
        if let Some((item_idx, grid_doc_line)) = state.pixel_paint_dirty.take() {
            // Pixel-only fast path: sync the single modified grid without
            // reparsing the entire document or invalidating the view cache.
            flush_pixel_change(lines, doc, state, item_idx, grid_doc_line);
        } else if state.skip_reconcile {
            // `lines` changed this frame; the cached view no longer reflects it.
            // A deferred reparse leaves `edit_gen` untouched, so the key alone
            // would not invalidate — drop the cache explicitly.
            state.view_cache = None;
            flush_document_changes(lines, doc, state);
        } else {
            state.view_cache = None;
            let on_ref_line = matches!(
                lines.get(state.cursor.line),
                Some(DocLine::Text(t)) if t.trim_start().starts_with("ref ") || t.trim_start().starts_with("point ")
            );
            let on_glyph_header = matches!(
                lines.get(state.cursor.line),
                Some(DocLine::Text(t)) if crate::editor::reconcile::parse_glyph_header_dims(t).is_some()
            );
            // A text line directly above a grid is that grid's header. While
            // it is being edited it may be transiently unparseable (e.g. the
            // height digits were just deleted); reconciling now would demote
            // the grid to text mid-edit, so hold off until the caret leaves.
            let owns_grid = matches!(lines.get(state.cursor.line), Some(DocLine::Text(_)))
                && matches!(lines.get(state.cursor.line + 1), Some(DocLine::Grid(_)));

            // Deferring only works while the line structure still matches
            // the derived `Document`: visual lines are built from the stale
            // item structure, so an edit that added or removed DocLines
            // would attribute grids and headers to the wrong lines.
            let structure_stable = doc.docline_file_lines.len() == lines.len();

            let defer = structure_stable
                && matches!(state.mode, EditMode::Normal)
                && ((on_ref_line && state.last_reparse_line == Some(state.cursor.line))
                    || on_glyph_header
                    || owns_grid);

            if defer {
                defer_document_changes(doc, state);
            } else {
                flush_document_changes(lines, doc, state);
            }
        }
        state.last_reparse_line = Some(state.cursor.line);
    } else {
        state.skip_reconcile = false;
        if let Some(pend_line) = state.pending_reparse_line {
            let should_flush =
                !state.active || pend_line != state.cursor.line;
            if should_flush {
                flush_document_changes(lines, doc, state);
            }
        }
    }

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

// ---------------------------------------------------------------------------
// Selection drawing
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_selection(
    painter: &egui::Painter,
    ui: &egui::Ui,
    font_id: &egui::FontId,
    vl: &VisualLine,
    origin: egui::Pos2,
    y: f32,
    h: f32,
    sel_lo: Caret,
    sel_hi: Caret,
    grid_cell: f32,
) {
    let dl = vl.doc_line;
    if dl < sel_lo.line || dl > sel_hi.line {
        return;
    }
    let sel_color = Palette::get(ui).selection;
    match &vl.kind {
        VLineKind::Text(text) => {
            let seg_len = text.chars().count();
            let seg_start = vl.col_offset;
            let seg_end = seg_start + seg_len;
            let doc_lo = if dl == sel_lo.line { sel_lo.col } else { 0 };
            let doc_hi = if dl == sel_hi.line {
                sel_hi.col
            } else {
                seg_end
            };
            let col_lo = doc_lo.max(seg_start).saturating_sub(seg_start).min(seg_len);
            let col_hi = doc_hi.max(seg_start).saturating_sub(seg_start).min(seg_len);
            if col_lo >= col_hi {
                return;
            }
            let x0 = grid_render::char_x_pos(ui, font_id, text, col_lo);
            let x1 = grid_render::char_x_pos(ui, font_id, text, col_hi);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(origin.x + LEFT_PAD + x0, origin.y + y),
                    egui::pos2(origin.x + LEFT_PAD + x1, origin.y + y + h),
                ),
                0.0,
                sel_color,
            );
        }
        VLineKind::GridRow { extent, .. } => {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(origin.x + LEFT_PAD, origin.y + y),
                    egui::vec2(extent.display_width(grid_cell), h),
                ),
                0.0,
                sel_color,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Edit border
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_edit_border(
    painter: &egui::Painter,
    mode: &EditMode,
    vl: &VisualLine,
    _doc: &Document,
    origin: egui::Pos2,
    y: f32,
    _composites: &HashMap<usize, GlyphComposite>,
    grid_cell: f32,
    pal: &Palette,
) {
    let editing_idx = match mode {
        EditMode::GlyphEdit { item_idx, .. } => Some(*item_idx),
        EditMode::LayerMove { item_idx, .. } => Some(*item_idx),
        _ => return,
    };
    let Some(eidx) = editing_idx else { return };

    match &vl.kind {
        VLineKind::GridRow {
            item_idx,
            row,
            own_width,
            own_height,
            extent,
            ..
        } if *item_idx == eidx && *row == 0 => {
            let own_x = origin.x + LEFT_PAD + (-extent.left) as f32 * grid_cell;
            let border_rect = egui::Rect::from_min_size(
                egui::pos2(own_x, origin.y + y),
                egui::vec2(
                    *own_width as f32 * grid_cell,
                    *own_height as f32 * grid_cell,
                ),
            );
            painter.rect_stroke(
                border_rect,
                0.0,
                egui::Stroke::new(2.0, pal.cursor_border),
                egui::epaint::StrokeKind::Outside,
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Scroll physics
// ---------------------------------------------------------------------------

pub(crate) fn apply_scroll_physics(ui: &egui::Ui, zoom_level: u32, salt: &str) {
    let cmd_held = ui.input(|i| i.modifiers.command);
    if cmd_held {
        return;
    }

    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pos| ui.max_rect().contains(pos));
    if !hovered {
        return;
    }

    let accel_id = egui::Id::new(("scroll_accel_state", salt));
    let now = ui.input(|i| i.time);

    let has_line_scroll = ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    ..
                }
            )
        })
    });
    let has_point_scroll = ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    ..
                }
            )
        })
    });

    let (mut last_tick_time, mut consecutive_ticks): (f64, u32) = ui
        .ctx()
        .data(|d| d.get_temp(accel_id).unwrap_or((0.0f64, 0u32)));

    if has_line_scroll {
        if now - last_tick_time < SCROLL_RAPID_THRESHOLD {
            consecutive_ticks += 1;
        } else {
            consecutive_ticks = 1;
        }
        last_tick_time = now;
    } else if now - last_tick_time > SCROLL_ACCEL_RESET {
        consecutive_ticks = 0;
    }

    ui.ctx()
        .data_mut(|d| d.insert_temp(accel_id, (last_tick_time, consecutive_ticks)));

    let accel = if consecutive_ticks > SCROLL_ACCEL_START {
        (1.0 + (consecutive_ticks - SCROLL_ACCEL_START) as f32 * SCROLL_ACCEL_STEP)
            .min(SCROLL_ACCEL_MAX)
    } else {
        1.0
    };

    let multiplier = SCROLL_BASE_MULTIPLIER * accel * zoom_level as f32;
    let in_discrete_tail = !has_line_scroll
        && !has_point_scroll
        && consecutive_ticks > 0
        && ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.1);

    if has_line_scroll || in_discrete_tail {
        ui.ctx()
            .input_mut(|i| i.smooth_scroll_delta.y *= multiplier);
    }
}

/// The source-file line number drawn in the gutter for a visual line, if any.
/// Wrapped text continuations and grid rows outside the glyph's own area
/// carry no number.
pub(crate) fn gutter_line_number(
    vl: &VisualLine,
    lines: &[DocLine],
    source_offsets: &[usize],
) -> Option<usize> {
    match &vl.kind {
        VLineKind::Text(_) if vl.col_offset == 0 => {
            source_offsets.get(vl.doc_line).map(|&off| off + 1)
        }
        VLineKind::Text(_) => None,
        VLineKind::GridRow {
            row,
            own_height,
            grid_doc_line,
            ..
        } => {
            if *row >= 0
                && *row < *own_height as i16
                && matches!(lines.get(*grid_doc_line), Some(DocLine::Grid(g)) if !g.is_all_empty())
            {
                source_offsets
                    .get(*grid_doc_line)
                    .map(|&off| off + *row as usize + 1)
            } else {
                None
            }
        }
    }
}

pub(crate) fn source_line_offsets(lines: &[DocLine]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(lines.len());
    let mut src = 0usize;
    for line in lines {
        offsets.push(src);
        match line {
            DocLine::Text(_) => src += 1,
            DocLine::Grid(g) => {
                if !g.is_all_empty() {
                    src += g.height as usize;
                }
            }
        }
    }
    offsets
}

fn line_to_item_idx(item_line_starts: &[usize], target_line: usize) -> Option<usize> {
    item_line_starts
        .iter()
        .rposition(|&start| start <= target_line)
}

fn defer_document_changes(doc: &mut Document, state: &mut EditorState) {
    state.pending_reparse_line = Some(state.cursor.line);
    // Reconciliation/derive may be delayed while a structural line is
    // actively edited, but the source buffer is already different from the
    // saved snapshot and must be protected by dirty/save/close handling
    // immediately. Do not advance edit_gen until derive actually runs.
    doc.dirty = !state.undo.is_at_saved();
    state.clear_document_sync_request();
}

/// Bring the parsed `Document` back in sync after a discrete source edit.
///
/// Application-level edit actions run after `show_document`, so callers must
/// invoke this helper when `EditorState::apply_edit_action` returns `true`.
/// Undo/redo set `skip_reconcile` because their recorded structural state must
/// be restored verbatim; other edits reconcile grids before deriving.
pub(crate) fn flush_document_changes(
    lines: &mut Vec<DocLine>,
    doc: &mut Document,
    state: &mut EditorState,
) {
    let skip_reconcile = std::mem::replace(&mut state.skip_reconcile, false);
    if !skip_reconcile {
        loop {
            let cursor = state.cursor;
            let Some(cursor_after) =
                crate::editor::reconcile::reconcile(lines, &mut state.undo, cursor)
            else {
                break;
            };
            state.cursor = cursor_after;
        }
    }

    rederive(lines, doc, state.undo.is_at_saved());
    state.cursor = caret::clamp(lines, state.cursor);
    state.pending_reparse_line = None;
    state.last_reparse_line = Some(state.cursor.line);
    state.clear_document_sync_request();
}

fn rederive(
    lines: &[DocLine],
    doc: &mut Document,
    is_at_saved: bool,
) {
    match crate::document_io::derive_document(lines, doc.path.clone()) {
        Ok((new_doc, _)) => {
            let items_changed = !doc.items.iter().filter(|i| i.affects_font())
                .eq(new_doc.items.iter().filter(|i| i.affects_font()));
            let next_gen = doc.edit_gen + 1;
            let pixel_gen = doc.pixel_gen;
            let content_gen = if items_changed { doc.content_gen + 1 } else { doc.content_gen };
            *doc = new_doc;
            doc.dirty = !is_at_saved;
            doc.edit_gen = next_gen;
            doc.pixel_gen = pixel_gen;
            doc.content_gen = content_gen;
        }
        Err(_) => {
            doc.dirty = !is_at_saved;
            doc.edit_gen += 1;
        }
    }
}

/// Lightweight rederive for pixel-only changes: sync the modified grid from
/// `DocLine::Grid` into the corresponding `Document` item, bypassing the
/// full text reparse of `derive_document`.
fn flush_pixel_change(
    lines: &[DocLine],
    doc: &mut Document,
    state: &mut EditorState,
    item_idx: usize,
    grid_doc_line: usize,
) {
    state.skip_reconcile = false;

    if let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) {
        if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get_mut(item_idx) {
            body.pixels = Some(grid.clone());
        }
    }
    doc.docline_file_lines = crate::document::compute_docline_file_lines(lines);
    doc.pixel_gen += 1;
    doc.content_gen += 1;
    doc.dirty = !state.undo.is_at_saved();

    state.pending_reparse_line = None;
    state.last_reparse_line = Some(state.cursor.line);
    state.clear_document_sync_request();
}

fn inline_ref_to_pixels(
    lines: &mut Vec<DocLine>,
    doc: &Document,
    state: &mut EditorState,
    edit_idx: usize,
    ref_idx: usize,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    let body = match doc.items.get(edit_idx) {
        Some(DocumentItem::Glyph { body, .. }) => body,
        _ => return false,
    };
    if ref_idx >= body.refs.len() {
        return false;
    }
    let has_grid = body.pixels.is_some();

    let gref = &body.refs[ref_idx];
    let resolved = match ref_composite::resolve_ref_name_with_parts(
        &gref.name,
        named_glyphs,
        name_parts,
    ) {
        Some(r) => r,
        None => return false,
    };

    let item_start = doc.item_line_starts[edit_idx];
    let grid_line_idx = item_start + 1;

    let (eff_row, eff_col) = ref_composite::ref_effective_offset(gref, resolved);
    let negated = gref.negated;

    if has_grid {
        let body_line_count = 1 + body.refs.len() + body.points.len();
        let old_lines: Vec<DocLine> =
            lines[grid_line_idx..grid_line_idx + body_line_count].to_vec();

        if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_line_idx) {
            merge_ref_pixels(grid, resolved, eff_row, eff_col, negated);
        }

        let ref_text_line_idx = grid_line_idx + 1 + ref_idx;
        lines.remove(ref_text_line_idx);

        let new_lines: Vec<DocLine> =
            lines[grid_line_idx..grid_line_idx + body_line_count - 1].to_vec();

        let caret = state.cursor;
        state.undo.break_coalesce();
        state.undo.push_lines(grid_line_idx, old_lines, new_lines, caret, caret);
    } else {
        let header_text = match &lines[item_start] {
            DocLine::Text(s) => s.clone(),
            _ => return false,
        };
        let tokens = match document_io::tokenize_tokens(&header_text) {
            Ok(t) => t,
            Err(_) => return false,
        };
        let has_dims = parse_glyph_header_dims(&tokens).is_some();
        let (w, h) = parse_glyph_header_dims(&tokens).unwrap_or_else(|| {
            let (_min_r, _min_c, max_r, max_c) =
                ref_composite::composite_bounds(None, &body.refs, named_glyphs, name_parts);
            let w = (max_c).max(0) as u16;
            let h = (max_r).max(0) as u16;
            (w, h)
        });
        if w == 0 || h == 0 {
            return false;
        }

        let body_line_count = body.refs.len() + body.points.len();
        let undo_start = if has_dims { grid_line_idx } else { item_start };
        let old_line_count = if has_dims { body_line_count } else { 1 + body_line_count };
        let old_lines: Vec<DocLine> =
            lines[undo_start..undo_start + old_line_count].to_vec();

        if !has_dims {
            let new_header = format!("{} {} {}", header_text.trim_end(), w, h);
            lines[item_start] = DocLine::Text(new_header);
        }
        let mut grid = PixelGrid::new(w, h);
        merge_ref_pixels(&mut grid, resolved, eff_row, eff_col, negated);
        lines.insert(grid_line_idx, DocLine::Grid(grid));

        let ref_text_line_idx = grid_line_idx + 1 + ref_idx;
        lines.remove(ref_text_line_idx);

        let new_line_count = if has_dims { 1 + body.refs.len() - 1 + body.points.len() }
            else { 1 + 1 + body.refs.len() - 1 + body.points.len() };
        let new_lines: Vec<DocLine> =
            lines[undo_start..undo_start + new_line_count].to_vec();

        let caret = state.cursor;
        state.undo.break_coalesce();
        state.undo.push_lines(undo_start, old_lines, new_lines, caret, caret);
    }

    state.mode = EditMode::GlyphEdit {
        item_idx: edit_idx,
        selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
    };

    true
}

fn merge_ref_pixels(
    grid: &mut PixelGrid,
    resolved: &ResolvedGlyph,
    eff_row: i32,
    eff_col: i32,
    negated: bool,
) {
    for r in 0..resolved.grid.height as i32 {
        for c in 0..resolved.grid.width as i32 {
            let shape = resolved.grid.get(r as u16, c as u16);
            if shape.is_empty() {
                continue;
            }
            let dr = eff_row + r;
            let dc = eff_col + c;
            if dr < 0 || dc < 0 || dr >= grid.height as i32 || dc >= grid.width as i32 {
                continue;
            }
            let current = grid.get(dr as u16, dc as u16);
            let result = if negated {
                pixel::shape_subtract(current, shape)
            } else {
                pixel::shape_union(current, shape)
            };
            grid.set(dr as u16, dc as u16, result);
        }
    }
}

fn parse_glyph_header_dims(tokens: &[String]) -> Option<(u16, u16)> {
    if tokens.first().map(|s| s.as_str()) != Some("glyph") || tokens.len() < 2 {
        return None;
    }
    let parts = &tokens[2..];
    let mut fp = 0;
    while fp < parts.len() {
        match parts[fp].as_str() {
            "sticky" | "=" => { fp += 1; }
            "advance" | "left" => { fp += 2; }
            other => {
                if let Ok(w) = other.parse::<u16>() {
                    let h = parts.get(fp + 1).and_then(|s| s.parse::<u16>().ok())?;
                    return Some((w, h));
                }
                fp += 1;
            }
        }
    }
    None
}

pub fn apply_edit_action_to_editor(
    action: crate::edit_menu::EditAction,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    ctx: &egui::Context,
) -> bool {
    state.apply_edit_action(action, lines, ctx)
}

fn resolve_color_for_display(
    token: &str,
    aliases: &ColorAliasMap,
) -> Option<egui::Color32> {
    if token == "fg" {
        return None;
    }
    if token.starts_with('#') {
        let rgba = crate::render::ttf_builder::parse_hex_color(token)?;
        return Some(egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a));
    }
    let (rgba, _) = aliases.get(token)?;
    Some(egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a))
}

fn contrast_text_color(bg: egui::Color32) -> egui::Color32 {
    let [r, g, b, _] = bg.to_array();
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 128.0 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_color_backgrounds(
    painter: &egui::Painter,
    ui: &egui::Ui,
    font_id: &egui::FontId,
    text: &str,
    col_offset: usize,
    base_x: f32,
    base_y: f32,
    row_h: f32,
    aliases: &ColorAliasMap,
) {
    let trimmed = text.trim_start();
    let leading = text.chars().count() - trimmed.chars().count();
    let spans = match tokenize_with_spans(trimmed) {
        Ok(s) if !s.is_empty() => s,
        _ => return,
    };
    let keyword = spans[0].value.as_str();
    let rest = &spans[1..];

    let mut color_spans: Vec<(usize, usize, egui::Color32)> = Vec::new();

    match keyword {
        "color" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                let val_span = &rest[2];
                if let Some(color) = resolve_color_for_display(&val_span.value, aliases) {
                    color_spans.push((
                        leading + val_span.raw_start,
                        leading + val_span.raw_end,
                        color,
                    ));
                }
            }
        }
        "ref" => {
            if let Some(fill_pos) = rest.iter().position(|s| s.value == "fill")
                && let Some(color_span) = rest.get(fill_pos + 1)
                    && let Some(color) = resolve_color_for_display(&color_span.value, aliases) {
                        color_spans.push((
                            leading + color_span.raw_start,
                            leading + color_span.raw_end,
                            color,
                        ));
                    }
        }
        _ => {}
    }

    for (col_start, col_end, bg_color) in &color_spans {
        let adj_start = col_start.saturating_sub(col_offset);
        let adj_end = col_end.saturating_sub(col_offset);
        let x0 = base_x + grid_render::char_x_pos(ui, font_id, text, adj_start);
        let x1 = base_x + grid_render::char_x_pos(ui, font_id, text, adj_end);
        let rect = egui::Rect::from_min_size(
            egui::pos2(x0, base_y),
            egui::vec2(x1 - x0, row_h),
        );
        painter.rect_filled(rect, 0.0, *bg_color);
        let token_text: String = text.chars().skip(adj_start).take(adj_end - adj_start).collect();
        let fg = contrast_text_color(*bg_color);
        painter.text(
            egui::pos2(x0, base_y),
            egui::Align2::LEFT_TOP,
            &token_text,
            font_id.clone(),
            fg,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::{derive_document, parse_doclines};
    use crate::edit_menu::EditAction;

    #[test]
    fn deferred_change_is_dirty_without_advancing_generation_and_is_per_editor() {
        let lines = vec![DocLine::Text("glyph foo 2 2".into())];
        let (mut doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        doc.edit_gen = 7;

        let mut first = EditorState::new();
        first.cursor = Caret::new(0, 4);
        first.undo.push_text(
            0,
            4,
            "a".into(),
            "b".into(),
            Caret::new(0, 4),
            Caret::new(0, 5),
        );
        let second = EditorState::new();

        defer_document_changes(&mut doc, &mut first);

        assert!(doc.dirty);
        assert_eq!(doc.edit_gen, 7);
        assert_eq!(first.pending_reparse_line, Some(0));
        assert_eq!(second.pending_reparse_line, None);
    }

    #[test]
    fn external_edit_action_can_be_flushed_immediately() {
        let mut lines = vec![DocLine::Text("//abc".into())];
        let (mut doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        let mut state = EditorState::new();
        state.selection_anchor = Some(Caret::new(0, 2));
        state.cursor = Caret::new(0, 3);

        assert!(state.apply_edit_action(
            EditAction::Delete,
            &mut lines,
            &egui::Context::default(),
        ));
        flush_document_changes(&mut lines, &mut doc, &mut state);

        assert_eq!(lines, vec![DocLine::Text("//bc".into())]);
        assert!(matches!(
            doc.items.first(),
            Some(crate::document::DocumentItem::Comment(text)) if text == "bc"
        ));
        assert!(doc.dirty);
        assert_eq!(doc.edit_gen, 1);
        assert_eq!(state.pending_reparse_line, None);
        assert!(!state.take_document_sync_request());
    }

    fn assert_all_doc_lines_covered(input: &str) {
        let lines = parse_doclines(input);
        let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();

        let last_item_end = doc
            .items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let start = doc.item_line_starts[idx];
                use crate::document::DocumentItem;
                match item {
                    DocumentItem::BlankLine
                    | DocumentItem::Comment(_)
                    | DocumentItem::Directive(_)
                    | DocumentItem::FontMeta(_)
                    | DocumentItem::Map { .. }
                    | DocumentItem::NameParts { .. }
                    | DocumentItem::Remap { .. }
                    | DocumentItem::Feature { .. }
                    | DocumentItem::FeatureAnchor { .. }
                    | DocumentItem::MapDecomposed { .. }
                    | DocumentItem::Color { .. }
                    | DocumentItem::AssertShape { .. } => start + 1,
                    DocumentItem::Glyph { body, .. } => {
                        let is_alias = body.is_simple_alias();
                        if is_alias {
                            start + 1
                        } else {
                            start + 1 + if body.pixels.is_some() { 1 } else { 0 } + body.refs.len() + body.points.len()
                        }
                    }
                }
            })
            .max()
            .unwrap_or(0);

        assert_eq!(
            last_item_end,
            lines.len(),
            "item_line_starts don't cover all {n} DocLines (last item ends at {last_item_end})",
            n = lines.len(),
        );

        // Check that starts are monotonically increasing and match
        for i in 1..doc.item_line_starts.len() {
            assert!(
                doc.item_line_starts[i] > doc.item_line_starts[i - 1],
                "item_line_starts not strictly increasing at {i}: {:?}",
                &doc.item_line_starts[i - 1..=i]
            );
        }
    }

    #[test]
    fn all_lines_covered_alias_then_blank() {
        assert_all_doc_lines_covered(
            "glyph minus = dash\n\
             \n\
             glyph plusminus 8 16\n\
             ................\n\
             ................\n\
             ................\n\
             ................\n\
             ......@@@@......\n\
             ......@@@@......\n\
             ......@@@@......\n\
             ..@@@@@@@@@@@@..\n\
             ......@@@@......\n\
             ......@@@@......\n\
             ......@@@@......\n\
             ................\n\
             ..@@@@@@@@@@@@..\n\
             ................\n\
             ................\n\
             ................\n",
        );
    }

    #[test]
    fn all_lines_covered_consecutive_aliases() {
        assert_all_doc_lines_covered(
            "glyph U+002B = plus\n\
             glyph U+2212 = minus\n\
             glyph U+00B1 = plusminus\n\
             glyph U+2213 = minusplus\n\
             glyph U+00D7 = times\n\
             glyph U+00F7 = div\n",
        );
    }

    #[test]
    fn all_lines_covered_glyph_with_ref_then_alias() {
        assert_all_doc_lines_covered(
            "glyph div 8 16\n\
             ................\n\
             ................\n\
             ................\n\
             ................\n\
             ................\n\
             ......@@@@......\n\
             ......@@@@......\n\
             ................\n\
             ................\n\
             ................\n\
             ......@@@@......\n\
             ......@@@@......\n\
             ................\n\
             ................\n\
             ................\n\
             ................\n\
             ref hyphen 0 0\n\
             \n\
             glyph U+002B = plus\n",
        );
    }

    #[test]
    fn all_lines_covered_ref_only_single_ref_at_origin() {
        assert_all_doc_lines_covered(
            "glyph composite\n\
             ref other 0 0\n\
             \n\
             glyph next 2 2\n\
             ..@@\n\
             @@..\n",
        );
    }

    #[test]
    fn all_lines_covered_all_directive_types() {
        assert_all_doc_lines_covered(
            "\
font-meta height 16 ascent 12 descent 4

// comment
name-parts $base = stem wide

glyph stem 2 2
@@@@
..@@

glyph wide 3 1
@@..@@

glyph alias = stem

glyph comp
ref stem
ref wide 1 0
point -join 0 0
point +join 2 0

glyph sticky-empty sticky advance 0

map A = stem
map B = wide
remap set1 : stem -> wide
feature liga for latn : set1
exclude-from-sample stem
",
        );
    }
}

fn handle_shape_shortcuts(ui: &egui::Ui, selected_shape: &mut pixel::PixelShape) {
    use pixel::*;

    // (key, cycle of shapes) — cycle length 1..=3
    const MAPPINGS: &[(egui::Key, &[PixelShape])] = &[
        (egui::Key::Num1, &[PixelShape(PX_ALMOSTFULL | PX_FULL)]),
        // asdf: halves → halfslant H (w2:h1, 3/4) → halfslant V (w1:h2, 3/4)
        (egui::Key::F, &[
            PixelShape(PX_HALF1 | PX_FULL),
            PixelShape(PX_HALFSLANT1H | PX_FULL),
            PixelShape(PX_HALFSLANT1V | PX_FULL),
        ]),
        (egui::Key::S, &[
            PixelShape(PX_HALF2 | PX_FULL),
            PixelShape(PX_HALFSLANT2H | PX_FULL),
            PixelShape(PX_HALFSLANT2V | PX_FULL),
        ]),
        (egui::Key::A, &[
            PixelShape(PX_HALF3 | PX_FULL),
            PixelShape(PX_HALFSLANT3H | PX_FULL),
            PixelShape(PX_HALFSLANT3V | PX_FULL),
        ]),
        (egui::Key::D, &[
            PixelShape(PX_HALF4 | PX_FULL),
            PixelShape(PX_HALFSLANT4H | PX_FULL),
            PixelShape(PX_HALFSLANT4V | PX_FULL),
        ]),
        // qwer: quad → cone
        (egui::Key::R, &[PixelShape(PX_QUAD1 | PX_FULL), PixelShape(PX_CONE1 | PX_FULL)]),
        (egui::Key::Q, &[PixelShape(PX_QUAD2 | PX_FULL), PixelShape(PX_CONE2 | PX_FULL)]),
        (egui::Key::W, &[PixelShape(PX_QUAD3 | PX_FULL), PixelShape(PX_CONE3 | PX_FULL)]),
        (egui::Key::E, &[PixelShape(PX_QUAD4 | PX_FULL), PixelShape(PX_CONE4 | PX_FULL)]),
        // zxcv: invquad → invcone
        (egui::Key::V, &[PixelShape(PX_INVQUAD1 | PX_FULL), PixelShape(PX_INVCONE1 | PX_FULL)]),
        (egui::Key::Z, &[PixelShape(PX_INVQUAD2 | PX_FULL), PixelShape(PX_INVCONE2 | PX_FULL)]),
        (egui::Key::X, &[PixelShape(PX_INVQUAD3 | PX_FULL), PixelShape(PX_INVCONE3 | PX_FULL)]),
        (egui::Key::C, &[PixelShape(PX_INVQUAD4 | PX_FULL), PixelShape(PX_INVCONE4 | PX_FULL)]),
    ];

    for &(key, cycle) in MAPPINGS {
        if ui.input(|i| i.key_pressed(key) && !i.modifiers.command && !i.modifiers.alt) {
            if cycle.len() == 1 {
                *selected_shape = cycle[0];
            } else {
                let cur_pos = cycle.iter().position(|s| {
                    *s == *selected_shape
                        || (s.is_slant_pair()
                            && *selected_shape == s.slant_direction_pair())
                });
                *selected_shape = match cur_pos {
                    Some(i) => cycle[(i + 1) % cycle.len()],
                    None => cycle[0],
                };
            }
        }
    }
}
