//! Composite (`ref`) resolution: anchor alignment, the layer stack a
//! glyph resolves to, and on-demand glyph synthesis.
//!
//! # Anchor exposure is opt-in
//!
//! Inside a composite ([`derive_ref_offsets_with`]), a ref's `-name` anchors
//! attach to a *unique* size-matching `+name` published by a sibling — or
//! declared by the composite itself — consuming it; the ref's own `+` anchors
//! are then published in turn. All of that happens regardless of flags.
//!
//! Several `-` anchors on one ref are **alternatives**, not several
//! attachments: they are how one combining mark reaches more than one anchor
//! system (`gr-psili` joins a Greek `+gr-above` or a plain `+above`). The
//! first with a candidate wins and the rest retire silently, taking their
//! `+` partners with them — a mark publishes only the system it joined. A `+`
//! with no `-` of the same name is a base's hosting point and is never
//! retired, which is why `s-upper` still offers `+above` to Ṩ's second mark
//! after `+below` was taken.
//!
//! What the composite *exposes* to the outside — GPOS base anchors, the editor's
//! anchor shadow, further composition — is only its own declared anchors plus
//! the surviving anchors of refs marked `inherit`. So a digraph or a circled
//! letter exposes nothing it did not say. `map generate` composites stand in
//! for their decomposition, so their synthesized refs inherit implicitly.
//!
//! The flag governs *exposure only*. It must not decide which form of a glyph
//! a sibling attaches to: that is a question about the alternative itself, and
//! [`try_lookahead_alt`] answers it from declared anchors and the alternative
//! index, exactly as `render/ttf_builder/gpos.rs` does. Reading the primary's
//! exposed set there instead once made every generated `ï í ì ī ǐ î ĭ ĩ`
//! compose over dotted `i-lower`, while shaping the decomposed input still
//! substituted `i-lower:dotless` — one font, two answers.
//!
//! Two rules here are load-bearing and deliberately loud — [`crate::issues`]
//! reports them as errors, through an anchors-only pass sharing
//! `render/glyph_cache.rs`'s driver:
//!
//! - an exposed set containing the same anchor name twice exposes *neither*
//!   (declare it on the composite explicitly instead);
//! - a `-` anchor with more than one size-matching `+` candidate attaches to
//!   *nothing*.
//!
//! # A negative `ref` offset is a bearing
//!
//! It is not something to normalize away. The glyph origin stays at (0, 0), the
//! outline keeps its negative coordinates (a negative lsb, or ink above the
//! ascent), and the advance still measures only the extent to the *right* of the
//! origin. `origin C R` is for shifting a glyph that has no such ref; it is not
//! for undoing a negative offset.
//!
//! Every composite path has to agree on this, which is why `CachedContours` and
//! `CachedGlyph` carry `origin_row`/`origin_col` — the logical coordinate of
//! raster cell (0, 0) — beside their normalized grid: a parent adds a ref
//! target's origin to the `ref` offset when placing it, or it silently loses
//! whatever sits left of that origin. A bearing exists only where something is
//! drawn, so `glyph_cache::trim_blank_before_origin` trims the
//! blank margin before the origin and pulls `origin_*` back towards zero.
//!
//! # On-demand glyphs
//!
//! A name that nothing defines but that matches a synthesizable shape is
//! generated on demand, and such a glyph is implicitly `inline`. The grammar,
//! the geometry and the bitmap-fill rule all live in [`crate::on_demand`];
//! resolution only has to ask it whether a name is one.

mod anchors;
mod composite;

pub(crate) use anchors::*;
#[cfg(any(feature = "editor", test))]
pub(crate) use composite::*;
use composite::{CompositeLayout, resolve_composite_layout};

use std::collections::HashMap;

use crate::document::{
    Document, DocumentItem, GlyphName, GlyphPoint, GlyphRef, NamePartsMap, PixelGrid,
    substitute_name_parts,
};
use crate::pattern::NamePattern;
// Only the composite helpers and their tests need this.
#[cfg(any(feature = "editor", test))]
use crate::document::GlyphBody;

#[cfg(feature = "editor")]
const PHI: f64 = 1.618033988749895;

#[cfg(feature = "editor")]
pub fn ref_color_sv(s: f32, v: f32, index: usize) -> egui::Color32 {
    let hue = ((index + 1) as f64 / PHI % 1.0 * 360.0) as f32;
    hsv_to_rgb(hue, s, v)
}

