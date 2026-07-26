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
use crate::specimen::SpecimenState;
use crate::sidebar::{Sidebar, SidebarAction};

type FontPair = (Vec<u8>, Vec<u8>);
type FontBuildMessage = (u64, Option<(FontPair, HashMap<String, u16>)>);
type DerivedDataMessage = (u64, HashMap<String, ResolvedGlyph>, crate::editor::ref_composite::AlternativesIndex, NamePartsMap, Vec<Issue>);
type AssertResultMessage = Vec<Issue>;

enum BackgroundTaskPhase {
    Running(std::time::Instant),
    Finished(std::time::Instant, std::time::Duration),
}

struct BackgroundTaskStatus {
    build: Option<BackgroundTaskPhase>,
    test: Option<BackgroundTaskPhase>,
}

impl BackgroundTaskStatus {
    fn new() -> Self {
        Self { build: None, test: None }
    }

    fn start_build(&mut self) {
        self.build = Some(BackgroundTaskPhase::Running(std::time::Instant::now()));
    }

    fn finish_build(&mut self) {
        if let Some(BackgroundTaskPhase::Running(start)) = self.build {
            self.build = Some(BackgroundTaskPhase::Finished(
                std::time::Instant::now(),
                start.elapsed(),
            ));
        }
    }

    fn start_test(&mut self) {
        self.test = Some(BackgroundTaskPhase::Running(std::time::Instant::now()));
    }

    fn finish_test(&mut self) {
        if let Some(BackgroundTaskPhase::Running(start)) = self.test {
            self.test = Some(BackgroundTaskPhase::Finished(
                std::time::Instant::now(),
                start.elapsed(),
            ));
        }
    }

    fn gc(&mut self) {
        let expire = std::time::Duration::from_secs(10);
        if let Some(BackgroundTaskPhase::Finished(at, _)) = self.build {
            if at.elapsed() >= expire {
                self.build = None;
            }
        }
        if let Some(BackgroundTaskPhase::Finished(at, _)) = self.test {
            if at.elapsed() >= expire {
                self.test = None;
            }
        }
    }
}

/// Whether a directory-snapshot document at `path` is shadowed by an open
/// document editing the same file.
fn shadowed_by_open(open_documents: &[OpenDocument], path: &std::path::Path) -> bool {
    open_documents
        .iter()
        .any(|open_doc| open_doc.document.path == path)
}

fn collect_effective_docs<'a>(
    open_documents: &'a [OpenDocument],
    font_base_docs: &'a [Document],
) -> Vec<&'a Document> {
    let mut all_docs: Vec<&Document> = open_documents
        .iter()
        .map(|open_doc| &open_doc.document)
        .collect();
    for base_doc in font_base_docs {
        if !shadowed_by_open(open_documents, &base_doc.path) {
            all_docs.push(base_doc);
        }
    }
    all_docs.sort_by(|a, b| a.path.cmp(&b.path));
    all_docs
}

pub struct UniformApp {
    font_dir: Option<PathBuf>,
    last_title: String,
    open_documents: Vec<OpenDocument>,
    active_doc_idx: Option<usize>,
    sidebar: Sidebar,
    escape_mode: bool,
    status_message: Option<(String, std::time::Instant)>,
    font_base_docs: Vec<Document>,
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
    named_glyphs: HashMap<String, ResolvedGlyph>,
    alt_index: crate::editor::ref_composite::AlternativesIndex,
    name_parts: NamePartsMap,
    color_aliases: crate::render::ttf_builder::ColorAliasMap,
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
    zoom_level: u32,
    last_export_path: Option<PathBuf>,
    close_confirmed: bool,
    hex_input: Option<String>,
    bottom_panel_height: f32,
    bottom_panel_height_override: bool,
    bottom_panel_tab: Option<usize>,
    preview_font_size: f32,
    preview_font_size_slider: f32,
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

pub struct OpenDocument {
    pub document: Document,
    pub lines: Vec<DocLine>,
    pub editor_state: EditorState,
}

impl OpenDocument {
    /// Flush pending line-level edits into the `Document` model, if any.
    fn flush_pending_changes(&mut self) {
        if self.editor_state.has_pending_document_sync() {
            crate::editor::document_view::flush_document_changes(
                &mut self.lines,
                &mut self.document,
                &mut self.editor_state,
            );
        }
    }
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

fn take_current_font_build(
    rx: &mpsc::Receiver<FontBuildMessage>,
    current_gen: u64,
) -> Option<Option<(FontPair, HashMap<String, u16>)>> {
    let mut received = None;
    while let Ok((build_gen, result)) = rx.try_recv() {
        if build_gen == current_gen {
            received = Some(result);
        }
    }
    received
}

fn take_latest_derived_data(
    rx: &mpsc::Receiver<DerivedDataMessage>,
) -> Option<DerivedDataMessage> {
    let mut received = None;
    while let Ok(msg) = rx.try_recv() {
        received = Some(msg);
    }
    received
}

fn min_bottom_panel_height(screen_height: f32) -> f32 {
    270.0_f32.min(screen_height * 0.5)
}

impl UniformApp {
    fn active_doc(&self) -> Option<&OpenDocument> {
        self.active_doc_idx.and_then(|i| self.open_documents.get(i))
    }

    fn active_doc_mut(&mut self) -> Option<&mut OpenDocument> {
        self.active_doc_idx.and_then(|i| self.open_documents.get_mut(i))
    }

    fn in_grid_edit(&self) -> bool {
        self.active_doc()
            .is_some_and(|d| matches!(
                d.editor_state.mode,
                crate::editor::EditMode::GlyphEdit { .. }
                    | crate::editor::EditMode::PixelSelect { .. }
            ))
    }

    fn ensure_min_panel_height(&mut self, screen_height: f32) {
        let min_h = min_bottom_panel_height(screen_height);
        if self.bottom_panel_height < min_h {
            self.bottom_panel_height = min_h;
            self.bottom_panel_height_override = true;
        }
    }

