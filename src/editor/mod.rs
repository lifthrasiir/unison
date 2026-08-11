//! The document editor: [`EditorState`], [`EditMode`] and the widget that draws
//! and edits one `.unf` file.
//!
//! # The editor is a widget
//!
//! An instance is `DocumentEditor { doc, lines, state, env }`, shown with
//! `.show(ui)`, and those borrows are its *entire* state.
//! [`document_view::EditorEnv`] is the borrowed, `Copy`, read-only side —
//! resolved glyphs, name parts, alternatives, colour aliases, the two generation
//! counters, zoom, font id — which any number of editors share. Constructing a
//! second `DocumentEditor` over a second document and [`EditorState`] is all it
//! takes to have two live editors in one frame; `view_tests.rs`'s
//! `two_editors_do_not_share_view_state` drives exactly that through
//! `EditorHarness::split`.
//!
//! What makes that work is [`ids`]: every id an editor uses — everything it parks
//! in `ctx.data()`, every named area, panel and interaction — is salted by the
//! instance's [`EditorId`], and [`Slot`] is the single inventory of those keys.
//! Anything global in the editor beyond the two exceptions named there is a bug.
//!
//! What is genuinely per-*pane* stays with the host in `app/`: the zoom level,
//! the panel sizes, the zoom-routing rects, and the per-window escape mode.
//! Those are not editor state and do not belong in [`EditorState`].

pub mod anchor_shadow;
pub mod annotations;
pub mod autocomplete;
pub mod caret;
pub mod codepoint_popup;
pub mod colors;
pub mod doc_input;
pub mod doc_links;
pub mod document_view;
pub mod editing;
pub mod glyph_resize;
pub mod glyph_widget;
pub mod grid_render;
#[cfg(test)]
pub(crate) mod harness;
pub mod ids;
pub mod inline_tools;
pub mod line_fields;
pub mod minimap;
pub mod pixel_interaction;
pub mod pixel_selection;
pub mod reconcile;
pub(crate) use crate::ref_composite;
pub mod undo;
#[cfg(test)]
mod view_tests;
pub mod visual_lines;

use crate::document::{DocLine, Document};
use crate::edit_menu::EditMenuCaps;
use crate::pixel::PixelShape;
use doc_links::RenameKind;
pub use ids::EditorId;
pub(crate) use ids::Slot;

#[derive(Clone, Debug, PartialEq)]
pub enum EditMode {
    Normal,
    GlyphEdit {
        item_idx: usize,
        selected_shape: PixelShape,
    },
    PixelSelect {
        item_idx: usize,
    },
    LayerMove {
        item_idx: usize,
        layer_idx: usize,
    },
    /// Dragging the glyph's own boundary; see [`glyph_resize`]. The session
    /// itself (the pristine block, the deltas so far) lives in
    /// [`EditorState::resize`], because a mode is copied into undo entries and
    /// a whole snapshot has no business there.
    GlyphResize {
        item_idx: usize,
    },
}

impl EditMode {
    /// The glyph item being pixel-edited, in either grid-editing mode.
    pub fn pixel_edit_item_idx(&self) -> Option<usize> {
        match self {
            EditMode::GlyphEdit { item_idx, .. } | EditMode::PixelSelect { item_idx } => {
                Some(*item_idx)
            }
            _ => None,
        }
    }

    /// The glyph item being edited in *any* grid mode, layer moving included.
    /// This is the one the layer palette (and its keyboard shortcuts) acts on.
    pub fn edit_item_idx(&self) -> Option<usize> {
        match self {
            EditMode::GlyphEdit { item_idx, .. }
            | EditMode::PixelSelect { item_idx }
            | EditMode::LayerMove { item_idx, .. }
            | EditMode::GlyphResize { item_idx } => Some(*item_idx),
            EditMode::Normal => None,
        }
    }
}