#[cfg(feature = "editor")]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    egui::Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// The flattened grids of [`resolve_expansion`], memoized across runs.
///
/// Composing one is a per-cell exact boolean over sub-pixel regions, and over
/// a full source it is essentially *all* of what a resolve costs: for `font/`,
/// 1.28 s of a 1.42 s run. Nothing else in the resolve is expensive, so an
/// editor that recomposes every glyph on every edit spends a second and a half
/// per keystroke burst on grids that did not change — while the font build,
/// which memoizes exactly this through
/// [`ContourCache`](crate::render::ttf_builder::ContourCache), lands in a
/// fraction of that. The pixel grid then trails the built font by a second or
/// more, and an edit cadence faster than the resolve cancels every one of them
/// so it never lands at all.
///
/// The key is the composite's *whole* input: its own pixels, and each layer's
/// target grid, scale and raster placement — the layers being the resolution's
/// own output, so anchor-derived offsets and the alternative that was picked
/// are already folded in. Two glyphs composing the same layers the same way
/// share an entry, and a glyph whose inputs are untouched by an edit is not
/// recomposed. Mirrors `ContourCache::composite_entries` deliberately: same
/// key material, same generation sweep.
#[derive(Default)]
pub struct CompositeGridCache {
    entries: HashMap<u64, (u64, PixelGrid)>,
    gen_id: u64,
    hits: usize,
    misses: usize,
}

impl CompositeGridCache {
    /// Entries reached during the run that just ended; everything else is what
    /// [`Self::evict_stale`] drops.
    fn begin_generation(&mut self) {
        self.gen_id += 1;
        self.hits = 0;
        self.misses = 0;
    }

    /// Only ever called by a run that *finished*: a cancelled resolve touched
    /// an arbitrary prefix of the glyphs, so what it did not reach is not
    /// stale, merely unvisited. Keeping it costs memory until the next
    /// completed run, which is the same trade `ContourCache` makes.
    fn evict_stale(&mut self) {
        let cur = self.gen_id;
        self.entries.retain(|_, (seen, _)| *seen == cur);
    }

    /// Drop everything, for a source that has nothing to do with what is
    /// cached — switching folders, where every key belongs to a font that is
    /// no longer open. Only the editor ever switches folders.
    #[cfg(feature = "editor")]
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Composites the last run reused and recomposed, in that order. Read by
    /// the editor's `UNIFORM_PERF` logging and by tests, and by nothing in the
    /// headless binary.
    #[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
    pub fn stats(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }

    /// The memoized grid for `key`, marked as reached by this run. The two
    /// halves are separate because the flattening between them runs on other
    /// threads, which cannot hold this `&mut`.
    fn take_hit(&mut self, key: u64) -> Option<PixelGrid> {
        let cur = self.gen_id;
        let (seen, grid) = self.entries.get_mut(&key)?;
        *seen = cur;
        self.hits += 1;
        Some(grid.clone())
    }

    /// Records what a miss flattened to.
    fn record_miss(&mut self, key: u64, grid: &PixelGrid) {
        let cur = self.gen_id;
        self.misses += 1;
        self.entries.insert(key, (cur, grid.clone()));
    }
}

fn hash_grid_into(grid: &PixelGrid, hasher: &mut std::collections::hash_map::DefaultHasher) {
    use std::hash::Hash;
    grid.width.hash(hasher);
    grid.height.hash(hasher);
    for px in &grid.pixels {
        px.0.hash(hasher);
    }
    if !grid.details.is_empty() {
        grid.den.hash(hasher);
        grid.details.hash(hasher);
    }
}

/// One glyph resolved against its refs, as the editor and the `assert`
/// directives see it.
///
/// This is deliberately the *source* view: a `desync` glyph's own pixel grid
/// is part of `grid` here even though the vector build of the font ignores it
/// (see [`crate::render::ttf_builder`]). The editor has to draw the grid it
/// lets you edit, and `assert same`/`distinct` compare what the file says; the
/// two faces only diverge where they are actually built, in `ttf_builder` and
/// `render/sample.rs`.
#[derive(Clone)]
pub struct ResolvedGlyph {
    pub grid: PixelGrid,
    /// Logical coordinate represented by raster cell `(0, 0)`. Keeping this
    /// separate from the raster is essential for nested refs whose bounds
    /// extend left/up from the glyph origin.
    pub(crate) origin_row: i32,
    pub(crate) origin_col: i32,
    pub(crate) resolved_anchors: Vec<GlyphPoint>,
    /// The glyph body's own declared anchor lines (not forwarded
    /// from refs).  Used by look-ahead alternative selection.
    pub(crate) declared_anchors: Vec<GlyphPoint>,
    pub scale: u8,
    /// The box the glyph's `glyph` header declares, in declared (un-`scale`d)
    /// units, or `None` for a glyph whose header declares none.
    ///
    /// Kept beside the resolved raster because [`crate::compose`] places an IDC
    /// component by what it *declares*, never by what it happens to resolve to,
    /// and the editor's live composite has to read the same number the build
    /// read — a glyph is not allowed to be laid out one way on screen and
    /// another in the font.
    #[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
    pub declared_box: Option<(u16, u16)>,
    /// Where this glyph's declared box starts inside its own grid, in declared
    /// (un-`scale`d) cells — [`GlyphBody::declared_origin`].
    ///
    /// A `ref` offset runs from the parent's box origin to the child's, so both
    /// ends of every placement read this: the child's own is subtracted here,
    /// the parent's is added by [`resolve_composite_layout`].
    pub(crate) declared_origin: (i16, i16),
    /// What this glyph is *declared* as, for the one consumer that has to undo
    /// one level of composition rather than read the result of all of them:
    /// the editor's "Inline once". `None` unless the glyph declares refs — a
    /// glyph made of pixels alone already *is* its `grid`, and an on-demand
    /// shape has no declaration to expand at all.
    #[cfg_attr(not(feature = "editor"), allow(dead_code))]
    pub inline_source: Option<std::sync::Arc<InlineSource>>,
}

