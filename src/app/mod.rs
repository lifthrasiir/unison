//! The eframe application: `UniformApp`, its panels and the background
//! pipeline that keeps the font and derived data fresh.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use crate::document::{DocLine, Document, NamePartsMap};
use crate::document_io;
use crate::editor::EditorState;
use crate::editor::doc_links::LinkTargetKind;
use crate::editor::document_view::debounced_scroll_step;
use crate::editor::ref_composite::ResolvedGlyph;
use crate::issues::Issue;
use crate::preview::widget::ShapedPreviewState;
use crate::render::SharedContourCache;
use crate::sidebar::{Sidebar, SidebarAction};
use crate::specimen::SpecimenState;

mod background;
mod docs;
mod fix;
mod history;
mod menus;
mod panels;
mod panes;
mod rename;
mod resize;
mod save;
mod search;
mod settings;
mod timing;
mod toast;
mod watch;
mod zoom;

use background::BackgroundTaskStatus;
use docs::OpenDocument;
use history::{NavEntry, NavHistory, NavLoc};
use menus::{EditTarget, MenuActions, NavAction};
use panes::Panes;
use search::SearchResults;
use settings::Settings;

type FontPair = (Vec<u8>, Vec<u8>);
/// How one font-build thread ended.
///
/// `Cancelled` is not an error and not a result: the build was told mid-flight
/// that its document set had been superseded, so it stopped and produced
/// nothing. It is reported anyway — rather than the thread just going quiet —
/// because the scheduler starts at most one build at a time and needs to know
/// the slot is free. See [`crate::cancel`].
enum FontBuildOutcome {
    /// The build ran to the end; `None` is a document set no font comes out of,
    /// or a worker that died on the way (see `background::ResultSlot`).
    Done(Option<crate::render::BuiltFontPair>),
    Cancelled,
}
type FontBuildMessage = (u64, FontBuildOutcome);
/// What one derived-data rebuild produces. A struct rather than a tuple
/// because every consumer picks fields out of it by name.
struct DerivedDataMessage {
    build_gen: u64,
    named_glyphs: HashMap<String, ResolvedGlyph>,
    alt_index: crate::editor::ref_composite::AlternativesIndex,
    name_parts: NamePartsMap,
    /// The first match of every `exists`, which is the one the editor draws a
    /// search-scoped block as. See [`crate::exists::FirstMatches`].
    exists_matches: crate::exists::FirstMatches,
    char_props: crate::ucd::CharProps,
    meta: crate::meta::FontMetrics,
    issues: Vec<Issue>,
    /// Which glyphs those issues are about, the specimen's cell backgrounds.
    glyph_flags: crate::glyph_flags::GlyphFlags,
    /// Every face the source declares, in declaration order, for the face
    /// picker. Resolution already collects them, so nothing else has to.
    face_ids: Vec<String>,
    /// What the specimen reads out of the documents, when its tab is open —
    /// a third full expansion, and the reason it is not done on the UI thread.
    /// See [`crate::specimen::SpecimenData`].
    specimen: Option<crate::specimen::SpecimenData>,
    /// What each stage of this rebuild cost, measured where it ran; see
    /// [`timing`].
    timing: timing::BackgroundTiming,
}
/// How one derived-data thread ended. `Failed` is a rebuild that died on the
/// way (see `background::ResultSlot`) and `Cancelled` one that was superseded
/// mid-resolve; both leave the previous derived data in place, since a stale
/// view of the font beats none, but only the first is worth telling the user
/// about.
enum DerivedDataResult {
    Done(Box<DerivedDataMessage>),
    Failed,
    Cancelled,
}
type AssertResultMessage = Vec<Issue>;
/// What one `fix` run produced, per document: the plan `crate::fix` made and
/// which document of the snapshot it was made against. Applying it is
/// `app::fix`'s job, on the UI thread, where the open documents are.
type FixResultMessage = Vec<crate::fix::clearance::DocumentFixes>;

