//! Every entry of the menu bar as a value: the one list the menu bar draws and
//! the palette ([`super::palette`]) searches.
//!
//! A menu entry used to be a button spelled out inline — its label, its
//! shortcut text, its enabled test and what a click does, all in the closure
//! that drew it. That is fine for one reader and wrong for two: the palette
//! would have had to repeat all four, and the copies would drift. So an entry
//! is a [`Command`] and those four are its methods. `menus.rs` keeps what is
//! only the bar's — the order, the separators, the submenus — and the keyboard
//! accelerators, which are not one key per entry (some are read by the editor
//! itself) and stay where they are.
//!
//! **Adding a menu entry** is a variant, its arm in [`Command::label`],
//! [`Command::shortcut`], [`Command::enabled`] and [`Command::apply`], a
//! `command_item` call where it goes in the bar, and its place in
//! [`Command::all`] so that the palette offers it.
//!
//! [`Command::apply`] requests rather than performs wherever a click always
//! did: most commands set a [`MenuActions`] field and are carried out at the
//! point of the frame that field was always read at. A command picked in the
//! palette therefore takes exactly the path a click takes — see
//! [`super::palette`] for the one frame it waits first.

use super::menus::{EditTarget, MenuActions, NavAction, SelMenuAction};
use super::panes::{PaneAction, SplitSide};
use super::zoom::{
    DEFAULT_PREVIEW_FONT_SIZE, MAX_PREVIEW_FONT_SIZE, MAX_ZOOM_LEVEL, MIN_PREVIEW_FONT_SIZE,
    MIN_ZOOM_LEVEL, ZoomTarget, preview_font_step, zoom_step,
};
use super::*;
use crate::edit_menu::{EditAction, EditMenuCaps, platform_shortcut_names};
use crate::editor::pixel_selection::SelectionTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    // File
    NewFile,
    OpenFolder,
    Save,
    SaveAll,
    RenameFile,
    RefreshFilesystem,
    ExportToLast,
    ExportToNew,
    Exit,
    // Edit
    Edit(EditAction),
    GoBack,
    GoForward,
    ToggleFold,
    ToggleComment,
    FindText,
    FindGlyph,
    FindNext,
    FindPrevious,
    Palette,
    GotoSymbol,
    RenameSymbol,
    TypeCodepoint,
    SelectionMode,
    DrawingMode,
    AdjustScale(u8),
    // Selection
    CancelSelection,
    Transform(SelectionTransform),
    // Font
    RunAssertionsInFile,
    RunAssertionsInAll,
    OptimizeClearance,
    // View
    SplitPane(SplitSide),
    FocusPane(SplitSide),
    SwapPanes,
    ClosePane,
    ClosePanels,
    /// One of the bottom panel's tabs, by index.
    ShowTab(usize),
    /// One of the declared faces, by index into `face_ids`.
    Face(usize),
    ToggleUiFont,
    ShowMetrics,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    StartupTiming,
    RebuildTiming,
    Theme(egui::ThemePreference),
}

/// What the enabled tests read that is not a field of the app: which surface
/// the Edit entries act on, and whether an editor holds the keyboard.
///
/// Both are as of the moment the menu — or the palette — was opened. That is
/// the whole reason this is passed in rather than read: opening either takes
/// the keyboard away from whatever had it.
#[derive(Clone, Copy)]
pub(super) struct CommandCx {
    pub edit_target: EditTarget,
    pub editor_focused: bool,
}

/// The bottom panel's tabs the View menu lists, in its order.
const TAB_LABELS: [&str; 3] = ["Preview", "Specimen", "Issues"];

