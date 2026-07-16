pub mod autocomplete;
pub mod caret;
pub mod colors;
pub mod doc_input;
pub mod doc_links;
pub mod document_view;
pub mod editing;
pub mod glyph_widget;
pub mod grid_render;
#[cfg(test)]
pub(crate) mod harness;
pub mod inline_tools;
pub mod minimap;
pub mod pixel_interaction;
pub mod reconcile;
pub(crate) use crate::ref_composite;
pub mod undo;
#[cfg(test)]
mod view_tests;
pub mod visual_lines;

use crate::document::DocLine;
use crate::edit_menu::EditMenuCaps;
use crate::pixel::PixelShape;
use doc_links::RenameKind;

/// Maps a hex-digit key press to its character, for Alt+hex Unicode entry.
pub(crate) fn key_to_hex_char(key: egui::Key) -> Option<char> {
    match key {
        egui::Key::Num0 => Some('0'),
        egui::Key::Num1 => Some('1'),
        egui::Key::Num2 => Some('2'),
        egui::Key::Num3 => Some('3'),
        egui::Key::Num4 => Some('4'),
        egui::Key::Num5 => Some('5'),
        egui::Key::Num6 => Some('6'),
        egui::Key::Num7 => Some('7'),
        egui::Key::Num8 => Some('8'),
        egui::Key::Num9 => Some('9'),
        egui::Key::A => Some('A'),
        egui::Key::B => Some('B'),
        egui::Key::C => Some('C'),
        egui::Key::D => Some('D'),
        egui::Key::E => Some('E'),
        egui::Key::F => Some('F'),
        _ => None,
    }
}

