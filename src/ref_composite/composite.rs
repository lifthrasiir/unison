//! Laying a composite out and flattening it: where each layer sits in the box,
//! the colors a fill displays as, and the grid the whole thing becomes.

use super::*;

/// Where a ref's target lands in the parent's raster.
///
/// Three terms, in the parent's raster units: the offset as written; the
/// child's box origin, subtracted, because an offset names the child's *box*
/// corner and not its grid's; and the child's resolved raster origin, which is
/// where the child's own composition already reached past its grid.
///
/// The *parent's* origin is deliberately absent. Everything inside a glyph —
/// its own pixels, its anchors, the refs it places — lives in one coordinate
/// system, its grid; the parent's box origin says only where that grid sits
/// relative to the pen, which is an output-stage bearing
/// (`ttf_builder::collect`). Letting it shift the refs but not the own pixels
/// is what pulled the two apart, and a composite that declares a grid *and*
/// refs would grow by the origin it declared.
fn ref_effective_offset_scaled(
    gref: &GlyphRef,
    resolved: &ResolvedGlyph,
    parent_scale: u8,
) -> (i32, i32) {
    let ps = parent_scale as i32;
    let rs = resolved.scale.max(1) as i32;
    let (child_c, child_r) = resolved.declared_origin;
    (
        gref.row() as i32 - child_r as i32 * ps + resolved.origin_row * ps / rs,
        gref.col() as i32 - child_c as i32 * ps + resolved.origin_col * ps / rs,
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
pub(super) struct ResolvedLayer<'a> {
    #[cfg_attr(not(any(feature = "editor", test)), expect(dead_code))]
    pub(super) ref_idx: usize,
    pub(super) gref: &'a GlyphRef,
    pub(super) resolved: &'a ResolvedGlyph,
    pub(super) raster_row: i32,
    pub(super) raster_col: i32,
}

/// All refs of a composite resolved once, with the resulting bounding box.
///
/// This is the single resolution pass shared by the build cache
/// (`resolve_expansion` → [`CompositeLayout::to_grid`]), the editor's live
/// composite ([`compute_composite`]) and bounds queries
/// ([`composite_bounds`]).  They must agree on how every ref resolves —
/// a historical divergence here made the editor and the flattened font
/// disagree on pattern refs — so none of them resolves refs on its own.
pub(super) struct CompositeLayout<'a> {
    pub(super) layers: Vec<ResolvedLayer<'a>>,
    pub(super) min_r: i32,
    pub(super) min_c: i32,
    pub(super) max_r: i32,
    pub(super) max_c: i32,
}

