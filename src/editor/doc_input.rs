//! The text-editing keys, shared by the document editor and the shaped preview.
//!
//! What a field is handed is the *end* of a long chain — a keyboard layout, the
//! platform IME, `winit`, `egui-winit` — and the chain has holes we cannot patch
//! from here. One known `winit` bug: with the Korean IME on a Sebeolsik layout,
//! Shift+B (the layout's `?`) delivers nothing at all — no `Text`, no `Ime`, not
//! even a key press — so no amount of handling below can recover the character.
//! When a keystroke appears to do nothing, rule this class out before looking
//! for a bug in this module.

use crate::document::DocLine;
use crate::editor::caret::{self, Caret};
use crate::editor::undo::UndoStack;
use crate::editor::{EditorState, Slot};

/// Everything the plain-text key handler touches, borrowed from whoever owns
/// it. `EditorState` keeps these as separate fields (next to a pile of things
/// only the document editor has); the shaped preview owns nothing else. Going
/// through this struct is what lets both run [`handle_text_keys`] — that is,
/// the same caret motions, word motions, selection, clipboard and IME
/// behavior — over the same `Vec<DocLine>` text model.
///
/// The preview's lines are always `DocLine::Text`, so the grid cases below are
/// dead for it; they are what the *document* editor needs and cost nothing to
/// carry.
pub(crate) struct TextEdit<'a> {
    pub lines: &'a mut Vec<DocLine>,
    pub cursor: &'a mut Caret,
    pub selection_anchor: &'a mut Option<Caret>,
    pub undo: &'a mut UndoStack,
    pub preedit: &'a mut String,
    /// Which keys the IME owns right now; see [`ImeKeyGuard`].
    pub ime_guard: &'a mut ImeKeyGuard,
}

impl TextEdit<'_> {
    fn selection_range(&self) -> Option<(Caret, Caret)> {
        caret::selection_range(*self.cursor, *self.selection_anchor)
    }

    /// Deletes the selection, reporting whether anything was actually removed.
    /// A *collapsed* anchor (one sitting exactly on the caret, left behind by
    /// a shift-selection that was undone or by a click) is not a selection: it
    /// is dropped and the key that asked for the delete goes on to do its own
    /// job, rather than being swallowed.
    fn delete_selection_if_any(&mut self) -> bool {
        let Some(anchor) = self.selection_anchor.take() else {
            return false;
        };
        if anchor == *self.cursor {
            return false;
        }
        *self.cursor =
            crate::editor::editing::delete_selection(self.lines, self.undo, *self.cursor, anchor);
        true
    }
}

pub(crate) fn handle_keys(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
) -> bool {
    let undo_pressed =
        ui.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z));
    let redo_pressed = ui.input(|i| {
        (i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z))
            || (i.modifiers.command && i.key_pressed(egui::Key::Y))
    });

    if undo_pressed && state.perform_undo(lines) {
        return true;
    }
    if redo_pressed && state.perform_redo(lines) {
        return true;
    }

    // Page Up/Down are read here rather than in the shared handler, so they
    // get the coarse version of the [`ImeKeyGuard`] rule: while a
    // composition is open they are simply not acted on. Nothing holds a page
    // key for later — a scroll that arrives two frames late is worse than one
    // that never happens.
    let composing = ui.input(|i| ime_composing(&i.events, &state.preedit));

    let changed = handle_text_keys(
        ui,
        &mut TextEdit {
            lines,
            cursor: &mut state.cursor,
            selection_anchor: &mut state.selection_anchor,
            undo: &mut state.undo,
            preedit: &mut state.preedit,
            ime_guard: &mut state.ime_guard,
        },
    );

    let page_dir = if composing {
        None
    } else if ui.input(|i| i.key_pressed(egui::Key::PageDown)) {
        Some(1i32)
    } else if ui.input(|i| i.key_pressed(egui::Key::PageUp)) {
        Some(-1i32)
    } else {
        None
    };
    if let Some(dir) = page_dir {
        let shift = ui.input(|i| i.modifiers.shift);
        ui.ctx().data_mut(|d| {
            d.insert_temp(state.key(Slot::PageScrollRequest), (dir, shift));
        });
    }

    changed
}

