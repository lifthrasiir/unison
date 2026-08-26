//! Gathering resolved glyph data for one build flavor: composite refs,
//! metrics and traced contours per glyph.

use super::contours::CachedContours;
use super::gsub::collect_gsub_data;
use super::*;
use crate::render::glyph_cache::CANCEL_STRIDE;

/// One glyph's declared box, in the terms this stage works in: an advance and
/// the two *bearings* of the box's corner, in pixels, each `None` when the
/// glyph leaves it to be derived. This is
/// [`GlyphBody::declared_origin`](crate::document::GlyphBody::declared_origin)
/// negated — the origin says where the box starts inside the grid, a bearing
/// says how far the grid sits from the pen, and an outline is shifted by the
/// latter.
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
    parent_scale: u8,
) -> (Vec<GlyphRef>, Vec<GlyphPoint>) {
    let origin_of = |name: &str| {
        use crate::render::glyph_cache::CachedGlyphEntry;
        resolve_cached_ref(name, cache).map_or((0, 0), |c| c.declared_origin())
    };
    let (mut effective_refs, anchors, _issues) = crate::ref_composite::derive_ref_offsets_with(
        points,
        refs,
        parent_scale,
        |name| resolve_cached_ref(name, cache).map(|resolved| resolved.anchors.clone()),
        |name| alt_index.get(name).map_or_else(Vec::new, |v| v.clone()),
        |name| declared_anchors_map.get(name).cloned(),
        origin_of,
    );
    crate::ref_composite::rebase_offsets_to_box(&mut effective_refs, parent_scale, origin_of);
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

/// Advance width, left offset, and top offset in font units, from the declared
/// box when the glyph states one, else the resolved raster's *right edge*.
///
/// That fallback is the box's far edge and not its size, the same answer
/// [`GlyphBody::declared_extent`](crate::document::GlyphBody::declared_extent)
/// gives a grid: the origin has already moved the near edge, and `left_offset`
/// is exactly that move (negated, in font units), so adding it is what keeps a
/// column given away as a bearing out of the advance.
fn resolve_glyph_metrics(
    glyph_meta: &GlyphMetaMap,
    name: &str,
    resolved_width: u16,
    scale: f32,
    base_scale: f32,
) -> (u16, i16, i16) {
    let meta = glyph_meta.get(name);
    let left_offset = meta
        .and_then(|m| m.left)
        .map_or(0, |left| (left as f32 * base_scale).round() as i16);
    let top_offset = meta
        .and_then(|m| m.top)
        .map_or(0, |top| (top as f32 * base_scale).round() as i16);
    let advance_width = match meta.and_then(|m| m.advance) {
        Some(adv) => (adv as f32 * base_scale).round() as u16,
        None => ((resolved_width as f32 * scale).round() as i32 + left_offset as i32)
            .clamp(0, u16::MAX as i32) as u16,
    };
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

#[cfg(any(feature = "editor", test))]
pub(super) fn compute_shared_font_input(
    docs: &[&Document],
    cancel: &crate::cancel::CancelToken,
) -> Option<SharedFontInput> {
    compute_shared_font_input_for(docs, crate::faces::FaceSet::collect(docs).primary(), cancel)
}

/// What every face needs out of the documents before anything is traced: its
/// own `meta`, its own GSUB, and the items a slice qualifier left it.
struct FaceInput {
    meta: FontMeta,
    scale: f32,
    all_items: Vec<DocumentItem>,
    gsub_data: GsubData,
    glyph_aliases: crate::alias::AliasMap,
}

/// Where the expansion comes from: computed here, or lent by a caller that
/// already has one.
///
/// It is the *union* expansion either way — face-independent, see
/// [`crate::faces::FaceSet::union`] — and the face this is being computed for
/// only decides which of its items survive [`face_items`].
///
/// A cancelled run returns `None`, like any other input that cannot produce a
/// font: name expansion is the one stage here big enough to be worth aborting,
/// so the token is checked around it rather than inside it.
///
/// The editor rebuilds the font and the derived data from the same edit, and
/// both used to expand for themselves — the larger half of what either costs,
/// paid twice. Lending is what makes it once; see
/// [`crate::app::UniformApp::rebuild`].
enum ExpansionSource<'a> {
    Compute,
    Lent(&'a super::expand::Expansion),
}

/// One face's view of the union expansion: the items it does not include,
/// dropped.
///
/// The expansion itself is face-independent — see
/// [`crate::faces::FaceSet::union`] — so this is where a face becomes one
/// again. An expanded item carries the single slice it was stated for
/// (`expand_inner` emits one copy per stated slice), which is all the filter
/// needs; an unqualified item is the base slice and belongs to every face.
///
/// Glyphs are deliberately untouched: every face draws from the same glyph set,
/// and what a slice changes is which character reaches which glyph.
pub(super) fn face_items<'a>(
    items: impl Iterator<Item = &'a DocumentItem>,
    face: &crate::faces::Face,
) -> Vec<DocumentItem> {
    items
        .filter(|item| match item.slice_qualifier() {
            [] => true,
            qual => qual.iter().any(|s| face.includes(Some(s.as_str()))),
        })
        .cloned()
        .collect()
}

fn compute_face_input(
    docs: &[&Document],
    face: &crate::faces::Face,
    cancel: &crate::cancel::CancelToken,
    source: ExpansionSource<'_>,
) -> Option<FaceInput> {
    if docs.is_empty() || cancel.is_cancelled() {
        return None;
    }

    let face_id = if face.id.is_empty() {
        None
    } else {
        Some(face.id.as_str())
    };
    let meta = FontMeta::for_face(docs, face_id);

    // Refused rather than reported: `issues::directives::check_meta` is what
    // says "meta height is 0", with the line it is written on, and a build
    // printing a second copy of it with no location was the only thing this
    // ever added.
    if meta.height() == 0 {
        return None;
    }

    let scale = UNITS_PER_EM as f32 / meta.height() as f32;

    let name_parts = collect_name_parts(docs);
    let union = crate::faces::FaceSet::collect(docs).union();
    let (glyph_aliases, all_items): (crate::alias::AliasMap, Vec<DocumentItem>) = match source {
        ExpansionSource::Compute => {
            let expansion = super::expand::expand_for(docs, &name_parts, &union);
            if cancel.is_cancelled() {
                return None;
            }
            let items = face_items(expansion.items(), face);
            (expansion.aliases, items)
        }
        // Copied rather than taken: the lender is still reading it — validation
        // walks the same expansion beside this build, and the glyph cache
        // consumes it after. A copy of the items is a fraction of what
        // producing them costs.
        ExpansionSource::Lent(expansion) => (
            expansion.aliases.clone(),
            face_items(expansion.items(), face),
        ),
    };

    // GSUB expands `remap` patterns straight from the documents rather than
    // from `all_items`, so it is one of the two places that has to
    // canonicalize aliases for itself.
    let mut gsub_data = collect_gsub_data(docs, &name_parts, &glyph_aliases);
    // Variation sequences, on the other hand, have to be read *after* the
    // slice expansion, since a pair can be stated for one slice only. The
    // selector set is read from the raw documents on purpose: see
    // `GsubData::uvs_selectors` on why glyph order cannot vary by face.
    gsub_data.uvs_selectors = super::gsub::collect_uvs_selectors(docs);
    gsub_data.uvs_pairs = super::gsub::collect_uvs_pairs(&all_items, &glyph_aliases);

    Some(FaceInput {
        meta,
        scale,
        all_items,
        gsub_data,
        glyph_aliases,
    })
}

/// The shared input for one face: its metadata, and an expansion with every
/// slice the face does not include already dropped.
pub(super) fn compute_shared_font_input_for(
    docs: &[&Document],
    face: &crate::faces::Face,
    cancel: &crate::cancel::CancelToken,
) -> Option<SharedFontInput> {
    shared_font_input(docs, face, cancel, ExpansionSource::Compute)
}

/// The same, from an expansion the caller already has.
pub(super) fn compute_shared_font_input_from(
    docs: &[&Document],
    face: &crate::faces::Face,
    expansion: &super::expand::Expansion,
    cancel: &crate::cancel::CancelToken,
) -> Option<SharedFontInput> {
    shared_font_input(docs, face, cancel, ExpansionSource::Lent(expansion))
}

fn shared_font_input(
    docs: &[&Document],
    face: &crate::faces::Face,
    cancel: &crate::cancel::CancelToken,
    source: ExpansionSource<'_>,
) -> Option<SharedFontInput> {
    let FaceInput {
        meta,
        scale,
        all_items,
        gsub_data,
        glyph_aliases,
    } = compute_face_input(docs, face, cancel, source)?;

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
            // The declared box, in the terms this stage works in: the origin is
            // where the box's corner sits inside the grid, and the bearings are
            // the blank the other way round, which is what an outline is
            // shifted by. Read through `declared_origin` so both spellings
            // reach here — see `GlyphBody`.
            let (origin_c, origin_r) = body.declared_origin();
            let advance = body.stated_advance();
            if advance.is_some() || (origin_c, origin_r) != (0, 0) {
                glyph_meta.insert(
                    n.clone(),
                    GlyphMeta {
                        advance,
                        left: Some(-origin_c),
                        top: Some(-origin_r),
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

/// What [`collect_face_cmap`] returns.
pub(super) struct FaceCmap {
    pub(super) meta: FontMeta,
    pub(super) scale: f32,
    /// Which codepoints each glyph name claims, for this face alone.
    pub(super) per_name: HashMap<String, Vec<u32>>,
    pub(super) gsub_data: GsubData,
}

/// Everything a *secondary* face contributes to its own font: its metadata,
/// its GSUB, and the characters each glyph name claims — and no geometry.
///
/// `build_faces` takes only the cmap out of a per-face collection; the glyphs,
/// and so every glyph id, come from the shared union store. And expansion
/// never filters a glyph by slice, so the union and every face would trace
/// *exactly the same glyph set*: running the full collector per face redid the
/// whole trace to keep one `HashMap`, which is where a two-face build used to
/// spend two thirds of its CPU. This is that map, and nothing else.
///
/// The expansion is the caller's — the union one, which every face's view is
/// taken out of by [`face_items`]. A face used to expand for itself here, which
/// on a font this size cost a quarter of a second per face for a result that
/// differed from the union's only in which `map` lines survived.
///
/// The full collector drops a glyph it cannot resolve, and so its cmap entry.
/// That rule is not lost here: a name this map claims still has to appear in
/// the union store to reach the font, and the union traces the same glyphs.
pub(super) fn collect_face_cmap(
    docs: &[&Document],
    face: &crate::faces::Face,
    expansion: &super::expand::Expansion,
    cancel: &crate::cancel::CancelToken,
) -> Option<FaceCmap> {
    let shared = compute_face_input(docs, face, cancel, ExpansionSource::Lent(expansion))?;
    let mut per_name: HashMap<String, Vec<u32>> = HashMap::new();
    for item in &shared.all_items {
        let DocumentItem::Map {
            char_repr,
            selector,
            glyphs,
            ..
        } = item
        else {
            continue;
        };
        let glyph = super::resolved_map_target(glyphs);
        // A variation sequence's target claims no codepoint: the base keeps
        // whatever its own `map` gave it, and the pair reaches the font through
        // cmap format 14 and the fallback lookup instead.
        if selector.is_some() {
            continue;
        }
        let mut pairs = expand_map_pairs(char_repr, glyph);
        shared.glyph_aliases.canonicalize_pairs(&mut pairs);
        for (cp, name) in pairs {
            per_name.entry(name).or_default().push(cp);
        }
    }
    // The selector glyphs themselves still need a plain cmap entry, or the
    // fallback lookup can never fire; see the full collector for why.
    for &sel in &shared.gsub_data.uvs_selectors {
        per_name
            .entry(super::vs_glyph_name(sel))
            .or_default()
            .push(sel);
    }
    for cps in per_name.values_mut() {
        cps.sort_unstable();
        cps.dedup();
    }
    Some(FaceCmap {
        meta: shared.meta,
        scale: shared.scale,
        per_name,
        gsub_data: shared.gsub_data,
    })
}

#[cfg(any(feature = "editor", test))]
pub(super) fn collect_glyph_data_cached(
    docs: &[&Document],
    bitmap: bool,
    contour_cache: Option<&mut ContourCache>,
) -> Option<CollectedFontData> {
    let never = crate::cancel::CancelToken::never();
    let shared = compute_shared_font_input(docs, &never)?;
    collect_glyph_data_with_shared(&shared, bitmap, contour_cache, &never)
}

/// Trace and collect every glyph of one build flavor.
///
/// This is where a face build spends nearly all of its time, so it is also
/// where cancellation has to bite: the token is checked around each stage and
/// inside the per-glyph loops, and a cancelled run returns `None` — the same
/// "no font came out of this" the collector already returns for input it cannot
/// build. Nothing downstream distinguishes the two, and nothing needs to: the
/// only caller that cancels is the one that stopped wanting the result.
pub(super) fn collect_glyph_data_with_shared(
    shared: &SharedFontInput,
    bitmap: bool,
    mut contour_cache: Option<&mut ContourCache>,
    cancel: &crate::cancel::CancelToken,
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

    let seed_timer = crate::startup::PerfStage::new("seed cache");
    let (mut cache, pending) = {
        let cc = &mut contour_cache;
        crate::render::glyph_cache::seed_cache(
            all_items,
            |pixels, desync| {
                // A `desync` grid is ink for the bitmap build and geometry for
                // nobody: the vector build keeps only the dimensions it
                // declares, so a blank grid of the same size stands in.
                if desync && !bitmap {
                    let blank = PixelGrid::new(pixels.width, pixels.height);
                    CachedContours::from_grid(&blank, bitmap, cc.as_deref_mut())
                } else {
                    CachedContours::from_grid(pixels, bitmap, cc.as_deref_mut())
                }
            },
            CachedContours::empty,
            cancel,
        )
    };
    drop(seed_timer);
    {
        let _t = crate::startup::PerfStage::new("resolve composites");
        let mut builder = super::contours::ContourBuilder::new(bitmap, contour_cache);
        crate::render::glyph_cache::resolve_pending(
            &mut cache,
            pending,
            |name| declared_anchors_map.get(name).cloned(),
            &mut builder,
            |_, _| {},
            cancel,
        );
    }
    if cancel.is_cancelled() {
        return None;
    }

    let glyph_bodies_map: HashMap<&str, &GlyphBody> =
        glyph_bodies.iter().map(|(n, b)| (n.as_str(), b)).collect();

    let mut glyph_data: Vec<CollectedGlyph> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    for (i, item) in all_items.iter().enumerate() {
        if i.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
            return None;
        }
        let DocumentItem::Map {
            char_repr,
            selector,
            glyphs,
            ..
        } = item
        else {
            continue;
        };
        let glyph = super::resolved_map_target(glyphs);

        // A variation sequence's target is collected like any other mapped
        // glyph — it needs the same outline, metrics and glyph id — but it
        // claims *no codepoint*. The base keeps whatever glyph its own `map`
        // gave it (claiming `char_repr` here would overwrite exactly that), and
        // the pair reaches the font through the cmap format 14 subtable and the
        // fallback lookup instead.
        //
        // Canonicalized, so a character mapped through an alias reaches the
        // target's `CollectedGlyph` and the two names share one glyph id.
        let pairs: Vec<(Option<u32>, String)> = match selector {
            Some(sel) => super::expand_uvs_map_triples(char_repr, sel, glyph)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, _, mut name)| {
                    shared.glyph_aliases.canonicalize(&mut name);
                    (None, name)
                })
                .collect(),
            None => {
                let mut pairs = expand_map_pairs(char_repr, glyph);
                shared.glyph_aliases.canonicalize_pairs(&mut pairs);
                pairs
                    .into_iter()
                    .map(|(cp, name)| (Some(cp), name))
                    .collect()
            }
        };
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
                codepoints: cp.iter().copied().collect(),
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

    // The selector glyphs the fallback lookup is written against. Blank and
    // zero-advance, because a selector is default-ignorable and invisible
    // whichever path reaches it. What matters is that it has a glyph id *and a
    // plain cmap entry*: `hb_font_get_nominal_glyph` is what a shaper calls
    // before handing an unmatched pair to GSUB, so without the entry the
    // selector arrives as `.notdef` and the fallback rule can never fire.
    //
    // Their codepoints put them in the shared sort below like any other glyph,
    // so the order stays face-independent.
    for &sel in &gsub_data.uvs_selectors {
        let name = super::vs_glyph_name(sel);
        if !seen_names.insert(name.clone()) {
            continue;
        }
        glyph_data.push(CollectedGlyph {
            name,
            codepoints: vec![sel],
            advance_width: 0,
            contours: Vec::new(),
            composite_refs: Vec::new(),
            color_layers: Vec::new(),
            mark: false,
            resolved_anchors: Vec::new(),
            declared_anchors: Vec::new(),
            left_offset: 0,
            top_offset: 0,
        });
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
        // `.notdef` is kept without being asked for: it is what a renderer
        // draws for a character the font does not cover, so nothing in the
        // source ever names it, and the ordinary rules would drop it.
        if let DocumentItem::Glyph {
            name: GlyphName(name),
            body,
        } = item
            && (body.keep || name == NOTDEF)
            && !seen_names.contains(name)
        {
            extra_name_set.insert(name.clone());
        }
    }

    // Include alternative glyphs needed for anchor-based features:
    // 1. Base alts: base:alt carries a "+X" the base itself does not, or one
    //    of a different size — a base offering several slot sizes reaches the
    //    wider ones only through them.
    // 2. Mark alts: mark has "-X" of one size, mark:alt has "-X" of a
    //    different size that matches some base's "+X".
    if !gsub_data.anchor_features.is_empty() {
        // What the passes above already asked for, frozen: the loops below add
        // to `extra_name_set` as they go, and an alternative is not itself a
        // reason to keep another glyph's alternatives.
        let extras_before: HashSet<String> = extra_name_set.clone();
        let anchor_names: Vec<&str> = gsub_data
            .anchor_features
            .iter()
            .map(|f| f.anchor.as_str())
            .collect();
        let alt_index = build_cached_alternatives(&cache);
        // A glyph reached only as an *extra* — a `remap` names it and nothing
        // maps it, which is what a ligature output is — is as real as any
        // other, and its alternatives carry the slots marks attach by. Asking
        // `seen_names` alone left every such glyph with none of them.
        let reachable = |name: &str| seen_names.contains(name) || extras_before.contains(name);

        // 1. Base alts
        for (base_name, alts) in &alt_index {
            if !reachable(base_name) {
                continue;
            }
            let declared = glyph_bodies_map
                .get(base_name.as_str())
                .map(|b| &b.points[..])
                .unwrap_or(&[]);
            for anchor_name in &anchor_names {
                let plus_name = format!("+{anchor_name}");
                let own = declared.iter().find(|p| p.position == plus_name);
                for (alt_name, alt_anchors) in alts {
                    // An alternative of the base's own size is the slot the
                    // base already offers, so nothing would ever substitute
                    // it in; every other size is a slot of its own.
                    let Some(alt_plus) = alt_anchors.iter().find(|p| p.position == plus_name)
                    else {
                        continue;
                    };
                    if own.is_some_and(|own| own.size_matches(alt_plus)) {
                        continue;
                    }
                    if !seen_names.contains(alt_name) {
                        extra_name_set.insert(alt_name.clone());
                    }
                }
            }
        }

        // 2. Mark alts: include mark:alt when its "-X" has a different
        //    size from the primary mark's "-X".
        for (mark_name, alts) in &alt_index {
            if !reachable(mark_name) {
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

    for (i, glyph_name) in extra_names.iter().enumerate() {
        if i.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
            return None;
        }
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
                // The same box every other glyph gets. The parent placing this
                // one has already subtracted its declared bearing
                // (`build_composite_refs`), because a component glyph is the
                // one that carries it; synthesizing the glyph without it put
                // the component a whole `origin` away from where the line
                // asked for it.
                let (advance_width, left_offset, top_offset) = resolve_glyph_metrics(
                    glyph_meta,
                    cr.component_name.as_str(),
                    resolved.width,
                    comp_glyph_scale,
                    scale,
                );
                let font_contours = scale_glyph_contours(
                    &resolved.contours,
                    comp_glyph_scale,
                    meta.ascent() * resolved.scale as u16,
                    left_offset,
                    top_offset,
                );
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
                    left_offset,
                    top_offset,
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
            body.scale,
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
        // Same rule as the outline path above: a `desync` grid is ink for the
        // bitmap build and geometry for nobody.
        let own_pixels = body.pixels.as_ref().filter(|_| bitmap || !body.desync);
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
                let orig_ref = &body.refs[ri];
                effective_visibility(orig_ref.visibility, orig_ref.fill.as_ref(), color_aliases)
            })
            .collect();
        // Negated layers drawn after ref `from` (all of them, for own pixels),
        // restricted to the pass that `skip` selects.
        let negated_after = |from: Option<usize>, skip: LayerVisibility| {
            let start = from.map_or(0, |i| i + 1);
            (start..ref_layers.len())
                .filter(|&j| body.refs[j].negated && ref_vis[j] != skip)
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

        if let Some(own_grid) = own_pixels
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
            let orig_ref = &body.refs[ri];
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
        if let Some(own_grid) = own_pixels
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
            let orig_ref = &body.refs[ri];
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

    // TrueType reserves GID 0 for `.notdef`, so the collected order *is* the
    // GID order only once `.notdef` sits at its head. A source that draws one
    // has it moved there; a source that does not gets an empty stand-in, so
    // that every later stage can say "GID = index" with no special case.
    //
    // The stand-in carries the same advance the blank GID 0 used to be given
    // by hand — the space glyph's, since that is the width a missing-character
    // box is expected to occupy.
    match glyph_data.iter().position(|g| g.name == NOTDEF) {
        Some(i) => {
            let notdef = glyph_data.remove(i);
            glyph_data.insert(0, notdef);
        }
        None => {
            let advance_width = glyph_data
                .iter()
                .find(|g| g.codepoints.contains(&0x20))
                .or(glyph_data.first())
                .map_or(UNITS_PER_EM / 2, |g| g.advance_width);
            glyph_data.insert(
                0,
                CollectedGlyph {
                    name: NOTDEF.to_string(),
                    codepoints: Vec::new(),
                    advance_width,
                    contours: Vec::new(),
                    composite_refs: Vec::new(),
                    color_layers: Vec::new(),
                    mark: false,
                    resolved_anchors: Vec::new(),
                    declared_anchors: Vec::new(),
                    left_offset: 0,
                    top_offset: 0,
                },
            );
        }
    }

    const MAX_GLYPHS: usize = u16::MAX as usize;
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
