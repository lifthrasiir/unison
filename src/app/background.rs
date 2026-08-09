//! The background pipeline: font builds, derived-data resolution and shape
//! assertions running off the UI thread, and how their results are applied.
//!
//! Rebuilds are debounced — 300 ms after an edit, 1000 ms after text input,
//! since typing a glyph name produces a burst of states nobody wants built — and
//! guarded against overlapping rebuild threads (`font_build_inflight`,
//! `derived_inflight`): without that guard a stage slower than the debounce
//! period respawns another every period, which is how a slow resolve once
//! snowballed into dozens of concurrent threads. Set `UNIFORM_PERF` for
//! `[perf]` per-stage timings.
//!
//! # At most one of each stage, and it can be told to stop
//!
//! The guard alone only moves the pile-up: a second build does not overlap the
//! first, it *queues behind* it on the shared contour cache, so a burst of
//! pixel clicks still meant the last edit's font appearing several full builds
//! later. So each stage also holds a [`CancelToken`](crate::cancel::CancelToken),
//! and the scheduler follows one rule:
//!
//! - a request arriving while the stage is idle starts it;
//! - a request arriving while it runs **cancels** what runs and re-arms itself,
//!   so the pump starts the new one as soon as the slot frees.
//!
//! There is therefore never a queue to drain, only ever one obsolete stage
//! being wound down. A cancelled stage reports back like any other — the slot
//! has to be freed however it ended — but carries `Cancelled` rather than a
//! result, because "nothing came out of this" and "this font is empty" must not
//! look alike: the second blanks the display.
//!
//! Cancelling the font build is unconditional (a new generation supersedes it
//! by definition); cancelling the resolve is not, since a resolve already
//! running against the current generation is exactly the one wanted.
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

