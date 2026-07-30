use crate::document::{DocLine, Document, DocumentItem, PixelGrid};
use crate::editor::undo::{self, PixelSelectionSnapshot};
use crate::editor::{EditMode, EditorState, Slot};
use crate::pixel::{self, PixelShape};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PixelSelection {
    pub item_idx: usize,
    pub row: i16,
    pub col: i16,
    pub width: u16,
    pub height: u16,
    pub float_pixels: Option<PixelGrid>,
}

impl PixelSelection {
    pub fn is_floating(&self) -> bool {
        self.float_pixels.is_some()
    }

    pub fn contains(&self, row: i16, col: i16) -> bool {
        row >= self.row
            && row < self.row + self.height as i16
            && col >= self.col
            && col < self.col + self.width as i16
    }

    pub fn grid_doc_line(&self, doc: &Document) -> Option<usize> {
        let start = doc.item_line_starts.get(self.item_idx)?;
        if let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(self.item_idx) {
            if body.pixels.is_some() {
                return Some(start + 1);
            }
        }
        None
    }

    pub fn to_snapshot(&self) -> PixelSelectionSnapshot {
        PixelSelectionSnapshot {
            item_idx: self.item_idx,
            row: self.row,
            col: self.col,
            width: self.width,
            height: self.height,
            float_pixels: self.float_pixels.clone(),
        }
    }