pub struct UniformApp {
    /// What the last run left behind, and what this one will leave. Held for
    /// the whole run rather than only read at startup, because the per-
    /// directory face memory has to survive switching directories — the choice
    /// is recorded when it is made, not when the application quits.
    settings: Settings,
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
    /// Notices that outlive one status-bar line: what an external change did,
    /// and the sticky one saying what it is still waiting to do.
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
    /// When the debounced rebuild — the font *and* the derived data, which are
    /// one thread; see [`UniformApp::rebuild`] — is due.
    rebuild_at: Option<std::time::Instant>,
    /// What the last few rebuilds cost, stage by stage. See [`timing`].
    rebuild_log: timing::RebuildLog,
    /// *View → Rebuild timing…* is showing.
    rebuild_timing_open: bool,
    /// The generations the specimen's data was last *asked* for. Opening the
    /// tab asks for a rebuild, and this is what keeps it to one ask rather than
    /// one per frame.
    specimen_asked_for: Option<(u64, u64)>,
    font_build_rx: mpsc::Receiver<FontBuildMessage>,
    font_build_tx: mpsc::Sender<FontBuildMessage>,
    font_build_gen: u64,
    /// A rebuild thread is running. At most one ever is: a second would only
    /// queue behind the first on `contour_cache`, and it is that queue — one
    /// full build per click, each finishing long after its own edit was
    /// superseded — that made a burst of pixel edits take seconds to show. A
    /// rebuild arriving while one runs cancels it and re-arms `rebuild_at`
    /// instead. It is also what keeps a stage slower than the debounce from
    /// respawning itself every period: on machines where a resolve takes longer
    /// than the debounce that used to snowball into dozens of concurrent
    /// threads starving each other (observed: 2s resolves stretching past 20s).
    rebuild_inflight: bool,
    /// Cancels the in-flight rebuild. Replaced, never reset, when the next one
    /// starts, so a rebuild can never inherit its predecessor's cancellation.
    rebuild_cancel: crate::cancel::CancelToken,
    contour_cache: SharedContourCache,
    /// The composed grids of the last resolve, so the next one only recomposes
    /// what an edit reached — the resolve's counterpart to `contour_cache`, and
    /// what keeps the pixel grid from trailing the built font by a full
    /// resolve. See [`crate::ref_composite::CompositeGridCache`].
    composite_grid_cache: Arc<Mutex<crate::ref_composite::CompositeGridCache>>,
    /// Which face the editor builds. The editor never builds a collection —
    /// one face at a time — so this picks the one the preview, the specimen
    /// and the UI font itself are drawn with. Empty means the primary face,
    /// which is also what an id no longer declared falls back to.
    selected_face: String,
    /// Face ids as of the last derived-data rebuild; what the picker offers.
    face_ids: Vec<String>,
    /// Behind an `Arc` only so a background run can share it: resolving the
    /// whole font takes about as long as building it, and the shape-assertion
    /// thread needs exactly the map the derived-data thread just produced.
    named_glyphs: Arc<HashMap<String, ResolvedGlyph>>,
    alt_index: crate::editor::ref_composite::AlternativesIndex,
    name_parts: NamePartsMap,
    /// What a `ref ($0)` under an `exists` draws — one match per scoped item,
    /// alongside the name parts it is bound like.
    exists_matches: crate::exists::FirstMatches,
    /// What the source's `prop` lines state about characters the UCD leaves
    /// blank. Rebuilt with the rest of the derived data, so the status bar
    /// picks a new `prop` line up a debounce later rather than instantly —
    /// which is all a character name needs.
    char_props: crate::ucd::CharProps,
    color_aliases: crate::render::ttf_builder::ColorAliasMap,
    font_meta: crate::meta::FontMetrics,
    /// View menu: draw the metric box over every glyph grid.
    show_metrics: bool,
    /// A menu-bar menu is showing its contents this frame. Set while the menu
    /// bar is drawn, so the editors below it — drawn later in the same frame —
    /// can tell "a menu has the keyboard" from a real loss of focus.
    menu_open: bool,
    named_glyphs_gen: u64,
    // Bumped whenever named_glyphs/name_parts/alt_index/color_aliases are
    // replaced; keys the editor's per-frame view cache.
    derived_gen: u64,
    derived_data_tx: mpsc::Sender<DerivedDataResult>,
    derived_data_rx: mpsc::Receiver<DerivedDataResult>,
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
    /// Which glyphs `issues` are about, propagated through the `ref` graph —
    /// what tints a specimen cell. Kept beside `issues` and replaced with it.
    glyph_flags: crate::glyph_flags::GlyphFlags,
    /// Which severities the Issues tab lists; see [`panels::IssueFilter`].
    issue_filter: panels::IssueFilter,
    file_parse_errors: Vec<(PathBuf, String)>,
    assert_issues: Vec<Issue>,
    assert_rx: mpsc::Receiver<AssertResultMessage>,
    assert_tx: mpsc::Sender<AssertResultMessage>,
    assert_running: bool,
    /// The plan a background `fix` run produced, on its way to the documents.
    /// It names the files by path and the lines by glyph, since what it is
    /// applied to is whatever those files are *now*. See [`fix`].
    fix_rx: mpsc::Receiver<FixResultMessage>,
    fix_tx: mpsc::Sender<FixResultMessage>,
    fix_running: bool,
    bg_tasks: BackgroundTaskStatus,
    /// The worker every write to an open document goes through. See [`save`].
    saves: save::SaveQueue,
    /// Startup instrumentation (`startup.rs`): whether the first frame has been
    /// through `update`, and whether its report window is open.
    first_frame_seen: bool,
    startup_timing_open: bool,
}