impl Command {
    /// Every command, in menu order. Faces are listed only for a source with
    /// more than one, exactly as the View menu offers them.
    pub(super) fn all(app: &UniformApp) -> Vec<Command> {
        use Command::*;
        use SelectionTransform as T;
        let mut all = vec![
            NewFile,
            OpenFolder,
            Save,
            SaveAll,
            RenameFile,
            RefreshFilesystem,
            ExportToLast,
            ExportToNew,
            Exit,
        ];
        all.extend(EditAction::MENU.iter().map(|&(action, _)| Edit(action)));
        all.extend([
            GoBack,
            GoForward,
            ToggleFold,
            ToggleComment,
            FindText,
            FindGlyph,
            FindNext,
            FindPrevious,
            Palette,
            GotoSymbol,
            RenameSymbol,
            TypeCodepoint,
            SelectionMode,
            DrawingMode,
        ]);
        all.extend((1..=10).map(AdjustScale));
        all.extend([
            CancelSelection,
            Transform(T::MirrorH),
            Transform(T::FlipV),
            Transform(T::RotateCCW),
            Transform(T::Rotate180),
            Transform(T::RotateCW),
            Transform(T::Opposite),
            Transform(T::OppositeBitmap),
            RunAssertionsInFile,
            RunAssertionsInAll,
            OptimizeClearance,
            SplitPane(SplitSide::Left),
            SplitPane(SplitSide::Right),
            FocusPane(SplitSide::Left),
            FocusPane(SplitSide::Right),
            SwapPanes,
            ClosePane,
            ClosePanels,
        ]);
        all.extend((0..TAB_LABELS.len()).map(ShowTab));
        if app.face_ids.len() > 1 {
            all.extend((0..app.face_ids.len()).map(Face));
        }
        all.extend([
            ToggleUiFont,
            ShowMetrics,
            ZoomIn,
            ZoomOut,
            ResetZoom,
            StartupTiming,
            RebuildTiming,
            Theme(egui::ThemePreference::System),
            Theme(egui::ThemePreference::Dark),
            Theme(egui::ThemePreference::Light),
        ]);
        all
    }