    pub fn from_snapshot(snap: &PixelSelectionSnapshot) -> Self {
        Self {
            item_idx: snap.item_idx,
            row: snap.row,
            col: snap.col,
            width: snap.width,
            height: snap.height,
            float_pixels: snap.float_pixels.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Drag state stored in the owning editor's egui temp slot
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum SelectDrag {
    New { anchor_row: i16, anchor_col: i16 },
    Move { accum: egui::Vec2 },
}

/// The owning editor's slot for the in-progress selection drag.
fn drag_id(state: &EditorState) -> egui::Id {
    state.key(Slot::PixelSelectDrag)
}

// ---------------------------------------------------------------------------
// Per-row interaction handler (called from document_view for each grid row)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_pixel_select_interaction(
    ui: &egui::Ui,
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: &mut bool,
    grid_doc_line: usize,
    item_idx: usize,
    pixel_row: i16,
    grid_width: u16,
    grid_height: u16,
    extent: super::document_view::GridExtent,
    strip: &super::document_view::GridStrip,
    grid_x: f32,
    grid_y: f32,
    grid_cell: f32,
) {
    if !matches!(state.mode, EditMode::PixelSelect { item_idx: eidx } if eidx == item_idx) {
        return;
    }

    let sel_drag_id = drag_id(state);
    let in_own_row = pixel_row >= 0 && pixel_row < grid_height as i16;

    // Right-click: cancel selection
    if ui.input(|i| i.pointer.secondary_pressed()) && state.pixel_selection.is_some() {
        let sel = state.pixel_selection.clone().unwrap();
        if sel.item_idx == item_idx {
            commit_and_clear(doc, lines, state, &sel);
            ui.ctx().request_repaint();
            return;
        }
    }

    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_down = ui.input(|i| i.pointer.primary_down());

    if !primary_pressed && !primary_down {
        ui.data_mut(|d| d.remove::<SelectDrag>(sel_drag_id));
        return;
    }

    let Some(hp) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };

    // Only process if pointer is on this row, inside the visible grid band.
    if hp.y < grid_y || hp.y >= grid_y + grid_cell || !strip.accepts_pointer(hp) {
        return;
    }

    let rel_x = hp.x - grid_x;
    let gc = (rel_x / grid_cell) as i32 + extent.left as i32;
    if !in_own_row || gc < 0 || gc >= grid_width as i32 {
        return;
    }
    let hover_col = gc as i16;
    let hover_row = pixel_row;

    if primary_pressed {
        let inside = state
            .pixel_selection
            .as_ref()
            .is_some_and(|s| s.item_idx == item_idx && s.contains(hover_row, hover_col));

        if inside {
            // Start move drag
            ui.data_mut(|d| {
                d.insert_temp(sel_drag_id, SelectDrag::Move { accum: egui::Vec2::ZERO })
            });
        } else {
            // Commit existing floating selection before starting new one
            if let Some(sel) = state.pixel_selection.clone() {
                if sel.item_idx == item_idx && sel.is_floating() {
                    commit_floating(doc, lines, state, &sel);
                    *needs_rederive = true;
                }
            }
            state.pixel_selection = Some(PixelSelection {
                item_idx,
                row: hover_row,
                col: hover_col,
                width: 1,
                height: 1,
                float_pixels: None,
            });
            ui.data_mut(|d| {
                d.insert_temp(
                    sel_drag_id,
                    SelectDrag::New {
                        anchor_row: hover_row,
                        anchor_col: hover_col,
                    },
                )
            });
            ui.ctx().request_repaint();
        }
        return;
    }

    // primary_down (held) - process drag
    let drag_state: Option<SelectDrag> = ui.data(|d| d.get_temp(sel_drag_id));
    let Some(drag) = drag_state else { return };

    match drag {
        SelectDrag::New {
            anchor_row,
            anchor_col,
        } => {
            let r0 = anchor_row.min(hover_row);
            let r1 = anchor_row.max(hover_row);
            let c0 = anchor_col.min(hover_col);
            let c1 = anchor_col.max(hover_col);
            state.pixel_selection = Some(PixelSelection {
                item_idx,
                row: r0,
                col: c0,
                width: (c1 - c0 + 1) as u16,
                height: (r1 - r0 + 1) as u16,
                float_pixels: None,
            });
            ui.ctx().request_repaint();
        }
        SelectDrag::Move { mut accum } => {
            let drag_delta = ui.input(|i| i.pointer.delta());
            accum += drag_delta;

            let dcol = (accum.x / grid_cell).round() as i16;
            let drow = (accum.y / grid_cell).round() as i16;

            if dcol == 0 && drow == 0 {
                ui.data_mut(|d| d.insert_temp(sel_drag_id, SelectDrag::Move { accum }));
                return;
            }

            let Some(sel) = state.pixel_selection.clone() else {
                return;
            };
            if sel.item_idx != item_idx {
                return;
            }

            // Clamp to grid bounds
            let new_row = (sel.row + drow).clamp(0, grid_height as i16 - sel.height as i16);
            let new_col = (sel.col + dcol).clamp(0, grid_width as i16 - sel.width as i16);
            let actual_drow = new_row - sel.row;
            let actual_dcol = new_col - sel.col;

            if actual_drow == 0 && actual_dcol == 0 {
                ui.data_mut(|d| d.insert_temp(sel_drag_id, SelectDrag::Move { accum }));
                return;
            }

            accum.x -= actual_dcol as f32 * grid_cell;
            accum.y -= actual_drow as f32 * grid_cell;
            ui.data_mut(|d| d.insert_temp(sel_drag_id, SelectDrag::Move { accum }));

            let before_snap = sel.to_snapshot();
            let mode_before = state.mode.clone();

            // First move: extract pixels from grid
            let mut pixel_changes = Vec::new();
            if !sel.is_floating() {
                let Some(extracted) = extract_grounded_to_float(
                    lines, grid_doc_line, &sel, grid_width, grid_height, &mut pixel_changes,
                ) else {
                    return;
                };

                state.pixel_selection = Some(PixelSelection {
                    item_idx,
                    row: new_row,
                    col: new_col,
                    width: sel.width,
                    height: sel.height,
                    float_pixels: Some(extracted),
                });
            } else {
                state.pixel_selection = Some(PixelSelection {
                    item_idx,
                    row: new_row,
                    col: new_col,
                    width: sel.width,
                    height: sel.height,
                    float_pixels: sel.float_pixels.clone(),
                });
            }

            let after_snap = state.pixel_selection.as_ref().unwrap().to_snapshot();
            state.undo.push_pixel_selection(
                grid_doc_line,
                pixel_changes,
                mode_before,
                state.mode.clone(),
                Some(before_snap),
                Some(after_snap),
                state.cursor,
                state.cursor,
            );
            state.skip_reconcile = true;
            *needs_rederive = true;
            ui.ctx().request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------
// Commit / merge / delete
// ---------------------------------------------------------------------------

pub(crate) fn commit_floating(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    sel: &PixelSelection,
) {
    if !sel.is_floating() {
        return;
    }
    let Some(grid_doc_line) = sel.grid_doc_line(doc) else {
        return;
    };
    let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) else {
        return;
    };
    let float = sel.float_pixels.as_ref().unwrap();

    let mut changes = Vec::new();
    for r in 0..sel.height {
        for c in 0..sel.width {
            let dr = sel.row + r as i16;
            let dc = sel.col + c as i16;
            if dr < 0 || dc < 0 || dr >= grid.height as i16 || dc >= grid.width as i16 {
                continue;
            }
            let old = grid.get(dr as u16, dc as u16);
            let new = float.get(r, c);
            if old != new {
                changes.push(undo::PixelChange {
                    row: dr as u16,
                    col: dc as u16,
                    old,
                    new,
                });
            }
        }
    }

    if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_doc_line) {
        for ch in &changes {
            grid.set(ch.row, ch.col, ch.new);
        }
    }

    let before = sel.to_snapshot();
    state.undo.push_pixel_selection(
        grid_doc_line,
        changes,
        state.mode.clone(),
        state.mode.clone(),
        Some(before),
        None,
        state.cursor,
        state.cursor,
    );
}

pub(crate) fn commit_and_clear(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    sel: &PixelSelection,
) {
    commit_floating(doc, lines, state, sel);
    state.pixel_selection = None;
}

pub(crate) fn handle_delete_selection(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
) {
    let Some(sel) = state.pixel_selection.clone() else {
        return;
    };
    let Some(grid_doc_line) = sel.grid_doc_line(doc) else {
        return;
    };

    if sel.is_floating() {
        // Discard floating layer (no merge)
        let before = sel.to_snapshot();
        state.undo.push_pixel_selection(
            grid_doc_line,
            Vec::new(),
            state.mode.clone(),
            state.mode.clone(),
            Some(before),
            None,
            state.cursor,
            state.cursor,
        );
    } else {
        // Fill grounded selection with empty
        let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) else {
            state.pixel_selection = None;
            return;
        };
        let mut changes = Vec::new();
        for r in 0..sel.height {
            for c in 0..sel.width {
                let gr = (sel.row + r as i16) as u16;
                let gc = (sel.col + c as i16) as u16;
                if gr < grid.height && gc < grid.width {
                    let old = grid.get(gr, gc);
                    if !old.is_empty() {
                        changes.push(undo::PixelChange {
                            row: gr,
                            col: gc,
                            old,
                            new: PixelShape::EMPTY,
                        });
                    }
                }
            }
        }
        if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_doc_line) {
            for ch in &changes {
                grid.set(ch.row, ch.col, ch.new);
            }
        }
        state
            .undo
            .push_pixel_batch(grid_doc_line, changes, state.cursor, state.cursor);
    }
    state.pixel_selection = None;
}

