//! The background pipeline: font builds, derived-data resolution and shape
//! assertions running off the UI thread, and how their results are applied.
//!
//! Rebuilds are debounced — 300 ms after an edit, 1000 ms after text input,
//! since typing a glyph name produces a burst of states nobody wants built — and
//! guarded against overlapping rebuild threads (`rebuild_inflight`): without
//! that guard a rebuild slower than the debounce period respawns another every
//! period, which is how a slow resolve once snowballed into dozens of
//! concurrent threads. Set `UNIFORM_PERF` for `[perf]` per-stage timings.
//!
//! # One rebuild, and it can be told to stop
//!
//! The font and the derived data are *one* thread, not two: both are wanted by
//! the same edit, both key on the same generation, and both start from the same
//! expansion — which [`UniformApp::rebuild`] therefore computes once and lends
//! to both. They still report separately, because the UI applies them
//! separately: a font that is ready need not wait for validation.
//!
//! The guard alone only moves a pile-up: a second rebuild does not overlap the
//! first, it *queues behind* it on the shared contour cache, so a burst of
//! pixel clicks still meant the last edit's font appearing several full builds
//! later. So the rebuild also holds a [`CancelToken`](crate::cancel::CancelToken),
//! and the scheduler follows one rule:
//!
//! - a request arriving while nothing runs starts it;
//! - a request arriving while one runs **cancels** it and re-arms itself, so
//!   the pump starts the new one as soon as the slot frees.
//!
//! There is therefore never a queue to drain, only ever one obsolete rebuild
//! being wound down. A cancelled rebuild reports back like any other — the slot
//! has to be freed however it ended — but carries `Cancelled` rather than a
//! result, because "nothing came out of this" and "this font is empty" must not
//! look alike: the second blanks the display.
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
/// `rebuild_inflight` stayed set so no later rebuild was ever started,
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

/// The background work the status bar shows a stopwatch for, one slot each.
/// Every slot behaves the same way — running, then finished and readable for a
/// few seconds — so they are started, finished and expired by the same three
/// functions rather than one pair per slot.
pub(super) struct BackgroundTaskStatus {
    pub(super) build: Option<BackgroundTaskPhase>,
    pub(super) test: Option<BackgroundTaskPhase>,
    /// A `uniform fix` run started from the Font menu.
    pub(super) optimize: Option<BackgroundTaskPhase>,
}

fn start(slot: &mut Option<BackgroundTaskPhase>) {
    *slot = Some(BackgroundTaskPhase::Running(std::time::Instant::now()));
}

fn finish(slot: &mut Option<BackgroundTaskPhase>) {
    if let Some(BackgroundTaskPhase::Running(began)) = slot {
        *slot = Some(BackgroundTaskPhase::Finished(
            std::time::Instant::now(),
            began.elapsed(),
        ));
    }
}

impl BackgroundTaskStatus {
    pub(super) fn new() -> Self {
        Self {
            build: None,
            test: None,
            optimize: None,
        }
    }

    fn slots(&mut self) -> [&mut Option<BackgroundTaskPhase>; 3] {
        [&mut self.build, &mut self.test, &mut self.optimize]
    }

    fn gc(&mut self) {
        let expire = std::time::Duration::from_secs(10);
        for slot in self.slots() {
            if let Some(BackgroundTaskPhase::Finished(at, _)) = slot
                && at.elapsed() >= expire
            {
                *slot = None;
            }
        }
    }
}