#[derive(Debug)]
pub enum PopupState {
    None,
    Rename {
        original_name: String,
        new_name: String,
        kind: RenameKind,
        focus_set: bool,
    },
    /// Ctrl+K code point entry. See [`codepoint_popup`].
    Codepoint(codepoint_popup::CodepointPopup),
}

pub struct EditorState {
    /// This instance's `egui` id namespace. Every id the editor derives is
    /// salted with it, so two editors in one context keep their scroll
    /// offsets, drags and popups apart. See [`ids`].
    id: EditorId,
    pub(crate) mode: EditMode,
    pub(crate) cursor: caret::Caret,
    pub(crate) selection_anchor: Option<caret::Caret>,
    cursor_item: Option<usize>,
    cursor_source_line: usize,
    pub(crate) active: bool,
    pub(crate) preedit: String,
    /// Which keys the IME owns while it composes; see
    /// [`doc_input::ImeKeyGuard`].
    pub(crate) ime_guard: doc_input::ImeKeyGuard,
    pub(crate) undo: undo::UndoStack,
    pub(crate) suppress_grid_click: bool,
    pub(crate) skip_reconcile: bool,
    pub(crate) pending_reparse_line: Option<usize>,
    pub(crate) last_reparse_line: Option<usize>,
    document_sync_requested: bool,
    pub(crate) popup: PopupState,
    /// What the next Ctrl+K popup opens with, extrapolated from the last two
    /// code points committed *in this buffer*. See [`codepoint_popup`].
    pub(crate) codepoint_prediction: codepoint_popup::CodepointPrediction,
    /// Id of the editor canvas widget, republished every frame so a popup
    /// that closes can hand keyboard focus back to it.
    pub(crate) canvas_id: Option<egui::Id>,
    /// Set when the caret was moved from outside the editor; the next frame
    /// takes the keyboard focus back. The caret only paints while the widget
    /// has focus, so a host-driven jump that skipped this would move a caret
    /// nobody can see — and `canvas_id` is still `None` for a document the
    /// host jumped into before it was ever drawn, so the host cannot ask
    /// egui directly.
    pub(crate) pending_focus: bool,
    pub(crate) autocomplete: Option<autocomplete::AutocompleteState>,
    scroll_to_cursor: bool,
    pub(crate) saved_scroll_frac: f32,
    zoom_changed_from: Option<u32>,
    pub(crate) grid_hover: bool,
    /// Quarter turns the shape palette is currently rotated by (0..4).
    ///
    /// Remembered *beside* the selected shape rather than inside it: the wheel
    /// rotates every cell of the palette at once, so switching shapes keeps the
    /// orientation. See [`glyph_widget::palette_shapes`].
    pub(crate) shape_rotation: u32,
    /// Horizontal scroll offset of the glyph grid strip, in pixels. Only
    /// grids wider than the strip use it, each clamped to its own overflow,
    /// so narrow grids stay put while a wide one scrolls.
    pub(crate) grid_scroll_x: f32,
    /// The live glyph-resize session, whenever [`EditMode::GlyphResize`] is
    /// the mode. The two are set and cleared together.
    pub(crate) resize: Option<glyph_resize::GlyphResize>,
    pub(crate) pixel_selection: Option<pixel_selection::PixelSelection>,
    /// The cell a pixel selection was *started* from, kept beside the rectangle
    /// the way `selection_anchor` is kept beside the text caret.
    ///
    /// The rectangle is normalized (origin + size), so it no longer records
    /// which corner the user pinned; a drag upward-left and one downward-right
    /// produce the same rectangle. Shift-click has to extend from the pinned
    /// corner, so that corner is remembered here instead of being re-derived.
    /// `None` means "no pinned corner" — a moved or pasted selection — and
    /// extension then falls back to the rectangle's top-left.
    pub(crate) pixel_select_anchor: Option<(i16, i16)>,
    /// Cached per-frame view data (composites, visual lines, source offsets);
    /// rebuilt only when the document or layout inputs change.
    pub(crate) view_cache: Option<document_view::ViewCache>,
    /// Set by pixel painting: (item_idx, grid_doc_line) of the modified grid.
    /// Consumed by the rederive path to bypass full `derive_document`.
    pub(crate) pixel_paint_dirty: Option<(usize, usize)>,
    /// True while pixel painting is in progress (mouse held). Suppresses
    /// TTF font rebuild until the drag ends.
    pub(crate) suppress_font_rebuild: bool,
    /// A link followed this frame, handed to the host at the end of it: the
    /// host owns the other files and the navigation history.
    pub(crate) pending_nav: Option<document_view::NavRequest>,
    /// A resize applied this frame, handed to the host at the end of it: the
    /// `ref`s it has to move along may live in any file. See
    /// [`glyph_resize`].
    pub(crate) pending_resize: Option<glyph_resize::ResizeAction>,
}