// ---------------------------------------------------------------------------
// Clipboard: copy / cut / paste
// ---------------------------------------------------------------------------

pub(crate) fn copy_selection(
    doc: &Document,
    lines: &[DocLine],
    sel: &PixelSelection,
) -> Option<String> {
    if sel.width == 0 || sel.height == 0 {
        return None;
    }

    let mut result = String::new();
    for r in 0..sel.height {
        if r > 0 {
            result.push('\n');
        }
        for c in 0..sel.width {
            let shape = if let Some(float) = &sel.float_pixels {
                float.get(r, c)
            } else {
                let grid_doc_line = sel.grid_doc_line(doc)?;
                let DocLine::Grid(grid) = &lines[grid_doc_line] else {
                    return None;
                };
                let gr = sel.row + r as i16;
                let gc = sel.col + c as i16;
                if gr >= 0
                    && gc >= 0
                    && (gr as u16) < grid.height
                    && (gc as u16) < grid.width
                {
                    grid.get(gr as u16, gc as u16)
                } else {
                    PixelShape::EMPTY
                }
            };
            let [c1, c2] = pixel::shape_to_chars(shape);
            result.push(c1);
            result.push(c2);
        }
    }
    Some(result)
}

pub(crate) fn parse_pixel_rect(text: &str) -> Option<PixelGrid> {
    let text = text.trim_end_matches('\n');
    if text.is_empty() {
        return None;
    }
    let row_texts: Vec<&str> = text.split('\n').collect();
    let height = row_texts.len();
    if height == 0 {
        return None;
    }
    let first_len = row_texts[0].chars().count();
    if first_len == 0 || first_len % 2 != 0 {
        return None;
    }
    let width = first_len / 2;

    // Verify all rows have the same length
    for row_text in &row_texts {
        if row_text.chars().count() != first_len {
            return None;
        }
    }

    let mut grid = PixelGrid::new(width as u16, height as u16);
    for (r, row_text) in row_texts.iter().enumerate() {
        let chars: Vec<char> = row_text.chars().collect();
        for c in 0..width {
            let shape = pixel::chars_to_shape(chars[c * 2], chars[c * 2 + 1])?;
            grid.set(r as u16, c as u16, shape);
        }
    }
    Some(grid)
}

pub(crate) fn paste_selection(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    clipboard_text: &str,
) -> bool {
    let Some(clip_grid) = parse_pixel_rect(clipboard_text) else {
        return false;
    };

    // Determine target item
    let Some(item_idx) = state.mode.pixel_edit_item_idx() else {
        return false;
    };

    // Must have a pixel grid
    let has_pixels = matches!(
        doc.items.get(item_idx),
        Some(DocumentItem::Glyph { body, .. }) if body.pixels.is_some()
    );
    if !has_pixels {
        return false;
    }

    // Size check against current selection
    if let Some(sel) = &state.pixel_selection {
        if sel.item_idx == item_idx
            && (clip_grid.width < sel.width || clip_grid.height < sel.height)
        {
            return false;
        }
    }

    // Commit existing floating selection
    if let Some(sel) = state.pixel_selection.clone() {
        if sel.item_idx == item_idx && sel.is_floating() {
            commit_floating(doc, lines, state, &sel);
        }
    }

    let before = state
        .pixel_selection
        .as_ref()
        .filter(|s| s.item_idx == item_idx)
        .map(|s| s.to_snapshot());

    let paste_row = state
        .pixel_selection
        .as_ref()
        .filter(|s| s.item_idx == item_idx)
        .map(|s| s.row)
        .unwrap_or(0);
    let paste_col = state
        .pixel_selection
        .as_ref()
        .filter(|s| s.item_idx == item_idx)
        .map(|s| s.col)
        .unwrap_or(0);

    let mode_before = state.mode.clone();
    state.mode = EditMode::PixelSelect { item_idx };

    let new_sel = PixelSelection {
        item_idx,
        row: paste_row,
        col: paste_col,
        width: clip_grid.width,
        height: clip_grid.height,
        float_pixels: Some(clip_grid),
    };
    let after = new_sel.to_snapshot();
    state.pixel_selection = Some(new_sel);

    let grid_doc_line = state
        .pixel_selection
        .as_ref()
        .and_then(|s| s.grid_doc_line(doc))
        .unwrap_or(0);

    state.undo.push_pixel_selection(
        grid_doc_line,
        Vec::new(),
        mode_before,
        state.mode.clone(),
        before,
        Some(after),
        state.cursor,
        state.cursor,
    );

    true
}

// ---------------------------------------------------------------------------
// Transform operations (mirror, flip, rotate, opposite)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionTransform {
    MirrorH,
    FlipV,
    RotateCW,
    RotateCCW,
    Rotate180,
    Opposite,
    OppositeBitmap,
}

