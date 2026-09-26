//! The menu bar and the actions it produces.

use super::commands::{Command, CommandCx};
use super::panes::{PaneAction, SplitSide};
use super::*;
use crate::edit_menu::EditAction;
use crate::editor::pixel_selection::SelectionTransform;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum EditTarget {
    Editor,
    Preview,
}

pub(super) enum SelMenuAction {
    Cancel,
    Transform(crate::editor::pixel_selection::SelectionTransform),
}

/// A step through the go-to-symbol history.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum NavAction {
    Back,
    Forward,
}

/// Everything the menu bar (and its keyboard accelerators) requested this
/// frame; dispatched after the panels have run.
#[derive(Default)]
pub(super) struct MenuActions {
    pub(super) new_file: bool,
    pub(super) open_folder: bool,
    pub(super) rename_file: bool,
    /// File ▸ Refresh filesystem (F5): scan the font directory now.
    pub(super) refresh_fs: bool,
    /// Edit ▸ Go to symbol: follow the link under the caret, as Ctrl/Cmd+`]`
    /// and a Ctrl/Cmd+click both do.
    pub(super) goto_symbol: bool,
    pub(super) rename_symbol: bool,
    pub(super) type_codepoint: bool,
    pub(super) export: bool,
    pub(super) export_new: bool,
    pub(super) exit: bool,
    pub(super) save: bool,
    pub(super) save_all: bool,
    pub(super) escape_toggled: bool,
    pub(super) run_assert_all: bool,
    pub(super) run_assert_file: bool,
    /// Font ▸ Optimize clearance: the `uniform fix --optimize-clearance` run.
    pub(super) optimize_clearance: bool,
    pub(super) edit_action: crate::edit_menu::EditAction,
    pub(super) sel_menu_action: Option<SelMenuAction>,
    pub(super) scale_action: Option<u8>,
    /// Split/swap/close, dispatched after the panes are laid out so it acts on
    /// the pane the focus is actually in this frame.
    pub(super) pane_action: PaneAction,
    /// Go back / go forward through the followed-link history.
    pub(super) nav_action: Option<NavAction>,
    /// Fold or unfold the group the caret is in. The shortcut itself is the
    /// editor's own (`document_view::keys`), so this carries only the menu.
    pub(super) toggle_fold: bool,
    /// Comment the selected lines out, or take the comment back off. Like
    /// `toggle_fold`, the chord belongs to the editor and this is the menu
    /// half alone.
    pub(super) toggle_comment: bool,
    /// Edit ▸ Find: reveal the search pane with its box focused, on the kind
    /// the entry names (`true` for glyph names). The menu half of Ctrl/Cmd+F.
    pub(super) find: Option<bool>,
    /// Edit ▸ Find next/previous: the menu half of Ctrl/Cmd+G, `true` forward.
    pub(super) find_step: Option<bool>,
    /// Edit ▸ Command palette: open it on the next frame, the chord being read
    /// at the top of this one.
    pub(super) palette: bool,
}

/// The subset of [`MenuActions`] dispatched after the central panel.
pub(super) struct EditMenuActions {
    edit_action: crate::edit_menu::EditAction,
    sel_menu_action: Option<SelMenuAction>,
    scale_action: Option<u8>,
}

impl MenuActions {
    pub(super) fn take_edit_actions(&mut self) -> EditMenuActions {
        EditMenuActions {
            edit_action: std::mem::take(&mut self.edit_action),
            sel_menu_action: self.sel_menu_action.take(),
            scale_action: self.scale_action.take(),
        }
    }
}

/// Reads the swap-panes chord (Cmd/Ctrl+Alt+X) off the event queue, removing
/// the event it arrived as.
///
/// It cannot be read as a key press, and the gate that decides that is not
/// ours to move: `egui-winit`'s `is_cut_command` matches any `command` + X
/// *regardless of alt*, pushes `Event::Cut` and returns without ever emitting
/// the `Event::Key`. `egui-winit` reaches us through `eframe` from crates.io,
/// so short of patching a fork of the egui workspace, the queue is the
/// earliest place we own. Leave the event in it and the focused editor obeys
/// it, cutting the selection instead of the panes swapping.
///
/// Alt is what tells the two apart — a plain Cmd/Ctrl+X carries no alt — and
/// the modifier test mirrors the other pane accelerators so that Windows'
/// Shift+Delete cut is not caught here either.
fn take_swap_cut_event(events: &mut Vec<egui::Event>, modifiers: egui::Modifiers) -> bool {
    if !(modifiers.command && modifiers.alt && !modifiers.shift) {
        return false;
    }
    let before = events.len();
    events.retain(|e| !matches!(e, egui::Event::Cut));
    events.len() != before
}