    pub fn new(cc: &eframe::CreationContext<'_>, font_dir: Option<PathBuf>) -> Self {
        let _ = cc;

        let (font_base_docs, file_parse_errors) = font_dir
            .as_ref()
            .map(|d| crate::render::ttf_builder::load_docs_from_directory_checked(d))
            .unwrap_or_default();

        let contour_cache = crate::render::new_contour_cache();
        let (font_data, font_name_to_gid) = if font_base_docs.is_empty() {
            (None, HashMap::new())
        } else {
            let refs: Vec<&Document> = font_base_docs.iter().collect();
            match crate::render::build_font_pair_cached(&refs, &contour_cache) {
                Some((pair, map)) => (Some(pair), map),
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
            active_doc_idx: None,
            sidebar: Sidebar::new(),
            escape_mode: false,
            status_message: None,
            font_base_docs,
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
            named_glyphs: HashMap::new(),
            alt_index: Default::default(),
            name_parts: NamePartsMap::new(),
            color_aliases: Default::default(),
            named_glyphs_gen: u64::MAX,
            derived_gen: 0,
            derived_data_tx,
            derived_data_rx,
            derived_rebuild_at: None,
            derived_inflight: false,
            zoom_level: 1,
            last_export_path: None,
            close_confirmed: false,
            hex_input: None,
            bottom_panel_height: 0.0,
            bottom_panel_height_override: false,
            bottom_panel_tab: None,
            preview_font_size: 32.0,
            preview_font_size_slider: 32.0,
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
        }

        app
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    fn flush_open_document(&mut self, idx: usize) {
        let Some(doc) = self.open_documents.get_mut(idx) else {
            return;
        };
        doc.flush_pending_changes();
    }

    fn flush_all_open_documents(&mut self) {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
        }
    }

    fn open_file(&mut self, path: PathBuf) {
        if let Some(idx) = self.active_doc_idx {
            self.flush_open_document(idx);
        }
        if let Some(idx) = self
            .open_documents
            .iter()
            .position(|d| d.document.path == path)
        {
            self.active_doc_idx = Some(idx);
            return;
        }

        // Replacing the directory snapshot with an opened file is a new
        // source revision even when the path is unchanged. The file may have
        // changed on disk since the folder was loaded.
        let base_gen = self
            .font_base_docs
            .iter()
            .find(|base| base.path == path)
            .map(|b| (b.edit_gen, b.content_gen));
        match load_open_document(path.clone(), base_gen) {
            Ok(open_doc) => {
                self.open_documents.push(open_doc);
                self.active_doc_idx = Some(self.open_documents.len() - 1);
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"));
            }
        }
    }

    fn current_font_gen(&self) -> u64 {
        // Order-independent combination (XOR of per-doc hashes) so the
        // effective doc set needs neither collection nor sorting per frame.
        fn doc_hash(doc: &Document) -> u64 {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            doc.path.hash(&mut hasher);
            doc.content_gen.hash(&mut hasher);
            doc.pixel_gen.hash(&mut hasher);
            hasher.finish()
        }
        let mut combined = 0u64;
        for open_doc in &self.open_documents {
            combined ^= doc_hash(&open_doc.document);
        }
        for base_doc in &self.font_base_docs {
            if !shadowed_by_open(&self.open_documents, &base_doc.path) {
                combined ^= doc_hash(base_doc);
            }
        }
        combined
    }

    fn collect_all_docs(&self) -> Vec<&Document> {
        collect_effective_docs(&self.open_documents, &self.font_base_docs)
    }

    fn run_shape_assertions(&mut self, ctx: &egui::Context, current_file_only: bool) {
        if self.assert_running {
            return;
        }
        self.assert_running = true;
        self.bg_tasks.start_test();
        self.status_message = Some(("Running shape assertions...".to_string(), std::time::Instant::now()));

        let all_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let active_path = if current_file_only {
            self.active_doc_idx.map(|i| self.open_documents[i].document.path.clone())
        } else {
            None
        };
        let tx = self.assert_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let refs: Vec<&Document> = all_docs.iter().collect();
            let name_parts = crate::document::collect_name_parts(&refs);
            let (resolved, _) = crate::ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts);

            let mut result = if let Some(built) =
                crate::render::build_font_with_gid_map(&refs)
            {
                let assert_result = if let Some(path) = &active_path {
                    let test_docs: Vec<&Document> = all_docs.iter()
                        .filter(|d| &d.path == path)
                        .collect();
                    crate::render::assert::run_assertions_for_files(&test_docs, &built.ttf, &built.gid_to_name, built.height)
                } else {
                    crate::render::assert::run_assertions(&refs, &built.ttf, &built.gid_to_name, built.height)
                };
                assert_result.issues
            } else {
                vec![Issue {
                    severity: crate::issues::Severity::Error,
                    message: "Font build failed — cannot run shape assertions".to_string(),
                    file: std::path::PathBuf::new(),
                    line: 0,
                    file_line: 0,
                }]
            };

            let sd_result = if let Some(path) = &active_path {
                let test_docs: Vec<&Document> = all_docs.iter()
                    .filter(|d| &d.path == path)
                    .collect();
                crate::render::assert::run_same_distinct_assertions_for_files(&test_docs, &resolved)
            } else {
                crate::render::assert::run_same_distinct_assertions(&refs, &resolved)
            };
            result.extend(sd_result.issues);

            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn rebuild_font(&mut self, ctx: &egui::Context) {
        self.bg_tasks.start_build();
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let tx = self.font_build_tx.clone();
        let ctx = ctx.clone();
        let cache = self.contour_cache.clone();
        std::thread::spawn(move || {
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = owned_docs.iter().collect();
            let pair = crate::render::build_font_pair_cached(&refs, &cache);
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] font build (background): {:?}", t0.elapsed());
            }
            let _ = tx.send((build_gen, pair));
            ctx.request_repaint();
        });
    }

