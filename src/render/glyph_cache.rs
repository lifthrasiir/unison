//! Shared machinery for resolving glyph composites into a name-keyed cache.
//!
//! `ttf_builder` (contours for the font) and `sample` (colored components for
//! the specimen) each need a cache of every glyph resolved against its refs.
//! Their cache *values* differ, but the resolution rules must not: which
//! glyphs seed the cache, when a pending composite becomes ready, how a ref
//! name falls back to pattern expansion, and how `:`-suffixed alternatives
//! are indexed.  Those rules live here, generic over the cache value, so the
//! two consumers cannot drift apart; only the per-value composite
//! construction stays with each caller.

use crate::hash::HashMap;

use crate::document::{DocumentItem, GlyphName, GlyphPoint, GlyphRef, PixelGrid};

/// How many glyphs pass between two cancellation checks. A check is a relaxed
/// atomic load, so this is not about the cost of checking but about not calling
/// it per *trivial* item: over ~18k glyphs a stride of 64 bounds the work an
/// aborted build still does at well under a frame, while leaving the hot loops
/// looking the way they did.
pub(crate) const CANCEL_STRIDE: usize = 64;

/// A glyph waiting for all of its refs to resolve.
pub(crate) struct PendingGlyph {
    pub name: String,
    pub pixels: Option<PixelGrid>,
    pub refs: Vec<GlyphRef>,
    pub points: Vec<GlyphPoint>,
    pub scale: u8,
    /// The glyph's declared box origin, in logical cells. Both ends of every
    /// placement need one: this glyph's, and each ref target's (through
    /// [`CachedGlyphEntry::declared_origin`]).
    pub declared_origin: (i16, i16),
    /// `desync`: `pixels` is bitmap ink and nothing else. What that means for
    /// a cache value is the *consumer's* decision — this driver only carries
    /// the flag — but every consumer that draws an outline has to make it, or
    /// the grid reappears in a face that must not have it.
    pub desync: bool,
}

/// A cache value the shared resolution driver can operate on.
pub(crate) trait CachedGlyphEntry {
    fn anchors(&self) -> &[GlyphPoint];
    /// Where this glyph's declared box starts inside its own grid, in logical
    /// cells — see [`crate::document::GlyphBody::declared_origin`].
    fn declared_origin(&self) -> (i16, i16);
    fn dims_mut(&mut self) -> (&mut u16, &mut u16);
    fn set_resolution(&mut self, anchors: Vec<GlyphPoint>, scale: u8, origin: (i16, i16));
}

/// Looks up `name` in the cache, falling back to the first expansion of a
/// pattern name (`digit(0|1)` resolves via `digit0` when the pattern string
/// itself is not a cache key).
pub(crate) fn resolve_cached<'a, V>(name: &str, cache: &'a HashMap<String, V>) -> Option<&'a V> {
    resolve_cached_named(name, cache).map(|(_, cached)| cached)
}

/// [`resolve_cached`], with the key the entry was found under: the name of the
/// glyph a `ref` to `name` actually reaches, which for a pattern is not `name`.
pub(crate) fn resolve_cached_named<'a, V>(
    name: &str,
    cache: &'a HashMap<String, V>,
) -> Option<(&'a str, &'a V)> {
    if let Some((key, cached)) = cache.get_key_value(name) {
        return Some((key, cached));
    }
    let expanded = crate::ref_composite::parse_ref_pattern(name)?;
    cache
        .get_key_value(&expanded.get(0))
        .map(|(key, cached)| (key.as_str(), cached))
}

