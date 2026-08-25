//! Sample generation: the sample HTML/PNG and the live HTML page.
//!
//! The sample draws glyphs from its *own* cache values, resolved through the
//! driver shared with the TTF builder (`render/glyph_cache.rs`) — so the
//! rules agree but the two pipelines are separate code. That is exactly where
//! its bugs come from: "the font is right, the sample is wrong" (zero-width
//! grids throwing off layout, remap-only glyphs missing, colour handling of
//! indirectly mapped glyphs). When fixing a rendering bug, check the TTF *and*
//! the sample.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::path::Path;

use crate::document::*;
use crate::pixel::PX_SUBPIXEL;
use crate::render::contour::{
    track_contour, track_contour_fullpixel, track_contour_multi_at, track_contour_multi_diff_at,
};
use crate::render::ttf_builder::{
    ColorAliasMap, Rgba, collect_color_aliases, effective_visibility, expand_map_pairs,
    resolve_fill_rgba,
};
use crate::ucd::CharProps;

#[derive(Clone)]
struct SampleComponent {
    row: i32,
    col: i32,
    grid: PixelGrid,
    negated: bool,
    fill_rgba: Option<Rgba>,
    visibility: LayerVisibility,
    /// From a `desync` glyph's own grid: ink for the small (full-pixel)
    /// renderings, invisible to the large (sub-pixel) ones — the sample's two
    /// drawing modes are the font's two faces, and the flag splits them here
    /// the same way. Carried up through composition, so a parent's layer list
    /// keeps the distinction.
    desync: bool,
}

struct SampleGlyph {
    width: u16,
    _height: u16,
    components: Vec<SampleComponent>,
    /// How far the glyph reaches before its origin (never positive), in
    /// scale-1 pixels: a bearing the sample cell has to make room for.
    origin_row: i16,
    origin_col: i16,
    left: i16,
    top: i16,
    /// The declared box's width in scale-1 pixels, when the glyph declares one:
    /// what the cell paints a background over, as opposed to `width`, which is
    /// what its ink needs. `None` for a glyph with no box of its own — a
    /// composite — where the two are the same thing.
    declared_width: Option<u16>,
    scale: u8,
}

struct SampleData {
    height: u16,
    #[allow(dead_code)]
    ascent: u16,
    #[allow(dead_code)]
    descent: u16,
    /// codepoint → glyph name, sorted by codepoint
    cmap: BTreeMap<u32, String>,
    /// glyph name → sample glyph data
    glyphs: HashMap<String, SampleGlyph>,
    /// codepoints excluded from sample display
    excluded: BTreeSet<u32>,
    /// OpenType feature tags
    features: Vec<String>,
}

/// The resolution the three sample documents share.
///
/// `sample.html`, `sample.png` and `live.html` all draw the same glyphs from
/// the same cache, and resolving that cache is most of what producing any one
/// of them costs. Held here so a `build` writing all three resolves once
/// instead of three times.
pub struct SampleSource {
    data: SampleData,
    char_props: CharProps,
}

/// The sample from documents alone, expanding for itself. Every shipping
/// caller has a [`Resolution`](crate::resolve::Resolution) to lend it, so this
/// is the tests' entry point and only theirs.
#[cfg(test)]
fn collect_sample_data(docs: &[&Document]) -> Option<SampleData> {
    collect_sample_data_with(docs, &crate::resolve::Resolution::compute(docs))
}

