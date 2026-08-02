//! The eframe application: `UniformApp`, its panels and the background
//! pipeline that keeps the font and derived data fresh.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc;

use crate::document::{DocLine, Document, NamePartsMap};
use crate::document_io;
use crate::editor::EditorState;
use crate::editor::doc_links::LinkTargetKind;
use crate::editor::document_view::debounced_scroll_step;
use crate::editor::ref_composite::ResolvedGlyph;
use crate::issues::{Issue, collect_issues};
use crate::preview::widget::ShapedPreviewState;
use crate::render::SharedContourCache;
use crate::sidebar::{Sidebar, SidebarAction};
use crate::specimen::SpecimenState;

mod background;
mod docs;
mod history;
mod menus;
mod panels;
mod panes;
mod rename;
mod search;
mod toast;
mod watch;
mod zoom;

use background::BackgroundTaskStatus;
use docs::OpenDocument;
use history::{NavEntry, NavHistory, NavLoc};
use menus::{EditTarget, MenuActions, NavAction};
use panes::Panes;
use search::SearchResults;
use zoom::DEFAULT_PREVIEW_FONT_SIZE;

type FontPair = (Vec<u8>, Vec<u8>);
type FontBuildMessage = (u64, Option<crate::render::BuiltFontPair>);
/// What one derived-data rebuild produces. A struct rather than a tuple
/// because every consumer picks fields out of it by name.
struct DerivedDataMessage {
    build_gen: u64,
    named_glyphs: HashMap<String, ResolvedGlyph>,
    alt_index: crate::editor::ref_composite::AlternativesIndex,
    name_parts: NamePartsMap,
    meta: crate::meta::FontMetrics,
    issues: Vec<Issue>,
    /// Every face the source declares, in declaration order, for the face
    /// picker. Resolution already collects them, so nothing else has to.
    face_ids: Vec<String>,
}
type AssertResultMessage = Vec<Issue>;

pub struct UniformApp {
    font_dir: Option<PathBuf>,
    last_title: String,
    open_documents: Vec<OpenDocument>,
    /// The one or two editor panes and which of them has the focus. The
    /// focused pane's document is what "the active document" means everywhere
    /// else in the application.
    panes: Panes,
    /// Followed links, so they can be walked back and forward again. It spans
    /// files, which is why it lives here and not in an `EditorState`.
    nav_history: NavHistory,
    /// The Search pane's contents: the last name a Ctrl/Cmd+click had nothing
    /// to navigate to, and every place it appears. Spans files for the same
    /// reason the history does.
    search: Option<SearchResults>,
    sidebar: Sidebar,
    /// The sidebar panel's rect as of the last frame. The file watcher holds a
    /// listing refresh back while the pointer is over it, so that rows never
    /// move under a click; see [`watch`].
    sidebar_rect: egui::Rect,
    /// The OS watch on the font directory, and the changes it has reported.
    watch: watch::WatchState,
    /// Notices that outlive one status-bar line — currently only "this file
    /// changed on disk while you were editing it".
    toasts: toast::Toasts,
    escape_mode: bool,
    status_message: Option<(String, std::time::Instant)>,
    font_base_docs: Vec<Document>,
    /// The text each snapshot document was parsed from, and the hash of the
    /// bytes it came from. Written only where `font_base_docs` is, by
    /// [`UniformApp::install_font_snapshot`], so the two can never disagree
    /// about what the directory holds. This is what keeps a Ctrl/Cmd+click off
    /// the filesystem; see [`docs::FontSource`].
    font_sources: HashMap<PathBuf, docs::FontSource>,
    font_data: Option<FontPair>,
    font_name_to_gid: HashMap<String, u16>,
    font_applied: Option<bool>,
    font_data_gen: u64,
    last_font_gen: u64,
    font_rebuild_at: Option<std::time::Instant>,
    font_build_rx: mpsc::Receiver<FontBuildMessage>,
    font_build_tx: mpsc::Sender<FontBuildMessage>,
    font_build_gen: u64,
    contour_cache: SharedContourCache,
    /// Which face the editor builds. The editor never builds a collection —
    /// one face at a time — so this picks the one the preview, the specimen
    /// and the UI font itself are drawn with. Empty means the primary face,
    /// which is also what an id no longer declared falls back to.
    selected_face: String,
    /// Face ids as of the last derived-data rebuild; what the picker offers.
    face_ids: Vec<String>,
    named_glyphs: HashMap<String, ResolvedGlyph>,
    alt_index: crate::editor::ref_composite::AlternativesIndex,
    name_parts: NamePartsMap,
    color_aliases: crate::render::ttf_builder::ColorAliasMap,
    font_meta: crate::meta::FontMetrics,
    /// View menu: draw the metric box over every glyph grid.
    show_metrics: bool,
    named_glyphs_gen: u64,
    // Bumped whenever named_glyphs/name_parts/alt_index/color_aliases are
    // replaced; keys the editor's per-frame view cache.
    derived_gen: u64,
    derived_data_tx: mpsc::Sender<DerivedDataMessage>,
    derived_data_rx: mpsc::Receiver<DerivedDataMessage>,
    derived_rebuild_at: Option<std::time::Instant>,
    /// A derived-data rebuild thread is currently running. Without this
    /// guard the scheduler below respawns a rebuild every debounce period
    /// for as long as one is in flight — on machines where a resolve takes
    /// longer than the debounce, that snowballs into dozens of concurrent
    /// resolve threads starving each other (observed: 2s resolves stretching
    /// past 20s under the pile-up).
    derived_inflight: bool,
    last_export_path: Option<PathBuf>,
    close_confirmed: bool,
    bottom_panel_height: f32,
    bottom_panel_height_override: bool,
    bottom_panel_tab: Option<usize>,
    preview_font_size: f32,
    preview_font_size_slider: f32,
    /// Screen rect of the shaped preview as of the last frame, used to route
    /// Cmd/Ctrl + wheel to whichever surface the pointer is over. `None` when
    /// the preview tab is hidden. The editors' own rects live on their panes.
    preview_view_rect: Option<egui::Rect>,
    shaped_preview: ShapedPreviewState,
    specimen: SpecimenState,
    issues: Vec<Issue>,
    issues_gen: u64,
    file_parse_errors: Vec<(PathBuf, String)>,
    assert_issues: Vec<Issue>,
    assert_rx: mpsc::Receiver<AssertResultMessage>,
    assert_tx: mpsc::Sender<AssertResultMessage>,
    assert_running: bool,
    bg_tasks: BackgroundTaskStatus,
}

