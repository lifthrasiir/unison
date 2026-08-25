//! The traced-contour cache and [`CachedContours`], the per-glyph entry that
//! drives composite resolution for the font build.
//!
//! [`ContourCache`] is persistent: it survives across incremental rebuilds.

use super::*;

/// Traced contours of one glyph, in the tracer's own float coordinates.
type TracedContours = Vec<Vec<(f32, f32)>>;

#[derive(Clone)]
struct CacheEntry<T> {
    value: T,
    gen_id: u64,
}

#[derive(Default, Clone)]
pub struct ContourCache {
    entries: HashMap<u64, CacheEntry<TracedContours>>,
    composite_entries: HashMap<u64, CacheEntry<CachedContours>>,
    gen_id: u64,
}

/// One [`ContourCache`] per build flavor.
///
/// Every key already carries the `bitmap` flag, so the two flavors never shared
/// an entry — they only shared a `&mut`. Holding them apart is what lets the
/// bitmap and vector collections of one font pair run at the same time instead
/// of one behind the other.
#[cfg(feature = "editor")]
#[derive(Default)]
pub struct ContourCaches([ContourCache; 2]);

#[cfg(feature = "editor")]
impl ContourCaches {
    /// `(bitmap, vector)`, borrowed at once.
    pub(super) fn split(&mut self) -> (&mut ContourCache, &mut ContourCache) {
        let (bitmap, vector) = self.0.split_at_mut(1);
        (&mut bitmap[0], &mut vector[0])
    }

    /// The one flavor a caller that builds only outlines needs.
    pub(super) fn vector(&mut self) -> &mut ContourCache {
        &mut self.0[1]
    }

    pub fn clear(&mut self) {
        for cache in &mut self.0 {
            cache.entries.clear();
            cache.composite_entries.clear();
        }
    }

    pub fn begin_generation(&mut self) {
        for cache in &mut self.0 {
            cache.gen_id += 1;
        }
    }

    pub fn evict_stale(&mut self) {
        for cache in &mut self.0 {
            let cur_gen = cache.gen_id;
            cache.entries.retain(|_, e| e.gen_id == cur_gen);
            cache.composite_entries.retain(|_, e| e.gen_id == cur_gen);
        }
    }
}

#[cfg(feature = "editor")]
pub type SharedContourCache = Arc<Mutex<ContourCaches>>;

#[cfg(feature = "editor")]
pub fn new_contour_cache() -> SharedContourCache {
    Arc::new(Mutex::new(ContourCaches::default()))
}

fn hash_grid_for_cache(grid: &PixelGrid, bitmap: bool) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    grid.width.hash(&mut hasher);
    grid.height.hash(&mut hasher);
    for px in &grid.pixels {
        px.0.hash(&mut hasher);
    }
    if !grid.details.is_empty() {
        grid.den.hash(&mut hasher);
        grid.details.hash(&mut hasher);
    }
    bitmap.hash(&mut hasher);
    hasher.finish()
}

fn cached_track_contour(
    cache: &mut ContourCache,
    grid: &PixelGrid,
    bitmap: bool,
) -> Vec<Vec<(f32, f32)>> {
    let key = hash_grid_for_cache(grid, bitmap);
    let cur_gen = cache.gen_id;
    if let Some(entry) = cache.entries.get_mut(&key) {
        entry.gen_id = cur_gen;
        return entry.value.clone();
    }
    let contours = track_contour(grid, PX_SUBPIXEL);
    cache.entries.insert(
        key,
        CacheEntry {
            value: contours.clone(),
            gen_id: cur_gen,
        },
    );
    contours
}

#[derive(Clone)]
pub(super) struct CachedContours {
    /// Extent to the right of / below the glyph origin, which is what the
    /// advance falls back to.  Area reached by a negative ref offset sits
    /// *before* the origin and is a bearing, so it is not counted here.
    pub(super) width: u16,
    pub(super) height: u16,
    /// Contours in the glyph's own logical space; negative coordinates are
    /// kept as such.
    pub(super) contours: Vec<Vec<(f32, f32)>>,
    /// The glyph's declared box origin in logical cells, carried so a ref's
    /// placement can run box to box (`glyph_cache::CachedGlyphEntry`).
    declared_origin: (i16, i16),
    pub(super) anchors: Vec<GlyphPoint>,
    pub(super) grid: Option<PixelGrid>,
    /// Logical coordinate of raster cell `(0, 0)` of `grid`, in this glyph's
    /// own scale.  Zero unless a ref reaches above/left of the origin; a
    /// parent has to add it to the `ref` offset, or it loses that area.
    origin_row: i32,
    origin_col: i32,
    /// For composite-eligible glyphs: (component_name, col_offset, row_offset)
    pub(super) composite_components: Option<Vec<(String, f32, f32)>>,
    pub(super) scale: u8,
}