/// Trim the blank margin a composite's raster grid has *before* its origin,
/// moving `origin_row`/`origin_col` back towards zero by as much as is
/// dropped.
///
/// A negative `ref` offset is a bearing only where something is actually
/// drawn.  Pulling a ref up into its own empty top rows (`ref X 0 -3` when
/// `X`'s first rows are blank) is the ordinary way to nudge a composite, and
/// it has to stay metrically identical to the same ink placed directly —
/// otherwise every such glyph would grow a phantom bearing that the sample
/// then pads its cell for.  Only the margin left of / above the origin is
/// trimmed, so the grid still starts exactly at `origin_*`.
pub(crate) fn trim_blank_before_origin(
    grid: &mut PixelGrid,
    origin_row: &mut i32,
    origin_col: &mut i32,
) {
    let blank_rows = (0..grid.height)
        .take_while(|&r| (0..grid.width).all(|c| grid.get(r, c).is_clear()))
        .count() as i32;
    let blank_cols = (0..grid.width)
        .take_while(|&c| (0..grid.height).all(|r| grid.get(r, c).is_clear()))
        .count() as i32;
    let trim_r = blank_rows.min(-*origin_row).max(0) as u16;
    let trim_c = blank_cols.min(-*origin_col).max(0) as u16;
    if trim_r == 0 && trim_c == 0 {
        return;
    }

    let (new_w, new_h) = (grid.width - trim_c, grid.height - trim_r);
    let mut trimmed = PixelGrid::new(new_w, new_h);
    trimmed.den = grid.den;
    for r in 0..new_h {
        for c in 0..new_w {
            trimmed.pixels[r as usize * new_w as usize + c as usize] =
                grid.get(r + trim_r, c + trim_c);
        }
    }
    trimmed.details = grid
        .details
        .iter()
        .filter(|&(&(r, c), _)| r >= trim_r && c >= trim_c)
        .map(|(&(r, c), d)| ((r - trim_r, c - trim_c), d.clone()))
        .collect();

    *grid = trimmed;
    *origin_row += trim_r as i32;
    *origin_col += trim_c as i32;
}

/// Index of `:`-suffixed alternatives: `foo:bar:baz` registers under `foo`
/// and `foo:bar`, carrying each alternative's resolved anchors.
pub(crate) fn build_alt_index<V: CachedGlyphEntry>(
    cache: &HashMap<String, V>,
) -> HashMap<String, Vec<(String, Vec<GlyphPoint>)>> {
    let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::default();
    for (name, cached) in cache {
        for prefix in crate::ref_composite::alternative_prefixes(name) {
            map.entry(prefix.to_string())
                .or_default()
                .push((name.clone(), cached.anchors().to_vec()));
        }
    }
    for alts in map.values_mut() {
        alts.sort_by(|(a, _), (b, _)| a.cmp(b));
    }
    map
}

/// Seeds the cache from expanded document items: pixel-only glyphs enter
/// directly via `from_grid` (which is told the glyph's name and whether it is
/// `desync`),
/// glyphs with refs (or pixels alongside refs) become pending, and bodiless
/// `keep` placeholders enter as `empty` entries that only carry anchors.
///
/// `from_grid` is where the font build traces contours, so `cancel` is checked
/// every [`CANCEL_STRIDE`] items; a cancelled seeding returns whatever it had
/// built so far, which the caller discards along with everything downstream.
pub(crate) fn seed_cache<'a, V: CachedGlyphEntry>(
    all_items: impl IntoIterator<Item = &'a DocumentItem>,
    mut from_grid: impl FnMut(&str, &PixelGrid, bool) -> V,
    mut empty: impl FnMut() -> V,
    cancel: &crate::cancel::CancelToken,
) -> (HashMap<String, V>, Vec<PendingGlyph>) {
    let mut cache: HashMap<String, V> = HashMap::default();
    let mut pending: Vec<PendingGlyph> = Vec::new();

    for (i, item) in all_items.into_iter().enumerate() {
        if i.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
            break;
        }
        let (cache_key, body) = match item {
            DocumentItem::Glyph {
                name: GlyphName(n),
                body,
            } => (n.clone(), body),
            _ => continue,
        };
        if !cache_key.is_empty() && !cache.contains_key(&cache_key) {
            if let Some(ref pixels) = body.pixels
                && body.refs.is_empty()
            {
                let mut cached = from_grid(&cache_key, pixels, body.desync);
                cached.set_resolution(body.points.clone(), body.scale, body.declared_origin());
                cache.insert(cache_key, cached);
            } else if body.pixels.is_some() || !body.refs.is_empty() {
                pending.push(PendingGlyph {
                    name: cache_key,
                    pixels: body.pixels.clone(),
                    refs: body.refs.clone(),
                    points: body.points.clone(),
                    scale: body.scale,
                    declared_origin: body.declared_origin(),
                    desync: body.desync,
                });
            } else if body.keep {
                let mut cached = empty();
                cached.set_resolution(body.points.clone(), 1, body.declared_origin());
                cache.insert(cache_key, cached);
            }
        }
    }

    (cache, pending)
}