impl EditorState {
    /// A new editor with a freshly allocated id namespace.
    pub fn new() -> Self {
        Self::with_id(EditorId::allocate())
    }

    /// A new editor bound to a caller-chosen id namespace, for a host that
    /// identifies its panes itself. Reusing an id hands the new editor the
    /// previous occupant's scroll offset and drag state.
    pub fn with_id(id: EditorId) -> Self {
        Self {
            id,
            mode: EditMode::Normal,
            cursor: caret::Caret::zero(),
            selection_anchor: None,
            cursor_item: None,
            cursor_source_line: 1,
            active: false,
            preedit: String::new(),
            ime_guard: Default::default(),
            undo: undo::UndoStack::new(),
            suppress_grid_click: false,
            skip_reconcile: false,
            pending_reparse_line: None,
            last_reparse_line: None,
            document_sync_requested: false,
            popup: PopupState::None,
            codepoint_prediction: Default::default(),
            canvas_id: None,
            pending_focus: false,
            autocomplete: None,
            scroll_to_cursor: false,
            saved_scroll_frac: 0.0,
            zoom_changed_from: None,
            grid_hover: false,
            shape_rotation: 0,
            grid_scroll_x: 0.0,
            resize: None,
            pixel_selection: None,
            pixel_select_anchor: None,
            view_cache: None,
            pixel_paint_dirty: None,
            suppress_font_rebuild: false,
            pending_nav: None,
            pending_resize: None,
        }
    }

    /// This editor's id namespace.
    pub fn id(&self) -> EditorId {
        self.id
    }

    /// The id of one of this editor's scratch slots.
    pub(crate) fn key(&self, slot: Slot) -> egui::Id {
        self.id.key(slot)
    }

