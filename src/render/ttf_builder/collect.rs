//! Gathering resolved glyph data for one build flavor: composite refs,
//! metrics and traced contours per glyph.

use super::contours::CachedContours;
use super::gsub::collect_gsub_data;
use super::*;

/// The explicit `advance`/`left`/`top` flags one glyph declares, in pixels as
/// written in the source; each is `None` when the glyph does not declare it.
#[derive(Clone, Copy)]
pub(super) struct GlyphMeta {
    advance: Option<u16>,
    left: Option<i16>,
    top: Option<i16>,
}

pub(super) type GlyphMetaMap = HashMap<String, GlyphMeta>;

/// [`derive_ref_offsets_with`](crate::ref_composite::derive_ref_offsets_with)
/// wired to the contour cache: anchors and alternatives are looked up from
/// `cache`/`alt_index`, declared anchors from `declared_anchors_map`.
fn derive_effective_refs(
    points: &[GlyphPoint],
    refs: &[GlyphRef],
    cache: &HashMap<String, CachedContours>,
    alt_index: &HashMap<String, Vec<(String, Vec<GlyphPoint>)>>,
    declared_anchors_map: &HashMap<String, Vec<GlyphPoint>>,
) -> (Vec<GlyphRef>, Vec<GlyphPoint>) {
    let (effective_refs, anchors, _issues) = crate::ref_composite::derive_ref_offsets_with(
        points,
        refs,
        |name| resolve_cached_ref(name, cache).map(|resolved| resolved.anchors.clone()),
        |name| alt_index.get(name).map_or_else(Vec::new, |v| v.clone()),
        |name| declared_anchors_map.get(name).cloned(),
    );
    (
        effective_refs,
        anchors.into_iter().map(|(p, _)| p).collect(),
    )
}

/// Scale pixel-space contours to font units, flipping y around the ascent
/// and shifting by `left_offset`/`top_offset` (already in font units).
fn scale_glyph_contours(
    contours: &[Vec<(f32, f32)>],
    scale: f32,
    ascent: u16,
    left_offset: i16,
    top_offset: i16,
) -> Vec<Vec<(i16, i16)>> {
    contours
        .iter()
        .map(|c| {
            c.iter()
                .map(|&(x, y)| {
                    (
                        (x * scale).round() as i16 + left_offset,
                        ((ascent as f32 - y) * scale).round() as i16 - top_offset,
                    )
                })
                .collect()
        })
        .collect()
}

/// Advance width, left offset, and top offset in font units, from explicit
/// `advance`/`left`/`top` flags when present, else the resolved raster width.
fn resolve_glyph_metrics(
    glyph_meta: &GlyphMetaMap,
    name: &str,
    resolved_width: u16,
    scale: f32,
    base_scale: f32,
) -> (u16, i16, i16) {
    let meta = glyph_meta.get(name);
    let advance_width = match meta.and_then(|m| m.advance) {
        Some(adv) => (adv as f32 * base_scale).round() as u16,
        None => (resolved_width as f32 * scale).round() as u16,
    };
    let left_offset = meta
        .and_then(|m| m.left)
        .map_or(0, |left| (left as f32 * base_scale).round() as i16);
    let top_offset = meta
        .and_then(|m| m.top)
        .map_or(0, |top| (top as f32 * base_scale).round() as i16);
    (advance_width, left_offset, top_offset)
}

/// Composite references for a resolved glyph in font units, or empty when
/// the glyph is forced inline.  Compensates for each component glyph's own
/// left/top offset so that the shift doesn't propagate into parent composites.
#[expect(clippy::too_many_arguments)]
fn build_composite_refs(
    resolved: &CachedContours,
    inline: bool,
    left_offset: i16,
    top_offset: i16,
    glyph_meta: &GlyphMetaMap,
    scale: f32,
    base_scale: f32,
    inline_glyphs: &HashSet<String>,
) -> Vec<CompositeRef> {
    if inline {
        return Vec::new();
    }
    let Some(comps) = &resolved.composite_components else {
        return Vec::new();
    };
    if comps
        .iter()
        .any(|(name, _, _)| inline_glyphs.contains(name.as_str()))
    {
        return Vec::new();
    }
    comps
        .iter()
        .map(|(name, dx, dy)| {
            let comp_meta = glyph_meta.get(name.as_str());
            let comp_left = comp_meta
                .and_then(|m| m.left)
                .map_or(0, |l| (l as f32 * base_scale).round() as i16);
            let comp_top = comp_meta
                .and_then(|m| m.top)
                .map_or(0, |t| (t as f32 * base_scale).round() as i16);
            CompositeRef {
                component_name: name.clone(),
                x_offset: ((*dx + left_offset as f32 / scale) * scale).round() as i16 - comp_left,
                y_offset: (-*dy * scale).round() as i16 - top_offset + comp_top,
            }
        })
        .collect()
}

