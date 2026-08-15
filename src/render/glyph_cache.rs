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

use std::collections::HashMap;

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
    /// `desync`: `pixels` is bitmap ink and nothing else. What that means for
    /// a cache value is the *consumer's* decision — this driver only carries
    /// the flag — but every consumer that draws an outline has to make it, or
    /// the grid reappears in a face that must not have it.
    pub desync: bool,
}

/// A cache value the shared resolution driver can operate on.
pub(crate) trait CachedGlyphEntry {
    fn anchors(&self) -> &[GlyphPoint];
    fn dims_mut(&mut self) -> (&mut u16, &mut u16);
    fn set_resolution(&mut self, anchors: Vec<GlyphPoint>, scale: u8);
}

/// Looks up `name` in the cache, falling back to the first expansion of a
/// pattern name (`digit(0|1)` resolves via `digit0` when the pattern string
/// itself is not a cache key).
pub(crate) fn resolve_cached<'a, V>(name: &str, cache: &'a HashMap<String, V>) -> Option<&'a V> {
    if let Some(cached) = cache.get(name) {
        return Some(cached);
    }
    let expanded = crate::ref_composite::parse_ref_pattern(name)?;
    cache.get(&expanded.get(0))
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
        .take_while(|&r| (0..grid.width).all(|c| grid.get(r, c).is_empty()))
        .count() as i32;
    let blank_cols = (0..grid.width)
        .take_while(|&c| (0..grid.height).all(|r| grid.get(r, c).is_empty()))
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
    let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
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
/// directly via `from_grid` (which is told whether the glyph is `desync`),
/// glyphs with refs (or pixels alongside refs) become pending, and bodiless
/// `keep` placeholders enter as `empty` entries that only carry anchors.
///
/// `from_grid` is where the font build traces contours, so `cancel` is checked
/// every [`CANCEL_STRIDE`] items; a cancelled seeding returns whatever it had
/// built so far, which the caller discards along with everything downstream.
pub(crate) fn seed_cache<'a, V: CachedGlyphEntry>(
    all_items: impl IntoIterator<Item = &'a DocumentItem>,
    mut from_grid: impl FnMut(&PixelGrid, bool) -> V,
    mut empty: impl FnMut() -> V,
    cancel: &crate::cancel::CancelToken,
) -> (HashMap<String, V>, Vec<PendingGlyph>) {
    let mut cache: HashMap<String, V> = HashMap::new();
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
                let mut cached = from_grid(pixels, body.desync);
                cached.set_resolution(body.points.clone(), body.scale);
                cache.insert(cache_key, cached);
            } else if body.pixels.is_some() || !body.refs.is_empty() {
                pending.push(PendingGlyph {
                    name: cache_key,
                    pixels: body.pixels.clone(),
                    refs: body.refs.clone(),
                    points: body.points.clone(),
                    scale: body.scale,
                    desync: body.desync,
                });
            } else if body.keep {
                let mut cached = empty();
                cached.set_resolution(body.points.clone(), 1);
                cache.insert(cache_key, cached);
            }
        }
    }

    (cache, pending)
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
pub(crate) fn resolve_pending<V: CachedGlyphEntry>(
    cache: &mut HashMap<String, V>,
    mut pending: Vec<PendingGlyph>,
    mut declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut build: impl FnMut(&PendingGlyph, &[GlyphRef], &HashMap<String, V>) -> V,
    mut on_issue: impl FnMut(&str, crate::ref_composite::DeriveIssue),
    cancel: &crate::cancel::CancelToken,
) {
    // How many alternatives of each base name are still unresolved. A
    // composite must not be derived while an alternative of one of its
    // offset-less refs is pending: that alternative would be missing from
    // `alt_index`, so a substitution that only *it* can satisfy (by anchor
    // size) silently falls through. Same guard, same relaxation for cycles,
    // as `ref_composite::resolve_expansion` — the two fixpoints must not
    // disagree about which alternative a composite gets.
    let mut pending_alts: HashMap<String, usize> = HashMap::new();
    for pg in &pending {
        for prefix in crate::ref_composite::alternative_prefixes(&pg.name) {
            *pending_alts.entry(prefix.to_string()).or_default() += 1;
        }
    }
    let mut relaxed = false;
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
        let mut i = 0;
        while i < pending.len() {
            steps += 1;
            if steps.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
                return;
            }
            let blocked = !pending[i]
                .refs
                .iter()
                .all(|gref| resolve_cached(&gref.name, cache).is_some())
                || (!relaxed
                    && pending[i]
                        .refs
                        .iter()
                        .any(|r| r.offset.is_none() && pending_alts.contains_key(&r.name)));
            if blocked {
                i += 1;
                continue;
            }
            let pg = pending.swap_remove(i);
            let (effective_refs, anchors, issues) = crate::ref_composite::derive_ref_offsets_with(
                &pg.points,
                &pg.refs,
                |name| resolve_cached(name, cache).map(|v| v.anchors().to_vec()),
                |name| alt_index.get(name).map_or_else(Vec::new, |v| v.clone()),
                &mut declared_anchors,
            );
            let errored = !issues.is_empty();
            for issue in issues {
                on_issue(&pg.name, issue);
            }
            let anchors: Vec<GlyphPoint> = anchors.into_iter().map(|(p, _)| p).collect();
            // The counts have to come down either way — they say how many
            // alternatives are still *pending*, and this one no longer is,
            // whether or not it produced a glyph. Leaving a count standing
            // would block every composite that refs the base until the first
            // barren round relaxes the guard.
            for prefix in crate::ref_composite::alternative_prefixes(&pg.name) {
                if let Some(count) = pending_alts.get_mut(prefix) {
                    *count -= 1;
                    if *count == 0 {
                        pending_alts.remove(prefix);
                    }
                }
                if errored {
                    continue;
                }
                // Merged right away rather than at round end: a composite
                // later in the same round has to see this alternative.
                let alts = alt_index.entry(prefix.to_string()).or_default();
                match alts.binary_search_by(|(a, _)| a.as_str().cmp(&pg.name)) {
                    Ok(pos) => alts[pos].1 = anchors.clone(),
                    Err(pos) => alts.insert(pos, (pg.name.clone(), anchors.clone())),
                }
            }
            // Still counts as progress: the glyph left `pending`, so a round
            // that only dropped glyphs has to be followed by another one.
            progress = true;
            if errored {
                continue;
            }
            let mut entry = build(&pg, &effective_refs, cache);
            if let Some(grid) = &pg.pixels {
                let (w, h) = entry.dims_mut();
                *w = (*w).max(grid.width);
                *h = (*h).max(grid.height);
            }
            entry.set_resolution(anchors, pg.scale);
            cache.insert(pg.name.clone(), entry);
        }
        if pending.is_empty() {
            break;
        }
        if progress {
            relaxed = false;
        } else if relaxed {
            break;
        } else {
            relaxed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancelToken;
    use crate::document::GlyphPoint;

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
        fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
            (&mut self.width, &mut self.height)
        }
        fn set_resolution(&mut self, anchors: Vec<GlyphPoint>, _scale: u8) {
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
            |_, _| {
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
            |_, _| Counted::default(),
            Counted::default,
            &never,
        );
        assert_eq!(pending.len(), N, "every composite starts out pending");

        let mut built = 0usize;
        resolve_pending(
            &mut cache,
            pending,
            |_| None,
            |_, _, _| {
                built += 1;
                cancel.cancel();
                Counted::default()
            },
            |_, _| {},
            &cancel,
        );

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
            |_, _| Counted::default(),
            Counted::default,
            &never,
        );
        let mut built = 0usize;
        resolve_pending(
            &mut cache,
            pending,
            |_| None,
            |_, _, _| {
                built += 1;
                Counted::default()
            },
            |_, _| {},
            &never,
        );
        assert_eq!(built, N);
    }
}