/// The key annotation the View menu's row for face `id` carries, given which
/// face is selected. The keys move the selection, so the row they annotate
/// moves with it; with two faces they both land on the same row.
pub(super) fn face_shortcut(faces: &[String], current: &str, id: &str) -> &'static str {
    let reaches = |delta: isize| {
        super::background::step_face_id(faces, current, delta).as_deref() == Some(id)
    };
    match (reaches(1), reaches(-1)) {
        (true, true) => "F10/F11",
        (true, false) => "F11",
        (false, true) => "F10",
        (false, false) => "",
    }
}

impl UniformApp {
    /// One menu entry, drawn from its [`Command`]: the label, the key, the
    /// enabled test, the checked fill, and on a click the command itself.
    fn command_item(
        &mut self,
        ui: &mut egui::Ui,
        cx: CommandCx,
        cmd: Command,
        menu: &mut MenuActions,
    ) {
        let label = cmd.label(self);
        // The font toggle is written in the font it switches to.
        let text: egui::WidgetText = match cmd {
            Command::ToggleUiFont => {
                let family = if self.escape_mode {
                    "UniformBitmap"
                } else {
                    "System"
                };
                egui::RichText::new(label)
                    .family(egui::FontFamily::Name(family.into()))
                    .into()
            }
            _ => label.into(),
        };
        let mut button = egui::Button::new(text);
        let shortcut = cmd.shortcut(self);
        if !shortcut.is_empty() {
            button = button.shortcut_text(shortcut);
        }
        if cmd.checked(self, ui.ctx()) {
            button = button.fill(ui.visuals().selection.bg_fill);
        }
        if ui.add_enabled(cmd.enabled(self, cx), button).clicked() {
            let ctx = ui.ctx().clone();
            cmd.apply(self, &ctx, menu);
            ui.close_menu();
        }
    }

    /// Menu entries in order, with a separator wherever `entries` holds `None`.
    fn command_items(
        &mut self,
        ui: &mut egui::Ui,
        cx: CommandCx,
        entries: &[Option<Command>],
        menu: &mut MenuActions,
    ) {
        for entry in entries {
            match entry {
                Some(cmd) => self.command_item(ui, cx, *cmd, menu),
                None => {
                    ui.separator();
                }
            }
        }
    }