/// The composite tracer [`resolve_pending`] drives, split so its expensive half
/// can leave the calling thread.
///
/// `lookup` and `store` are the memo around the tracer — the traced-contour
/// cache the font build carries between rebuilds — and they own a `&mut` to it,
/// so they stay on the driving thread. [`key`](CompositeBuilder::key) and
/// [`build`](CompositeBuilder::build) are what is left once the memo is out of
/// the way: pure functions of the glyph, its derived refs and the cache, which
/// is why a whole wave of them can run at once.
///
/// A consumer with nothing to memoize (validation, the specimen) passes an
/// [`FnBuilder`] and never sees the split.
pub(crate) trait CompositeBuilder<V>: Sync {
    /// What [`store`](CompositeBuilder::store) files a freshly built value
    /// under — the memo key `key` already computed, so it is not derived
    /// twice. `()` for a builder that memoizes nothing.
    ///
    /// `Sync` because it rides in the wave the tracing threads read.
    type Key: Send + Sync;

    /// The memo key for this composite. A pure function of the glyph, its
    /// derived refs and the cache, like `build`, so a wave's keys are computed
    /// on every core: hashing every composite's ink was a fifth of a warm
    /// rebuild's serial resolve.
    fn key(&self, pg: &PendingGlyph, refs: &[GlyphRef], cache: &HashMap<String, V>) -> Self::Key;

    /// The memo's value under `key`, when it holds one. Serial.
    fn lookup(&mut self, key: &Self::Key) -> Option<V>;

    /// The tracer proper. Called only for a memo miss, on an arbitrary thread,
    /// possibly several at once.
    fn build(&self, pg: &PendingGlyph, refs: &[GlyphRef], cache: &HashMap<String, V>) -> V;

    /// Records what `build` produced. Serial.
    fn store(&mut self, key: Self::Key, value: &V);
}

/// A [`CompositeBuilder`] that is only a tracer: no memo, so every composite is
/// a miss and `store` has nowhere to put anything.
pub(crate) struct FnBuilder<F>(pub(crate) F);

impl<V, F> CompositeBuilder<V> for FnBuilder<F>
where
    F: Fn(&PendingGlyph, &[GlyphRef], &HashMap<String, V>) -> V + Sync,
{
    type Key = ();

    fn key(&self, _pg: &PendingGlyph, _refs: &[GlyphRef], _cache: &HashMap<String, V>) {}

    fn lookup(&mut self, _key: &()) -> Option<V> {
        None
    }

    fn build(&self, pg: &PendingGlyph, refs: &[GlyphRef], cache: &HashMap<String, V>) -> V {
        (self.0)(pg, refs, cache)
    }

    fn store(&mut self, _key: (), _value: &V) {}
}

/// One entry of a resolution wave: the pending glyph, the refs and anchors its
/// derive produced, and the memo lookup (`K` the key, `Option<V>` the hit).
type WaveEntry<K, V> = (PendingGlyph, Vec<GlyphRef>, Vec<GlyphPoint>, K, Option<V>);

/// Traces every memo miss in `wave`, in parallel, returning one slot per wave
/// entry (`None` where the memo already had it, or where a cancel stopped the
/// run short of it).
fn trace_wave<V, B>(
    wave: &[WaveEntry<B::Key, V>],
    builder: &B,
    cache: &HashMap<String, V>,
    cancel: &crate::cancel::CancelToken,
) -> Vec<Option<V>>
where
    V: CachedGlyphEntry + Send + Sync,
    B: CompositeBuilder<V>,
{
    let misses: Vec<usize> = wave
        .iter()
        .enumerate()
        .filter(|(_, e)| e.4.is_none())
        .map(|(i, _)| i)
        .collect();
    let mut out: Vec<Option<V>> = (0..wave.len()).map(|_| None).collect();
    let traced = crate::parallel::map_indexed(misses.len(), cancel, |at| {
        let (pg, refs, ..) = &wave[misses[at]];
        builder.build(pg, refs, cache)
    });
    for (at, value) in traced.into_iter().enumerate() {
        out[misses[at]] = value;
    }
    out
}