pub(super) struct SharedFontInput {
    meta: FontMeta,
    scale: f32,
    all_items: Vec<DocumentItem>,
    declared_anchors_map: HashMap<String, Vec<GlyphPoint>>,
    gsub_data: GsubData,
    color_aliases: ColorAliasMap,
    glyph_aliases: crate::alias::AliasMap,
    glyph_meta: GlyphMetaMap,
    inline_glyphs: HashSet<String>,
    glyph_bodies: Vec<(String, GlyphBody)>,
}

pub(super) fn compute_shared_font_input(docs: &[&Document]) -> Option<SharedFontInput> {
    compute_shared_font_input_for(docs, crate::faces::FaceSet::collect(docs).primary())
}

/// The shared input for one face: its metadata, and an expansion with every
/// slice the face does not include already dropped.
pub(super) fn compute_shared_font_input_for(
    docs: &[&Document],
    face: &crate::faces::Face,
) -> Option<SharedFontInput> {
    if docs.is_empty() {
        return None;
    }

    let face_id = if face.id.is_empty() {
        None
    } else {
        Some(face.id.as_str())
    };
    let meta = FontMeta::for_face(docs, face_id);

    if meta.height() == 0 {
        eprintln!("error: `meta height` must be > 0");
        return None;
    }

    let scale = UNITS_PER_EM as f32 / meta.height() as f32;

    let name_parts = collect_name_parts(docs);
    let expansion = super::expand::expand_for(docs, &name_parts, face);
    let glyph_aliases = expansion.aliases;
    let all_items: Vec<DocumentItem> = expansion.items.into_iter().map(|e| e.item).collect();

    let mut declared_anchors_map: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
    for item in &all_items {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        {
            declared_anchors_map
                .entry(n.clone())
                .or_insert_with(|| body.points.clone());
        }
    }

    // GSUB expands `remap` patterns straight from the documents rather than
    // from `all_items`, so it is one of the two places that has to
    // canonicalize aliases for itself.
    let gsub_data = collect_gsub_data(docs, &name_parts, &glyph_aliases);
    let color_aliases = collect_color_aliases(docs);

    let mut glyph_meta: GlyphMetaMap = HashMap::new();
    let mut inline_glyphs: HashSet<String> = HashSet::new();
    let mut glyph_bodies: Vec<(String, GlyphBody)> = Vec::new();
    let mut seen_bodies: HashSet<String> = HashSet::new();
    for item in &all_items {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        {
            if body.advance.is_some() || body.left.is_some() || body.top.is_some() {
                glyph_meta.insert(
                    n.clone(),
                    GlyphMeta {
                        advance: body.advance,
                        left: body.left,
                        top: body.top,
                    },
                );
            }
            if body.inline {
                inline_glyphs.insert(n.clone());
            }
            if seen_bodies.insert(n.clone()) {
                glyph_bodies.push((n.clone(), body.clone()));
            }
        }
    }

    Some(SharedFontInput {
        meta,
        scale,
        all_items,
        declared_anchors_map,
        gsub_data,
        color_aliases,
        glyph_aliases,
        glyph_meta,
        inline_glyphs,
        glyph_bodies,
    })
}

pub(super) fn collect_glyph_data_cached(
    docs: &[&Document],
    bitmap: bool,
    contour_cache: Option<&mut ContourCache>,
) -> Option<CollectedFontData> {
    let shared = compute_shared_font_input(docs)?;
    collect_glyph_data_with_shared(&shared, bitmap, contour_cache)
}

