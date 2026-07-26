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

/// A glyph waiting for all of its refs to resolve.
pub(crate) struct PendingGlyph {
    pub name: String,
    pub pixels: Option<PixelGrid>,
    pub refs: Vec<GlyphRef>,
    pub points: Vec<GlyphPoint>,
    pub scale: u8,
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
pub(crate) fn resolve_cached<'a, V>(
    name: &str,
    cache: &'a HashMap<String, V>,
) -> Option<&'a V> {
    if let Some(cached) = cache.get(name) {
        return Some(cached);
    }
    let expanded = crate::ref_composite::parse_ref_pattern(name)?;
    cache.get(&expanded.get(0))
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
/// directly via `from_grid`, glyphs with refs (or pixels alongside refs)
/// become pending, and sticky placeholder glyphs enter as `empty` entries
/// that only carry anchors.
pub(crate) fn seed_cache<V: CachedGlyphEntry>(
    all_items: &[DocumentItem],
    mut from_grid: impl FnMut(&PixelGrid) -> V,
    mut empty: impl FnMut() -> V,
) -> (HashMap<String, V>, Vec<PendingGlyph>) {
    let mut cache: HashMap<String, V> = HashMap::new();
    let mut pending: Vec<PendingGlyph> = Vec::new();

    for item in all_items {
        let (cache_key, body) = match item {
            DocumentItem::Glyph { name: GlyphName(n), body } => (n.clone(), body),
            _ => continue,
        };
        if !cache_key.is_empty() && !cache.contains_key(&cache_key) {
            if let Some(ref pixels) = body.pixels && body.refs.is_empty() {
                let mut cached = from_grid(pixels);
                cached.set_resolution(body.points.clone(), body.scale);
                cache.insert(cache_key, cached);
            } else if body.pixels.is_some() || !body.refs.is_empty() {
                pending.push(PendingGlyph {
                    name: cache_key,
                    pixels: body.pixels.clone(),
                    refs: body.refs.clone(),
                    points: body.points.clone(),
                    scale: body.scale,
                });
            } else if body.sticky {
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
/// are reported elsewhere.
pub(crate) fn resolve_pending<V: CachedGlyphEntry>(
    cache: &mut HashMap<String, V>,
    mut pending: Vec<PendingGlyph>,
    mut declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut build: impl FnMut(&PendingGlyph, &[GlyphRef], &HashMap<String, V>) -> V,
) {
    let mut progress = true;
    while progress {
        progress = false;
        let alt_index = build_alt_index(cache);
        let mut i = 0;
        while i < pending.len() {
            if !pending[i]
                .refs
                .iter()
                .all(|gref| resolve_cached(&gref.name, cache).is_some())
            {
                i += 1;
                continue;
            }
            let pg = pending.swap_remove(i);
            let (effective_refs, anchors) = crate::ref_composite::derive_ref_offsets_with(
                &pg.points,
                &pg.refs,
                |name| resolve_cached(name, cache).map(|v| v.anchors().to_vec()),
                |name| alt_index.get(name).map_or_else(Vec::new, |v| v.clone()),
                &mut declared_anchors,
            );
            let mut entry = build(&pg, &effective_refs, cache);
            if let Some(grid) = &pg.pixels {
                let (w, h) = entry.dims_mut();
                *w = (*w).max(grid.width);
                *h = (*h).max(grid.height);
            }
            entry.set_resolution(anchors, pg.scale);
            cache.insert(pg.name.clone(), entry);
            progress = true;
        }
    }
}