/// A composite's own declaration, as "Inline once" pastes it in place of a
/// `ref` to it.
///
/// The refs are the *effective* ones — an anchor-positioned ref carries the
/// offset resolution derived for it, and the name of the alternative that was
/// actually chosen — so inlining reproduces the placement on screen instead of
/// re-deriving it in a parent whose anchors are not the same. Coordinates are
/// this glyph's own, at its own `scale`; the caller rebases them.
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
pub struct InlineSource {
    pub refs: Vec<GlyphRef>,
    /// The pixels this glyph draws itself, at its logical origin.
    pub pixels: Option<PixelGrid>,
}

/// The declared box behind a bare grid, for the one caller that has a grid but
/// no glyph body to ask: an on-demand shape, whose name states its own box.
/// Everything with a body goes through
/// [`GlyphBody::declared_extent`](crate::document::GlyphBody::declared_extent),
/// which knows about `extent` and `advance` as well.
pub fn declared_box(pixels: Option<&PixelGrid>, scale: u8) -> Option<(u16, u16)> {
    let g = pixels?;
    let s = scale.max(1) as u16;
    Some((g.width / s, g.height / s))
}

fn saturating_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn raster_dimension(min: i32, max: i32) -> u16 {
    max.saturating_sub(min).clamp(0, u16::MAX as i32) as u16
}

/// Pattern expansion used by runtime ref lookup and dependency resolution.
pub(crate) fn parse_ref_pattern(name: &str) -> Option<NamePattern> {
    NamePattern::parse_element(name).ok()
}

/// Detect any on-demand glyph for `name`: first tries WxH rect, then
/// color/mono composite (checking whether X:mono and X:color exist).
pub fn resolve_named_glyphs_with_parts(
    docs: &[&Document],
    name_parts: &NamePartsMap,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    let expand = crate::startup::PerfStage::new("expand");
    let expansion = crate::render::ttf_builder::expand_documents(docs, name_parts);
    drop(expand);
    let _resolve = crate::startup::PerfStage::new("resolve expansion");
    resolve_expansion(expansion, name_parts, &crate::cancel::CancelToken::never())
}

/// Compose every glyph in an already-expanded document set.
///
/// Name-part substitution, pattern expansion and on-demand/decomposed-map
/// synthesis all happen in [`crate::render::ttf_builder::expand_documents`],
/// so this function starts from the same glyph set the font build sees. It
/// used to redo all of that itself, which is how the editor and the built
/// font could disagree about which glyphs exist.
///
/// A cancelled run returns what it had resolved so far rather than an error:
/// the shape of that result — some composites resolved, the rest absent — is
/// one the pipeline already produces for a source whose refs do not resolve, so
/// no consumer needs a new case for it. The caller that cancelled discards it
/// whole; see [`crate::cancel`].
pub fn resolve_expansion(
    expansion: crate::render::ttf_builder::Expansion,
    name_parts: &NamePartsMap,
    cancel: &crate::cancel::CancelToken,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    resolve_expansion_cached(expansion, name_parts, cancel, None)
}

/// [`resolve_expansion`], reusing the flattened grids of an earlier run through
/// `grid_cache`. See [`CompositeGridCache`] for why anything resolving
/// repeatedly — which means the editor — wants one.
pub fn resolve_expansion_cached(
    expansion: crate::render::ttf_builder::Expansion,
    name_parts: &NamePartsMap,
    cancel: &crate::cancel::CancelToken,
    grid_cache: Option<&mut CompositeGridCache>,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    // The expansion is consumed, not borrowed: it already owns a full copy of
    // every glyph body, and cloning it a second time into `pending` cost more
    // than sharing the expansion saved.
    let aliases = expansion.aliases;
    let bodies = expansion.items.into_iter().filter_map(|e| match e.item {
        DocumentItem::Glyph {
            name: GlyphName(key),
            body,
        } => Some((key, body)),
        _ => None,
    });
    resolve_glyph_bodies(bodies, &aliases, name_parts, cancel, grid_cache)
}

