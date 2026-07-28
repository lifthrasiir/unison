//! The menu bar and the actions it produces.

use super::*;
use super::panels::min_bottom_panel_height;
use super::panes::{PaneAction, SplitSide};
use super::zoom::{
    DEFAULT_PREVIEW_FONT_SIZE, MAX_PREVIEW_FONT_SIZE, MAX_ZOOM_LEVEL, MIN_PREVIEW_FONT_SIZE,
    MIN_ZOOM_LEVEL, ZoomTarget, preview_font_step, zoom_step
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum EditTarget {
    Editor,
    Preview,
}

enum SelMenuAction {
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
    new_file: bool,
    open_folder: bool,
    rename_file: bool,
    rename_symbol: bool,
    export: bool,
    export_new: bool,
    pub(super) exit: bool,
    pub(super) save: bool,
    pub(super) save_all: bool,
    pub(super) escape_toggled: bool,
    pub(super) run_assert_all: bool,
    pub(super) run_assert_file: bool,
    edit_action: crate::edit_menu::EditAction,
    sel_menu_action: Option<SelMenuAction>,
    scale_action: Option<u8>,
    /// Split/swap/close, dispatched after the panes are laid out so it acts on
    /// the pane the focus is actually in this frame.
    pub(super) pane_action: PaneAction,
    /// Go back / go forward through the followed-link history.
    pub(super) nav_action: Option<NavAction>,
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

impl UniformApp {
    /// The top menu bar plus its global keyboard accelerators; every request
    /// lands in `menu` for dispatch after the panels.
    pub(super) fn show_menu_bar(
        &mut self,
        ctx: &egui::Context,
        menu: &mut MenuActions,
        edit_target: EditTarget,
        editor_focused: bool,
    ) {
        let theme_before = ctx.options(|o| o.theme_preference);
        let (mod_name, shift_name) = crate::edit_menu::platform_shortcut_names();
        let exit_shortcut = if cfg!(target_os = "macos") { "⌘Q" } else { "Alt+F4" };

        let menu_new_file = &mut menu.new_file;
        let menu_open_folder = &mut menu.open_folder;
        let menu_rename = &mut menu.rename_file;
        let menu_rename_symbol = &mut menu.rename_symbol;
        let menu_export = &mut menu.export;
        let menu_export_new = &mut menu.export_new;
        let menu_exit = &mut menu.exit;
        let ctrl_s_pressed = &mut menu.save;
        let ctrl_shift_s_pressed = &mut menu.save_all;
        let escape_toggled = &mut menu.escape_toggled;
        let run_assert_all = &mut menu.run_assert_all;
        let run_assert_file = &mut menu.run_assert_file;
        let edit_action = &mut menu.edit_action;
        let sel_menu_action = &mut menu.sel_menu_action;
        let scale_action = &mut menu.scale_action;
        let pane_action = &mut menu.pane_action;
        let nav_action = &mut menu.nav_action;

        use crate::edit_menu::EditMenuCaps;

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("New file...").shortcut_text(format!("{mod_name}N"))).clicked() {
                        *menu_new_file = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Open folder...").shortcut_text(format!("{mod_name}{shift_name}O"))).clicked() {
                        *menu_open_folder = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    let has_active = self.active_doc_idx().is_some();
                    if ui
                        .add_enabled(has_active, egui::Button::new("Save").shortcut_text(format!("{mod_name}S")))
                        .clicked()
                    {
                        *ctrl_s_pressed = true;
                        ui.close_menu();
                    }
                    if ui
                        .add(egui::Button::new("Save all").shortcut_text(format!("{mod_name}{shift_name}S")))
                        .clicked()
                    {
                        *ctrl_shift_s_pressed = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has_active && !editor_focused, egui::Button::new("Rename file...").shortcut_text("F2"))
                        .clicked()
                    {
                        *menu_rename = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    let export_label = if let Some(ref p) = self.last_export_path {
                        format!(
                            "Export to {}",
                            p.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "last font".into()),
                        )
                    } else {
                        "Export to last font".into()
                    };
                    if ui
                        .add_enabled(
                            self.last_export_path.is_some(),
                            egui::Button::new(export_label)
                                .shortcut_text(format!("{mod_name}E")),
                        )
                        .clicked()
                    {
                        *menu_export = true;
                        ui.close_menu();
                    }
                    if ui
                        .add(
                            egui::Button::new("Export to new font...")
                                .shortcut_text(format!("{mod_name}{shift_name}E")),
                        )
                        .clicked()
                    {
                        *menu_export_new = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Exit").shortcut_text(exit_shortcut)).clicked() {
                        *menu_exit = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let caps = match edit_target {
                        EditTarget::Preview => self.shaped_preview.edit_menu_caps(),
                        EditTarget::Editor => {
                            self.active_doc()
                                .map(|d| d.editor_state.edit_menu_caps())
                                .unwrap_or(EditMenuCaps {
                                    can_undo: false,
                                    can_redo: false,
                                    has_selection: false,
                                    can_edit: false,
                                })
                        }
                    };
                    *edit_action = crate::edit_menu::show_edit_menu_items(ui, &caps, true);
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.nav_history.can_go_back(),
                            egui::Button::new("Go back").shortcut_text(format!("{mod_name}T")),
                        )
                        .clicked()
                    {
                        *nav_action = Some(NavAction::Back);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.nav_history.can_go_forward(),
                            egui::Button::new("Go forward")
                                .shortcut_text(format!("{mod_name}{shift_name}T")),
                        )
                        .clicked()
                    {
                        *nav_action = Some(NavAction::Forward);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(editor_focused, egui::Button::new("Rename symbol...").shortcut_text("F2"))
                        .clicked()
                    {
                        *menu_rename_symbol = true;
                        ui.close_menu();
                    }
                    let in_grid_edit = self.in_grid_edit();
                    ui.separator();
                    if ui
                        .add_enabled(in_grid_edit, egui::Button::new("Selection mode").shortcut_text("`"))
                        .clicked()
                    {
                        if let Some(d) = self.active_doc_mut() {
                            if let crate::editor::EditMode::GlyphEdit { item_idx, .. } = d.editor_state.mode {
                                d.editor_state.mode = crate::editor::EditMode::PixelSelect { item_idx };
                            }
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(in_grid_edit, egui::Button::new("Drawing mode").shortcut_text("1"))
                        .clicked()
                    {
                        if let Some(d) = self.active_doc_mut() {
                            if let crate::editor::EditMode::PixelSelect { item_idx } = d.editor_state.mode {
                                d.editor_state.mode = crate::editor::EditMode::GlyphEdit {
                                    item_idx,
                                    selected_shape: crate::pixel::PixelShape::new(
                                        crate::pixel::PX_ALMOSTFULL, true,
                                    ),
                                };
                            }
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    let current_scale = self.active_doc()
                        .and_then(|d| {
                            crate::editor::pixel_selection::can_adjust_scale(
                                &d.document, &d.lines, &d.editor_state,
                            )
                        });
                    ui.add_enabled_ui(current_scale.is_some(), |ui| {
                        ui.menu_button("Adjust scale", |ui| {
                            for s in 1u8..=10 {
                                let label = if current_scale == Some(s) {
                                    format!("{s} ✓")
                                } else {
                                    format!("{s}")
                                };
                                if ui.add_enabled(
                                    current_scale != Some(s),
                                    egui::Button::new(label),
                                ).clicked() {
                                    *scale_action = Some(s);
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                });
                ui.menu_button("Selection", |ui| {
                    let (mod_name, _) = crate::edit_menu::platform_shortcut_names();
                    let active_doc = self.active_doc();
                    let in_grid_mode = self.in_grid_edit();
                    let has_sel = active_doc.is_some_and(|d|
                        d.editor_state.pixel_selection.is_some()
                    );

                    use crate::editor::pixel_selection::{can_transform, SelectionTransform};

                    let can_do = |t: SelectionTransform| -> bool {
                        if !in_grid_mode { return false; }
                        if let Some(d) = active_doc {
                            return can_transform(&d.document, &d.editor_state, t);
                        }
                        false
                    };

                    if ui.add_enabled(
                        has_sel,
                        egui::Button::new("Cancel selection").shortcut_text("Esc"),
                    ).clicked() {
                        *sel_menu_action = Some(SelMenuAction::Cancel);
                        ui.close_menu();
                    }

                    ui.separator();

                    if ui.add_enabled(
                        can_do(SelectionTransform::MirrorH),
                        egui::Button::new("Mirror selection").shortcut_text(format!("{mod_name}M")),
                    ).clicked() {
                        *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::MirrorH));
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        can_do(SelectionTransform::FlipV),
                        egui::Button::new("Flip selection").shortcut_text(format!("{mod_name}I")),
                    ).clicked() {
                        *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::FlipV));
                        ui.close_menu();
                    }
                    ui.menu_button("Rotate selection", |ui| {
                        if ui.add_enabled(
                            can_do(SelectionTransform::RotateCCW),
                            egui::Button::new("Counterclockwise").shortcut_text(format!("{mod_name}J")),
                        ).clicked() {
                            *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::RotateCCW));
                            ui.close_menu();
                        }
                        if ui.add_enabled(
                            can_do(SelectionTransform::Rotate180),
                            egui::Button::new("180 degrees").shortcut_text(format!("{mod_name}K")),
                        ).clicked() {
                            *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::Rotate180));
                            ui.close_menu();
                        }
                        if ui.add_enabled(
                            can_do(SelectionTransform::RotateCW),
                            egui::Button::new("Clockwise").shortcut_text(format!("{mod_name}L")),
                        ).clicked() {
                            *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::RotateCW));
                            ui.close_menu();
                        }
                    });

                    ui.separator();

                    if ui.add_enabled(
                        can_do(SelectionTransform::Opposite),
                        egui::Button::new("Opposite subglyphs").shortcut_text(format!("{mod_name}O")),
                    ).clicked() {
                        *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::Opposite));
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        can_do(SelectionTransform::OppositeBitmap),
                        egui::Button::new("Opposite bitmap").shortcut_text(format!("{mod_name}\u{21e7}O")),
                    ).clicked() {
                        *sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::OppositeBitmap));
                        ui.close_menu();
                    }
                });
                ui.menu_button("Font", |ui| {
                    if ui.add_enabled(
                        !self.assert_running && self.active_doc_idx().is_some(),
                        egui::Button::new("Run assertions (current file)").shortcut_text("F6"),
                    ).clicked() {
                        *run_assert_file = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        !self.assert_running,
                        egui::Button::new("Run assertions (all files)").shortcut_text(format!("{mod_name}F6")),
                    ).clicked() {
                        *run_assert_all = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    // Splitting is only offered from a single pane that has a
                    // document: from a placeholder it would leave two of them,
                    // and there is no third pane.
                    let alt_name = if cfg!(target_os = "macos") { "\u{2325}" } else { "Alt+" };
                    for (side, label, key) in [
                        (SplitSide::Left, "Split editor left", "\u{2190}"),
                        (SplitSide::Right, "Split editor right", "\u{2192}"),
                    ] {
                        if ui
                            .add_enabled(
                                self.panes.can_split(),
                                egui::Button::new(label)
                                    .shortcut_text(format!("{mod_name}{alt_name}{key}")),
                            )
                            .clicked()
                        {
                            *pane_action = PaneAction::Split(side);
                            ui.close_menu();
                        }
                    }
                    if ui
                        .add_enabled(
                            self.panes.can_swap(),
                            egui::Button::new("Swap editor panes")
                                .shortcut_text(format!("{mod_name}{alt_name}X")),
                        )
                        .clicked()
                    {
                        *pane_action = PaneAction::Swap;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.panes.can_close(),
                            egui::Button::new("Close editor pane")
                                .shortcut_text(format!("{mod_name}W")),
                        )
                        .clicked()
                    {
                        *pane_action = PaneAction::Close;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Close panes").shortcut_text(format!("{mod_name}`"))).clicked() {
                        self.bottom_panel_tab = None;
                        ui.close_menu();
                    }
                    ui.separator();
                    for (tab, label) in [(0, "Preview"), (1, "Specimen"), (2, "Issues")] {
                        let selected = self.bottom_panel_tab == Some(tab);
                        let mut btn = egui::Button::new(label)
                            .shortcut_text(format!("{mod_name}{}", tab + 1));
                        if selected {
                            btn = btn.fill(ui.visuals().selection.bg_fill);
                        }
                        if ui.add(btn).clicked() {
                            self.bottom_panel_tab = Some(tab);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    let (font_label, preview_family) = if self.escape_mode {
                        ("Use dogfooded font", egui::FontFamily::Name("UniformBitmap".into()))
                    } else {
                        ("Use system font", egui::FontFamily::Name("System".into()))
                    };
                    let label = egui::RichText::new(font_label).family(preview_family);
                    if ui.add(egui::Button::new(label).shortcut_text("F12")).clicked() {
                        self.escape_mode = !self.escape_mode;
                        *escape_toggled = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    // The zoom entries drive whichever surface has the focus, and are
                    // disabled outright when that is neither the editor nor the preview.
                    let zoom_target = self.focused_zoom_target();
                    let editor_zoom = self.focused_zoom_level();
                    let (can_in, can_out, can_reset) = match zoom_target {
                        ZoomTarget::Editor(_) => (
                            editor_zoom < MAX_ZOOM_LEVEL,
                            editor_zoom > MIN_ZOOM_LEVEL,
                            editor_zoom != 1,
                        ),
                        ZoomTarget::Preview => (
                            self.preview_font_size < MAX_PREVIEW_FONT_SIZE,
                            self.preview_font_size > MIN_PREVIEW_FONT_SIZE,
                            self.preview_font_size != DEFAULT_PREVIEW_FONT_SIZE,
                        ),
                        ZoomTarget::None => (false, false, false),
                    };
                    if ui.add_enabled(
                        can_in,
                        egui::Button::new("Zoom in").shortcut_text(format!("{mod_name}=")),
                    ).clicked() {
                        match zoom_target {
                            ZoomTarget::Editor(idx) => {
                                self.set_pane_zoom_level(idx, zoom_step(editor_zoom, 1));
                            }
                            ZoomTarget::Preview => {
                                let size = preview_font_step(self.preview_font_size, 1);
                                self.set_preview_font_size(size);
                            }
                            ZoomTarget::None => {}
                        }
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        can_out,
                        egui::Button::new("Zoom out").shortcut_text(format!("{mod_name}-")),
                    ).clicked() {
                        match zoom_target {
                            ZoomTarget::Editor(idx) => {
                                self.set_pane_zoom_level(idx, zoom_step(editor_zoom, -1));
                            }
                            ZoomTarget::Preview => {
                                let size = preview_font_step(self.preview_font_size, -1);
                                self.set_preview_font_size(size);
                            }
                            ZoomTarget::None => {}
                        }
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        can_reset,
                        egui::Button::new("Reset zoom").shortcut_text(format!("{mod_name}0")),
                    ).clicked() {
                        match zoom_target {
                            ZoomTarget::Editor(idx) => {
                                self.set_pane_zoom_level(idx, 1);
                            }
                            ZoomTarget::Preview => {
                                self.set_preview_font_size(DEFAULT_PREVIEW_FONT_SIZE);
                            }
                            ZoomTarget::None => {}
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.menu_button("Color Scheme", |ui| {
                        if ui
                            .radio(theme_before == egui::ThemePreference::System, "System")
                            .clicked()
                        {
                            ctx.set_theme(egui::ThemePreference::System);
                            ui.close_menu();
                        }
                        if ui
                            .radio(theme_before == egui::ThemePreference::Dark, "Dark")
                            .clicked()
                        {
                            ctx.set_theme(egui::ThemePreference::Dark);
                            ui.close_menu();
                        }
                        if ui
                            .radio(theme_before == egui::ThemePreference::Light, "Light")
                            .clicked()
                        {
                            ctx.set_theme(egui::ThemePreference::Light);
                            ui.close_menu();
                        }
                    });
                });
            });
        });
        if ctx.options(|o| o.theme_preference) != theme_before {
            self.font_applied = None;
        }

        ctx.input(|i| {
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::N) {
                *menu_new_file = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::O) {
                if !self.in_grid_edit() {
                    *menu_open_folder = true;
                }
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::S) {
                *ctrl_s_pressed = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S) {
                *ctrl_shift_s_pressed = true;
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::E) {
                *menu_export = true;
            }
            // Pane commands. The arrows carry Alt as well as Cmd/Ctrl because
            // bare Alt + arrow is word-wise cursor movement on macOS. Swap
            // (Cmd/Ctrl+Alt+X) is not here: it never arrives as a key press,
            // see `take_swap_cut_event` below.
            if i.modifiers.command && !i.modifiers.shift {
                if i.key_pressed(egui::Key::W) {
                    *pane_action = PaneAction::Close;
                }
                if i.modifiers.alt {
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        *pane_action = PaneAction::Split(SplitSide::Left);
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        *pane_action = PaneAction::Split(SplitSide::Right);
                    }
                }
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::E) {
                *menu_export_new = true;
            }
            // Go back / go forward through followed links. Both are dispatched
            // even with nothing to go to; the history just reports there is no
            // step to take.
            if i.modifiers.command && i.key_pressed(egui::Key::T) {
                *nav_action = Some(if i.modifiers.shift {
                    NavAction::Forward
                } else {
                    NavAction::Back
                });
            }
            if cfg!(target_os = "macos") {
                if i.modifiers.command && i.key_pressed(egui::Key::Q) {
                    *menu_exit = true;
                }
            } else if i.modifiers.alt && i.key_pressed(egui::Key::F4) {
                *menu_exit = true;
            }
            if i.modifiers.command && !i.modifiers.shift {
                for (key, tab) in [
                    (egui::Key::Num1, 0),
                    (egui::Key::Num2, 1),
                    (egui::Key::Num3, 2),
                ] {
                    if i.key_pressed(key) {
                        self.bottom_panel_tab = Some(tab);
                        let min_h = min_bottom_panel_height(i.screen_rect.height());
                        if self.bottom_panel_height < min_h {
                            self.bottom_panel_height = min_h;
                            self.bottom_panel_height_override = true;
                        }
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

        if menu.open_folder
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
            && self.confirm_close_and_maybe_save() {
                self.font_dir = Some(dir.clone());
                self.open_documents.clear();
                // The pane layout is not carried across folders: its documents
                // are gone, and pane indices would dangle. The navigation
                // history indexes the same list, so it goes with them.
                self.panes = Panes::new();
                self.nav_history.clear();
                self.sidebar.set_directory(&dir);
                let (base_docs, parse_errors) = crate::render::ttf_builder::load_docs_from_directory_checked(&dir);
                self.font_base_docs = base_docs;
                self.file_parse_errors = parse_errors;
                let refs: Vec<&Document> = self.font_base_docs.iter().collect();
                self.contour_cache.lock().unwrap().clear();
                match crate::render::build_font_pair_cached(&refs, &self.contour_cache) {
                    Some((pair, gid_map)) => {
                        self.font_data = Some(pair);
                        self.font_name_to_gid = gid_map;
                    }
                    None => {
                        self.font_data = None;
                        self.font_name_to_gid.clear();
                    }
                }
                self.font_build_gen = self.font_build_gen.wrapping_add(1);
                self.font_data_gen = self.font_build_gen;
                self.font_applied = None;
                self.shaped_preview.invalidate_font(self.font_data_gen);
                self.font_rebuild_at = None;
                self.last_font_gen = self.current_font_gen();
                self.rebuild_named_glyphs_sync();
                self.named_glyphs_gen = self.font_build_gen;
                {
                    let all_docs = self.collect_all_docs();
                    self.issues = collect_issues(&all_docs);
                }
                self.issues_gen = self.font_build_gen;
                self.set_status(format!("Opened folder {}", dir.display()));
            }

        if menu.rename_file
            && let Some(doc) = self.active_doc() {
                let path = doc.document.path.clone();
                self.sidebar.start_rename(&path);
            }

        if menu.rename_symbol
            && let Some(doc) = self.active_doc_mut() {
                doc.editor_state.start_rename_at_cursor(&doc.lines);
            }

        if menu.escape_toggled {
            self.font_applied = None;
        }
        self.apply_font(ctx);

        if menu.save {
            self.save_active();
        }
        if menu.save_all
            && self.save_all() {
                self.set_status("Saved all files".to_string());
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
                    self.shaped_preview.apply_edit_action(actions.edit_action, ctx);
                }
                EditTarget::Editor => {
                    self.with_active_doc_flush(|doc| {
                        doc.editor_state.apply_edit_action(
                            actions.edit_action,
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
        egui::Modifiers { alt, ctrl: command, shift, mac_cmd: command, command }
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
