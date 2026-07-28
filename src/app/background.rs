//! The background pipeline: font builds, derived-data resolution and shape
//! assertions running off the UI thread, and how their results are applied.

use super::*;
use super::docs::shadowed_by_open;

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

    pub(super) fn rebuild_named_glyphs_sync(&mut self) {
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

    /// Schedules debounced font/derived-data rebuilds and drains the three
    /// background channels (font build, derived data, shape assertions).
    pub(super) fn pump_background_pipeline(&mut self, ctx: &egui::Context) {
        let any_pixel_painting = self.open_documents.iter()
            .any(|d| d.editor_state.suppress_font_rebuild);
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
    }
}

#[cfg(test)]
mod font_build_tests {
    use super::*;
    use crate::app::docs::{OpenDocument, collect_effective_docs};

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