impl CachedContours {
    /// Where this glyph's raster grid sits when referenced from a parent at
    /// `(ref_row, ref_col)`, rescaled to the parent's resolution.
    /// Where a ref to this glyph lands in the parent's raster. The same three
    /// terms as [`crate::ref_composite::ref_effective_offset_scaled`], which
    /// this has to agree with cell for cell: the offset runs box origin to box
    /// origin, so both origins come in, and the target's own resolved raster
    /// origin is added on top.
    pub(super) fn placed_at(&self, ref_row: i32, ref_col: i32, parent_scale: u8) -> (i32, i32) {
        let rs = self.scale.max(1) as i32;
        let ps = parent_scale.max(1) as i32;
        let (child_c, child_r) = self.declared_origin;
        (
            ref_row - child_r as i32 * ps + self.origin_row * ps / rs,
            ref_col - child_c as i32 * ps + self.origin_col * ps / rs,
        )
    }
}

impl crate::render::glyph_cache::CachedGlyphEntry for CachedContours {
    fn anchors(&self) -> &[GlyphPoint] {
        &self.anchors
    }

    fn declared_origin(&self) -> (i16, i16) {
        self.declared_origin
    }

    fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
        (&mut self.width, &mut self.height)
    }

    fn set_resolution(&mut self, anchors: Vec<GlyphPoint>, scale: u8, origin: (i16, i16)) {
        self.anchors = anchors;
        self.scale = scale;
        self.declared_origin = origin;
    }
}

/// A ref's placement in the parent's *logical* space, box origin to box origin.
///
/// The raster paths go through [`CachedContours::placed_at`], which adds the
/// target's resolved raster origin on top; the contour paths want only the two
/// declared origins, because a contour already carries its own negative
/// coordinates. Both have to agree about the box term or a glyph's outline and
/// its TrueType composite reference end up in different places.
fn box_placement(gref: &GlyphRef, cached: &CachedContours, parent_scale: u8) -> (f32, f32) {
    let ps = parent_scale.max(1) as f32;
    let (child_c, child_r) = cached.declared_origin;
    (
        gref.col() as f32 - child_c as f32 * ps,
        gref.row() as f32 - child_r as f32 * ps,
    )
}

impl CachedContours {
    pub(super) fn empty() -> Self {
        Self {
            declared_origin: (0, 0),
            width: 0,
            height: 0,
            contours: Vec::new(),
            anchors: Vec::new(),
            grid: None,
            origin_row: 0,
            origin_col: 0,
            composite_components: None,
            scale: 1,
        }
    }

    pub(super) fn from_grid(grid: &PixelGrid, bitmap: bool, cc: Option<&mut ContourCache>) -> Self {
        if bitmap {
            let mut bitmap_grid = grid.clone();
            for pixel in &mut bitmap_grid.pixels {
                if pixel.is_bitmap_filled() {
                    *pixel = PixelShape::new(PX_ALMOSTFULL, true);
                } else {
                    *pixel = PixelShape::EMPTY;
                }
            }
            let contours = match cc {
                Some(c) => cached_track_contour(c, &bitmap_grid, true),
                None => track_contour(&bitmap_grid, PX_SUBPIXEL),
            };
            Self {
                declared_origin: (0, 0),
                width: bitmap_grid.width,
                height: bitmap_grid.height,
                contours,
                anchors: Vec::new(),
                grid: Some(bitmap_grid),
                origin_row: 0,
                origin_col: 0,
                composite_components: None,
                scale: 1,
            }
        } else {
            let contours = match cc {
                Some(c) => cached_track_contour(c, grid, false),
                None => track_contour(grid, PX_SUBPIXEL),
            };
            Self {
                declared_origin: (0, 0),
                width: grid.width,
                height: grid.height,
                contours,
                anchors: Vec::new(),
                grid: Some(grid.clone()),
                origin_row: 0,
                origin_col: 0,
                composite_components: None,
                scale: 1,
            }
        }
    }