/// Flatten `roots` and everything they reach, and nothing else.
///
/// This is what a caller with no [`Expansion`](crate::render::ttf_builder::Expansion)
/// to resolve — the expansion itself, and `fix::clearance` — uses to measure a
/// part that is a composite: the glyphs it walks are the ones the parts refer
/// to, transitively, rather than the whole font, since flattening 18k glyphs to
/// measure a hundred of them is not a trade anything here can afford.
///
/// `body_of` answers what a name is declared as. A name it does not know is an
/// **on-demand** shape if it parses as one — synthesized here exactly as
/// [`inject_on_demand_glyph_items`](crate::render::ttf_builder) would, because
/// the caller inside the expansion runs before that pass — and otherwise
/// nothing, in which case whatever refers to it stays unresolved and so
/// unmeasured. A glyph split by an IDC line is treated the same way: its refs
/// have not been derived yet at the point the expansion asks, and one of the
/// two callers seeing them and the other not is exactly the disagreement this
/// shared walk exists to prevent.
pub(crate) fn resolve_reachable<'a, 'b>(
    roots: impl Iterator<Item = &'a str>,
    body_of: &dyn Fn(&str) -> Option<&'b crate::document::GlyphBody>,
    aliases: &crate::alias::AliasMap,
    name_parts: &NamePartsMap,
) -> HashMap<String, ResolvedGlyph> {
    use crate::document::GlyphBody;

    let mut bodies: Vec<(String, GlyphBody)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<String> = Vec::new();
    for root in roots {
        if seen.insert(root.to_string()) {
            queue.push(root.to_string());
        }
    }
    while let Some(name) = queue.pop() {
        // The same three forms [`lookup_ref_name`] tries, in the same order, so
        // a ref written as a pattern or through a name part is followed here
        // the way it will be followed there.
        let subst = substitute_name_parts(&name, name_parts);
        let expanded = parse_ref_pattern(&subst).map(|pattern| pattern.get(0));
        let found = [Some(name.as_str()), Some(subst.as_str()), expanded.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|n| body_of(n));
        if let Some(body) = found {
            // Not measurable until its line has been expanded into refs.
            if !body.compose.is_empty() {
                continue;
            }
            for r in &body.refs {
                if seen.insert(r.name.clone()) {
                    queue.push(r.name.clone());
                }
            }
            // Kept under the name that was *written*, whichever of the three
            // forms found it, so the composite naming it finds it back.
            bodies.push((name, body.clone()));
            continue;
        }
        // Only the self-contained shapes, as [`synthesized_on_demand`] takes: a
        // color/mono pair is built from two glyph bodies, which is a synthesis
        // and not a lookup.
        let shape = [Some(name.as_str()), Some(subst.as_str()), expanded.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|n| match crate::on_demand::parse_on_demand_glyph(n) {
                Some(crate::on_demand::OnDemandGlyph::Shape(spec)) => Some(spec),
                _ => None,
            });
        if let Some(spec) = shape {
            bodies.push((
                name,
                GlyphBody {
                    scale: spec.scale,
                    pixels: Some(crate::on_demand::make_on_demand_grid(&spec)),
                    inline: true,
                    ..GlyphBody::new()
                },
            ));
        }
    }
    resolve_glyph_bodies(
        bodies.into_iter(),
        aliases,
        name_parts,
        &crate::cancel::CancelToken::never(),
        None,
    )
    .0
}