/// Drains the font-build channel, reporting whether any thread ended (so the
/// scheduler can free the slot) and, separately, the one result worth applying.
///
/// A cancelled build is a thread that ended with nothing, and a build for a
/// superseded generation is a result nobody may apply — the two are dropped by
/// the same filter, which is why cancelling needs no special case here.
fn take_current_font_build(
    rx: &mpsc::Receiver<FontBuildMessage>,
    current_gen: u64,
) -> (bool, Option<Option<crate::render::BuiltFontPair>>) {
    let mut ended = false;
    let mut received = None;
    while let Ok((build_gen, outcome)) = rx.try_recv() {
        ended = true;
        if let FontBuildOutcome::Done(result) = outcome
            && build_gen == current_gen
        {
            received = Some(result);
        }
    }
    (ended, received)
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
        // The derived-data thread has already resolved every glyph of exactly
        // these sources; when its result is current there is nothing to redo.
        // `named_glyphs_gen` is the build generation it was resolved from, so
        // this is the same check every other consumer of the derived data makes.
        let resolved =
            (self.named_glyphs_gen == self.font_build_gen).then(|| Arc::clone(&self.named_glyphs));
        // Shared with the font build, and warm from it: the vector contours a
        // face build needs are mostly the ones the displayed font was traced
        // from. See `build_font_with_gid_map_for_cached`.
        let cache = self.contour_cache.clone();
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
            // Which files the assertions are *read* from; which faces exist and
            // what each contains stays a property of the whole source.
            let test_docs: Vec<&Document> = match &active_path {
                Some(path) => all_docs.iter().filter(|d| &d.path == path).collect(),
                None => refs.clone(),
            };

            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let shape_result =
                crate::render::assert::run_assertions_for_files(&test_docs, &refs, &mut |face| {
                    crate::render::build_font_with_gid_map_for_cached(&refs, face, &cache)
                });
            let mut result = shape_result.issues;
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] shape assertions: {:?}", t0.elapsed());
            }

            // Resolution is as expensive as a font build, so it is neither done
            // when nothing asks for it nor redone when the editor already has it.
            if crate::render::assert::has_same_distinct_assertions(&test_docs) {
                let perf_t1 = perf_log_enabled().then(std::time::Instant::now);
                let resolved = resolved.unwrap_or_else(|| {
                    let name_parts = crate::document::collect_name_parts(&refs);
                    Arc::new(
                        crate::ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts).0,
                    )
                });
                let sd_result = crate::render::assert::run_same_distinct_assertions_for_files(
                    &test_docs, &resolved,
                );
                result.extend(sd_result.issues);
                if let Some(t1) = perf_t1 {
                    eprintln!("[perf] same/distinct assertions: {:?}", t1.elapsed());
                }
            }

            slot.set(result);
        });
    }

    /// Start a font build for the current generation, or — when one is already
    /// running — cancel that one and leave the request armed for the pump to
    /// retry once the slot frees.
    ///
    /// Waiting for the running build rather than spawning alongside it is the
    /// point: two builds do not overlap, they serialize on the contour cache,
    /// and the second one's wait is the whole latency this avoids.
    fn rebuild_font(&mut self, ctx: &egui::Context) {
        if self.font_build_inflight {
            self.font_cancel.cancel();
            self.font_rebuild_at = Some(std::time::Instant::now());
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
            return;
        }

        self.bg_tasks.start_build();
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let tx = self.font_build_tx.clone();
        let ctx = ctx.clone();
        let cache = self.contour_cache.clone();
        let face = self.selected_face.clone();
        let cancel = crate::cancel::CancelToken::new();
        self.font_cancel = cancel.clone();
        self.font_build_inflight = true;
        std::thread::spawn(move || {
            let mut slot = ResultSlot::new(tx, ctx, (build_gen, FontBuildOutcome::Done(None)));
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = owned_docs.iter().collect();
            let face_id = (!face.is_empty()).then_some(face.as_str());
            let pair = crate::render::build_font_pair_cached_for(&refs, &cache, face_id, &cancel);
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] font build (background): {:?}", t0.elapsed());
            }
            // A cancelled build returns `None` like a failed one; only the
            // token tells the two apart, and only so a cancellation does not
            // blank the displayed font.
            let outcome = if cancel.is_cancelled() {
                FontBuildOutcome::Cancelled
            } else {
                FontBuildOutcome::Done(pair)
            };
            slot.set((build_gen, outcome));
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
        // Recorded against the directory the moment it is chosen, not at
        // save time, so that switching directories does not lose the choice
        // made in the one being left.
        if let Some(dir) = self.font_dir.clone() {
            self.settings.remember_face(&dir, &face);
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
        let char_props = crate::ucd::CharProps::collect(&all_docs);
        drop(all_docs);
        self.font_meta = font_meta.metrics;
        self.named_glyphs = std::sync::Arc::new(named_glyphs);
        self.alt_index = alt_index;
        self.name_parts = name_parts;
        self.char_props = char_props;
        self.derived_gen = self.derived_gen.wrapping_add(1);
        if let Some(t0) = perf_t0 {
            eprintln!("[perf] resolve (sync, main thread): {:?}", t0.elapsed());
        }
    }

    fn rebuild_derived_data(&mut self, ctx: &egui::Context) {
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let file_parse_errors = self.file_parse_errors.clone();
        let tx = self.derived_data_tx.clone();
        let ctx = ctx.clone();
        let cancel = crate::cancel::CancelToken::new();
        self.derived_cancel = cancel.clone();
        self.derived_inflight = Some(build_gen);
        std::thread::spawn(move || {
            let mut slot = ResultSlot::new(tx, ctx, DerivedDataResult::Failed);
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = owned_docs.iter().collect();
            // One resolution feeds both the glyph cache and validation; they
            // used to expand the whole document set independently.
            let Some(resolution) = crate::resolve::Resolution::compute_cancellable(&refs, &cancel)
            else {
                slot.set(DerivedDataResult::Cancelled);
                return;
            };
            // Validation only reads names and diagnostics, so it runs before
            // the expansion is consumed by the glyph cache.
            let mut issues = crate::issues::collect_issues_with(&refs, &resolution);
            let char_props = crate::ucd::CharProps::collect(&refs);
            let face_ids: Vec<String> = resolution
                .faces
                .faces
                .iter()
                .map(|f| f.id.clone())
                .collect();
            let name_parts = resolution.name_parts;
            let (named_glyphs, alt_index) = crate::editor::ref_composite::resolve_expansion(
                resolution.expansion,
                &name_parts,
                &cancel,
            );
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] resolve (derived thread): {:?}", t0.elapsed());
            }
            // Resolution stops where it was interrupted, so what it holds is a
            // partial font; it is discarded rather than published.
            if cancel.is_cancelled() {
                slot.set(DerivedDataResult::Cancelled);
                return;
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
            slot.set(DerivedDataResult::Done(Box::new(DerivedDataMessage {
                build_gen,
                named_glyphs,
                alt_index,
                meta: resolution.meta.metrics,
                name_parts,
                char_props,
                issues,
                face_ids,
            })));
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
        // Drained before anything is scheduled, so a build that just ended
        // frees its slot for a request armed in this very frame rather than in
        // the next one.
        let (build_ended, build_result) =
            take_current_font_build(&self.font_build_rx, self.font_build_gen);
        if build_ended {
            self.font_build_inflight = false;
        }
        if let Some(result) = build_result {
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

        if font_gen != self.last_font_gen && !any_pixel_painting {
            self.last_font_gen = font_gen;
            self.font_build_gen = self.font_build_gen.wrapping_add(1);
            // Whatever is building is building a document set that no longer
            // exists. Told now rather than when the debounce expires, it has
            // the whole debounce period to notice and get out of the way.
            self.font_cancel.cancel();
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
            // Cleared first: `rebuild_font` re-arms it itself when it has to
            // wait for the build it just cancelled.
            self.font_rebuild_at = None;
            self.rebuild_font(ctx);
        }

        if let Some(result) = take_latest_derived_data(&self.derived_data_rx) {
            self.derived_inflight = None;
            match result {
                // The previous derived data stays in both non-`Done` cases — a
                // stale view of the font beats none — but only a rebuild that
                // *died* is something the user should hear about; one that was
                // cancelled is about to be replaced by design.
                DerivedDataResult::Failed => {
                    self.status_message = Some((
                        "Resolving the font sources failed (internal error).".to_string(),
                        std::time::Instant::now(),
                    ));
                }
                DerivedDataResult::Cancelled => {}
                DerivedDataResult::Done(data) => {
                    let data = *data;
                    self.named_glyphs = std::sync::Arc::new(data.named_glyphs);
                    self.alt_index = data.alt_index;
                    self.name_parts = data.name_parts;
                    self.char_props = data.char_props;
                    self.font_meta = data.meta;
                    self.named_glyphs_gen = data.build_gen;
                    self.derived_gen = self.derived_gen.wrapping_add(1);
                    self.issues = data.issues;
                    self.issues_gen = data.build_gen;
                    self.face_ids = data.face_ids;
                    // The first moment a face id restored from the settings
                    // can be checked against what this directory declares. An
                    // id it no longer has is dropped rather than selected —
                    // the source is edited between runs, and a face can go
                    // away.
                    if let Some(face) = self.pending_face.take()
                        && self.face_ids.contains(&face)
                    {
                        self.set_selected_face(face, ctx);
                    }
                    // The selection is not silently rewritten when its face
                    // goes away: the build falls back to the primary on its
                    // own, and an edit that briefly breaks a `face` line must
                    // not lose the choice.
                    let all_docs = self.collect_all_docs();
                    let doc_refs: Vec<&Document> = all_docs.to_vec();
                    self.color_aliases =
                        crate::render::ttf_builder::collect_color_aliases(&doc_refs);
                }
            }
        }

        // Unlike the font build, a resolve in flight may already be the one
        // wanted: it is cancelled only when the generation it started from has
        // been superseded, and a new one is armed only when none in flight will
        // deliver the current generation.
        if self.font_build_gen != self.named_glyphs_gen {
            if self
                .derived_inflight
                .is_some_and(|g| g != self.font_build_gen)
            {
                self.derived_cancel.cancel();
            }
            if self.derived_rebuild_at.is_none()
                && self.derived_inflight != Some(self.font_build_gen)
            {
                self.derived_rebuild_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(300));
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
        if let Some(at) = self.derived_rebuild_at
            && std::time::Instant::now() >= at
        {
            if self.derived_inflight.is_some() {
                // Waiting on the cancelled resolve rather than starting a
                // second one: two concurrent resolves of an 18k-glyph font
                // starve each other, which is what the guard has always been
                // for.
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            } else {
                self.derived_rebuild_at = None;
                self.rebuild_derived_data(ctx);
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
        tx.send((2, FontBuildOutcome::Done(Some(built(2)))))
            .unwrap();
        tx.send((1, FontBuildOutcome::Done(Some(built(1)))))
            .unwrap();

        let (ended, result) = take_current_font_build(&rx, 2);
        assert!(ended, "both threads ended, so the build slot is free");
        let inner = result.expect("the current generation's build is applied");
        let inner = inner.expect("that build produced a font");
        assert_eq!(inner.bitmap, vec![2]);
        assert_eq!(inner.vector, vec![20]);
    }

    /// A cancelled build frees the slot the scheduler waits on, but is never
    /// applied — not even when it happens to carry the current generation,
    /// which is the case a `Done(None)` of the same generation would blank the
    /// displayed font for.
    #[test]
    fn a_cancelled_build_frees_the_slot_without_replacing_the_font() {
        let (tx, rx) = mpsc::channel::<FontBuildMessage>();
        tx.send((4, FontBuildOutcome::Cancelled)).unwrap();

        let (ended, result) = take_current_font_build(&rx, 4);
        assert!(ended);
        assert!(result.is_none(), "nothing to apply from a cancelled build");
    }

    #[test]
    fn current_failed_build_clears_previous_font() {
        let (tx, rx) = mpsc::channel();
        tx.send((3, FontBuildOutcome::Done(None))).unwrap();

        let (ended, result) = take_current_font_build(&rx, 3);
        assert!(ended);
        assert!(result.expect("a current result, applied").is_none());
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
