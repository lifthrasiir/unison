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
//! origin. `left`/`top` are for shifting a glyph that has no such ref; they are
//! not for undoing a negative offset.
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
    pub declared_box: Option<(u16, u16)>,
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

/// The declared box behind a grid: the `W H` the header wrote, before `scale`
/// multiplied it. The single place that undoes that multiplication, so the
/// resolution cache and [`crate::compose`] cannot disagree about what a glyph
/// declares.
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

/// A problem found while deriving a composite's offsets and anchors. Carried
/// as data rather than a formatted string so each consumer can attach the
/// glyph name and provenance it knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeriveIssue {
    /// Two surviving anchors with the same name would both be exposed. The
    /// composite exposes neither: a digraph must not pick one side's
    /// attachment point silently. Declare the anchor explicitly instead.
    DuplicateExposed { position: String },
    /// A `-` anchor found more than one size-matching `+` anchor to attach
    /// to. Nothing is attached or consumed.
    AmbiguousAttachment { position: String, ref_name: String },
    /// A `-` anchor found a same-name `+` of a different size (`(w, h)` in
    /// cells) and therefore attached to nothing. A near-miss like this almost
    /// always means the wrong `:narrow`/`:wide` variant — a `-` with no
    /// same-name `+` at all is ordinary forwarding and stays quiet.
    ///
    /// Once reported as a warning, on the reading that the composite still
    /// resolves around it. It does not: the mark sits at the pen instead of
    /// over its base, which is not a glyph anyone meant to ship, and a
    /// warning left the build shipping it anyway.
    SizeMismatchedAttachment {
        position: String,
        ref_name: String,
        minus: (u16, u16),
        plus: (u16, u16),
    },
}

impl DeriveIssue {
    pub(crate) fn message(&self, glyph: &str) -> String {
        match self {
            DeriveIssue::DuplicateExposed { position } => format!(
                "glyph '{glyph}' would expose anchor '{position}' from more than one \
                 source; it exposes neither — declare the anchor explicitly",
            ),
            DeriveIssue::AmbiguousAttachment { position, ref_name } => format!(
                "glyph '{glyph}': ref '{ref_name}' finds more than one '{}{}' anchor \
                 to attach its '{position}' to; nothing is attached",
                "+",
                position.strip_prefix('-').unwrap_or(position),
            ),
            DeriveIssue::SizeMismatchedAttachment {
                position,
                ref_name,
                minus,
                plus,
            } => {
                format!(
                    "glyph '{glyph}': ref '{ref_name}' has '{position}' ({}x{}) matching \
                     a '+{}' only by name ({}x{}); not attached — check the other variants",
                    minus.0,
                    minus.1,
                    position.strip_prefix('-').unwrap_or(position),
                    plus.0,
                    plus.1,
                )
            }
        }
    }
}

/// An anchor in the derivation pool: the point (already translated into the
/// composite's coordinates) and where it came from — `None` for the
/// composite's own declared anchors, `Some(i)` for one published by ref `i`.
type PoolAnchor = (GlyphPoint, Option<usize>);

/// Derive effective ref offsets and the anchors exposed by the resulting
/// composite without changing the source refs.  A target's `-name` anchors
/// consume matching `+name` anchors that are already available, then the
/// target's `+name` anchors are published for following refs.  A target's
/// several `-name` anchors are alternative ways in, so only the first with a
/// candidate is used; see the module docs for what the rest take with them.
///
/// What the composite *exposes* is opt-in: its own declared anchors, plus the
/// surviving anchors (published `+`, unconsumed `-`) of refs marked
/// `inherit`.  Attachment between sibling refs works regardless of the flag.
/// If two survivors of the exposed set share a name, the composite exposes
/// neither and a [`DeriveIssue::DuplicateExposed`] is reported; likewise a
/// `-` anchor with more than one size-matching `+` candidate attaches to
/// nothing and reports [`DeriveIssue::AmbiguousAttachment`].
///
/// Ref order does not matter: when a target carries `-name` and some other
/// still-unresolved ref could yet publish `+name`, resolution is deferred
/// until it does.  A minus anchor no remaining ref can satisfy does *not*
/// defer — deferral would let explicit-offset sibling refs commit first and
/// miss their consumption.  Refs that remain unresolved after the fixpoint
/// fall back to `(0, 0)`.
///
/// `lookup_alternatives` returns sorted alternative glyph names for a base
/// name (e.g. for "foo" it returns ["foo:bar", "foo:baz"]).  When the
/// primary ref's anchors don't size-match, alternatives are tried in order.
///
/// `lookup_declared_anchors` returns a ref target's own declared anchors
/// (not forwarded from its refs).  This enables look-ahead alternative
/// selection: if a later ref would consume `+X` via `-X` and the current
/// ref's declared anchors lack `+X` (it is only forwarded from sub-refs),
/// an alternative that declares `+X` directly is preferred.
pub(crate) fn derive_ref_offsets_with(
    declared_anchors: &[GlyphPoint],
    refs: &[GlyphRef],
    lookup_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    lookup_alternatives: impl FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
    lookup_declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
) -> (Vec<GlyphRef>, Vec<PoolAnchor>, Vec<DeriveIssue>) {
    let outcome = derive_ref_offsets_detailed(
        declared_anchors,
        refs,
        lookup_anchors,
        lookup_alternatives,
        lookup_declared_anchors,
    );
    (outcome.effective, outcome.exposed, outcome.issues)
}