/// The largest type size the bitmap face is drawn at. Anything above it is
/// the *vector* face's, since an enlarged bitmap only shows its own pixels.
pub const BITMAP_MAX_SIZE: f32 = 16.0;

pub fn uniform_font_id(ctx: &egui::Context, size: f32) -> egui::FontId {
    let bitmap_family = egui::FontFamily::Name("UniformBitmap".into());
    let has_uniform = ctx.fonts(|f| f.families().contains(&bitmap_family));
    if !has_uniform {
        return egui::FontId::new(size, egui::FontFamily::Proportional);
    }
    egui::FontId::new(size, uniform_family_at_size(&bitmap_family, size))
}

/// The family a piece of text drawn at `size` belongs in, given the family the
/// text around it uses. A size is not carried on the family, so a caller that
/// derives one size from another — a heading off the body text
/// ([`crate::editor::document_view::layout::heading_font_size`]) — has to
/// re-pick the face as well, or a 48 px `#` line draws as a scaled-up bitmap.
/// Only the upgrade is made: a caller already on the vector face asked for it.
pub fn uniform_family_at_size(base: &egui::FontFamily, size: f32) -> egui::FontFamily {
    let is_bitmap = matches!(base, egui::FontFamily::Name(n) if &**n == "UniformBitmap");
    if is_bitmap && size > BITMAP_MAX_SIZE {
        egui::FontFamily::Name("UniformVector".into())
    } else {
        base.clone()
    }
}

