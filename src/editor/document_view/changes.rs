//! Applying editor changes back to the document, and re-deriving what a
//! change invalidated.

use super::*;

/// Applies this frame's document changes: the pixel-only fast path, an
/// immediate flush, or a deferred reparse while the caret sits on a line
/// whose transient state must not be reconciled yet.
pub(super) fn apply_pending_rederive(
    doc: &mut Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: bool,
) {
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
                Some(DocLine::Text(t)) if t.trim_start().starts_with("ref ") || t.trim_start().starts_with("anchor ")
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

            // A `ref` line is deferred from its *first* keystroke, exactly like
            // a header: a half-typed glyph name resolves to nothing, and for a
            // ref-only glyph the composite is the only grid there is — reparsing
            // mid-edit collapsed it to bare text rows. Waiting for the caret to
            // leave keeps the last resolved shape on screen; it used to reparse
            // once and then defer, so the collapse also outlived typing the name
            // back in full.
            let defer = structure_stable
                && matches!(state.mode, EditMode::Normal)
                && (on_ref_line || on_glyph_header || owns_grid);

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
            // Deferring is only ever chosen in `Normal` mode; entering a pixel
            // mode (e.g. clicking straight into the grid from the header line)
            // ends the text edit just as much as moving the caret away does.
            let should_flush = !state.active
                || pend_line != state.cursor.line
                || !matches!(state.mode, EditMode::Normal);
            if should_flush {
                flush_document_changes(lines, doc, state);
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

pub(super) fn line_to_item_idx(item_line_starts: &[usize], target_line: usize) -> Option<usize> {
    item_line_starts
        .iter()
        .rposition(|&start| start <= target_line)
}

pub(super) fn defer_document_changes(doc: &mut Document, state: &mut EditorState) {
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

pub(super) fn rederive(lines: &[DocLine], doc: &mut Document, is_at_saved: bool) {
    match crate::document_io::derive_document(lines, doc.path.clone()) {
        Ok((new_doc, _)) => {
            let items_changed = !doc
                .items
                .iter()
                .filter(|i| i.affects_font())
                .eq(new_doc.items.iter().filter(|i| i.affects_font()));
            let next_gen = doc.edit_gen + 1;
            let pixel_gen = doc.pixel_gen;
            let content_gen = if items_changed {
                doc.content_gen + 1
            } else {
                doc.content_gen
            };
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

    if let Some(DocLine::Grid(grid)) = lines.get(grid_doc_line)
        && let Some(DocumentItem::Glyph { body, .. }) = doc.items.get_mut(item_idx)
    {
        body.pixels = Some(grid.clone());
    }
    doc.docline_file_lines = crate::document::compute_docline_file_lines(lines);
    doc.pixel_gen += 1;
    doc.content_gen += 1;
    doc.dirty = !state.undo.is_at_saved();

    state.pending_reparse_line = None;
    state.last_reparse_line = Some(state.cursor.line);
    state.clear_document_sync_request();
}

pub(super) fn inline_ref_to_pixels(
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
    let resolved =
        match ref_composite::resolve_ref_name_for_view(&gref.name, named_glyphs, name_parts) {
            Some(r) => r,
            None => return false,
        };

    let item_start = doc.item_line_starts[edit_idx];
    let grid_line_idx = item_start + 1;

    let parent_scale = body.scale;
    let ref_scale = resolved.scale.max(1);
    // Flattening writes the ref's pixels into the file, so its exact regions
    // have to land back on the shape catalog first — `merge_ref_pixels` works
    // on shape codes and would read a `PX_CUSTOM` cell as empty.
    let mut scaled_ref_grid = if ref_scale == parent_scale {
        resolved.grid.clone()
    } else {
        resolved.grid.rescale(ref_scale, parent_scale)
    };
    scaled_ref_grid.snap_details_to_catalog();
    let ps = parent_scale as i32;
    let rs = ref_scale as i32;
    let eff_row = gref.row() as i32 + resolved.origin_row * ps / rs;
    let eff_col = gref.col() as i32 + resolved.origin_col * ps / rs;
    let negated = gref.negated;

    if has_grid {
        let body_line_count = 1 + body.refs.len() + body.points.len();
        let old_lines: Vec<DocLine> =
            lines[grid_line_idx..grid_line_idx + body_line_count].to_vec();

        if let Some(DocLine::Grid(grid)) = lines.get_mut(grid_line_idx) {
            merge_ref_pixels(grid, &scaled_ref_grid, eff_row, eff_col, negated);
        }

        // Scanned, not computed from `ref_idx`: an `anchor` line may sit
        // between the ref lines (see `layer_doc_line`).
        let ref_text_line_idx = pixel_interaction::layer_doc_line(lines, body, item_start, ref_idx);
        lines.remove(ref_text_line_idx);

        let new_lines: Vec<DocLine> =
            lines[grid_line_idx..grid_line_idx + body_line_count - 1].to_vec();

        let caret = state.cursor;
        state.undo.break_coalesce();
        state
            .undo
            .push_lines(grid_line_idx, old_lines, new_lines, caret, caret);
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
            let (_min_r, _min_c, max_r, max_c) = ref_composite::composite_bounds(
                None,
                &body.refs,
                named_glyphs,
                name_parts,
                body.scale,
            );
            let w = (max_c).max(0) as u16;
            let h = (max_r).max(0) as u16;
            (w, h)
        });
        if w == 0 || h == 0 {
            return false;
        }

        let body_line_count = body.refs.len() + body.points.len();
        let undo_start = if has_dims { grid_line_idx } else { item_start };
        let old_line_count = if has_dims {
            body_line_count
        } else {
            1 + body_line_count
        };
        let old_lines: Vec<DocLine> = lines[undo_start..undo_start + old_line_count].to_vec();

        if !has_dims {
            let new_header = document_io::append_to_line(&header_text, &format!("{w} {h}"));
            lines[item_start] = DocLine::Text(new_header);
        }
        // Scanned before the grid is inserted (`body.pixels` is still `None`
        // here, matching `lines`), then shifted past the insertion.
        let ref_text_line_idx = pixel_interaction::layer_doc_line(lines, body, item_start, ref_idx);
        let mut grid = PixelGrid::new(w, h);
        merge_ref_pixels(&mut grid, &scaled_ref_grid, eff_row, eff_col, negated);
        lines.insert(grid_line_idx, DocLine::Grid(grid));

        lines.remove(ref_text_line_idx + 1);

        let new_line_count = if has_dims {
            1 + body.refs.len() - 1 + body.points.len()
        } else {
            1 + 1 + body.refs.len() - 1 + body.points.len()
        };
        let new_lines: Vec<DocLine> = lines[undo_start..undo_start + new_line_count].to_vec();

        let caret = state.cursor;
        state.undo.break_coalesce();
        state
            .undo
            .push_lines(undo_start, old_lines, new_lines, caret, caret);
    }

    state.mode = EditMode::GlyphEdit {
        item_idx: edit_idx,
        selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
    };

    true
}

fn merge_ref_pixels(
    grid: &mut PixelGrid,
    ref_grid: &PixelGrid,
    eff_row: i32,
    eff_col: i32,
    negated: bool,
) {
    for r in 0..ref_grid.height as i32 {
        for c in 0..ref_grid.width as i32 {
            let shape = ref_grid.get(r as u16, c as u16);
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
    let dims = crate::document_io::glyph_header_dims(&tokens[1..])?;
    Some((dims.width, dims.height))
}

pub fn apply_edit_action_to_editor(
    action: crate::edit_menu::EditAction,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    ctx: &egui::Context,
) -> bool {
    state.apply_edit_action(action, lines, ctx)
}