/// Drops the pending glyphs whose refs can never resolve, before the expensive
/// loop below ever walks them.
///
/// The fixpoint reaches the same set on its own — a glyph blocked on a name
/// nothing defines simply survives every round and is discarded at the end — but
/// it re-walks that glyph, and every glyph waiting on it, once per round while
/// doing so. A source that declares a family far larger than what is drawn (a
/// `glyph han-($#4e00..9fff)` block whose parts are still a work queue) makes
/// that the *majority* of the pending list: ~105k of 125k, walked five times
/// over, in a font where fewer than 20k composites are real.
///
/// This is the same closure over names alone, with no geometry and no anchors:
/// a name can enter the cache only if every ref of the glyph that carries it can,
/// so one forward sweep per level of nesting settles it. Glyphs the real loop
/// drops for a *derive* reason are not predicted here — anchors are exactly what
/// this pass does not look at — so it only ever removes glyphs the loop was
/// going to discard anyway.
fn drop_unresolvable<V>(cache: &HashMap<String, V>, pending: &mut Vec<PendingGlyph>) {
    // Mirrors `resolve_cached`: a ref hits its own name, or the first expansion
    // of it read as a pattern.
    fn ref_known(name: &str, known: &crate::hash::HashSet<String>) -> bool {
        known.contains(name)
            || crate::ref_composite::parse_ref_pattern(name)
                .is_some_and(|expanded| known.contains(&expanded.get(0)))
    }

    let mut known: crate::hash::HashSet<String> = cache.keys().cloned().collect();
    let mut unsettled: Vec<&PendingGlyph> = pending.iter().collect();
    loop {
        let before = unsettled.len();
        let mut still: Vec<&PendingGlyph> = Vec::with_capacity(before);
        let mut resolved: Vec<&str> = Vec::new();
        for pg in unsettled {
            if pg.refs.iter().all(|r| ref_known(&r.name, &known)) {
                resolved.push(&pg.name);
            } else {
                still.push(pg);
            }
        }
        for name in resolved {
            known.insert(name.to_string());
        }
        unsettled = still;
        if unsettled.len() == before {
            break;
        }
    }
    if unsettled.is_empty() {
        return;
    }
    let dead: crate::hash::HashSet<&str> = unsettled.iter().map(|pg| pg.name.as_str()).collect();
    // Owned up front: `dead` borrows `pending`, and the retain below needs it
    // to have let go.
    let dead: crate::hash::HashSet<String> = dead.into_iter().map(|s| s.to_string()).collect();
    pending.retain(|pg| !dead.contains(&pg.name));
}

/// What a glyph is to a pass that only needs to know *whether* it resolves,
/// and to what anchors: everything the derivation reads, and no raster.
///
/// Which glyphs resolve is decided by their refs and anchors alone — a
/// composite whose anchors derive to nothing is dropped, one whose refs never
/// resolve is never tried — so this answers it without composing anything.
pub(crate) struct AnchorsOnly {
    pub(crate) anchors: Vec<GlyphPoint>,
    declared_origin: (i16, i16),
    w: u16,
    h: u16,
}

impl AnchorsOnly {
    fn new() -> Self {
        Self {
            anchors: Vec::new(),
            declared_origin: (0, 0),
            w: 0,
            h: 0,
        }
    }
}

impl CachedGlyphEntry for AnchorsOnly {
    fn anchors(&self) -> &[GlyphPoint] {
        &self.anchors
    }
    fn declared_origin(&self) -> (i16, i16) {
        self.declared_origin
    }
    fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
        (&mut self.w, &mut self.h)
    }
    fn set_resolution(&mut self, anchors: Vec<GlyphPoint>, _scale: u8, origin: (i16, i16)) {
        self.anchors = anchors;
        self.declared_origin = origin;
    }
}