pub(super) fn collect_glyph_data_with_shared(
    shared: &SharedFontInput,
    bitmap: bool,
    mut contour_cache: Option<&mut ContourCache>,
) -> Option<CollectedFontData> {
    let meta = &shared.meta;
    let scale = shared.scale;
    let all_items = &shared.all_items;
    let declared_anchors_map = &shared.declared_anchors_map;
    let gsub_data = &shared.gsub_data;
    let color_aliases = &shared.color_aliases;
    let glyph_meta = &shared.glyph_meta;
    let inline_glyphs = &shared.inline_glyphs;
    let glyph_bodies = &shared.glyph_bodies;

    let (mut cache, pending) = {
        let cc = &mut contour_cache;
        crate::render::glyph_cache::seed_cache(
            all_items,
            |pixels| CachedContours::from_grid(pixels, bitmap, cc.as_deref_mut()),
            CachedContours::empty,
        )
    };
    {
        let cc = &mut contour_cache;
        crate::render::glyph_cache::resolve_pending(
            &mut cache,
            pending,
            |name| declared_anchors_map.get(name).cloned(),
            |pg, effective_refs, cache| {
                CachedContours::from_components(
                    pg.pixels.as_ref(),
                    effective_refs,
                    cache,
                    bitmap,
                    cc.as_deref_mut(),
                    pg.scale,
                )
                .unwrap_or_else(|| {
                    if let Some(grid) = &pg.pixels {
                        CachedContours::from_grid(grid, bitmap, cc.as_deref_mut())
                    } else {
                        CachedContours::empty()
                    }
                })
            },
            |_, _| {},
        );
    }

    let glyph_bodies_map: HashMap<&str, &GlyphBody> =
        glyph_bodies.iter().map(|(n, b)| (n.as_str(), b)).collect();

    let mut glyph_data: Vec<CollectedGlyph> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    for item in all_items {
        let DocumentItem::Map {
            char_repr, glyph, ..
        } = item
        else {
            continue;
        };

        // Canonicalized, so a character mapped through an alias reaches the
        // target's `CollectedGlyph` and the two names share one glyph id.
        let mut pairs = expand_map_pairs(char_repr, glyph);
        shared.glyph_aliases.canonicalize_pairs(&mut pairs);
        for (cp, glyph_name) in &pairs {
            let Some(resolved) = cache.get(glyph_name.as_str()) else {
                continue;
            };

            let glyph_scale = scale / resolved.scale as f32;
            let (advance_width, left_offset, top_offset) =
                resolve_glyph_metrics(glyph_meta, glyph_name, resolved.width, glyph_scale, scale);
            let font_contours = scale_glyph_contours(
                &resolved.contours,
                glyph_scale,
                meta.ascent() * resolved.scale as u16,
                left_offset,
                top_offset,
            );
            let composite_refs = build_composite_refs(
                resolved,
                inline_glyphs.contains(glyph_name.as_str()),
                left_offset,
                top_offset,
                glyph_meta,
                glyph_scale,
                scale,
                inline_glyphs,
            );

            let is_mark = glyph_bodies_map
                .get(glyph_name.as_str())
                .is_some_and(|b| b.mark);
            let glyph_anchors = cache
                .get(glyph_name.as_str())
                .map(|c| c.anchors.clone())
                .unwrap_or_default();
            let declared_anchors = glyph_bodies_map
                .get(glyph_name.as_str())
                .map(|b| b.points.clone())
                .unwrap_or_default();

            seen_names.insert(glyph_name.clone());
            glyph_data.push(CollectedGlyph {
                name: glyph_name.clone(),
                codepoints: vec![*cp],
                advance_width,
                contours: font_contours,
                composite_refs,
                color_layers: Vec::new(),
                mark: is_mark,
                resolved_anchors: glyph_anchors,
                declared_anchors,
                left_offset,
                top_offset,
            });
        }
    }

    // One entry per glyph *name*, carrying every character that reaches it.
    //
    // The order is `(lowest codepoint, name)`, and both halves matter. Sorting
    // by codepoint keeps the runs that make a format 4 cmap compact. Falling
    // back to the name makes the order total, so it does not depend on the
    // order the maps happened to be walked in — which is what lets two faces
    // of a collection share `glyf`, `loca` and `hmtx`. Unmapped glyphs sort
    // last, by name.
    {
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut merged: Vec<CollectedGlyph> = Vec::with_capacity(glyph_data.len());
        for glyph in glyph_data {
            match by_name.get(&glyph.name) {
                Some(&i) => {
                    let existing: &mut CollectedGlyph = &mut merged[i];
                    existing.codepoints.extend(glyph.codepoints);
                }
                None => {
                    by_name.insert(glyph.name.clone(), merged.len());
                    merged.push(glyph);
                }
            }
        }
        for glyph in &mut merged {
            glyph.codepoints.sort_unstable();
            glyph.codepoints.dedup();
        }
        merged.sort_by(|a, b| {
            let key = |g: &CollectedGlyph| {
                (
                    g.codepoints.first().copied().unwrap_or(u32::MAX),
                    g.name.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
        glyph_data = merged;
    }

    let mut remap_referenced: HashSet<&str> = HashSet::new();
    for remaps in gsub_data.remap_sets.values() {
        for r in remaps {
            for seq in &r.source {
                for name in seq {
                    remap_referenced.insert(name.as_str());
                }
            }
            for seq in &r.target {
                for name in seq {
                    remap_referenced.insert(name.as_str());
                }
            }
            for names in &r.lookbehind {
                for name in names {
                    remap_referenced.insert(name.as_str());
                }
            }
            for names in &r.lookahead {
                for name in names {
                    remap_referenced.insert(name.as_str());
                }
            }
        }
    }

    let mut extra_name_set: HashSet<String> = remap_referenced
        .iter()
        .filter(|n| !seen_names.contains(**n))
        .map(|n| n.to_string())
        .collect();
    for item in all_items {
        if let DocumentItem::Glyph {
            name: GlyphName(name),
            body,
        } = item
            && body.sticky
            && !seen_names.contains(name)
        {
            extra_name_set.insert(name.clone());
        }
    }

    // Include alternative glyphs needed for anchor-based features:
    // 1. Base alts: base lacks own "+X" but base:alt has it.
    // 2. Mark alts: mark has "-X" of one size, mark:alt has "-X" of a
    //    different size that matches some base's "+X".
    if !gsub_data.anchor_features.is_empty() {
        let anchor_names: Vec<&str> = gsub_data
            .anchor_features
            .iter()
            .map(|(_, _, a)| a.as_str())
            .collect();
        let alt_index = build_cached_alternatives(&cache);

        // 1. Base alts
        for (base_name, alts) in &alt_index {
            if !seen_names.contains(base_name) {
                continue;
            }
            let declared = glyph_bodies_map
                .get(base_name.as_str())
                .map(|b| &b.points[..])
                .unwrap_or(&[]);
            for anchor_name in &anchor_names {
                let plus_name = format!("+{anchor_name}");
                if declared.iter().any(|p| p.position == plus_name) {
                    continue;
                }
                for (alt_name, alt_anchors) in alts {
                    if alt_anchors.iter().any(|p| p.position == plus_name)
                        && !seen_names.contains(alt_name)
                    {
                        extra_name_set.insert(alt_name.clone());
                    }
                }
            }
        }

        // 2. Mark alts: include mark:alt when its "-X" has a different
        //    size from the primary mark's "-X".
        for (mark_name, alts) in &alt_index {
            if !seen_names.contains(mark_name) {
                continue;
            }
            let mark_body = match glyph_bodies_map.get(mark_name.as_str()) {
                Some(b) if b.mark => *b,
                _ => continue,
            };
            for anchor_name in &anchor_names {
                let minus_name = format!("-{anchor_name}");
                let Some(mark_minus) = mark_body.points.iter().find(|p| p.position == minus_name)
                else {
                    continue;
                };
                for (alt_name, alt_anchors) in alts {
                    if seen_names.contains(alt_name) || extra_name_set.contains(alt_name) {
                        continue;
                    }
                    if let Some(alt_minus) = alt_anchors.iter().find(|p| p.position == minus_name)
                        && !alt_minus.size_matches(mark_minus)
                    {
                        extra_name_set.insert(alt_name.clone());
                    }
                }
            }
        }
    }

    let mut extra_names: Vec<String> = extra_name_set.into_iter().collect();
    extra_names.sort();

    for glyph_name in &extra_names {
        let empty_cached = CachedContours::empty();
        let resolved = cache.get(glyph_name.as_str()).unwrap_or(&empty_cached);
        let glyph_scale = scale / resolved.scale as f32;
        let (advance_width, left_offset, top_offset) =
            resolve_glyph_metrics(glyph_meta, glyph_name, resolved.width, glyph_scale, scale);
        let font_contours = scale_glyph_contours(
            &resolved.contours,
            glyph_scale,
            meta.ascent() * resolved.scale as u16,
            left_offset,
            top_offset,
        );
        let composite_refs = build_composite_refs(
            resolved,
            inline_glyphs.contains(glyph_name.as_str()),
            left_offset,
            top_offset,
            glyph_meta,
            glyph_scale,
            scale,
            inline_glyphs,
        );

        let is_mark = glyph_bodies_map
            .get(glyph_name.as_str())
            .is_some_and(|b| b.mark);
        let glyph_anchors = cache
            .get(glyph_name.as_str())
            .map(|c| c.anchors.clone())
            .unwrap_or_default();
        let declared_anchors = glyph_bodies_map
            .get(glyph_name.as_str())
            .map(|b| b.points.clone())
            .unwrap_or_default();

        glyph_data.push(CollectedGlyph {
            name: glyph_name.clone(),
            codepoints: Vec::new(),
            advance_width,
            contours: font_contours,
            composite_refs,
            color_layers: Vec::new(),
            mark: is_mark,
            resolved_anchors: glyph_anchors,
            declared_anchors,
            left_offset,
            top_offset,
        });
    }

    // Ensure composite component glyphs are included in the font
    let mut all_names: HashSet<String> = glyph_data.iter().map(|g| g.name.clone()).collect();
    let mut component_extras: Vec<CollectedGlyph> = Vec::new();
    for g in &glyph_data {
        for cr in &g.composite_refs {
            if !all_names.contains(&cr.component_name) {
                all_names.insert(cr.component_name.clone());
                let empty_cached = CachedContours::empty();
                let resolved = cache
                    .get(cr.component_name.as_str())
                    .unwrap_or(&empty_cached);
                let comp_glyph_scale = scale / resolved.scale as f32;
                let font_contours = scale_glyph_contours(
                    &resolved.contours,
                    comp_glyph_scale,
                    meta.ascent() * resolved.scale as u16,
                    0,
                    0,
                );
                let advance_width = (resolved.width as f32 * comp_glyph_scale).round() as u16;
                component_extras.push(CollectedGlyph {
                    name: cr.component_name.clone(),
                    codepoints: Vec::new(),
                    advance_width,
                    contours: font_contours,
                    composite_refs: Vec::new(),
                    color_layers: Vec::new(),
                    mark: false,
                    resolved_anchors: Vec::new(),
                    declared_anchors: Vec::new(),
                    left_offset: 0,
                    top_offset: 0,
                });
            }
        }
    }
    glyph_data.append(&mut component_extras);

    if glyph_data.is_empty() {
        return None;
    }

    // Build color palette: collect all unique RGBA colors used across fills
    let mut palette_colors: Vec<Rgba> = Vec::new();
    let mut color_to_index: HashMap<Rgba, u16> = HashMap::new();
    // Build per-glyph color layers
    let color_alt_index = build_cached_alternatives(&cache);
    for g in &mut glyph_data {
        let Some(body) = glyph_bodies_map.get(g.name.as_str()) else {
            continue;
        };
        let has_fill_or_vis = body
            .refs
            .iter()
            .any(|r| r.fill.is_some() || r.visibility.is_some());
        if !has_fill_or_vis {
            continue;
        }

        let (effective_refs, _) = derive_effective_refs(
            &body.points,
            &body.refs,
            &cache,
            &color_alt_index,
            declared_anchors_map,
        );

        let color_glyph_scale = scale / body.scale as f32;
        let color_ascent = meta.ascent() * body.scale as u16;
        let g_meta = glyph_meta.get(&g.name);
        let left_offset = g_meta
            .and_then(|m| m.left)
            .map_or(0, |left| (left as f32 * scale).round() as i16);
        let top_offset = g_meta
            .and_then(|m| m.top)
            .map_or(0, |top| (top as f32 * scale).round() as i16);

        // A `negated` ref draws nothing of its own — it only removes area from
        // the layers under it.  This path splits a composite into per-layer
        // contour sets, so each surviving layer has to be traced against the
        // negated layers that follow it, and negated refs contribute no layer
        // of their own.  Cutting is per pass: a monoonly negation cannot reach
        // the coloronly layers, which are not present when it is drawn.
        let has_negated = body.refs.iter().any(|r| r.negated);
        let ref_layers: Vec<Option<(PixelGrid, i32, i32)>> = if has_negated {
            effective_refs
                .iter()
                .map(|eref| {
                    let ref_cached = resolve_cached_ref(&eref.name, &cache)?;
                    let grid = ref_cached.grid.as_ref()?;
                    let (rs, ps) = (ref_cached.scale.max(1), body.scale.max(1));
                    let scaled = if rs == ps {
                        grid.clone()
                    } else {
                        grid.rescale(rs, ps)
                    };
                    let (row, col) = ref_cached.placed_at(eref.row() as i32, eref.col() as i32, ps);
                    Some((scaled, row, col))
                })
                .collect()
        } else {
            Vec::new()
        };
        let ref_vis: Vec<LayerVisibility> = (0..effective_refs.len())
            .map(|ri| {
                let orig_ref = &body.refs[ri.min(body.refs.len() - 1)];
                effective_visibility(orig_ref.visibility, orig_ref.fill.as_ref(), color_aliases)
            })
            .collect();
        // Negated layers drawn after ref `from` (all of them, for own pixels),
        // restricted to the pass that `skip` selects.
        let negated_after = |from: Option<usize>, skip: LayerVisibility| {
            let start = from.map_or(0, |i| i + 1);
            (start..ref_layers.len())
                .filter(|&j| body.refs[j.min(body.refs.len() - 1)].negated && ref_vis[j] != skip)
                .filter_map(|j| ref_layers[j].as_ref().map(|(g, r, c)| (g, *r, *c, true)))
                .collect::<Vec<_>>()
        };
        // Trace one positive layer minus the negated layers that follow it.
        // `None` when nothing cuts this layer, so a layer no negation reaches
        // keeps its own exactly traced contours instead of being re-traced.
        let cut_contours =
            |grid: &PixelGrid, row: i32, col: i32, negs: Vec<(&PixelGrid, i32, i32, bool)>| {
                if negs.is_empty() {
                    return None;
                }
                let mut layers = vec![(grid, row, col, false)];
                layers.extend(negs);
                Some(track_contour_multi_diff_at(&layers, PX_SUBPIXEL))
            };

        // Collect foreground contours (own pixels + refs without fill or with fill=fg)
        // and separate color layers (refs with non-fg fill).
        let mut fg_contours: Vec<Vec<(i16, i16)>> = Vec::new();

        if let Some(ref own_grid) = body.pixels
            && !own_grid.is_all_empty()
        {
            let c = has_negated
                .then(|| {
                    cut_contours(
                        own_grid,
                        0,
                        0,
                        negated_after(None, LayerVisibility::MonoOnly),
                    )
                })
                .flatten()
                .unwrap_or_else(|| track_contour(own_grid, PX_SUBPIXEL));
            fg_contours.extend(scale_glyph_contours(
                &c,
                color_glyph_scale,
                color_ascent,
                left_offset,
                top_offset,
            ));
        }

        for (ri, eref) in effective_refs.iter().enumerate() {
            let orig_ref = &body.refs[ri.min(body.refs.len() - 1)];
            let fill = orig_ref.fill.as_ref();
            let vis = ref_vis[ri];
            if vis == LayerVisibility::MonoOnly || orig_ref.negated {
                continue;
            }

            let Some(ref_cached) = resolve_cached_ref(&eref.name, &cache) else {
                continue;
            };
            let dx = eref.col() as f32;
            let dy = eref.row() as f32;
            let rsf = body.scale as f32 / ref_cached.scale.max(1) as f32;

            let cut = has_negated
                .then(|| {
                    let (grid, row, col) = ref_layers[ri].as_ref()?;
                    cut_contours(
                        grid,
                        *row,
                        *col,
                        negated_after(Some(ri), LayerVisibility::MonoOnly),
                    )
                })
                .flatten();
            let layer_contours: Vec<Vec<(i16, i16)>> = if let Some(c) = cut {
                scale_glyph_contours(&c, color_glyph_scale, color_ascent, left_offset, top_offset)
            } else {
                ref_cached
                    .contours
                    .iter()
                    .map(|c| {
                        c.iter()
                            .map(|&(x, y)| {
                                (
                                    ((x * rsf + dx) * color_glyph_scale).round() as i16
                                        + left_offset,
                                    ((color_ascent as f32 - (y * rsf + dy)) * color_glyph_scale)
                                        .round() as i16
                                        - top_offset,
                                )
                            })
                            .collect()
                    })
                    .collect()
            };

            if layer_contours.is_empty() {
                continue;
            }

            let is_fg = fill.is_none() || fill.is_some_and(|f| f.color == "fg");
            if is_fg {
                fg_contours.extend(layer_contours);
            } else {
                let f = fill.unwrap();
                let palette_index = if let Some(rgba) = resolve_fill_rgba(f, color_aliases) {
                    *color_to_index.entry(rgba.clone()).or_insert_with(|| {
                        let idx = palette_colors.len() as u16;
                        palette_colors.push(rgba);
                        idx
                    })
                } else {
                    0xFFFF
                };
                g.color_layers.push(CollectedColorLayer {
                    contours: layer_contours,
                    palette_index,
                });
            }
        }

        if !fg_contours.is_empty() {
            g.color_layers.insert(
                0,
                CollectedColorLayer {
                    contours: fg_contours,
                    palette_index: 0xFFFF,
                },
            );
        }

        // Rebuild fallback contours: only non-coloronly layers
        let mut fallback_contours: Vec<Vec<(i16, i16)>> = Vec::new();
        if let Some(ref own_grid) = body.pixels
            && !own_grid.is_all_empty()
        {
            let c = has_negated
                .then(|| {
                    cut_contours(
                        own_grid,
                        0,
                        0,
                        negated_after(None, LayerVisibility::ColorOnly),
                    )
                })
                .flatten()
                .unwrap_or_else(|| track_contour(own_grid, PX_SUBPIXEL));
            fallback_contours.extend(scale_glyph_contours(
                &c,
                color_glyph_scale,
                color_ascent,
                left_offset,
                top_offset,
            ));
        }
        for (ri, eref) in effective_refs.iter().enumerate() {
            let orig_ref = &body.refs[ri.min(body.refs.len() - 1)];
            let vis = ref_vis[ri];
            if vis == LayerVisibility::ColorOnly || orig_ref.negated {
                continue;
            }
            let cut = has_negated
                .then(|| {
                    let (grid, row, col) = ref_layers[ri].as_ref()?;
                    cut_contours(
                        grid,
                        *row,
                        *col,
                        negated_after(Some(ri), LayerVisibility::ColorOnly),
                    )
                })
                .flatten();
            if let Some(c) = cut {
                fallback_contours.extend(scale_glyph_contours(
                    &c,
                    color_glyph_scale,
                    color_ascent,
                    left_offset,
                    top_offset,
                ));
                continue;
            }
            let Some(ref_cached) = resolve_cached_ref(&eref.name, &cache) else {
                continue;
            };
            let dx = eref.col() as f32;
            let dy = eref.row() as f32;
            let fb_rsf = body.scale as f32 / ref_cached.scale.max(1) as f32;
            for c in &ref_cached.contours {
                fallback_contours.push(
                    c.iter()
                        .map(|&(x, y)| {
                            (
                                ((x * fb_rsf + dx) * color_glyph_scale).round() as i16
                                    + left_offset,
                                ((color_ascent as f32 - (y * fb_rsf + dy)) * color_glyph_scale)
                                    .round() as i16
                                    - top_offset,
                            )
                        })
                        .collect(),
                );
            }
        }
        g.contours = fallback_contours;
        g.composite_refs.clear();
    }

    // Sort palette colors for determinism
    {
        let mut sorted_colors: Vec<Rgba> = palette_colors.clone();
        sorted_colors.sort();
        sorted_colors.dedup();
        let old_to_new: HashMap<u16, u16> = palette_colors
            .iter()
            .enumerate()
            .map(|(old_idx, rgba)| {
                let new_idx = sorted_colors.iter().position(|c| c == rgba).unwrap() as u16;
                (old_idx as u16, new_idx)
            })
            .collect();
        palette_colors = sorted_colors;
        for g in &mut glyph_data {
            for layer in &mut g.color_layers {
                if layer.palette_index != 0xFFFF {
                    layer.palette_index = old_to_new[&layer.palette_index];
                }
            }
        }
    }

    // .notdef takes one GID slot, so usable glyph count is u16::MAX - 1 = 65534.
    const MAX_GLYPHS: usize = u16::MAX as usize - 1;
    if glyph_data.len() > MAX_GLYPHS {
        eprintln!(
            "error: too many glyphs ({}, max {})",
            glyph_data.len(),
            MAX_GLYPHS,
        );
        return None;
    }

    Some((
        meta.clone(),
        scale,
        glyph_data,
        gsub_data.clone(),
        palette_colors,
    ))
}