    /// The id of a per-sub-object scratch slot of this editor.
    pub(crate) fn keyed(&self, slot: Slot, extra: impl std::hash::Hash) -> egui::Id {
        self.id.keyed(slot, extra)
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

    /// What the Edit menu may offer for this editor.
    ///
    /// In a pixel mode the menu acts on the glyph's grid rather than on the
    /// document text, so "has a selection" means the pixel selection — or, with
    /// nothing framed, the implicit whole grid that the keys act on
    /// (`pixel_selection::effective_selection`). A menu that stayed disabled
    /// there would be a second, worse answer to the same question the keyboard
    /// already answers.
    pub fn edit_menu_caps(&self, doc: &Document) -> EditMenuCaps {
        let has_selection = if self.mode.pixel_edit_item_idx().is_some() {
            pixel_selection::effective_selection(doc, self).is_some()
        } else {
            self.selection_range().is_some()
        };
        // A live resize preview is text no undo entry describes, so stepping
        // the stack under it would mix an uncommitted edit with a committed
        // one. The keyboard refuses the same chord for the same reason.
        let resizing = matches!(self.mode, EditMode::GlyphResize { .. });
        EditMenuCaps {
            can_undo: self.undo.can_undo() && !resizing,
            can_redo: self.undo.can_redo() && !resizing,
            has_selection,
            can_edit: matches!(self.mode, EditMode::Normal)
                || self.mode.pixel_edit_item_idx().is_some(),
        }
    }

    /// Takes the keyboard focus back on the next frame.
    ///
    /// Clicking a menu-bar button hands egui's focus to *that button*, and the
    /// editor is left with none: every key it handles — Ctrl+V, the pixel
    /// transforms, Escape — then goes nowhere until the user clicks back into
    /// the document. Since the button stays on screen, egui's dead-man's switch
    /// never fires and the focus is never returned on its own. Any host action
    /// dispatched from a menu that acts on this editor has to call this, the way
    /// `refocus_after_menu` does for the editor's own context menus.
    pub(crate) fn refocus(&mut self) {
        self.pending_focus = true;
    }

    pub fn goto_line(&mut self, line: usize) {
        self.mode = EditMode::Normal;
        self.selection_anchor = None;
        self.cursor = caret::Caret::new(line, 0);
        self.scroll_to_cursor = true;
        self.pending_focus = true;
    }

    /// Moves the caret to a remembered position, clamped to the document as it
    /// stands now. Navigation history records raw line/column pairs, so an edit
    /// since the jump can leave one pointing past the end.
    pub fn goto_caret(&mut self, lines: &[DocLine], line: usize, col: usize) {
        self.mode = EditMode::Normal;
        self.selection_anchor = None;
        self.cursor = caret::clamp(lines, caret::Caret::new(line, col));
        self.scroll_to_cursor = true;
        self.pending_focus = true;
    }

    /// Puts the editor back to a plain caret after its buffer was replaced
    /// wholesale from outside (a file that changed on disk).
    ///
    /// Everything dropped here indexes the *old* buffer: a grid-editing mode
    /// names an item by index, a pixel selection names a grid by line, a rename
    /// popup names a token that may be gone. `caret` is expected to be clamped
    /// to the new lines already, since only the caller has them.
    pub(crate) fn reset_for_external_reload(&mut self, caret: caret::Caret) {
        self.mode = EditMode::Normal;
        self.selection_anchor = None;
        self.resize = None;
        self.pixel_selection = None;
        self.pixel_select_anchor = None;
        self.autocomplete = None;
        self.popup = PopupState::None;
        self.cursor = caret;
        self.cursor_item = None;
        self.view_cache = None;
        self.pixel_paint_dirty = None;
        self.pending_reparse_line = None;
        self.last_reparse_line = None;
        // The lines came from the serializer, so they are canonical already;
        // reconciling them would only be a chance to move what was loaded.
        self.skip_reconcile = true;
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
            && let Some(target) = doc_links::find_renameable_at_caret(
                line_text,
                self.cursor.col,
                crate::document::at_base_at_line(lines, self.cursor.line).as_deref(),
            )
        {
            self.popup = PopupState::Rename {
                original_name: target.name.clone(),
                new_name: target.name,
                kind: target.kind,
                focus_set: false,
            };
        }
    }

    /// Opens the Ctrl+K code point popup at the caret. Like a rename, it only
    /// makes sense over document text, and only one popup is open at a time.
    pub fn start_codepoint_entry(&mut self) {
        if !matches!(self.mode, EditMode::Normal) {
            return;
        }
        if !matches!(self.popup, PopupState::None) {
            return;
        }
        self.popup = PopupState::Codepoint(codepoint_popup::CodepointPopup::seeded(
            self.codepoint_prediction.predicted(),
        ));
    }

    /// The status-bar line for an open code point popup — the code point being
    /// typed and its Unicode name. `None` when no such popup is open.
    ///
    /// `char_props` carries what the source's `prop` lines state; the host owns
    /// it because it spans every open document, not this one.
    pub fn codepoint_status(&self, char_props: &crate::ucd::CharProps) -> Option<String> {
        match &self.popup {
            PopupState::Codepoint(p) => Some(p.status_label(char_props)),
            _ => None,
        }
    }

    /// Undoes one entry and restores caret/selection state; returns whether
    /// anything changed.  The single implementation behind both the raw
    /// Cmd+Z path and the Edit-menu action.
    pub fn perform_undo(&mut self, lines: &mut Vec<DocLine>) -> bool {
        let sel_ctx = Some(undo::SelectionUndoCtx {
            mode: &mut self.mode,
            pixel_selection: &mut self.pixel_selection,
        });
        if let Some(c) = self.undo.undo_with_sel(lines, sel_ctx) {
            self.cursor = caret::clamp(lines, c);
            self.selection_anchor = None;
            self.skip_reconcile = true;
            true
        } else {
            false
        }
    }

    pub fn perform_redo(&mut self, lines: &mut Vec<DocLine>) -> bool {
        let sel_ctx = Some(undo::SelectionUndoCtx {
            mode: &mut self.mode,
            pixel_selection: &mut self.pixel_selection,
        });
        if let Some(c) = self.undo.redo_with_sel(lines, sel_ctx) {
            self.cursor = caret::clamp(lines, c);
            self.selection_anchor = None;
            self.skip_reconcile = true;
            true
        } else {
            false
        }
    }

    /// Runs an Edit-menu action against this editor.
    ///
    /// In a pixel mode it is routed to the pixel grid, so a menu item does
    /// exactly what its keyboard shortcut does; `doc` is what the pixel paths
    /// need to find the grid behind the mode's item index.
    pub fn apply_edit_action(
        &mut self,
        action: crate::edit_menu::EditAction,
        doc: &Document,
        lines: &mut Vec<DocLine>,
        ctx: &egui::Context,
    ) -> bool {
        use crate::edit_menu::EditAction;
        if self.mode.pixel_edit_item_idx().is_some()
            && let Some(changed) = self.apply_pixel_edit_action(action, doc, lines, ctx)
        {
            if changed {
                self.request_document_sync();
            }
            return changed;
        }
        let changed = match action {
            EditAction::None => false,
            EditAction::Undo => self.perform_undo(lines),
            EditAction::Redo => self.perform_redo(lines),
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
                self.cursor = caret::Caret::new(last, caret::line_char_len(lines, last));
                false
            }
        };

        if changed {
            self.request_document_sync();
        }
        changed
    }

