//! Keyboard handling for the document view.

use super::*;

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
    composites: &HashMap<usize, GlyphComposite>,
    prev_cursor: Caret,
    needs_rederive: &mut bool,
) {
    if state.active {
        // Autocomplete key handling takes priority
        let ac_result = crate::editor::autocomplete::handle_keys(ui, lines, state);
        if matches!(ac_result, crate::editor::autocomplete::HandleResult::TextChanged) {
            *needs_rederive = true;
        }

        if matches!(ac_result, crate::editor::autocomplete::HandleResult::NotConsumed) {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                if !matches!(state.popup, PopupState::None) {
                    state.popup = PopupState::None;
                    state.preedit.clear();
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

            // Undo/redo in GlyphEdit/LayerMove modes (Normal mode handles it via doc_input::handle_keys)
            if !matches!(state.mode, EditMode::Normal)
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

            // Backtick / 1..9: pick a layer palette slot outright
            handle_palette_shortcuts(ui, doc, composites, state);

            // PixelSelect key handling
            if matches!(state.mode, EditMode::PixelSelect { .. }) {
                // Delete/Backspace: delete selection
                if ui.input(|i| {
                    (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                        && !i.modifiers.command
                }) && state.pixel_selection.is_some()
                {
                    pixel_selection::handle_delete_selection(doc, lines, state);
                    *needs_rederive = true;
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
                    *needs_rederive = true;
                }
                if let Some(text) = sel_paste_text {
                    if pixel_selection::paste_selection(doc, lines, state, &text) {
                        *needs_rederive = true;
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
                        *needs_rederive = true;
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
                            *needs_rederive = true;
                        }
                    }
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

            if matches!(state.mode, EditMode::Normal)
                && matches!(state.popup, PopupState::None)
            {
                let text_changed = doc_input::handle_keys(ui, lines, state);
                *needs_rederive |= text_changed;

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
    let to_pixel_select = ui.input(|i| {
        i.key_pressed(egui::Key::Backtick) && !i.modifiers.command && !i.modifiers.alt
    });
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