pub fn uniform_font_id(ctx: &egui::Context, size: f32) -> egui::FontId {
    let bitmap_family = egui::FontFamily::Name("UniformBitmap".into());
    let has_uniform = ctx.fonts(|f| f.families().contains(&bitmap_family));
    if !has_uniform {
        return egui::FontId::new(size, egui::FontFamily::Proportional);
    }
    let family = if size <= 16.0 {
        bitmap_family
    } else {
        egui::FontFamily::Name("UniformVector".into())
    };
    egui::FontId::new(size, family)
}

/// Whether `[perf]` stage timing logs are enabled (UNIFORM_PERF env var).
fn perf_log_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("UNIFORM_PERF").is_some())
}

impl UniformApp {
    /// The document of the focused pane, which is what the menus, the status
    /// bar and the window title mean by "active".
    fn active_doc_idx(&self) -> Option<usize> {
        self.panes.active_doc_idx()
    }

    fn active_doc(&self) -> Option<&OpenDocument> {
        self.active_doc_idx()
            .and_then(|i| self.open_documents.get(i))
    }

    fn active_doc_mut(&mut self) -> Option<&mut OpenDocument> {
        self.active_doc_idx()
            .and_then(|i| self.open_documents.get_mut(i))
    }

    /// The document shown by pane `idx`, if that pane has one.
    fn pane_doc_mut(&mut self, idx: usize) -> Option<&mut OpenDocument> {
        let doc_idx = self.panes.get(idx)?.doc_idx?;
        self.open_documents.get_mut(doc_idx)
    }

    /// Moves the focus onto whichever pane is showing an editor whose widget
    /// holds the keyboard focus. Run after the panes are laid out, so "the
    /// pane the focus was last in" is what the sidebar and the menus see.
    fn sync_pane_focus(&mut self) {
        for idx in 0..self.panes.len() {
            let active = self
                .panes
                .get(idx)
                .and_then(|p| p.doc_idx)
                .and_then(|d| self.open_documents.get(d))
                .is_some_and(|d| d.editor_state.is_active());
            if active {
                self.panes.set_focus(idx);
                return;
            }
        }
    }