/// [`resolve_expansion_cached`] over a glyph set stated directly, for the one
/// caller that has no [`Expansion`](crate::render::ttf_builder::Expansion) to
/// give: the expansion itself.
///
/// A clearance check has to measure the ink of the parts an IDC line names, and
/// a part that is a composite draws none of its own — so the parts are resolved
/// here, in the middle of the expansion that will go on to resolve everything.
/// It is the same resolution over a subset, which is the point: a part measured
/// by a second, simpler flattener would be measured differently from the one
/// the font is built from. See `ttf_builder::expand::ink_profiles`.
///
/// `aliases` is only read at the very end, to give every alias name the
/// resolution of its target; the bodies themselves are expected to name
/// canonical targets already, as an expansion's do.
pub(crate) fn resolve_glyph_bodies(
    bodies: impl Iterator<Item = (String, crate::document::GlyphBody)>,
    aliases: &crate::alias::AliasMap,
    name_parts: &NamePartsMap,
    cancel: &crate::cancel::CancelToken,
    mut grid_cache: Option<&mut CompositeGridCache>,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    if let Some(gc) = grid_cache.as_deref_mut() {
        gc.begin_generation();
    }
    let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();

    struct Pending {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
        scale: u8,
        declared_box: Option<(u16, u16)>,
        declared_origin: (i16, i16),
    }

    let mut pending: Vec<Pending> = Vec::new();
    // Mirrors `pending` names for O(1) duplicate checks; a linear scan here
    // is quadratic over the whole font (~18k glyphs).
    let mut pending_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (i, (key, body)) in bodies.enumerate() {
        if i.is_multiple_of(crate::render::glyph_cache::CANCEL_STRIDE) && cancel.is_cancelled() {
            let alt_index = AlternativesIndex::build(&cache);
            return (cache, alt_index);
        }
        // First definition wins, matching the font build.
        if cache.contains_key(&key) || pending_names.contains(&key) {
            continue;
        }
        if body.refs.is_empty() {
            let declared_box = body.declared_extent();
            let declared_origin = body.declared_origin();
            cache.insert(
                key,
                ResolvedGlyph {
                    grid: body.pixels.unwrap_or_else(|| PixelGrid::new(0, 0)),
                    origin_row: 0,
                    origin_col: 0,
                    resolved_anchors: body.points.clone(),
                    declared_anchors: body.points,
                    scale: body.scale,
                    declared_box,
                    declared_origin,
                    inline_source: None,
                },
            );
        } else {
            pending_names.insert(key.clone());
            let declared_origin = body.declared_origin();
            pending.push(Pending {
                name: key,
                declared_box: body.declared_extent(),
                declared_origin,
                pixels: body.pixels,
                refs: body.refs,
                points: body.points,
                scale: body.scale,
            });
        }
    }

    // Built once and grown incrementally: a full rebuild clones every name
    // and anchor list in the cache, which is too expensive to repeat per
    // fixpoint round.
    let mut alt_index = AlternativesIndex::build(&cache);

    // How many alternatives of each base name are still unresolved. A composite
    // must not be derived while one of them is pending: the alternative would be
    // missing from `alt_index`, so a ref whose anchors only size-match *that*
    // alternative silently falls back to offset (0, 0) instead. `i-upper` +
    // `acute-above` did exactly that — serving `i-upper`'s two-cell `+above` is
    // the whole reason `acute-above:wide` exists.
    let mut pending_alts: HashMap<String, usize> = HashMap::new();
    for pg in &pending {
        for prefix in alternative_prefixes(&pg.name) {
            *pending_alts.entry(prefix.to_string()).or_default() += 1;
        }
    }

    // Dropped for one round whenever that guard is what blocks every remaining
    // glyph (a reference cycle running through an alternative), so resolution
    // still terminates with the fallbacks it produced before.
    let mut relaxed = false;
    // Inner-loop steps rather than resolved glyphs, so a round that resolves
    // nothing is interruptible too.
    let mut steps = 0usize;
    loop {
        if cancel.is_cancelled() {
            return (cache, alt_index);
        }
        let mut progress = false;
        // What this round will flatten. Deriving stays serial — it reads and
        // writes `alt_index` and `pending_alts`, and a composite later in the
        // round has to see the alternatives resolved before it — but flattening
        // is pure, and it is where the time goes. See
        // `glyph_cache::resolve_pending`, whose rounds are waves for the same
        // reason.
        let mut wave: Vec<(Pending, Vec<GlyphRef>, Vec<GlyphPoint>)> = Vec::new();
        for pg in std::mem::take(&mut pending) {
            steps += 1;
            if steps.is_multiple_of(crate::render::glyph_cache::CANCEL_STRIDE)
                && cancel.is_cancelled()
            {
                return (cache, alt_index);
            }
            if !pg
                .refs
                .iter()
                .all(|r| resolve_ref_name_with_parts(&r.name, &cache, name_parts).is_some())
                || (!relaxed
                    && pg
                        .refs
                        .iter()
                        .any(|r| r.offset.is_none() && pending_alts.contains_key(&r.name)))
            {
                pending.push(pg);
                continue;
            }
            let origin_of = |name: &str| {
                resolve_ref_name_with_parts(name, &cache, name_parts)
                    .map_or((0, 0), |resolved| resolved.declared_origin)
            };
            let (mut effective_refs, exposed, _issues) = derive_ref_offsets_with(
                &pg.points,
                &pg.refs,
                pg.scale,
                |name| {
                    resolve_ref_name_with_parts(name, &cache, name_parts)
                        .map(|resolved| resolved.resolved_anchors.clone())
                },
                |name| alt_index.get(name).to_vec(),
                |name| {
                    resolve_ref_name_with_parts(name, &cache, name_parts)
                        .map(|resolved| resolved.declared_anchors.clone())
                },
                origin_of,
            );
            rebase_offsets_to_box(&mut effective_refs, pg.scale, origin_of);
            let anchors: Vec<GlyphPoint> = exposed.into_iter().map(|(p, _)| p).collect();
            for prefix in alternative_prefixes(&pg.name) {
                if let Some(count) = pending_alts.get_mut(prefix) {
                    *count -= 1;
                    if *count == 0 {
                        pending_alts.remove(prefix);
                    }
                }
            }
            // Merged right away rather than at round end: a composite later in
            // the same round has to see the alternatives resolved before it.
            alt_index.extend([(pg.name.clone(), anchors.clone())]);
            wave.push((pg, effective_refs, anchors));
            progress = true;
        }

        // Flattened in a block of its own: a layout borrows the cache, and the
        // insertion below needs it back.
        let flattened: Vec<Option<(PixelGrid, i32, i32)>> = {
            let layouts: Vec<CompositeLayout<'_>> = wave
                .iter()
                .map(|(pg, refs, _)| {
                    resolve_composite_layout(
                        pg.pixels.as_ref(),
                        refs,
                        &cache,
                        name_parts,
                        pg.scale,
                        false,
                    )
                })
                .collect();
            let keys: Vec<u64> = layouts
                .iter()
                .zip(&wave)
                .map(|(layout, (pg, ..))| layout.grid_cache_key(pg.pixels.as_ref(), pg.scale))
                .collect();
            // The memo is one `&mut`, so it is read here and written below,
            // with only the misses in between running on other threads.
            let mut grids: Vec<Option<PixelGrid>> = match grid_cache.as_deref_mut() {
                Some(gc) => keys.iter().map(|&key| gc.take_hit(key)).collect(),
                None => (0..wave.len()).map(|_| None).collect(),
            };
            let misses: Vec<usize> = (0..wave.len()).filter(|&i| grids[i].is_none()).collect();
            let built = crate::parallel::map_indexed(misses.len(), cancel, |at| {
                let i = misses[at];
                layouts[i].to_grid(wave[i].0.pixels.as_ref(), wave[i].0.scale)
            });
            for (at, grid) in built.into_iter().enumerate() {
                let i = misses[at];
                if let Some(gc) = grid_cache.as_deref_mut()
                    && let Some(grid) = &grid
                {
                    gc.record_miss(keys[i], grid);
                }
                grids[i] = grid;
            }
            grids
                .into_iter()
                .zip(&layouts)
                .map(|(grid, layout)| grid.map(|grid| (grid, layout.min_r, layout.min_c)))
                .collect()
        };

        for ((pg, effective_refs, anchors), flat) in
            std::mem::take(&mut wave).into_iter().zip(flattened)
        {
            // `None` only where a cancel stopped the round short of this glyph.
            // Left out rather than inserted blank: an absent composite is a
            // state the pipeline handles, an empty one is a glyph that draws
            // nothing.
            let Some((grid, min_r, min_c)) = flat else {
                continue;
            };
            // Moved, not cloned: nothing below reads the pending body again,
            // so carrying the declaration costs one allocation per composite.
            let inline_source = std::sync::Arc::new(InlineSource {
                refs: effective_refs,
                pixels: pg.pixels,
            });
            cache.insert(
                pg.name,
                ResolvedGlyph {
                    grid,
                    origin_row: min_r,
                    origin_col: min_c,
                    resolved_anchors: anchors,
                    declared_anchors: pg.points,
                    scale: pg.scale,
                    declared_box: pg.declared_box,
                    declared_origin: pg.declared_origin,
                    inline_source: Some(inline_source),
                },
            );
        }
        if cancel.is_cancelled() {
            return (cache, alt_index);
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

    // Alias names, added last. The expansion resolved every *reference* to its
    // canonical target, so nothing above needed them; what needs them is the
    // editor, which checks the names it finds in the text against this map and
    // would otherwise underline a perfectly good `ref` to an alias. Added
    // after `alt_index` is built, so an alias named `x:alt` never becomes an
    // alternative of `x`.
    for (name, target) in aliases.entries() {
        if cache.contains_key(name) {
            continue;
        }
        if let Some(resolved) = cache.get(target).cloned() {
            cache.insert(name.clone(), resolved);
        }
    }

    // Only here, on the one path that composed every glyph there is: an early
    // return above means the run was cancelled part-way, and what it never
    // reached must not be mistaken for what the source no longer has.
    if let Some(gc) = grid_cache {
        gc.evict_stale();
    }
    (cache, alt_index)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct GlyphComposite {
    pub width: u16,
    pub height: u16,
    pub own_offset_row: i16,
    pub own_offset_col: i16,
    pub layers: Vec<CompositeLayer>,
    /// Anchors the composite exposes through `inherit` refs rather than its
    /// own `anchor` lines, with the source ref's index. The subglyph palette
    /// lists them after the declared points, colored like the source
    /// subglyph; they have no document line, so they cannot be moved or
    /// renamed.
    pub inherited_anchors: Vec<(GlyphPoint, usize)>,
}

#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
impl GlyphComposite {
    pub fn any_layer_filled_at(&self, composite_row: i16, composite_col: i16) -> bool {
        let mut filled = false;
        for layer in &self.layers {
            let lr = composite_row - layer.offset_row;
            let lc = composite_col - layer.offset_col;
            if lr >= 0
                && lr < layer.grid.height as i16
                && lc >= 0
                && lc < layer.grid.width as i16
                && layer.grid.get(lr as u16, lc as u16).is_bitmap_filled()
            {
                filled = !layer.negated;
            }
        }
        filled
    }
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct CompositeLayer {
    pub ref_idx: usize,
    /// The resolved name (may differ from the source ref if an alternative was chosen).
    pub resolved_name: String,
    pub grid: PixelGrid,
    pub offset_row: i16,
    pub offset_col: i16,
    /// The ref placement in the owning glyph's logical coordinate space.
    /// This differs from `offset_*` when the resolved target has a negative
    /// logical origin.
    pub logical_offset_row: i16,
    pub logical_offset_col: i16,
    pub negated: bool,
    #[cfg(feature = "editor")]
    pub fill_color: Option<egui::Color32>,
}

/// A synthesized on-demand shape, memoized per name.
///
/// [`resolve_ref_name_for_view`] hands out borrows into a map it does not
/// own, so a glyph synthesized during the lookup needs a home that outlives the
/// call. One name is synthesized once and kept for the process: the set of
/// on-demand names a session mentions is small (a handful per font, plus the
/// intermediate names a ref typed character by character passes through), each
/// entry is one small grid, and the alternative — handing back an owned value —
/// would change every call site of the lookup and of the layout beneath it.
/// The grids themselves are already memoized in [`crate::on_demand`].
fn synthesized_on_demand(name: &str) -> Option<&'static ResolvedGlyph> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, &'static ResolvedGlyph>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(resolved) = cache.lock().unwrap().get(name) {
        return Some(resolved);
    }
    // Only the self-contained shapes: the color/mono pair of
    // [`crate::on_demand::detect_on_demand_glyph`] is synthesized from two
    // glyph bodies, which is the expansion's job and not a lookup's.
    let spec = match crate::on_demand::parse_on_demand_glyph(name)? {
        crate::on_demand::OnDemandGlyph::Shape(spec) => spec,
        crate::on_demand::OnDemandGlyph::ColorMono { .. } => return None,
    };
    let grid = crate::on_demand::make_on_demand_grid(&spec);
    let resolved: &'static ResolvedGlyph = Box::leak(Box::new(ResolvedGlyph {
        // An on-demand name states its own box, so the shape declares one the
        // way a header does — there is simply no header to read it off.
        declared_box: declared_box(Some(&grid), spec.scale),
        // An on-demand shape is its own box: the name states the size and
        // there is nowhere to write an origin, so the grid's corner is it.
        declared_origin: (0, 0),
        grid,
        origin_row: 0,
        origin_col: 0,
        resolved_anchors: Vec::new(),
        declared_anchors: Vec::new(),
        scale: spec.scale,
        inline_source: None,
    }));
    cache.lock().unwrap().insert(name.to_string(), resolved);
    Some(resolved)
}