/// What [`derive_ref_offsets_with`] worked out, plus *how* each ref got its
/// placement.
pub(crate) struct DeriveOutcome {
    pub effective: Vec<GlyphRef>,
    pub exposed: Vec<PoolAnchor>,
    pub issues: Vec<DeriveIssue>,
    /// Per ref: the placement came from an anchor match rather than from the
    /// line. A ref written with an offset is never one of these, and neither
    /// is an offset-less ref that found no anchor and fell back to (0, 0) —
    /// the distinction the two cannot express themselves, and the one a
    /// glyph resize needs: an anchored ref follows its target's anchors on
    /// its own, so its line must be left alone. See
    /// [`crate::editor::glyph_resize`].
    // Only the editor resizes glyphs, and the headless build has no other
    // reader; the flags are still computed there because they fall out of the
    // fixpoint that has to run anyway.
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    pub anchor_placed: Vec<bool>,
}

/// [`derive_ref_offsets_with`] with the anchor-placement flags kept.
pub(crate) fn derive_ref_offsets_detailed(
    declared_anchors: &[GlyphPoint],
    refs: &[GlyphRef],
    mut lookup_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut lookup_alternatives: impl FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
    mut lookup_declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
) -> DeriveOutcome {
    let mut issues: Vec<DeriveIssue> = Vec::new();
    let mut survived_minus: Vec<PoolAnchor> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .map(|p| (p.clone(), None))
        .collect();
    let mut available_plus: Vec<PoolAnchor> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .map(|p| (p.clone(), None))
        .collect();

    let target_anchors_list: Vec<Option<Vec<GlyphPoint>>> =
        refs.iter().map(|gref| lookup_anchors(&gref.name)).collect();

    let target_declared_anchors_list: Vec<Option<Vec<GlyphPoint>>> = refs
        .iter()
        .map(|gref| lookup_declared_anchors(&gref.name))
        .collect();

    let alternatives_list: Vec<Vec<(String, Vec<GlyphPoint>)>> = refs
        .iter()
        .map(|gref| {
            if gref.offset.is_some() {
                Vec::new()
            } else {
                lookup_alternatives(&gref.name)
            }
        })
        .collect();

    let n = refs.len();
    let mut effective_refs: Vec<Option<GlyphRef>> = vec![None; n];
    let mut anchor_placed = vec![false; n];

    loop {
        let mut progress = false;
        for (i, gref) in refs.iter().enumerate() {
            if effective_refs[i].is_some() {
                continue;
            }

            let Some(ref target_anchors) = target_anchors_list[i] else {
                effective_refs[i] = Some(gref.clone());
                progress = true;
                continue;
            };

            if let Some(offset) = gref.offset {
                commit_ref(
                    gref,
                    i,
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            match try_match_minus_plus(target_anchors, &available_plus) {
                MatchOutcome::Unique(offset) => {
                    anchor_placed[i] = true;
                    commit_ref(
                        gref,
                        i,
                        offset,
                        target_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    progress = true;
                    continue;
                }
                MatchOutcome::Ambiguous => {
                    // The attachment is ill-defined; commit unattached and be
                    // loud (commit_ref reports it) rather than silently
                    // swapping in an alternative or picking one candidate.
                    commit_ref(
                        gref,
                        i,
                        (0, 0),
                        target_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    progress = true;
                    continue;
                }
                MatchOutcome::NoMatch => {}
            }

            // Try alternatives when primary doesn't size-match.
            let mut alt_matched = false;
            for (alt_name, alt_anchors) in &alternatives_list[i] {
                if let MatchOutcome::Unique(offset) =
                    try_match_minus_plus(alt_anchors, &available_plus)
                {
                    let alt_gref = GlyphRef {
                        raw_name: None,
                        comment: None,
                        name: alt_name.clone(),
                        offset: None,
                        negated: gref.negated,
                        inherit: gref.inherit,
                        if_exists: gref.if_exists,
                        fill: gref.fill.clone(),
                        visibility: gref.visibility,
                    };
                    anchor_placed[i] = true;
                    commit_ref(
                        &alt_gref,
                        i,
                        offset,
                        alt_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    alt_matched = true;
                    progress = true;
                    break;
                }
            }
            if alt_matched {
                continue;
            }

            // Defer only while some other still-unresolved ref could publish a
            // `+` this candidate's `-` might match. A minus nothing remaining
            // can satisfy (a base's own `-center`, say) must not wait: waiting
            // would let explicit-offset sibling refs commit first and miss the
            // consumption of this ref's `+` anchors.
            let wanted: Vec<&str> = target_anchors
                .iter()
                .chain(alternatives_list[i].iter().flat_map(|(_, a)| a.iter()))
                .filter_map(|p| p.position.strip_prefix('-'))
                .collect();
            let could_match_later = !wanted.is_empty()
                && (0..n).any(|j| {
                    j != i
                        && effective_refs[j].is_none()
                        && target_anchors_list[j]
                            .iter()
                            .flatten()
                            .chain(alternatives_list[j].iter().flat_map(|(_, a)| a.iter()))
                            .filter_map(|p| p.position.strip_prefix('+'))
                            .any(|base| wanted.contains(&base))
                });
            if could_match_later {
                continue;
            }

            // Look-ahead substitution: if this ref publishes +anchor
            // that a subsequent unresolved ref would consume via -anchor,
            // prefer an alternative that provides +anchor directly.
            let alt_found = try_lookahead_alt(
                i,
                n,
                target_anchors,
                target_declared_anchors_list[i].as_deref(),
                &available_plus,
                &alternatives_list[i],
                &effective_refs,
                &target_anchors_list,
            );
            if let Some((alt_name, alt_anchors)) = alt_found {
                let alt_gref = GlyphRef {
                    raw_name: None,
                    comment: None,
                    name: alt_name,
                    offset: None,
                    negated: gref.negated,
                    inherit: gref.inherit,
                    if_exists: gref.if_exists,
                    fill: gref.fill.clone(),
                    visibility: gref.visibility,
                };
                commit_ref(
                    &alt_gref,
                    i,
                    (0, 0),
                    alt_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
            } else {
                commit_ref(
                    gref,
                    i,
                    (0, 0),
                    target_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
            }
            progress = true;
        }
        if !progress {
            break;
        }
    }

    for (i, gref) in refs.iter().enumerate() {
        if effective_refs[i].is_some() {
            continue;
        }
        let target_anchors = target_anchors_list[i].as_deref().unwrap_or(&[]);
        let (resolved_name, offset, used_anchors) = if let Some(offset) = gref.offset {
            (gref.name.clone(), offset, target_anchors)
        } else {
            match try_match_minus_plus(target_anchors, &available_plus) {
                MatchOutcome::Unique(offset) => {
                    anchor_placed[i] = true;
                    (gref.name.clone(), offset, target_anchors)
                }
                // Ambiguity is reported by commit_ref below; commit unattached.
                MatchOutcome::Ambiguous => (gref.name.clone(), (0, 0), target_anchors),
                MatchOutcome::NoMatch => {
                    let mut found = None;
                    for (alt_name, alt_anchors) in &alternatives_list[i] {
                        if let MatchOutcome::Unique(offset) =
                            try_match_minus_plus(alt_anchors, &available_plus)
                        {
                            anchor_placed[i] = true;
                            found = Some((alt_name.clone(), offset, alt_anchors.as_slice()));
                            break;
                        }
                    }
                    if found.is_none()
                        && let Some((alt_name, alt_anchors)) = try_lookahead_alt(
                            i,
                            n,
                            target_anchors,
                            target_declared_anchors_list[i].as_deref(),
                            &available_plus,
                            &alternatives_list[i],
                            &effective_refs,
                            &target_anchors_list,
                        )
                    {
                        found = Some((alt_name.clone(), (0, 0), alt_anchors));
                    }
                    found.unwrap_or_else(|| (gref.name.clone(), (0, 0), target_anchors))
                }
            }
        };
        let resolved_gref = GlyphRef {
            raw_name: None,
            comment: None,
            name: resolved_name,
            offset: gref.offset,
            negated: gref.negated,
            inherit: gref.inherit,
            if_exists: gref.if_exists,
            fill: gref.fill.clone(),
            visibility: gref.visibility,
        };
        commit_ref(
            &resolved_gref,
            i,
            offset,
            used_anchors,
            &mut available_plus,
            &mut survived_minus,
            &mut issues,
            &mut effective_refs[i],
        );
    }

    let effective_refs: Vec<GlyphRef> = effective_refs.into_iter().map(Option::unwrap).collect();

    // Exposure is opt-in: declared anchors always pass, a ref's survivors only
    // through its `inherit` flag. Survivors sharing a name are then dropped
    // together — the composite must not pick one of them silently. Each
    // exposed anchor keeps its source (`None` = declared, `Some(i)` = ref
    // `i`), which is how the palette colors an inherited anchor like the
    // subglyph it came from.
    let mut exposed: Vec<PoolAnchor> = survived_minus
        .into_iter()
        .chain(available_plus)
        .filter(|(_, source)| source.is_none_or(|i| refs[i].inherit))
        .collect();
    let mut duplicated: Vec<String> = Vec::new();
    for (i, (p, _)) in exposed.iter().enumerate() {
        if exposed[..i].iter().any(|(o, _)| o.position == p.position)
            && !duplicated.contains(&p.position)
        {
            duplicated.push(p.position.clone());
        }
    }
    for position in duplicated {
        exposed.retain(|(p, _)| p.position != position);
        issues.push(DeriveIssue::DuplicateExposed { position });
    }

    DeriveOutcome {
        effective: effective_refs,
        exposed,
        issues,
        anchor_placed,
    }
}

/// Look-ahead alternative selection: when an unresolved sibling would consume
/// `-anchor` and an alternative of the ref at index `i` declares the matching
/// `+anchor` that the ref itself does *not* declare, prefer that alternative.
///
/// This handles cases like `i-lower` (whose `+above` lives on
/// `i-lower:dotless`) followed by `dia-above` (which needs `-above`):
/// `i-lower:dotless` should be used because it is the correct visual form
/// when the above-anchor is consumed (the dot would conflict).  But when
/// followed by `dia-below`, no substitution occurs because `i-lower` has
/// `+below` as its own declared anchor.
///
/// The question is asked of the *alternative*, never of what the primary
/// exposes: `inherit` decides what a glyph offers, not which form of it gets
/// picked. `ttf_builder/gpos.rs`'s base-alternative selection reads
/// `declared_anchors` plus the alternative index for the same reason, and the
/// two must agree — otherwise a `map generate` composite and its typed
/// decomposition render differently, which is what made every generated
/// `ï í ì ī ǐ î ĭ ĩ` keep its dot once anchor inheritance became explicit.
///
/// A second, size-driven trigger substitutes a *publisher* whose `+X`
/// name-matches an unresolved sibling's `-X` but size-mismatches it, when an
/// alternative's `+X` fits — the consumer side cannot adapt there, since its
/// own alternatives were already tried and rejected. Both triggers scan every
/// unresolved sibling, not just the following ones: a deferred consumer
/// *earlier* in the ref list is still waiting on this publisher.
#[expect(clippy::too_many_arguments)]
fn try_lookahead_alt<'a>(
    i: usize,
    n: usize,
    target_anchors: &[GlyphPoint],
    target_declared_anchors: Option<&[GlyphPoint]>,
    available_plus: &[PoolAnchor],
    alternatives: &'a [(String, Vec<GlyphPoint>)],
    effective_refs: &[Option<GlyphRef>],
    target_anchors_list: &[Option<Vec<GlyphPoint>>],
) -> Option<(String, &'a [GlyphPoint])> {
    let unresolved = |j: usize| j != i && effective_refs[j].is_none();

    if let Some(declared) = target_declared_anchors {
        for (alt_name, alt_anchors) in alternatives {
            for plus in alt_anchors.iter().filter(|p| p.position.starts_with('+')) {
                // The primary declares it, so the primary *is* the right form:
                // `i-lower` keeps its dot under `dia-below`.
                if declared.iter().any(|o| o.position == plus.position) {
                    continue;
                }
                // Already published — by the composite itself or by a sibling
                // that committed earlier — so there is nothing to switch for.
                if available_plus
                    .iter()
                    .any(|(a, _)| a.position == plus.position)
                {
                    continue;
                }
                let Some(base) = plus.position.strip_prefix('+') else {
                    continue;
                };
                let minus_name = format!("-{base}");
                let needed = (0..n).any(|j| {
                    unresolved(j)
                        && target_anchors_list[j]
                            .as_deref()
                            .unwrap_or(&[])
                            .iter()
                            .any(|p| p.position == minus_name)
                });
                if needed {
                    return Some((alt_name.clone(), alt_anchors.as_slice()));
                }
            }
        }
    }

    // Size-aware publisher substitution: a `+X` this ref would publish
    // (declared or forwarded) name-matches an unresolved sibling's `-X` but
    // size-mismatches it, and an alternative's `+X` fits that consumer. The
    // consumer cannot adapt — its own alternatives were already tried — so
    // the publisher must: `enclosing-circle:alt` exists exactly for the
    // descenders whose `-center` is one cell, not two.
    for (alt_name, alt_anchors) in alternatives {
        for plus in target_anchors
            .iter()
            .filter(|p| p.position.starts_with('+'))
        {
            let Some(base) = plus.position.strip_prefix('+') else {
                continue;
            };
            let minus_name = format!("-{base}");
            for j in (0..n).filter(|&j| unresolved(j)) {
                for minus in target_anchors_list[j]
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|p| p.position == minus_name)
                {
                    // The primary (or something already published) serves
                    // this consumer: nothing to fix.
                    if plus.size_matches(minus)
                        || available_plus
                            .iter()
                            .any(|(p, _)| p.position == plus.position && p.size_matches(minus))
                    {
                        continue;
                    }
                    if alt_anchors
                        .iter()
                        .any(|a| a.position == plus.position && a.size_matches(minus))
                    {
                        return Some((alt_name.clone(), alt_anchors.as_slice()));
                    }
                }
            }
        }
    }
    None
}

/// What matching a target's `-` anchors against the pool produced.
enum MatchOutcome {
    NoMatch,
    Unique((i16, i16)),
    /// The first `-` anchor with any candidate had more than one — the
    /// attachment is ill-defined, and no other anchor is tried instead.
    Ambiguous,
}

fn try_match_minus_plus(
    target_anchors: &[GlyphPoint],
    available_plus: &[PoolAnchor],
) -> MatchOutcome {
    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        let mut candidates = available_plus
            .iter()
            .filter(|(p, _)| p.position.strip_prefix('+') == Some(base) && p.size_matches(minus));
        let Some((plus, _)) = candidates.next() else {
            continue;
        };
        if candidates.next().is_some() {
            return MatchOutcome::Ambiguous;
        }
        return MatchOutcome::Unique((
            saturating_i16(plus.col as i32 - minus.col as i32),
            saturating_i16(plus.row as i32 - minus.row as i32),
        ));
    }
    MatchOutcome::NoMatch
}

fn translate_point(p: &GlyphPoint, off_col: i16, off_row: i16) -> GlyphPoint {
    GlyphPoint {
        comment: None,
        position: p.position.clone(),
        col: saturating_i16(p.col as i32 + off_col as i32),
        row: saturating_i16(p.row as i32 + off_row as i32),
        col_end: saturating_i16(p.col_end as i32 + off_col as i32),
        row_end: saturating_i16(p.row_end as i32 + off_row as i32),
    }
}

#[expect(clippy::too_many_arguments)]
fn commit_ref(
    gref: &GlyphRef,
    ref_idx: usize,
    offset: (i16, i16),
    target_anchors: &[GlyphPoint],
    available_plus: &mut Vec<PoolAnchor>,
    survived_minus: &mut Vec<PoolAnchor>,
    issues: &mut Vec<DeriveIssue>,
    out: &mut Option<GlyphRef>,
) {
    let effective = GlyphRef {
        raw_name: None,
        comment: None,
        name: gref.name.clone(),
        offset: Some(offset),
        negated: gref.negated,
        inherit: gref.inherit,
        if_exists: gref.if_exists,
        fill: gref.fill.clone(),
        visibility: gref.visibility,
    };
    let off_col = effective.col();
    let off_row = effective.row();

    // Consume before publishing. In particular, a component carrying
    // both `-join` and `+join` must publish its outgoing anchor rather
    // than immediately deleting it again. Consumption needs a *unique*
    // size-matching `+`: more than one means the attachment is ill-defined,
    // so nothing is consumed (and nothing survives) — loudly.
    //
    // Several `-` anchors are *alternatives*, not several attachments: one
    // mark that can adjoin to more than one anchor system (`gr-psili` joins
    // either a Greek `+gr-above` or a plain `+above`). The first one with a
    // candidate decides — the very order `try_match_minus_plus` derived the
    // offset from — and the rest retire without a trace: no survivor, and no
    // near-miss report against a `+` this ref was never going to use. That
    // silence is load-bearing now that a near-miss drops the glyph.
    let minus_anchors: Vec<&GlyphPoint> = target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .collect();
    let mut joined: Option<&str> = None;
    for minus in &minus_anchors {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        let matching: Vec<usize> = available_plus
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| {
                p.position.strip_prefix('+') == Some(base) && p.size_matches(minus)
            })
            .map(|(i, _)| i)
            .collect();
        if matching.is_empty() {
            continue;
        }
        joined = Some(base);
        if matching.len() == 1 {
            available_plus.remove(matching[0]);
        } else {
            issues.push(DeriveIssue::AmbiguousAttachment {
                position: minus.position.clone(),
                ref_name: gref.name.clone(),
            });
        }
        break;
    }
    if joined.is_none() {
        for minus in &minus_anchors {
            let Some(base) = minus.position.strip_prefix('-') else {
                continue;
            };
            // A same-name `+` of the wrong size is a near-miss worth
            // flagging; no same-name `+` at all is plain forwarding.
            if let Some((near, _)) = available_plus
                .iter()
                .find(|(p, _)| p.position.strip_prefix('+') == Some(base))
            {
                issues.push(DeriveIssue::SizeMismatchedAttachment {
                    position: minus.position.clone(),
                    ref_name: gref.name.clone(),
                    minus: (minus.width(), minus.height()),
                    plus: (near.width(), near.height()),
                });
            }
            survived_minus.push((translate_point(minus, off_col, off_row), Some(ref_idx)));
        }
    }
    for plus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
    {
        // A `+` whose `-` partner retired goes with it: having joined one
        // system, the mark offers the next mark only that same system. A `+`
        // with no `-` partner at all is a base's hosting point, untouched —
        // `s-upper` keeps offering `+above` after `+below` was taken.
        if let Some(joined) = joined
            && let Some(base) = plus.position.strip_prefix('+')
            && base != joined
            && minus_anchors
                .iter()
                .any(|m| m.position.strip_prefix('-') == Some(base))
        {
            continue;
        }
        available_plus.push((translate_point(plus, off_col, off_row), Some(ref_idx)));
    }

    *out = Some(effective);
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
    }

    let mut pending: Vec<Pending> = Vec::new();
    // Mirrors `pending` names for O(1) duplicate checks; a linear scan here
    // is quadratic over the whole font (~18k glyphs).
    let mut pending_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    let aliases = expansion.aliases;

    // The expansion is consumed, not borrowed: it already owns a full copy of
    // every glyph body, and cloning it a second time into `pending` cost more
    // than sharing the expansion saved.
    for (i, expanded) in expansion.items.into_iter().enumerate() {
        if i.is_multiple_of(crate::render::glyph_cache::CANCEL_STRIDE) && cancel.is_cancelled() {
            let alt_index = AlternativesIndex::build(&cache);
            return (cache, alt_index);
        }
        let DocumentItem::Glyph {
            name: GlyphName(key),
            body,
        } = expanded.item
        else {
            continue;
        };
        // First definition wins, matching the font build.
        if cache.contains_key(&key) || pending_names.contains(&key) {
            continue;
        }
        if body.refs.is_empty() {
            let declared_box = declared_box(body.pixels.as_ref(), body.scale);
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
                    inline_source: None,
                },
            );
        } else {
            pending_names.insert(key.clone());
            pending.push(Pending {
                name: key,
                declared_box: declared_box(body.pixels.as_ref(), body.scale),
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
            let (effective_refs, exposed, _issues) = derive_ref_offsets_with(
                &pg.points,
                &pg.refs,
                |name| {
                    resolve_ref_name_with_parts(name, &cache, name_parts)
                        .map(|resolved| resolved.resolved_anchors.clone())
                },
                |name| alt_index.get(name).to_vec(),
                |name| {
                    resolve_ref_name_with_parts(name, &cache, name_parts)
                        .map(|resolved| resolved.declared_anchors.clone())
                },
            );
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
                && layer.grid.get(lr as u16, lc as u16).is_filled()
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

fn ref_effective_offset_scaled(
    gref: &GlyphRef,
    resolved: &ResolvedGlyph,
    parent_scale: u8,
) -> (i32, i32) {
    let ps = parent_scale as i32;
    let rs = resolved.scale.max(1) as i32;
    (
        gref.row() as i32 + resolved.origin_row * ps / rs,
        gref.col() as i32 + resolved.origin_col * ps / rs,
    )
}

fn ref_grid_scaled(grid: &PixelGrid, ref_scale: u8, parent_scale: u8) -> PixelGrid {
    if ref_scale == parent_scale {
        grid.clone()
    } else {
        grid.rescale(ref_scale.max(1), parent_scale.max(1))
    }
}

/// One ref resolved to its target and raster placement.
struct ResolvedLayer<'a> {
    #[cfg_attr(not(any(feature = "editor", test)), expect(dead_code))]
    ref_idx: usize,
    gref: &'a GlyphRef,
    resolved: &'a ResolvedGlyph,
    raster_row: i32,
    raster_col: i32,
}

/// All refs of a composite resolved once, with the resulting bounding box.
///
/// This is the single resolution pass shared by the build cache
/// (`resolve_expansion` → [`CompositeLayout::to_grid`]), the editor's live
/// composite ([`compute_composite`]) and bounds queries
/// ([`composite_bounds`]).  They must agree on how every ref resolves —
/// a historical divergence here made the editor and the flattened font
/// disagree on pattern refs — so none of them resolves refs on its own.
struct CompositeLayout<'a> {
    layers: Vec<ResolvedLayer<'a>>,
    min_r: i32,
    min_c: i32,
    max_r: i32,
    max_c: i32,
}

fn resolve_composite_layout<'a>(
    own_pixels: Option<&PixelGrid>,
    refs: &'a [GlyphRef],
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    parent_scale: u8,
    // `synthesize`: make up an on-demand target the map does not have yet. The
    // view sets it; `resolve_expansion`, which is what puts those targets in
    // the map, does not.
    synthesize: bool,
) -> CompositeLayout<'a> {
    let mut min_r: i32 = 0;
    let mut min_c: i32 = 0;
    let mut max_r: i32 = 0;
    let mut max_c: i32 = 0;

    if let Some(grid) = own_pixels {
        max_r = grid.height as i32;
        max_c = grid.width as i32;
    }

    let ps = parent_scale as i32;
    let mut layers = Vec::new();
    for (ref_idx, gref) in refs.iter().enumerate() {
        let Some(resolved) = lookup_ref_name(&gref.name, named_glyphs, name_parts, synthesize)
        else {
            continue;
        };
        let rs = resolved.scale.max(1) as i32;
        let (raster_row, raster_col) = ref_effective_offset_scaled(gref, resolved, parent_scale);
        if resolved.grid.width != 0 && resolved.grid.height != 0 {
            let scaled_h = resolved.grid.height as i32 * ps / rs;
            let scaled_w = resolved.grid.width as i32 * ps / rs;
            min_r = min_r.min(raster_row);
            min_c = min_c.min(raster_col);
            max_r = max_r.max(raster_row + scaled_h);
            max_c = max_c.max(raster_col + scaled_w);
        }
        layers.push(ResolvedLayer {
            ref_idx,
            gref,
            resolved,
            raster_row,
            raster_col,
        });
    }

    CompositeLayout {
        layers,
        min_r,
        min_c,
        max_r,
        max_c,
    }
}

impl CompositeLayout<'_> {
    #[cfg(any(feature = "editor", test))]
    fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.min_r, self.min_c, self.max_r, self.max_c)
    }

    /// Everything [`Self::to_grid`] reads, as one hash: the box it rasterizes
    /// into, the glyph's own pixels, and every layer's target grid, scale and
    /// raster placement. Nothing about *how* the layers were chosen enters —
    /// they are already the choice — so a composite keyed on this recomposes
    /// only when the ink going into it differs.
    fn grid_cache_key(&self, own_pixels: Option<&PixelGrid>, parent_scale: u8) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        parent_scale.hash(&mut hasher);
        (self.min_r, self.min_c, self.max_r, self.max_c).hash(&mut hasher);
        match own_pixels {
            Some(grid) => {
                1u8.hash(&mut hasher);
                hash_grid_into(grid, &mut hasher);
            }
            None => 0u8.hash(&mut hasher),
        }
        self.layers.len().hash(&mut hasher);
        for layer in &self.layers {
            hash_grid_into(&layer.resolved.grid, &mut hasher);
            layer.resolved.scale.hash(&mut hasher);
            (layer.raster_row, layer.raster_col).hash(&mut hasher);
            layer.gref.negated.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Flatten into one grid: own pixels plus each layer blitted at its
    /// raster placement (negated refs subtract).
    fn to_grid(&self, own_pixels: Option<&PixelGrid>, parent_scale: u8) -> PixelGrid {
        let width = raster_dimension(self.min_c, self.max_c);
        let height = raster_dimension(self.min_r, self.max_r);
        let mut result = PixelGrid::new(width, height);

        if let Some(grid) = own_pixels {
            result.blit(grid, -self.min_r, -self.min_c, false);
        }

        for layer in &self.layers {
            let scaled = ref_grid_scaled(&layer.resolved.grid, layer.resolved.scale, parent_scale);
            result.blit(
                &scaled,
                layer.raster_row - self.min_r,
                layer.raster_col - self.min_c,
                layer.gref.negated,
            );
        }

        result
    }
}