/// Drains the font-build channel down to the one result worth applying.
///
/// A cancelled build is a rebuild that produced nothing, and a build for a
/// superseded generation is a result nobody may apply — the two are dropped by
/// the same filter, which is why cancelling needs no special case here.
///
/// Says nothing about whether the rebuild ended: the font message is sent as
/// soon as the bytes exist, with the rest of the rebuild still behind it.
fn take_current_font_build(
    rx: &mpsc::Receiver<FontBuildMessage>,
    current_gen: u64,
) -> Option<Option<crate::render::BuiltFontPair>> {
    let mut received = None;
    while let Ok((build_gen, outcome)) = rx.try_recv() {
        if let FontBuildOutcome::Done(result) = outcome
            && build_gen == current_gen
        {
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
        start(&mut self.bg_tasks.test);
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
                    glyph: None,
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

    /// Plan every clearance rewrite the source's `audit ideal-clearance` rules
    /// ask for, off the UI thread.
    ///
    /// It reads the documents and writes nothing, which is what lets it run
    /// like the assertions do — on a copy of the sources, with the result
    /// applied on the UI thread once it lands ([`super::fix`]). At 20k glyphs
    /// the search is a second of walking, and the editor must not stop for it.
    pub(super) fn run_clearance_optimizer(&mut self, ctx: &egui::Context) {
        if self.fix_running {
            return;
        }
        self.fix_running = true;
        start(&mut self.bg_tasks.optimize);
        self.set_status("Optimizing clearances...".to_string());

        let all_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let tx = self.fix_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // A worker that dies delivers an empty plan, which reads as "there
            // was nothing to do" — the flag it latched is cleared either way.
            let mut slot = ResultSlot::new(tx, ctx, Vec::new());
            let perf_t0 = perf_log_enabled().then(std::time::Instant::now);
            let refs: Vec<&Document> = all_docs.iter().collect();
            let plan = crate::fix::clearance::optimize_clearance(&refs);
            if let Some(t0) = perf_t0 {
                eprintln!("[perf] optimize clearance: {:?}", t0.elapsed());
            }
            slot.set(plan);
        });
    }

    /// Rebuild the font and the derived data for the current generation, or —
    /// when one is already running — cancel that one and leave the request
    /// armed for the pump to retry once the slot frees.
    ///
    /// Waiting for the running rebuild rather than spawning alongside it is the
    /// point: two do not overlap, they serialize on the contour cache, and the
    /// second one's wait is the whole latency this avoids.
    ///
    /// # One expansion, two consumers
    ///
    /// The font build and the derived data used to be two threads that each
    /// expanded the whole document set for themselves — the larger half of what
    /// either costs, paid twice per edit. They are one thread now, which
    /// expands once and lends it: the font build reads it, validation reads it
    /// beside the font build, and the glyph cache consumes it last, when both
    /// readers are done. It is the arrangement `main.rs` already uses for the
    /// `build` subcommand.
    ///
    /// Merging them costs nothing in scheduling terms, because they were
    /// already locked together: both keyed on `font_build_gen`, both armed by
    /// the same edit, and the derived one could not run ahead of a font build
    /// it shared a generation with.
    ///
    /// The lending only applies where the two want the same face. The derived
    /// data is always the *primary* face's; a selection naming another face
    /// makes the font build expand its own, which is what it always did.
    fn rebuild(&mut self, ctx: &egui::Context) {
        if self.rebuild_inflight {
            self.rebuild_cancel.cancel();
            self.rebuild_log.cancelled_current();
            self.rebuild_at = Some(std::time::Instant::now());
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
            return;
        }

        start(&mut self.bg_tasks.build);
        let build_gen = self.font_build_gen;
        self.rebuild_log.started(build_gen);
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let file_parse_errors = self.file_parse_errors.clone();
        let font_tx = self.font_build_tx.clone();
        let derived_tx = self.derived_data_tx.clone();
        let contour_cache = self.contour_cache.clone();
        // Survives this thread, so the next rebuild recomposes only what an
        // edit reached. `rebuild_inflight` already keeps a second one from
        // starting, so the lock below is never contended — it is what makes the
        // cache shareable at all, not a queue.
        let grid_cache = self.composite_grid_cache.clone();
        let face = self.selected_face.clone();
        // Only when its tab is open: what the specimen reads out of the
        // documents is a third full expansion, and nobody who cannot see it
        // should pay for it. Opening the tab asks for a rebuild of its own —
        // see the pump.
        let want_specimen = self.bottom_panel_tab == Some(super::panels::SPECIMEN_TAB);
        let cancel = crate::cancel::CancelToken::new();
        self.rebuild_cancel = cancel.clone();
        self.rebuild_inflight = true;
        let font_ctx = ctx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // Dropped in reverse order of declaration, so the derived result
            // goes out first; either message frees the slot.
            let mut font_slot =
                ResultSlot::new(font_tx, font_ctx, (build_gen, FontBuildOutcome::Done(None)));
            let mut slot = ResultSlot::new(derived_tx, ctx, DerivedDataResult::Failed);
            let t0 = std::time::Instant::now();
            let mut timing = super::timing::BackgroundTiming::default();
            let refs: Vec<&Document> = owned_docs.iter().collect();
            let Some(resolution) = crate::resolve::Resolution::compute_cancellable(&refs, &cancel)
            else {
                font_slot.set((build_gen, FontBuildOutcome::Cancelled));
                slot.set(DerivedDataResult::Cancelled);
                return;
            };
            timing.expand = t0.elapsed();

            // The font build and validation both read the expansion, and
            // neither writes anything the other looks at, so they run at once.
            let face_id = (!face.is_empty()).then_some(face.as_str());
            let ((pair, font_took), (mut issues, glyph_flags, validate_took, flags_took)) =
                std::thread::scope(|scope| {
                    let build = scope.spawn(|| {
                        let t = std::time::Instant::now();
                        let lendable = crate::render::resolve_face(&resolution.faces, face_id).id
                            == resolution.faces.primary().id;
                        let pair = if lendable {
                            crate::render::build_font_pair_cached_from(
                                &refs,
                                &contour_cache,
                                &resolution,
                                &cancel,
                            )
                        } else {
                            crate::render::build_font_pair_cached_for(
                                &refs,
                                &contour_cache,
                                face_id,
                                &cancel,
                            )
                        };
                        (pair, t.elapsed())
                    });
                    let t = std::time::Instant::now();
                    let issues = crate::issues::collect_issues_with(&refs, &resolution);
                    let validate_took = t.elapsed();
                    // Computed here rather than on the UI thread because it
                    // needs the expansion, which the glyph cache consumes below.
                    let t = std::time::Instant::now();
                    let glyph_flags =
                        crate::glyph_flags::collect(&refs, &issues, &resolution.expansion);
                    // Read before the join, or the wait for the font build —
                    // which is the *other* leg of this scope and usually the
                    // longer one — is charged to whatever was measured last.
                    let flags_took = t.elapsed();
                    (
                        build.join().unwrap(),
                        (issues, glyph_flags, validate_took, flags_took),
                    )
                });
            timing.font = font_took;
            timing.validate = validate_took;
            timing.flags = flags_took;
            // A cancelled build returns `None` like a failed one; only the
            // token tells the two apart, and only so a cancellation does not
            // blank the displayed font.
            // Copied before the pair leaves for the UI, and only where the
            // specimen is going to be collected below: it needs the *built*
            // font's glyph set, which is the only honest answer to whether a
            // cell can be drawn.
            let gid_map = want_specimen
                .then(|| pair.as_ref().map(|p| p.name_to_gid.clone()))
                .flatten();
            font_slot.set((
                build_gen,
                if cancel.is_cancelled() {
                    FontBuildOutcome::Cancelled
                } else {
                    FontBuildOutcome::Done(pair)
                },
            ));
            // Sent now rather than when this thread ends, because a `ResultSlot`
            // delivers on drop and everything below — the recomposition, the
            // specimen's data — is work the *font* does not wait for. Holding it
            // to the end made the font appear a second late for no reason, and
            // made the two end-to-end numbers in the report read as one.
            drop(font_slot);

            let char_props = crate::ucd::CharProps::collect(&refs);
            let face_ids: Vec<String> = resolution
                .faces
                .faces
                .iter()
                .map(|f| f.id.clone())
                .collect();
            let name_parts = resolution.name_parts;
            // Before the expansion is consumed below: the editor draws a
            // search-scoped block as its first match, and this is the only
            // place that holds the searches at all.
            let exists_matches =
                crate::exists::FirstMatches::collect(&refs, &resolution.expansion.exists);
            // Beside the recomposition, which shares none of its inputs: one
            // reads the expansion, the other the documents.
            // The searches and the aliases are kept back from the
            // recomposition, which needs neither, so the specimen can read the
            // ones this rebuild already derived instead of deriving them again.
            let crate::render::ttf_builder::Expansion {
                items,
                aliases,
                exists,
                ..
            } = resolution.expansion;
            let mut gc = grid_cache.lock().unwrap();
            let (specimen, (named_glyphs, alt_index, recompose_took)) =
                std::thread::scope(|scope| {
                    let collect = scope.spawn(|| {
                        let gid_map = gid_map.as_ref()?;
                        let t = std::time::Instant::now();
                        let data = crate::specimen::SpecimenData::collect(
                            &refs,
                            &name_parts,
                            &exists,
                            &aliases,
                            gid_map,
                            face_id,
                            &glyph_flags,
                        );
                        Some((data, t.elapsed()))
                    });
                    let t = std::time::Instant::now();
                    let (named_glyphs, alt_index) =
                        crate::editor::ref_composite::resolve_expanded_items(
                            items,
                            &aliases,
                            &name_parts,
                            &cancel,
                            Some(&mut gc),
                        );
                    // Read before the join, or the wait for the other leg of
                    // this scope is charged to this one.
                    let recompose_took = t.elapsed();
                    (
                        collect.join().unwrap(),
                        (named_glyphs, alt_index, recompose_took),
                    )
                });
            timing.recompose = recompose_took;
            let (specimen, specimen_took) = match specimen {
                Some((data, took)) => (Some(data), took),
                None => (None, std::time::Duration::ZERO),
            };
            timing.specimen = specimen_took;
            timing.total = t0.elapsed();
            if perf_log_enabled() {
                let (hits, misses) = gc.stats();
                eprintln!(
                    "[perf] rebuild (background): {:?} (expand {:?}, font {:?}, validate {:?}, \
                     recompose {:?}; {misses} composite(s) recomposed, {hits} reused)",
                    timing.total, timing.expand, timing.font, timing.validate, timing.recompose,
                );
            }
            drop(gc);
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
                        glyph: None,
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
                exists_matches,
                char_props,
                issues,
                glyph_flags,
                face_ids,
                specimen,
                timing,
            })));
        });
    }

    /// Hands the first build of a freshly loaded directory to the background
    /// pipeline, with no font of its own to show in the meantime.
    ///
    /// Both callers — startup and Open Folder — used to build the font on the
    /// UI thread, which on a network share is a ten-second freeze with nothing
    /// drawn. The request is armed for *now* rather than debounced: the debounce
    /// exists to absorb a burst of keystrokes, and a directory that has just
    /// been read has nothing to absorb.
    ///
    /// `last_font_gen` is taken here so that the first pump sees no generation
    /// change and therefore does not re-arm the request 300 ms out, on top of
    /// the one this just made due.
    pub(super) fn arm_initial_font_build(&mut self) {
        self.font_data = None;
        self.font_name_to_gid.clear();
        self.font_data_gen = self.font_build_gen;
        self.font_applied = None;
        self.last_font_gen = self.current_font_gen();
        self.rebuild_at = Some(std::time::Instant::now());
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
        self.rebuild_at = None;
        self.rebuild(ctx);
    }

    pub(super) fn apply_font(&mut self, ctx: &egui::Context) {
        let want_custom = !self.escape_mode && self.font_data.is_some();
        if self.font_applied == Some(want_custom) {
            return;
        }
        let started = std::time::Instant::now();

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
        // The atlas is filled lazily, so what this costs is only the copy and
        // the parse; the refill lands in `RebuildTiming::slowest_frame`.
        self.rebuild_log
            .ui_stage(|e| &mut e.apply_font, started.elapsed());
    }

    /// Schedules debounced font/derived-data rebuilds and drains the three
    /// background channels (font build, derived data, shape assertions).
    pub(super) fn pump_background_pipeline(&mut self, ctx: &egui::Context) {
        let any_pixel_painting = self
            .open_documents
            .iter()
            .any(|d| d.editor_state.suppress_font_rebuild);
        let font_gen = self.current_font_gen();
        // Drained before anything is scheduled, so a rebuild that just ended
        // frees its slot for a request armed in this very frame rather than in
        // the next one. The *font* message is not what frees it: it is sent as
        // soon as the bytes exist, with the rest of the rebuild still running
        // behind it, and starting a second rebuild then is exactly the pile-up
        // `rebuild_inflight` exists to prevent. The derived message is the one
        // that means the thread is done, however it ended.
        let build_result = take_current_font_build(&self.font_build_rx, self.font_build_gen);
        if let Some(result) = build_result {
            finish(&mut self.bg_tasks.build);
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
            self.rebuild_log.font_applied(self.font_build_gen);
        }

        if font_gen != self.last_font_gen && !any_pixel_painting {
            self.last_font_gen = font_gen;
            // The clock the end-to-end numbers are measured from; see
            // [`super::timing`].
            self.rebuild_log.requested();
            self.font_build_gen = self.font_build_gen.wrapping_add(1);
            // Whatever is building is building a document set that no longer
            // exists. Told now rather than when the debounce expires, it has
            // the whole debounce period to notice and get out of the way.
            self.rebuild_cancel.cancel();
            if self.rebuild_inflight {
                self.rebuild_log.cancelled_current();
            }
            let had_text_input =
                ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_))));
            let debounce_ms = if had_text_input { 1000 } else { 300 };
            self.rebuild_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(debounce_ms));
            ctx.request_repaint_after(std::time::Duration::from_millis(debounce_ms));
        }
        if let Some(at) = self.rebuild_at
            && std::time::Instant::now() >= at
        {
            // Cleared first: `rebuild` re-arms it itself when it has to
            // wait for the build it just cancelled.
            self.rebuild_at = None;
            self.rebuild(ctx);
        }

        if let Some(result) = take_latest_derived_data(&self.derived_data_rx) {
            self.rebuild_inflight = false;
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
                    self.rebuild_log
                        .derived_applied(data.build_gen, data.timing);
                    self.named_glyphs = std::sync::Arc::new(data.named_glyphs);
                    self.alt_index = data.alt_index;
                    self.name_parts = data.name_parts;
                    self.exists_matches = data.exists_matches;
                    self.char_props = data.char_props;
                    self.font_meta = data.meta;
                    self.named_glyphs_gen = data.build_gen;
                    self.derived_gen = self.derived_gen.wrapping_add(1);
                    self.issues = data.issues;
                    self.issues_gen = data.build_gen;
                    self.glyph_flags = data.glyph_flags;
                    // The list an edit to a `face` line changes; the startup
                    // one was collected from the same directives before the
                    // first build, so a remembered face is already selected by
                    // the time this arrives and no rebuild follows it.
                    self.face_ids = data.face_ids;
                    // Keyed on the generations of the *results* it was
                    // collected beside, which is what `SpecimenState` compares
                    // against; see `crate::specimen::SpecimenState::cached_gen`.
                    if let Some(specimen) = data.specimen {
                        self.specimen
                            .apply(specimen, self.font_data_gen, self.derived_gen);
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

        // Opening the specimen asks for a rebuild of its own: what it reads out
        // of the documents is collected in the background, and only by a
        // rebuild that knew its tab was open. Recorded so the ask is one rather
        // than one per frame, and so a collection that somehow produces nothing
        // cannot loop.
        //
        // *After* the derived result above, not before it. Both generations
        // this compares against are stepped by results the pump applies, and
        // the derived one is applied here — asking any earlier means asking
        // against a generation that is about to change in this very frame,
        // which is a second rebuild of what the first one already delivered.
        let want_specimen = self.bottom_panel_tab == Some(super::panels::SPECIMEN_TAB);
        let gens = (self.font_data_gen, self.derived_gen);
        if want_specimen
            && self.specimen.needs_rebuild(gens.0, gens.1)
            && self.specimen_asked_for != Some(gens)
            && !self.rebuild_inflight
            && self.rebuild_at.is_none()
        {
            self.specimen_asked_for = Some(gens);
            // Its own request, so its own clock: what the report measures is
            // the wait after whatever asked for the rebuild.
            self.rebuild_log.requested();
            self.rebuild_at = Some(std::time::Instant::now());
            ctx.request_repaint();
        }

        if let Ok(assert_issues) = self.assert_rx.try_recv() {
            let count = assert_issues.len();
            self.assert_issues = assert_issues;
            self.assert_running = false;
            finish(&mut self.bg_tasks.test);
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

        if let Ok(plan) = self.fix_rx.try_recv() {
            self.fix_running = false;
            finish(&mut self.bg_tasks.optimize);
            self.apply_clearance_plan(plan);
        }

        self.bg_tasks.gc();
        if self.bg_tasks.build.is_some()
            || self.bg_tasks.test.is_some()
            || self.bg_tasks.optimize.is_some()
        {
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

        let result = take_current_font_build(&rx, 2);
        let inner = result.expect("the current generation's build is applied");
        let inner = inner.expect("that build produced a font");
        assert_eq!(inner.bitmap, vec![2]);
        assert_eq!(inner.vector, vec![20]);
    }

    /// A cancelled build is never applied — not even when it happens to carry
    /// the current generation, which is the case a `Done(None)` of the same
    /// generation would blank the displayed font for.
    #[test]
    fn a_cancelled_build_never_replaces_the_font() {
        let (tx, rx) = mpsc::channel::<FontBuildMessage>();
        tx.send((4, FontBuildOutcome::Cancelled)).unwrap();

        assert!(
            take_current_font_build(&rx, 4).is_none(),
            "nothing to apply from a cancelled build"
        );
    }

    #[test]
    fn current_failed_build_clears_previous_font() {
        let (tx, rx) = mpsc::channel();
        tx.send((3, FontBuildOutcome::Done(None))).unwrap();

        let result = take_current_font_build(&rx, 3);
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

#[cfg(test)]
mod startup_tests {
    use super::*;

    /// Its own directory per test, removed when the test ends. Written inline:
    /// `font/` is downstream data and no test may read it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "uniform-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Startup reads the directory and stops there: the font build that used to
    /// run on the UI thread before eframe ever painted (10.4 s of an 18.6 s
    /// cold start over SMB — see `startup.rs`) belongs to the background
    /// pipeline, which the first frame starts with no debounce to wait out.
    ///
    /// A font that is not built yet is a state the editor already has to
    /// handle — an empty directory and a failed build both produce it — so what
    /// this asserts is only where the work runs, not that anything is missing.
    #[test]
    fn startup_hands_the_first_font_build_to_the_background() {
        let dir = TempDir::new("startup");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\nglyph a 2 2\n@@\n.@\n\nmap A = a\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));

        assert_eq!(app.font_base_docs.len(), 1, "the directory is read");
        assert!(
            app.font_data.is_none(),
            "no font is built on the way to the first frame"
        );
        assert!(!app.rebuild_inflight, "and none is running yet either");

        app.pump_background_pipeline(&ctx);
        assert!(
            app.rebuild_inflight,
            "the first frame starts the build straight away, without the edit debounce"
        );
    }

    /// A remembered face must not cost a second font build.
    ///
    /// It used to: the first build ran before anything knew which faces the
    /// directory declares, so the choice was applied only when the first
    /// resolve reported them — and applying it bumped the generation and built
    /// the whole font again. Over SMB that was a ten-second build followed by a
    /// five-second one before any text appeared. Which faces exist is a scan of
    /// the `face` directives, not a resolve, so it is known before the first
    /// build is armed.
    #[test]
    fn a_remembered_face_does_not_cost_a_second_font_build() {
        let dir = TempDir::new("face");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\n\
             slice narrow\nface regular\nface term : narrow\n\n\
             glyph a 2 2\n@@\n.@\n\nmap A = a\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        settings.remember_face(&dir.0, "term");
        let mut app = UniformApp::with_settings(&ctx, settings, Some(dir.0.clone()));

        assert_eq!(
            app.selected_face(),
            "term",
            "the remembered face is known before the first build, not after the first resolve"
        );

        app.pump_background_pipeline(&ctx);
        let build_gen = app.font_build_gen;
        assert!(app.rebuild_inflight);

        // Drive the pipeline until the first resolve has been applied: that is
        // the moment the face used to be switched under it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.named_glyphs_gen != app.font_build_gen {
            assert!(
                std::time::Instant::now() < deadline,
                "the pipeline never delivered a resolve"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.pump_background_pipeline(&ctx);
        }

        assert_eq!(
            app.font_build_gen, build_gen,
            "the resolve landing must not start another build"
        );
        assert_eq!(app.selected_face(), "term");
    }

    /// Every stage of a rebuild is measured, and the report says which ones
    /// were not.
    ///
    /// The point of the report is that a machine can be slow in the UI half
    /// rather than the background half, so a test that only proved the
    /// background numbers exist would miss what it is for; this drives a real
    /// rebuild and asserts the report names both halves.
    #[test]
    fn a_rebuild_reports_what_each_of_its_stages_cost() {
        let dir = TempDir::new("timing");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\nglyph a 2 2\n@@\n.@\n\nmap A = a\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        assert!(
            app.rebuild_log.report().contains("No rebuild yet"),
            "nothing has been rebuilt yet"
        );

        app.pump_background_pipeline(&ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.named_glyphs_gen != app.font_build_gen {
            assert!(
                std::time::Instant::now() < deadline,
                "the pipeline never delivered a rebuild"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.pump_background_pipeline(&ctx);
        }

        let report = app.rebuild_log.report();
        for stage in [
            "expand",
            "font build",
            "validate",
            "recompose",
            "background total",
            "apply font",
            "specimen",
            "edit to font on screen",
        ] {
            assert!(report.contains(stage), "{stage} is missing from\n{report}");
        }
        // The first build follows no edit, so the end-to-end numbers have
        // nothing to measure from and say so rather than reading as zero.
        let line = report
            .lines()
            .find(|l| l.contains("edit to font on screen"))
            .unwrap();
        assert!(
            line.trim_end().ends_with('-'),
            "an unmeasured stage reads as a dash, not a zero: {line:?}"
        );

        // An edit, and the numbers that were dashes above are measured: this is
        // the wait the report exists to attribute.
        let gen_before = app.font_build_gen;
        app.font_base_docs.push(
            crate::document_io::parse_document_from_str(
                "glyph b 2 2\n@@\n@.\n",
                dir.0.join("b.unf"),
            )
            .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.font_build_gen == gen_before || app.named_glyphs_gen != app.font_build_gen {
            assert!(
                std::time::Instant::now() < deadline,
                "the edit never produced a rebuild"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.pump_background_pipeline(&ctx);
        }
        let report = app.rebuild_log.report();
        let line = report
            .lines()
            .find(|l| l.contains("edit to derived applied"))
            .unwrap();
        assert!(
            line.trim_end().ends_with("ms"),
            "an edit gives the end-to-end numbers something to measure: {line:?}"
        );
    }

    /// What the specimen reads out of the documents is collected in the
    /// background, and only when its tab is open.
    ///
    /// It used to be collected on the UI thread, the first time the tab was
    /// drawn — a third full expansion of the document set, which on a slow
    /// machine stopped the editor for over a second with nothing on screen to
    /// say why. Both halves are asserted here: that opening the tab gets the
    /// data without the UI doing the work, and that a closed tab costs nothing.
    #[test]
    fn the_specimen_is_collected_in_the_background_and_only_when_shown() {
        let dir = TempDir::new("specimen-bg");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\nglyph a 2 2\n@@\n.@\n\nmap A = a\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let settle = |app: &mut UniformApp| {
            while app.named_glyphs_gen != app.font_build_gen
                || app.rebuild_inflight
                || app.rebuild_at.is_some()
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the pipeline never settled"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
                app.pump_background_pipeline(&ctx);
            }
        };

        app.pump_background_pipeline(&ctx);
        settle(&mut app);
        assert!(
            app.specimen
                .needs_rebuild(app.font_data_gen, app.derived_gen),
            "a closed tab collects nothing"
        );

        // Opening it asks for a rebuild of its own, which brings the data.
        app.bottom_panel_tab = Some(super::super::panels::SPECIMEN_TAB);
        app.pump_background_pipeline(&ctx);
        settle(&mut app);
        assert!(
            !app.specimen
                .needs_rebuild(app.font_data_gen, app.derived_gen),
            "the rebuild that knew the tab was open collected it"
        );
    }

    /// Opening a file the directory snapshot already holds must not rebuild the
    /// font.
    ///
    /// It used to: opening bumped the document's `content_gen` past the
    /// snapshot's, and `current_font_gen` hashes that — so every Ctrl/Cmd+click
    /// into a file cost a full build of a font whose bytes could not have
    /// changed, since the opened text is the snapshot's own bytes re-parsed.
    #[test]
    fn opening_a_file_from_the_snapshot_does_not_rebuild_the_font() {
        let dir = TempDir::new("open");
        let path = dir.0.join("a.unf");
        std::fs::write(
            &path,
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\nglyph a 2 2\n@@\n.@\n\nmap A = a\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        let before = app.current_font_gen();

        app.open_file(path);
        assert_eq!(app.open_documents.len(), 1, "the file is open");
        assert_eq!(
            app.current_font_gen(),
            before,
            "the same bytes, so the same font: nothing to rebuild"
        );

        // A file the snapshot does not have is a real change to the document
        // set, and does move the generation.
        let fresh = dir.0.join("b.unf");
        std::fs::write(&fresh, "glyph b 2 2\n@@\n@.\n").unwrap();
        app.open_file(fresh);
        assert_eq!(app.open_documents.len(), 2);
        assert_ne!(
            app.current_font_gen(),
            before,
            "a document the font did not have is a rebuild"
        );
    }
}