    fn in_grid_edit(&self) -> bool {
        self.active_doc().is_some_and(|d| {
            matches!(
                d.editor_state.mode,
                crate::editor::EditMode::GlyphEdit { .. }
                    | crate::editor::EditMode::PixelSelect { .. }
            )
        })
    }

    pub fn new(cc: &eframe::CreationContext<'_>, font_dir: Option<PathBuf>) -> Self {
        // We map Cmd/Ctrl +/-/0 onto our own integral zoom level, so egui must not also
        // grab them for `pixels_per_point` scaling.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let (font_base_docs, file_parse_errors, font_sources) = font_dir
            .as_ref()
            .map(|d| crate::render::ttf_builder::load_docs_from_directory_with_sources(d))
            .unwrap_or_default();
        let font_sources = docs::font_sources_from(font_sources);

        let contour_cache = crate::render::new_contour_cache();
        let (font_data, font_name_to_gid) = if font_base_docs.is_empty() {
            (None, HashMap::new())
        } else {
            let refs: Vec<&Document> = font_base_docs.iter().collect();
            match crate::render::build_font_pair_cached(&refs, &contour_cache) {
                Some(built) => (Some((built.bitmap, built.vector)), built.name_to_gid),
                None => (None, HashMap::new()),
            }
        };

        let (font_build_tx, font_build_rx) = mpsc::channel();
        let (derived_data_tx, derived_data_rx) = mpsc::channel();
        let (assert_tx, assert_rx) = mpsc::channel();
        let mut app = Self {
            font_dir: font_dir.clone(),
            last_title: String::new(),
            open_documents: Vec::new(),
            panes: Panes::new(),
            nav_history: NavHistory::new(),
            search: None,
            sidebar: Sidebar::new(),
            sidebar_rect: egui::Rect::NOTHING,
            watch: watch::WatchState::new(),
            toasts: toast::Toasts::new(),
            escape_mode: false,
            status_message: None,
            font_base_docs,
            font_sources,
            font_data,
            font_name_to_gid,
            font_applied: None,
            font_data_gen: 0,
            last_font_gen: 0,
            font_rebuild_at: None,
            font_build_rx,
            font_build_tx,
            font_build_gen: 0,
            contour_cache,
            selected_face: String::new(),
            face_ids: Vec::new(),
            named_glyphs: HashMap::new(),
            alt_index: Default::default(),
            name_parts: NamePartsMap::new(),
            color_aliases: Default::default(),
            font_meta: Default::default(),
            show_metrics: true,
            named_glyphs_gen: u64::MAX,
            derived_gen: 0,
            derived_data_tx,
            derived_data_rx,
            derived_rebuild_at: None,
            derived_inflight: false,
            last_export_path: None,
            close_confirmed: false,
            bottom_panel_height: 0.0,
            bottom_panel_height_override: false,
            bottom_panel_tab: None,
            preview_font_size: DEFAULT_PREVIEW_FONT_SIZE,
            preview_font_size_slider: DEFAULT_PREVIEW_FONT_SIZE,
            preview_view_rect: None,
            shaped_preview: ShapedPreviewState::new(),
            specimen: SpecimenState::new(),
            issues: Vec::new(),
            issues_gen: u64::MAX,
            file_parse_errors,
            assert_issues: Vec::new(),
            assert_rx,
            assert_tx,
            assert_running: false,
            bg_tasks: BackgroundTaskStatus::new(),
        };

        if let Some(dir) = &font_dir {
            app.sidebar.set_directory(dir);
            app.watch.set_directory(dir, &cc.egui_ctx);
        }

        app
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    /// Carries out a link the editor followed and records it in the navigation
    /// history, so Go Back can undo the jump.
    fn follow_nav_request(
        &mut self,
        ctx: &egui::Context,
        from_doc: usize,
        nav: crate::editor::document_view::NavRequest,
    ) {
        use crate::editor::document_view::NavTarget;
        let from = NavLoc::new(from_doc, nav.from.line, nav.from.col);
        let to = match nav.target {
            // The editor already moved its own caret; only the record is left.
            NavTarget::Local { line } => Some(NavLoc::new(from_doc, line, 0)),
            NavTarget::CrossFile(goto) => {
                match self.goto_glyph(ctx, &goto.name, &goto.kind) {
                    Some((doc_idx, line)) => Some(NavLoc::new(doc_idx, line, 0)),
                    // Nothing declares the name, so there is no jump to make
                    // or to record — list who writes it instead.
                    None => {
                        self.search_name(ctx, &goto.name, goto.kind);
                        None
                    }
                }
            }
            // The token clicked is the declaration itself.
            NavTarget::Search(goto) => {
                self.search_name(ctx, &goto.name, goto.kind);
                None
            }
        };
        if let Some(to) = to {
            self.nav_history.push(NavEntry { from, to });
        }
    }

    /// Walks the navigation history one step and moves the caret there.
    fn navigate_history(&mut self, ctx: &egui::Context, forward: bool) {
        let target = if forward {
            self.nav_history.go_forward()
        } else {
            self.nav_history.go_back()
        };
        let Some(loc) = target else { return };
        if self.open_documents.get(loc.doc_idx).is_none() {
            return;
        }
        self.panes.show_document(loc.doc_idx);
        let doc = &mut self.open_documents[loc.doc_idx];
        doc.editor_state.goto_caret(&doc.lines, loc.line, loc.col);
        self.focus_pane_editor(ctx);
    }

    /// Opens and reveals a link target that is not in the document the link was
    /// written in. Reports where it landed, as `(document index, line)`.
    fn goto_glyph(
        &mut self,
        _ctx: &egui::Context,
        name: &str,
        kind: &LinkTargetKind,
    ) -> Option<(usize, usize)> {
        use crate::document::{DocumentItem, GlyphName};
        use crate::editor::doc_links::find_link_target_in_doc;

        let target_path =
            {
                let all_docs = self.collect_all_docs();
                all_docs.iter().find_map(|doc| {
                    let has_match = match kind {
                    // An alias line defines the name it declares as much as a
                    // glyph block does — it is where "go to definition" has to
                    // land for a `ref` to an alias.
                    LinkTargetKind::Glyph => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Glyph { name: GlyphName(n), .. } if n == name)
                            || matches!(
                                item,
                                DocumentItem::GlyphAlias { name: GlyphName(n), .. } if n == name
                            )
                    }),
                    LinkTargetKind::NameParts => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::NameParts { name: n, .. } if n == name)
                    }),
                    LinkTargetKind::Remap => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Remap { feature: f, .. } if f == name)
                            || matches!(item, DocumentItem::RemapGroup { name: n, .. } if n == name)
                    }),
                    LinkTargetKind::Color => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Color { name: n, .. } if n == name)
                    }),
                    LinkTargetKind::Face => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Face { id, .. } if id == name)
                    }),
                    LinkTargetKind::Slice => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Slice { id, .. } if id == name)
                    }),
                    // Neither is declared anywhere in particular.
                    LinkTargetKind::Anchor | LinkTargetKind::Feature => false,
                };
                    has_match.then(|| doc.path.clone())
                })
            };

        let path = target_path?;

        self.open_file(path.clone());

        let idx = self
            .open_documents
            .iter()
            .position(|d| d.document.path == path)?;
        self.panes.show_document(idx);

        let doc = &mut self.open_documents[idx];
        let line_idx = find_link_target_in_doc(&doc.lines, name, kind)?;
        doc.editor_state.goto_line(line_idx);
        Some((idx, line_idx))
    }
}