pub(super) fn resolve_composite_layout<'a>(
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
    pub(super) fn grid_cache_key(&self, own_pixels: Option<&PixelGrid>, parent_scale: u8) -> u64 {
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
    pub(super) fn to_grid(&self, own_pixels: Option<&PixelGrid>, parent_scale: u8) -> PixelGrid {
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

/// How deep a `ref` chain the colour walk follows. The resolved cache is built
/// bottom-up and so cannot be cyclic, but a name that reaches itself through
/// pattern expansion has no such guarantee, and the walk is on the stack.
#[cfg(feature = "editor")]
const COLOR_WALK_DEPTH: u32 = 16;

/// Does this `ref` target draw a colour of its own, anywhere below it?
///
/// Names and `fill`s only — no layout, no grids — because every unfilled `ref`
/// in the document asks this, and only the ones that answer yes pay for
/// [`layer_cell_colors`]. The first `fill` found ends the search.
#[cfg(feature = "editor")]
fn target_draws_color(
    resolved: &ResolvedGlyph,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    depth: u32,
) -> bool {
    if depth == 0 {
        return false;
    }
    let Some(src) = resolved.inline_source.as_ref() else {
        return false;
    };
    src.refs.iter().any(|r| {
        r.fill.is_some()
            || lookup_ref_name(&r.name, named_glyphs, name_parts, true).is_some_and(|child| {
                target_draws_color(child, named_glyphs, name_parts, depth - 1)
            })
    })
}

/// The colours one layer draws, cell by cell over the grid it flattened to.
///
/// A `ref` with no `fill` of its own draws whatever colours its target draws,
/// however deep they sit, and a `fill` is a claim over everything below it —
/// the same two rules the font build follows (`ColorPiece` in
/// `render::ttf_builder`'s `collect`), so the editor and the font agree on
/// which cell is which colour.
///
/// The layer stays *one* layer: a ref index is how everything else in the
/// editor addresses a layer — the active-ref highlight, "Inline once", the
/// minimap — so what travels up is a colour per cell and not a layer per
/// colour. `None` at a cell means the layer's own colour, and `None` for the
/// whole layer means nothing below it was coloured.
#[cfg(feature = "editor")]
fn layer_cell_colors(
    resolved: &ResolvedGlyph,
    grid: &PixelGrid,
    parent_scale: u8,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<Vec<Option<egui::Color32>>> {
    if grid.width == 0
        || grid.height == 0
        || !target_draws_color(resolved, named_glyphs, name_parts, COLOR_WALK_DEPTH)
    {
        return None;
    }
    let mut walk = ColorWalk {
        named_glyphs,
        name_parts,
        color_aliases,
        width: grid.width,
        height: grid.height,
        cells: vec![None; grid.width as usize * grid.height as usize],
        painted: false,
    };
    // One cell of the target's own grid is this many cells of the layer's,
    // which is that grid rescaled to the parent.
    let f = parent_scale.max(1) as f32 / resolved.scale.max(1) as f32;
    walk.walk(resolved, 0.0, 0.0, f, None, COLOR_WALK_DEPTH);
    walk.painted.then_some(walk.cells)
}

/// The walk [`layer_cell_colors`] runs, carrying the map it stamps into.
#[cfg(feature = "editor")]
struct ColorWalk<'a> {
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &'a NamePartsMap,
    color_aliases: &'a crate::render::ttf_builder::ColorAliasMap,
    width: u16,
    height: u16,
    cells: Vec<Option<egui::Color32>>,
    painted: bool,
}

#[cfg(feature = "editor")]
impl ColorWalk<'_> {
    /// Stamp `color` over every inked cell of `grid`, whose cell `(0, 0)` sits
    /// at `(row, col)` in map cells and whose cells are `f` map cells across.
    fn stamp(&mut self, grid: &PixelGrid, row: f32, col: f32, f: f32, color: egui::Color32) {
        for r in 0..grid.height {
            for c in 0..grid.width {
                if grid.get(r, c).is_clear() {
                    continue;
                }
                let r0 = (row + r as f32 * f).round() as i32;
                let r1 = (row + (r + 1) as f32 * f).round() as i32;
                let c0 = (col + c as f32 * f).round() as i32;
                let c1 = (col + (c + 1) as f32 * f).round() as i32;
                for mr in r0.max(0)..r1.min(self.height as i32) {
                    for mc in c0.max(0)..c1.min(self.width as i32) {
                        self.cells[mr as usize * self.width as usize + mc as usize] = Some(color);
                        self.painted = true;
                    }
                }
            }
        }
    }

    /// Walk one target's own declaration, colouring what it draws. `inherited`
    /// is the colour claimed by a `fill` further up, if any; a target with no
    /// declaration left to walk is stamped whole in it.
    fn walk(
        &mut self,
        resolved: &ResolvedGlyph,
        row: f32,
        col: f32,
        f: f32,
        inherited: Option<egui::Color32>,
        depth: u32,
    ) {
        let src = match (depth > 0).then_some(resolved.inline_source.as_ref()).flatten() {
            Some(src) => src,
            None => {
                if let Some(color) = inherited {
                    self.stamp(&resolved.grid, row, col, f, color);
                }
                return;
            }
        };
        // The target's grid starts at its resolved raster origin, so its own
        // pixels — which sit at its logical origin — are that far into it.
        let (base_row, base_col) = (
            row - resolved.origin_row as f32 * f,
            col - resolved.origin_col as f32 * f,
        );
        if let Some(pixels) = &src.pixels
            && let Some(color) = inherited
        {
            self.stamp(pixels, base_row, base_col, f, color);
        }
        let layout = resolve_composite_layout(
            src.pixels.as_ref(),
            &src.refs,
            self.named_glyphs,
            self.name_parts,
            resolved.scale,
            true,
        );
        for layer in &layout.layers {
            // A negation draws nothing of its own; it only takes area away,
            // and the flattened grid this map is read against already lost it.
            if layer.gref.negated {
                continue;
            }
            let color = match &layer.gref.fill {
                // `fill fg` resolves to no colour: the layer's own is right.
                Some(fill) => resolve_fill_display_color(fill, self.color_aliases),
                None => inherited,
            };
            let (r, c) = (
                base_row + layer.raster_row as f32 * f,
                base_col + layer.raster_col as f32 * f,
            );
            let child_f = f * resolved.scale.max(1) as f32 / layer.resolved.scale.max(1) as f32;
            if layer.gref.fill.is_some() {
                // A `fill` claims everything below it, so the target goes down
                // as one drawing in that one colour.
                if let Some(color) = color {
                    self.stamp(&layer.resolved.grid, r, c, child_f, color);
                }
            } else {
                self.walk(layer.resolved, r, c, child_f, color, depth - 1);
            }
        }
    }
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
    let parent = body.declared_extent();
    body.compose
        .iter()
        .flat_map(|c| {
            // No family and no clearance rule: the check reports, and the
            // view only draws.
            crate::compose::expand_compose("", parent, body.scale, c, &dims, None, None)
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
    aligns: &crate::document::AnchorAligns,
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

    let origin_of = |name: &str| {
        resolve_ref_name_for_view(name, named_glyphs, name_parts)
            .map_or((0, 0), |resolved| resolved.declared_origin)
    };
    let (mut effective_refs, exposed, _) = derive_ref_offsets_with(
        &body.points,
        &all_refs,
        body.scale,
        aligns,
        |name| {
            resolve_ref_name_for_view(name, named_glyphs, name_parts)
                .map(|resolved| resolved.resolved_anchors.clone())
        },
        |name| alt_index.get(name).to_vec(),
        |name| {
            resolve_ref_name_for_view(name, named_glyphs, name_parts)
                .map(|resolved| resolved.declared_anchors.clone())
        },
        origin_of,
    );
    rebase_offsets_to_box(&mut effective_refs, body.scale, origin_of);
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

    // The *raster* the view draws into, not the declared box: a composite that
    // resolved to nothing still gets a cell to be empty in, rather than a
    // zero-sized grid every consumer would have to special-case. A glyph that
    // declares a zero-width box is unaffected — no box is read here.
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
        // A `fill` is the layer's one colour whatever the target draws; only a
        // ref that writes none lets its target's colours through.
        #[cfg(feature = "editor")]
        let cell_colors = orig_ref.fill.is_none().then(|| {
            layer_cell_colors(
                layer.resolved,
                &scaled_grid,
                body.scale,
                named_glyphs,
                name_parts,
                color_aliases,
            )
        }).flatten();
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
            #[cfg(feature = "editor")]
            cell_colors,
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
pub(super) fn composite_to_grid(
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