fn apply_transform(grid: &PixelGrid, transform: SelectionTransform) -> PixelGrid {
    match transform {
        SelectionTransform::MirrorH => grid.mirror_h(),
        SelectionTransform::FlipV => grid.flip_v(),
        SelectionTransform::RotateCW => grid.rotate_cw(),
        SelectionTransform::RotateCCW => grid.rotate_ccw(),
        SelectionTransform::Rotate180 => grid.rotate_180(),
        SelectionTransform::Opposite => grid.opposite(),
        SelectionTransform::OppositeBitmap => grid.opposite_bitmap(),
    }
}

/// Extracts the grounded selection's pixels into a floating grid, recording
/// each cleared cell in `pixel_changes` and applying the clears to the doc
/// grid.  Returns `None` when `grid_doc_line` is not a pixel grid.
fn extract_grounded_to_float(
    lines: &mut [DocLine],
    grid_doc_line: usize,
    sel: &PixelSelection,
    grid_width: u16,
    grid_height: u16,
    pixel_changes: &mut Vec<undo::PixelChange>,
) -> Option<PixelGrid> {
    let DocLine::Grid(grid) = lines.get(grid_doc_line)? else {
        return None;
    };
    let mut extracted = PixelGrid::new(sel.width, sel.height);
    for r in 0..sel.height {
        for c in 0..sel.width {
            let gr = sel.row + r as i16;
            let gc = sel.col + c as i16;
            if gr >= 0 && gr < grid_height as i16 && gc >= 0 && gc < grid_width as i16 {
                let shape = grid.get(gr as u16, gc as u16);
                extracted.set(r, c, shape);
                if !shape.is_empty() {
                    pixel_changes.push(undo::PixelChange {
                        row: gr as u16,
                        col: gc as u16,
                        old: shape,
                        new: PixelShape::EMPTY,
                    });
                }
            }
        }
    }
    if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_doc_line) {
        for ch in pixel_changes.iter() {
            grid.set(ch.row, ch.col, ch.new);
        }
    }
    Some(extracted)
}

pub(crate) fn can_transform(
    doc: &Document,
    state: &EditorState,
    transform: SelectionTransform,
) -> bool {
    let Some(item_idx) = state.mode.pixel_edit_item_idx() else {
        return false;
    };
    let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) else {
        return false;
    };
    let Some(pixels) = &body.pixels else {
        return false;
    };

    let (sel_w, sel_h) = if let Some(sel) = &state.pixel_selection {
        if sel.item_idx != item_idx {
            return false;
        }
        (sel.width, sel.height)
    } else {
        (pixels.width, pixels.height)
    };

    match transform {
        SelectionTransform::RotateCW | SelectionTransform::RotateCCW => {
            if sel_w == sel_h {
                return true;
            }
            if state.pixel_selection.is_some() {
                // After rotation, dimensions become (sel_h × sel_w); check if it fits
                sel_h <= pixels.width && sel_w <= pixels.height
            } else {
                false
            }
        }
        _ => true,
    }
}

pub(crate) fn handle_transform_selection(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    transform: SelectionTransform,
) -> bool {
    let Some(item_idx) = state.mode.pixel_edit_item_idx() else {
        return false;
    };
    let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) else {
        return false;
    };
    let Some(pixels) = &body.pixels else {
        return false;
    };
    let grid_width = pixels.width;
    let grid_height = pixels.height;

    let start = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
    let grid_doc_line = start + 1;

    if state.pixel_selection.is_some() {
        // Transform the selection (grounded → floating, then transform floating)
        let sel = state.pixel_selection.clone().unwrap();
        if sel.item_idx != item_idx {
            return false;
        }

        let before_snap = sel.to_snapshot();
        let mode_before = state.mode.clone();

        let mut pixel_changes = Vec::new();
        let source_grid = if let Some(float) = &sel.float_pixels {
            float.clone()
        } else {
            // Extract pixels from grid (grounded → floating)
            let Some(extracted) = extract_grounded_to_float(
                lines, grid_doc_line, &sel, grid_width, grid_height, &mut pixel_changes,
            ) else {
                return false;
            };
            extracted
        };

        let transformed = apply_transform(&source_grid, transform);

        let new_w = transformed.width;
        let new_h = transformed.height;

        // Compute new position: try to keep center, clamp to grid bounds
        let old_center_r = sel.row as f32 + sel.height as f32 / 2.0;
        let old_center_c = sel.col as f32 + sel.width as f32 / 2.0;
        let new_row = ((old_center_r - new_h as f32 / 2.0).round() as i16)
            .clamp(0, grid_height as i16 - new_h as i16);
        let new_col = ((old_center_c - new_w as f32 / 2.0).round() as i16)
            .clamp(0, grid_width as i16 - new_w as i16);

        state.mode = EditMode::PixelSelect { item_idx };
        state.pixel_selection = Some(PixelSelection {
            item_idx,
            row: new_row,
            col: new_col,
            width: new_w,
            height: new_h,
            float_pixels: Some(transformed),
        });

        let after_snap = state.pixel_selection.as_ref().unwrap().to_snapshot();
        state.undo.push_pixel_selection(
            grid_doc_line,
            pixel_changes,
            mode_before,
            state.mode.clone(),
            Some(before_snap),
            Some(after_snap),
            state.cursor,
            state.cursor,
        );
        state.skip_reconcile = true;
        true
    } else {
        // No selection: transform entire glyph grid in-place
        let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) else {
            return false;
        };
        let old_grid = grid.clone();

        let transformed = apply_transform(&old_grid, transform);

        // Compute pixel changes for undo
        let mut changes = Vec::new();
        for r in 0..grid_height {
            for c in 0..grid_width {
                let old = old_grid.get(r, c);
                let new = transformed.get(r, c);
                if old != new {
                    changes.push(undo::PixelChange { row: r, col: c, old, new });
                }
            }
        }

        if changes.is_empty() {
            return false;
        }

        if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_doc_line) {
            for ch in &changes {
                grid.set(ch.row, ch.col, ch.new);
            }
        }

        state
            .undo
            .push_pixel_batch(grid_doc_line, changes, state.cursor, state.cursor);
        state.skip_reconcile = true;
        true
    }
}