/// The shared body: every text-editing key, clipboard event and IME event, on
/// the borrowed state in `te`. Returns whether the text changed.
///
/// Undo and redo are *not* handled here — the document editor has to run them
/// through `EditorState` (reconcile suppression, rederive) while the preview
/// drives its undo stack directly.
pub(crate) fn handle_text_keys(ui: &egui::Ui, te: &mut TextEdit<'_>) -> bool {
    let mut changed = false;
    let mut clipboard_out: Option<String> = None;

    let events = ui.input(|input| input.events.clone());

    // Strictly in the order the platform queued them: for a key the IME might
    // claim, *where* it sits relative to the IME's own events is the whole
    // signal. See [`ImeKeyGuard`].
    let mut composing = !te.preedit.is_empty();
    for event in &events {
        match event {
            egui::Event::Ime(egui::ImeEvent::Preedit(s)) => {
                s.clone_into(te.preedit);
                composing = !s.is_empty();
                if !composing {
                    te.ime_guard.composition_ended(false);
                }
            }
            // The composition ended without committing anything — the IME was
            // switched off, or it dropped what it had. Clearing the preedit
            // here also keeps the composing state from getting stuck, which
            // would swallow keys forever.
            egui::Event::Ime(egui::ImeEvent::Disabled) => {
                te.preedit.clear();
                if composing {
                    te.ime_guard.composition_ended(false);
                }
                composing = false;
            }
            egui::Event::Ime(egui::ImeEvent::Commit(s)) => {
                te.preedit.clear();
                te.delete_selection_if_any();
                *te.cursor = crate::editor::editing::insert_str(te.lines, te.undo, *te.cursor, s);
                changed = true;
                composing = false;
                te.ime_guard.composition_ended(true);
            }
            egui::Event::Ime(egui::ImeEvent::Enabled) => {}
            _ => {
                if te.ime_guard.claims(event, composing) {
                    continue;
                }
                apply_event(te, event, &mut changed, &mut clipboard_out);
            }
        }
    }
    te.ime_guard.end_frame();

    if let Some(text) = clipboard_out {
        ui.ctx().copy_text(text);
    }

    changed
}