/// Resolve a ref name against the resolved-glyph map: the name as written, then
/// with its name parts substituted, then the first expansion of the pattern it
/// may be.
pub fn resolve_ref_name_with_parts<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> Option<&'a ResolvedGlyph> {
    lookup_ref_name(name, named_glyphs, name_parts, false)
}

/// The same lookup, plus on-demand synthesis for a name the map does not have.
///
/// The expansion injects exactly this glyph
/// ([`crate::render::ttf_builder::expand_documents`]), so the map holds it after
/// the next resolve — but that resolve walks the whole font and waits behind the
/// editor's debounce, so until it lands an on-demand ref is the one kind that
/// draws nothing while every other ref draws the moment it is typed. Only the
/// *view* uses this: [`resolve_expansion`] is what produces those injected
/// entries and must keep composing from them alone, or a composite would resolve
/// from one of the two and the font build from the other.
///
/// A defined glyph still wins — the map is consulted first, in every form, just
/// as injection only fires for names the sources leave undefined.
#[cfg(any(feature = "editor", test))]
pub fn resolve_ref_name_for_view<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> Option<&'a ResolvedGlyph> {
    lookup_ref_name(name, named_glyphs, name_parts, true)
}

fn lookup_ref_name<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    synthesize: bool,
) -> Option<&'a ResolvedGlyph> {
    if let Some(resolved) = named_glyphs.get(name) {
        return Some(resolved);
    }
    let subst = substitute_name_parts(name, name_parts);
    if let Some(resolved) = named_glyphs.get(&subst) {
        return Some(resolved);
    }
    let expanded = parse_ref_pattern(&subst).map(|pattern| pattern.get(0));
    if let Some(expanded) = &expanded
        && let Some(resolved) = named_glyphs.get(expanded)
    {
        return Some(resolved);
    }
    if !synthesize {
        return None;
    }
    synthesized_on_demand(name)
        .or_else(|| synthesized_on_demand(&subst))
        .or_else(|| expanded.as_deref().and_then(synthesized_on_demand))
}

