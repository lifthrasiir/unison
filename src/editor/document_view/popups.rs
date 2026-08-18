//! Popups anchored to the caret: rename, autocomplete and the error tooltip.

use super::*;

/// Just below `state`'s caret, whose screen position and row height the paint
/// loop stores per frame — where every caret-anchored popup goes.
pub(crate) fn caret_anchor_pos(ctx: &egui::Context, state: &EditorState) -> egui::Pos2 {
    let stored_pos: Option<egui::Pos2> = ctx.data(|d| d.get_temp(state.key(Slot::CursorScreenPos)));
    let stored_rh: f32 = ctx.data(|d| d.get_temp(state.key(Slot::CursorRowHeight)).unwrap_or(16.0));
    let pos = stored_pos.unwrap_or(egui::pos2(100.0, 100.0));
    egui::pos2(pos.x, pos.y + stored_rh + 2.0)
}

/// A foreground popup area at that anchor.
fn caret_anchored_area(ctx: &egui::Context, state: &EditorState, slot: Slot) -> egui::Area {
    egui::Area::new(state.key(slot))
        .order(egui::Order::Foreground)
        .fixed_pos(caret_anchor_pos(ctx, state))
}

/// The rename popup; returns the confirmed rename, if any.
pub(super) fn show_rename_popup(ui: &egui::Ui, state: &mut EditorState) -> Option<RenameAction> {
    use crate::editor::codepoint_popup::{FieldFrame, FieldOutcome, resolve_field};

    let mut rename_result: Option<RenameAction> = None;
    if matches!(state.popup, PopupState::Rename { .. }) {
        #[cfg(test)]
        let area_id = state.key(Slot::RenamePopup);
        let area = caret_anchored_area(ui.ctx(), state, Slot::RenamePopup);

        let area_resp = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .show(ui, |ui| {
                    ui.set_min_width(200.0);
                    let kind_label = match &state.popup {
                        PopupState::Rename { kind, .. } => match kind {
                            RenameKind::Glyph => "Rename glyph",
                            RenameKind::NameParts => "Rename name-parts",
                            RenameKind::Point => "Rename point",
                            RenameKind::Color => "Rename color",
                            RenameKind::Face => "Rename face",
                            RenameKind::Slice => "Rename slice",
                            RenameKind::RemapGroup => "Rename remap group",
                        },
                        _ => "Rename",
                    };
                    ui.label(kind_label);
                    let PopupState::Rename {
                        new_name,
                        focus_set,
                        ..
                    } = &mut state.popup
                    else {
                        return None;
                    };
                    let (resp, commit_clicked) = ui
                        .horizontal(|ui| {
                            let te = egui::TextEdit::singleline(new_name).desired_width(160.0);
                            let resp = ui.add(te);
                            // The pointer's way of pressing Enter, for anyone
                            // who reached the field with the mouse.
                            let button = ui.small_button("Rename");
                            #[cfg(test)]
                            crate::editor::harness::capture_popup_rect(
                                ui.ctx(),
                                area_id,
                                "commit",
                                button.rect,
                            );
                            (resp, button.clicked())
                        })
                        .inner;
                    if !*focus_set {
                        resp.request_focus();
                        if let Some(mut te_state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                            te_state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(new_name.chars().count()),
                                )));
                            te_state.store(ui.ctx(), resp.id);
                        }
                        *focus_set = true;
                    }
                    Some(FieldFrame {
                        id: resp.id,
                        lost_focus: resp.lost_focus(),
                        confirmed: commit_clicked
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))),
                    })
                })
                .inner
        });

        #[cfg(test)]
        crate::editor::harness::capture_popup_rect(
            ui.ctx(),
            area_id,
            "panel",
            area_resp.response.rect,
        );

        let outcome = area_resp
            .inner
            .as_ref()
            .map(|frame| resolve_field(ui.ctx(), frame, area_resp.response.rect));
        match outcome {
            Some(FieldOutcome::Commit) => {
                restore_editor_focus(ui, state);
                if let PopupState::Rename {
                    original_name,
                    new_name,
                    kind,
                    ..
                } = std::mem::replace(&mut state.popup, PopupState::None)
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
            Some(FieldOutcome::Cancel) => {
                restore_editor_focus(ui, state);
                state.popup = PopupState::None;
            }
            Some(FieldOutcome::Open) | None => {}
        }
    }
    rename_result
}