// ---------------------------------------------------------------------------
// Scale adjustment
// ---------------------------------------------------------------------------

fn round_half_to_even(x: f64) -> i64 {
    let floor = x.floor();
    let frac = x - floor;
    if frac > 0.5 {
        floor as i64 + 1
    } else if frac < 0.5 {
        floor as i64
    } else {
        let f = floor as i64;
        if f % 2 == 0 { f } else { f + 1 }
    }
}

fn scale_offset(v: i16, old_scale: u8, new_scale: u8) -> i16 {
    round_half_to_even(v as f64 * new_scale as f64 / old_scale as f64) as i16
}

fn scale_range(start: i16, end: i16, old_scale: u8, new_scale: u8) -> (i16, i16) {
    let new_start = scale_offset(start, old_scale, new_scale);
    let new_end = scale_offset(end + 1, old_scale, new_scale) - 1;
    (new_start, new_end.max(new_start))
}

pub(crate) fn current_glyph_item_idx(
    doc: &Document,
    lines: &[DocLine],
    state: &EditorState,
) -> Option<usize> {
    match state.mode.pixel_edit_item_idx() {
        Some(item_idx) => Some(item_idx),
        None => {
            let cursor_line = state.cursor.line;
            let idx = doc
                .item_line_starts
                .iter()
                .rposition(|&start| start <= cursor_line)?;
            if let Some(DocumentItem::Glyph { .. }) = doc.items.get(idx) {
                // Verify cursor is within this glyph block (before the next item)
                let end = doc
                    .item_line_starts
                    .get(idx + 1)
                    .copied()
                    .unwrap_or(lines.len());
                if cursor_line < end { Some(idx) } else { None }
            } else {
                None
            }
        }
    }
}

pub(crate) fn can_adjust_scale(
    doc: &Document,
    lines: &[DocLine],
    state: &EditorState,
) -> Option<u8> {
    let item_idx = current_glyph_item_idx(doc, lines, state)?;
    let DocumentItem::Glyph { body, .. } = doc.items.get(item_idx)? else {
        return None;
    };
    body.pixels.as_ref()?;
    Some(body.scale)
}

pub(crate) fn handle_adjust_scale(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    new_scale: u8,
) -> bool {
    let Some(item_idx) = current_glyph_item_idx(doc, lines, state) else {
        return false;
    };
    let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) else {
        return false;
    };
    let old_scale = body.scale;
    if old_scale == new_scale || new_scale == 0 {
        return false;
    }
    if body.pixels.is_none() {
        return false;
    }

    let start = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
    let end = doc
        .item_line_starts
        .get(item_idx + 1)
        .copied()
        .unwrap_or(lines.len());

    let old_lines: Vec<DocLine> = lines[start..end].to_vec();
    let mut new_lines: Vec<DocLine> = Vec::with_capacity(old_lines.len());

    for (i, line) in old_lines.iter().enumerate() {
        match line {
            DocLine::Text(t) if i == 0 => {
                new_lines.push(DocLine::Text(rewrite_scale_in_header(t, new_scale)));
            }
            DocLine::Grid(grid) => {
                new_lines.push(DocLine::Grid(grid.rescale(old_scale, new_scale)));
            }
            DocLine::Text(t) => {
                let trimmed = t.trim();
                if let Ok(tokens) = crate::document_io::tokenize_tokens(trimmed) {
                    if tokens.first().is_some_and(|k| k == "ref") {
                        new_lines.push(DocLine::Text(
                            rewrite_ref_line(&tokens, old_scale, new_scale),
                        ));
                        continue;
                    }
                    if tokens.first().is_some_and(|k| k == "anchor") {
                        new_lines.push(DocLine::Text(
                            rewrite_anchor_line(&tokens, old_scale, new_scale),
                        ));
                        continue;
                    }
                }
                new_lines.push(line.clone());
            }
        }
    }

    let cursor = state.cursor;
    state.undo.break_coalesce();
    state.undo.push_lines(start, old_lines, new_lines.clone(), cursor, cursor);
    state.undo.break_coalesce();
    lines.splice(start..end, new_lines);
    state.skip_reconcile = true;
    true
}

fn rewrite_scale_in_header(header: &str, new_scale: u8) -> String {
    let Ok(tokens) = crate::document_io::tokenize_tokens(header.trim()) else {
        return header.to_string();
    };

    let mut result: Vec<String> = Vec::new();
    let mut i = 0;
    let mut scale_written = false;
    while i < tokens.len() {
        if tokens[i] == "scale" && i + 1 < tokens.len() {
            if new_scale > 1 {
                result.push("scale".into());
                result.push(new_scale.to_string());
                scale_written = true;
            }
            i += 2;
        } else {
            result.push(crate::document_io::quote_token(&tokens[i]));
            i += 1;
        }
    }
    if new_scale > 1 && !scale_written {
        result.push("scale".into());
        result.push(new_scale.to_string());
    }
    result.join(" ")
}