/// The sample over an expansion someone else already paid for.
///
/// [`Resolution`](crate::resolve::Resolution) holds the expansion this wants,
/// and a `build` computes one anyway to validate and build with. Expanding is
/// the larger half of collecting the sample, so all three share the one rather
/// than doing it three times.
///
/// That expansion is the union of every slice, so the *cmap* below is the one
/// place a face still has to be applied: the sample shows the primary face, and
/// a `map` stated for a slice it does not include maps nothing here.
fn collect_sample_data_with(
    docs: &[&Document],
    resolution: &crate::resolve::Resolution,
) -> Option<SampleData> {
    if docs.is_empty() {
        return None;
    }

    let meta = crate::meta::FontMeta::collect(docs);
    let (height, ascent, descent) = (meta.height(), meta.ascent(), meta.descent());
    if height == 0 {
        return None;
    }

    let color_aliases = collect_color_aliases(docs);

    let glyph_aliases = &resolution.expansion.aliases;
    let all_items = || resolution.expansion.items();
    let face = resolution.faces.primary();

    // Build contour cache for named glyphs
    struct CachedGlyph {
        /// Extent right of / below the glyph origin; area a negative ref
        /// offset puts *before* the origin is a bearing and is not counted.
        width: u16,
        height: u16,
        contours: Vec<Vec<(f32, f32)>>,
        /// The glyph's declared box origin in logical cells, carried so a ref's
        /// placement can run box to box (`glyph_cache::CachedGlyphEntry`).
        declared_origin: (i16, i16),
        anchors: Vec<GlyphPoint>,
        grid: Option<PixelGrid>,
        /// Logical coordinate of raster cell `(0, 0)` of `grid`, in this
        /// glyph's own scale.  Negative once a ref reaches left of / above
        /// the origin.  Mirrors `CachedContours`.
        origin_row: i32,
        origin_col: i32,
        /// Components in the glyph's own logical space, so they may sit at
        /// negative rows/columns.
        components: Vec<SampleComponent>,
        scale: u8,
    }

    impl CachedGlyph {
        fn empty() -> Self {
            Self {
                declared_origin: (0, 0),
                width: 0,
                height: 0,
                contours: Vec::new(),
                anchors: Vec::new(),
                grid: None,
                origin_row: 0,
                origin_col: 0,
                components: Vec::new(),
                scale: 1,
            }
        }

        /// `desync`: the grid is the glyph's bitmap and not its outline, so it
        /// stays a component (the full-pixel renderings draw those) but
        /// contributes no contour, and the raster grid a parent composes
        /// against is a blank of the same size.
        fn from_grid(grid: &PixelGrid, desync: bool) -> Self {
            let contours = if desync {
                Vec::new()
            } else {
                track_contour(grid, PX_SUBPIXEL)
            };
            Self {
                declared_origin: (0, 0),
                width: grid.width,
                height: grid.height,
                contours,
                anchors: Vec::new(),
                grid: Some(if desync {
                    PixelGrid::new(grid.width, grid.height)
                } else {
                    grid.clone()
                }),
                origin_row: 0,
                origin_col: 0,
                components: vec![SampleComponent {
                    row: 0,
                    col: 0,
                    grid: grid.clone(),
                    negated: false,
                    fill_rgba: None,
                    visibility: LayerVisibility::Both,
                    desync,
                }],
                scale: 1,
            }
        }

        /// Where this glyph's raster grid sits when referenced from a parent
        /// at `(ref_row, ref_col)`, rescaled to the parent's resolution.
        fn placed_at(&self, ref_row: i32, ref_col: i32, parent_scale: u8) -> (i32, i32) {
            let rs = self.scale.max(1) as i32;
            let ps = parent_scale.max(1) as i32;
            // The offset names this glyph's declared box corner, so the box
            // comes out of it before its raster's own reach goes in — the same
            // two terms `ref_effective_offset_scaled` applies for the build.
            let (box_col, box_row) = self.declared_origin;
            (
                ref_row - box_row as i32 * ps + self.origin_row * ps / rs,
                ref_col - box_col as i32 * ps + self.origin_col * ps / rs,
            )
        }
    }

    impl crate::render::glyph_cache::CachedGlyphEntry for CachedGlyph {
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

    use crate::render::glyph_cache::resolve_cached as resolve_cached_ref;

    let mut glyph_declared_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
    let mut glyph_offsets: HashMap<String, (i16, i16)> = HashMap::new();
    let mut glyph_declared_widths: HashMap<String, u16> = HashMap::new();
    for item in all_items() {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        {
            glyph_declared_anchors
                .entry(n.clone())
                .or_insert_with(|| body.points.clone());
            if body.origin.is_some() {
                // The sample draws in grid coordinates and wants the bearings,
                // which are the origin negated.
                let (col, row) = body.declared_origin();
                glyph_offsets.entry(n.clone()).or_insert((-col, -row));
            }
            if let Some((w, _)) = body.declared_extent() {
                glyph_declared_widths.entry(n.clone()).or_insert(w);
            }
        }
    }

    let (mut cache, pending) = crate::render::glyph_cache::seed_cache(
        all_items(),
        CachedGlyph::from_grid,
        CachedGlyph::empty,
        &crate::cancel::CancelToken::never(),
    );
    crate::render::glyph_cache::resolve_pending(
        &mut cache,
        pending,
        |name| glyph_declared_anchors.get(name).cloned(),
        &mut crate::render::glyph_cache::FnBuilder(
            |pg: &crate::render::glyph_cache::PendingGlyph,
             effective_refs: &[GlyphRef],
             cache: &_| {
                composite_glyph(
                    pg.pixels.as_ref(),
                    pg.desync,
                    effective_refs,
                    cache,
                    &color_aliases,
                    pg.scale,
                )
                .unwrap_or_else(|| {
                    if let Some(grid) = &pg.pixels {
                        CachedGlyph::from_grid(grid, pg.desync)
                    } else {
                        CachedGlyph::empty()
                    }
                })
            },
        ),
        |_, _| {},
        &crate::cancel::CancelToken::never(),
    );

    fn ref_fill_info(
        gref: &GlyphRef,
        color_aliases: &ColorAliasMap,
    ) -> (Option<Rgba>, LayerVisibility) {
        let rgba = gref
            .fill
            .as_ref()
            .and_then(|f| resolve_fill_rgba(f, color_aliases));
        let vis = effective_visibility(gref.visibility, gref.fill.as_ref(), color_aliases);
        (rgba, vis)
    }

    fn rescale_ref_grid(cached: &CachedGlyph, parent_scale: u8) -> Option<PixelGrid> {
        let ref_grid = cached.grid.as_ref()?;
        let rs = cached.scale.max(1);
        let ps = parent_scale.max(1);
        Some(if rs == ps {
            ref_grid.clone()
        } else {
            ref_grid.rescale(rs, ps)
        })
    }

    /// The layers a `ref` contributes to its parent's flat layer list.
    ///
    /// A composite that subtracts is **not** a list of independent layers: its
    /// negated parts subtract from its own earlier parts and from nothing
    /// else. Spliced into the parent's list they go on erasing whatever the
    /// parent drew under them; and under a negated `ref`, flipping each part
    /// in turn (`comp.negated ^ gref.negated`) re-adds the target's holes as
    /// ink the parent never had — a difference of differences is neither.
    /// The target's resolved raster grid already holds that difference, so it
    /// stands in as one layer. Its `desync` parts travel on separately: no
    /// raster carries them (the vector pass blanks them out), and they are the
    /// bitmap face's ink.
    ///
    /// `placed` is the target's grid rescaled to the parent and put where it
    /// logically sits, as computed for the raster pass.
    fn push_ref_components(
        out: &mut Vec<SampleComponent>,
        gref: &GlyphRef,
        cached: &CachedGlyph,
        placed: Option<&(PixelGrid, i32, i32)>,
        fill_rgba: Option<Rgba>,
        fill_vis: LayerVisibility,
        parent_scale: u8,
    ) {
        let ps = parent_scale.max(1);
        let rs = cached.scale.max(1);
        let rsf = ps as f32 / rs as f32;
        // Box coordinates in, grid coordinates out: see `placed_at`.
        let (box_col, box_row) = cached.declared_origin;
        let (off_r, off_c) = (
            gref.row() as i32 - box_row as i32 * ps as i32,
            gref.col() as i32 - box_col as i32 * ps as i32,
        );
        let overridden = gref.fill.is_some() || gref.visibility.is_some();

        let translated = |comp: &SampleComponent| SampleComponent {
            row: (comp.row as f32 * rsf).round() as i32 + off_r,
            col: (comp.col as f32 * rsf).round() as i32 + off_c,
            grid: if rs == ps {
                comp.grid.clone()
            } else {
                comp.grid.rescale(rs, ps)
            },
            negated: comp.negated ^ gref.negated,
            fill_rgba: fill_rgba.clone().or_else(|| comp.fill_rgba.clone()),
            visibility: if overridden {
                fill_vis
            } else {
                comp.visibility
            },
            desync: comp.desync,
        };

        let subtracts = cached.components.iter().any(|c| c.negated);
        if let (true, Some((grid, row, col))) = (subtracts, placed) {
            // One colour for the whole group: a difference has no per-part
            // colouring left to hand out, so only a fill the parts already
            // agree on (or the `ref`'s own) survives.
            let mut parts = cached.components.iter().filter(|c| !c.desync);
            let first = parts.next().map(|c| (c.fill_rgba.clone(), c.visibility));
            let agreed = first
                .filter(|(rgba, vis)| parts.all(|c| c.fill_rgba == *rgba && c.visibility == *vis));
            out.push(SampleComponent {
                row: *row,
                col: *col,
                grid: grid.clone(),
                negated: gref.negated,
                fill_rgba: fill_rgba
                    .clone()
                    .or_else(|| agreed.as_ref().and_then(|(rgba, _)| rgba.clone())),
                visibility: match (overridden, &agreed) {
                    (true, _) => fill_vis,
                    (false, Some((_, vis))) => *vis,
                    (false, None) => LayerVisibility::Both,
                },
                desync: false,
            });
            for comp in cached.components.iter().filter(|c| c.desync) {
                out.push(translated(comp));
            }
            return;
        }

        for comp in &cached.components {
            out.push(translated(comp));
        }
    }

    fn composite_glyph(
        own_pixels: Option<&PixelGrid>,
        desync: bool,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedGlyph>,
        color_aliases: &ColorAliasMap,
        parent_scale: u8,
    ) -> Option<CachedGlyph> {
        let has_negated = refs.iter().any(|r| r.negated);
        // An all-empty own grid only declares dimensions; treating it as a
        // real layer would pin the composite's origin to (0, 0) and shift
        // refs placed at negative offsets into positive territory.  The
        // declared dims are re-applied by the caller.  Mirrors
        // `CachedContours::from_components_inner`.
        let own_pixels = own_pixels.filter(|g| !g.is_all_empty());
        // A `desync` grid still bounds the glyph and still draws in the
        // full-pixel renderings, but it is not part of the outline — so it is
        // kept out of every layer a contour (or a parent's raster) is traced
        // from, exactly as in the TTF builder's vector pass.
        let own_outline = own_pixels.filter(|_| !desync);
        let ps = parent_scale.max(1);

        // Each ref grid is placed where it logically sits: a target that
        // itself reaches left of / above its origin starts that much before
        // the `ref` offset.
        let ref_scaled: Vec<Option<(PixelGrid, i32, i32)>> = refs
            .iter()
            .map(|gref| {
                let cached = resolve_cached_ref(&gref.name, cache)?;
                let grid = rescale_ref_grid(cached, ps)?;
                let (row, col) = cached.placed_at(gref.row() as i32, gref.col() as i32, ps);
                Some((grid, row, col))
            })
            .collect();

        if has_negated || own_pixels.is_some() {
            let (min_r, min_c, raster_w, raster_h) = crate::render::contour::layer_bounds(
                own_pixels.map(|g| (g, 0, 0)).into_iter().chain(
                    ref_scaled.iter().flatten().filter_map(|(g, row, col)| {
                        (g.width != 0 && g.height != 0).then_some((g, *row, *col))
                    }),
                ),
            );
            let (raster_w, raster_h) = (raster_w as i32, raster_h as i32);
            let mut result = PixelGrid::new(raster_w as u16, raster_h as u16);

            let mut components = Vec::new();
            let mut contour_layers: Vec<(&PixelGrid, i32, i32)> = Vec::new();
            let mut diff_layers: Vec<(&PixelGrid, i32, i32, bool)> = Vec::new();

            if let Some(grid) = own_pixels {
                components.push(SampleComponent {
                    row: 0,
                    col: 0,
                    grid: grid.clone(),
                    negated: false,
                    fill_rgba: None,
                    visibility: LayerVisibility::Both,
                    desync,
                });
            }
            if let Some(grid) = own_outline {
                result.blit(grid, -min_r, -min_c, false);
                if has_negated {
                    diff_layers.push((grid, 0, 0, false));
                } else {
                    contour_layers.push((grid, 0, 0));
                }
            }

            for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
                let Some(cached) = resolve_cached_ref(&gref.name, cache) else {
                    continue;
                };
                let (fill_rgba, fill_vis) = ref_fill_info(gref, color_aliases);
                if let Some((sg, row, col)) = sg {
                    result.blit(sg, row - min_r, col - min_c, gref.negated);
                    if has_negated {
                        diff_layers.push((sg, *row, *col, gref.negated));
                    } else if !gref.negated {
                        contour_layers.push((sg, *row, *col));
                    }
                }
                push_ref_components(
                    &mut components,
                    gref,
                    cached,
                    sg.as_ref(),
                    fill_rgba,
                    fill_vis,
                    ps,
                );
            }

            let contours = if has_negated {
                track_contour_multi_diff_at(&diff_layers, PX_SUBPIXEL)
            } else {
                track_contour_multi_at(&contour_layers, PX_SUBPIXEL)
            };
            let (mut origin_row, mut origin_col) = (min_r, min_c);
            crate::render::glyph_cache::trim_blank_before_origin(
                &mut result,
                &mut origin_row,
                &mut origin_col,
            );

            return Some(CachedGlyph {
                declared_origin: (0, 0),
                width: (min_c + raster_w).max(0) as u16,
                height: (min_r + raster_h).max(0) as u16,
                contours,
                anchors: Vec::new(),
                grid: Some(result),
                origin_row,
                origin_col,
                components,
                scale: ps,
            });
        }

        // Simple contour translation
        let mut all_contours = Vec::new();
        let mut max_width = 0i32;
        let mut max_height = 0i32;
        let mut min_r = 0i32;
        let mut min_c = 0i32;
        let mut components = Vec::new();

        for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let (box_col, box_row) = cached.declared_origin;
            let dx = gref.col() as f32 - box_col as f32 * ps as f32;
            let dy = gref.row() as f32 - box_row as f32 * ps as f32;
            let rs = cached.scale.max(1);
            let rsf = ps as f32 / rs as f32;
            for contour in &cached.contours {
                let translated: Vec<(f32, f32)> = contour
                    .iter()
                    .map(|&(x, y)| (x * rsf + dx, y * rsf + dy))
                    .collect();
                all_contours.push(translated);
            }
            // Extend by the ref's *declared* extent, not its raster grid: a
            // glyph with declared dims and an all-empty own grid has a grid
            // narrower than its advance.  Mirrors `from_components_inner`.
            let scaled_w = (cached.width as f32 * rsf).round() as i32;
            let scaled_h = (cached.height as f32 * rsf).round() as i32;
            max_width = max_width.max(dx as i32 + scaled_w);
            max_height = max_height.max(dy as i32 + scaled_h);
            if let Some((_, row, col)) = sg {
                min_r = min_r.min(*row);
                min_c = min_c.min(*col);
            }

            let (fill_rgba, fill_vis) = ref_fill_info(gref, color_aliases);
            push_ref_components(
                &mut components,
                gref,
                cached,
                sg.as_ref(),
                fill_rgba,
                fill_vis,
                ps,
            );
        }
        let (max_width, max_height) = (max_width.max(0), max_height.max(0));

        let mut combined_grid: Option<PixelGrid> = None;
        for (grid, row, col) in ref_scaled.iter().flatten() {
            let cg = combined_grid.get_or_insert_with(|| {
                PixelGrid::new((max_width - min_c) as u16, (max_height - min_r) as u16)
            });
            let (off_r, off_c) = (row - min_r, col - min_c);
            for r in 0..grid.height as i32 {
                for c in 0..grid.width as i32 {
                    let shape = grid.get(r as u16, c as u16);
                    if !shape.is_clear() {
                        let (dr, dc) = (off_r + r, off_c + c);
                        if dr >= 0 && dc >= 0 && dr < cg.height as i32 && dc < cg.width as i32 {
                            cg.set(dr as u16, dc as u16, shape);
                        }
                    }
                }
            }
        }

        let (mut origin_row, mut origin_col) = (min_r, min_c);
        if let Some(grid) = &mut combined_grid {
            crate::render::glyph_cache::trim_blank_before_origin(
                grid,
                &mut origin_row,
                &mut origin_col,
            );
        }

        Some(CachedGlyph {
            declared_origin: (0, 0),
            width: max_width as u16,
            height: max_height as u16,
            contours: all_contours,
            anchors: Vec::new(),
            grid: combined_grid,
            origin_row,
            origin_col,
            components,
            scale: ps,
        })
    }

    // Collect cmap
    //
    // The same skip the build's collector makes: a mapping whose target never
    // reached the cache claims no codepoint (`collect.rs` walks past the pair
    // that `cache.get` cannot answer). A target nothing defines is never
    // seeded, and a glyph dropped because one of its own refs never resolved is
    // absent for the same reason; letting either in would show the sample a
    // character the font does not map, which is the exact way the sample and
    // the font start disagreeing.
    let mut cmap: BTreeMap<u32, String> = BTreeMap::new();
    for item in all_items() {
        // The expansion is face-independent; the sample is not.
        if !item
            .slice_qualifier()
            .iter()
            .all(|s| face.includes(Some(s.as_str())))
        {
            continue;
        }
        match item {
            // A variation sequence claims no codepoint of its own; the base is
            // already in the cmap through its own `map`.
            DocumentItem::Map {
                selector: Some(_), ..
            } => {}
            DocumentItem::Map {
                char_repr, glyphs, ..
            } => {
                let glyph = crate::render::ttf_builder::resolved_map_target(glyphs);
                let mut pairs = expand_map_pairs(char_repr, glyph);
                glyph_aliases.canonicalize_pairs(&mut pairs);
                for (cp, glyph_name) in pairs {
                    if !cache.contains_key(&glyph_name) {
                        continue;
                    }
                    cmap.entry(cp).or_insert(glyph_name);
                }
            }
            DocumentItem::MapDecomposed {
                char_repr, glyph, ..
            } => {
                let pairs =
                    crate::render::ttf_builder::decomposed_map_pairs(char_repr, glyph.as_deref());
                for (cp, glyph_name) in pairs {
                    if !cache.contains_key(&glyph_name) {
                        continue;
                    }
                    cmap.entry(cp).or_insert(glyph_name);
                }
            }
            _ => {}
        }
    }

    let excluded: BTreeSet<u32> = crate::document::excluded_from_sample(all_items());

    // Collect features
    let mut features: Vec<String> = Vec::new();
    let mut seen_features: HashSet<String> = HashSet::new();
    for item in all_items() {
        if let DocumentItem::Feature { name, .. } = item
            && seen_features.insert(name.clone())
        {
            features.push(name.clone());
        }
    }

    // Build sample glyphs from cache.  Components are kept at their
    // original scale; width/height are normalized to scale=1 for layout.
    let mut sample_glyphs: HashMap<String, SampleGlyph> = HashMap::new();
    for glyph_name in cmap.values() {
        if sample_glyphs.contains_key(glyph_name) {
            continue;
        }
        if let Some(cached) = cache.get(glyph_name) {
            let (left, top) = glyph_offsets.get(glyph_name).copied().unwrap_or((0, 0));
            let declared_width = glyph_declared_widths.get(glyph_name).copied();
            let s = cached.scale.max(1);
            sample_glyphs.insert(
                glyph_name.clone(),
                SampleGlyph {
                    width: cached.width.div_ceil(s as u16),
                    _height: cached.height.div_ceil(s as u16),
                    components: cached.components.clone(),
                    origin_row: cached.origin_row.div_euclid(s as i32) as i16,
                    origin_col: cached.origin_col.div_euclid(s as i32) as i16,
                    left,
                    top,
                    declared_width,
                    scale: s,
                },
            );
        }
    }

    Some(SampleData {
        height,
        ascent,
        descent,
        cmap,
        glyphs: sample_glyphs,
        excluded,
        features,
    })
}