/// The Ctrl+K code point popup. While it is open the decoded character is the
/// editor's preedit, so confirming it is literally an IME commit at the caret;
/// returns whether the document changed.
pub(super) fn show_codepoint_popup(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
) -> bool {
    use crate::editor::codepoint_popup::CodepointOutcome;

    if !matches!(state.popup, PopupState::Codepoint(_)) {
        return false;
    }
    let area_id = state.key(Slot::CodepointPopup);
    let pos = caret_anchor_pos(ui.ctx(), state);

    let PopupState::Codepoint(popup) = &mut state.popup else {
        unreachable!("checked just above")
    };
    let outcome = popup.show(ui.ctx(), area_id, pos);
    // Republished every frame: the digits may have changed, and the host's
    // preedit is the only preview there is.
    state.preedit = popup.preedit();

    let mut changed = false;
    match outcome {
        CodepointOutcome::Open => {}
        CodepointOutcome::Commit(text) => {
            state.popup = PopupState::None;
            state.preedit.clear();
            restore_editor_focus(ui, state);
            if let Some(ch) = text.chars().next() {
                state.codepoint_prediction.record(ch);
                crate::editor::doc_input::delete_selection_if_any(lines, state);
                state.cursor =
                    crate::editor::editing::insert_str(lines, &mut state.undo, state.cursor, &text);
                changed = true;
            }
        }
        CodepointOutcome::Cancel => {
            state.popup = PopupState::None;
            state.preedit.clear();
            restore_editor_focus(ui, state);
        }
    }
    changed
}

/// Hands keyboard focus back to the editor canvas after a popup closes, so
/// typing continues in the document instead of going nowhere. The shaped
/// preview does the same with its own field; the rule itself lives in
/// [`crate::editor::codepoint_popup::restore_host_focus`].
fn restore_editor_focus(ui: &egui::Ui, state: &EditorState) {
    let Some(wid) = state.canvas_id else { return };
    crate::editor::codepoint_popup::restore_host_focus(ui.ctx(), wid);
}

pub(super) fn show_autocomplete_popup(
    ui: &egui::Ui,
    lines: &mut [DocLine],
    state: &mut EditorState,
    needs_rederive: &mut bool,
) {
    if state.autocomplete.is_some() {
        let ac_area =
            caret_anchored_area(ui.ctx(), state, Slot::AutocompletePopup).show(ui.ctx(), |ui| {
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
                            crate::editor::autocomplete::CompletionKind::RemapGroup => "R",
                        };
                        let text = format!("{kind_char}  {}", candidate.label);
                        if ui.selectable_label(selected, &text).clicked() {
                            clicked_idx = Some(i);
                        }
                    }
                    if ac.candidates.len() > crate::editor::autocomplete::MAX_VISIBLE {
                        ui.label(format!("{}/{}", ac.selected + 1, ac.candidates.len()));
                    }
                    clicked_idx
                })
            });
        if let Some(clicked) = ac_area.inner.inner {
            if let Some(ac) = &mut state.autocomplete {
                ac.selected = clicked;
            }
            crate::editor::autocomplete::apply_completion(lines, state);
            *needs_rederive = true;
        }
    }
}

/// Error tooltip: show when caret is inside an error span.
pub(super) fn show_error_tooltip(ui: &egui::Ui, state: &EditorState, pal: &Palette) {
    if state.active && matches!(state.popup, PopupState::None) && state.autocomplete.is_none() {
        let tooltip_data: Option<Option<(egui::Pos2, String)>> = ui
            .ctx()
            .data(|d| d.get_temp(state.key(Slot::ErrorTooltipData)));
        if let Some(Some((pos, msg))) = tooltip_data {
            egui::Area::new(state.key(Slot::ErrorTooltip))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.colored_label(pal.error, msg);
                    });
                });
        }
    }
}