fn rewrite_ref_line(tokens: &[String], old_scale: u8, new_scale: u8) -> String {
    // ref NAME [COL ROW] [negated] [fill COLOR] [coloronly|monoonly]
    if tokens.len() < 2 {
        return tokens.iter().map(|t| crate::document_io::quote_token(t)).collect::<Vec<_>>().join(" ");
    }
    let mut out = vec![
        "ref".to_string(),
        crate::document_io::quote_token(&tokens[1]),
    ];
    let mut i = 2;
    if i + 1 < tokens.len()
        && tokens[i].parse::<i16>().is_ok()
        && tokens[i + 1].parse::<i16>().is_ok()
    {
        let col: i16 = tokens[i].parse().unwrap();
        let row: i16 = tokens[i + 1].parse().unwrap();
        out.push(scale_offset(col, old_scale, new_scale).to_string());
        out.push(scale_offset(row, old_scale, new_scale).to_string());
        i += 2;
    }
    while i < tokens.len() {
        out.push(crate::document_io::quote_token(&tokens[i]));
        i += 1;
    }
    out.join(" ")
}

fn rewrite_anchor_line(tokens: &[String], old_scale: u8, new_scale: u8) -> String {
    // anchor POSITION COL_RANGE ROW_RANGE
    if tokens.len() != 4 {
        return tokens.iter().map(|t| crate::document_io::quote_token(t)).collect::<Vec<_>>().join(" ");
    }
    let keyword = &tokens[0];
    let position = crate::document_io::quote_token(&tokens[1]);

    let col_s = scale_range_token(&tokens[2], old_scale, new_scale);
    let row_s = scale_range_token(&tokens[3], old_scale, new_scale);

    format!("{keyword} {position} {col_s} {row_s}")
}

fn scale_range_token(tok: &str, old_scale: u8, new_scale: u8) -> String {
    if let Some((start_s, end_s)) = tok.split_once("..") {
        if let (Ok(start), Ok(end)) = (start_s.parse::<i16>(), end_s.parse::<i16>()) {
            let (new_start, new_end) = scale_range(start, end, old_scale, new_scale);
            return format!("{new_start}..{new_end}");
        }
    } else if let Ok(v) = tok.parse::<i16>() {
        let (new_start, new_end) = scale_range(v, v, old_scale, new_scale);
        if new_start == new_end {
            return new_start.to_string();
        }
        return format!("{new_start}..{new_end}");
    }
    tok.to_string()
}

// ---------------------------------------------------------------------------
// Reconciliation — called once per frame at the top of show_document
// ---------------------------------------------------------------------------