    /// The top menu bar plus its global keyboard accelerators; every request
    /// lands in `menu` for dispatch after the panels.
    ///
    /// What each entry is and does is its [`Command`]'s; this is the layout.
    pub(super) fn show_menu_bar(
        &mut self,
        ctx: &egui::Context,
        menu: &mut MenuActions,
        edit_target: EditTarget,
        editor_focused: bool,
    ) {
        use Command::*;
        use SelectionTransform as T;
        let cx = CommandCx {
            edit_target,
            editor_focused,
        };

        // Whether any of the bar's menus is showing its contents this frame.
        // `menu_button` returns `Some` inner exactly then — including on the
        // frame the button was clicked, which is the frame that matters: the
        // click takes the keyboard focus away from the editor, and the editor
        // is drawn *after* this panel. See `UniformApp::menu_open`.
        let mut any_menu_open = false;

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                let file_open = ui.menu_button("File", |ui| {
                    self.command_items(
                        ui,
                        cx,
                        &[
                            Some(NewFile),
                            None,
                            Some(OpenFolder),
                            None,
                            Some(Save),
                            Some(SaveAll),
                            None,
                            Some(RenameFile),
                            Some(RefreshFilesystem),
                            None,
                            Some(ExportToLast),
                            Some(ExportToNew),
                            None,
                            Some(Exit),
                        ],
                        menu,
                    );
                });
                any_menu_open |= file_open.inner.is_some();
                let edit_open = ui.menu_button("Edit", |ui| {
                    for (action, separated) in EditAction::MENU {
                        if separated {
                            ui.separator();
                        }
                        self.command_item(ui, cx, Edit(action), menu);
                    }
                    self.command_items(
                        ui,
                        cx,
                        &[
                            None,
                            Some(GoBack),
                            Some(GoForward),
                            None,
                            Some(ToggleFold),
                            Some(ToggleComment),
                            None,
                            Some(FindText),
                            Some(FindGlyph),
                            Some(FindNext),
                            Some(FindPrevious),
                            Some(Palette),
                            None,
                            Some(GotoSymbol),
                            Some(RenameSymbol),
                            Some(TypeCodepoint),
                            None,
                            Some(SelectionMode),
                            Some(DrawingMode),
                            None,
                        ],
                        menu,
                    );
                    let can_scale = self.current_scale().is_some();
                    ui.add_enabled_ui(can_scale, |ui| {
                        ui.menu_button("Adjust scale", |ui| {
                            for s in 1u8..=10 {
                                self.command_item(ui, cx, AdjustScale(s), menu);
                            }
                        });
                    });
                });
                any_menu_open |= edit_open.inner.is_some();
                let sel_open = ui.menu_button("Selection", |ui| {
                    self.command_items(
                        ui,
                        cx,
                        &[
                            Some(CancelSelection),
                            None,
                            Some(Transform(T::MirrorH)),
                            Some(Transform(T::FlipV)),
                        ],
                        menu,
                    );
                    ui.menu_button("Rotate selection", |ui| {
                        for t in [T::RotateCCW, T::Rotate180, T::RotateCW] {
                            self.command_item(ui, cx, Transform(t), menu);
                        }
                    });
                    self.command_items(
                        ui,
                        cx,
                        &[
                            None,
                            Some(Transform(T::Opposite)),
                            Some(Transform(T::OppositeBitmap)),
                        ],
                        menu,
                    );
                });
                any_menu_open |= sel_open.inner.is_some();
                let font_open = ui.menu_button("Font", |ui| {
                    // Below the separator: everything that rewrites the source
                    // (`uniform fix`), as against the checks above it.
                    self.command_items(
                        ui,
                        cx,
                        &[
                            Some(RunAssertionsInFile),
                            Some(RunAssertionsInAll),
                            None,
                            Some(OptimizeClearance),
                        ],
                        menu,
                    );
                });
                any_menu_open |= font_open.inner.is_some();
                let view_open = ui.menu_button("View", |ui| {
                    self.command_items(
                        ui,
                        cx,
                        &[
                            Some(SplitPane(SplitSide::Left)),
                            Some(SplitPane(SplitSide::Right)),
                            Some(FocusPane(SplitSide::Left)),
                            Some(FocusPane(SplitSide::Right)),
                            Some(SwapPanes),
                            Some(ClosePane),
                            None,
                            Some(ClosePanels),
                            None,
                            Some(ShowTab(0)),
                            Some(ShowTab(1)),
                            Some(ShowTab(2)),
                            None,
                        ],
                        menu,
                    );
                    // A source with one face has nothing to pick, and its face
                    // id is the empty implicit one — unnameable in a menu.
                    if self.face_ids.len() > 1 {
                        ui.menu_button("Face", |ui| {
                            for i in 0..self.face_ids.len() {
                                self.command_item(ui, cx, Face(i), menu);
                            }
                        });
                    }
                    self.command_items(
                        ui,
                        cx,
                        &[
                            Some(ToggleUiFont),
                            None,
                            Some(ShowMetrics),
                            None,
                            Some(ZoomIn),
                            Some(ZoomOut),
                            Some(ResetZoom),
                            None,
                            Some(StartupTiming),
                            Some(RebuildTiming),
                            None,
                        ],
                        menu,
                    );
                    ui.menu_button("Color Scheme", |ui| {
                        for theme in [
                            egui::ThemePreference::System,
                            egui::ThemePreference::Dark,
                            egui::ThemePreference::Light,
                        ] {
                            self.command_item(ui, cx, Theme(theme), menu);
                        }
                    });
                });
                any_menu_open |= view_open.inner.is_some();
            });
        });
        self.menu_open = any_menu_open;

        ctx.input(|i| {
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::N) {
                menu.new_file = true;
            }
            if i.modifiers.command
                && i.modifiers.shift
                && i.key_pressed(egui::Key::O)
                && !self.in_grid_edit()
            {
                menu.open_folder = true;
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::S) {
                menu.save = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S) {
                menu.save_all = true;
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::E) {
                menu.export = true;
            }
            // Pane commands. The arrows carry Alt as well as Cmd/Ctrl because
            // bare Alt + arrow is word-wise cursor movement on macOS. Swap
            // (Cmd/Ctrl+Alt+X) is not here: it never arrives as a key press,
            // see `take_swap_cut_event` below.
            if i.modifiers.command && !i.modifiers.shift {
                if i.key_pressed(egui::Key::W) {
                    menu.pane_action = PaneAction::Close;
                }
                if i.modifiers.alt {
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        menu.pane_action = PaneAction::Split(SplitSide::Left);
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        menu.pane_action = PaneAction::Split(SplitSide::Right);
                    }
                    // Moving the focus between panes, vim-style.
                    if i.key_pressed(egui::Key::H) {
                        menu.pane_action = PaneAction::Focus(SplitSide::Left);
                    }
                    if i.key_pressed(egui::Key::L) {
                        menu.pane_action = PaneAction::Focus(SplitSide::Right);
                    }
                }
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::E) {
                menu.export_new = true;
            }
            // Go back / go forward through followed links. Both are dispatched
            // even with nothing to go to; the history just reports there is no
            // step to take.
            if i.modifiers.command && i.key_pressed(egui::Key::T) {
                menu.nav_action = Some(if i.modifiers.shift {
                    NavAction::Forward
                } else {
                    NavAction::Back
                });
            }
            if cfg!(target_os = "macos") {
                if i.modifiers.command && i.key_pressed(egui::Key::Q) {
                    menu.exit = true;
                }
            } else if i.modifiers.alt && i.key_pressed(egui::Key::F4) {
                menu.exit = true;
            }
            if i.modifiers.command && !i.modifiers.shift {
                for (key, tab) in [
                    (egui::Key::Num1, 0),
                    (egui::Key::Num2, 1),
                    (egui::Key::Num3, 2),
                ] {
                    if i.key_pressed(key) {
                        self.open_bottom_panel(tab, i.screen_rect.height());
                    }
                }
                if i.key_pressed(egui::Key::Backtick) {
                    self.bottom_panel_tab = None;
                }
            }
        });
    }

    /// Takes the swap-panes chord out of the input queue before anything else
    /// reads it. Runs with the other input-queue rewriting at the top of the
    /// frame, since the event has to be gone before the editors are drawn.
    pub(super) fn intercept_swap_panes_chord(
        &mut self,
        ctx: &egui::Context,
        menu: &mut MenuActions,
    ) {
        ctx.input_mut(|i| {
            let modifiers = i.modifiers;
            if take_swap_cut_event(&mut i.events, modifiers) {
                menu.pane_action = PaneAction::Swap;
            }
        });
    }

    /// Dispatches the file-level menu requests (and the escape-font toggle)
    /// before the panels are laid out.
    pub(super) fn apply_file_menu_actions(&mut self, ctx: &egui::Context, menu: &MenuActions) {
        if menu.new_file && self.font_dir.is_some() {
            self.sidebar.start_new_file();
        }

        // The scan itself is started by `pump_file_watch`, which has already
        // run this frame — so a refresh asked for through the menu begins on
        // the next one, and the repaint `request_filesystem_refresh` asks for
        // is what brings it.
        if menu.refresh_fs {
            self.request_filesystem_refresh(ctx);
        }

        if menu.open_folder
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
            && self.confirm_close_and_maybe_save()
        {
            self.font_dir = Some(dir.clone());
            self.open_documents.clear();
            // The pane layout is not carried across folders: its documents
            // are gone, and pane indices would dangle. The navigation
            // history indexes the same list, so it goes with them. The zoom
            // level is a view preference rather than part of that layout, so
            // it does carry.
            self.panes = Panes::new_with_zoom(self.panes.focused().zoom_level);
            self.nav_history.clear();
            // Its hits name files that are no longer the ones on screen.
            self.search = SearchState::default();
            self.sidebar.set_directory(&dir);
            self.watch.set_directory(&dir, ctx);
            // Through the watch's (just cleared) cache, so the first refresh
            // in this folder compares against what was read here.
            let (base_docs, parse_errors, sources) = self.watch.load_directory(&dir);
            self.install_font_snapshot(base_docs, parse_errors, sources);
            // The faces of the old folder mean nothing in the new one. This
            // folder's own last face is applied straight away, from a scan of
            // its `face` lines rather than from a resolve — exactly as at
            // startup, and for the same reason: a face applied later is a
            // second full build.
            self.face_ids = {
                let refs: Vec<&Document> = self.font_base_docs.iter().map(|d| &**d).collect();
                crate::faces::FaceSet::collect(&refs)
                    .faces
                    .iter()
                    .map(|f| f.id.clone())
                    .collect()
            };
            self.selected_face = self
                .settings
                .face_for(&dir)
                .filter(|f| self.face_ids.iter().any(|id| id == f))
                .unwrap_or_default()
                .to_string();
            // Both background stages are still working on the folder that just
            // went away. Nothing they produce is wanted, and the font build in
            // particular holds the contour cache this thread is about to clear
            // — so it would be waited on rather than merely wasted.
            self.rebuild_cancel.cancel();
            self.contour_cache.lock().unwrap().clear();
            self.composite_grid_cache.lock().unwrap().clear();
            self.font_build_gen = self.font_build_gen.wrapping_add(1);
            // Neither the font nor the derived data is built here: a folder on
            // a share takes tens of seconds to build and resolve, and doing it
            // on this thread is the freeze that startup no longer has. The
            // pipeline picks both up on the next frame, and until it does this
            // folder looks like a directory whose first build has not landed —
            // which is exactly what it is.
            self.arm_initial_font_build();
            self.shaped_preview.invalidate_font(self.font_data_gen);
            // The old folder's derived data is *wrong* here rather than merely
            // stale, so it is dropped rather than left to be replaced.
            self.named_glyphs = Arc::default();
            self.resolved_gen = self.resolved_gen.wrapping_add(1);
            self.composite_seeds = Arc::default();
            self.alt_index = Default::default();
            self.name_parts = NamePartsMap::default();
            self.char_props = Default::default();
            self.color_aliases = Default::default();
            self.anchor_aligns = Default::default();
            self.font_meta = Default::default();
            self.issues.clear();
            // No resolve has run for this generation: what arms the derived-data
            // rebuild on the next pump.
            self.named_glyphs_gen = u64::MAX;
            self.issues_gen = u64::MAX;
            self.set_status(format!("Opened folder {}", dir.display()));
        }

        if menu.rename_file
            && let Some(doc) = self.active_doc()
        {
            let path = doc.document.path.clone();
            self.sidebar.start_rename(&path);
        }

        if menu.goto_symbol
            && let Some(doc) = self.active_doc_mut()
        {
            doc.editor_state.request_goto_symbol();
        }

        if menu.rename_symbol
            && let Some(doc) = self.active_doc_mut()
        {
            doc.editor_state.start_rename_at_cursor(&doc.lines);
        }

        if menu.type_codepoint
            && let Some(doc) = self.active_doc_mut()
        {
            doc.editor_state.start_codepoint_entry(&doc.lines);
        }

        if menu.escape_toggled {
            self.font_applied = None;
        }
        self.apply_font(ctx);

        if menu.save {
            self.save_active();
        }
        if menu.save_all {
            // The message is the queue's to print: the files are still being
            // written when this returns. See [`super::save`].
            self.save_all();
        }

        if menu.export {
            if let Some(path) = self.last_export_path.clone() {
                self.export_to_path(path);
            } else {
                self.export_with_dialog();
            }
        }
        if menu.export_new {
            self.export_with_dialog();
        }
    }

    /// Dispatches the Edit/Selection menu requests after the central panel
    /// (so this frame's editor input has already been applied).
    pub(super) fn apply_edit_menu_actions(
        &mut self,
        ctx: &egui::Context,
        edit_target: EditTarget,
        actions: EditMenuActions,
    ) {
        use crate::edit_menu::EditAction;

        if actions.edit_action != EditAction::None {
            match edit_target {
                EditTarget::Preview => {
                    self.shaped_preview
                        .apply_edit_action(actions.edit_action, ctx);
                }
                EditTarget::Editor => {
                    self.with_active_doc_flush(|doc| {
                        doc.editor_state.apply_edit_action(
                            actions.edit_action,
                            &doc.document,
                            &mut doc.lines,
                            ctx,
                        )
                    });
                }
            }
        }

        if let Some(action) = actions.sel_menu_action {
            match action {
                SelMenuAction::Cancel => {
                    self.refocus_active_editor();
                    if let Some(doc) = self.active_doc_mut() {
                        if let Some(sel) = doc.editor_state.pixel_selection.clone() {
                            crate::editor::pixel_selection::commit_and_clear(
                                &doc.document,
                                &mut doc.lines,
                                &mut doc.editor_state,
                                &sel,
                            );
                        }
                        doc.editor_state.pixel_selection = None;
                    }
                }
                SelMenuAction::Transform(t) => {
                    self.with_active_doc_flush(|doc| {
                        crate::editor::pixel_selection::handle_transform_selection(
                            &doc.document,
                            &mut doc.lines,
                            &mut doc.editor_state,
                            t,
                        )
                    });
                }
            }
        }

        if let Some(new_scale) = actions.scale_action {
            self.with_active_doc_flush(|doc| {
                crate::editor::pixel_selection::handle_adjust_scale(
                    &doc.document,
                    &mut doc.lines,
                    &mut doc.editor_state,
                    new_scale,
                )
            });
        }
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    fn mods(command: bool, alt: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            alt,
            ctrl: command,
            shift,
            mac_cmd: command,
            command,
        }
    }

    /// The swap chord arrives as a cut, so it must be taken *and* removed:
    /// left in the queue, the focused editor cuts the selection instead.
    #[test]
    fn the_swap_chord_is_taken_out_of_the_queue() {
        let mut events = vec![egui::Event::Cut];
        assert!(take_swap_cut_event(&mut events, mods(true, true, false)));
        assert!(events.is_empty());
    }

    /// A plain Cmd/Ctrl+X is a real cut and must reach the editor untouched.
    #[test]
    fn a_plain_cut_is_left_alone() {
        let mut events = vec![egui::Event::Cut];
        assert!(!take_swap_cut_event(&mut events, mods(true, false, false)));
        assert_eq!(events.len(), 1);
        // Windows' Shift+Delete cut, with alt held for some other reason.
        let mut events = vec![egui::Event::Cut];
        assert!(!take_swap_cut_event(&mut events, mods(true, true, true)));
        assert_eq!(events.len(), 1);
    }

    /// Holding the chord's modifiers over unrelated input takes nothing.
    #[test]
    fn other_events_under_the_same_modifiers_are_untouched() {
        let mut events = vec![egui::Event::Copy, egui::Event::Paste("x".into())];
        assert!(!take_swap_cut_event(&mut events, mods(true, true, false)));
        assert_eq!(events.len(), 2);
    }
}

#[cfg(test)]
mod face_menu_tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The View menu annotates the faces the keys reach, so the annotation has
    /// to follow the selection around the list.
    #[test]
    fn the_key_annotations_follow_the_selection() {
        let faces = ids(&["a", "b", "c"]);
        assert_eq!(face_shortcut(&faces, "a", "b"), "F11");
        assert_eq!(face_shortcut(&faces, "a", "c"), "F10");
        assert_eq!(face_shortcut(&faces, "a", "a"), "");
        assert_eq!(face_shortcut(&faces, "b", "c"), "F11");
        assert_eq!(face_shortcut(&faces, "b", "a"), "F10");
    }

    /// With two faces both keys reach the same one, and the row says so rather
    /// than claiming only one of them works.
    #[test]
    fn two_faces_carry_both_keys_on_the_other_row() {
        let faces = ids(&["a", "b"]);
        assert_eq!(face_shortcut(&faces, "a", "b"), "F10/F11");
        assert_eq!(face_shortcut(&faces, "a", "a"), "");
    }
}