/// Every glyph of `items` that resolves, through the same driver and the same
/// derivation the font build runs, with the anchors it exposes; `on_issue`
/// hears each derivation that failed, whose glyph is left out as the build
/// leaves it out.
pub(crate) fn resolve_anchors_only<'a, I: Iterator<Item = &'a DocumentItem>>(
    items: impl Fn() -> I,
    on_issue: impl FnMut(&str, crate::ref_composite::DeriveIssue),
) -> HashMap<String, AnchorsOnly> {
    let never = crate::cancel::CancelToken::never();
    let mut declared_anchors: HashMap<&str, &[GlyphPoint]> = HashMap::default();
    for item in items() {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        {
            declared_anchors.entry(n).or_insert(&body.points);
        }
    }
    let (mut cache, pending) = seed_cache(
        items(),
        |_, _, _| AnchorsOnly::new(),
        AnchorsOnly::new,
        &never,
    );
    resolve_pending(
        &mut cache,
        pending,
        &crate::document::collect_anchor_aligns(items()),
        |name| declared_anchors.get(name).map(|pts| pts.to_vec()),
        &mut FnBuilder(|_: &_, _: &_, _: &_| AnchorsOnly::new()),
        on_issue,
        &never,
    );
    cache
}

/// The schedule both composite fixpoints run — this module's
/// [`resolve_pending`] for the build and `ref_composite`'s for the editor — so
/// the two cannot disagree about when a composite is derived, and therefore
/// about which alternative it gets. What each one *builds* per wave differs
/// (contours here, grids there); when is the part that has to be one rule.
///
/// Three things decide it:
/// - A composite waits while any of its refs resolves to nothing yet.
/// - It also waits while an offset-less ref still has an alternative pending:
///   that alternative would be missing from the index the derivation picks
///   from, so a substitution only *it* can satisfy (by anchor size) would
///   silently fall through. `i-upper` + `acute-above` did exactly that.
/// - That second guard is dropped for one round whenever it is all that holds
///   every remaining glyph back (a reference cycle through an alternative), so
///   resolution still ends, with the fallbacks it produced before.
///
/// A derivation that picked an alternative derived earlier in the *same* wave
/// defers too ([`Rounds::picked_unbuilt`]): that one is indexed but not yet
/// built, and a layer the builder cannot find is dropped without a word.
pub(crate) struct Rounds {
    /// How many alternatives of each base name are still pending.
    pending_alts: HashMap<String, usize>,
    relaxed: bool,
}

impl Rounds {
    /// The schedule over the composites named by `pending`.
    pub(crate) fn new<'a>(pending: impl Iterator<Item = &'a str>) -> Self {
        let mut pending_alts: HashMap<String, usize> = HashMap::default();
        for name in pending {
            for prefix in crate::ref_composite::alternative_prefixes(name) {
                *pending_alts.entry(prefix.to_string()).or_default() += 1;
            }
        }
        Self {
            pending_alts,
            relaxed: false,
        }
    }

    /// Whether a composite with these refs has to wait for a later round;
    /// `resolves` says whether a name is in the cache.
    pub(crate) fn must_wait(&self, refs: &[GlyphRef], resolves: impl Fn(&str) -> bool) -> bool {
        !refs.iter().all(|r| resolves(&r.name))
            || (!self.relaxed
                && refs
                    .iter()
                    .any(|r| r.offset.is_none() && self.pending_alts.contains_key(&r.name)))
    }

    /// Whether a derivation settled on a ref that is not built yet — an
    /// alternative derived earlier in this wave — so the composite waits.
    pub(crate) fn picked_unbuilt(
        effective_refs: &[GlyphRef],
        resolves: impl Fn(&str) -> bool,
    ) -> bool {
        effective_refs.iter().any(|r| !resolves(&r.name))
    }

    /// `name` is no longer pending, whatever became of it. The counts have to
    /// come down either way: leaving one standing would block every composite
    /// that refs the base until the first barren round relaxed the guard.
    pub(crate) fn settle(&mut self, name: &str) {
        for prefix in crate::ref_composite::alternative_prefixes(name) {
            if let Some(count) = self.pending_alts.get_mut(prefix) {
                *count -= 1;
                if *count == 0 {
                    self.pending_alts.remove(prefix);
                }
            }
        }
    }

    /// At the end of a round: whether another one is due.
    pub(crate) fn next_round(&mut self, pending_left: bool, progress: bool) -> bool {
        if !pending_left {
            return false;
        }
        if progress {
            self.relaxed = false;
        } else if self.relaxed {
            return false;
        } else {
            self.relaxed = true;
        }
        true
    }
}

