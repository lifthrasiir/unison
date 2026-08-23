//! What one rebuild cost, stage by stage, and on which thread.
//!
//! The background pipeline's own stages are measurable without a window —
//! `uniform probe -i DIR --edit` does exactly that — but the wait a user sits
//! through is longer than what that reports. The font bytes still have to reach
//! egui's atlas and the specimen still has to be laid out again, and both of
//! those happen on the *UI* thread, in a later frame, on the far side of the
//! channel. A machine can be slow in either half.
//!
//! Which is the point: the two halves are not comparable between platforms, and
//! a change that halves the background work shows up as nothing at all if the
//! machine in question was spending its time in the other half. So this
//! measures the whole path — the debounce, the background stages, the UI
//! stages, and the two end-to-end numbers that matter (an edit to the font on
//! screen, an edit to the rest of the derived data) — rather than adding
//! another `[perf]` line to the half that was already visible.
//!
//! Read it in *View → Rebuild timing…*, which is the only route when the binary
//! was started from a shortcut with no console, or off stderr with
//! `UNIFORM_PERF` set.
//!
//! Every field is an `Option`, and a blank one means *not measured* rather than
//! zero: the specimen is only laid out when its tab is open, and a rebuild that
//! was cancelled never reaches most of these at all.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many rebuilds the log keeps. Enough to see whether a number is the
/// steady state or a one-off, and few enough to read at a glance.
const KEPT: usize = 6;

/// How many frames the UI-thread record keeps: a few seconds of a repainting
/// editor, which is long enough for a stall to still be in it when the window
/// is opened to look for one.
const FRAMES: usize = 240;

/// The background thread's own stages, measured where they run and carried back
/// with the derived data — the font build's duration included, even though its
/// *result* travels on the other channel, because only this thread can time it.
#[derive(Clone, Copy, Default)]
pub(super) struct BackgroundTiming {
    pub(super) expand: Duration,
    pub(super) font: Duration,
    pub(super) validate: Duration,
    pub(super) flags: Duration,
    pub(super) recompose: Duration,
    /// Collecting what the specimen reads out of the documents, which runs
    /// beside the recomposition and only when its tab is open — zero otherwise.
    pub(super) specimen: Duration,
    /// The thread's own wall time, which is less than the sum above: the font
    /// build and validation run at once.
    pub(super) total: Duration,
}

/// One rebuild, filled in as its pieces arrive.
#[derive(Clone, Default)]
pub(super) struct RebuildTiming {
    pub(super) build_gen: u64,
    /// Edit to the rebuild starting: the debounce, plus whatever a rebuild it
    /// superseded had left to wind down.
    pub(super) waited: Option<Duration>,
    pub(super) background: Option<BackgroundTiming>,
    /// Installing the new font in egui. The atlas itself is filled lazily, so
    /// what that costs lands in `slowest_frame` rather than here.
    pub(super) apply_font: Option<Duration>,
    /// Laying the specimen out again for what the rebuild collected — steps 2
    /// and 3, which are the UI thread's and run in the frame after the data
    /// lands. `None` when its tab is closed.
    pub(super) specimen_layout: Option<Duration>,
    /// The slowest `update()` since the font landed — where a font atlas being
    /// refilled shows up, since egui does that inside the first layout that
    /// needs it rather than in `set_fonts`.
    pub(super) slowest_frame: Option<Duration>,
    /// Edit to the font being on screen.
    pub(super) to_font: Option<Duration>,
    /// Edit to the derived data — names, issues, glyph flags — being applied.
    pub(super) to_derived: Option<Duration>,
    pub(super) cancelled: bool,
}

/// The last few rebuilds, and the marks the one in progress is measured from.
#[derive(Default)]
pub(super) struct RebuildLog {
    entries: VecDeque<RebuildTiming>,
    /// When whatever asked for the current rebuild happened. Reset by each
    /// further ask, so what is measured is the wait after the *last* one —
    /// which is the wait the user is looking at. Not reset by a rebuild
    /// re-arming itself after cancelling one: that is the same ask, still
    /// waiting.
    edit_at: Option<Instant>,
    started_at: Option<Instant>,
    /// Set while a font result is fresh, so the frames right after it are
    /// watched for the atlas refill.
    watch_frames_until: Option<Instant>,
    /// The last [`FRAMES`] frames, newest first. A stall on the UI thread is
    /// felt exactly like a slow rebuild and is invisible to every other number
    /// here — the file watch, a repaint over a slow surface and the preview's
    /// shaping all live there — so it is worth its own line even though it
    /// belongs to no rebuild.
    frames: VecDeque<Duration>,
}

impl RebuildLog {
    /// Something asked for a rebuild — an edit, or a panel wanting data no
    /// rebuild has collected yet. The clock the end-to-end numbers are measured
    /// from.
    pub(super) fn requested(&mut self) {
        self.edit_at = Some(Instant::now());
    }

    /// A rebuild thread was spawned for `build_gen`.
    pub(super) fn started(&mut self, build_gen: u64) {
        let now = Instant::now();
        self.started_at = Some(now);
        let waited = self.edit_at.map(|at| now.duration_since(at));
        self.entries.push_front(RebuildTiming {
            build_gen,
            waited,
            ..Default::default()
        });
        self.entries.truncate(KEPT);
    }

    /// The entry a result belongs to, or `None` when it is for a rebuild the
    /// log no longer keeps.
    fn entry(&mut self, build_gen: u64) -> Option<&mut RebuildTiming> {
        self.entries.iter_mut().find(|e| e.build_gen == build_gen)
    }