    /// The pixel-grid reading of an Edit-menu action, or `None` for the actions
    /// that mean the same thing in every mode and fall through to the text
    /// path (nothing, undo, redo).
    fn apply_pixel_edit_action(
        &mut self,
        action: crate::edit_menu::EditAction,
        doc: &Document,
        lines: &mut [DocLine],
        ctx: &egui::Context,
    ) -> Option<bool> {
        use crate::edit_menu::EditAction;
        match action {
            EditAction::None | EditAction::Undo | EditAction::Redo => None,
            EditAction::Copy => {
                if let Some(sel) = pixel_selection::effective_selection(doc, self)
                    && let Some(text) = pixel_selection::copy_selection(doc, lines, &sel)
                {
                    ctx.copy_text(text);
                }
                Some(false)
            }
            EditAction::Cut => {
                let Some(sel) = pixel_selection::effective_selection(doc, self) else {
                    return Some(false);
                };
                if let Some(text) = pixel_selection::copy_selection(doc, lines, &sel) {
                    ctx.copy_text(text);
                }
                pixel_selection::handle_delete_selection(doc, lines, self);
                Some(true)
            }
            EditAction::Delete => {
                pixel_selection::handle_delete_selection(doc, lines, self);
                Some(true)
            }
            EditAction::Paste => {
                let text = arboard::Clipboard::new()
                    .ok()
                    .and_then(|mut clip| clip.get_text().ok())
                    .filter(|t| !t.is_empty());
                match text {
                    Some(text) => Some(pixel_selection::paste_selection(doc, lines, self, &text)),
                    None => Some(false),
                }
            }
            EditAction::SelectAll => Some(pixel_selection::select_all(doc, lines, self)),
        }
    }
}