/// Whether `[perf]` stage timing logs are enabled (UNIFORM_PERF env var).
fn perf_log_enabled() -> bool {
    crate::startup::perf_logging()
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

        let mut settings = cc
            .storage
            .and_then(|s| eframe::get_value::<Settings>(s, eframe::APP_KEY))
            .unwrap_or_default();
        settings.clamp();

        Self::with_settings(&cc.egui_ctx, settings, font_dir)
    }

    /// Everything of the startup path that does not need a window: the split
    /// exists so the tests can build a whole `UniformApp` against a bare
    /// [`egui::Context`], which is the only way to assert what startup does and
    /// — more to the point — what it no longer does on the UI thread.
    fn with_settings(
        egui_ctx: &egui::Context,
        settings: Settings,
        font_dir: Option<PathBuf>,
    ) -> Self {
        // The argument wins over the remembered directory, and a remembered
        // one that has since been moved or deleted is simply not there.
        let font_dir = font_dir.or_else(|| {
            settings
                .font_dir
                .as_ref()
                .filter(|d| d.is_dir())
                .map(PathBuf::from)
        });

        crate::startup::mark("settings loaded");

        // Loaded through the cache the watch then takes over, so the first
        // *File ▸ Refresh filesystem* re-reads only what has moved since.
        let mut dir_cache = crate::render::ttf_builder::DirCache::new();
        let (font_base_docs, file_parse_errors, font_sources) = font_dir
            .as_ref()
            .map(|d| crate::render::ttf_builder::load_docs_from_directory_cached(d, &mut dir_cache))
            .unwrap_or_default();
        crate::startup::mark(format!("read {} .unf file(s)", font_base_docs.len()));
        let font_sources = docs::font_sources_from(font_sources);

        let contour_cache = crate::render::new_contour_cache();
        // No font is built here. It used to be, and on a network share that one
        // call was 10.4 of the 18.6 seconds before the window appeared — all of
        // it with nothing on screen, and all of it duplicated work, since the
        // first frame armed a background rebuild for the same documents anyway.
        // `arm_initial_font_build` below hands it to the pipeline instead; the
        // editor renders in the system font until it lands, which is the same
        // state an empty directory or a failed build already produces.

        let (font_build_tx, font_build_rx) = mpsc::channel();
        let (derived_data_tx, derived_data_rx) = mpsc::channel();
        let (assert_tx, assert_rx) = mpsc::channel();
        let (fix_tx, fix_rx) = mpsc::channel();

        let zoom_level = settings.zoom_level;
        let show_metrics = settings.show_metrics;
        let escape_mode = settings.escape_mode;
        let bottom_panel_tab = settings.bottom_panel_tab;
        let issue_filter = settings.issue_filter;
        let preview_font_size = settings.preview_font_size;

        let mut shaped_preview = ShapedPreviewState::new();
        shaped_preview.set_text(&settings.preview_text);
        shaped_preview.color_font = settings.preview_color_font;
        shaped_preview.direction = settings.preview_direction;
        shaped_preview.select_backend_named(&settings.preview_backend);
        let mut specimen = SpecimenState::new();
        specimen.options = settings.specimen;

        // Which faces this directory declares is a scan of its `face` lines —
        // the same `FaceSet` the resolve reports, only without the resolve — so
        // the remembered face can be applied *before* the first build is armed.
        // Waiting for the resolve meant the first build used the primary face
        // and the choice arriving after it built the whole font a second time,
        // which over SMB read as ten seconds of build followed by five more.
        let face_ids: Vec<String> = {
            let refs: Vec<&Document> = font_base_docs.iter().collect();
            crate::faces::FaceSet::collect(&refs)
                .faces
                .iter()
                .map(|f| f.id.clone())
                .collect()
        };
        // A face the directory no longer declares is dropped rather than
        // selected: the source is edited between runs, and a face can go away.
        let selected_face = font_dir
            .as_deref()
            .and_then(|d| settings.face_for(d))
            .filter(|f| face_ids.iter().any(|id| id == f))
            .unwrap_or_default()
            .to_string();
        crate::startup::mark("faces collected");

        let mut app = Self {
            settings,
            font_dir: font_dir.clone(),
            last_title: String::new(),
            open_documents: Vec::new(),
            panes: Panes::new_with_zoom(zoom_level),
            nav_history: NavHistory::new(),
            search: None,
            sidebar: Sidebar::new(),
            sidebar_rect: egui::Rect::NOTHING,
            watch: watch::WatchState::with_cache(dir_cache),
            toasts: toast::Toasts::new(),
            escape_mode,
            status_message: None,
            font_base_docs,
            font_sources,
            font_data: None,
            font_name_to_gid: HashMap::new(),
            font_applied: None,
            font_data_gen: 0,
            last_font_gen: 0,
            rebuild_at: None,
            rebuild_log: timing::RebuildLog::default(),
            rebuild_timing_open: false,
            specimen_asked_for: None,
            font_build_rx,
            font_build_tx,
            font_build_gen: 0,
            rebuild_inflight: false,
            rebuild_cancel: crate::cancel::CancelToken::never(),
            contour_cache,
            composite_grid_cache: Arc::default(),
            selected_face,
            face_ids,
            named_glyphs: Arc::default(),
            alt_index: Default::default(),
            name_parts: NamePartsMap::new(),
            exists_matches: Default::default(),
            char_props: Default::default(),
            color_aliases: Default::default(),
            font_meta: Default::default(),
            show_metrics,
            menu_open: false,
            named_glyphs_gen: u64::MAX,
            derived_gen: 0,
            derived_data_tx,
            derived_data_rx,
            last_export_path: None,
            close_confirmed: false,
            bottom_panel_height: 0.0,
            bottom_panel_height_override: false,
            bottom_panel_tab,
            preview_font_size,
            preview_font_size_slider: (preview_font_size / 16.0).round() * 16.0,
            preview_view_rect: None,
            shaped_preview,
            specimen,
            issues: Vec::new(),
            issues_gen: u64::MAX,
            glyph_flags: crate::glyph_flags::GlyphFlags::default(),
            issue_filter,
            file_parse_errors,
            assert_issues: Vec::new(),
            assert_rx,
            assert_tx,
            assert_running: false,
            fix_rx,
            fix_tx,
            fix_running: false,
            bg_tasks: BackgroundTaskStatus::new(),
            saves: save::SaveQueue::new(egui_ctx),
            first_frame_seen: false,
            startup_timing_open: false,
        };

        if let Some(dir) = &font_dir {
            app.sidebar.set_directory(dir);
            crate::startup::mark("sidebar directory listing");
            app.watch.set_directory(dir, egui_ctx);
            crate::startup::mark("file watch registered");
        }
        app.arm_initial_font_build();

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
        let from = NavLoc::new(from_doc, nav.from.line, nav.from.col).seen_at(nav.from_offset);
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
        // Unreachable in practice: `open_documents` is only appended to, and
        // the one thing that clears it (opening another folder) clears this
        // history with it. Kept as a safety net; consuming the history step
        // is acceptable for a state that cannot legitimately arise.
        if self.open_documents.get(loc.doc_idx).is_none() {
            return;
        }
        self.panes.show_document(loc.doc_idx);
        let intent = loc.scroll_intent();
        let doc = &mut self.open_documents[loc.doc_idx];
        doc.editor_state
            .goto_caret_with(Some(&doc.lines), loc.line, loc.col, intent);
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
        use crate::editor::doc_links::{find_link_target_in_doc, pattern_denotes};

        // A block's name may be written as a *pattern*, which declares every
        // name it expands to; `pattern_denotes` is the same test the Search
        // pane matches with, so the two agree on where a name is declared. An
        // `exists` on the line above is part of what the header says, and here
        // that is one index back in the item list — the binding is adjacency.
        let declares = |items: &[DocumentItem], idx: usize, n: &str| {
            let exists = match idx.checked_sub(1).and_then(|p| items.get(p)) {
                Some(DocumentItem::Exists { pattern, .. }) => Some(pattern.as_str()),
                _ => None,
            };
            n == name || pattern_denotes(n, true, name, &self.name_parts, exists, &[])
        };
        let target_path =
            {
                let all_docs = self.collect_all_docs();
                all_docs.iter().find_map(|doc| {
                    let has_match = match kind {
                    // An alias line defines the name it declares as much as a
                    // glyph block does — it is where "go to definition" has to
                    // land for a `ref` to an alias.
                    LinkTargetKind::Glyph => doc.items.iter().enumerate().any(|(i, item)| {
                        matches!(item, DocumentItem::Glyph { name: GlyphName(n), .. }
                            if declares(&doc.items, i, n))
                            || matches!(
                                item,
                                DocumentItem::GlyphAlias { name: GlyphName(n), .. }
                                    if declares(&doc.items, i, n)
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

        let line_idx = find_link_target_in_doc(
            &self.open_documents[idx].lines,
            name,
            kind,
            &self.name_parts,
        )?;
        self.open_documents[idx].editor_state.goto_line(line_idx);
        Some((idx, line_idx))
    }
}

impl eframe::App for UniformApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.save_settings(storage);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The frame the user actually waited for: everything before it happened
        // with no window on screen. See `startup.rs`.
        let first_frame = !self.first_frame_seen;
        if first_frame {
            self.first_frame_seen = true;
            crate::startup::mark("first frame begins");
        }
        // Timed for the frames right after a font lands: egui refills its atlas
        // inside the first layout that needs the new font, not in `set_fonts`,
        // so that cost is only visible as a slow frame. See [`timing`].
        let frame_started = std::time::Instant::now();

        self.sync_window_title(ctx);

        let mut menu = MenuActions::default();
        // Collected here and acted on below: switching a face rebuilds the
        // font, and that must not run while the input lock is held.
        let mut face_step = 0isize;
        // Read before `pump_file_watch` below, so the scan a refresh asks for
        // starts on this frame rather than the next.
        let mut refresh_fs = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F5) {
                refresh_fs = true;
            }
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

        if refresh_fs {
            self.request_filesystem_refresh(ctx);
        }

        self.intercept_swap_panes_chord(ctx, &mut menu);
        self.handle_zoom_scroll(ctx);
        self.handle_zoom_keys(ctx);

        // Before the pipeline: a reload steps the documents' generations, and
        // this frame's rebuild scheduling should see them.
        self.pump_file_watch(ctx);

        self.pump_background_pipeline(ctx);
        self.pump_saves(ctx);

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
        if menu.optimize_clearance {
            self.run_clearance_optimizer(ctx);
        }
        self.apply_file_menu_actions(ctx, &menu);

        self.show_sidebar_panel(ctx, editor_focused);

        self.show_status_bar(ctx);

        let bottom = self.show_bottom_panel(ctx);

        let editor_panel = self.show_editor_panel(ctx);
        let divider_closed_pane = editor_panel.divider_closed_pane;
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

        if let Some((from_doc, nav)) = editor_panel.nav {
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

        // The same fold the editor's own Ctrl/Cmd+. does, for a user who
        // reached it through the menu instead — and so with the keyboard focus
        // sitting on a menu button, which `with_active_doc_flush` hands back.
        if menu.toggle_fold {
            self.with_active_doc_flush(|doc| {
                let line = doc.editor_state.cursor_line();
                crate::editor::folding::toggle_at(&mut doc.lines, &mut doc.editor_state, line)
            });
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

        if let Some(rename) = editor_panel.rename {
            self.execute_rename(&rename);
        }

        if let Some(resize) = editor_panel.resize {
            self.execute_resize(&resize);
        }

        self.apply_edit_menu_actions(ctx, edit_target, menu.take_edit_actions());

        // Last, so the notices are over the panes rather than under them.
        if self.toasts.show(ctx) == Some(watch::HELD_CHANGES_TOAST) {
            self.apply_held_watch_changes(ctx);
        }

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

        self.show_startup_timing_window(ctx);
        self.show_rebuild_timing_window(ctx);
        self.rebuild_log.frame(frame_started.elapsed());

        if first_frame {
            crate::startup::mark("first frame built");
            crate::startup::first_frame_done();
            crate::startup::log_report_once();
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

    /// The startup report as a window, for the common case of a launch with no
    /// console to have read `UNIFORM_PERF` output from. The text is the same
    /// [`crate::startup::report`] the stderr dump prints, and Copy puts it on
    /// the clipboard so it can be pasted into a bug report.
    fn show_startup_timing_window(&mut self, ctx: &egui::Context) {
        if !self.startup_timing_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Startup timing")
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                let report = crate::startup::report();
                if ui.button("Copy").clicked() {
                    ctx.copy_text(report.clone());
                }
                ui.separator();
                egui::ScrollArea::both().max_height(420.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&report).monospace())
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
            });
        self.startup_timing_open = open;
    }

    /// What the last few rebuilds cost, as a window — the same report the
    /// stderr dump prints, and the only route on a launch with no console. See
    /// [`timing`] for why the UI-thread stages are in it.
    fn show_rebuild_timing_window(&mut self, ctx: &egui::Context) {
        if !self.rebuild_timing_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Rebuild timing")
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                let report = self.rebuild_log.report();
                if ui.button("Copy").clicked() {
                    ctx.copy_text(report.clone());
                }
                ui.separator();
                egui::ScrollArea::both().max_height(420.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&report).monospace())
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
            });
        self.rebuild_timing_open = open;
    }

    /// Hands the keyboard back to the active editor, which a menu-bar click
    /// took away. See [`crate::editor::EditorState::refocus`].
    fn refocus_active_editor(&mut self) {
        if let Some(doc) = self.active_doc_mut() {
            doc.editor_state.refocus();
        }
    }

    /// Runs a document mutation on the active document and flushes the line
    /// buffer back into the `Document` when it reports a change.
    ///
    /// Every caller is a menu item acting on the active editor, so the focus
    /// the menu took goes back whether or not the mutation changed anything.
    fn with_active_doc_flush(&mut self, f: impl FnOnce(&mut OpenDocument) -> bool) {
        self.refocus_active_editor();
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
