use crate::document::{DocLine, Document, DocumentItem, PixelGrid};
use crate::editor::undo::{self, PixelSelectionSnapshot};
use crate::editor::{EditMode, EditorState};
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
// Drag state stored in egui temp data
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum SelectDrag {
    New { anchor_row: i16, anchor_col: i16 },
    Move { accum: egui::Vec2 },
}

fn drag_id() -> egui::Id {
    egui::Id::new("pixel_select_drag")
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
    grid_x: f32,
    grid_y: f32,
    grid_cell: f32,
) {
    if !matches!(state.mode, EditMode::PixelSelect { item_idx: eidx } if eidx == item_idx) {
        return;
    }

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
        ui.data_mut(|d| d.remove::<SelectDrag>(drag_id()));
        return;
    }

    let Some(hp) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };

    // Only process if pointer is on this row
    if hp.y < grid_y || hp.y >= grid_y + grid_cell {
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
                d.insert_temp(drag_id(), SelectDrag::Move { accum: egui::Vec2::ZERO })
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
                    drag_id(),
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
    let drag_state: Option<SelectDrag> = ui.data(|d| d.get_temp(drag_id()));
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
                ui.data_mut(|d| d.insert_temp(drag_id(), SelectDrag::Move { accum }));
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
                ui.data_mut(|d| d.insert_temp(drag_id(), SelectDrag::Move { accum }));
                return;
            }

            accum.x -= actual_dcol as f32 * grid_cell;
            accum.y -= actual_drow as f32 * grid_cell;
            ui.data_mut(|d| d.insert_temp(drag_id(), SelectDrag::Move { accum }));

            let before_snap = sel.to_snapshot();
            let mode_before = state.mode.clone();

            // First move: extract pixels from grid
            let mut pixel_changes = Vec::new();
            if !sel.is_floating() {
                let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) else {
                    return;
                };
                let mut extracted = PixelGrid::new(sel.width, sel.height);
                for r in 0..sel.height {
                    for c in 0..sel.width {
                        let gr = sel.row + r as i16;
                        let gc = sel.col + c as i16;
                        if gr >= 0
                            && gr < grid_height as i16
                            && gc >= 0
                            && gc < grid_width as i16
                        {
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
                // Apply extraction to grid
                if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_doc_line) {
                    for ch in &pixel_changes {
                        grid.set(ch.row, ch.col, ch.new);
                    }
                }

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
    let item_idx = match &state.mode {
        EditMode::GlyphEdit { item_idx, .. } | EditMode::PixelSelect { item_idx } => *item_idx,
        _ => return false,
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
}

pub(crate) fn can_transform(
    doc: &Document,
    state: &EditorState,
    transform: SelectionTransform,
) -> bool {
    let item_idx = match &state.mode {
        EditMode::GlyphEdit { item_idx, .. } | EditMode::PixelSelect { item_idx } => *item_idx,
        _ => return false,
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
    let item_idx = match &state.mode {
        EditMode::GlyphEdit { item_idx, .. } | EditMode::PixelSelect { item_idx } => *item_idx,
        _ => return false,
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
            let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line) else {
                return false;
            };
            let mut extracted = PixelGrid::new(sel.width, sel.height);
            for r in 0..sel.height {
                for c in 0..sel.width {
                    let gr = sel.row + r as i16;
                    let gc = sel.col + c as i16;
                    if gr >= 0
                        && gr < grid_height as i16
                        && gc >= 0
                        && gc < grid_width as i16
                    {
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
                for ch in &pixel_changes {
                    grid.set(ch.row, ch.col, ch.new);
                }
            }
            extracted
        };

        let transformed = match transform {
            SelectionTransform::MirrorH => source_grid.mirror_h(),
            SelectionTransform::FlipV => source_grid.flip_v(),
            SelectionTransform::RotateCW => source_grid.rotate_cw(),
            SelectionTransform::RotateCCW => source_grid.rotate_ccw(),
            SelectionTransform::Rotate180 => source_grid.rotate_180(),
            SelectionTransform::Opposite => source_grid.opposite(),
        };

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

        let transformed = match transform {
            SelectionTransform::MirrorH => old_grid.mirror_h(),
            SelectionTransform::FlipV => old_grid.flip_v(),
            SelectionTransform::RotateCW => old_grid.rotate_cw(),
            SelectionTransform::RotateCCW => old_grid.rotate_ccw(),
            SelectionTransform::Rotate180 => old_grid.rotate_180(),
            SelectionTransform::Opposite => old_grid.opposite(),
        };

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
}