/// Fixpoint loop resolving pending glyphs against the cache.  Each round
/// takes every pending glyph whose refs all resolve, derives its effective
/// ref offsets and anchors, builds the composite via `build`, applies the
/// shared fixups (declared dims win over the composite's raster extent;
/// resolved anchors and the declaring scale are recorded) and inserts it.
/// Glyphs whose refs never resolve are dropped, matching how missing refs
/// are reported elsewhere. So is a glyph whose derivation reported any
/// [`crate::ref_composite::DeriveIssue`]: an anchor that
/// derived to nothing leaves an outline that still looks plausible, and
/// keeping it meant the error had no effect an author could see — the font
/// mapped the character to that outline, and the specimen drew the cell.
/// Dropping it puts an anchor error on the same footing as a missing ref, and
/// takes its dependents with it for the same reason.
///
/// `build` is the composite tracer, which is most of a font build's cost, so
/// `cancel` is checked both per round and every [`CANCEL_STRIDE`] glyphs within
/// one. Returning early leaves the cache holding whatever resolved so far —
/// indistinguishable, to everything downstream, from a source whose remaining
/// composites never resolved. That is a state the pipeline already handles, and
/// the caller throws the result away regardless.
///
/// # Why a round is a *wave*
///
/// Each round derives every glyph the cache can already satisfy, traces them
/// all, and only then inserts them. Deriving and inserting stay serial — they
/// read and write `alt_index`, the [`Rounds`] and the cache itself — but tracing
/// is pure with respect to that bookkeeping: a glyph enters a wave only once
/// every ref of it is *already* in the cache, so nothing a wave produces can
/// change what another member of the same wave sees. That is what lets
/// [`CompositeBuilder::build`] run on every core at once, and tracing is where
/// nearly all of a font build's time goes.
///
/// The cost is a round per level of nesting rather than per level that a lucky
/// walk order failed to collapse: a glyph made ready by an insertion now waits
/// for the next wave instead of being picked up later in the same pass. Composite
/// nesting is a handful of levels deep, and a round is a scan plus one
/// [`build_alt_index`], so that trade is heavily on the wave's side.
pub(crate) fn resolve_pending<V, B>(
    cache: &mut HashMap<String, V>,
    mut pending: Vec<PendingGlyph>,
    aligns: &crate::document::AnchorAligns,
    mut declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    builder: &mut B,
    mut on_issue: impl FnMut(&str, crate::ref_composite::DeriveIssue),
    cancel: &crate::cancel::CancelToken,
) where
    V: CachedGlyphEntry + Send + Sync,
    B: CompositeBuilder<V>,
{
    drop_unresolvable(cache, &mut pending);
    // Counted after the drop above, so an alternative that can never resolve
    // does not hold the guard shut: it would never reach `alt_index` either
    // way, and leaving it counted only delayed every dependent composite to
    // the relaxation round.
    let mut rounds = Rounds::new(pending.iter().map(|pg| pg.name.as_str()));
    // Counts inner-loop steps, not resolved glyphs: a round that resolves
    // nothing still walks every pending glyph, and that walk has to be
    // interruptible too.
    let mut steps = 0usize;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let mut progress = false;
        let mut alt_index = build_alt_index(cache);
        // What this round will trace: the glyph, the refs its derivation
        // settled on, the anchors to record with it, and the memo key the
        // builder handed back (with the value already, when the memo had it).
        let mut wave = Vec::new();
        // Held out of `pending` until the scan is over, which would otherwise
        // walk into them again this same round.
        let mut deferred = Vec::new();
        let mut i = 0;
        while i < pending.len() {
            steps += 1;
            if steps.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
                return;
            }
            if rounds.must_wait(&pending[i].refs, |name| {
                resolve_cached(name, cache).is_some()
            }) {
                i += 1;
                continue;
            }
            let pg = pending.swap_remove(i);
            let origin_of =
                |name: &str| resolve_cached(name, cache).map_or((0, 0), |v| v.declared_origin());
            let (mut effective_refs, anchors, issues) =
                crate::ref_composite::derive_ref_offsets_with(
                    &pg.points,
                    &pg.refs,
                    pg.scale,
                    aligns,
                    |name| resolve_cached(name, cache).map(|v| v.anchors().to_vec()),
                    |name| alt_index.get(name).map_or_else(Vec::new, |v| v.clone()),
                    &mut declared_anchors,
                    origin_of,
                );
            if Rounds::picked_unbuilt(&effective_refs, |name| {
                resolve_cached(name, cache).is_some()
            }) {
                deferred.push(pg);
                continue;
            }
            crate::ref_composite::rebase_offsets_to_box(&mut effective_refs, pg.scale, origin_of);
            let errored = !issues.is_empty();
            for issue in issues {
                on_issue(&pg.name, issue);
            }
            let anchors: Vec<GlyphPoint> = anchors.into_iter().map(|(p, _)| p).collect();
            rounds.settle(&pg.name);
            // Still counts as progress: the glyph left `pending`, so a round
            // that only dropped glyphs has to be followed by another one.
            progress = true;
            // A glyph whose derivation failed is dropped, and so is no
            // alternative for anything to pick.
            if errored {
                continue;
            }
            for prefix in crate::ref_composite::alternative_prefixes(&pg.name) {
                // Merged right away rather than at round end: a composite
                // later in the same round has to see this alternative.
                let alts = alt_index.entry(prefix.to_string()).or_default();
                match alts.binary_search_by(|(a, _)| a.as_str().cmp(&pg.name)) {
                    Ok(pos) => alts[pos].1 = anchors.clone(),
                    Err(pos) => alts.insert(pos, (pg.name.clone(), anchors.clone())),
                }
            }
            wave.push((pg, effective_refs, anchors));
        }
        pending.append(&mut deferred);
        let keys = crate::parallel::map_indexed(wave.len(), cancel, |i| {
            let (pg, refs, _) = &wave[i];
            builder.key(pg, refs, cache)
        });
        if cancel.is_cancelled() {
            return;
        }
        let wave: Vec<WaveEntry<B::Key, V>> = wave
            .into_iter()
            .zip(keys)
            .map(|((pg, refs, anchors), key)| {
                let key = key.expect("an uncancelled run fills every slot");
                let hit = builder.lookup(&key);
                (pg, refs, anchors, key, hit)
            })
            .collect();

        // Only the memo misses are traced, and every one of them is filed back
        // into the memo here rather than by the tracer, which cannot reach it.
        let traced = trace_wave(&wave, builder, cache, cancel);
        if cancel.is_cancelled() {
            return;
        }
        for ((pg, _, anchors, key, hit), built) in wave.into_iter().zip(traced) {
            let mut entry = match built {
                Some(value) => {
                    builder.store(key, &value);
                    value
                }
                None => match hit {
                    Some(value) => value,
                    None => continue,
                },
            };
            if let Some(grid) = &pg.pixels {
                let (w, h) = entry.dims_mut();
                *w = (*w).max(grid.width);
                *h = (*h).max(grid.height);
            }
            entry.set_resolution(anchors, pg.scale, pg.declared_origin);
            cache.insert(pg.name, entry);
        }

        if !rounds.next_round(!pending.is_empty(), progress) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancelToken;
    use crate::document::GlyphPoint;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The smallest thing the driver can cache: enough to be a
    /// `CachedGlyphEntry`, nothing more.
    #[derive(Default)]
    struct Counted {
        width: u16,
        height: u16,
        anchors: Vec<GlyphPoint>,
    }

    impl CachedGlyphEntry for Counted {
        fn anchors(&self) -> &[GlyphPoint] {
            &self.anchors
        }
        fn declared_origin(&self) -> (i16, i16) {
            (0, 0)
        }
        fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
            (&mut self.width, &mut self.height)
        }
        fn set_resolution(&mut self, anchors: Vec<GlyphPoint>, _scale: u8, _origin: (i16, i16)) {
            self.anchors = anchors;
        }
    }

    /// `n` glyphs, each a one-pixel grid, plus `n` composites referencing the
    /// first of them. Big enough that a stop after the first composite is
    /// visible against [`CANCEL_STRIDE`].
    fn source(n: usize) -> crate::document::Document {
        let mut src = String::from("glyph base 1 1\n@\n");
        for i in 0..n {
            src.push_str(&format!("glyph plain{i} 1 1\n@\n"));
        }
        for i in 0..n {
            src.push_str(&format!("glyph comp{i}\nref base 0 0\n"));
        }
        crate::document_io::parse_document_from_str(&src, "cancel.unf".into()).unwrap()
    }

    const N: usize = CANCEL_STRIDE * 8;

    /// Cancelling mid-seed stops tracing: without the check, `from_grid` runs
    /// once per glyph however stale the build already is, and on a real font
    /// that is the bulk of a build nobody will read.
    #[test]
    fn cancelling_mid_seed_stops_before_the_last_glyph() {
        let doc = source(N);
        let cancel = CancelToken::new();
        let mut traced = 0usize;

        let (cache, _pending) = seed_cache(
            &doc.items,
            |_, _, _| {
                traced += 1;
                cancel.cancel();
                Counted::default()
            },
            Counted::default,
            &cancel,
        );

        assert!(
            traced < N,
            "seeding traced all {N} grids despite being cancelled at the first"
        );
        assert!(
            cache.len() <= CANCEL_STRIDE,
            "seeding ran {} glyphs past the cancel, more than one stride",
            cache.len()
        );
    }

    /// The same for the fixpoint loop, which is where composites — the
    /// expensive half — are built.
    #[test]
    fn cancelling_mid_resolve_stops_before_the_last_composite() {
        let doc = source(N);
        let cancel = CancelToken::new();
        let never = CancelToken::never();
        let (mut cache, pending) = seed_cache(
            &doc.items,
            |_, _, _| Counted::default(),
            Counted::default,
            &never,
        );
        assert_eq!(pending.len(), N, "every composite starts out pending");

        let built = AtomicUsize::new(0);
        resolve_pending(
            &mut cache,
            pending,
            &Default::default(),
            |_| None,
            &mut FnBuilder(|_: &_, _: &_, _: &_| {
                built.fetch_add(1, Ordering::Relaxed);
                cancel.cancel();
                Counted::default()
            }),
            |_, _| {},
            &cancel,
        );

        // A wave traces on every core, so the threads already inside `build`
        // when the first one cancels still finish theirs — a handful, not a
        // stride each, since the token is read per composite.
        let built = built.into_inner();
        assert!(
            built < N,
            "the fixpoint loop built all {N} composites despite being cancelled at the first"
        );
        assert!(
            built <= CANCEL_STRIDE,
            "the loop ran {built} composites past the cancel, more than one stride"
        );
    }

    /// A `never` token changes nothing: the same source resolves completely.
    #[test]
    fn an_uncancelled_resolve_still_builds_everything() {
        let doc = source(N);
        let never = CancelToken::never();
        let (mut cache, pending) = seed_cache(
            &doc.items,
            |_, _, _| Counted::default(),
            Counted::default,
            &never,
        );
        let built = AtomicUsize::new(0);
        resolve_pending(
            &mut cache,
            pending,
            &Default::default(),
            |_| None,
            &mut FnBuilder(|_: &_, _: &_, _: &_| {
                built.fetch_add(1, Ordering::Relaxed);
                Counted::default()
            }),
            |_, _| {},
            &never,
        );
        assert_eq!(built.into_inner(), N);
    }
}