    fn goto_glyph(&mut self, _ctx: &egui::Context, name: &str, kind: &LinkTargetKind) {
        use crate::document::{DocumentItem, GlyphName};
        use crate::editor::doc_links::find_link_target_in_doc;

        let target_path = {
            let all_docs = self.collect_all_docs();
            all_docs.iter().find_map(|doc| {
                let has_match = match kind {
                    LinkTargetKind::Glyph => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Glyph { name: GlyphName(n), .. } if n == name)
                    }),
                    LinkTargetKind::NameParts => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::NameParts { name: n, .. } if n == name)
                    }),
                    LinkTargetKind::Remap => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Remap { feature: f, .. } if f == name)
                    }),
                    LinkTargetKind::Color => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Color { name: n, .. } if n == name)
                    }),
                };
                has_match.then(|| doc.path.clone())
            })
        };

        let Some(path) = target_path else { return };

        self.open_file(path.clone());

        let idx = match self.open_documents.iter().position(|d| d.document.path == path) {
            Some(i) => i,
            None => return,
        };
        self.active_doc_idx = Some(idx);

        let doc = &mut self.open_documents[idx];
        if let Some(line_idx) = find_link_target_in_doc(&doc.lines, name, kind) {
            doc.editor_state.goto_line(line_idx);
        }
    }

    fn execute_rename(&mut self, action: &crate::editor::document_view::RenameAction) {
        use crate::editor::doc_links::RenameKind;

        let saved_active = self.active_doc_idx;
        let mut changed_count = 0usize;

        // First pass: check which unopened files would be affected and open them.
        // Uses already-parsed font_base_docs (in memory) to avoid disk I/O
        // for the check; affected files are loaded in parallel.
        let to_open: Vec<PathBuf> = self.font_base_docs.iter()
            .filter(|base| {
                !shadowed_by_open(&self.open_documents, &base.path)
                    && doc_may_reference(&base.items, &action.old_name, &action.kind)
            })
            .map(|base| base.path.clone())
            .collect();

        if !to_open.is_empty() {
            let base_docs = &self.font_base_docs;
            let loaded: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = to_open.iter().map(|path| {
                    let path = path.clone();
                    let base_gen = base_docs.iter().find(|b| b.path == path)
                        .map(|b| (b.edit_gen, b.content_gen));
                    s.spawn(move || load_open_document(path, base_gen).ok())
                }).collect();
                handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
            });
            for open_doc in loaded {
                self.open_documents.push(open_doc);
            }
        }

        // Second pass: apply rename in place to all open documents.
        // Only touches Text lines; Grid lines are never cloned or compared.
        for doc in &mut self.open_documents {
            let changed_text = rename_in_place(&mut doc.lines, &action.old_name, &action.new_name, &action.kind);
            if !changed_text.is_empty() {
                doc.editor_state.undo.break_coalesce();
                let ops: Vec<_> = changed_text.iter().map(|(idx, old_text)| {
                    let new_text = match &doc.lines[*idx] {
                        DocLine::Text(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    crate::editor::undo::UndoOp::Lines {
                        at: *idx,
                        old: vec![DocLine::Text(old_text.clone())],
                        new: vec![DocLine::Text(new_text)],
                    }
                }).collect();
                doc.editor_state.undo.push_compound(
                    ops,
                    doc.editor_state.cursor,
                    doc.editor_state.cursor,
                );
                match crate::document_io::derive_document(&doc.lines, doc.document.path.clone()) {
                    Ok((new_doc, _)) => {
                        let items_changed = !doc.document.items.iter().filter(|i| i.affects_font())
                            .eq(new_doc.items.iter().filter(|i| i.affects_font()));
                        let next_gen = doc.document.edit_gen + 1;
                        let pixel_gen = doc.document.pixel_gen;
                        let content_gen = if items_changed {
                            doc.document.content_gen + 1
                        } else {
                            doc.document.content_gen
                        };
                        doc.document = new_doc;
                        doc.document.dirty = true;
                        doc.document.edit_gen = next_gen;
                        doc.document.pixel_gen = pixel_gen;
                        doc.document.content_gen = content_gen;
                    }
                    Err(_) => {
                        doc.document.dirty = true;
                        doc.document.edit_gen += 1;
                    }
                }
                changed_count += 1;
            }
        }

        // Restore active tab
        self.active_doc_idx = saved_active;

        if changed_count > 0 {
            let kind_str = match action.kind {
                RenameKind::Glyph => "glyph",
                RenameKind::NameParts => "name-parts",
                RenameKind::Point => "point",
                RenameKind::Color => "color",
            };
            self.set_status(format!(
                "Renamed {} '{}' → '{}' ({} file{})",
                kind_str,
                action.old_name,
                action.new_name,
                changed_count,
                if changed_count == 1 { "" } else { "s" },
            ));
        }
    }

    fn rebuild_named_glyphs_sync(&mut self) {
        let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
        let all_docs = self.collect_all_docs();
        let name_parts = crate::document::collect_name_parts(&all_docs);
        let (named_glyphs, alt_index) =
            crate::editor::ref_composite::resolve_named_glyphs_with_parts(
                &all_docs,
                &name_parts,
            );
        self.named_glyphs = named_glyphs;
        self.alt_index = alt_index;
        self.name_parts = name_parts;
        self.derived_gen = self.derived_gen.wrapping_add(1);
        if let Some(t0) = perf_t0 {
            eprintln!("[perf] resolve (sync, main thread): {:?}", t0.elapsed());
        }
    }

    fn rebuild_derived_data(&self, ctx: &egui::Context) {
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let file_parse_errors = self.file_parse_errors.clone();
        let tx = self.derived_data_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = owned_docs.iter().collect();
            // One resolution feeds both the glyph cache and validation; they
            // used to expand the whole document set independently.
            let resolution = crate::resolve::Resolution::compute(&refs);
            // Validation only reads names and diagnostics, so it runs before
            // the expansion is consumed by the glyph cache.
            let mut issues = crate::issues::collect_issues_with(&refs, &resolution);
            let name_parts = resolution.name_parts;
            let (named_glyphs, alt_index) = crate::editor::ref_composite::resolve_expansion(
                resolution.expansion,
                &name_parts,
            );
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] resolve (derived thread): {:?}", t0.elapsed());
            }
            for (path, msg) in &file_parse_errors {
                issues.insert(0, Issue {
                    severity: crate::issues::Severity::Error,
                    message: msg.clone(),
                    file: path.clone(),
                    line: 0,
                    file_line: 1,
                });
            }
            let _ = tx.send((build_gen, named_glyphs, alt_index, name_parts, issues));
            ctx.request_repaint();
        });
    }

    fn apply_font(&mut self, ctx: &egui::Context) {
        let want_custom = !self.escape_mode && self.font_data.is_some();
        if self.font_applied == Some(want_custom) {
            return;
        }

        let mut fonts = egui::FontDefinitions::default();
        let system_family = egui::FontFamily::Name("System".into());
        let bitmap_family = egui::FontFamily::Name("UniformBitmap".into());
        let vector_family = egui::FontFamily::Name("UniformVector".into());

        let system_fonts = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();

        let (mut bitmap_list, mut vector_list) = if let Some((bitmap_ttf, vector_ttf)) = &self.font_data {
            fonts.font_data.insert(
                "uniform_bitmap".into(),
                egui::FontData::from_owned(bitmap_ttf.clone()).into(),
            );
            fonts.font_data.insert(
                "uniform_vector".into(),
                egui::FontData::from_owned(vector_ttf.clone()).into(),
            );
            (
                vec!["uniform_bitmap".to_string()],
                vec!["uniform_vector".to_string()],
            )
        } else {
            (Vec::new(), Vec::new())
        };
        bitmap_list.extend(system_fonts.clone());
        vector_list.extend(system_fonts.clone());

        fonts
            .families
            .insert(bitmap_family, bitmap_list.clone());
        fonts
            .families
            .insert(vector_family, vector_list);

        if want_custom {
            fonts
                .families
                .insert(egui::FontFamily::Proportional, bitmap_list.clone());
            fonts
                .families
                .insert(egui::FontFamily::Monospace, bitmap_list);
        }

        fonts.families.insert(system_family, system_fonts);
        ctx.set_fonts(fonts);

        let mut style = (*ctx.style()).clone();
        for (_, font_id) in style.text_styles.iter_mut() {
            font_id.size = 16.0;
        }
        ctx.set_style(style);

        self.font_applied = Some(want_custom);
    }

    fn has_unsaved_changes(&self) -> bool {
        self.open_documents.iter().any(|d| {
            d.document.dirty || d.editor_state.has_pending_document_sync()
        })
    }

    fn save_all(&mut self) -> bool {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
            if !doc.document.dirty {
                continue;
            }
            let mut buf = Vec::new();
            if let Err(e) = document_io::serialize_doclines(&doc.lines, &mut buf)
                .and_then(|()| {
                    document_io::write_and_sync(&doc.document.path, &buf)
                })
            {
                self.status_message =
                    Some((format!("Save error: {e}"), std::time::Instant::now()));
                return false;
            }
            doc.document.dirty = false;
            doc.editor_state.undo.mark_saved();
        }
        true
    }

    fn confirm_close_and_maybe_save(&mut self) -> bool {
        if !self.has_unsaved_changes() {
            return true;
        }

        let save = "Save";
        let dont_save = "Don't Save";
        let cancel = "Cancel";

        let result = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Do you want to save changes before closing?")
            .set_description("Your unsaved changes will be lost if you close without saving.")
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                save.into(),
                dont_save.into(),
                cancel.into(),
            ))
            .show();

        match &result {
            rfd::MessageDialogResult::Yes => {
                self.save_all()
            }
            rfd::MessageDialogResult::Custom(s) if s == save => {
                self.save_all()
            }
            rfd::MessageDialogResult::No => true,
            rfd::MessageDialogResult::Custom(s) if s == dont_save => true,
            _ => false,
        }
    }

    fn export_to_path(&mut self, path: PathBuf) {
        self.flush_all_open_documents();
        let all_docs = self.collect_all_docs();
        let Some(font_bytes) = crate::render::build_font_from_documents(&all_docs) else {
            self.set_status("Export failed: could not build font".to_string());
            return;
        };
        let is_woff2 = path.extension().and_then(|e| e.to_str()) == Some("woff2");
        let output_bytes = if is_woff2 {
            match crate::render::ttf_to_woff2(&font_bytes) {
                Ok(b) => b,
                Err(e) => {
                    self.set_status(format!("Export error: {e}"));
                    return;
                }
            }
        } else {
            font_bytes
        };
        match std::fs::write(&path, &output_bytes) {
            Ok(()) => {
                self.last_export_path = Some(path.clone());
                self.set_status(format!(
                    "Exported {} ({} bytes)",
                    path.display(),
                    output_bytes.len(),
                ));
            }
            Err(e) => {
                self.set_status(format!("Export error: {e}"));
            }
        }
    }

    fn export_with_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export Font")
            .add_filter("TrueType Font", &["ttf"])
            .add_filter("WOFF2 Font", &["woff2"]);
        if let Some(ref last) = self.last_export_path {
            if let Some(dir) = last.parent() {
                dialog = dialog.set_directory(dir);
            }
            if let Some(name) = last.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy().to_string());
            }
        } else if let Some(ref dir) = self.font_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.export_to_path(path);
        }
    }

    fn save_active(&mut self) {
        if let Some(idx) = self.active_doc_idx
            && let Some(doc) = self.open_documents.get_mut(idx) {
                doc.flush_pending_changes();
                let mut buf = Vec::new();
                let result = document_io::serialize_doclines(&doc.lines, &mut buf)
                    .and_then(|()| {
                        document_io::write_and_sync(&doc.document.path, &buf)
                    });
                let path_display = doc.document.path.display().to_string();
                match result {
                    Ok(()) => {
                        doc.document.dirty = false;
                        doc.editor_state.undo.mark_saved();
                        self.status_message =
                            Some((format!("Saved {path_display}"), std::time::Instant::now()));
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Save error: {e}"), std::time::Instant::now()));
                    }
                }
            }
    }
}

