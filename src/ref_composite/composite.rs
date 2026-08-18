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

    let origin_of = |name: &str| {
        resolve_ref_name_for_view(name, named_glyphs, name_parts)
            .map_or((0, 0), |resolved| resolved.declared_origin)
    };
    let (mut effective_refs, exposed, _) = derive_ref_offsets_with(
        &body.points,
        &all_refs,
        body.scale,
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