    fn hash_composite_key(
        own_pixels: Option<&PixelGrid>,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedContours>,
        bitmap: bool,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bitmap.hash(&mut hasher);
        if let Some(grid) = own_pixels {
            1u8.hash(&mut hasher);
            hash_grid_for_cache(grid, bitmap).hash(&mut hasher);
        } else {
            0u8.hash(&mut hasher);
        }
        refs.len().hash(&mut hasher);
        for gref in refs {
            gref.name.hash(&mut hasher);
            gref.offset.hash(&mut hasher);
            gref.negated.hash(&mut hasher);
            if let Some(resolved) = resolve_cached_ref(&gref.name, cache) {
                if let Some(ref grid) = resolved.grid {
                    hash_grid_for_cache(grid, bitmap).hash(&mut hasher);
                }
                resolved.origin_row.hash(&mut hasher);
                resolved.origin_col.hash(&mut hasher);
                resolved.width.hash(&mut hasher);
                resolved.height.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    fn from_components_inner(
        own_pixels: Option<&PixelGrid>,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedContours>,
        bitmap: bool,
        parent_scale: u8,
    ) -> Option<Self> {
        let has_negated = refs.iter().any(|r| r.negated);
        let own_pixels = own_pixels.filter(|g| !g.is_all_empty());
        let ps = parent_scale.max(1);

        // Extent of the refs measured by their *declared* dimensions rather
        // than by the raster they light: a `desync` target, or one with an
        // all-empty own grid, is wider than the cells its own refs fill, and
        // that declared width is what it is worth to a parent.  The simple
        // path below extends by this directly; the two raster paths take it as
        // a floor, so the same composite does not get a different advance
        // depending on whether its layers happen to conflict at the subpixel
        // level (which is what routes it away from the simple path).
        // `at` reads the offset as written, so the box term has to come in the
        // same way it does for every other placement — otherwise a rebased
        // offset inflates the extent by exactly the origin it was rebased by.
        let declared_extent = |axis: fn(&CachedContours) -> u16,
                               at: fn(&GlyphRef) -> i16,
                               origin: fn((i16, i16)) -> i16| {
            refs.iter()
                .filter_map(|gref| {
                    let cached = resolve_cached_ref(&gref.name, cache)?;
                    let scale_f = ps as f32 / cached.scale.max(1) as f32;
                    let shift = -(origin(cached.declared_origin) as i32) * ps as i32;
                    Some(at(gref) as i32 + shift + (axis(cached) as f32 * scale_f).round() as i32)
                })
                .max()
                .unwrap_or(0)
        };

        // Pre-rescale ref grids so their raster resolution matches the parent,
        // and place each one where it logically sits: a target that itself
        // reaches left of / above its origin starts that much before the
        // `ref` offset, and dropping that is how the left column of a nested
        // negative-offset composite used to disappear.
        let ref_scaled: Vec<Option<(PixelGrid, i32, i32)>> = refs
            .iter()
            .map(|gref| {
                let cached = resolve_cached_ref(&gref.name, cache)?;
                let ref_grid = cached.grid.as_ref()?;
                let rs = cached.scale.max(1);
                let grid = if rs == ps {
                    ref_grid.clone()
                } else {
                    ref_grid.rescale(rs, ps)
                };
                let (row, col) = cached.placed_at(gref.row() as i32, gref.col() as i32, ps);
                Some((grid, row, col))
            })
            .collect();

        if has_negated {
            // Collect layers with negation flags and trace contours via
            // track_contour_multi_diff, which applies the stack in `ref` order
            // per pixel, unioning positives and subtracting negations.
            let mut diff_layers: Vec<(&PixelGrid, i32, i32, bool)> = Vec::new();
            if let Some(grid) = own_pixels {
                diff_layers.push((grid, 0, 0, false));
            }
            for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
                if let Some((sg, row, col)) = sg {
                    diff_layers.push((sg, *row, *col, gref.negated));
                }
            }

            let contours = if bitmap {
                let bitmap_grids: Vec<PixelGrid> = diff_layers
                    .iter()
                    .map(|(g, _, _, _)| to_bitmap_grid(g))
                    .collect();
                let bitmap_layers: Vec<(&PixelGrid, i32, i32, bool)> = bitmap_grids
                    .iter()
                    .zip(diff_layers.iter())
                    .map(|(bg, &(_, r, c, neg))| (bg as &PixelGrid, r, c, neg))
                    .collect();
                track_contour_multi_diff_at(&bitmap_layers, PX_SUBPIXEL)
            } else {
                track_contour_multi_diff_at(&diff_layers, PX_SUBPIXEL)
            };

            // Build flattened grid for downstream composites that reference
            // this glyph.  shape_subtract may produce PX_DOT for some pixels,
            // which is acceptable here since the grid is only used for pixel
            // lookups, not for contour tracing.
            let (min_r, min_c, raster_w, raster_h) = crate::render::contour::layer_bounds(
                diff_layers.iter().map(|&(g, r, c, _)| (g, r, c)),
            );
            let mut result = PixelGrid::new(raster_w as u16, raster_h as u16);

            if let Some(grid) = own_pixels {
                result.blit(grid, -min_r, -min_c, false);
            }

            for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
                if let Some((sg, row, col)) = sg {
                    result.blit(sg, row - min_r, col - min_c, gref.negated);
                }
            }

            let (mut origin_row, mut origin_col) = (min_r, min_c);
            crate::render::glyph_cache::trim_blank_before_origin(
                &mut result,
                &mut origin_row,
                &mut origin_col,
            );

            return Some(Self {
                declared_origin: (0, 0),
                width: (min_c + raster_w as i32)
                    .max(declared_extent(|c| c.width, |g| g.col(), |o| o.0))
                    .max(0) as u16,
                height: (min_r + raster_h as i32)
                    .max(declared_extent(|c| c.height, |g| g.row(), |o| o.1))
                    .max(0) as u16,
                contours,
                anchors: Vec::new(),
                grid: Some(result),
                origin_row,
                origin_col,
                composite_components: None,
                scale: 1,
            });
        }

        let mut layers: Vec<(&PixelGrid, i32, i32)> = Vec::new();
        if let Some(grid) = own_pixels {
            layers.push((grid, 0, 0));
        }
        for sg in ref_scaled.iter().flatten() {
            layers.push((&sg.0, sg.1, sg.2));
        }

        let needs_multi = own_pixels.is_some() || layers_have_subpixel_conflicts(&layers);

        if needs_multi {
            // Use track_contour_multi to correctly union overlapping subpixels.
            let contours = if bitmap {
                let bitmap_grids: Vec<PixelGrid> =
                    layers.iter().map(|(g, _, _)| to_bitmap_grid(g)).collect();
                let bitmap_layers: Vec<(&PixelGrid, i32, i32)> = bitmap_grids
                    .iter()
                    .zip(layers.iter())
                    .map(|(bg, &(_, r, c))| (bg, r, c))
                    .collect();
                track_contour_multi_at(&bitmap_layers, PX_SUBPIXEL)
            } else {
                track_contour_multi_at(&layers, PX_SUBPIXEL)
            };

            // The flattened grid a *parent* composite reads this glyph out
            // of. It is not a bookkeeping copy: once the parent's own layers
            // conflict it re-traces its contours from these grids, so a layer
            // has to be *unioned* onto what is already there the way
            // `PixelGrid::blit` does it — a `ref` dropping a subpixel over the
            // host's own full pixel would otherwise replace it, and the parent
            // would draw in pieces what the child's own outline drew whole.
            // (`blit` also settles a hardblank before the region layer sees it,
            // which is the case that made overwriting cost the parent ink.)
            let (min_r, min_c, raster_w, raster_h) =
                crate::render::contour::layer_bounds(layers.iter().copied());
            let (raster_w, raster_h) = (raster_w as i32, raster_h as i32);
            let mut result = PixelGrid::new(raster_w as u16, raster_h as u16);
            for &(grid, row_off, col_off) in &layers {
                result.blit(grid, row_off - min_r, col_off - min_c, false);
            }

            // Pure-ref composites (no own pixels) can still use TrueType
            // composite format; the contours above serve as a fallback
            // for inline glyphs.
            let composite_components = if own_pixels.is_none() {
                Some(
                    refs.iter()
                        .filter_map(|gref| {
                            let cached = resolve_cached_ref(&gref.name, cache)?;
                            let (dx, dy) = box_placement(gref, cached, ps);
                            Some((gref.name.clone(), dx, dy))
                        })
                        .collect(),
                )
            } else {
                None
            };

            let (mut origin_row, mut origin_col) = (min_r, min_c);
            crate::render::glyph_cache::trim_blank_before_origin(
                &mut result,
                &mut origin_row,
                &mut origin_col,
            );

            return Some(Self {
                declared_origin: (0, 0),
                width: (min_c + raster_w)
                    .max(declared_extent(|c| c.width, |g| g.col(), |o| o.0))
                    .max(0) as u16,
                height: (min_r + raster_h)
                    .max(declared_extent(|c| c.height, |g| g.row(), |o| o.1))
                    .max(0) as u16,
                contours,
                anchors: Vec::new(),
                grid: Some(result),
                origin_row,
                origin_col,
                composite_components,
                scale: 1,
            });
        }

        // No negated refs, no own pixels, no overlap: simple contour translation
        let mut all_contours = Vec::new();
        let mut max_width = 0i32;
        let mut max_height = 0i32;
        let mut min_r = 0i32;
        let mut min_c = 0i32;
        let mut components = Vec::new();

        for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let rs = cached.scale.max(1);
            let scale_f = ps as f32 / rs as f32;
            let (dx, dy) = box_placement(gref, cached, ps);
            components.push((gref.name.clone(), dx, dy));
            for contour in &cached.contours {
                let translated: Vec<(f32, f32)> = contour
                    .iter()
                    .map(|&(x, y)| (x * scale_f + dx, y * scale_f + dy))
                    .collect();
                all_contours.push(translated);
            }
            // Extend by the ref's *declared* extent, not its raster grid: a
            // glyph with declared dims and an all-empty own grid has a grid
            // narrower than its advance.  The offset is read as written, so the
            // box term comes in the same way `declared_extent` above brings it
            // in — the extent is measured from where the target's *grid* lands,
            // not from the box corner the offset names.
            let scaled_w = (cached.width as f32 * scale_f).round() as i32;
            let scaled_h = (cached.height as f32 * scale_f).round() as i32;
            let (shift_c, shift_r) = (
                -(cached.declared_origin.0 as i32) * ps as i32,
                -(cached.declared_origin.1 as i32) * ps as i32,
            );
            max_width = max_width.max(gref.col() as i32 + shift_c + scaled_w);
            max_height = max_height.max(gref.row() as i32 + shift_r + scaled_h);
            if let Some((_, row, col)) = sg {
                min_r = min_r.min(*row);
                min_c = min_c.min(*col);
            }
        }
        let (max_width, max_height) = (max_width.max(0), max_height.max(0));

        // No layer conflicts here, so this stacking cannot lose anything —
        // union it all the same, so the flattened grid is built by one rule.
        let mut combined_grid: Option<PixelGrid> = None;
        for (grid, row, col) in ref_scaled.iter().flatten() {
            let cg = combined_grid.get_or_insert_with(|| {
                PixelGrid::new((max_width - min_c) as u16, (max_height - min_r) as u16)
            });
            cg.blit(grid, row - min_r, col - min_c, false);
        }

        let (mut origin_row, mut origin_col) = (min_r, min_c);
        if let Some(grid) = &mut combined_grid {
            crate::render::glyph_cache::trim_blank_before_origin(
                grid,
                &mut origin_row,
                &mut origin_col,
            );
        }

        Some(Self {
            declared_origin: (0, 0),
            width: max_width as u16,
            height: max_height as u16,
            contours: all_contours,
            anchors: Vec::new(),
            grid: combined_grid,
            origin_row,
            origin_col,
            composite_components: Some(components),
            scale: 1,
        })
    }
}