    /// The menu the entry is in.
    pub(super) fn menu(self) -> &'static str {
        use Command::*;
        match self {
            NewFile | OpenFolder | Save | SaveAll | RenameFile | RefreshFilesystem
            | ExportToLast | ExportToNew | Exit => "File",
            Edit(_) | GoBack | GoForward | ToggleFold | ToggleComment | FindText | FindGlyph
            | FindNext | FindPrevious | Palette | GotoSymbol | RenameSymbol | TypeCodepoint
            | SelectionMode | DrawingMode | AdjustScale(_) => "Edit",
            CancelSelection | Transform(_) => "Selection",
            RunAssertionsInFile | RunAssertionsInAll | OptimizeClearance => "Font",
            SplitPane(_) | FocusPane(_) | SwapPanes | ClosePane | ClosePanels | ShowTab(_)
            | Face(_) | ToggleUiFont | ShowMetrics | ZoomIn | ZoomOut | ResetZoom
            | StartupTiming | RebuildTiming | Theme(_) => "View",
        }
    }

    /// The submenu the entry is in, if it is not directly in its menu.
    pub(super) fn submenu(self) -> Option<&'static str> {
        use SelectionTransform as T;
        match self {
            Command::AdjustScale(_) => Some("Adjust scale"),
            Command::Transform(T::RotateCCW | T::Rotate180 | T::RotateCW) => {
                Some("Rotate selection")
            }
            Command::Face(_) => Some("Face"),
            Command::Theme(_) => Some("Color Scheme"),
            _ => None,
        }
    }

    /// The entry's text in its menu.
    pub(super) fn label(self, app: &UniformApp) -> String {
        use Command::*;
        use SelectionTransform as T;
        let text = match self {
            NewFile => "New file...",
            OpenFolder => "Open folder...",
            Save => "Save",
            SaveAll => "Save all",
            RenameFile => "Rename file...",
            RefreshFilesystem => "Refresh filesystem",
            ExportToLast => {
                let name = app.last_export_path.as_ref().map_or_else(
                    || "last font".to_string(),
                    |p| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "last font".into())
                    },
                );
                return format!("Export to {name}");
            }
            ExportToNew => "Export to new font...",
            Exit => "Exit",
            Edit(action) => action.label(),
            GoBack => "Go back",
            GoForward => "Go forward",
            ToggleFold => "Fold/unfold innermost group",
            ToggleComment => "Toggle line comment",
            FindText => "Find text...",
            FindGlyph => "Find glyph...",
            FindNext => "Find next",
            FindPrevious => "Find previous",
            Palette => "Command palette...",
            GotoSymbol => "Go to symbol",
            RenameSymbol => "Rename symbol...",
            TypeCodepoint => "Type code point...",
            SelectionMode => "Selection mode",
            DrawingMode => "Drawing mode",
            AdjustScale(s) if app.current_scale() == Some(s) => return format!("{s} ✓"),
            AdjustScale(s) => return s.to_string(),
            CancelSelection => "Cancel selection",
            Transform(T::MirrorH) => "Mirror selection",
            Transform(T::FlipV) => "Flip selection",
            Transform(T::RotateCCW) => "Counterclockwise",
            Transform(T::Rotate180) => "180 degrees",
            Transform(T::RotateCW) => "Clockwise",
            Transform(T::Opposite) => "Opposite subglyphs",
            Transform(T::OppositeBitmap) => "Opposite bitmap",
            RunAssertionsInFile => "Run assertions (current file)",
            RunAssertionsInAll => "Run assertions (all files)",
            OptimizeClearance => "Optimize clearance",
            SplitPane(SplitSide::Left) => "Split editor left",
            SplitPane(SplitSide::Right) => "Split editor right",
            FocusPane(SplitSide::Left) => "Focus editor pane left",
            FocusPane(SplitSide::Right) => "Focus editor pane right",
            SwapPanes => "Swap editor panes",
            ClosePane => "Close editor pane",
            ClosePanels => "Close panes",
            ShowTab(tab) => TAB_LABELS.get(tab).copied().unwrap_or_default(),
            Face(i) => return app.face_ids.get(i).cloned().unwrap_or_default(),
            ToggleUiFont if app.escape_mode => "Use dogfooded font",
            ToggleUiFont => "Use system font",
            ShowMetrics => "Show glyph metrics",
            ZoomIn => "Zoom in",
            ZoomOut => "Zoom out",
            ResetZoom => "Reset zoom",
            StartupTiming => "Startup timing\u{2026}",
            RebuildTiming => "Rebuild timing\u{2026}",
            Theme(egui::ThemePreference::System) => "System",
            Theme(egui::ThemePreference::Dark) => "Dark",
            Theme(egui::ThemePreference::Light) => "Light",
        };
        text.to_string()
    }

    /// The whole path of the entry, `Menu: Submenu: Label` — what the palette
    /// lists and matches, so a query can name the menu as well as the entry.
    pub(super) fn title(self, app: &UniformApp) -> String {
        match self.submenu() {
            Some(sub) => format!("{}: {sub}: {}", self.menu(), self.label(app)),
            None => format!("{}: {}", self.menu(), self.label(app)),
        }
    }

    /// The key annotation beside the entry; empty for one with no key.
    pub(super) fn shortcut(self, app: &UniformApp) -> String {
        use Command::*;
        use SelectionTransform as T;
        let (m, shift) = platform_shortcut_names();
        let alt = if cfg!(target_os = "macos") {
            "\u{2325}"
        } else {
            "Alt+"
        };
        match self {
            NewFile => format!("{m}N"),
            OpenFolder => format!("{m}{shift}O"),
            Save => format!("{m}S"),
            SaveAll => format!("{m}{shift}S"),
            RenameFile | RenameSymbol => "F2".into(),
            RefreshFilesystem => "F5".into(),
            ExportToLast => format!("{m}E"),
            ExportToNew => format!("{m}{shift}E"),
            Exit if cfg!(target_os = "macos") => "⌘Q".into(),
            Exit => "Alt+F4".into(),
            Edit(action) => action.shortcut(),
            GoBack => format!("{m}T"),
            GoForward => format!("{m}{shift}T"),
            ToggleFold => format!("{m};"),
            ToggleComment => format!("{m}/"),
            FindText => format!("{m}F"),
            FindGlyph => format!("{m}{shift}F"),
            FindNext => format!("{m}G"),
            FindPrevious => format!("{m}{shift}G"),
            Palette => format!("{m}P"),
            GotoSymbol => format!("{m}]"),
            TypeCodepoint => "Ctrl+K".into(),
            SelectionMode => "`".into(),
            DrawingMode => "1".into(),
            CancelSelection => "Esc".into(),
            // The transforms answer to the bare letter as well as to the
            // Ctrl/Cmd chord (see `document_view::keys`); the menu names the
            // shorter of the two.
            Transform(T::MirrorH) => "M".into(),
            Transform(T::FlipV) => "I".into(),
            Transform(T::RotateCCW) => "J".into(),
            Transform(T::Rotate180) => "K".into(),
            Transform(T::RotateCW) => "L".into(),
            Transform(T::Opposite) => "O".into(),
            Transform(T::OppositeBitmap) => format!("{shift}O"),
            RunAssertionsInFile => "F6".into(),
            RunAssertionsInAll => format!("{m}F6"),
            SplitPane(SplitSide::Left) => format!("{m}{alt}\u{2190}"),
            SplitPane(SplitSide::Right) => format!("{m}{alt}\u{2192}"),
            // Moving the focus is vim-keyed rather than arrow-keyed: the arrows
            // are taken by the splits above, and h/l leave j/k free for a
            // horizontal split should one ever land.
            FocusPane(SplitSide::Left) => format!("{m}{alt}H"),
            FocusPane(SplitSide::Right) => format!("{m}{alt}L"),
            SwapPanes => format!("{m}{alt}X"),
            ClosePane => format!("{m}W"),
            ClosePanels => format!("{m}`"),
            ShowTab(tab) => format!("{m}{}", tab + 1),
            // The keys annotate the faces they actually reach, so the
            // annotation moves with the selection rather than sitting on a
            // fixed "next/previous" entry.
            Face(i) => app.face_ids.get(i).map_or_else(String::new, |id| {
                super::menus::face_shortcut(&app.face_ids, app.selected_face(), id).into()
            }),
            ToggleUiFont => "F12".into(),
            ZoomIn => format!("{m}="),
            ZoomOut => format!("{m}-"),
            ResetZoom => format!("{m}0"),
            AdjustScale(_) | OptimizeClearance | ShowMetrics | StartupTiming | RebuildTiming
            | Theme(_) => String::new(),
        }
    }

    /// Whether the entry can do anything right now.
    pub(super) fn enabled(self, app: &UniformApp, cx: CommandCx) -> bool {
        use Command::*;
        let has_active = app.active_doc_idx().is_some();
        match self {
            NewFile | OpenFolder | SaveAll | ExportToNew | Exit => true,
            Save => has_active,
            // F2 is the editor's rename-symbol while it holds the keyboard.
            RenameFile => has_active && !cx.editor_focused,
            RefreshFilesystem => app.font_dir.is_some(),
            ExportToLast => app.last_export_path.is_some(),
            Edit(action) => action.enabled(&app.edit_caps(cx.edit_target)),
            GoBack => app.nav_history.can_go_back(),
            GoForward => app.nav_history.can_go_forward(),
            ToggleFold | ToggleComment | GotoSymbol | RenameSymbol | TypeCodepoint => {
                cx.editor_focused
            }
            // Always enabled: the box is where the reader types, and it is
            // reachable whether or not an editor holds the keyboard.
            FindText | FindGlyph | Palette => true,
            FindNext | FindPrevious => !app.search.hits().is_empty(),
            SelectionMode | DrawingMode => app.in_grid_edit(),
            AdjustScale(s) => app.current_scale().is_some_and(|current| current != s),
            CancelSelection => app
                .active_doc()
                .is_some_and(|d| d.editor_state.pixel_selection.is_some()),
            Transform(t) => {
                app.in_grid_edit()
                    && app.active_doc().is_some_and(|d| {
                        crate::editor::pixel_selection::can_transform(
                            &d.document,
                            &d.editor_state,
                            t,
                        )
                    })
            }
            RunAssertionsInFile => !app.assert_running && has_active,
            RunAssertionsInAll => !app.assert_running,
            OptimizeClearance => !app.fix_running,
            // Splitting is only offered from a single pane that has a
            // document: from a placeholder it would leave two of them, and
            // there is no third pane.
            SplitPane(_) => app.panes.can_split(),
            FocusPane(side) => app.panes.can_focus_side(side),
            SwapPanes => app.panes.can_swap(),
            ClosePane => app.panes.can_close(),
            ClosePanels | ShowTab(_) | Face(_) | ToggleUiFont | ShowMetrics | StartupTiming
            | RebuildTiming | Theme(_) => true,
            // The zoom entries drive whichever surface has the focus, and are
            // disabled outright when that is neither the editor nor the preview.
            ZoomIn | ZoomOut | ResetZoom => {
                let level = app.focused_zoom_level();
                let size = app.preview_font_size;
                match (self, app.focused_zoom_target()) {
                    (ZoomIn, ZoomTarget::Editor(_)) => level < MAX_ZOOM_LEVEL,
                    (ZoomOut, ZoomTarget::Editor(_)) => level > MIN_ZOOM_LEVEL,
                    (_, ZoomTarget::Editor(_)) => level != 1,
                    (ZoomIn, ZoomTarget::Preview) => size < MAX_PREVIEW_FONT_SIZE,
                    (ZoomOut, ZoomTarget::Preview) => size > MIN_PREVIEW_FONT_SIZE,
                    (_, ZoomTarget::Preview) => size != DEFAULT_PREVIEW_FONT_SIZE,
                    (_, ZoomTarget::None) => false,
                }
            }
        }
    }

    /// Whether the entry is drawn as the current choice of its group.
    pub(super) fn checked(self, app: &UniformApp, ctx: &egui::Context) -> bool {
        match self {
            Command::ShowTab(tab) => app.bottom_panel_tab == Some(tab),
            Command::Face(i) => app
                .face_ids
                .get(i)
                .is_some_and(|id| id == app.selected_face()),
            Command::ShowMetrics => app.show_metrics,
            Command::Theme(theme) => ctx.options(|o| o.theme_preference) == theme,
            _ => false,
        }
    }

    /// Carries the entry out — or, for everything a click on it always
    /// deferred, asks `menu` for it so the frame carries it out where it
    /// always did.
    pub(super) fn apply(self, app: &mut UniformApp, ctx: &egui::Context, menu: &mut MenuActions) {
        use Command::*;
        match self {
            NewFile => menu.new_file = true,
            OpenFolder => menu.open_folder = true,
            Save => menu.save = true,
            SaveAll => menu.save_all = true,
            RenameFile => menu.rename_file = true,
            RefreshFilesystem => menu.refresh_fs = true,
            ExportToLast => menu.export = true,
            ExportToNew => menu.export_new = true,
            Exit => menu.exit = true,
            Edit(action) => menu.edit_action = action,
            GoBack => menu.nav_action = Some(NavAction::Back),
            GoForward => menu.nav_action = Some(NavAction::Forward),
            ToggleFold => menu.toggle_fold = true,
            ToggleComment => menu.toggle_comment = true,
            FindText => menu.find = Some(false),
            FindGlyph => menu.find = Some(true),
            FindNext => menu.find_step = Some(true),
            FindPrevious => menu.find_step = Some(false),
            Palette => menu.palette = true,
            GotoSymbol => menu.goto_symbol = true,
            RenameSymbol => menu.rename_symbol = true,
            TypeCodepoint => menu.type_codepoint = true,
            SelectionMode => {
                if let Some(d) = app.active_doc_mut()
                    && let crate::editor::EditMode::GlyphEdit { item_idx, .. } = d.editor_state.mode
                {
                    d.editor_state.mode =
                        crate::editor::EditMode::pixel_select(item_idx, &d.editor_state.mode);
                    d.editor_state.refocus();
                }
            }
            DrawingMode => {
                if let Some(d) = app.active_doc_mut()
                    && let crate::editor::EditMode::PixelSelect { item_idx, .. } =
                        d.editor_state.mode
                {
                    d.editor_state.mode = crate::editor::EditMode::GlyphEdit {
                        item_idx,
                        selected_shape: crate::pixel::PixelShape::new(
                            crate::pixel::PX_ALMOSTFULL,
                            true,
                        ),
                    };
                    d.editor_state.refocus();
                }
            }
            AdjustScale(s) => menu.scale_action = Some(s),
            CancelSelection => menu.sel_menu_action = Some(SelMenuAction::Cancel),
            Transform(t) => menu.sel_menu_action = Some(SelMenuAction::Transform(t)),
            RunAssertionsInFile => menu.run_assert_file = true,
            RunAssertionsInAll => menu.run_assert_all = true,
            OptimizeClearance => menu.optimize_clearance = true,
            SplitPane(side) => menu.pane_action = PaneAction::Split(side),
            FocusPane(side) => menu.pane_action = PaneAction::Focus(side),
            SwapPanes => menu.pane_action = PaneAction::Swap,
            ClosePane => menu.pane_action = PaneAction::Close,
            ClosePanels => app.bottom_panel_tab = None,
            ShowTab(tab) => {
                let screen_h = ctx.input(|i| i.screen_rect.height());
                app.open_bottom_panel(tab, screen_h);
            }
            Face(i) => {
                if let Some(id) = app.face_ids.get(i).cloned() {
                    app.set_selected_face(id, ctx);
                }
            }
            ToggleUiFont => {
                app.escape_mode = !app.escape_mode;
                menu.escape_toggled = true;
            }
            ShowMetrics => app.show_metrics = !app.show_metrics,
            ZoomIn => app.zoom_focused(Some(1)),
            ZoomOut => app.zoom_focused(Some(-1)),
            ResetZoom => app.zoom_focused(None),
            // The only route to the startup report when the binary was
            // launched with no console to print it to; see `startup.rs`.
            StartupTiming => app.startup_timing_open = true,
            // What one *edit* costs, which is a different question and has a
            // report of its own; see `app::timing`.
            RebuildTiming => app.rebuild_timing_open = true,
            Theme(theme) => {
                ctx.set_theme(theme);
                // The UI font is re-applied for the new visuals.
                app.font_applied = None;
            }
        }
    }
}

