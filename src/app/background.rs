//! The background pipeline: font builds, derived-data resolution and shape
//! assertions running off the UI thread, and how their results are applied.
//!
//! Rebuilds are debounced — 300 ms after an edit, 1000 ms after text input,
//! since typing a glyph name produces a burst of states nobody wants built — and
//! guarded against overlapping rebuild threads (`derived_inflight`): without
//! that guard a resolve slower than the debounce period respawns another rebuild
//! every period, which is how a slow resolve once snowballed into dozens of
//! concurrent threads. Set `UNIFORM_PERF` for `[perf]` per-stage timings.
//!
//! Results carry the generation of the request that produced them, and consumers
//! key their caches on the generation of the *result* they read, never of the
//! request; [`crate::specimen`] is where getting that wrong shows.

use super::docs::shadowed_by_open;
use super::*;

/// A background thread's result, sent when the thread ends — *however* it ends.
///
/// A panicking worker used to leave the UI waiting for a message that would
/// never arrive, and every one of these threads latches a flag while it runs:
/// `derived_inflight` stayed set so no later resolve was ever started,
/// `assert_running` stayed set so "Running shape assertions…" never cleared and
/// the run could not even be retried, and the same for the watcher's `scanning`.
/// One panic deep in the builder therefore froze a whole part of the editor
/// until it was restarted. The value the slot is created with is what the UI
/// receives if the thread unwinds; the worker overwrites it with the real
/// result on the way out.
pub(super) struct ResultSlot<T> {
    tx: mpsc::Sender<T>,
    ctx: egui::Context,
    value: Option<T>,
}

impl<T> ResultSlot<T> {
    pub(super) fn new(tx: mpsc::Sender<T>, ctx: egui::Context, on_panic: T) -> Self {
        Self {
            tx,
            ctx,
            value: Some(on_panic),
        }
    }

    pub(super) fn set(&mut self, value: T) {
        self.value = Some(value);
    }
}

impl<T> Drop for ResultSlot<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            let _ = self.tx.send(value);
        }
        self.ctx.request_repaint();
    }
}

pub(super) enum BackgroundTaskPhase {
    Running(std::time::Instant),
    Finished(std::time::Instant, std::time::Duration),
}

pub(super) struct BackgroundTaskStatus {
    pub(super) build: Option<BackgroundTaskPhase>,
    pub(super) test: Option<BackgroundTaskPhase>,
}

impl BackgroundTaskStatus {
    pub(super) fn new() -> Self {
        Self {
            build: None,
            test: None,
        }
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
        if let Some(BackgroundTaskPhase::Finished(at, _)) = self.build
            && at.elapsed() >= expire
        {
            self.build = None;
        }
        if let Some(BackgroundTaskPhase::Finished(at, _)) = self.test
            && at.elapsed() >= expire
        {
            self.test = None;
        }
    }
}

fn take_current_font_build(
    rx: &mpsc::Receiver<FontBuildMessage>,
    current_gen: u64,
) -> Option<Option<crate::render::BuiltFontPair>> {
    let mut received = None;
    while let Ok((build_gen, result)) = rx.try_recv() {
        if build_gen == current_gen {
            received = Some(result);
        }
    }
    received
}

fn take_latest_derived_data(rx: &mpsc::Receiver<DerivedDataResult>) -> Option<DerivedDataResult> {
    let mut received = None;
    while let Ok(msg) = rx.try_recv() {
        received = Some(msg);
    }
    received
}