/// The font build's [`CompositeBuilder`](crate::render::glyph_cache::CompositeBuilder):
/// [`ContourCache`] on one side of the split, the tracer on the other.
///
/// The cache is what keeps the borrow here: it is a `&mut` that only `lookup`
/// and `store` touch, which is exactly why `build` — holding `&self` — is free
/// to run on every core at once.
pub(super) struct ContourBuilder<'a> {
    bitmap: bool,
    cache: Option<&'a mut ContourCache>,
}

impl<'a> ContourBuilder<'a> {
    pub(super) fn new(bitmap: bool, cache: Option<&'a mut ContourCache>) -> Self {
        Self { bitmap, cache }
    }

    /// The glyph's own grid, as this flavor sees it: the vector build resolves a
    /// `desync` glyph from its refs alone, and `resolve_pending` re-applies the
    /// grid's dimensions afterwards, so the advance is unaffected.
    fn own_pixels<'p>(
        &self,
        pg: &'p crate::render::glyph_cache::PendingGlyph,
    ) -> Option<&'p PixelGrid> {
        pg.pixels.as_ref().filter(|_| self.bitmap || !pg.desync)
    }
}

impl crate::render::glyph_cache::CompositeBuilder<CachedContours> for ContourBuilder<'_> {
    type Key = u64;

    fn lookup(
        &mut self,
        pg: &crate::render::glyph_cache::PendingGlyph,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedContours>,
    ) -> (u64, Option<CachedContours>) {
        let key = CachedContours::hash_composite_key(self.own_pixels(pg), refs, cache, self.bitmap);
        let hit = self.cache.as_deref_mut().and_then(|cc| {
            let cur_gen = cc.gen_id;
            cc.composite_entries.get_mut(&key).map(|entry| {
                entry.gen_id = cur_gen;
                entry.value.clone()
            })
        });
        (key, hit)
    }

    fn build(
        &self,
        pg: &crate::render::glyph_cache::PendingGlyph,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedContours>,
    ) -> CachedContours {
        let own = self.own_pixels(pg);
        CachedContours::from_components_inner(own, refs, cache, self.bitmap, pg.scale)
            .unwrap_or_else(|| match own {
                // Unreachable while `resolve_pending` only ever hands over a
                // glyph whose every ref already resolved, which is the only way
                // the tracer above gives up. Kept as the same fallback it always
                // was rather than as a panic.
                Some(grid) => CachedContours::from_grid(grid, self.bitmap, None),
                None => CachedContours::empty(),
            })
    }

    fn store(&mut self, key: u64, value: &CachedContours) {
        if let Some(cc) = self.cache.as_deref_mut() {
            let gen_id = cc.gen_id;
            cc.composite_entries.insert(
                key,
                CacheEntry {
                    value: value.clone(),
                    gen_id,
                },
            );
        }
    }
}