impl UniformApp {
    /// What the Edit entries may do on `target`.
    pub(super) fn edit_caps(&self, target: EditTarget) -> EditMenuCaps {
        match target {
            EditTarget::Preview => self.shaped_preview.edit_menu_caps(),
            EditTarget::Editor => self
                .active_doc()
                .map(|d| d.editor_state.edit_menu_caps(&d.document))
                .unwrap_or(EditMenuCaps {
                    can_undo: false,
                    can_redo: false,
                    has_selection: false,
                    can_edit: false,
                }),
        }
    }

    /// The pixel scale the active glyph can be moved off, if it can be
    /// rescaled at all.
    pub(super) fn current_scale(&self) -> Option<u8> {
        self.active_doc().and_then(|d| {
            crate::editor::pixel_selection::can_adjust_scale(&d.document, &d.lines, &d.editor_state)
        })
    }

    /// One zoom step on whichever surface has the focus; `None` resets it.
    fn zoom_focused(&mut self, delta: Option<i32>) {
        match self.focused_zoom_target() {
            ZoomTarget::Editor(idx) => {
                let level = delta.map_or(1, |d| zoom_step(self.focused_zoom_level(), d));
                self.set_pane_zoom_level(idx, level);
            }
            ZoomTarget::Preview => {
                let size = delta.map_or(DEFAULT_PREVIEW_FONT_SIZE, |d| {
                    preview_font_step(self.preview_font_size, d)
                });
                self.set_preview_font_size(size);
            }
            ZoomTarget::None => {}
        }
    }
}