impl eframe::App for UniformApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::stackmon::phase("update:begin");
        crate::stackmon::probe();
        {
            let title = if let Some(idx) = self.active_doc_idx {
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

        let mut escape_toggled = false;
        let mut run_assert_all = false;
        let mut run_assert_file = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F12) {
                self.escape_mode = !self.escape_mode;
                escape_toggled = true;
            }
            if i.key_pressed(egui::Key::F6) {
                if i.modifiers.command || i.modifiers.ctrl {
                    run_assert_all = true;
                } else {
                    run_assert_file = true;
                }
            }
        });

        // Alt + hex digit codepoint input
        {
            let mut hex_char_to_inject: Option<char> = None;
            let hex_input = &mut self.hex_input;
            ctx.input_mut(|input| {
                let alt_held = input.modifiers.alt;
                input.events.retain(|event| {
                    match event {
                        egui::Event::Key {
                            key, pressed: true, modifiers, ..
                        } if modifiers.alt && !modifiers.command && !modifiers.ctrl => {
                            if let Some(hex) = key_to_hex_char(*key) {
                                let buf = hex_input.get_or_insert_with(String::new);
                                if buf.len() < 6 {
                                    buf.push(hex);
                                }
                                return false;
                            }
                            if hex_input.is_some() {
                                *hex_input = None;
                                return false;
                            }
                            true
                        }
                        egui::Event::Key {
                            key: _, pressed: false, modifiers, ..
                        } if !alt_held && hex_input.is_some() => {
                            let _ = modifiers;
                            if let Some(hex_str) = hex_input.take()
                                && let Some(ch) = validate_hex_codepoint(&hex_str) {
                                    hex_char_to_inject = Some(ch);
                                }
                            true
                        }
                        egui::Event::Text(_) if hex_input.is_some() => false,
                        _ => true,
                    }
                });
                if !alt_held && hex_input.is_some()
                    && let Some(hex_str) = hex_input.take()
                        && let Some(ch) = validate_hex_codepoint(&hex_str) {
                            hex_char_to_inject = Some(ch);
                        }
                if let Some(ch) = hex_char_to_inject {
                    input.events.push(egui::Event::Text(ch.to_string()));
                }
            });
        }

        // Cmd/Ctrl + scroll wheel to adjust zoom level
        // (skip when hovering on the editing grid — ctrl+scroll cycles layers there)
        {
            let cmd_held = ctx.input(|i| i.modifiers.command);
            let grid_hover = self.active_doc()
                .is_some_and(|d| d.editor_state.is_grid_hover());
            if cmd_held && !grid_hover
                && let Some(step) = debounced_scroll_step(ctx) {
                    let old_zoom = self.zoom_level;
                    if step < 0 {
                        self.zoom_level = (self.zoom_level + 1).min(8);
                    } else {
                        self.zoom_level = (self.zoom_level - 1).max(1);
                    }
                    if self.zoom_level != old_zoom
                        && let Some(idx) = self.active_doc_idx
                            && let Some(doc) = self.open_documents.get_mut(idx) {
                                doc.editor_state.notify_zoom_change(old_zoom);
                            }
                    ctx.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                }
        }

        let any_pixel_painting = self.open_documents.iter()
            .any(|d| d.editor_state.suppress_font_rebuild);
        crate::stackmon::phase("update:derived-data");
        let font_gen = self.current_font_gen();
        if font_gen != self.last_font_gen && !any_pixel_painting {
            self.last_font_gen = font_gen;
            self.font_build_gen = self.font_build_gen.wrapping_add(1);
            let had_text_input = ctx.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Text(_)))
            });
            let debounce_ms = if had_text_input { 1000 } else { 300 };
            self.font_rebuild_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(debounce_ms));
            ctx.request_repaint_after(std::time::Duration::from_millis(debounce_ms));
        }
        if let Some(at) = self.font_rebuild_at
            && std::time::Instant::now() >= at {
                self.rebuild_font(ctx);
                self.font_rebuild_at = None;
            }

        {
            if let Some(result) =
                take_current_font_build(&self.font_build_rx, self.font_build_gen)
            {
                self.bg_tasks.finish_build();
                match result {
                    Some((pair, gid_map)) => {
                        self.font_data = Some(pair);
                        self.font_name_to_gid = gid_map;
                    }
                    None => {
                        self.font_data = None;
                        self.font_name_to_gid.clear();
                    }
                }
                self.font_data_gen = self.font_build_gen;
                self.font_applied = None;
                self.shaped_preview.invalidate_font(self.font_data_gen);
            }
        }

        if self.font_build_gen != self.named_glyphs_gen
            && self.derived_rebuild_at.is_none()
            && !self.derived_inflight
        {
            self.derived_rebuild_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(300));
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        if let Some(at) = self.derived_rebuild_at
            && std::time::Instant::now() >= at
            && !self.derived_inflight
        {
            self.rebuild_derived_data(ctx);
            self.derived_inflight = true;
            self.derived_rebuild_at = None;
        }

        if let Some((data_gen, named_glyphs, alt_index, name_parts, issues)) =
            take_latest_derived_data(&self.derived_data_rx)
        {
            self.derived_inflight = false;
            self.named_glyphs = named_glyphs;
            self.alt_index = alt_index;
            self.name_parts = name_parts;
            self.named_glyphs_gen = data_gen;
            self.derived_gen = self.derived_gen.wrapping_add(1);
            self.issues = issues;
            self.issues_gen = data_gen;
            let all_docs = self.collect_all_docs();
            let doc_refs: Vec<&Document> = all_docs.to_vec();
            self.color_aliases = crate::render::ttf_builder::collect_color_aliases(&doc_refs);
        }

        if let Ok(assert_issues) = self.assert_rx.try_recv() {
            let count = assert_issues.len();
            self.assert_issues = assert_issues;
            self.assert_running = false;
            self.bg_tasks.finish_test();
            let total_msg = if count == 0 {
                "All shape assertions passed.".to_string()
            } else {
                format!("{count} shape assertion(s) failed.")
            };
            self.status_message = Some((total_msg, std::time::Instant::now()));
            if count > 0 {
                self.bottom_panel_tab = Some(2);
            }
        }

        self.bg_tasks.gc();
        if self.bg_tasks.build.is_some() || self.bg_tasks.test.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        let theme_before = ctx.options(|o| o.theme_preference);
        let (mod_name, shift_name) = crate::edit_menu::platform_shortcut_names();
        let exit_shortcut = if cfg!(target_os = "macos") { "⌘Q" } else { "Alt+F4" };

        let mut menu_new_file = false;
        let mut menu_open_folder = false;
        let mut menu_rename = false;
        let mut menu_rename_symbol = false;
        let mut menu_export = false;
        let mut menu_export_new = false;
        let mut menu_exit = false;
        let mut ctrl_s_pressed = false;
        let mut ctrl_shift_s_pressed = false;

        use crate::edit_menu::{EditAction, EditMenuCaps};
        use crate::editor::pixel_selection::SelectionTransform;

        #[derive(Clone, Copy, PartialEq)]
        enum EditTarget { Editor, Preview }

        enum SelMenuAction {
            Cancel,
            Transform(SelectionTransform),
        }

        let mut edit_action = EditAction::None;
        let mut sel_menu_action: Option<SelMenuAction> = None;
        let mut scale_action: Option<u8> = None;
        let edit_target = if self.shaped_preview.is_focused() {
            EditTarget::Preview
        } else {
            EditTarget::Editor
        };

        let editor_focused = self.active_doc()
            .is_some_and(|d| d.editor_state.is_active());

        crate::stackmon::phase("update:menu_bar");
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("New file...").shortcut_text(format!("{mod_name}N"))).clicked() {
                        menu_new_file = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Open folder...").shortcut_text(format!("{mod_name}{shift_name}O"))).clicked() {
                        menu_open_folder = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    let has_active = self.active_doc_idx.is_some();
                    if ui
                        .add_enabled(has_active, egui::Button::new("Save").shortcut_text(format!("{mod_name}S")))
                        .clicked()
                    {
                        ctrl_s_pressed = true;
                        ui.close_menu();
                    }
                    if ui
                        .add(egui::Button::new("Save all").shortcut_text(format!("{mod_name}{shift_name}S")))
                        .clicked()
                    {
                        ctrl_shift_s_pressed = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has_active && !editor_focused, egui::Button::new("Rename file...").shortcut_text("F2"))
                        .clicked()
                    {
                        menu_rename = true;
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
                        menu_export = true;
                        ui.close_menu();
                    }
                    if ui
                        .add(
                            egui::Button::new("Export to new font...")
                                .shortcut_text(format!("{mod_name}{shift_name}E")),
                        )
                        .clicked()
                    {
                        menu_export_new = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Exit").shortcut_text(exit_shortcut)).clicked() {
                        menu_exit = true;
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
                    edit_action = crate::edit_menu::show_edit_menu_items(ui, &caps, true);
                    ui.separator();
                    if ui
                        .add_enabled(editor_focused, egui::Button::new("Rename symbol...").shortcut_text("F2"))
                        .clicked()
                    {
                        menu_rename_symbol = true;
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
                                    scale_action = Some(s);
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
                        sel_menu_action = Some(SelMenuAction::Cancel);
                        ui.close_menu();
                    }

                    ui.separator();

                    if ui.add_enabled(
                        can_do(SelectionTransform::MirrorH),
                        egui::Button::new("Mirror selection").shortcut_text(format!("{mod_name}M")),
                    ).clicked() {
                        sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::MirrorH));
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        can_do(SelectionTransform::FlipV),
                        egui::Button::new("Flip selection").shortcut_text(format!("{mod_name}I")),
                    ).clicked() {
                        sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::FlipV));
                        ui.close_menu();
                    }
                    ui.menu_button("Rotate selection", |ui| {
                        if ui.add_enabled(
                            can_do(SelectionTransform::RotateCCW),
                            egui::Button::new("Counterclockwise").shortcut_text(format!("{mod_name}J")),
                        ).clicked() {
                            sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::RotateCCW));
                            ui.close_menu();
                        }
                        if ui.add_enabled(
                            can_do(SelectionTransform::Rotate180),
                            egui::Button::new("180 degrees").shortcut_text(format!("{mod_name}K")),
                        ).clicked() {
                            sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::Rotate180));
                            ui.close_menu();
                        }
                        if ui.add_enabled(
                            can_do(SelectionTransform::RotateCW),
                            egui::Button::new("Clockwise").shortcut_text(format!("{mod_name}L")),
                        ).clicked() {
                            sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::RotateCW));
                            ui.close_menu();
                        }
                    });

                    ui.separator();

                    if ui.add_enabled(
                        can_do(SelectionTransform::Opposite),
                        egui::Button::new("Opposite subglyphs").shortcut_text(format!("{mod_name}O")),
                    ).clicked() {
                        sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::Opposite));
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        can_do(SelectionTransform::OppositeBitmap),
                        egui::Button::new("Opposite bitmap").shortcut_text(format!("{mod_name}\u{21e7}O")),
                    ).clicked() {
                        sel_menu_action = Some(SelMenuAction::Transform(SelectionTransform::OppositeBitmap));
                        ui.close_menu();
                    }
                });
                ui.menu_button("Font", |ui| {
                    if ui.add_enabled(
                        !self.assert_running && self.active_doc_idx.is_some(),
                        egui::Button::new("Run assertions (current file)").shortcut_text("F6"),
                    ).clicked() {
                        run_assert_file = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(
                        !self.assert_running,
                        egui::Button::new("Run assertions (all files)").shortcut_text(format!("{mod_name}F6")),
                    ).clicked() {
                        run_assert_all = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
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
                        escape_toggled = true;
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
        if run_assert_all {
            self.run_shape_assertions(ctx, false);
        } else if run_assert_file {
            self.run_shape_assertions(ctx, true);
        }
        if ctx.options(|o| o.theme_preference) != theme_before {
            self.font_applied = None;
        }

        ctx.input(|i| {
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::N) {
                menu_new_file = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::O) {
                if !self.in_grid_edit() {
                    menu_open_folder = true;
                }
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::S) {
                ctrl_s_pressed = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S) {
                ctrl_shift_s_pressed = true;
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::E) {
                menu_export = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::E) {
                menu_export_new = true;
            }
            if cfg!(target_os = "macos") {
                if i.modifiers.command && i.key_pressed(egui::Key::Q) {
                    menu_exit = true;
                }
            } else if i.modifiers.alt && i.key_pressed(egui::Key::F4) {
                menu_exit = true;
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

        if menu_new_file && self.font_dir.is_some() {
            self.sidebar.start_new_file();
        }

        if menu_open_folder
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
            && self.confirm_close_and_maybe_save() {
                self.font_dir = Some(dir.clone());
                self.open_documents.clear();
                self.active_doc_idx = None;
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

        if menu_rename
            && let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get(idx)  {
                    self.sidebar.start_rename(&doc.document.path);
                }

        if menu_rename_symbol
            && let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get_mut(idx) {
                    doc.editor_state.start_rename_at_cursor(&doc.lines);
                }

        if escape_toggled {
            self.font_applied = None;
        }
        self.apply_font(ctx);

        if ctrl_s_pressed {
            self.save_active();
        }
        if ctrl_shift_s_pressed
            && self.save_all() {
                self.set_status("Saved all files".to_string());
            }

        if menu_export {
            if let Some(path) = self.last_export_path.clone() {
                self.export_to_path(path);
            } else {
                self.export_with_dialog();
            }
        }
        if menu_export_new {
            self.export_with_dialog();
        }

        let mut sidebar_actions = Vec::new();
        let mut goto_glyph_request: Option<crate::editor::document_view::GotoGlyph> = None;
        let mut rename_request: Option<crate::editor::document_view::RenameAction> = None;
        crate::stackmon::phase("update:sidebar");
        egui::SidePanel::left("sidebar")
            .default_width(200.0)
            .show(ctx, |ui| {
                let dirty_paths: Vec<&std::path::Path> = self.open_documents
                    .iter()
                    .filter(|d| d.document.dirty)
                    .map(|d| d.document.path.as_path())
                    .collect();
                // Field-level accesses so the `sidebar` borrow stays disjoint
                // from the `open_documents` one.
                let active_path = self
                    .active_doc_idx
                    .and_then(|i| self.open_documents.get(i))
                    .map(|d| d.document.path.as_path());
                sidebar_actions = self.sidebar.show(ui, active_path, &dirty_paths, editor_focused);
            });

        for action in sidebar_actions {
            match action {
                SidebarAction::OpenFile(path) => {
                    self.open_file(path);
                }
                SidebarAction::FileRenamed { old, new } => {
                    for doc in &mut self.open_documents {
                        if doc.document.path == old {
                            doc.document.path = new.clone();
                        }
                    }
                    for doc in &mut self.font_base_docs {
                        if doc.path == old {
                            doc.path = new.clone();
                        }
                    }
                    self.set_status(format!(
                        "Renamed {} -> {}",
                        old.file_name().unwrap_or_default().to_string_lossy(),
                        new.file_name().unwrap_or_default().to_string_lossy(),
                    ));
                }
                SidebarAction::FileCreated(path) => {
                    self.set_status(format!("Created {}", path.display()));
                    self.open_file(path);
                }
            }
        }

        crate::stackmon::phase("update:status");
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.escape_mode {
                    ui.label(
                        egui::RichText::new("[ESCAPE MODE]")
                            .color(egui::Color32::YELLOW)
                            .strong(),
                    );
                }

                if let Some(hex) = &self.hex_input {
                    ui.label(
                        egui::RichText::new(format!("U+{hex}"))
                            .color(egui::Color32::from_rgb(100, 200, 255))
                            .strong(),
                    );
                } else if self.shaped_preview.is_focused()
                    && let Some(label) = self.shaped_preview.selection_codepoints_label() {
                        ui.label(label);
                    }
                else if let Some(status) = &self.specimen.hover_status {
                    ui.label(status);
                }
                else if let Some((msg, time)) = &self.status_message
                    && time.elapsed().as_secs() < 5 {
                        ui.label(msg);
                    }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.zoom_level > 1 {
                        ui.label(format!("{}x", self.zoom_level));
                    }
                    if let Some(idx) = self.active_doc_idx
                        && let Some(doc) = self.open_documents.get(idx) {
                            let name = doc
                                .document
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let line = doc.editor_state.cursor_source_line();
                            let dirty = if doc.document.dirty { " [modified]" } else { "" };
                            ui.label(format!("{name}:{line}{dirty}"));
                        }
                    for (label, phase) in [
                        ("Build", &self.bg_tasks.build),
                        ("Test", &self.bg_tasks.test),
                    ] {
                        if let Some(phase) = phase {
                            let (text, color) = match phase {
                                BackgroundTaskPhase::Running(start) => {
                                    let secs = start.elapsed().as_secs_f64();
                                    (format!("{label} {secs:.1}s"), egui::Color32::from_rgb(100, 180, 255))
                                }
                                BackgroundTaskPhase::Finished(_, dur) => {
                                    let secs = dur.as_secs_f64();
                                    (format!("{label} {secs:.1}s"), egui::Color32::from_rgb(100, 200, 100))
                                }
                            };
                            ui.label(egui::RichText::new(format!("{{{text}}}")).color(color));
                        }
                    }
                });
            });
        });

        let mut specimen_clicked_glyph: Option<crate::specimen::SpecimenClick> = None;
        let mut issues_click: Option<(PathBuf, usize)> = None;
        let bottom_panel_expanded = self.bottom_panel_tab.is_some();
        if self.bottom_panel_height_override {
            self.bottom_panel_height_override = false;
            let panel_id = egui::Id::new("bottom_panel");
            if let Some(mut state) =
                ctx.data_mut(|d| d.get_persisted::<egui::panel::PanelState>(panel_id))
            {
                let h = state.rect.height();
                if h < self.bottom_panel_height {
                    state.rect.set_height(self.bottom_panel_height);
                    ctx.data_mut(|d| d.insert_persisted(panel_id, state));
                }
            }
        }
        crate::stackmon::phase("update:preview");
        let mut bottom_panel = egui::TopBottomPanel::bottom("bottom_panel")
            .resizable(bottom_panel_expanded);
        if bottom_panel_expanded {
            bottom_panel = bottom_panel
                .default_height(self.bottom_panel_height)
                .min_height(100.0);
        }
        bottom_panel.show(ctx, |ui| {
                if bottom_panel_expanded {
                    self.bottom_panel_height = ui.available_height();
                }
                ui.horizontal(|ui| {
                    let screen_h = ui.ctx().input(|i| i.screen_rect.height());
                    for (idx, label) in [(0, "Preview"), (1, "Specimen")] {
                        let selected = self.bottom_panel_tab == Some(idx);
                        if ui.selectable_label(selected, label).clicked() {
                            if selected {
                                self.bottom_panel_tab = None;
                            } else {
                                self.bottom_panel_tab = Some(idx);
                                self.ensure_min_panel_height(screen_h);
                            }
                        }
                    }
                    let total_issues = self.issues.len() + self.assert_issues.len();
                    let issues_label = if total_issues == 0 {
                        "Issues".to_string()
                    } else {
                        let errors = self.issues.iter().chain(self.assert_issues.iter())
                            .filter(|i| i.severity == crate::issues::Severity::Error)
                            .count();
                        let warnings = total_issues - errors;
                        let mut parts = Vec::new();
                        if errors > 0 { parts.push(format!("{errors} error{}", if errors == 1 { "" } else { "s" })); }
                        if warnings > 0 { parts.push(format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" })); }
                        format!("Issues ({})", parts.join(", "))
                    };
                    let issues_selected = self.bottom_panel_tab == Some(2);
                    if ui.selectable_label(issues_selected, issues_label).clicked() {
                        if issues_selected {
                            self.bottom_panel_tab = None;
                        } else {
                            self.bottom_panel_tab = Some(2);
                            self.ensure_min_panel_height(screen_h);
                        }
                    }
                });
                if self.bottom_panel_tab != Some(1) {
                    self.specimen.hover_status = None;
                }
                if self.bottom_panel_tab.is_none() {
                    return;
                }
                ui.separator();
                match self.bottom_panel_tab {
                    Some(0) => {
                        ui.horizontal(|ui| {
                            ui.label("Font size:");
                            let slider_resp = ui.add(
                                egui::Slider::new(
                                    &mut self.preview_font_size_slider,
                                    16.0..=128.0,
                                )
                                .show_value(false)
                                .step_by(16.0)
                                .fixed_decimals(0),
                            );
                            if slider_resp.changed() {
                                self.preview_font_size = self.preview_font_size_slider;
                            }
                            let drag_resp = ui.add(
                                egui::DragValue::new(&mut self.preview_font_size)
                                    .range(16.0..=128.0)
                                    .suffix("px")
                                    .fixed_decimals(0)
                                    .speed(1.0),
                            );
                            if drag_resp.changed() {
                                self.preview_font_size_slider =
                                    (self.preview_font_size / 16.0).round() * 16.0;
                            }
                            ui.separator();
                            self.shaped_preview.show_engine_combo(ui);
                            ui.separator();
                            ui.checkbox(&mut self.shaped_preview.color_font, "Color");
                        });
                        ui.separator();
                        self.shaped_preview.show(
                            ui,
                            self.font_data.as_ref(),
                            self.font_data_gen,
                            self.preview_font_size,
                        );
                    }
                    Some(1) => {
                        if self.specimen.needs_rebuild(self.font_build_gen) {
                            let all_docs = collect_effective_docs(
                                &self.open_documents,
                                &self.font_base_docs,
                            );
                            self.specimen.rebuild_if_needed(
                                &all_docs, &self.name_parts, &self.font_name_to_gid, self.font_build_gen,
                            );
                        }
                        specimen_clicked_glyph = self.specimen.show(
                            ui,
                            self.font_data.as_ref(),
                            self.font_data_gen,
                        );
                    }
                    Some(2) => {
                        let mut all_issues: Vec<&Issue> = self.issues.iter().collect();
                        all_issues.extend(self.assert_issues.iter());
                        show_issues_tab(ui, &all_issues, &mut issues_click);
                    }
                    _ => {}
                }
            });

        crate::stackmon::phase("update:central/editor");
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.open_documents.is_empty() {
                ui.centered_and_justified(|ui| {
                    if self.font_dir.is_some() {
                        ui.label("Select a file from the sidebar");
                    } else {
                        ui.label("Usage: uniform <font-directory>");
                    }
                });
            } else if let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get_mut(idx) {
                    let font_size = 16.0 * self.zoom_level as f32;
                    let editor_font_id = if self.escape_mode {
                        egui::FontId::new(font_size, egui::FontFamily::Monospace)
                    } else {
                        uniform_font_id(ui.ctx(), font_size)
                    };
                    let result = crate::editor::document_view::show_document(
                        ui,
                        &mut doc.document,
                        &mut doc.lines,
                        &mut doc.editor_state,
                        &self.named_glyphs,
                        &self.name_parts,
                        &self.alt_index,
                        &self.color_aliases,
                        self.derived_gen,
                        self.font_data_gen,
                        self.zoom_level,
                        &editor_font_id,
                    );
                    if let Some(goto) = result.goto {
                        goto_glyph_request = Some(goto);
                    }
                    if let Some(rename) = result.rename {
                        rename_request = Some(rename);
                    }
                }
        });

        if let Some(goto) = goto_glyph_request {
            self.goto_glyph(ctx, &goto.name, &goto.kind);
        }

        if let Some(click) = specimen_clicked_glyph {
            self.goto_glyph(ctx, &click.name, &click.kind);
        }

        if let Some((path, line)) = issues_click {
            self.open_file(path.clone());
            if let Some(idx) = self.open_documents.iter().position(|d| d.document.path == path) {
                self.active_doc_idx = Some(idx);
                self.open_documents[idx].editor_state.goto_line(line);
            }
        }

        if let Some(rename) = rename_request {
            self.execute_rename(&rename);
        }

        if edit_action != EditAction::None {
            match edit_target {
                EditTarget::Preview => {
                    self.shaped_preview.apply_edit_action(edit_action, ctx);
                }
                EditTarget::Editor => {
                    if let Some(idx) = self.active_doc_idx
                        && let Some(doc) = self.open_documents.get_mut(idx) {
                            let changed = doc.editor_state.apply_edit_action(
                                edit_action,
                                &mut doc.lines,
                                ctx,
                            );
                            if changed {
                                crate::editor::document_view::flush_document_changes(
                                    &mut doc.lines,
                                    &mut doc.document,
                                    &mut doc.editor_state,
                                );
                            }
                        }
                }
            }
        }

        if let Some(action) = sel_menu_action {
            if let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get_mut(idx)
            {
                match action {
                    SelMenuAction::Cancel => {
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
                    SelMenuAction::Transform(t) => {
                        if crate::editor::pixel_selection::handle_transform_selection(
                            &doc.document,
                            &mut doc.lines,
                            &mut doc.editor_state,
                            t,
                        ) {
                            crate::editor::document_view::flush_document_changes(
                                &mut doc.lines,
                                &mut doc.document,
                                &mut doc.editor_state,
                            );
                        }
                    }
                }
            }
        }

        if let Some(new_scale) = scale_action {
            if let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get_mut(idx)
            {
                if crate::editor::pixel_selection::handle_adjust_scale(
                    &doc.document,
                    &mut doc.lines,
                    &mut doc.editor_state,
                    new_scale,
                ) {
                    crate::editor::document_view::flush_document_changes(
                        &mut doc.lines,
                        &mut doc.document,
                        &mut doc.editor_state,
                    );
                }
            }
        }

        crate::stackmon::phase("update:end (egui tessellate/paint follows)");
        // Decide whether to close only after this frame's editor input has
        // updated the source buffer and dirty state.
        if menu_exit && self.confirm_close_and_maybe_save() {
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

use crate::editor::{key_to_hex_char, validate_hex_codepoint};

fn show_issues_tab(
    ui: &mut egui::Ui,
    issues: &[&Issue],
    click: &mut Option<(PathBuf, usize)>,
) {
    if issues.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("No issues");
        });
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (issue_idx, issue) in issues.iter().enumerate() {
            let icon = match issue.severity {
                crate::issues::Severity::Error => "\u{2716}",
                crate::issues::Severity::Warning => "\u{26A0}",
            };
            let icon_color = match issue.severity {
                crate::issues::Severity::Error => egui::Color32::from_rgb(220, 60, 60),
                crate::issues::Severity::Warning => egui::Color32::from_rgb(200, 180, 50),
            };
            let file_name = issue
                .file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let location = format!("{file_name}:{}", issue.file_line);

            let row_id = ui.id().with(("issue_row", issue_idx));
            let resp = ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icon).color(icon_color).size(16.0));
                ui.label(egui::RichText::new(&issue.message).size(16.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(&location)
                            .size(16.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });

            let click_resp = ui.interact(
                resp.response.rect,
                row_id,
                egui::Sense::click(),
            );
            if click_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if click_resp.clicked() {
                *click = Some((issue.file.clone(), issue.line));
            }
        }
    });
}

