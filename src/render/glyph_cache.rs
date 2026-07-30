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
/// directly via `from_grid`, glyphs with refs (or pixels alongside refs)
/// become pending, and sticky placeholder glyphs enter as `empty` entries
/// that only carry anchors.
pub(crate) fn seed_cache<'a, V: CachedGlyphEntry>(
    all_items: impl IntoIterator<Item = &'a DocumentItem>,
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
    mut on_issue: impl FnMut(&str, crate::ref_composite::DeriveIssue),
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
            let (effective_refs, anchors, issues) = crate::ref_composite::derive_ref_offsets_with(
                &pg.points,
                &pg.refs,
                |name| resolve_cached(name, cache).map(|v| v.anchors().to_vec()),
                |name| alt_index.get(name).map_or_else(Vec::new, |v| v.clone()),
                &mut declared_anchors,
            );
            for issue in issues {
                on_issue(&pg.name, issue);
            }
            let mut entry = build(&pg, &effective_refs, cache);
            if let Some(grid) = &pg.pixels {
                let (w, h) = entry.dims_mut();
                *w = (*w).max(grid.width);
                *h = (*h).max(grid.height);
            }
            entry.set_resolution(anchors.into_iter().map(|(p, _)| p).collect(), pg.scale);
            cache.insert(pg.name.clone(), entry);
            progress = true;
        }
    }
}