/// Check that a ref name resolves to valid glyphs. For pattern refs, ALL
/// expansions must exist; returns false if any expansion is missing.
#[cfg(any(feature = "editor", test))]
pub fn is_ref_valid(
    name: &str,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    if named_glyphs.contains_key(name) {
        return true;
    }
    if crate::on_demand::parse_on_demand_glyph(name).is_some() {
        return true;
    }
    // `ref ($0)` under an `exists` names what the search matched. Nothing on
    // the line says what that is, so it is valid here and answered where the
    // search is — [`crate::exists`]. The editor asks this while typing, before
    // any search has run, which is why the test is on the text.
    if crate::exists::mentions_capture(name) {
        return true;
    }
    let subst = substitute_name_parts(name, name_parts);
    if named_glyphs.contains_key(&subst) {
        return true;
    }
    if crate::on_demand::parse_on_demand_glyph(&subst).is_some() {
        return true;
    }
    if let Some(expanded) = parse_ref_pattern(&subst) {
        return expanded.iter().all(|n| {
            named_glyphs.contains_key(&n) || crate::on_demand::parse_on_demand_glyph(&n).is_some()
        });
    }
    false
}

/// Iterator over the base prefixes an alternative name registers under:
/// `foo:bar:baz` yields `foo:bar`, then `foo`.
pub(crate) fn alternative_prefixes(name: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(name.rfind(':').map(|p| &name[..p]), |prefix| {
        prefix.rfind(':').map(|p| &prefix[..p])
    })
}