/// Loads a file from disk into a fresh `OpenDocument`: parse, serialize to
/// canonical text, re-derive from the resulting doclines, and bump the
/// generation counters past the directory snapshot's (`base_gen`).  Shared by
/// interactive open and the parallel loads `execute_rename` performs.
fn load_open_document(
    path: PathBuf,
    base_gen: Option<(u64, u64)>,
) -> anyhow::Result<OpenDocument> {
    let doc = document_io::parse_document(&path)?;
    let mut buf = Vec::new();
    document_io::serialize_document(&doc, &mut buf).ok();
    let canonical = String::from_utf8(buf).unwrap_or_default();
    let mut lines = document_io::parse_doclines(&canonical);
    if lines.is_empty() {
        lines.push(crate::document::DocLine::Text(String::new()));
    }
    let mut doc = doc;
    if let Ok((fresh_doc, _)) = document_io::derive_document(&lines, path.clone()) {
        doc = fresh_doc;
    }
    let (edit_gen, content_gen) = base_gen
        .map(|(e, c)| (e.wrapping_add(1), c.wrapping_add(1)))
        .unwrap_or((1, 1));
    doc.edit_gen = edit_gen;
    doc.content_gen = content_gen;
    Ok(OpenDocument {
        document: doc,
        lines,
        editor_state: EditorState::new(),
    })
}