pub(crate) fn reconcile(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
) -> bool {
    let sel = match &state.pixel_selection {
        Some(s) => s.clone(),
        None => return false,
    };
    let matches_mode = matches!(
        &state.mode,
        EditMode::PixelSelect { item_idx } if *item_idx == sel.item_idx
    );
    if !matches_mode || !state.active {
        commit_and_clear(doc, lines, state, &sel);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::caret::Caret;

    fn c(line: usize, col: usize) -> Caret {
        Caret::new(line, col)
    }

    #[test]
    fn parse_pixel_rect_valid() {
        let text = "@@..\n..@@";
        let grid = parse_pixel_rect(text).unwrap();
        assert_eq!(grid.width, 2);
        assert_eq!(grid.height, 2);
        assert!(grid.get(0, 0).is_filled());
        assert!(grid.get(0, 1).is_empty());
        assert!(grid.get(1, 0).is_empty());
        assert!(grid.get(1, 1).is_filled());
    }

    #[test]
    fn parse_pixel_rect_invalid() {
        assert!(parse_pixel_rect("").is_none());
        assert!(parse_pixel_rect("@").is_none()); // odd length
        assert!(parse_pixel_rect("@@\n@").is_none()); // inconsistent lengths
        assert!(parse_pixel_rect("ZZ").is_none()); // invalid shape chars
    }

    #[test]
    fn selection_contains() {
        let sel = PixelSelection {
            item_idx: 0,
            row: 2,
            col: 3,
            width: 4,
            height: 3,
            float_pixels: None,
        };
        assert!(sel.contains(2, 3));
        assert!(sel.contains(4, 6));
        assert!(!sel.contains(1, 3));
        assert!(!sel.contains(5, 3));
        assert!(!sel.contains(2, 2));
        assert!(!sel.contains(2, 7));
    }

    #[test]
    fn snapshot_roundtrip() {
        let sel = PixelSelection {
            item_idx: 5,
            row: -1,
            col: 3,
            width: 2,
            height: 4,
            float_pixels: Some(PixelGrid::new(2, 4)),
        };
        let snap = sel.to_snapshot();
        let restored = PixelSelection::from_snapshot(&snap);
        assert_eq!(sel, restored);
    }

    #[test]
    fn copy_grounded_selection() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@@@..\n..@@@@";
        let lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
        let sel = PixelSelection {
            item_idx: 0,
            row: 0,
            col: 1,
            width: 2,
            height: 2,
            float_pixels: None,
        };
        let text = copy_selection(&doc, &lines, &sel).unwrap();
        assert_eq!(text, "@@..\n@@@@");
    }

    #[test]
    fn copy_floating_selection() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n......\n......";
        let lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
        let mut float = PixelGrid::new(2, 1);
        float.set(0, 0, PixelShape::new(pixel::PX_ALMOSTFULL, true));
        float.set(0, 1, PixelShape::EMPTY);
        let sel = PixelSelection {
            item_idx: 0,
            row: 0,
            col: 0,
            width: 2,
            height: 1,
            float_pixels: Some(float),
        };
        let text = copy_selection(&doc, &lines, &sel).unwrap();
        assert_eq!(text, "@@..");
    }

    #[test]
    fn delete_grounded_clears_pixels() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@@@@@\n@@@@@@";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };
        state.pixel_selection = Some(PixelSelection {
            item_idx: 0,
            row: 0,
            col: 0,
            width: 2,
            height: 1,
            float_pixels: None,
        });

        handle_delete_selection(&doc, &mut lines, &mut state);
        assert!(state.pixel_selection.is_none());

        let DocLine::Grid(grid) = &lines[1] else {
            panic!("expected grid");
        };
        assert!(grid.get(0, 0).is_empty());
        assert!(grid.get(0, 1).is_empty());
        assert!(grid.get(0, 2).is_filled());
        assert!(grid.get(1, 0).is_filled());
    }

    #[test]
    fn delete_floating_discards() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n......\n......";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut float = PixelGrid::new(2, 1);
        float.set(0, 0, PixelShape::new(pixel::PX_ALMOSTFULL, true));

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };
        state.pixel_selection = Some(PixelSelection {
            item_idx: 0,
            row: 1,
            col: 1,
            width: 2,
            height: 1,
            float_pixels: Some(float),
        });

        handle_delete_selection(&doc, &mut lines, &mut state);
        assert!(state.pixel_selection.is_none());

        // Grid should remain all empty — floating pixels were discarded, not merged
        let DocLine::Grid(grid) = &lines[1] else {
            panic!("expected grid");
        };
        for r in 0..2 {
            for c in 0..3 {
                assert!(grid.get(r, c).is_empty());
            }
        }
    }

    #[test]
    fn commit_floating_merges_overwrite() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@@@@@\n@@@@@@";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut float = PixelGrid::new(2, 1);
        float.set(0, 0, PixelShape::EMPTY);
        float.set(0, 1, PixelShape::EMPTY);

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };

        let sel = PixelSelection {
            item_idx: 0,
            row: 0,
            col: 0,
            width: 2,
            height: 1,
            float_pixels: Some(float),
        };

        commit_floating(&doc, &mut lines, &mut state, &sel);

        let DocLine::Grid(grid) = &lines[1] else {
            panic!("expected grid");
        };
        // Overwrite means the empty float pixels replace filled grid pixels
        assert!(grid.get(0, 0).is_empty());
        assert!(grid.get(0, 1).is_empty());
        assert!(grid.get(0, 2).is_filled());
    }

    #[test]
    fn mirror_h_entire_glyph() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@@@..\n..@@@@";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };

        let changed = handle_transform_selection(
            &doc, &mut lines, &mut state, SelectionTransform::MirrorH,
        );
        assert!(changed);

        let DocLine::Grid(grid) = &lines[1] else { panic!("expected grid") };
        assert!(grid.get(0, 0).is_empty());
        assert!(grid.get(0, 1).is_filled());
        assert!(grid.get(0, 2).is_filled());
        assert!(grid.get(1, 0).is_filled());
        assert!(grid.get(1, 1).is_filled());
        assert!(grid.get(1, 2).is_empty());
    }

    #[test]
    fn flip_v_entire_glyph() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@@@..\n......";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };

        handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::FlipV);

        let DocLine::Grid(grid) = &lines[1] else { panic!("expected grid") };
        assert!(grid.get(0, 0).is_empty());
        assert!(grid.get(0, 1).is_empty());
        assert!(grid.get(0, 2).is_empty());
        assert!(grid.get(1, 0).is_filled());
        assert!(grid.get(1, 1).is_filled());
        assert!(grid.get(1, 2).is_empty());
    }

    #[test]
    fn rotate_180_entire_glyph() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@....\n......";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };

        handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::Rotate180);

        let DocLine::Grid(grid) = &lines[1] else { panic!("expected grid") };
        assert!(grid.get(0, 0).is_empty());
        assert!(grid.get(0, 1).is_empty());
        assert!(grid.get(0, 2).is_empty());
        assert!(grid.get(1, 0).is_empty());
        assert!(grid.get(1, 1).is_empty());
        assert!(grid.get(1, 2).is_filled());
    }

    #[test]
    fn rotate_cw_blocked_on_non_square_glyph() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 3 2\n@@@@..\n......";
        let lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };

        assert!(!can_transform(&doc, &state, SelectionTransform::RotateCW));
        assert!(!can_transform(&doc, &state, SelectionTransform::RotateCCW));
        assert!(can_transform(&doc, &state, SelectionTransform::Rotate180));
        assert!(can_transform(&doc, &state, SelectionTransform::MirrorH));
    }

    #[test]
    fn transform_grounded_selection_becomes_floating() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 4 4\n@@@@....\n........\n........\n........";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };
        state.pixel_selection = Some(PixelSelection {
            item_idx: 0,
            row: 0,
            col: 0,
            width: 4,
            height: 1,
            float_pixels: None,
        });

        handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::MirrorH);

        let sel = state.pixel_selection.as_ref().unwrap();
        assert!(sel.is_floating());
        assert_eq!(sel.width, 4);
        assert_eq!(sel.height, 1);
        let float = sel.float_pixels.as_ref().unwrap();
        // Original: filled, filled, empty, empty → mirrored: empty, empty, filled, filled
        assert!(float.get(0, 0).is_empty());
        assert!(float.get(0, 1).is_empty());
        assert!(float.get(0, 2).is_filled());
        assert!(float.get(0, 3).is_filled());
    }

    #[test]
    fn rotate_cw_selection_changes_dimensions() {
        use crate::document_io::parse_doclines;
        let content = "glyph test 4 4\n@@@@@@..\n........\n........\n........";
        let mut lines = parse_doclines(content);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

        let mut state = EditorState::new();
        state.mode = EditMode::PixelSelect { item_idx: 0 };
        state.pixel_selection = Some(PixelSelection {
            item_idx: 0,
            row: 0,
            col: 0,
            width: 3,
            height: 1,
            float_pixels: None,
        });

        // 3x1 selection: fits when rotated (becomes 1x3, which fits in 4x4 grid)
        assert!(can_transform(&doc, &state, SelectionTransform::RotateCW));

        handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::RotateCW);

        let sel = state.pixel_selection.as_ref().unwrap();
        assert!(sel.is_floating());
        assert_eq!(sel.width, 1);
        assert_eq!(sel.height, 3);
    }

    // -----------------------------------------------------------------------
    // Scale adjustment tests
    // -----------------------------------------------------------------------

    #[test]
    fn round_half_to_even_cases() {
        assert_eq!(round_half_to_even(0.5), 0);
        assert_eq!(round_half_to_even(1.5), 2);
        assert_eq!(round_half_to_even(2.5), 2);
        assert_eq!(round_half_to_even(3.5), 4);
        assert_eq!(round_half_to_even(-0.5), 0);
        assert_eq!(round_half_to_even(-1.5), -2);
        assert_eq!(round_half_to_even(2.3), 2);
        assert_eq!(round_half_to_even(2.7), 3);
    }

    fn make_scale_test_doc(source: &str) -> (Document, Vec<DocLine>, EditorState) {
        let lines = crate::document_io::parse_doclines(source);
        let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
        let state = EditorState::new();
        (doc, lines, state)
    }

    #[test]
    fn adjust_scale_rescales_grid() {
        let source = "\
glyph foo 2 2
@@..
..@@
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        assert_eq!(can_adjust_scale(&doc, &lines, &state), Some(1));
        assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

        // Header should now have scale 2
        let header = lines[0].as_text().unwrap();
        assert!(header.contains("scale 2"), "header: {header}");

        // Grid should be 4x4
        let grid = lines[1].as_grid().unwrap();
        assert_eq!((grid.width, grid.height), (4, 4));
        // Top-left 2×2 block should be filled (was one filled pixel)
        assert!(grid.get(0, 0).is_filled());
        assert!(grid.get(0, 1).is_filled());
        assert!(grid.get(1, 0).is_filled());
        assert!(grid.get(1, 1).is_filled());
        // Top-right 2×2 block should be empty
        assert!(grid.get(0, 2).is_empty());
    }

    #[test]
    fn adjust_scale_updates_ref_offsets() {
        let source = "\
glyph foo 3 3
......
......
......
ref bar 2 4
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

        // ref line should have scaled offsets: 2*2/1=4, 4*2/1=8
        let ref_text = lines[2].as_text().unwrap();
        assert_eq!(ref_text.trim(), "ref bar 4 8");
    }

    #[test]
    fn adjust_scale_updates_anchor_positions() {
        let source = "\
glyph foo 4 4
........
........
........
........
anchor top 1 2
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 3));

        let anchor_text = lines[2].as_text().unwrap();
        // Single cell at scale 1 → 3-cell range at scale 3
        assert_eq!(anchor_text.trim(), "anchor top 3..5 6..8");
    }

    #[test]
    fn adjust_scale_noop_for_same_scale() {
        let source = "\
glyph foo 2 2 scale 2
@@@@
@@@@
@@@@
@@@@
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        assert_eq!(can_adjust_scale(&doc, &lines, &state), Some(2));
        assert!(!handle_adjust_scale(&doc, &mut lines, &mut state, 2));
    }

    #[test]
    fn adjust_scale_removes_scale_when_1() {
        let source = "\
glyph foo 2 2 scale 2
@@@@
@@@@
@@@@
@@@@
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 1));

        let header = lines[0].as_text().unwrap();
        assert!(!header.contains("scale"), "header: {header}");
        let grid = lines[1].as_grid().unwrap();
        assert_eq!((grid.width, grid.height), (2, 2));
    }

    #[test]
    fn adjust_scale_undo_restores_original() {
        let source = "\
glyph foo 2 2
@@..
..@@
ref bar 1 2
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        let original_lines = lines.clone();
        assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 3));

        // Verify it changed
        assert_ne!(lines, original_lines);

        // Undo
        state.undo.undo(&mut lines);
        assert_eq!(lines, original_lines);
    }

    #[test]
    fn adjust_scale_anchor_range() {
        let source = "\
glyph foo 4 4
........
........
........
........
anchor top 1..3 2..3
";
        let (doc, mut lines, mut state) = make_scale_test_doc(source);
        state.cursor = c(0, 0);

        assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

        let anchor_text = lines[2].as_text().unwrap();
        assert_eq!(anchor_text.trim(), "anchor top 2..7 4..7");
    }
}