pub(super) fn layers_have_subpixel_conflicts(layers: &[(&PixelGrid, i32, i32)]) -> bool {
    for i in 0..layers.len() {
        let (g1, r1, c1) = layers[i];
        for &(g2, r2, c2) in &layers[i + 1..] {
            let overlap_r0 = r1.max(r2);
            let overlap_r1 = (r1 + g1.height as i32).min(r2 + g2.height as i32);
            let overlap_c0 = c1.max(c2);
            let overlap_c1 = (c1 + g1.width as i32).min(c2 + g2.width as i32);
            for r in overlap_r0..overlap_r1 {
                for c in overlap_c0..overlap_c1 {
                    let (p1, p2) = (
                        ((r - r1) as u16, (c - c1) as u16),
                        ((r - r2) as u16, (c - c2) as u16),
                    );
                    let s1 = g1.get(p1.0, p1.1);
                    let s2 = g2.get(p2.0, p2.1);
                    if s1.is_clear() || s2.is_clear() {
                        continue;
                    }
                    if s1 != s2 {
                        return true;
                    }
                    // Equal shape ids normally union to themselves, but two
                    // custom cells share one id while holding different
                    // geometry.
                    if s1.shape_id() == PX_CUSTOM
                        && g1.region_at(p1.0, p1.1) != g2.region_at(p2.0, p2.1)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn to_bitmap_grid(grid: &PixelGrid) -> PixelGrid {
    let mut bg = PixelGrid::new(grid.width, grid.height);
    for r in 0..grid.height {
        for c in 0..grid.width {
            if grid.get(r, c).is_bitmap_filled() {
                bg.set(r, c, PixelShape::new(PX_ALMOSTFULL, true));
            }
        }
    }
    bg
}

pub(super) fn gcd(a: i32, b: i32) -> i32 {
    crate::pattern::gcd(a.unsigned_abs() as usize, b.unsigned_abs() as usize) as i32
}

pub(super) fn contour_signed_area(contour: &[(i16, i16)]) -> i64 {
    let n = contour.len();
    if n < 3 {
        return 0;
    }
    let mut area = 0i64;
    for i in 0..n {
        let (x0, y0) = contour[i];
        let (x1, y1) = contour[(i + 1) % n];
        area += x0 as i64 * y1 as i64 - x1 as i64 * y0 as i64;
    }
    area
}