/// Bounding box (min_row, min_col, max_row, max_col) of a composite made of
/// `own_pixels` (if any) plus `refs`, each resolved against `named_glyphs`
/// via [`resolve_ref_name_for_view`] (which falls back to pattern expansion
/// when a ref name isn't a direct cache key).
#[cfg(feature = "editor")]
pub(crate) fn composite_bounds(
    own_pixels: Option<&PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    parent_scale: u8,
) -> (i32, i32, i32, i32) {
    resolve_composite_layout(
        own_pixels,
        refs,
        named_glyphs,
        name_parts,
        parent_scale,
        true,
    )
    .bounds()
}

#[cfg(feature = "editor")]
fn resolve_fill_display_color(
    fill: &crate::document::RefFill,
    aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<egui::Color32> {
    if fill.color == "fg" {
        return None;
    }
    if fill.color.starts_with('#') {
        let rgba = crate::render::ttf_builder::parse_hex_color(&fill.color)?;
        return Some(egui::Color32::from_rgba_unmultiplied(
            rgba.r, rgba.g, rgba.b, rgba.a,
        ));
    }
    let (rgba, _) = aliases.get(&fill.color)?;
    Some(egui::Color32::from_rgba_unmultiplied(
        rgba.r, rgba.g, rgba.b, rgba.a,
    ))
}

/// The `ref`s a body's IDC lines stand for, as the *editor* sees them.
///
/// The same derivation the build runs in `ttf_builder::expand`, reading the
/// same declared boxes ([`ResolvedGlyph::declared_box`]) — the live view of a
/// glyph being edited must place its parts exactly where the font will.
/// Diagnostics are dropped here: `issues.rs` reports them once, from the build
/// side, and the view's job is only to draw.
#[cfg(any(feature = "editor", test))]
fn compose_refs_for_view(
    body: &GlyphBody,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> Vec<GlyphRef> {
    if body.compose.is_empty() {
        return Vec::new();
    }
    let dims = |name: &str| match resolve_ref_name_for_view(name, named_glyphs, name_parts) {
        None => crate::compose::PartDims::Unknown,
        Some(resolved) => match resolved.declared_box {
            Some((w, h)) => crate::compose::PartDims::Size(w, h),
            None => crate::compose::PartDims::Undeclared,
        },
    };
    let parent = declared_box(body.pixels.as_ref(), body.scale);
    body.compose
        .iter()
        .flat_map(|c| {
            // No clearance rule: the check reports, and the view only draws.
            crate::compose::expand_compose("", parent, body.scale, c, &dims, None)
                .0
                .into_iter()
        })
        .collect()
}

#[cfg(any(feature = "editor", test))]
pub fn compute_composite(
    body: &GlyphBody,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &AlternativesIndex,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<GlyphComposite> {
    // Derived refs go in front, so the stack is the one the font builds; the
    // layers keep pointing at *source* ref lines, so a derived layer takes an
    // index past their end and the editor's line lookups simply miss it rather
    // than landing on the wrong line.
    let derived = compose_refs_for_view(body, named_glyphs, name_parts);
    if body.refs.is_empty() && derived.is_empty() {
        return None;
    }
    let all_refs: Vec<GlyphRef> = derived.iter().chain(body.refs.iter()).cloned().collect();
    let source_idx = |i: usize| {
        i.checked_sub(derived.len())
            .unwrap_or_else(|| body.refs.len() + i)
    };

    let (effective_refs, exposed, _) = derive_ref_offsets_with(
        &body.points,
        &all_refs,
        |name| {
            resolve_ref_name_for_view(name, named_glyphs, name_parts)
                .map(|resolved| resolved.resolved_anchors.clone())
        },
        |name| alt_index.get(name).to_vec(),
        |name| {
            resolve_ref_name_for_view(name, named_glyphs, name_parts)
                .map(|resolved| resolved.declared_anchors.clone())
        },
    );
    let inherited_anchors: Vec<(GlyphPoint, usize)> = exposed
        .into_iter()
        .filter_map(|(p, source)| source.map(|ref_idx| (p, source_idx(ref_idx))))
        .collect();

    let layout = resolve_composite_layout(
        body.pixels.as_ref(),
        &effective_refs,
        named_glyphs,
        name_parts,
        body.scale,
        true,
    );
    let (min_r, min_c, max_r, max_c) = layout.bounds();

    let width = raster_dimension(min_c, max_c).max(1);
    let height = raster_dimension(min_r, max_r).max(1);

    let mut layers = Vec::new();
    for layer in &layout.layers {
        let scaled_grid = ref_grid_scaled(&layer.resolved.grid, layer.resolved.scale, body.scale);
        let orig_ref = &all_refs[layer.ref_idx];
        #[cfg(feature = "editor")]
        let fill_color = orig_ref
            .fill
            .as_ref()
            .and_then(|f| resolve_fill_display_color(f, color_aliases));
        #[cfg(not(feature = "editor"))]
        {
            let _ = color_aliases;
            let _ = &orig_ref.fill;
        }
        layers.push(CompositeLayer {
            ref_idx: source_idx(layer.ref_idx),
            resolved_name: layer.gref.name.clone(),
            grid: scaled_grid,
            offset_row: saturating_i16(layer.raster_row - min_r),
            offset_col: saturating_i16(layer.raster_col - min_c),
            logical_offset_row: layer.gref.row(),
            logical_offset_col: layer.gref.col(),
            negated: layer.gref.negated,
            #[cfg(feature = "editor")]
            fill_color,
        });
    }

    Some(GlyphComposite {
        width,
        height,
        own_offset_row: saturating_i16(-min_r),
        own_offset_col: saturating_i16(-min_c),
        layers,
        inherited_anchors,
    })
}

#[cfg(test)]
fn composite_to_grid(
    own_pixels: &Option<PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    parent_scale: u8,
) -> PixelGrid {
    resolve_composite_layout(
        own_pixels.as_ref(),
        refs,
        named_glyphs,
        name_parts,
        parent_scale,
        true,
    )
    .to_grid(own_pixels.as_ref(), parent_scale)
}

#[cfg(test)]
#[path = "ref_composite_tests.rs"]
mod tests;