/// Apply rename in place, returning the old text values of changed lines
/// (as `(line_index, old_text)` pairs) so callers can build undo entries
/// without cloning the entire document (which is expensive when Grid lines
/// dominate).
fn rename_in_place(
    lines: &mut [DocLine],
    old_name: &str,
    new_name: &str,
    kind: &crate::editor::doc_links::RenameKind,
) -> Vec<(usize, String)> {
    let mut changed = Vec::new();
    for (i, line) in lines.iter_mut().enumerate() {
        let DocLine::Text(s) = line else { continue };
        if let Some(t) = rename_in_line(s, old_name, new_name, kind) {
            changed.push((i, std::mem::replace(s, t)));
        }
    }
    changed
}

fn doc_may_reference(
    items: &[crate::document::DocumentItem],
    name: &str,
    kind: &crate::editor::doc_links::RenameKind,
) -> bool {
    use crate::document::DocumentItem;
    use crate::editor::doc_links::RenameKind;

    for item in items {
        match (kind, item) {
            (RenameKind::Glyph, DocumentItem::Glyph { name: gn, body }) => {
                if gn.0 == name { return true; }
                if body.refs.iter().any(|r| r.name == name) { return true; }
            }
            (RenameKind::Glyph, DocumentItem::Map { glyph, .. }) => {
                if glyph == name { return true; }
            }
            (RenameKind::Glyph, DocumentItem::Remap { .. }) => {
                let mut all = item.remap_operands();
                if all.any(|s| s == name) { return true; }
            }
            (RenameKind::Glyph, DocumentItem::Directive(s)) => {
                if s.contains(name) { return true; }
            }
            (RenameKind::NameParts, DocumentItem::NameParts { name: n, values }) => {
                if n == name || values.iter().any(|v| v == name) { return true; }
            }
            (RenameKind::NameParts, DocumentItem::Glyph { name: gn, body }) => {
                if gn.0.contains(name) { return true; }
                if body.refs.iter().any(|r| r.name.contains(name)) { return true; }
            }
            (RenameKind::Point, DocumentItem::Glyph { body, .. }) => {
                let stripped = name.trim_start_matches(['+', '-']);
                if body.points.iter().any(|p| {
                    let ps = p.position.trim_start_matches(['+', '-']);
                    ps == stripped
                }) { return true; }
            }
            (RenameKind::Color, DocumentItem::Color { name: n, .. }) => {
                if n == name { return true; }
            }
            (RenameKind::Color, DocumentItem::Glyph { body, .. }) => {
                if body.refs.iter().any(|r| r.fill.as_ref().is_some_and(|f| f.color == name)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Applies a rename to one line by splicing new text over the classified
/// name fields (`crate::editor::line_fields`).  Detection and mutation share
/// the classification, so whatever the rename popup identified is exactly
/// what gets rewritten.  Returns `None` when the line is unaffected.
fn rename_in_line(
    full: &str,
    old_name: &str,
    new_name: &str,
    kind: &crate::editor::doc_links::RenameKind,
) -> Option<String> {
    use crate::editor::doc_links::RenameKind;
    use crate::editor::line_fields::{FieldRole, classify_line};

    // (char col start, char col end, replacement)
    let mut reps: Vec<(usize, usize, String)> = Vec::new();
    for f in classify_line(full) {
        let rep = match (kind, f.role) {
            (RenameKind::Glyph, FieldRole::GlyphDef | FieldRole::GlyphRef)
                if f.token == old_name =>
            {
                Some(crate::document_io::quote_token(new_name))
            }
            (
                RenameKind::NameParts,
                FieldRole::GlyphDef | FieldRole::GlyphRef | FieldRole::NamePartsValue,
            ) => {
                let new_tok = replace_dollar_var(&f.token, old_name, new_name);
                (new_tok != f.token).then(|| crate::document_io::quote_token(&new_tok))
            }
            (RenameKind::NameParts, FieldRole::NamePartsDef) if f.token == old_name => {
                Some(new_name.to_string())
            }
            (RenameKind::Point, FieldRole::PointDef) => {
                let (prefix, bare) = match f.token.strip_prefix(['+', '-']) {
                    Some(stripped) => (&f.token[..1], stripped),
                    None => ("", f.token.as_str()),
                };
                (bare == old_name).then(|| format!("{prefix}{new_name}"))
            }
            (RenameKind::Color, FieldRole::ColorDef | FieldRole::ColorRef)
                if f.token == old_name =>
            {
                Some(crate::document_io::quote_token(new_name))
            }
            _ => None,
        };
        if let Some(r) = rep {
            reps.push((f.col_start, f.col_end, r));
        }
    }

    if reps.is_empty() {
        return None;
    }

    // Renaming an anchor also migrates the legacy `point` keyword.
    if matches!(kind, RenameKind::Point) {
        let trimmed = full.trim_start();
        if trimmed.starts_with("point ") {
            let leading = full.chars().count() - trimmed.chars().count();
            reps.push((leading, leading + "point".len(), "anchor".to_string()));
        }
    }

    use crate::editor::caret::char_to_byte;
    reps.sort_by_key(|&(start, _, _)| std::cmp::Reverse(start));
    let mut out = full.to_string();
    for (start, end, replacement) in reps {
        let byte_start = char_to_byte(&out, start);
        let byte_end = char_to_byte(&out, end);
        out.replace_range(byte_start..byte_end, &replacement);
    }
    Some(out)
}

fn replace_dollar_var(text: &str, old_var: &str, new_var: &str) -> String {
    // Replace $old_name with $new_name, being careful about word boundaries
    // old_var includes the $ prefix
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let old_chars: Vec<char> = old_var.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' && i + old_chars.len() <= chars.len() {
            let slice: String = chars[i..i + old_chars.len()].iter().collect();
            if slice == old_var {
                // Check that the next char is NOT alphanumeric/dash/underscore (word boundary)
                let next_idx = i + old_chars.len();
                let at_boundary = next_idx >= chars.len()
                    || !(chars[next_idx].is_alphanumeric() || chars[next_idx] == '-' || chars[next_idx] == '_');
                if at_boundary {
                    result.push_str(new_var);
                    i += old_chars.len();
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::document::DocLine;
    use crate::editor::doc_links::RenameKind;

    fn t(s: &str) -> DocLine { DocLine::Text(s.to_string()) }

    fn do_rename(lines: &[DocLine], old: &str, new: &str, kind: &RenameKind) -> Vec<String> {
        let mut lines = lines.to_vec();
        rename_in_place(&mut lines, old, new, kind);
        lines.into_iter()
            .filter_map(|l| if let DocLine::Text(s) = l { Some(s) } else { None })
            .collect()
    }

    #[test]
    fn rename_glyph_header() {
        let lines = vec![t("glyph foo 8 16")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16"]);
    }

    #[test]
    fn rename_glyph_ref() {
        let lines = vec![t("ref foo 0 0")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["ref bar 0 0"]);
    }

    #[test]
    fn rename_glyph_map() {
        let lines = vec![t("map A = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["map A = bar"]);
    }

    #[test]
    fn rename_glyph_alias() {
        let lines = vec![t("glyph new-name = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph new-name = bar"]);
    }

    #[test]
    fn rename_glyph_def_in_alias_form() {
        let lines = vec![t("glyph foo = other")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar = other"]);
    }

    #[test]
    fn rename_glyph_alias_after_flags() {
        let lines = vec![t("glyph new-name advance 8 = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph new-name advance 8 = bar"]);
    }

    #[test]
    fn rename_glyph_remap() {
        let lines = vec![t("remap liga : a b : foo -> bar-lig : c")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["remap liga : a b : quux -> bar-lig : c"]);
    }

    #[test]
    fn rename_glyph_exclude() {
        let lines = vec![t("exclude-from-sample foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["exclude-from-sample bar"]);
    }

    #[test]
    fn rename_glyph_no_partial_match() {
        let lines = vec![t("glyph foobar 8 16"), t("ref foo-ext 0 0")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph foobar 8 16", "ref foo-ext 0 0"]);
    }

    #[test]
    fn rename_name_parts_def() {
        let lines = vec![t("name-parts $init = a b c")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["name-parts $vowel = a b c"]);
    }

    #[test]
    fn rename_name_parts_ref_in_glyph() {
        let lines = vec![t("glyph hangul-($init)-l 8 16")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["glyph hangul-($vowel)-l 8 16"]);
    }

    #[test]
    fn rename_name_parts_ref_in_ref() {
        let lines = vec![t("ref hangul-init-$init 0 0")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["ref hangul-init-$vowel 0 0"]);
    }

    #[test]
    fn rename_name_parts_in_values() {
        let lines = vec![t("name-parts $combo = $init $final")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["name-parts $combo = $vowel $final"]);
    }

    #[test]
    fn rename_name_parts_no_partial() {
        let lines = vec![t("ref hangul-$initial 0 0")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        // $initial should NOT be renamed to $vowelial
        assert_eq!(result, vec!["ref hangul-$initial 0 0"]);
    }

    #[test]
    fn rename_point_plus() {
        let lines = vec![t("point +above 4 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1"]);
    }

    #[test]
    fn rename_point_minus() {
        let lines = vec![t("point -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor -top 2 1"]);
    }

    #[test]
    fn rename_point_both_variants() {
        let lines = vec![t("point +above 4 1"), t("point -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1", "anchor -top 2 1"]);
    }

    #[test]
    fn rename_glyph_assert_same() {
        let lines = vec![t("assert same foo bar")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["assert same quux bar"]);
    }

    #[test]
    fn rename_glyph_assert_shape() {
        // Mutation follows the same classification the rename popup uses,
        // so `assert shape` glyph slots rename too.
        let lines = vec![t("assert shape AB : foo : b-upper")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["assert shape AB : quux : b-upper"]);
    }

    #[test]
    fn rename_preserves_irregular_spacing() {
        let lines = vec![t("  remap liga :  foo   ->  bar")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["  remap liga :  quux   ->  bar"]);
    }

    #[test]
    fn rename_leaves_unrelated_lines() {
        let lines = vec![
            t("glyph foo 8 16"),
            t("ref baz 0 0"),
            t("map X = foo"),
        ];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16", "ref baz 0 0", "map X = bar"]);
    }
}

#[cfg(test)]
mod font_build_tests {
    use super::*;

    #[test]
    fn stale_background_font_cannot_replace_current_result() {
        let (tx, rx) = mpsc::channel();
        let m2: HashMap<String, u16> = HashMap::new();
        let m1: HashMap<String, u16> = HashMap::new();
        tx.send((2, Some(((vec![2], vec![20]), m2.clone())))).unwrap();
        tx.send((1, Some(((vec![1], vec![10]), m1)))).unwrap();

        let result = take_current_font_build(&rx, 2);
        assert!(result.is_some());
        let inner = result.unwrap();
        assert!(inner.is_some());
        let ((bitmap, vector), _map) = inner.unwrap();
        assert_eq!(bitmap, vec![2]);
        assert_eq!(vector, vec![20]);
    }

    #[test]
    fn current_failed_build_clears_previous_font() {
        let (tx, rx) = mpsc::channel();
        tx.send((3, None)).unwrap();

        let result = take_current_font_build(&rx, 3);
        assert!(result.is_some());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn opening_document_replaces_base_in_path_order() {
        let base_a = Document::new("a.unf".into());
        let base_b = Document::new("b.unf".into());
        let open_b = OpenDocument {
            document: Document::new("b.unf".into()),
            lines: Vec::new(),
            editor_state: EditorState::new(),
        };

        let open = [open_b];
        let base = [base_b, base_a];
        let docs = collect_effective_docs(&open, &base);
        assert_eq!(
            docs.iter().map(|doc| doc.path.as_path()).collect::<Vec<_>>(),
            [std::path::Path::new("a.unf"), std::path::Path::new("b.unf")],
        );
        assert_eq!(docs.len(), 2, "the base copy of b.unf must be replaced");
    }
}
