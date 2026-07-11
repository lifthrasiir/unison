use crate::document::DocLine;
use crate::editor::EditorState;
use crate::editor::caret::{self, Caret};

pub(crate) fn handle_keys(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
) -> bool {
    let mut changed = false;

    let undo_pressed =
        ui.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z));
    let redo_pressed = ui.input(|i| {
        (i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z))
            || (i.modifiers.command && i.key_pressed(egui::Key::Y))
    });

    if undo_pressed && let Some(c) = state.undo.undo(lines) {
        state.cursor = caret::clamp(lines, c);
        state.selection_anchor = None;
        state.skip_reconcile = true;
        return true;
    }
    if redo_pressed && let Some(c) = state.undo.redo(lines) {
        state.cursor = caret::clamp(lines, c);
        state.selection_anchor = None;
        state.skip_reconcile = true;
        return true;
    }

    let mut clipboard_out: Option<String> = None;

    ui.input(|input| {
        for event in &input.events {
            match event {
                egui::Event::Copy => {
                    if let Some((lo, hi)) = state.selection_range() {
                        let text = caret::extract_text(lines, lo, hi);
                        if !text.is_empty() {
                            clipboard_out = Some(text);
                        }
                    }
                }
                egui::Event::Cut => {
                    if let Some((lo, hi)) = state.selection_range() {
                        let text = caret::extract_text(lines, lo, hi);
                        if !text.is_empty() {
                            clipboard_out = Some(text);
                        }
                        state.cursor = crate::editor::editing::delete_selection(
                            lines,
                            &mut state.undo,
                            state.cursor,
                            state.selection_anchor.unwrap(),
                        );
                        state.selection_anchor = None;
                        changed = true;
                    }
                }
                egui::Event::Paste(text_to_paste) => {
                    if !text_to_paste.is_empty() {
                        delete_selection_if_any(lines, state);
                        paste_text(lines, &mut state.undo, &mut state.cursor, text_to_paste);
                        changed = true;
                    }
                }
                egui::Event::Text(s) => {
                    if let Some(anchor) = state.selection_anchor {
                        state.cursor = crate::editor::editing::delete_selection(
                            lines,
                            &mut state.undo,
                            state.cursor,
                            anchor,
                        );
                        state.selection_anchor = None;
                    }
                    state.cursor =
                        crate::editor::editing::insert_str(lines, &mut state.undo, state.cursor, s);
                    changed = true;
                }
                egui::Event::Ime(egui::ImeEvent::Preedit(s)) => {
                    state.preedit = s.clone();
                }
                egui::Event::Ime(egui::ImeEvent::Commit(s)) => {
                    state.preedit.clear();
                    if let Some((_lo, _hi)) = state.selection_range() {
                        state.cursor = crate::editor::editing::delete_selection(
                            lines,
                            &mut state.undo,
                            state.cursor,
                            state.selection_anchor.unwrap(),
                        );
                        state.selection_anchor = None;
                    }
                    state.cursor =
                        crate::editor::editing::insert_str(lines, &mut state.undo, state.cursor, s);
                    changed = true;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let shift = modifiers.shift;
                    let word_mod = if cfg!(target_os = "macos") {
                        modifiers.alt
                    } else {
                        modifiers.ctrl
                    };
                    match key {
                        egui::Key::Z if modifiers.command => {}
                        egui::Key::Y if modifiers.command => {}
                        egui::Key::Backspace => {
                            if delete_selection_if_any(lines, state) {
                                changed = true;
                            } else if word_mod {
                                let word_start = caret::move_word_left(lines, state.cursor);
                                if word_start != state.cursor {
                                    state.cursor = crate::editor::editing::delete_selection(
                                        lines,
                                        &mut state.undo,
                                        state.cursor,
                                        word_start,
                                    );
                                    changed = true;
                                }
                            } else {
                                let new_c = crate::editor::editing::backspace(
                                    lines,
                                    &mut state.undo,
                                    state.cursor,
                                );
                                changed = new_c != state.cursor;
                                state.cursor = new_c;
                            }
                        }
                        egui::Key::Delete => {
                            if delete_selection_if_any(lines, state) {
                                changed = true;
                            } else if word_mod {
                                let word_end = caret::move_word_right(lines, state.cursor);
                                if word_end != state.cursor {
                                    state.cursor = crate::editor::editing::delete_selection(
                                        lines,
                                        &mut state.undo,
                                        state.cursor,
                                        word_end,
                                    );
                                    changed = true;
                                }
                            } else {
                                let new_c = crate::editor::editing::delete(
                                    lines,
                                    &mut state.undo,
                                    state.cursor,
                                );
                                changed = new_c != state.cursor;
                                state.cursor = new_c;
                            }
                        }
                        egui::Key::Enter => {
                            delete_selection_if_any(lines, state);
                            state.cursor = crate::editor::editing::insert_newline(
                                lines,
                                &mut state.undo,
                                state.cursor,
                            );
                            changed = true;
                        }
                        egui::Key::ArrowLeft => {
                            update_selection(state, shift);
                            if word_mod {
                                state.cursor = caret::move_word_left(lines, state.cursor);
                            } else if modifiers.command {
                                state.cursor = caret::home(lines, state.cursor);
                            } else {
                                state.cursor = caret::move_left(lines, state.cursor);
                            }
                        }
                        egui::Key::ArrowRight => {
                            update_selection(state, shift);
                            if word_mod {
                                state.cursor = caret::move_word_right(lines, state.cursor);
                            } else if modifiers.command {
                                state.cursor = caret::end(lines, state.cursor);
                            } else {
                                state.cursor = caret::move_right(lines, state.cursor);
                            }
                        }
                        egui::Key::ArrowUp => {
                            update_selection(state, shift);
                            if modifiers.command {
                                state.cursor = caret::doc_home(lines);
                            } else {
                                state.cursor = caret::move_up(lines, state.cursor);
                            }
                        }
                        egui::Key::ArrowDown => {
                            update_selection(state, shift);
                            if modifiers.command {
                                state.cursor = caret::doc_end(lines);
                            } else {
                                state.cursor = caret::move_down(lines, state.cursor);
                            }
                        }
                        egui::Key::Home => {
                            update_selection(state, shift);
                            if modifiers.command {
                                state.cursor = caret::doc_home(lines);
                            } else {
                                state.cursor = caret::home(lines, state.cursor);
                            }
                        }
                        egui::Key::End => {
                            update_selection(state, shift);
                            if modifiers.command {
                                state.cursor = caret::doc_end(lines);
                            } else {
                                state.cursor = caret::end(lines, state.cursor);
                            }
                        }
                        egui::Key::PageUp | egui::Key::PageDown => {}
                        egui::Key::A if modifiers.command => {
                            state.selection_anchor = Some(Caret::zero());
                            let last = lines.len().saturating_sub(1);
                            state.cursor = Caret::new(last, caret::line_char_len(lines, last));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    });

    if let Some(text) = clipboard_out {
        ui.ctx().copy_text(text);
    }

    let page_dir = if ui.input(|i| i.key_pressed(egui::Key::PageDown)) {
        Some(1i32)
    } else if ui.input(|i| i.key_pressed(egui::Key::PageUp)) {
        Some(-1i32)
    } else {
        None
    };
    if let Some(dir) = page_dir {
        let shift = ui.input(|i| i.modifiers.shift);
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("page_scroll_request"), (dir, shift));
        });
    }

    changed
}

pub(crate) fn delete_selection_if_any(lines: &mut Vec<DocLine>, state: &mut EditorState) -> bool {
    if let Some(anchor) = state.selection_anchor {
        state.cursor =
            crate::editor::editing::delete_selection(lines, &mut state.undo, state.cursor, anchor);
        state.selection_anchor = None;
        true
    } else {
        false
    }
}

pub(crate) fn paste_text(
    lines: &mut Vec<DocLine>,
    undo: &mut crate::editor::undo::UndoStack,
    cursor: &mut Caret,
    text: &str,
) {
    for (i, chunk) in text.split('\n').enumerate() {
        if i > 0 {
            *cursor = crate::editor::editing::insert_newline(lines, undo, *cursor);
        }
        if !chunk.is_empty() {
            let clean: String = chunk.replace('\r', "");
            *cursor = crate::editor::editing::insert_str(lines, undo, *cursor, &clean);
        }
    }
}

fn update_selection(state: &mut EditorState, shift: bool) {
    if shift {
        if state.selection_anchor.is_none() {
            state.selection_anchor = Some(state.cursor);
        }
    } else {
        state.selection_anchor = None;
    }
}
