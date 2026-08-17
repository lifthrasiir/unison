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
                // An IDC line is deferred for the same reason as a `ref`: it is
                // the whole shape of the glyph, and half a component name
                // resolves to nothing.
                Some(DocLine::Text(t)) if t.trim_start().starts_with("ref ")
                    || t.trim_start().starts_with("anchor ")
                    || t.split_whitespace().next().and_then(crate::compose::IdcOp::from_token).is_some()
            );
            let on_glyph_header = matches!(
                lines.get(state.cursor.line),
                Some(DocLine::Text(t)) if crate::editor::reconcile::parse_glyph_header_dims(t).is_some()
            );
            // A heading is a header too: its fold group must not come and go
            // under the caret as the level is retyped, so it settles when the
            // caret leaves exactly as a `glyph` line's does. See
            // `folding::settle_edited_header`.
            let on_heading = matches!(
                lines.get(state.cursor.line),
                Some(DocLine::Text(t)) if crate::document_io::split_heading(t.trim()).is_some()
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
                && (on_ref_line || on_glyph_header || on_heading || owns_grid);

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

/// How many lines the buffer would occupy in the file. A grid is one
/// `DocLine` but as many source lines as it has rows — and every one of those
/// rows carries a line number — so this is the largest number the gutter can
/// ever be asked to draw.
pub(super) fn source_line_count(lines: &[DocLine]) -> usize {
    lines
        .iter()
        .map(|line| match line {
            DocLine::Text(_) => 1,
            DocLine::Grid(g) if g.is_all_empty() => 0,
            DocLine::Grid(g) => g.height as usize,
        })
        .sum()
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

/// Run the inline command the subglyph menu chose on ref `ref_idx` of the
/// glyph being edited.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_inline_action(
    action: crate::editor::inline_tools::InlineAction,
    lines: &mut Vec<DocLine>,
    doc: &Document,
    state: &mut EditorState,
    edit_idx: usize,
    ref_idx: usize,
    composite: Option<&ref_composite::GlyphComposite>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    use crate::editor::inline_tools::InlineAction;
    match action {
        InlineAction::Once => inline_ref_once(
            lines,
            doc,
            state,
            edit_idx,
            ref_idx,
            composite,
            named_glyphs,
            name_parts,
        ),
        InlineAction::ToPixels => inline_ref_to_pixels(
            lines,
            doc,
            state,
            edit_idx,
            ref_idx,
            named_glyphs,
            name_parts,
        ),
    }
}

/// Ink to merge into the edited glyph's own pixel grid, already snapped to the
/// shape catalog and in that glyph's subcell units.
struct MergeGrid {
    grid: PixelGrid,
    row: i32,
    col: i32,
    negated: bool,
}

/// Replace a `ref` with the target's own declaration — its refs, rebased onto
/// where the ref sat, and the pixels it draws itself.
///
/// This is one step of what [`inline_ref_to_pixels`] does all the way down: a
/// target that is itself a composite stays composed, one level nearer. The
/// offsets come from the resolved composite where there is one, so a ref that
/// states no offset at all (an anchor-positioned one) inlines where it is
/// drawn rather than at `(0, 0)`.
///
/// A target with no refs has no declaration to expand — it *is* its pixels —
/// so there this falls back to flattening. Flags that do not survive the move
/// (a `negated` target whose own refs subtract, `inherit` on a ref whose
/// anchors now belong to another glyph) are combined with the ref's own as
/// best they can be; the composition is only exactly preserved for the common
/// case of a plain additive target.
#[allow(clippy::too_many_arguments)]
pub(super) fn inline_ref_once(
    lines: &mut Vec<DocLine>,
    doc: &Document,
    state: &mut EditorState,
    edit_idx: usize,
    ref_idx: usize,
    composite: Option<&ref_composite::GlyphComposite>,
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
    let gref = &body.refs[ref_idx];
    // The layer as the composite placed it: the alternative that was actually
    // chosen, and the offset an anchor-positioned ref has no `offset` to state.
    let layer = composite.and_then(|c| c.layers.iter().find(|l| l.ref_idx == ref_idx));
    let target_name = layer.map_or(gref.name.as_str(), |l| l.resolved_name.as_str());
    let resolved =
        match ref_composite::resolve_ref_name_for_view(target_name, named_glyphs, name_parts) {
            Some(r) => r,
            None => return false,
        };
    let Some(source) = resolved.inline_source.clone() else {
        return inline_ref_to_pixels(
            lines,
            doc,
            state,
            edit_idx,
            ref_idx,
            named_glyphs,
            name_parts,
        );
    };

    let parent_scale = body.scale.max(1);
    let ref_scale = resolved.scale.max(1);
    let (ps, rs) = (parent_scale as i32, ref_scale as i32);
    // Where the target's own origin sits in this glyph's coordinates. Its
    // refs and its pixels are both stated from there.
    let base_row = layer.map_or(gref.row(), |l| l.logical_offset_row) as i32;
    let base_col = layer.map_or(gref.col(), |l| l.logical_offset_col) as i32;
    // The target states its refs in *its* subcells; this glyph's grid counts
    // in its own. A finer target (`scale 4` inlined into a `scale 2` glyph)
    // has positions this glyph's lattice cannot state, so the offset lands on
    // the nearest cell it can — the only lossy part of inlining, and only
    // between glyphs of different `scale`.
    let rebase = |v: i16| div_round(v as i32 * ps, rs);

    let replacement: Vec<String> = source
        .refs
        .iter()
        .map(|sub| {
            let mut inlined = sub.clone();
            // An `@…` name stands for the enclosing base glyph, which from
            // here on is a different one; the resolved name is what it meant.
            inlined.raw_name = None;
            // Both flags describe a relation to the glyph the ref was written
            // in, and the inlined ref now sits in this one.
            inlined.negated = sub.negated != gref.negated;
            inlined.inherit = sub.inherit && gref.inherit;
            let col = clamp_offset(base_col + rebase(sub.col()));
            let row = clamp_offset(base_row + rebase(sub.row()));
            inlined.format_line(Some((col, row)))
        })
        .collect();

    // The pixels the target draws itself sit at its logical origin; everything
    // else it is made of stays a ref.
    let merge = source
        .pixels
        .as_ref()
        .filter(|g| !g.is_all_empty())
        .map(|g| {
            let mut grid = if ref_scale == parent_scale {
                g.clone()
            } else {
                g.rescale(ref_scale, parent_scale)
            };
            // Written into the file, so exact regions have to land back on the
            // shape catalog first — see [`inline_ref_to_pixels`].
            grid.snap_details_to_catalog();
            MergeGrid {
                grid,
                row: base_row,
                col: base_col,
                negated: gref.negated,
            }
        });

    apply_inline(
        lines,
        doc,
        state,
        edit_idx,
        ref_idx,
        merge,
        replacement,
        named_glyphs,
        name_parts,
    )
}

fn clamp_offset(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// `num / den` rounded to the nearest integer, halves away from zero. Plain
/// integer division truncates *towards* zero, which would move a negative
/// offset (a bearing) right while moving its positive mirror left.
fn div_round(num: i32, den: i32) -> i32 {
    debug_assert!(den > 0);
    if num >= 0 {
        (num + den / 2) / den
    } else {
        -((-num + den / 2) / den)
    }
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

    let gref = &body.refs[ref_idx];
    let resolved =
        match ref_composite::resolve_ref_name_for_view(&gref.name, named_glyphs, name_parts) {
            Some(r) => r,
            None => return false,
        };

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
    let merge = MergeGrid {
        grid: scaled_ref_grid,
        row: gref.row() as i32 + resolved.origin_row * ps / rs,
        col: gref.col() as i32 + resolved.origin_col * ps / rs,
        negated: gref.negated,
    };

    apply_inline(
        lines,
        doc,
        state,
        edit_idx,
        ref_idx,
        Some(merge),
        Vec::new(),
        named_glyphs,
        name_parts,
    )
}

/// The line surgery both inline commands share: merge `merge` into the glyph's
/// own grid (creating one when there is ink to put there and no grid yet), then
/// put `replacement` where the `ref` line was.
///
/// Everything from the header down is one undo entry: creating a grid rewrites
/// the header, and the ref's own line may be anywhere among the layer lines.
#[allow(clippy::too_many_arguments)]
fn apply_inline(
    lines: &mut Vec<DocLine>,
    doc: &Document,
    state: &mut EditorState,
    edit_idx: usize,
    ref_idx: usize,
    merge: Option<MergeGrid>,
    replacement: Vec<String>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    let body = match doc.items.get(edit_idx) {
        Some(DocumentItem::Glyph { body, .. }) => body,
        _ => return false,
    };
    let item_start = doc.item_line_starts[edit_idx];
    let grid_line_idx = item_start + 1;
    let has_grid = body.pixels.is_some();
    let old_len = 1 + usize::from(has_grid) + body.refs.len() + body.points.len();
    if item_start + old_len > lines.len() {
        return false;
    }
    let old_lines: Vec<DocLine> = lines[item_start..item_start + old_len].to_vec();

    // Scanned, not computed from `ref_idx`: an `anchor` line may sit between
    // the ref lines (see `layer_doc_line`). Scanned before a grid is inserted,
    // while `lines` still matches `body.pixels`.
    let mut ref_text_line_idx = pixel_interaction::layer_doc_line(lines, body, item_start, ref_idx);

    let mut inserted_grid = 0usize;
    if merge.is_some() && !has_grid {
        let header_text = match &lines[item_start] {
            DocLine::Text(s) => s.clone(),
            _ => return false,
        };
        let tokens = match document_io::tokenize_tokens(&header_text) {
            Ok(t) => t,
            Err(_) => return false,
        };
        let dims = parse_glyph_header_dims(&tokens);
        let (w, h) = dims.unwrap_or_else(|| {
            let (_min_r, _min_c, max_r, max_c) = ref_composite::composite_bounds(
                None,
                &body.refs,
                named_glyphs,
                name_parts,
                body.scale,
            );
            ((max_c).max(0) as u16, (max_r).max(0) as u16)
        });
        if w == 0 || h == 0 {
            return false;
        }
        if dims.is_none() {
            let new_header = document_io::append_to_line(&header_text, &format!("{w} {h}"));
            lines[item_start] = DocLine::Text(new_header);
        }
        lines.insert(grid_line_idx, DocLine::Grid(PixelGrid::new(w, h)));
        ref_text_line_idx += 1;
        inserted_grid = 1;
    }

    if let Some(merge) = &merge
        && let Some(DocLine::Grid(grid)) = lines.get_mut(grid_line_idx)
    {
        merge_ref_pixels(grid, &merge.grid, merge.row, merge.col, merge.negated);
    }

    lines.remove(ref_text_line_idx);
    let replacement_len = replacement.len();
    for (i, text) in replacement.into_iter().enumerate() {
        lines.insert(ref_text_line_idx + i, DocLine::Text(text));
    }

    let new_len = old_len + inserted_grid - 1 + replacement_len;
    let new_lines: Vec<DocLine> = lines[item_start..item_start + new_len].to_vec();

    let caret = state.cursor;
    state.undo.break_coalesce();
    state
        .undo
        .push_lines(item_start, old_lines, new_lines, caret, caret);

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
            if shape.is_clear() {
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
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    ctx: &egui::Context,
) -> bool {
    state.apply_edit_action(action, doc, lines, ctx)
}
