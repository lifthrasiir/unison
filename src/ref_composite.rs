use std::collections::HashMap;

use crate::document::{
    Document, DocumentItem, GlyphBody, GlyphPoint, GlyphRef, NamePartsMap, PixelGrid,
    expand_name_pattern, substitute_name_parts,
};

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

pub struct ResolvedGlyph {
    pub grid: PixelGrid,
    /// Logical coordinate represented by raster cell `(0, 0)`. Keeping this
    /// separate from the raster is essential for nested refs whose bounds
    /// extend left/up from the glyph origin.
    pub(crate) origin_row: i32,
    pub(crate) origin_col: i32,
    anchors: Vec<GlyphPoint>,
}

fn saturating_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn raster_dimension(min: i32, max: i32) -> u16 {
    max.saturating_sub(min).clamp(0, u16::MAX as i32) as u16
}

/// Expansion used by runtime ref lookup and dependency resolution.
pub(crate) fn expand_ref_names(name: &str) -> Option<Vec<String>> {
    expand_name_pattern(name).ok().map(|names| names.into_vec())
}

/// Derive effective ref offsets and the anchors exposed by the resulting
/// composite without changing the source refs. A target's `-name` anchors
/// consume matching `+name` anchors that are already available, then the
/// target's `+name` anchors are published for following refs. Unconsumed
/// minus anchors remain exposed so aliases/composites forward anchors from
/// their targets.
///
/// Ref order does not matter: when a target carries `-name` but no matching
/// `+name` is available yet, resolution is deferred until a later ref
/// publishes it. Refs that remain unresolved after the fixpoint fall back
/// to `(0, 0)`.
///
/// `lookup_alternatives` returns sorted alternative glyph names for a base
/// name (e.g. for "foo" it returns ["foo:bar", "foo:baz"]). When the primary
/// ref's anchors don't size-match, alternatives are tried in order.
pub(crate) fn derive_ref_offsets_with<F, G>(
    own_points: &[GlyphPoint],
    refs: &[GlyphRef],
    mut lookup_anchors: F,
    mut lookup_alternatives: G,
) -> (Vec<GlyphRef>, Vec<GlyphPoint>)
where
    F: FnMut(&str) -> Option<Vec<GlyphPoint>>,
    G: FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
{
    let mut exposed_minus: Vec<GlyphPoint> = own_points
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .cloned()
        .collect();
    let mut available_plus: Vec<GlyphPoint> = own_points
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .cloned()
        .collect();

    let target_anchors_list: Vec<Option<Vec<GlyphPoint>>> = refs
        .iter()
        .map(|gref| lookup_anchors(&gref.name))
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
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut exposed_minus,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            if let Some(offset) = try_match_minus_plus(target_anchors, &available_plus) {
                commit_ref(
                    gref,
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut exposed_minus,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            // Try alternatives when primary doesn't size-match.
            let mut alt_matched = false;
            for (alt_name, alt_anchors) in &alternatives_list[i] {
                if let Some(offset) = try_match_minus_plus(alt_anchors, &available_plus) {
                    let alt_gref = GlyphRef {
                        name: alt_name.clone(),
                        offset: None,
                        negated: gref.negated,
                    };
                    commit_ref(
                        &alt_gref,
                        offset,
                        alt_anchors,
                        &mut available_plus,
                        &mut exposed_minus,
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

            // Defer if any candidate has minus anchors (might match later).
            let has_minus = target_anchors
                .iter()
                .any(|p| p.position.starts_with('-'))
                || alternatives_list[i]
                    .iter()
                    .any(|(_, a)| a.iter().any(|p| p.position.starts_with('-')));
            if has_minus {
                continue;
            }

            commit_ref(
                gref,
                (0, 0),
                target_anchors,
                &mut available_plus,
                &mut exposed_minus,
                &mut effective_refs[i],
            );
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
                Some(offset) => (gref.name.clone(), offset, target_anchors),
                None => {
                    let mut found = None;
                    for (alt_name, alt_anchors) in &alternatives_list[i] {
                        if let Some(offset) = try_match_minus_plus(alt_anchors, &available_plus) {
                            found = Some((alt_name.clone(), offset, alt_anchors.as_slice()));
                            break;
                        }
                    }
                    found.unwrap_or_else(|| (gref.name.clone(), (0, 0), target_anchors))
                }
            }
        };
        let resolved_gref = GlyphRef {
            name: resolved_name,
            offset: gref.offset,
            negated: gref.negated,
        };
        commit_ref(
            &resolved_gref,
            offset,
            used_anchors,
            &mut available_plus,
            &mut exposed_minus,
            &mut effective_refs[i],
        );
    }

    let effective_refs = effective_refs.into_iter().map(Option::unwrap).collect();
    exposed_minus.extend(available_plus);
    (effective_refs, exposed_minus)
}

fn try_match_minus_plus(
    target_anchors: &[GlyphPoint],
    available_plus: &[GlyphPoint],
) -> Option<(i16, i16)> {
    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let base = minus.position.strip_prefix('-')?;
        if let Some(plus) = available_plus
            .iter()
            .find(|p| p.position.strip_prefix('+') == Some(base) && p.size_matches(minus))
        {
            return Some((
                saturating_i16(plus.col as i32 - minus.col as i32),
                saturating_i16(plus.row as i32 - minus.row as i32),
            ));
        }
    }
    None
}

fn commit_ref(
    gref: &GlyphRef,
    offset: (i16, i16),
    target_anchors: &[GlyphPoint],
    available_plus: &mut Vec<GlyphPoint>,
    exposed_minus: &mut Vec<GlyphPoint>,
    out: &mut Option<GlyphRef>,
) {
    let effective = GlyphRef {
        name: gref.name.clone(),
        offset: Some(offset),
        negated: gref.negated,
    };
    let off_col = effective.col();
    let off_row = effective.row();

    // Consume before publishing. In particular, a component carrying
    // both `-join` and `+join` must publish its outgoing anchor rather
    // than immediately deleting it again.
    let consumed_names: Vec<String> = target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .filter_map(|minus| {
            let base = minus.position.strip_prefix('-')?;
            if available_plus.iter().any(|p| {
                p.position.strip_prefix('+') == Some(base) && p.size_matches(minus)
            }) {
                Some(base.to_string())
            } else {
                None
            }
        })
        .collect();
    available_plus.retain(|p| {
        !p.position
            .strip_prefix('+')
            .is_some_and(|base| consumed_names.iter().any(|n| n == base))
    });

    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        if !consumed_names.iter().any(|n| n == base) {
            exposed_minus.push(GlyphPoint {
                position: minus.position.clone(),
                col: saturating_i16(minus.col as i32 + off_col as i32),
                row: saturating_i16(minus.row as i32 + off_row as i32),
                col_end: saturating_i16(minus.col_end as i32 + off_col as i32),
                row_end: saturating_i16(minus.row_end as i32 + off_row as i32),
            });
        }
    }
    for plus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
    {
        let Some(base) = plus.position.strip_prefix('+') else {
            continue;
        };
        if !available_plus
            .iter()
            .any(|p| p.position.strip_prefix('+') == Some(base))
        {
            available_plus.push(GlyphPoint {
                position: plus.position.clone(),
                col: saturating_i16(plus.col as i32 + off_col as i32),
                row: saturating_i16(plus.row as i32 + off_row as i32),
                col_end: saturating_i16(plus.col_end as i32 + off_col as i32),
                row_end: saturating_i16(plus.row_end as i32 + off_row as i32),
            });
        }
    }

    *out = Some(effective);
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn resolve_named_glyphs_with_parts(
    docs: &[&Document],
    name_parts: &NamePartsMap,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();

    struct Pending {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
    }

    let mut pending: Vec<Pending> = Vec::new();

    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Glyph { name, body } = item {
                let raw_key = name.display();
                let key = substitute_name_parts(&raw_key, name_parts);
                let expanded_keys = expand_ref_names(&key);
                let expanded_count = expanded_keys.as_ref().map_or(1, |e| e.len());
                if expanded_count <= 1 {
                    if !cache.contains_key(&key) && !pending.iter().any(|p| p.name == key) {
                        if body.refs.is_empty() {
                            cache.insert(
                                key,
                                ResolvedGlyph {
                                    grid: body
                                        .pixels
                                        .clone()
                                        .unwrap_or_else(|| PixelGrid::new(0, 0)),
                                    origin_row: 0,
                                    origin_col: 0,
                                    anchors: body.points.clone(),
                                },
                            );
                        } else {
                            let subs_refs: Vec<GlyphRef> = body
                                .refs
                                .iter()
                                .map(|r| GlyphRef {
                                    name: substitute_name_parts(&r.name, name_parts),
                                    offset: r.offset,
                                    negated: r.negated,
                                })
                                .collect();

                            pending.push(Pending {
                                name: key,
                                pixels: body.pixels.clone(),
                                refs: subs_refs,
                                points: body.points.clone(),
                            });
                        }
                    }
                } else {
                    let expanded_keys = expanded_keys.unwrap();
                    let ref_expansions: Vec<Option<_>> = body
                        .refs
                        .iter()
                        .map(|r| {
                            let subst = substitute_name_parts(&r.name, name_parts);
                            expand_ref_names(&subst)
                        })
                        .collect();
                    for (k, expanded_name) in expanded_keys.into_iter().enumerate() {
                        if cache.contains_key(&expanded_name)
                            || pending.iter().any(|p| p.name == expanded_name)
                        {
                            continue;
                        }
                        let expanded_refs: Vec<GlyphRef> = body
                            .refs
                            .iter()
                            .enumerate()
                            .map(|(ri, r)| {
                                let rname = ref_expansions[ri]
                                    .as_ref()
                                    .and_then(|e| {
                                        if e.len() > 1 {
                                            e.get(k % e.len()).cloned()
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or_else(|| substitute_name_parts(&r.name, name_parts));
                                GlyphRef {
                                    name: rname,
                                    offset: r.offset,
                                    negated: r.negated,
                                }
                            })
                            .collect();
                        if expanded_refs.is_empty() {
                            cache.insert(
                                expanded_name,
                                ResolvedGlyph {
                                    grid: body
                                        .pixels
                                        .clone()
                                        .unwrap_or_else(|| PixelGrid::new(0, 0)),
                                    origin_row: 0,
                                    origin_col: 0,
                                    anchors: body.points.clone(),
                                },
                            );
                        } else {
                            pending.push(Pending {
                                name: expanded_name,
                                pixels: body.pixels.clone(),
                                refs: expanded_refs,
                                points: body.points.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    let mut progress = true;
    while progress {
        progress = false;
        let alt_index = AlternativesIndex::build(&cache);
        pending.retain(|pg| {
            if !pg
                .refs
                .iter()
                .all(|r| resolve_ref_name_with_parts(&r.name, &cache, name_parts).is_some())
            {
                return true;
            }
            let (effective_refs, anchors) =
                derive_ref_offsets_with(
                    &pg.points,
                    &pg.refs,
                    |name| {
                        resolve_ref_name_with_parts(name, &cache, name_parts)
                            .map(|resolved| resolved.anchors.clone())
                    },
                    |name| alt_index.get(name).to_vec(),
                );
            let (min_r, min_c, _, _) =
                composite_bounds(pg.pixels.as_ref(), &effective_refs, &cache, name_parts);
            let grid = composite_to_grid(&pg.pixels, &effective_refs, &cache, name_parts);
            cache.insert(
                pg.name.clone(),
                ResolvedGlyph {
                    grid,
                    origin_row: min_r,
                    origin_col: min_c,
                    anchors,
                },
            );
            progress = true;
            false
        });
    }

    let alt_index = AlternativesIndex::build(&cache);
    (cache, alt_index)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct GlyphComposite {
    pub width: u16,
    pub height: u16,
    pub own_offset_row: i16,
    pub own_offset_col: i16,
    pub layers: Vec<CompositeLayer>,
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
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
}

pub fn resolve_ref_name<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
) -> Option<&'a ResolvedGlyph> {
    resolve_ref_name_with_parts(name, named_glyphs, &NamePartsMap::new())
}

pub fn resolve_ref_name_with_parts<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> Option<&'a ResolvedGlyph> {
    if let Some(resolved) = named_glyphs.get(name) {
        return Some(resolved);
    }
    let subst = substitute_name_parts(name, name_parts);
    if let Some(resolved) = named_glyphs.get(&subst) {
        return Some(resolved);
    }
    if let Some(expanded) = expand_ref_names(&subst)
        && let Some(first) = expanded.first()
    {
        return named_glyphs.get(first);
    }
    None
}

/// Check that a ref name resolves to valid glyphs. For pattern refs, ALL
/// expansions must exist; returns false if any expansion is missing.
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn is_ref_valid(
    name: &str,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    if named_glyphs.contains_key(name) {
        return true;
    }
    let subst = substitute_name_parts(name, name_parts);
    if named_glyphs.contains_key(&subst) {
        return true;
    }
    if let Some(expanded) = expand_ref_names(&subst) {
        return expanded
            .into_iter()
            .all(|n| named_glyphs.contains_key(&n));
    }
    false
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
            let mut prefix = name.as_str();
            while let Some(colon_pos) = prefix.rfind(':') {
                prefix = &prefix[..colon_pos];
                map.entry(prefix.to_string())
                    .or_default()
                    .push((name.clone(), resolved.anchors.clone()));
            }
        }
        for alts in map.values_mut() {
            alts.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        Self { map }
    }

    pub fn get(&self, base_name: &str) -> &[(String, Vec<GlyphPoint>)] {
        self.map.get(base_name).map_or(&[], |v| v.as_slice())
    }
}

/// The effective (row, col) offset of a resolved ref within its owning glyph.
pub(crate) fn ref_effective_offset(gref: &GlyphRef, resolved: &ResolvedGlyph) -> (i32, i32) {
    (
        gref.row() as i32 + resolved.origin_row,
        gref.col() as i32 + resolved.origin_col,
    )
}

/// Bounding box (min_row, min_col, max_row, max_col) of a composite made of
/// `own_pixels` (if any) plus `refs`, each resolved against `named_glyphs`
/// via [`resolve_ref_name`] (which falls back to pattern expansion when a
/// ref name isn't a direct cache key).
pub(crate) fn composite_bounds(
    own_pixels: Option<&PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> (i32, i32, i32, i32) {
    let mut min_r: i32 = 0;
    let mut min_c: i32 = 0;
    let mut max_r: i32 = 0;
    let mut max_c: i32 = 0;

    if let Some(grid) = own_pixels {
        max_r = grid.height as i32;
        max_c = grid.width as i32;
    }

    for gref in refs {
        let resolved = if name_parts.is_empty() {
            resolve_ref_name(&gref.name, named_glyphs)
        } else {
            resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts)
        };
        if let Some(resolved) = resolved {
            let (eff_row, eff_col) = ref_effective_offset(gref, resolved);
            if resolved.grid.width != 0 && resolved.grid.height != 0 {
                min_r = min_r.min(eff_row);
                min_c = min_c.min(eff_col);
                max_r = max_r.max(eff_row + resolved.grid.height as i32);
                max_c = max_c.max(eff_col + resolved.grid.width as i32);
            }
        }
    }

    (min_r, min_c, max_r, max_c)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn compute_composite(
    body: &GlyphBody,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &AlternativesIndex,
) -> Option<GlyphComposite> {
    if body.refs.is_empty() {
        return None;
    }

    let (effective_refs, _) = derive_ref_offsets_with(
        &body.points,
        &body.refs,
        |name| {
            resolve_ref_name_with_parts(name, named_glyphs, name_parts)
                .map(|resolved| resolved.anchors.clone())
        },
        |name| alt_index.get(name).to_vec(),
    );

    let (min_r, min_c, max_r, max_c) = composite_bounds(
        body.pixels.as_ref(),
        &effective_refs,
        named_glyphs,
        name_parts,
    );

    let width = raster_dimension(min_c, max_c).max(1);
    let height = raster_dimension(min_r, max_r).max(1);

    let mut layers = Vec::new();
    for (idx, gref) in effective_refs.iter().enumerate() {
        if let Some(resolved) = resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts) {
            let (raster_row, raster_col) = ref_effective_offset(gref, resolved);
            layers.push(CompositeLayer {
                ref_idx: idx,
                resolved_name: gref.name.clone(),
                grid: resolved.grid.clone(),
                offset_row: saturating_i16(raster_row - min_r),
                offset_col: saturating_i16(raster_col - min_c),
                logical_offset_row: gref.row(),
                logical_offset_col: gref.col(),
                negated: gref.negated,
            });
        }
    }

    Some(GlyphComposite {
        width,
        height,
        own_offset_row: saturating_i16(-min_r),
        own_offset_col: saturating_i16(-min_c),
        layers,
    })
}

fn composite_to_grid(
    own_pixels: &Option<PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> PixelGrid {
    let (min_r, min_c, max_r, max_c) =
        composite_bounds(own_pixels.as_ref(), refs, named_glyphs, name_parts);

    let width = raster_dimension(min_c, max_c);
    let height = raster_dimension(min_r, max_r);
    let mut result = PixelGrid::new(width, height);

    // The owning glyph is the initial canvas. Refs are then applied in
    // document order, so a negated ref can actually cut the owning pixels.
    if let Some(grid) = own_pixels {
        let off_r = -min_r;
        let off_c = -min_c;
        for r in 0..grid.height as i32 {
            for c in 0..grid.width as i32 {
                let shape = grid.get(r as u16, c as u16);
                if !shape.is_empty() {
                    let dr = off_r + r;
                    let dc = off_c + c;
                    if dr >= 0 && dc >= 0 && dr < height as i32 && dc < width as i32 {
                        result.set(dr as u16, dc as u16, shape);
                    }
                }
            }
        }
    }

    for gref in refs {
        if let Some(resolved) = resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts) {
            let (eff_row, eff_col) = ref_effective_offset(gref, resolved);
            let off_r = eff_row - min_r;
            let off_c = eff_col - min_c;
            for r in 0..resolved.grid.height as i32 {
                for c in 0..resolved.grid.width as i32 {
                    let shape = resolved.grid.get(r as u16, c as u16);
                    if !shape.is_empty() {
                        let dr = off_r + r;
                        let dc = off_c + c;
                        if dr >= 0 && dc >= 0 && dr < height as i32 && dc < width as i32 {
                            if gref.negated {
                                let current = result.get(dr as u16, dc as u16);
                                // Subtraction must never create ink on an
                                // empty canvas. PixelShape can represent the
                                // common full-cell-minus-mask case exactly.
                                if !current.is_empty() {
                                    let out = if shape.shape_id() == 0 && shape.is_filled() {
                                        crate::pixel::PixelShape::EMPTY
                                    } else if current.shape_id() == 0 && current.is_filled() {
                                        shape.negated()
                                    } else if current == shape {
                                        crate::pixel::PixelShape::EMPTY
                                    } else {
                                        current
                                    };
                                    result.set(dr as u16, dc as u16, out);
                                }
                            } else {
                                result.set(dr as u16, dc as u16, shape);
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::PixelShape;

    fn filled_grid(w: u16, h: u16) -> PixelGrid {
        let mut g = PixelGrid::new(w, h);
        for r in 0..h {
            for c in 0..w {
                g.set(r, c, PixelShape::new(0, true));
            }
        }
        g
    }

    /// `compute_composite` resolves ref names via `resolve_ref_name`, which
    /// falls back to `expand_name_pattern` when a direct cache lookup misses
    /// (e.g. a ref pointing at a pattern name like "digit(0|1)" whose
    /// expansions, not the raw pattern string, are the cache keys).
    /// `composite_to_grid` used to do a bare `cache.get(&gref.name)` with no
    /// such fallback, so the same ref would render live via
    /// `compute_composite` but silently drop out of the flattened grid
    /// produced by `composite_to_grid`.
    #[test]
    fn composite_to_grid_resolves_pattern_refs_like_compute_composite() {
        let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();
        cache.insert(
            "digit0".to_string(),
            ResolvedGlyph {
                grid: filled_grid(2, 2),
                origin_row: 0,
                origin_col: 0,
                anchors: Vec::new(),
            },
        );

        let refs = vec![GlyphRef {
            name: "digit(0|1)".to_string(),
            offset: None,
            negated: false,
        }];

        // compute_composite resolves the pattern ref via resolve_ref_name's
        // fallback and includes the layer.
        let body = GlyphBody {
            refs: refs.clone(),
            ..GlyphBody::new()
        };
        let empty_parts = NamePartsMap::new();
        let composite = compute_composite(&body, &cache, &empty_parts, &AlternativesIndex::default()).expect("has refs");
        assert_eq!(
            composite.layers.len(),
            1,
            "compute_composite should include the pattern-resolved layer"
        );

        // composite_to_grid must resolve the same ref the same way, and thus
        // produce a non-empty grid with the layer's pixels present.
        let grid = composite_to_grid(&None, &refs, &cache, &empty_parts);
        assert_eq!(
            grid.get(0, 0),
            PixelShape::new(0, true),
            "composite_to_grid should include the pattern-resolved layer's pixels"
        );
    }

    #[test]
    fn adjoin_resolves_offset_from_points() {
        use crate::document_io;

        let input = "\
glyph target 10 10
....................
....................
....................
....................
....................
....................
....................
....................
....................
....................
point -blah 5 5

glyph container 12 12
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
point +blah 3 3
ref target
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();

        let docs = vec![&doc];
        let name_parts = NamePartsMap::new();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

        let container = resolved
            .get("container")
            .expect("container should be resolved");
        // target placed at offset (col=3-5, row=3-5) = (-2, -2).
        // Container own pixels 12×12 at (0,0), target 10×10 at (-2,-2).
        // Bounding box: min=-2, max=12 → total 14×14.
        assert_eq!(
            container.grid.width, 14,
            "width should be 14 (12 + 2 for negative offset)"
        );
        assert_eq!(
            container.grid.height, 14,
            "height should be 14 (12 + 2 for negative offset)"
        );
    }

    #[test]
    fn auto_offsets_are_rederived_without_mutating_source_refs() {
        use crate::document_io;

        let input = "\
glyph target 1 1
@@
point -join 0 0

glyph container 1 1
..
point +join 3 0
ref target
";
        let mut doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let name_parts = NamePartsMap::new();

        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        assert_eq!(resolved["container"].grid.width, 4);
        let container_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
                _ => None,
        })
        .unwrap();
        assert_eq!(container_body.refs[0].offset, None);
        let composite = compute_composite(container_body, &resolved, &name_parts, &_alt_idx).unwrap();
        assert_eq!(
            (
                composite.layers[0].offset_row - composite.own_offset_row,
                composite.layers[0].offset_col - composite.own_offset_col,
            ),
            (0, 3)
        );

        let target_body = doc
            .items
            .iter_mut()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "target" => Some(body),
                _ => None,
            })
            .unwrap();
        target_body.points[0].col = 2;
        target_body.points[0].col_end = 2;

        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        assert_eq!(resolved["container"].grid.width, 2);
        let container_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
                _ => None,
        })
        .unwrap();
        assert_eq!(container_body.refs[0].offset, None);
        let composite = compute_composite(container_body, &resolved, &name_parts, &_alt_idx).unwrap();
        assert_eq!(
            (
                composite.layers[0].offset_row - composite.own_offset_row,
                composite.layers[0].offset_col - composite.own_offset_col,
            ),
            (0, 1)
        );
    }

    #[test]
    fn anchors_are_forwarded_transitively_and_publish_after_consume() {
        use crate::document_io;

        let input = "\
glyph link 1 1
@@
point -join 0 0
point +join 2 0

glyph wrapped
ref link

glyph chain 1 1
..
point +join 0 0
ref wrapped
ref wrapped
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
        assert_eq!(resolved["chain"].grid.width, 3);
        assert!(resolved["chain"].grid.get(0, 0).is_filled());
        assert!(resolved["chain"].grid.get(0, 2).is_filled());
    }

    #[test]
    fn substituted_and_pattern_refs_resolve_in_all_container_shapes() {
        use crate::document_io;

        let input = "\
name-parts $base = stem

glyph stem 1 1
@@

glyph stem-a 1 1
@@

glyph stem-b 1 1
@@

glyph via-parts
ref $base

glyph via-pattern
ref stem-(a|b)

glyph pair-(a|b)
ref $base

glyph U+2800..2801
ref $base

glyph pipe-a|pipe-b
ref $base
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs = [&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

        for name in [
            "via-parts",
            "via-pattern",
            "pair-a",
            "pair-b",
            "U+2800",
            "U+2801",
            "pipe-a",
            "pipe-b",
        ] {
            assert!(
                resolved
                    .get(name)
                    .is_some_and(|g| g.grid.get(0, 0).is_filled()),
                "{name} did not resolve"
            );
        }
        assert!(is_ref_valid("$base", &resolved, &name_parts));
        assert!(is_ref_valid("stem-(a|b)", &resolved, &name_parts));
    }

    #[test]
    fn adjoin_resolves_minus_before_plus_ref_order() {
        use crate::document_io;

        let input = "\
glyph inner 8 8
................
................
................
................
................
................
................
................
point +center 4 4

glyph outer 12 12
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
point -center 6 6

glyph combo-plus-first
ref inner
ref outer

glyph combo-minus-first
ref outer
ref inner
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

        let pf = resolved.get("combo-plus-first").unwrap();
        let mf = resolved.get("combo-minus-first").unwrap();
        assert_eq!(
            (pf.grid.width, pf.grid.height),
            (mf.grid.width, mf.grid.height),
            "ref order should not affect resolved dimensions"
        );
    }

    #[test]
    fn anchor_range_parsing_and_size_match() {
        use crate::document_io;

        let input = "\
glyph target-wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 1..2 0..1

glyph target-narrow 2 2
@@@@
@@@@
anchor -join 0 0

glyph container 6 2
............
............
anchor +join 3..4 0..1
ref target-wide
ref target-narrow
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

        let container = resolved.get("container").unwrap();
        // target-wide has 2x2 anchor matching +join (2x2): offset = 3-1 = 2, placed at col 2.
        // target-narrow has 1x1 anchor, doesn't match +join (2x2), falls back to (0,0).
        // container own 6px + target-wide 4px at col 2 → max(6, 2+4) = 6.
        // target-narrow 2px at col 0 → still within bounds.
        assert_eq!(container.grid.width, 6);
    }

    #[test]
    fn alternative_glyph_selected_on_size_mismatch() {
        use crate::document_io;

        let input = "\
glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 0..1 0

glyph container 6 2
............
............
anchor +join 3..4 0
ref stem
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let name_parts = NamePartsMap::new();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        // stem has 1x1 anchor, +join is 2x1. stem:wide has 2x1 anchor → matches.
        let container = resolved.get("container").unwrap();
        // stem:wide is 4 wide, placed at col 3-0=3? No: +join col=3..4, -join col=0..1
        // offset = plus.col - minus.col = 3 - 0 = 3
        // container pixels: 6 wide, stem:wide at col 3 → extends to col 7.
        // total width = max(6, 3+4) = 7
        assert_eq!(container.grid.width, 7);

        // Verify via compute_composite that resolved_name is the alternative.
        let container_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
                _ => None,
            })
            .unwrap();
        let composite = compute_composite(container_body, &resolved, &name_parts, &_alt_idx).unwrap();
        assert_eq!(composite.layers[0].resolved_name, "stem:wide");
    }

    #[test]
    fn alternative_glyph_alphabetical_priority() {
        use crate::document_io;

        let input = "\
glyph base 1 1
@@
anchor -a 0 0

glyph base:zzz 2 2
@@@@
@@@@
anchor -a 0..1 0..1

glyph base:aaa 2 2
@@@@
@@@@
anchor -a 0..1 0..1

glyph host 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +a 1..2 1..2
ref base
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let name_parts = NamePartsMap::new();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        let host_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "host" => Some(body),
                _ => None,
            })
            .unwrap();
        let composite = compute_composite(host_body, &resolved, &name_parts, &_alt_idx).unwrap();
        // base:aaa comes before base:zzz alphabetically.
        assert_eq!(composite.layers[0].resolved_name, "base:aaa");
    }

    #[test]
    fn pattern_ref_selects_alternative_by_anchor_size() {
        use crate::document_io;

        let input = "\
name-parts $ab = a b

glyph enclosing 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph a-inner 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 2

glyph b-inner 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 2

glyph b-inner:compressed 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 1..2

glyph ($ab)-combo
ref enclosing
ref ($ab)-inner
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs = [&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

        let b_refs = vec![
            GlyphRef { name: "enclosing".to_string(), offset: None, negated: false },
            GlyphRef { name: "b-inner".to_string(), offset: None, negated: false },
        ];
        let (effective, _) = derive_ref_offsets_with(
            &[],
            &b_refs,
            |name| resolved.get(name).map(|r| r.anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
        );
        assert_eq!(
            effective[1].name, "b-inner:compressed",
            "b-inner:compressed should be selected because its -center (1x2) matches +center (1x2)"
        );
    }
}