// ---------------------------------------------------------------------------
// SVG path generation from contours
// ---------------------------------------------------------------------------

fn contours_to_svg_path(
    contours: &[Vec<(f32, f32)>],
    scale: f32,
    off_x: f32,
    off_y: f32,
) -> String {
    let mut path = String::new();
    for contour in contours {
        if contour.is_empty() {
            continue;
        }
        let (x0, y0) = contour[0];
        let _ = write!(path, "M{} {}", (x0 + off_x) * scale, (y0 + off_y) * scale);
        let mut prev_x = (x0 + off_x) * scale;
        let mut prev_y = (y0 + off_y) * scale;
        for &(x, y) in &contour[1..] {
            let sx = (x + off_x) * scale;
            let sy = (y + off_y) * scale;
            let dx = sx - prev_x;
            let dy = sy - prev_y;
            if dy == 0.0 {
                let _ = write!(path, "h{dx}");
            } else if dx == 0.0 {
                let _ = write!(path, "v{dy}");
            } else {
                let _ = write!(path, "l{dx} {dy}");
            }
            prev_x = sx;
            prev_y = sy;
        }
        path.push('z');
    }
    path
}

fn path_hash_color(path: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in path.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h & 0x7f7f7f) + 0x808080
}

impl SampleGlyph {
    fn normalized_components(&self) -> Vec<SampleComponent> {
        let s = self.scale.max(1);
        if s == 1 {
            return self.components.clone();
        }
        let sf = s as f32;
        self.components
            .iter()
            .map(|c| SampleComponent {
                row: (c.row as f32 / sf).round() as i32,
                col: (c.col as f32 / sf).round() as i32,
                grid: c.grid.rescale(s, 1),
                negated: c.negated,
                fill_rgba: c.fill_rgba.clone(),
                visibility: c.visibility,
                desync: c.desync,
            })
            .collect()
    }
}