    fn since_edit(&self) -> Option<Duration> {
        self.edit_at.map(|at| at.elapsed())
    }

    pub(super) fn font_applied(&mut self, build_gen: u64) {
        let elapsed = self.since_edit();
        self.watch_frames_until = Some(Instant::now() + Duration::from_millis(500));
        if let Some(entry) = self.entry(build_gen) {
            entry.to_font = elapsed;
        }
    }

    pub(super) fn derived_applied(&mut self, build_gen: u64, background: BackgroundTiming) {
        let elapsed = self.since_edit();
        if let Some(entry) = self.entry(build_gen) {
            entry.background = Some(background);
            entry.to_derived = elapsed;
        }
    }

    /// The rebuild in flight was superseded. Marked on the newest entry rather
    /// than by generation, because that entry *is* the one in flight: a rebuild
    /// only starts when the previous one has ended.
    pub(super) fn cancelled_current(&mut self) {
        if let Some(entry) = self.entries.front_mut() {
            entry.cancelled = true;
        }
    }

    /// A UI-thread stage of the newest rebuild. Recorded against the newest
    /// entry rather than a generation, because neither stage knows which
    /// rebuild produced the data it is working from.
    pub(super) fn ui_stage(
        &mut self,
        pick: impl Fn(&mut RebuildTiming) -> &mut Option<Duration>,
        took: Duration,
    ) {
        if let Some(entry) = self.entries.front_mut() {
            *pick(entry) = Some(took);
        }
    }

    /// One frame's `update()`. Always kept in the frame record, and attributed
    /// to the newest rebuild while a font it installed is still fresh.
    pub(super) fn frame(&mut self, took: Duration) {
        self.frames.push_front(took);
        self.frames.truncate(FRAMES);

        let Some(until) = self.watch_frames_until else {
            return;
        };
        if Instant::now() > until {
            self.watch_frames_until = None;
            return;
        }
        if let Some(entry) = self.entries.front_mut()
            && entry.slowest_frame.is_none_or(|prev| took > prev)
        {
            entry.slowest_frame = Some(took);
        }
    }

    /// The human-readable report. Both the window and the stderr dump print
    /// exactly this.
    pub(super) fn report(&self) -> String {
        fn ms(d: Option<Duration>) -> String {
            match d {
                Some(d) => format!("{:.1} ms", d.as_secs_f64() * 1000.0),
                None => "-".to_string(),
            }
        }
        let mut out = String::new();
        out.push_str("Rebuild timing\n");
        out.push_str("==============\n\n");
        if cfg!(debug_assertions) {
            out.push_str(
                "(debug build \u{2014} compute stages are 10-30x slower than a release build)\n\n",
            );
        }
        out.push_str(
            "One edit, from the debounce to the font being on screen. The font build\n\
             and validation run at once on the background thread, so its total is less\n\
             than their sum; `apply font` and `specimen` are on the UI thread, in a\n\
             later frame. A dash is a stage this rebuild never reached.\n\n",
        );

        // First, because it belongs to no rebuild and answers a question the
        // rest cannot: whether the editor is slow between edits too.
        if !self.frames.is_empty() {
            let mut sorted: Vec<Duration> = self.frames.iter().copied().collect();
            sorted.sort_unstable();
            let slow = sorted
                .iter()
                .filter(|d| **d >= Duration::from_millis(16))
                .count();
            out.push_str(&format!("UI frames (last {})\n", sorted.len()));
            out.push_str(&format!(
                "  {:<32}{:>10}\n",
                "slowest",
                ms(sorted.last().copied())
            ));
            out.push_str(&format!(
                "  {:<32}{:>10}\n",
                "median",
                ms(sorted.get(sorted.len() / 2).copied())
            ));
            out.push_str(&format!("  {:<32}{slow:>10}\n\n", "over 16 ms"));
        }

        if self.entries.is_empty() {
            out.push_str("No rebuild yet \u{2014} edit something.\n");
        }
        for entry in &self.entries {
            out.push_str(&format!(
                "generation {}{}\n",
                entry.build_gen,
                if entry.cancelled { "  (cancelled)" } else { "" }
            ));
            let bg = entry.background;
            let row = |label: &str, d: Option<Duration>| format!("  {label:<32}{:>10}\n", ms(d));
            out.push_str(&row("waited (debounce)", entry.waited));
            out.push_str("  -- background thread --\n");
            out.push_str(&row("expand", bg.map(|b| b.expand)));
            out.push_str(&row("font build", bg.map(|b| b.font)));
            out.push_str(&row("validate", bg.map(|b| b.validate)));
            out.push_str(&row("glyph flags", bg.map(|b| b.flags)));
            out.push_str(&row("recompose", bg.map(|b| b.recompose)));
            out.push_str(&row("specimen data", bg.map(|b| b.specimen)));
            out.push_str(&row("background total", bg.map(|b| b.total)));
            out.push_str("  -- UI thread --\n");
            out.push_str(&row("apply font", entry.apply_font));
            out.push_str(&row("specimen layout", entry.specimen_layout));
            out.push_str(&row("slowest frame after", entry.slowest_frame));
            out.push_str("  -- end to end --\n");
            out.push_str(&row("edit to font on screen", entry.to_font));
            out.push_str(&row("edit to derived applied", entry.to_derived));
            out.push('\n');
        }
        out
    }
}