/// Applies one non-IME event: clipboard, text, or a key press.
fn apply_event(
    te: &mut TextEdit<'_>,
    event: &egui::Event,
    changed: &mut bool,
    clipboard_out: &mut Option<String>,
) {
    {
        match event {
            egui::Event::Copy => {
                let (lo, hi) = copy_range(te);
                let text = caret::extract_text(te.lines, lo, hi);
                if !text.is_empty() {
                    *clipboard_out = Some(text);
                }
            }
            egui::Event::Cut => {
                let (lo, hi) = copy_range(te);
                let text = caret::extract_text(te.lines, lo, hi);
                if !text.is_empty() {
                    *clipboard_out = Some(text);
                }
                *te.cursor = if let Some(anchor) = te.selection_anchor.take() {
                    crate::editor::editing::delete_selection(te.lines, te.undo, *te.cursor, anchor)
                } else {
                    crate::editor::editing::delete_selection(te.lines, te.undo, lo, hi)
                };
                *changed = true;
            }
            egui::Event::Paste(text_to_paste) => {
                if !text_to_paste.is_empty() {
                    paste_text(
                        te.lines,
                        te.undo,
                        te.cursor,
                        te.selection_anchor.take(),
                        text_to_paste,
                    );
                    *changed = true;
                }
            }
            egui::Event::Text(s) => {
                te.delete_selection_if_any();
                *te.cursor = crate::editor::editing::insert_str(te.lines, te.undo, *te.cursor, s);
                *changed = true;
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
                        if te.delete_selection_if_any() {
                            *changed = true;
                        } else if word_mod {
                            let word_start = caret::move_word_left(te.lines, *te.cursor);
                            if word_start != *te.cursor {
                                *te.cursor = crate::editor::editing::delete_selection(
                                    te.lines, te.undo, *te.cursor, word_start,
                                );
                                *changed = true;
                            }
                        } else {
                            let (new_c, deleted) =
                                crate::editor::editing::backspace(te.lines, te.undo, *te.cursor);
                            *te.cursor = new_c;
                            *changed = deleted;
                        }
                    }
                    egui::Key::Delete => {
                        if te.delete_selection_if_any() {
                            *changed = true;
                        } else if word_mod {
                            let word_end = caret::move_word_right(te.lines, *te.cursor);
                            if word_end != *te.cursor {
                                *te.cursor = crate::editor::editing::delete_selection(
                                    te.lines, te.undo, *te.cursor, word_end,
                                );
                                *changed = true;
                            }
                        } else {
                            let (new_c, deleted) =
                                crate::editor::editing::delete(te.lines, te.undo, *te.cursor);
                            *te.cursor = new_c;
                            *changed = deleted;
                        }
                    }
                    egui::Key::Enter => {
                        te.delete_selection_if_any();
                        *te.cursor =
                            crate::editor::editing::insert_newline(te.lines, te.undo, *te.cursor);
                        *changed = true;
                    }
                    egui::Key::ArrowLeft => {
                        update_selection(te, shift);
                        if word_mod {
                            *te.cursor = caret::move_word_left(te.lines, *te.cursor);
                        } else if modifiers.command {
                            *te.cursor = caret::home(te.lines, *te.cursor);
                        } else {
                            *te.cursor = caret::move_left(te.lines, *te.cursor);
                        }
                    }
                    egui::Key::ArrowRight => {
                        update_selection(te, shift);
                        if word_mod {
                            *te.cursor = caret::move_word_right(te.lines, *te.cursor);
                        } else if modifiers.command {
                            *te.cursor = caret::end(te.lines, *te.cursor);
                        } else {
                            *te.cursor = caret::move_right(te.lines, *te.cursor);
                        }
                    }
                    egui::Key::ArrowUp => {
                        update_selection(te, shift);
                        if modifiers.command {
                            *te.cursor = caret::doc_home(te.lines);
                        } else {
                            *te.cursor = caret::move_up(te.lines, *te.cursor);
                        }
                    }
                    egui::Key::ArrowDown => {
                        update_selection(te, shift);
                        if modifiers.command {
                            *te.cursor = caret::doc_end(te.lines);
                        } else {
                            *te.cursor = caret::move_down(te.lines, *te.cursor);
                        }
                    }
                    egui::Key::Home => {
                        update_selection(te, shift);
                        if modifiers.command {
                            *te.cursor = caret::doc_home(te.lines);
                        } else {
                            *te.cursor = caret::home(te.lines, *te.cursor);
                        }
                    }
                    egui::Key::End => {
                        update_selection(te, shift);
                        if modifiers.command {
                            *te.cursor = caret::doc_end(te.lines);
                        } else {
                            *te.cursor = caret::end(te.lines, *te.cursor);
                        }
                    }
                    egui::Key::PageUp | egui::Key::PageDown => {}
                    egui::Key::A if modifiers.command => {
                        *te.selection_anchor = Some(Caret::zero());
                        let last = te.lines.len().saturating_sub(1);
                        *te.cursor = Caret::new(last, caret::line_char_len(te.lines, last));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Is an IME composition in progress over `events`? True when one was already
/// open (`preedit` is non-empty) or when this very batch opens one.
pub(crate) fn ime_composing(events: &[egui::Event], preedit: &str) -> bool {
    if !preedit.is_empty() {
        return true;
    }
    events
        .iter()
        .any(|e| matches!(e, egui::Event::Ime(egui::ImeEvent::Preedit(s)) if !s.is_empty()))
}

/// Who owns a key press while an IME is composing.
///
/// The IMEs disagree, and the platform tells the two apart only by *when* it
/// delivers the key relative to the IME's own events:
///
/// * The Japanese and Chinese IMEs consume the key. An arrow walks the
///   composition and the IME answers with a fresh preedit; Enter confirms the
///   composition and nothing else. Either way the key press we were handed
///   before that answer was never ours.
/// * The Korean IME consumes nothing but Backspace. It answers a key it does
///   not want by committing what it has — and then macOS re-delivers the same
///   physical key press a second time, after the IME events, which `egui`
///   flags as a repeat (`winit` knows this too: "we'll end up sending it twice
///   with some IMEs like Korean one"). That second copy is the pass-through,
///   and it is the one to act on.
///
/// So the rule is positional, and a key press that lands just after a
/// composition ended is only ours if we *saw the IME swallow the same key
/// first*. That last clause is what tells a pass-through from a key the user
/// never pressed at us: picking a Hanja from the conversion window ends the
/// composition and delivers only the trailing Enter — the press that opened
/// and drove the candidate window went to the window, not to us, so that Enter
/// belongs to the IME even though it arrives after the commit.
///
/// | Sequence | Enter is |
/// | --- | --- |
/// | press, commit, press (re-delivered) | ours — commit, then break the line |
/// | press, commit (Japanese: no re-delivery) | the IME's — commit only |
/// | commit, press (Hanja window picked it) | the IME's — commit only |
///
/// Backspace is the exception on both sides — every IME eats it while
/// composing — so it stays dropped for the whole window after the composition
/// ends as well, since the platform can split one key's events across frames.
#[derive(Default)]
pub(crate) struct ImeKeyGuard {
    /// Keys whose press the open composition swallowed. A press of one of
    /// these arriving after the composition ends is the platform re-delivering
    /// it, which is the copy the document acts on.
    swallowed: Vec<egui::Key>,
    /// Frames left in the window that follows a composition, in which the
    /// rules above still apply.
    after_composition: u8,
    /// Whether that composition ended by committing text. Only a commit can be
    /// the tail of a candidate window, so only a commit lets an unmatched key
    /// press be claimed; a composition merely switched off (no commit) leaves
    /// the keyboard to the document at once.
    ended_with_commit: bool,
}

/// How long the aftermath of a composition lasts, in frames. Long enough to
/// cover a key's events being split across frames, short enough to be
/// invisible next to the ~500ms key-repeat delay.
const AFTER_COMPOSITION_FRAMES: u8 = 2;

/// Keys an open composition may claim. They are the motion and editing keys
/// the IMEs bind: everything else (text, shortcuts, Escape) always passes
/// straight through.
fn is_claimable(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::Enter
            | egui::Key::Backspace
            | egui::Key::Delete
            | egui::Key::ArrowLeft
            | egui::Key::ArrowRight
            | egui::Key::ArrowUp
            | egui::Key::ArrowDown
            | egui::Key::Home
            | egui::Key::End
    )
}

impl ImeKeyGuard {
    fn composition_ended(&mut self, with_commit: bool) {
        self.after_composition = AFTER_COMPOSITION_FRAMES;
        self.ended_with_commit = with_commit;
    }

    fn end_frame(&mut self) {
        // Only the window's own frames count down; while a composition is
        // still open its swallowed keys have to survive to be matched.
        if self.after_composition > 0 {
            self.after_composition -= 1;
            if self.after_composition == 0 {
                self.swallowed.clear();
            }
        }
    }

    /// Does the IME own this event rather than the document?
    fn claims(&mut self, event: &egui::Event, composing: bool) -> bool {
        let egui::Event::Key {
            key, pressed: true, ..
        } = event
        else {
            return false;
        };
        if !is_claimable(*key) {
            return false;
        }
        if composing {
            self.swallowed.push(*key);
            return true;
        }
        if self.after_composition == 0 {
            return false;
        }
        // Backspace shortened the composition; the platform re-delivering it
        // does not make it a deletion in the document.
        if *key == egui::Key::Backspace {
            return true;
        }
        match self.swallowed.iter().position(|k| k == key) {
            // The re-delivered copy of a press we hid: the pass-through.
            Some(pos) => {
                self.swallowed.remove(pos);
                false
            }
            // A press with no press of its own before it — the IME's candidate
            // window had the keyboard for that one.
            None => self.ended_with_commit,
        }
    }
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
    selection_anchor: Option<Caret>,
    text: &str,
) {
    if text.is_empty() {
        return;
    }

    let sel = selection_anchor.and_then(|anchor| {
        let lo = (*cursor).min(anchor);
        let hi = (*cursor).max(anchor);
        if lo != hi { Some((lo, hi)) } else { None }
    });

    let chunks: Vec<&str> = text.split('\n').collect();

    // No selection, single-line paste: use Text op (supports coalescing).
    if sel.is_none() && chunks.len() == 1 {
        let clean: String = chunks[0].replace('\r', "");
        if !clean.is_empty() {
            *cursor = crate::editor::editing::insert_str(lines, undo, *cursor, &clean);
        }
        return;
    }

    // Everything else (multi-line paste, or paste-over-selection) is a
    // single Lines op so the whole thing undoes atomically.
    let (lo, hi) = sel.unwrap_or((*cursor, *cursor));

    let prefix = match &lines[lo.line] {
        DocLine::Text(s) => {
            let byte = crate::editor::caret::char_to_byte(s, lo.col);
            s[..byte].to_string()
        }
        DocLine::Grid(_) => return,
    };

    let suffix = match &lines[hi.line] {
        DocLine::Text(s) => {
            let byte = crate::editor::caret::char_to_byte(s, hi.col);
            s[byte..].to_string()
        }
        DocLine::Grid(_) => String::new(),
    };

    let old: Vec<DocLine> = lines[lo.line..=hi.line].to_vec();
    let mut new: Vec<DocLine> = Vec::with_capacity(chunks.len());

    let first_clean = chunks[0].replace('\r', "");
    if chunks.len() == 1 {
        let col = prefix.chars().count() + first_clean.chars().count();
        new.push(DocLine::Text(format!("{prefix}{first_clean}{suffix}")));
        let caret_after = Caret::new(lo.line, col);
        undo.push_lines(lo.line, old, new.clone(), *cursor, caret_after);
        lines.splice(lo.line..=hi.line, new);
        *cursor = caret_after;
        return;
    }

    let last_clean = chunks[chunks.len() - 1].replace('\r', "");

    let mut content = format!("{prefix}{first_clean}");
    for chunk in &chunks[1..chunks.len() - 1] {
        content.push('\n');
        content.push_str(&chunk.replace('\r', ""));
    }
    content.push('\n');
    content.push_str(&last_clean);
    content.push_str(&suffix);

    let new = crate::document_io::parse_doclines(&content);

    let caret_after = match new.last() {
        Some(DocLine::Grid(_)) => Caret::new(lo.line + new.len() - 1, 0),
        Some(DocLine::Text(s)) => {
            let suffix_chars = suffix.chars().count();
            Caret::new(lo.line + new.len() - 1, s.chars().count() - suffix_chars)
        }
        None => Caret::new(lo.line, 0),
    };
    undo.push_lines(lo.line, old, new.clone(), *cursor, caret_after);
    lines.splice(lo.line..=hi.line, new);
    *cursor = caret_after;
}

/// The range Copy/Cut operate on: the selection, or the whole current line
/// when nothing is selected.
fn copy_range(te: &TextEdit<'_>) -> (Caret, Caret) {
    te.selection_range()
        .unwrap_or_else(|| current_line_range(te.lines, *te.cursor))
}

fn current_line_range(lines: &[DocLine], cursor: Caret) -> (Caret, Caret) {
    let lo = Caret::new(cursor.line, 0);
    let hi = if cursor.line + 1 < lines.len() {
        Caret::new(cursor.line + 1, 0)
    } else {
        Caret::new(cursor.line, caret::line_char_len(lines, cursor.line))
    };
    (lo, hi)
}

fn update_selection(te: &mut TextEdit<'_>, shift: bool) {
    if shift {
        if te.selection_anchor.is_none() {
            *te.selection_anchor = Some(*te.cursor);
        }
    } else {
        *te.selection_anchor = None;
    }
}