/// Cell size and the offset the glyph is drawn at inside it.  A glyph can
/// reach before its origin, either because an `origin` puts it there or
/// because a ref sits at a negative offset; the cell grows to the left/top by
/// that much so the sample shows the bearing instead of clipping it.
fn sample_display_metrics(sg: &SampleGlyph, font_height: u16) -> (u16, u16, i16, i16) {
    let pad_c = -(sg.origin_col + sg.left).min(0);
    let pad_r = -(sg.origin_row + sg.top).min(0);
    let display_w = pad_c as u16 + sg.width + sg.left.max(0) as u16;
    let display_h = font_height + pad_r as u16;
    (display_w, display_h, pad_c + sg.left, pad_r + sg.top)
}

/// The rectangle a cell paints its background over: `(x, width, height)` in the
/// cell's own pixels.
///
/// This is the *declared box*, not the drawn area — a glyph may put ink outside
/// it and a combining mark is nothing but that. Only the width is the box's
/// own; a cell is an em box tall, since a glyph that declares no height has the
/// grid's, which for a mark is two rows of ink nowhere near where its box
/// corner sits.
fn sample_background(sg: &SampleGlyph, font_height: u16) -> (i16, u16, u16) {
    let (display_w, display_h, col_off, _) = sample_display_metrics(sg, font_height);
    // The pen, i.e. the box's corner: the cell drew the *grid* at `col_off`,
    // and the box's corner sits `origin` cells along from it — which is the
    // bearing negated.
    let pen = col_off - sg.left;
    let width = sg.declared_width.unwrap_or(sg.width);
    // Never past the cell, so a declared box wider than the ink cannot paint
    // outside the image it was measured for.
    let width = width.min(display_w.saturating_sub(pen.max(0) as u16));
    (pen, width, display_h)
}