impl eframe::App for UniformApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_window_title(ctx);

        let mut menu = MenuActions::default();
        // Collected here and acted on below: switching a face rebuilds the
        // font, and that must not run while the input lock is held.
        let mut face_step = 0isize;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F11) {
                face_step = 1;
            }
            if i.key_pressed(egui::Key::F10) {
                face_step = -1;
            }
            if i.key_pressed(egui::Key::F12) {
                self.escape_mode = !self.escape_mode;
                menu.escape_toggled = true;
            }
            if i.key_pressed(egui::Key::F6) {
                if i.modifiers.command || i.modifiers.ctrl {
                    menu.run_assert_all = true;
                } else {
                    menu.run_assert_file = true;
                }
            }
        });

        if face_step != 0 {
            self.step_face(face_step, ctx);
        }

        self.intercept_swap_panes_chord(ctx, &mut menu);
        self.handle_zoom_scroll(ctx);
        self.handle_zoom_keys(ctx);

        // Before the pipeline: a reload steps the documents' generations, and
        // this frame's rebuild scheduling should see them.
        self.pump_file_watch(ctx);

        self.pump_background_pipeline(ctx);

        let edit_target = if self.shaped_preview.is_focused() {
            EditTarget::Preview
        } else {
            EditTarget::Editor
        };
        let editor_focused = self
            .active_doc()
            .is_some_and(|d| d.editor_state.is_active());

        self.show_menu_bar(ctx, &mut menu, edit_target, editor_focused);
        if menu.run_assert_all {
            self.run_shape_assertions(ctx, false);
        } else if menu.run_assert_file {
            self.run_shape_assertions(ctx, true);
        }
        self.apply_file_menu_actions(ctx, &menu);

        self.show_sidebar_panel(ctx, editor_focused);

        self.show_status_bar(ctx);

        let bottom = self.show_bottom_panel(ctx);

        let (nav_request, rename_request, divider_closed_pane) = self.show_editor_panel(ctx);
        // Now that this frame's editors have run, "the pane the focus is in"
        // is up to date — everything below acts on that pane.
        self.sync_pane_focus();
        if let Some(pane) = divider_closed_pane {
            self.panes.set_focus(pane);
            self.close_focused_pane();
            self.focus_pane_editor(ctx);
        }
        if self.apply_pane_action(menu.pane_action) {
            self.focus_pane_editor(ctx);
        }

        if let Some((from_doc, nav)) = nav_request {
            self.follow_nav_request(ctx, from_doc, nav);
        }

        // A specimen click is not a link in a document, so there is no position
        // to come back to and nothing to record.
        if let Some(click) = bottom.specimen_click {
            self.goto_glyph(ctx, &click.name, &click.kind);
        }

        // A search hit *is* a jump between two places in the source, so it is
        // recorded — from wherever the caret was, since the pane is not a link.
        if let Some(hit_idx) = bottom.search_click {
            self.goto_search_hit(ctx, hit_idx);
        }

        // After the jump above, so a Go Back in the same frame as a click would
        // still see that jump.
        match menu.nav_action {
            Some(NavAction::Back) => self.navigate_history(ctx, false),
            Some(NavAction::Forward) => self.navigate_history(ctx, true),
            None => {}
        }

        if let Some((path, line)) = bottom.issue_click {
            self.open_file(path.clone());
            if let Some(idx) = self
                .open_documents
                .iter()
                .position(|d| d.document.path == path)
            {
                self.panes.show_document(idx);
                self.open_documents[idx].editor_state.goto_line(line);
            }
        }

        if let Some(rename) = rename_request {
            self.execute_rename(&rename);
        }

        self.apply_edit_menu_actions(ctx, edit_target, menu.take_edit_actions());

        // Last, so the notices are over the panes rather than under them.
        self.toasts.show(ctx);

        // Decide whether to close only after this frame's editor input has
        // updated the source buffer and dirty state.
        if menu.exit && self.confirm_close_and_maybe_save() {
            self.close_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if ctx.input(|i| i.viewport().close_requested())
            && !self.close_confirmed
            && !self.confirm_close_and_maybe_save()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
}

impl UniformApp {
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = if let Some(idx) = self.active_doc_idx() {
            let doc = &self.open_documents[idx];
            let path = doc.document.path.display().to_string();
            if doc.document.dirty {
                format!("{path}* - Uniform")
            } else {
                format!("{path} - Uniform")
            }
        } else if let Some(dir) = &self.font_dir {
            format!("{} - Uniform", dir.display())
        } else {
            "Uniform".to_string()
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    /// Runs a document mutation on the active document and flushes the line
    /// buffer back into the `Document` when it reports a change.
    fn with_active_doc_flush(&mut self, f: impl FnOnce(&mut OpenDocument) -> bool) {
        if let Some(doc) = self.active_doc_mut()
            && f(doc)
        {
            crate::editor::document_view::flush_document_changes(
                &mut doc.lines,
                &mut doc.document,
                &mut doc.editor_state,
            );
        }
    }
}