/// Parses an accumulated hex string into a scalar value, rejecting
/// surrogates and out-of-range codepoints.
pub(crate) fn validate_hex_codepoint(hex: &str) -> Option<char> {
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditMode {
    Normal,
    GlyphEdit {
        item_idx: usize,
        selected_shape: PixelShape,
    },
    LayerMove {
        item_idx: usize,
        layer_idx: usize,
    },
}

pub enum PopupState {
    None,
    Rename {
        original_name: String,
        new_name: String,
        kind: RenameKind,
        focus_set: bool,
    },
}

pub struct EditorState {
    pub(crate) mode: EditMode,
    pub(crate) cursor: caret::Caret,
    pub(crate) selection_anchor: Option<caret::Caret>,
    cursor_item: Option<usize>,
    cursor_source_line: usize,
    pub(crate) active: bool,
    pub(crate) preedit: String,
    pub(crate) undo: undo::UndoStack,
    pub(crate) suppress_grid_click: bool,
    pub(crate) skip_reconcile: bool,
    pub(crate) pending_reparse_line: Option<usize>,
    pub(crate) last_reparse_line: Option<usize>,
    document_sync_requested: bool,
    pub(crate) popup: PopupState,
    pub(crate) autocomplete: Option<autocomplete::AutocompleteState>,
    scroll_to_cursor: bool,
    zoom_changed_from: Option<u32>,
    pub(crate) grid_hover: bool,
    /// Cached per-frame view data (composites, visual lines, source offsets);
    /// rebuilt only when the document or layout inputs change.
    pub(crate) view_cache: Option<document_view::ViewCache>,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            mode: EditMode::Normal,
            cursor: caret::Caret::zero(),
            selection_anchor: None,
            cursor_item: None,
            cursor_source_line: 1,
            active: false,
            preedit: String::new(),
            undo: undo::UndoStack::new(),
            suppress_grid_click: false,
            skip_reconcile: false,
            pending_reparse_line: None,
            last_reparse_line: None,
            document_sync_requested: false,
            popup: PopupState::None,
            autocomplete: None,
            scroll_to_cursor: false,
            zoom_changed_from: None,
            grid_hover: false,
            view_cache: None,
        }
    }

    pub fn selection_range(&self) -> Option<(caret::Caret, caret::Caret)> {
        caret::selection_range(self.cursor, self.selection_anchor)
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn cursor_source_line(&self) -> usize {
        self.cursor_source_line
    }

    pub fn edit_menu_caps(&self) -> EditMenuCaps {
        EditMenuCaps {
            can_undo: self.undo.can_undo(),
            can_redo: self.undo.can_redo(),
            has_selection: self.selection_range().is_some(),
            can_edit: matches!(self.mode, EditMode::Normal),
        }
    }

    pub fn goto_line(&mut self, line: usize) {
        self.mode = EditMode::Normal;
        self.selection_anchor = None;
        self.cursor = caret::Caret::new(line, 0);
        self.scroll_to_cursor = true;
    }

    pub fn notify_zoom_change(&mut self, old_zoom: u32) {
        self.zoom_changed_from = Some(old_zoom);
    }

    pub(crate) fn take_zoom_change(&mut self) -> Option<u32> {
        self.zoom_changed_from.take()
    }

    pub(crate) fn take_scroll_to_cursor(&mut self) -> bool {
        std::mem::replace(&mut self.scroll_to_cursor, false)
    }

    pub fn is_grid_hover(&self) -> bool {
        self.grid_hover
    }

    /// Queue a line-to-document synchronization for mutations performed
    /// outside `show_document` (for example, an action from the application
    /// menu after the editor was rendered for the frame).
    pub(crate) fn request_document_sync(&mut self) {
        self.document_sync_requested = true;
    }

    pub(crate) fn take_document_sync_request(&mut self) -> bool {
        std::mem::replace(&mut self.document_sync_requested, false)
    }

    pub(crate) fn has_pending_document_sync(&self) -> bool {
        self.document_sync_requested || self.pending_reparse_line.is_some()
    }

    pub(crate) fn clear_document_sync_request(&mut self) {
        self.document_sync_requested = false;
    }

    pub fn start_rename_at_cursor(&mut self, lines: &[DocLine]) {
        if !matches!(self.mode, EditMode::Normal) {
            return;
        }
        if !matches!(self.popup, PopupState::None) {
            return;
        }
        if let Some(DocLine::Text(line_text)) = lines.get(self.cursor.line)
            && let Some(target) =
                doc_links::find_renameable_at_caret(line_text, self.cursor.col)
            {
                self.popup = PopupState::Rename {
                    original_name: target.name.clone(),
                    new_name: target.name,
                    kind: target.kind,
                    focus_set: false,
                };
            }
    }

    pub fn apply_edit_action(
        &mut self,
        action: crate::edit_menu::EditAction,
        lines: &mut Vec<DocLine>,
        ctx: &egui::Context,
    ) -> bool {
        use crate::edit_menu::EditAction;
        let changed = match action {
            EditAction::None => false,
            EditAction::Undo => {
                if let Some(c) = self.undo.undo(lines) {
                    self.cursor = caret::clamp(lines, c);
                    self.selection_anchor = None;
                    self.skip_reconcile = true;
                    true
                } else {
                    false
                }
            }
            EditAction::Redo => {
                if let Some(c) = self.undo.redo(lines) {
                    self.cursor = caret::clamp(lines, c);
                    self.selection_anchor = None;
                    self.skip_reconcile = true;
                    true
                } else {
                    false
                }
            }
            EditAction::Cut => {
                if let Some((lo, hi)) = self.selection_range() {
                    let text = caret::extract_text(lines, lo, hi);
                    ctx.copy_text(text);
                    self.cursor = crate::editor::editing::delete_selection(
                        lines,
                        &mut self.undo,
                        self.cursor,
                        self.selection_anchor.unwrap(),
                    );
                    self.selection_anchor = None;
                    true
                } else {
                    false
                }
            }
            EditAction::Copy => {
                if let Some((lo, hi)) = self.selection_range() {
                    let text = caret::extract_text(lines, lo, hi);
                    ctx.copy_text(text);
                }
                false
            }
            EditAction::Paste => {
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && let Ok(text_to_paste) = clip.get_text()
                        && !text_to_paste.is_empty()
                {
                    doc_input::paste_text(
                        lines,
                        &mut self.undo,
                        &mut self.cursor,
                        self.selection_anchor.take(),
                        &text_to_paste,
                    );
                    true
                } else {
                    false
                }
            }
            EditAction::Delete => {
                if self.selection_anchor.is_some() {
                    doc_input::delete_selection_if_any(lines, self);
                    true
                } else {
                    false
                }
            }
            EditAction::SelectAll => {
                self.selection_anchor = Some(caret::Caret::zero());
                let last = lines.len().saturating_sub(1);
                self.cursor =
                    caret::Caret::new(last, caret::line_char_len(lines, last));
                false
            }
        };

        if changed {
            self.request_document_sync();
        }
        changed
    }
}