fn composite_components(width: u16, height: u16, components: &[SampleComponent]) -> PixelGrid {
    use crate::pixel::PixelShape;
    let mut grid = PixelGrid::new(width, height);
    for comp in components {
        for r in 0..comp.grid.height as i32 {
            for c in 0..comp.grid.width as i32 {
                let shape = comp.grid.get(r as u16, c as u16);
                if shape.is_bitmap_filled() {
                    let dr = comp.row + r;
                    let dc = comp.col + c;
                    if dr >= 0 && dc >= 0 && dr < height as i32 && dc < width as i32 {
                        if comp.negated {
                            grid.set(dr as u16, dc as u16, PixelShape::EMPTY);
                        } else {
                            grid.set(dr as u16, dc as u16, shape);
                        }
                    }
                }
            }
        }
    }
    grid
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// The tooltip text for one code point: `U+XXXX NAME (c)`.
///
/// The name comes from [`CharProps`], not from `unicode_names2` directly, so a
/// Private Use character the source named with a `prop` line reads as that name
/// here rather than as a bare code point.
fn char_name_str(cp: u32, char_props: &CharProps) -> String {
    if let Some(ch) = char::from_u32(cp) {
        let name = char_props.name(cp).unwrap_or_default();
        if name.is_empty() {
            format!("U+{cp:04X} ({ch})")
        } else {
            format!("U+{cp:04X} {name} ({ch})")
        }
    } else {
        format!("U+{cp:04X}")
    }
}

// ---------------------------------------------------------------------------
// sample.html
// ---------------------------------------------------------------------------

impl SampleSource {
    /// Every character the primary face maps, and the glyph name it maps to.
    pub fn cmap(&self) -> &BTreeMap<u32, String> {
        &self.data.cmap
    }

    /// The `exclude-from-sample` code points.
    pub fn excluded(&self) -> &BTreeSet<u32> {
        &self.data.excluded
    }

    /// The `prop` lines of the source: what a character is called here, and
    /// which Private Use code points are characters at all.
    pub fn char_props(&self) -> &CharProps {
        &self.char_props
    }

    /// The OpenType feature tags the font carries.
    pub fn features(&self) -> &[String] {
        &self.data.features
    }

    /// Resolve once for every sample document that follows, over an expansion
    /// the caller already has — see [`collect_sample_data_with`].
    pub fn collect_with(
        docs: &[&Document],
        resolution: &crate::resolve::Resolution,
    ) -> Option<Self> {
        Some(Self {
            data: collect_sample_data_with(docs, resolution)?,
            char_props: CharProps::collect(docs),
        })
    }
}

pub fn write_sample_html(w: &mut dyn Write, src: &SampleSource) -> io::Result<()> {
    let SampleSource { data, char_props } = src;

    let svg_scale: f32 = 2.0;

    write!(w, "\
<!doctype html>
<html><head><meta charset=utf-8><title>Unison: graphic sample</title><style>
body{{background:black;color:white;line-height:1}}div{{color:gray}}#sampleglyphs{{display:none}}body.sample #sampleglyphs{{display:block}}body.sample #glyphs{{display:none}}.scaled{{font-size:500%}}
svg{{background:#111;fill:white;vertical-align:top}}.glyphs>:nth-child(even) svg{{background:#222}}:target svg{{background:#333}}svg:hover>path:not(.c),body.sample svg>path:not(.c){{fill:white}}a svg>path:not(.c){{fill:gray}}
</style></head><body>
<input id=sample placeholder='Input sample text here' size=40> <input type=reset id=reset value=Reset> | {nchars} characters
<hr><div id=sampleglyphs></div><div id=glyphs><span class=glyphs>",
        nchars = data.cmap.len(),
    )?;

    // Small glyphs (1x scale)
    let mut excluded_run = false;
    for (&cp, glyph_name) in &data.cmap {
        if data.excluded.contains(&cp) {
            if !excluded_run {
                write!(w, "\u{2026}")?;
                excluded_run = true;
            }
            continue;
        }
        excluded_run = false;

        let title = html_escape(&char_name_str(cp, char_props));
        write!(
            w,
            "<a href='#u{cp:x}'><span id='sm-u{cp:x}' title='{title}'>"
        )?;
        if let Some(sg) = data.glyphs.get(glyph_name) {
            let (display_w, display_h, col_off, row_off) = sample_display_metrics(sg, data.height);
            let norm = sg.normalized_components();
            let shifted: Vec<SampleComponent> = norm
                .iter()
                .filter(|c| c.visibility != LayerVisibility::ColorOnly)
                .map(|c| {
                    let mut c2 = c.clone();
                    c2.col += col_off as i32;
                    c2.row += row_off as i32;
                    c2
                })
                .collect();
            let combined = composite_components(display_w, display_h, &shifted);
            let contours = track_contour_fullpixel(&combined);
            let path = contours_to_svg_path(&contours, 1.0, 0.0, 0.0);
            write!(
                w,
                "<svg viewBox=\"0 0 {display_w} {display_h}\" width=\"{display_w}\" height=\"{display_h}\"><path d='{path}'/></svg>"
            )?;
        }
        write!(w, "</span></a>")?;
    }

    write!(w, "</span><hr><span class='scaled glyphs'>")?;

    // Large glyphs (scaled)
    excluded_run = false;
    for (&cp, glyph_name) in &data.cmap {
        if data.excluded.contains(&cp) {
            if !excluded_run {
                write!(w, "\u{2026}")?;
                excluded_run = true;
            }
            continue;
        }
        excluded_run = false;

        let title = html_escape(&char_name_str(cp, char_props));
        write!(w, "<span id='u{cp:x}' title='{title}'>")?;
        if let Some(sg) = data.glyphs.get(glyph_name) {
            let (display_w, display_h, col_off, row_off) = sample_display_metrics(sg, data.height);
            let vw = display_w as f32 * svg_scale;
            let vh = display_h as f32 * svg_scale;
            let sw = display_w as u32 * 5;
            let sh = display_h as u32 * 5;
            let gs = sg.scale.max(1) as f32;
            let comp_svg_scale = svg_scale / gs;
            write!(
                w,
                "<svg viewBox=\"0 0 {vw} {vh}\" width=\"{sw}\" height=\"{sh}\">"
            )?;
            for comp in &sg.components {
                if comp.visibility == LayerVisibility::MonoOnly {
                    continue;
                }
                // The scaled specimen draws the sub-pixel geometry, i.e. the
                // vector face; a `desync` layer has none.
                if comp.desync {
                    continue;
                }
                let contours = track_contour(&comp.grid, PX_SUBPIXEL);
                let off_x = comp.col as f32 + col_off as f32 * gs;
                let off_y = comp.row as f32 + row_off as f32 * gs;
                let path = contours_to_svg_path(&contours, comp_svg_scale, off_x, off_y);
                if !path.is_empty() {
                    if comp.negated {
                        write!(w, "<path class='c' d='{path}' fill='#000'/>")?;
                    } else if let Some(ref rgba) = comp.fill_rgba {
                        write!(
                            w,
                            "<path class='c' d='{path}' fill='#{:02x}{:02x}{:02x}'/>",
                            rgba.r, rgba.g, rgba.b
                        )?;
                    } else {
                        let color = path_hash_color(&path);
                        write!(w, "<path d='{path}' fill='#{color:06x}'/>")?;
                    }
                }
            }
            write!(w, "</svg>")?;
        }
        write!(w, "</span>")?;
    }

    write!(w, "</span></div><script>\n\
prevt=0;
function $(x){{return document.getElementById(x)}}
function f(t,h){{if(t.normalize)t=t.normalize();if(prevt===t)return;prevt=t;if(!h)location.hash=t?'#!'+encodeURIComponent(t):'';$('sample').value=t;document.body.className=t?'sample':'';var sm='',bg='';for(var i=0;i<t.length;++i){{var c=t.charCodeAt(i).toString(16);sm+=($('sm-u'+c)||{{}}).innerHTML||t[i];bg+=($('u'+c)||{{}}).innerHTML||t[i]}}$('sampleglyphs').innerHTML=sm+'<hr><span class=scaled>'+bg+'</span>'}}
(window.onhashchange=function(){{var h=location.hash||'';f(h.match(/^#!/)?decodeURIComponent(h.substring(2)):'',1);return false}})();
$('sample').onchange=$('sample').onkeyup=function(e){{f(this.value)}}
$('reset').onclick=function(){{$('sample').value='';f('')}}
</script></body></html>\n")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// sample.png
// ---------------------------------------------------------------------------

pub fn write_sample_png(w: &mut dyn Write, src: &SampleSource) -> io::Result<()> {
    let data = &src.data;

    let max_height = data.height as u32;
    let line_width: u32 = 512;
    let num_glyphs_per_line: &[u32] = &[64, 32, 16, 8, 4, 2, 1];

    fn multiples(a: u32, b: u32) -> u32 {
        a & (-(b as i32) as u32)
    }

    // Determine glyph widths and unavailable width slots
    let mut glyph_widths: HashMap<String, (u32, u32)> = HashMap::new();
    let mut unavailable_widths: HashSet<(u32, u32)> = HashSet::new();

    for (&cp, glyph_name) in &data.cmap {
        if let Some(sg) = data.glyphs.get(glyph_name) {
            let (dw, dh, _, _) = sample_display_metrics(sg, data.height);
            let w = dw as u32;
            let h = dh as u32;
            glyph_widths.insert(glyph_name.clone(), (w, h));
            for &ngl in num_glyphs_per_line {
                if w > line_width / ngl {
                    unavailable_widths.insert((ngl, multiples(cp, ngl)));
                }
            }
        }
    }

    // Determine glyph positions
    let mut last: Option<(u32, u32)> = None;
    let mut row: i32 = -1;
    let mut gap: u32 = 0;
    let mut positions: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    let mut row_starts: Vec<u32> = Vec::new();
    let mut row_offsets: Vec<u32> = Vec::new();
    let max_glyphs_per_line: u32 = 64;

    for &cp in data.cmap.keys() {
        let mut current = None;
        for &ngl in num_glyphs_per_line {
            if !unavailable_widths.contains(&(ngl, multiples(cp, ngl))) {
                current = Some((ngl, multiples(cp, ngl)));
                break;
            }
        }
        let Some(cur) = current else { continue };
        let ngl = cur.0;
        if last != Some(cur) {
            if cur.1.saturating_sub(last.map_or(0, |(_, l)| l)) > max_glyphs_per_line {
                gap += 8;
            }
            row += 1;
            row_starts.push(multiples(cp, ngl));
            row_offsets.push(gap);
            last = Some(cur);
        }
        positions.insert(cp, (row as u32, (cp & (ngl - 1)) * (line_width / ngl)));
    }
    let nrows = (row + 1) as u32;
    if nrows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no glyph positions",
        ));
    }

    let label_width: u32 = 8 * 8 + 1;
    let img_width = label_width + 1 + line_width + 1;
    let img_height = (max_height + 1) * nrows + 1 + gap;

    // Render to RGBA pixel buffer
    let mut pixels = vec![0xFFu8; (img_width * img_height * 4) as usize];
    let stride = img_width as usize;

    fn set_pixel(pixels: &mut [u8], stride: usize, x: usize, y: usize, r: u8, g: u8, b: u8) {
        let off = (y * stride + x) * 4;
        pixels[off] = r;
        pixels[off + 1] = g;
        pixels[off + 2] = b;
        pixels[off + 3] = 255;
    }

    // Draw grid lines
    for row_idx in 0..nrows {
        let prev_offset = if row_idx > 0 {
            row_offsets[row_idx as usize - 1]
        } else {
            0
        };
        let offset = row_offsets[row_idx as usize];

        for extra_y in prev_offset..offset {
            let y = (max_height + 1) * row_idx + extra_y;
            if y < img_height {
                for x in label_width..img_width {
                    set_pixel(
                        &mut pixels,
                        stride,
                        x as usize,
                        y as usize,
                        0x80,
                        0x80,
                        0x80,
                    );
                }
            }
        }

        let y = (max_height + 1) * row_idx + offset;
        if y < img_height {
            for x in label_width..img_width {
                set_pixel(
                    &mut pixels,
                    stride,
                    x as usize,
                    y as usize,
                    0x80,
                    0x80,
                    0x80,
                );
            }
        }

        let label = format!("U+{:04X}", row_starts[row_idx as usize]);
        let label_y = y + 1;
        for (char_idx, ch) in label.chars().enumerate() {
            let cp = ch as u32;
            if let Some(glyph_name) = data.cmap.get(&cp)
                && let Some(sg) = data.glyphs.get(glyph_name)
            {
                render_glyph_bitmap_rgba(
                    &mut pixels,
                    stride,
                    img_height as usize,
                    (char_idx as u32 * 8) as i32,
                    label_y as i32,
                    sg,
                    [0x80, 0x80, 0x80],
                    false,
                );
            }
        }
    }
    // Bottom border
    {
        let y = img_height - 1;
        for x in label_width..img_width {
            set_pixel(
                &mut pixels,
                stride,
                x as usize,
                y as usize,
                0x80,
                0x80,
                0x80,
            );
        }
    }

    // Fill glyph content area with gray so empty slots are distinguishable
    for row_idx in 0..nrows {
        let offset = row_offsets[row_idx as usize];
        let y = (max_height + 1) * row_idx + offset + 1;
        for dy in 0..max_height {
            let py = (y + dy) as usize;
            if py < img_height as usize {
                for x in (label_width + 1) as usize..(img_width - 1) as usize {
                    set_pixel(&mut pixels, stride, x, py, 0xC0, 0xC0, 0xC0);
                }
            }
        }
    }

    // Render each glyph
    for (&cp, glyph_name) in &data.cmap {
        let Some(&(r, left)) = positions.get(&cp) else {
            continue;
        };
        let Some(sg) = data.glyphs.get(glyph_name) else {
            continue;
        };
        let y = (max_height + 1) * r + row_offsets[r as usize] + 1;
        let x = label_width + 1 + left;

        let (_, dh, col_off, row_off) = sample_display_metrics(sg, data.height);

        // The background is the *declared box*, so a glyph that claims no width
        // — every combining mark — draws onto the page rather than onto a cell
        // of its own. Ink outside the box is allowed and simply lands on the
        // gray behind it.
        let (box_x, box_w, _) = sample_background(sg, data.height);
        for dy in 0..dh.min(max_height as u16) as u32 {
            for dx in 0..box_w as u32 {
                let py = (y + dy) as usize;
                let Some(px) = (x as i32 + box_x as i32 + dx as i32).try_into().ok() else {
                    continue;
                };
                let px: usize = px;
                if py < img_height as usize && px < stride {
                    set_pixel(&mut pixels, stride, px, py, 0xFF, 0xFF, 0xFF);
                }
            }
        }

        render_glyph_bitmap_rgba(
            &mut pixels,
            stride,
            img_height as usize,
            x as i32 + col_off as i32,
            y as i32 + row_off as i32,
            sg,
            [0x00, 0x00, 0x00],
            true,
        );
    }

    // Encode as PNG
    encode_rgba_png(w, &pixels, img_width, img_height)
}

// Destination surface plus the glyph and its colors.
#[expect(clippy::too_many_arguments)]
fn render_glyph_bitmap_rgba(
    pixels: &mut [u8],
    stride: usize,
    img_height: usize,
    x: i32,
    y: i32,
    sg: &SampleGlyph,
    fg_color: [u8; 3],
    use_fill_colors: bool,
) {
    let norm = sg.normalized_components();
    for comp in &norm {
        if use_fill_colors && comp.visibility == LayerVisibility::MonoOnly {
            continue;
        }

        let [cr, cg, cb] = if use_fill_colors && !comp.negated {
            if let Some(ref rgba) = comp.fill_rgba {
                [rgba.r, rgba.g, rgba.b]
            } else {
                fg_color
            }
        } else if comp.negated {
            [0xFF, 0xFF, 0xFF]
        } else {
            fg_color
        };

        for r in 0..comp.grid.height as i32 {
            for c in 0..comp.grid.width as i32 {
                let shape = comp.grid.get(r as u16, c as u16);
                if shape.is_bitmap_filled() {
                    let py = y + comp.row + r;
                    let px = x + comp.col + c;
                    if py >= 0 && px >= 0 && (py as usize) < img_height && (px as usize) < stride {
                        let off = (py as usize * stride + px as usize) * 4;
                        pixels[off] = cr;
                        pixels[off + 1] = cg;
                        pixels[off + 2] = cb;
                        pixels[off + 3] = 255;
                    }
                }
            }
        }
    }
}

fn encode_rgba_png(w: &mut dyn Write, pixels: &[u8], width: u32, height: u32) -> io::Result<()> {
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// live.html
// ---------------------------------------------------------------------------

pub fn write_live_html(
    w: &mut dyn Write,
    src: &SampleSource,
    ttf_bytes: &[u8],
    data_dir: Option<&Path>,
) -> io::Result<()> {
    write_live_html_inner(w, src, ttf_bytes, None, data_dir)
}

pub fn write_live_html_woff2(
    w: &mut dyn Write,
    src: &SampleSource,
    ttf_bytes: &[u8],
    woff2_bytes: &[u8],
    data_dir: Option<&Path>,
) -> io::Result<()> {
    write_live_html_inner(w, src, ttf_bytes, Some(woff2_bytes), data_dir)
}

fn write_live_html_inner(
    w: &mut dyn Write,
    src: &SampleSource,
    ttf_bytes: &[u8],
    woff2_bytes: Option<&[u8]>,
    data_dir: Option<&Path>,
) -> io::Result<()> {
    let SampleSource { data, char_props } = src;

    let (font_mime, font_data) = if let Some(w2) = woff2_bytes {
        ("font/woff2", w2)
    } else {
        ("font/ttf", ttf_bytes)
    };
    let font_base64 = base64_encode(font_data);

    let features = if data.features.is_empty() {
        "inherit".to_string()
    } else {
        data.features
            .iter()
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(",")
    };

    let has_udhr = data_dir.is_some_and(|d| d.join("udhr-article1.json").exists());
    let has_confusables = data_dir.is_some_and(|d| {
        std::fs::read_dir(d)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with("confusables") && n.ends_with(".txt"))
                })
            })
            .unwrap_or(false)
    });
    // CLDR subdivision containment, e.g. `cldr-subdivisions-48.2.0.json`; the version is part of
    // the file name, so match by prefix rather than pinning a release here.
    let subdivisions_path = data_dir.and_then(|d| {
        std::fs::read_dir(d).ok().and_then(|entries| {
            entries.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("cldr-subdivisions-") && n.ends_with(".json"))
            })
        })
    });

    write!(w, "\
<!doctype html>
<html><head><meta charset=utf-8><title>Unison: live sample</title>
<style>
@font-face{{font-family:Unison;src:url(data:{font_mime};base64,{font_base64});font-feature-settings:{features}}}
pre{{font-family:Unison,monospace;font-size:200%;line-height:1;margin:0;white-space:pre-wrap}}pre span{{background:#eee}}.hide{{display:none}}
</style>
<script>
window.onload=function(){{var e=document.getElementById('edit');e.contentEditable='true';for(var x=document.querySelectorAll('a[href^=\"#\"]'),i=0;x[i];++i)x[i].onclick=function(){{e.innerHTML=document.getElementById(this.getAttribute('href').substring(1)).innerHTML;return false}}}}
</script>
</head><body><pre>
Hello? This is the <u>Unison</u> font.
You can play with it right here.
Please note that this is in development and subject to change.

Load: ")?;

    let mut links: Vec<(&str, &str)> = Vec::new();
    if has_udhr {
        links.push(("udhr", "UDHR"));
    }
    if has_confusables {
        links.push(("confus", "Confusables"));
    }
    links.push(("hangul", "All Hangul"));
    links.push(("flags", "All Flags"));
    links.push(("all", "All Glyphs"));
    for (i, (id, label)) in links.iter().enumerate() {
        if i > 0 {
            write!(w, ", ")?;
        }
        write!(w, "<a href='#{id}'>{label}</a>")?;
    }

    write!(w, "\n\
\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}
</pre><pre id=edit>")?;

    // Logo
    write!(
        w,
        r#"
888     888          d8b
888     888          Y8P
888     888
888     888 88888b.  888 .d8888b   .d88b.  88888b.
888     888 888 "88b 888 88K      d88""88b 888 "88b
888     888 888  888 888 "Y8888b. 888  888 888  888
Y88b. .d88P 888  888 888      X88 Y88..88P 888  888
 "Y88888P"  888  888 888  88888P'  "Y88P"  888  888
"#
    )?;

    // UDHR section
    if has_udhr {
        write_live_udhr(w, data_dir.unwrap(), &data.cmap)?;
    }

    // Confusables section
    if has_confusables {
        write_live_confusables(w, data_dir.unwrap(), &data.cmap, char_props)?;
    }

    // Hangul section
    write_live_hangul(w)?;

    // Flags section
    write_live_flags(w, subdivisions_path.as_deref())?;

    // All Glyphs section
    write!(
        w,
        "</pre><pre id=all class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}All Supported Glyphs\u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n"
    )?;

    let mut chars = String::new();
    let mut prev_block: Option<u32> = None;
    for &cp in data.cmap.keys() {
        let block = cp >> 5;
        if prev_block.is_some_and(|pb| pb != block) {
            writeln!(w, "<span>{}</span>", html_escape(&chars))?;
            chars.clear();
        }
        prev_block = Some(block);
        if let Some(ch) = char::from_u32(cp) {
            chars.push(ch);
        }
    }
    if !chars.is_empty() {
        writeln!(w, "<span>{}</span>", html_escape(&chars))?;
    }

    writeln!(w, "</pre></body></html>")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: UDHR section
// ---------------------------------------------------------------------------

fn write_live_udhr(
    w: &mut dyn Write,
    data_dir: &Path,
    cmap: &BTreeMap<u32, String>,
) -> io::Result<()> {
    #[derive(serde::Deserialize)]
    struct UdhrEntry {
        lang: String,
        text: String,
    }

    let path = data_dir.join("udhr-article1.json");
    let content = std::fs::read_to_string(&path)?;
    let entries: Vec<UdhrEntry> = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let cmap_set: HashSet<u32> = cmap.keys().copied().collect();

    // Filter to entries whose characters are all in the font
    let mut displayable: Vec<&UdhrEntry> = Vec::new();
    let mut unsupported_chars_by_entry: HashMap<usize, BTreeSet<char>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        let mut unsupported = BTreeSet::new();
        for ch in entry.text.chars() {
            if !cmap_set.contains(&(ch as u32)) {
                unsupported.insert(ch);
            }
        }
        if unsupported.is_empty() {
            displayable.push(entry);
        } else {
            unsupported_chars_by_entry.insert(i, unsupported);
        }
    }

    // Greedy set cover in JSON order: select entries that add new codepoints
    let mut covered: HashSet<u32> = HashSet::new();
    let mut selected_indices: Vec<usize> = Vec::new();

    for (i, entry) in displayable.iter().enumerate() {
        let has_new = entry.text.chars().any(|ch| !covered.contains(&(ch as u32)));
        if has_new {
            selected_indices.push(i);
            for ch in entry.text.chars() {
                covered.insert(ch as u32);
            }
        }
    }

    let udhr_title = "Article 1 of Universal Declaration of Human Rights";
    let border: String = std::iter::repeat_n('\u{2500}', udhr_title.len()).collect();
    write!(
        w,
        "</pre><pre id=udhr class=hide>\n\
\u{250c}{border}\u{2510}\n\
\u{2502}{udhr_title}\u{2502}\n\
\u{2514}{border}\u{2518}\n\n"
    )?;

    let selected_set: HashSet<usize> = selected_indices.iter().copied().collect();

    let mut disp_idx = 0;
    for (orig_idx, entry) in entries.iter().enumerate() {
        if unsupported_chars_by_entry.contains_key(&orig_idx) {
            continue;
        }
        if selected_set.contains(&disp_idx) {
            writeln!(
                w,
                "\u{2022} {}: <span>{}</span>",
                html_escape(&entry.lang),
                html_escape(&entry.text),
            )?;
        }
        disp_idx += 1;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: Confusables section
// ---------------------------------------------------------------------------

fn write_live_confusables(
    w: &mut dyn Write,
    data_dir: &Path,
    cmap: &BTreeMap<u32, String>,
    char_props: &CharProps,
) -> io::Result<()> {
    let confusables_path = std::fs::read_dir(data_dir)?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("confusables") && n.ends_with(".txt"))
        })
        .map(|e| e.path())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "confusables file not found"))?;

    let content = std::fs::read_to_string(&confusables_path)?;
    let cmap_set: HashSet<u32> = cmap.keys().copied().collect();

    // Parse confusables: source_cp -> target_cp (both single codepoints)
    // Group by target_cp to form equivalence groups
    let mut groups: BTreeMap<Vec<u32>, Vec<Vec<u32>>> = BTreeMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: XXXX ; YYYY ZZZZ... ; MA/SA  # comment
        let parts: Vec<&str> = line.splitn(4, ';').collect();
        if parts.len() < 3 {
            continue;
        }
        let source_cps: Vec<u32> = parts[0]
            .split_whitespace()
            .filter_map(|s| u32::from_str_radix(s.trim(), 16).ok())
            .collect();
        let target_cps: Vec<u32> = parts[1]
            .split_whitespace()
            .filter_map(|s| u32::from_str_radix(s.trim(), 16).ok())
            .collect();
        if source_cps.is_empty() || target_cps.is_empty() {
            continue;
        }
        groups
            .entry(target_cps.clone())
            .or_default()
            .push(source_cps);
    }

    // Build equivalence groups: target + all sources that map to it
    // Filter: only include groups where at least 2 members are fully in the font
    struct ConfusGroup {
        members: Vec<Vec<u32>>,
    }

    let mut display_groups: Vec<ConfusGroup> = Vec::new();

    for (target, sources) in &groups {
        let mut all_members: Vec<&Vec<u32>> = vec![target];
        for src in sources {
            all_members.push(src);
        }

        let displayable: Vec<Vec<u32>> = all_members
            .into_iter()
            .filter(|cps| cps.iter().all(|cp| cmap_set.contains(cp)))
            .cloned()
            .collect();

        if displayable.len() >= 2 {
            // Sort: target first, then sources by codepoint
            let mut members = displayable;
            members.sort();
            members.dedup();
            display_groups.push(ConfusGroup { members });
        }
    }

    // Sort groups by first member's first codepoint
    display_groups.sort_by(|a, b| a.members[0].cmp(&b.members[0]));

    // Merge groups that share members (transitive closure)
    let mut merged: Vec<ConfusGroup> = Vec::new();
    for group in display_groups {
        let mut found = None;
        for (i, existing) in merged.iter().enumerate() {
            if group.members.iter().any(|m| existing.members.contains(m)) {
                found = Some(i);
                break;
            }
        }
        if let Some(i) = found {
            for m in group.members {
                if !merged[i].members.contains(&m) {
                    merged[i].members.push(m);
                }
            }
            merged[i].members.sort();
        } else {
            merged.push(group);
        }
    }

    write!(
        w,
        "</pre><pre id=confus class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}Confusables\u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n\n"
    )?;

    for group in &merged {
        for (i, member) in group.members.iter().enumerate() {
            if i > 0 {
                write!(w, " ")?;
            }
            // Build title with each codepoint's name
            let title_parts: Vec<String> = member
                .iter()
                .map(|&cp| char_name_str(cp, char_props))
                .collect();
            let title = html_escape(&title_parts.join("\n"));
            let text: String = member.iter().filter_map(|&cp| char::from_u32(cp)).collect();
            write!(w, "<span title='{title}'>{}</span>", html_escape(&text))?;
        }
        writeln!(w)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: Hangul section
// ---------------------------------------------------------------------------

fn write_live_hangul(w: &mut dyn Write) -> io::Result<()> {
    write!(
        w,
        "</pre><pre id=hangul class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}All Hangul Syllables\u{2502}\n\
\u{2502} (Modern + Ancient) \u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n\n\
<div style='white-space:pre'><span><a href='#' onclick=\"\
var p=[],i,j,k,a=[],b=[],c=[''];\
function v(z,s){{for(i=0;s[i];++i)for(j=s[i][0];j&lt;=(s[i][1]||s[i][0]);++j)z.push(String.fromCharCode(j))}}\
v(a,[[0x115f],[0x1100,0x115e],[0xa960,0xa97c]]);\
v(b,[[0x1160,0x117e],[0x119e],[0x11a1],[0x11a3,0x11a4]]);\
v(c,[[0x11a8,0x11ff],[0xd7cb,0xd7fb]]);\
for(i=0;i&lt;a.length;++i,p.push('\\n'))for(j=0;j&lt;b.length;++j,p.push('\\n'))for(k=0;k&lt;c.length;++k)p.push(a[i]+b[j]+c[k]);\
this.parentNode.replaceChild(document.createTextNode(p.join('')),this);\
return!1\">Render!</a></span></div>\n"
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: Flags section
// ---------------------------------------------------------------------------

/// Emoji tag sequence for a subdivision code, per UTS #51 Annex C.1: tag_base U+1F3F4, one tag
/// character per `[0-9a-z]` of the code, then tag_end U+E007F. Returns `None` for a code that
/// cannot form a well-formed sequence, so a bad data file degrades instead of emitting garbage.
fn subdivision_flag_seq(code: &str) -> Option<String> {
    if code.is_empty() || code.len() > 6 {
        return None;
    }
    let mut s = String::from("\u{1f3f4}");
    for b in code.bytes() {
        if !b.is_ascii_lowercase() && !b.is_ascii_digit() {
            return None;
        }
        s.push(char::from_u32(0xe0000 + b as u32)?);
    }
    s.push('\u{e007f}');
    Some(s)
}

fn write_live_flags(w: &mut dyn Write, subdivisions_path: Option<&Path>) -> io::Result<()> {
    write!(
        w,
        "</pre><pre id=flags class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}All Flags\u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n\n"
    )?;

    // Every regional indicator pair, 26 per line.
    let mut line = String::new();
    for hi in 0..26u32 {
        line.clear();
        for lo in 0..26u32 {
            line.push(char::from_u32(0x1f1e6 + hi).unwrap());
            line.push(char::from_u32(0x1f1e6 + lo).unwrap());
        }
        writeln!(w, "{line}")?;
    }

    let Some(path) = subdivisions_path else {
        return Ok(());
    };

    #[derive(serde::Deserialize)]
    struct SubdivisionFile {
        subdivisions: BTreeMap<String, Vec<String>>,
    }

    let content = std::fs::read_to_string(path)?;
    let parsed: SubdivisionFile = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    writeln!(w)?;
    for (region, codes) in &parsed.subdivisions {
        let seqs: Vec<String> = codes
            .iter()
            .filter_map(|c| subdivision_flag_seq(c))
            .collect();
        if seqs.is_empty() {
            continue;
        }
        writeln!(w, "{region} {}", seqs.join(""))?;
    }

    Ok(())
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
#[path = "sample_tests.rs"]
mod tests;