/// Pre-built index mapping each base name to its sorted alternatives.
/// For glyph "foo:bar", entries are added under base "foo".
/// For "foo:bar:baz", entries are added under "foo" and "foo:bar".
#[derive(Clone, Debug, Default)]
pub struct AlternativesIndex {
    map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>>,
}

impl AlternativesIndex {
    pub fn build(named_glyphs: &HashMap<String, ResolvedGlyph>) -> Self {
        let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
        for (name, resolved) in named_glyphs {
            for prefix in alternative_prefixes(name) {
                map.entry(prefix.to_string())
                    .or_default()
                    .push((name.clone(), resolved.resolved_anchors.clone()));
            }
        }
        for alts in map.values_mut() {
            alts.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        Self { map }
    }

    /// Add entries for newly resolved glyphs, keeping each alternative list
    /// sorted (the same order [`Self::build`] produces). Lets the resolve
    /// fixpoint grow one index instead of rebuilding it from the whole cache
    /// every round.
    fn extend(&mut self, entries: impl IntoIterator<Item = (String, Vec<GlyphPoint>)>) {
        for (name, anchors) in entries {
            for prefix in alternative_prefixes(&name) {
                let alts = self.map.entry(prefix.to_string()).or_default();
                match alts.binary_search_by(|(a, _)| a.as_str().cmp(&name)) {
                    // Same glyph resolved again (cache overwrite): keep the
                    // list free of duplicate names, like a full rebuild would.
                    Ok(pos) => alts[pos].1 = anchors.clone(),
                    Err(pos) => alts.insert(pos, (name.clone(), anchors.clone())),
                }
            }
        }
    }

    pub fn get(&self, base_name: &str) -> &[(String, Vec<GlyphPoint>)] {
        self.map.get(base_name).map_or(&[], |v| v.as_slice())
    }
}

#[cfg(test)]
#[path = "ref_composite_tests.rs"]
mod tests;