impl UniformApp {
    pub(super) fn current_font_gen(&self) -> u64 {
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

    pub(super) fn run_shape_assertions(&mut self, ctx: &egui::Context, current_file_only: bool) {
        if self.assert_running {
            return;
        }
        self.assert_running = true;
        self.bg_tasks.start_test();
        self.status_message = Some((
            "Running shape assertions...".to_string(),
            std::time::Instant::now(),
        ));

        let all_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let active_path = if current_file_only {
            self.active_doc_idx()
                .map(|i| self.open_documents[i].document.path.clone())
        } else {
            None
        };
        let tx = self.assert_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut slot = ResultSlot::new(
                tx,
                ctx,
                vec![Issue {
                    severity: crate::issues::Severity::Error,
                    message: "Shape assertions failed to run (internal error)".to_string(),
                    file: std::path::PathBuf::new(),
                    line: 0,
                    file_line: 0,
                }],
            );
            let refs: Vec<&Document> = all_docs.iter().collect();
            let name_parts = crate::document::collect_name_parts(&refs);
            let (resolved, _) =
                crate::ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts);

            let mut result = if let Some(built) = crate::render::build_font_with_gid_map(&refs) {
                let assert_result = if let Some(path) = &active_path {
                    let test_docs: Vec<&Document> =
                        all_docs.iter().filter(|d| &d.path == path).collect();
                    crate::render::assert::run_assertions_for_files(
                        &test_docs,
                        &refs,
                        &built.ttf,
                        &built.gid_to_name,
                        built.height,
                    )
                } else {
                    crate::render::assert::run_assertions(
                        &refs,
                        &built.ttf,
                        &built.gid_to_name,
                        built.height,
                    )
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
                let test_docs: Vec<&Document> =
                    all_docs.iter().filter(|d| &d.path == path).collect();
                crate::render::assert::run_same_distinct_assertions_for_files(&test_docs, &resolved)
            } else {
                crate::render::assert::run_same_distinct_assertions(&refs, &resolved)
            };
            result.extend(sd_result.issues);

            slot.set(result);
        });
    }

    fn rebuild_font(&mut self, ctx: &egui::Context) {
        self.bg_tasks.start_build();
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let tx = self.font_build_tx.clone();
        let ctx = ctx.clone();
        let cache = self.contour_cache.clone();
        let face = self.selected_face.clone();
        std::thread::spawn(move || {
            let mut slot = ResultSlot::new(tx, ctx, (build_gen, None));
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = owned_docs.iter().collect();
            let face_id = (!face.is_empty()).then_some(face.as_str());
            let pair = crate::render::build_font_pair_cached_for(&refs, &cache, face_id);
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] font build (background): {:?}", t0.elapsed());
            }
            slot.set((build_gen, pair));
        });
    }

    /// Move the selection `delta` faces along the declared order, wrapping.
    /// F11 and F10 in [`crate::app::UniformApp::update`].
    pub(super) fn step_face(&mut self, delta: isize, ctx: &egui::Context) {
        let current = self.selected_face().to_string();
        if let Some(next) = step_face_id(&self.face_ids, &current, delta) {
            self.set_selected_face(next, ctx);
        }
    }

    /// The face the built font actually reflects: the selection, or the
    /// primary face when nothing is selected or the selection names a face the
    /// source no longer declares. Mirrors the fallback in
    /// [`crate::render::build_font_pair_cached_for`], so the picker cannot show
    /// a face the preview is not drawn with.
    pub(super) fn selected_face(&self) -> &str {
        if self.face_ids.contains(&self.selected_face) {
            &self.selected_face
        } else {
            self.face_ids.first().map(String::as_str).unwrap_or("")
        }
    }

    /// Switch which face the editor builds, and rebuild the font at once —
    /// the debounce exists to absorb keystrokes, and a picked face is a single
    /// deliberate act with nothing to absorb.
    ///
    /// The generation is bumped because every consumer of the built font keys
    /// its cache on it: without that the preview would keep shaping with the
    /// old face's cmap.
    pub(super) fn set_selected_face(&mut self, face: String, ctx: &egui::Context) {
        if self.selected_face == face {
            return;
        }
        self.selected_face = face;
        self.font_build_gen = self.font_build_gen.wrapping_add(1);
        self.font_rebuild_at = None;
        self.rebuild_font(ctx);
    }

    pub(super) fn rebuild_named_glyphs_sync(&mut self) {
        let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
        let all_docs = self.collect_all_docs();
        let name_parts = crate::document::collect_name_parts(&all_docs);
        let (named_glyphs, alt_index) =
            crate::editor::ref_composite::resolve_named_glyphs_with_parts(&all_docs, &name_parts);
        let font_meta = crate::meta::FontMeta::collect(&all_docs);
        drop(all_docs);
        self.font_meta = font_meta.metrics;
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
            let mut slot = ResultSlot::new(tx, ctx, None);
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = owned_docs.iter().collect();
            // One resolution feeds both the glyph cache and validation; they
            // used to expand the whole document set independently.
            let resolution = crate::resolve::Resolution::compute(&refs);
            // Validation only reads names and diagnostics, so it runs before
            // the expansion is consumed by the glyph cache.
            let mut issues = crate::issues::collect_issues_with(&refs, &resolution);
            let face_ids: Vec<String> = resolution
                .faces
                .faces
                .iter()
                .map(|f| f.id.clone())
                .collect();
            let name_parts = resolution.name_parts;
            let (named_glyphs, alt_index) =
                crate::editor::ref_composite::resolve_expansion(resolution.expansion, &name_parts);
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] resolve (derived thread): {:?}", t0.elapsed());
            }
            for (path, msg) in &file_parse_errors {
                issues.insert(
                    0,
                    Issue {
                        severity: crate::issues::Severity::Error,
                        message: msg.clone(),
                        file: path.clone(),
                        line: 0,
                        file_line: 1,
                    },
                );
            }
            slot.set(Some(DerivedDataMessage {
                build_gen,
                named_glyphs,
                alt_index,
                meta: resolution.meta.metrics,
                name_parts,
                issues,
                face_ids,
            }));
        });
    }

    pub(super) fn apply_font(&mut self, ctx: &egui::Context) {
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

        let (mut bitmap_list, mut vector_list) =
            if let Some((bitmap_ttf, vector_ttf)) = &self.font_data {
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

        fonts.families.insert(bitmap_family, bitmap_list.clone());
        fonts.families.insert(vector_family, vector_list);

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

    /// Schedules debounced font/derived-data rebuilds and drains the three
    /// background channels (font build, derived data, shape assertions).
    pub(super) fn pump_background_pipeline(&mut self, ctx: &egui::Context) {
        let any_pixel_painting = self
            .open_documents
            .iter()
            .any(|d| d.editor_state.suppress_font_rebuild);
        let font_gen = self.current_font_gen();
        if font_gen != self.last_font_gen && !any_pixel_painting {
            self.last_font_gen = font_gen;
            self.font_build_gen = self.font_build_gen.wrapping_add(1);
            let had_text_input =
                ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_))));
            let debounce_ms = if had_text_input { 1000 } else { 300 };
            self.font_rebuild_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(debounce_ms));
            ctx.request_repaint_after(std::time::Duration::from_millis(debounce_ms));
        }
        if let Some(at) = self.font_rebuild_at
            && std::time::Instant::now() >= at
        {
            self.rebuild_font(ctx);
            self.font_rebuild_at = None;
        }

        if let Some(result) = take_current_font_build(&self.font_build_rx, self.font_build_gen) {
            self.bg_tasks.finish_build();
            match result {
                Some(built) => {
                    self.font_data = Some((built.bitmap, built.vector));
                    self.font_name_to_gid = built.name_to_gid;
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

        if let Some(result) = take_latest_derived_data(&self.derived_data_rx) {
            self.derived_inflight = false;
            // A rebuild that died carries nothing: the flag is cleared either
            // way, but the previous derived data stays — a stale view of the
            // font beats none — and the next edit schedules another rebuild.
            if result.is_none() {
                self.status_message = Some((
                    "Resolving the font sources failed (internal error).".to_string(),
                    std::time::Instant::now(),
                ));
            }
            if let Some(data) = result {
                self.named_glyphs = data.named_glyphs;
                self.alt_index = data.alt_index;
                self.name_parts = data.name_parts;
                self.font_meta = data.meta;
                self.named_glyphs_gen = data.build_gen;
                self.derived_gen = self.derived_gen.wrapping_add(1);
                self.issues = data.issues;
                self.issues_gen = data.build_gen;
                self.face_ids = data.face_ids;
                // The selection is not silently rewritten when its face goes
                // away: the build falls back to the primary on its own, and an
                // edit that briefly breaks a `face` line must not lose the
                // choice.
                let all_docs = self.collect_all_docs();
                let doc_refs: Vec<&Document> = all_docs.to_vec();
                self.color_aliases = crate::render::ttf_builder::collect_color_aliases(&doc_refs);
            }
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
    }
}

/// The face `delta` steps along `faces` from `current`, wrapping at both ends,
/// or `None` when there is nowhere to go. An unrecognized `current` — the
/// selection outlives the `face` line it named — steps from the primary.
pub(super) fn step_face_id(faces: &[String], current: &str, delta: isize) -> Option<String> {
    let n = faces.len() as isize;
    if n < 2 {
        return None;
    }
    let idx = faces.iter().position(|f| f == current).unwrap_or(0) as isize;
    Some(faces[(idx + delta).rem_euclid(n) as usize].clone())
}

#[cfg(test)]
mod font_build_tests {
    use super::*;
    use crate::app::docs::{OpenDocument, collect_effective_docs};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// F11/F10 walk the declared order and wrap, so the pair is enough to reach
    /// every face of a two-face source with either key.
    #[test]
    fn stepping_a_face_wraps_around_the_declared_order() {
        let faces = ids(&["regular", "term", "mono"]);
        assert_eq!(step_face_id(&faces, "regular", 1).unwrap(), "term");
        assert_eq!(step_face_id(&faces, "mono", 1).unwrap(), "regular");
        assert_eq!(step_face_id(&faces, "regular", -1).unwrap(), "mono");
        assert_eq!(step_face_id(&faces, "term", -1).unwrap(), "regular");
    }

    /// A stale selection — the `face` line it named was just edited away —
    /// steps from the primary rather than doing nothing.
    #[test]
    fn stepping_from_an_unknown_face_starts_at_the_primary() {
        let faces = ids(&["regular", "term"]);
        assert_eq!(step_face_id(&faces, "gone", 1).unwrap(), "term");
    }

    /// Nothing to step through: a single-face source (and the implicit face of
    /// a source declaring none) has no second face to reach.
    #[test]
    fn stepping_a_single_face_source_does_nothing() {
        assert_eq!(step_face_id(&ids(&["regular"]), "regular", 1), None);
        assert_eq!(step_face_id(&[], "", 1), None);
    }

    #[test]
    fn stale_background_font_cannot_replace_current_result() {
        let (tx, rx) = mpsc::channel();
        let built = |n: u8| crate::render::BuiltFontPair {
            bitmap: vec![n],
            vector: vec![n * 10],
            name_to_gid: HashMap::new(),
        };
        tx.send((2, Some(built(2)))).unwrap();
        tx.send((1, Some(built(1)))).unwrap();

        let result = take_current_font_build(&rx, 2);
        assert!(result.is_some());
        let inner = result.unwrap();
        assert!(inner.is_some());
        let inner = inner.unwrap();
        assert_eq!(inner.bitmap, vec![2]);
        assert_eq!(inner.vector, vec![20]);
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
            disk_hash: None,
            external_change: false,
            owed_external_toast: false,
        };

        let open = [open_b];
        let base = [base_b, base_a];
        let docs = collect_effective_docs(&open, &base);
        assert_eq!(
            docs.iter()
                .map(|doc| doc.path.as_path())
                .collect::<Vec<_>>(),
            [std::path::Path::new("a.unf"), std::path::Path::new("b.unf")],
        );
        assert_eq!(docs.len(), 2, "the base copy of b.unf must be replaced");
    }

    /// A worker that unwinds still delivers a result, so the flag the UI
    /// latched while it ran is cleared. Without this the editor's resolve, its
    /// shape-assertion run or its file watch stops for the rest of the session
    /// after a single panic deep in the builder.
    #[test]
    fn a_panicking_worker_still_sends_its_fallback() {
        let (tx, rx) = mpsc::channel::<&'static str>();
        let ctx = egui::Context::default();

        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicking = std::thread::spawn({
            let tx = tx.clone();
            let ctx = ctx.clone();
            move || {
                let mut slot = ResultSlot::new(tx, ctx, "died");
                if std::hint::black_box(true) {
                    panic!("deep in the builder");
                }
                slot.set("finished");
            }
        });
        assert!(panicking.join().is_err());
        std::panic::set_hook(hook);
        assert_eq!(rx.try_recv(), Ok("died"));

        std::thread::spawn(move || {
            let mut slot = ResultSlot::new(tx, ctx, "died");
            slot.set("finished");
        })
        .join()
        .unwrap();
        assert_eq!(rx.try_recv(), Ok("finished"), "the real result wins");
    }
}
