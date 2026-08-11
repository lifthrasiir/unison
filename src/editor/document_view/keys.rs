//! Keyboard handling for the document view.

use super::*;
use crate::editor::glyph_resize;

/// All keyboard input for the focused document: autocomplete keys first,
/// then mode switches, undo/redo, pixel-selection clipboard/transforms, and
/// finally plain text editing.  `needs_rederive` accumulates whether any of
/// it changed the document.
#[expect(clippy::too_many_arguments)]
pub(super) fn handle_document_keys(
    ui: &egui::Ui,
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &crate::editor::ref_composite::AlternativesIndex,
    composites: &HashMap<usize, GlyphComposite>,
    prev_cursor: Caret,
    needs_rederive: &mut bool,
) {
    if state.active {
        // Autocomplete key handling takes priority
        let ac_result = crate::editor::autocomplete::handle_keys(ui, lines, state);
        if matches!(
            ac_result,
            crate::editor::autocomplete::HandleResult::TextChanged
        ) {
            *needs_rederive = true;
        }

        if matches!(
            ac_result,
            crate::editor::autocomplete::HandleResult::NotConsumed
        ) {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                if !matches!(state.popup, PopupState::None) {
                    state.popup = PopupState::None;
                    state.preedit.clear();
                } else if matches!(state.mode, EditMode::GlyphResize { .. }) {
                    // Not a mode switch: the previewed resize has to be taken
                    // back out of the document too.
                    *needs_rederive |= glyph_resize::cancel(lines, state);
                } else if !matches!(state.mode, EditMode::Normal) {
                    state.mode = EditMode::Normal;
                }
            }

            *needs_rederive |= handle_resize_keys(
                ui,
                doc,
                lines,
                state,
                crate::editor::glyph_resize::ResolveEnv {
                    named_glyphs,
                    name_parts,
                    alt_index,
                },
            );

            // F2: rename symbol at caret
            if matches!(state.mode, EditMode::Normal)
                && matches!(state.popup, PopupState::None)
                && ui.input(|i| i.key_pressed(egui::Key::F2))
                && let Some(DocLine::Text(line_text)) = lines.get(state.cursor.line)
                && let Some(target) = doc_links::find_renameable_at_caret(
                    line_text,
                    state.cursor.col,
                    crate::document::at_base_at_line(lines, state.cursor.line).as_deref(),
                )
            {
                state.popup = PopupState::Rename {
                    original_name: target.name.clone(),
                    new_name: target.name,
                    kind: target.kind,
                    focus_set: false,
                };
            }

            // Ctrl+K: type a character by its code point. Ctrl, not Alt: on
            // macOS an Option+letter chord is a dead key that never reaches
            // the app at all. See `crate::editor::codepoint_popup`.
            // Exclude Cmd+K via `mac_cmd`, never via `command` — off the Mac
            // `command` mirrors `ctrl`, so testing it rejects every Ctrl+K.
            if matches!(state.mode, EditMode::Normal)
                && matches!(state.popup, PopupState::None)
                && ui.input(|i| {
                    i.modifiers.ctrl
                        && !i.modifiers.mac_cmd
                        && !i.modifiers.alt
                        && i.key_pressed(egui::Key::K)
                })
            {
                state.start_codepoint_entry();
            }

            // Undo/redo in GlyphEdit/LayerMove modes (Normal mode handles it via doc_input::handle_keys).
            // Not while resizing: the preview is uncommitted text no undo entry
            // describes, so stepping the stack under it would mix the two.
            if !matches!(state.mode, EditMode::Normal | EditMode::GlyphResize { .. })
                && matches!(state.popup, PopupState::None)
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
                        *needs_rederive = true;
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
                        *needs_rederive = true;
                    }
                }
            }

            // Backtick / 1..9: pick a layer palette slot outright. A resize
            // owns the glyph until it is applied or cancelled, so no shortcut
            // may switch the mode out from under it.
            if !matches!(state.mode, EditMode::GlyphResize { .. }) {
                handle_palette_shortcuts(ui, doc, composites, state);
            }

            // Select-all / clipboard for the pixel grid, in *both* pixel modes:
            // the grid is what the user is working on in either, and with
            // nothing framed all of it is the target
            // (`pixel_selection::effective_selection`).
            if matches!(
                state.mode,
                EditMode::GlyphEdit { .. } | EditMode::PixelSelect { .. }
            ) {
                // Ctrl/Cmd+A: frame the whole grid. Nothing else claims it
                // here — `doc_input::handle_keys`, which selects the document
                // text, only runs in Normal mode.
                if ui.input(|i| {
                    i.modifiers.command
                        && !i.modifiers.shift
                        && !i.modifiers.alt
                        && i.key_pressed(egui::Key::A)
                }) {
                    pixel_selection::select_all(doc, lines, state);
                }

                // Delete/Backspace: delete selection
                if ui.input(|i| {
                    (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                        && !i.modifiers.command
                }) {
                    pixel_selection::handle_delete_selection(doc, lines, state);
                    *needs_rederive = true;
                }

                // Copy/Cut/Paste via egui events
                let mut sel_clipboard_out: Option<String> = None;
                let mut sel_do_cut = false;
                let mut sel_paste_text: Option<String> = None;
                let effective = pixel_selection::effective_selection(doc, state);
                ui.input(|input| {
                    for event in &input.events {
                        match event {
                            egui::Event::Copy => {
                                if let Some(sel) = &effective {
                                    sel_clipboard_out =
                                        pixel_selection::copy_selection(doc, lines, sel);
                                }
                            }
                            egui::Event::Cut => {
                                if let Some(sel) = &effective {
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
                    *needs_rederive = true;
                }
                if let Some(text) = sel_paste_text
                    && pixel_selection::paste_selection(doc, lines, state, &text)
                {
                    *needs_rederive = true;
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
                if let Some(t) = transform
                    && pixel_selection::can_transform(doc, state, t)
                    && pixel_selection::handle_transform_selection(doc, lines, state, t)
                {
                    *needs_rederive = true;
                }
            }

            // Subpixel shape shortcuts in GlyphEdit mode
            if let EditMode::GlyphEdit { selected_shape, .. } = &mut state.mode {
                handle_shape_shortcuts(ui, selected_shape);
                // The shortcuts name absolute orientations, so the palette
                // follows them rather than the other way round.
                crate::editor::glyph_widget::sync_rotation(
                    *selected_shape,
                    &mut state.shape_rotation,
                );
            }

            if matches!(state.mode, EditMode::Normal) && matches!(state.popup, PopupState::None) {
                let text_changed = doc_input::handle_keys(ui, lines, state);
                *needs_rederive |= text_changed;

                // Update autocomplete candidates after text changes
                if text_changed && state.autocomplete.is_some() {
                    crate::editor::autocomplete::update_after_edit(lines, state);
                }
            }
        }

        // Ctrl+J (or Cmd+Period on macOS) to trigger autocomplete. The same
        // Ctrl+J walks the list once the popup is open — opening it *is* the
        // first step down — so the popup's own handler claims the key first
        // and this only ever sees the press that opens it.
        // Exclude Cmd+J via `mac_cmd`, never via `command`: off the Mac
        // `command` mirrors `ctrl`, which would reject every Ctrl+J.
        let trigger_ac = ui.input(|i| {
            let ctrl_j = i.modifiers.ctrl
                && !i.modifiers.mac_cmd
                && !i.modifiers.alt
                && i.key_pressed(egui::Key::J);
            let cmd_period = cfg!(target_os = "macos")
                && i.modifiers.command
                && i.key_pressed(egui::Key::Period);
            ctrl_j || cmd_period
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
            && (state.cursor.line != ac.line || state.cursor.col < ac.replace_start)
        {
            state.autocomplete = None;
        }
        // Also re-filter if cursor moved within the token but no text changed
        if state.autocomplete.is_some() && state.cursor != prev_cursor && !*needs_rederive {
            crate::editor::autocomplete::update_after_edit(lines, state);
        }
    }
}

/// `` ` `` and `1`..`9` select a slot of the layer palette (the inline tools
/// preview row) directly, whatever the current mode is: slot 1 is the glyph's
/// own pixel grid, slots 2.. are its subglyph layers in palette order (refs,
/// then points, then inherited anchors).  `1` and `` ` `` both switch *to* the
/// pixel grid and only differ in which detail mode they land in, so neither
/// depends on a pixel grid being selected already.
///
/// A slot with no layer behind it is a no-op — the mode is left alone rather
/// than clamped onto the last layer, since a mistyped digit should not move
/// the selection somewhere unrelated.
fn handle_palette_shortcuts(
    ui: &egui::Ui,
    doc: &Document,
    composites: &HashMap<usize, GlyphComposite>,
    state: &mut EditorState,
) {
    const DIGITS: [egui::Key; 9] = [
        egui::Key::Num1,
        egui::Key::Num2,
        egui::Key::Num3,
        egui::Key::Num4,
        egui::Key::Num5,
        egui::Key::Num6,
        egui::Key::Num7,
        egui::Key::Num8,
        egui::Key::Num9,
    ];

    let Some(item_idx) = state.mode.edit_item_idx() else {
        return;
    };

    // `` ` `` wins if both arrive in the same frame; it is the only key that
    // asks for PixelSelect.
    let to_pixel_select = ui
        .input(|i| i.key_pressed(egui::Key::Backtick) && !i.modifiers.command && !i.modifiers.alt);
    if to_pixel_select {
        // Reconciliation will commit any floating selection.
        state.mode = EditMode::PixelSelect { item_idx };
        return;
    }

    let slot = ui.input(|i| {
        if i.modifiers.command || i.modifiers.alt {
            return None;
        }
        DIGITS.iter().position(|&k| i.key_pressed(k))
    });
    let Some(slot) = slot else { return };

    if slot == 0 {
        if !matches!(state.mode, EditMode::GlyphEdit { .. }) {
            state.mode = EditMode::GlyphEdit {
                item_idx,
                selected_shape: pixel::PixelShape::new(pixel::PX_ALMOSTFULL, true),
            };
        }
        return;
    }

    let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx) else {
        return;
    };
    let inherited = composites
        .get(&item_idx)
        .map_or(0, |c| c.inherited_anchors.len());
    let layer_count = body.refs.len() + body.points.len() + inherited;
    let layer_idx = slot - 1;
    if layer_idx < layer_count {
        state.mode = EditMode::LayerMove {
            item_idx,
            layer_idx,
        };
    }
}

fn handle_shape_shortcuts(ui: &egui::Ui, selected_shape: &mut pixel::PixelShape) {
    use pixel::*;

    // (key, cycle of shapes) — cycle length 1..=3
    const MAPPINGS: &[(egui::Key, &[PixelShape])] = &[
        (egui::Key::Num1, &[PixelShape(PX_ALMOSTFULL | PX_FULL)]),
        // asdf: halves → halfslant H (w2:h1, 3/4) → halfslant V (w1:h2, 3/4)
        (
            egui::Key::F,
            &[
                PixelShape(PX_HALF1 | PX_FULL),
                PixelShape(PX_HALFSLANT1H | PX_FULL),
                PixelShape(PX_HALFSLANT1V | PX_FULL),
            ],
        ),
        (
            egui::Key::S,
            &[
                PixelShape(PX_HALF2 | PX_FULL),
                PixelShape(PX_HALFSLANT2H | PX_FULL),
                PixelShape(PX_HALFSLANT2V | PX_FULL),
            ],
        ),
        (
            egui::Key::A,
            &[
                PixelShape(PX_HALF3 | PX_FULL),
                PixelShape(PX_HALFSLANT3H | PX_FULL),
                PixelShape(PX_HALFSLANT3V | PX_FULL),
            ],
        ),
        (
            egui::Key::D,
            &[
                PixelShape(PX_HALF4 | PX_FULL),
                PixelShape(PX_HALFSLANT4H | PX_FULL),
                PixelShape(PX_HALFSLANT4V | PX_FULL),
            ],
        ),
        // qwer: quad → cone
        (
            egui::Key::R,
            &[
                PixelShape(PX_QUAD1 | PX_FULL),
                PixelShape(PX_CONE1 | PX_FULL),
            ],
        ),
        (
            egui::Key::Q,
            &[
                PixelShape(PX_QUAD2 | PX_FULL),
                PixelShape(PX_CONE2 | PX_FULL),
            ],
        ),
        (
            egui::Key::W,
            &[
                PixelShape(PX_QUAD3 | PX_FULL),
                PixelShape(PX_CONE3 | PX_FULL),
            ],
        ),
        (
            egui::Key::E,
            &[
                PixelShape(PX_QUAD4 | PX_FULL),
                PixelShape(PX_CONE4 | PX_FULL),
            ],
        ),
        // zxcv: invquad → invcone
        (
            egui::Key::V,
            &[
                PixelShape(PX_INVQUAD1 | PX_FULL),
                PixelShape(PX_INVCONE1 | PX_FULL),
            ],
        ),
        (
            egui::Key::Z,
            &[
                PixelShape(PX_INVQUAD2 | PX_FULL),
                PixelShape(PX_INVCONE2 | PX_FULL),
            ],
        ),
        (
            egui::Key::X,
            &[
                PixelShape(PX_INVQUAD3 | PX_FULL),
                PixelShape(PX_INVCONE3 | PX_FULL),
            ],
        ),
        (
            egui::Key::C,
            &[
                PixelShape(PX_INVQUAD4 | PX_FULL),
                PixelShape(PX_INVCONE4 | PX_FULL),
            ],
        ),
    ];

    for &(key, cycle) in MAPPINGS {
        if ui.input(|i| i.key_pressed(key) && !i.modifiers.command && !i.modifiers.alt) {
            if cycle.len() == 1 {
                *selected_shape = cycle[0];
            } else {
                let cur_pos = cycle.iter().position(|s| {
                    *s == *selected_shape
                        || (s.is_slant_pair() && *selected_shape == s.slant_direction_pair())
                });
                *selected_shape = match cur_pos {
                    Some(i) => cycle[(i + 1) % cycle.len()],
                    None => cycle[0],
                };
            }
        }
    }
}

/// Everything the glyph-resize mode listens to: `F2` to enter it, the arrow
/// keys to move one boundary, `Enter` to apply.
///
/// An arrow moves the boundary *towards* the direction it names — `Up` pulls
/// the top edge up (growing the glyph), `Shift+Up` pulls the **bottom** edge
/// up (shrinking it). Shifting the near edge instead would move the boundary
/// against the key that asked for it.
fn handle_resize_keys(
    ui: &egui::Ui,
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    env: crate::editor::glyph_resize::ResolveEnv<'_>,
) -> bool {
    use crate::editor::glyph_resize::{self as resize, ResizeSide};

    if !matches!(state.popup, PopupState::None) {
        return false;
    }

    if ui.input(|i| i.key_pressed(egui::Key::F2))
        && !matches!(state.mode, EditMode::GlyphResize { .. })
        && let Some(item_idx) = resize_target_at_caret(doc, lines, state)
        && resize::begin(doc, lines, state, item_idx, env)
    {
        return false;
    }

    if !matches!(state.mode, EditMode::GlyphResize { .. }) {
        return false;
    }

    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        state.pending_resize = resize::finish(doc, lines, state);
        // The preview is rolled back by `finish`; the host puts the resize
        // back as the one edit it records.
        return true;
    }

    let mut changed = false;
    for (key, shift, side) in [
        (egui::Key::ArrowUp, false, ResizeSide::Top),
        (egui::Key::ArrowUp, true, ResizeSide::Bottom),
        (egui::Key::ArrowDown, false, ResizeSide::Bottom),
        (egui::Key::ArrowDown, true, ResizeSide::Top),
        (egui::Key::ArrowLeft, false, ResizeSide::Left),
        (egui::Key::ArrowLeft, true, ResizeSide::Right),
        (egui::Key::ArrowRight, false, ResizeSide::Right),
        (egui::Key::ArrowRight, true, ResizeSide::Left),
    ] {
        if ui.input(|i| i.key_pressed(key) && i.modifiers.shift == shift && !i.modifiers.command) {
            // Unshifted moves the near edge outwards; shifted moves the far
            // edge, which travels the same way on screen and shrinks the glyph.
            let steps = if shift { -1 } else { 1 };
            changed |= resize::nudge(lines, state, side, steps);
        }
    }
    changed
}

/// The glyph a `F2` would resize: the one being pixel-edited, or the one whose
/// grid the caret sits on.
fn resize_target_at_caret(doc: &Document, lines: &[DocLine], state: &EditorState) -> Option<usize> {
    if let Some(item_idx) = state.mode.pixel_edit_item_idx() {
        return Some(item_idx);
    }
    if !matches!(state.mode, EditMode::Normal) {
        return None;
    }
    matches!(lines.get(state.cursor.line), Some(DocLine::Grid(_)))
        .then(|| line_to_item_idx(&doc.item_line_starts, state.cursor.line))
        .flatten()
}
