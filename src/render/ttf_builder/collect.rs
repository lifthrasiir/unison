//! Gathering resolved glyph data for one build flavor: composite refs,
//! metrics and traced contours per glyph.

use super::contours::CachedContours;
use super::gsub::collect_gsub_data;
use super::*;
use crate::render::glyph_cache::CANCEL_STRIDE;
use std::rc::Rc;

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
    aligns: &crate::document::AnchorAligns,
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
        aligns,
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

/// Every glyph the bitmap build is to draw with *vector* geometry: the
/// `vectoronly` glyphs and everything they reach through `ref`.
///
/// The exemption has to reach the whole subtree, not just the flagged glyph.
/// [`CachedContours::from_grid`] squares a grid off **into the cache** in the
/// bitmap flavor, so a composite assembled from cached components inherits
/// their squared-off form; exempting only the parent would compose blocks and
/// call the result vector artwork. Reaching down instead means a component
/// shared with an unflagged glyph is drawn as vector artwork for that glyph
/// too — [`crate::issues`] reports exactly that case rather than letting it
/// pass, because the alternative (a second, differently traced copy of the
/// component) is a glyph the source never wrote.
///
/// The walk stays inside the drawing that asked, which is only ever a
/// question for a synthesized color/mono glyph: see
/// [`GlyphBody::vectoronly_layers`].
///
/// Empty for the vector build, which draws everything this way already.
fn vectoronly_closure<'a>(
    all_items: impl IntoIterator<Item = &'a DocumentItem>,
    bitmap: bool,
) -> HashSet<String> {
    let mut exempt: HashSet<String> = HashSet::new();
    if !bitmap {
        return exempt;
    }
    let mut refs_of: HashMap<&str, &[GlyphRef]> = HashMap::new();
    let mut layers_of: HashMap<&str, Option<LayerVisibility>> = HashMap::new();
    let mut queue: Vec<&str> = Vec::new();
    for item in all_items {
        let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        else {
            continue;
        };
        refs_of.entry(n.as_str()).or_insert(&body.refs);
        if body.vectoronly {
            layers_of.insert(n.as_str(), body.vectoronly_layers);
            queue.push(n.as_str());
        }
    }
    while let Some(name) = queue.pop() {
        if !exempt.insert(name.to_string()) {
            continue;
        }
        let layers = layers_of.get(name).copied().flatten();
        for r in refs_of.get(name).copied().unwrap_or(&[]) {
            if !GlyphBody::vectoronly_covers(layers, r) {
                continue;
            }
            if !exempt.contains(r.name.as_str()) {
                // Borrowed from the map so the walk stays allocation-free
                // except for the set itself.
                if let Some((k, _)) = refs_of.get_key_value(r.name.as_str()) {
                    queue.push(k);
                } else {
                    exempt.insert(r.name.clone());
                }
            }
        }
    }
    exempt
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
    pub(super) meta: FontMeta,
    scale: f32,
    all_items: Vec<DocumentItem>,
    declared_anchors_map: HashMap<String, Vec<GlyphPoint>>,
    /// The reduction each anchor class states — the one table the GPOS builder
    /// and the composite derivation both read, so a mark a shaped run places
    /// and a mark a precomposed glyph places land together.
    anchor_aligns: crate::document::AnchorAligns,
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

    let anchor_aligns = crate::document::collect_anchor_aligns(all_items.iter());

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
        anchor_aligns,
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

/// One drawing a colour glyph is made of, in that glyph's own logical space.
///
/// A `ref` to a glyph that is itself coloured is *spliced*: the target's own
/// pieces come in one by one, each keeping the colour it was drawn in, so a
/// colour survives however many `ref`s it is reached through. A `fill` on the
/// way down is a claim over everything below it — the target is flattened to
/// one piece in that colour — which is why the colour rides on a piece and not
/// on the `ref` line.
#[derive(Clone)]
struct ColorPiece {
    /// `None`: drawn in the text colour. `Some(rgba)`: a fill of its own, with
    /// `Some(None)` a fill naming a colour nothing resolves — still a layer of
    /// its own, which the rasterizer draws in the text colour.
    fill: Option<Option<Rgba>>,
    vis: LayerVisibility,
    negated: bool,
    /// The chain of `ref` indices this piece was reached by, empty for the
    /// glyph's own pixels: the identity the two builds pair a layer by, see
    /// [`CollectedColorLayer::source`].
    path: Vec<u16>,
    /// The piece's outline in the glyph's logical space.
    contours: Vec<Vec<(f32, f32)>>,
    /// The piece's raster grid and where it sits in the glyph's raster space.
    /// Only a negation reads it; `None` when the target resolved to no grid.
    grid: Option<(PixelGrid, i32, i32)>,
}

/// Everything the colour decomposition looks a name up in. One flavor's worth:
/// `bitmap`/`exempt` are the face being built, so the memo beside it is too.
struct PieceCtx<'a> {
    cache: &'a HashMap<String, CachedContours>,
    alt_index: &'a HashMap<String, Vec<(String, Vec<GlyphPoint>)>>,
    declared_anchors_map: &'a HashMap<String, Vec<GlyphPoint>>,
    aligns: &'a crate::document::AnchorAligns,
    color_aliases: &'a ColorAliasMap,
    bodies: &'a HashMap<&'a str, &'a GlyphBody>,
    bitmap: bool,
    exempt: &'a HashSet<String>,
}

/// How deep a `ref` chain the colour decomposition follows. The ref graph
/// resolves bottom-up and so cannot be cyclic, but a name that reaches itself
/// through pattern expansion has no such guarantee, and the recursion is on the
/// stack.
const COLOR_PIECE_DEPTH: u32 = 32;

/// The body a `ref` name stands for, with the same pattern fallback
/// [`resolve_cached`](crate::render::glyph_cache::resolve_cached) gives the
/// contour cache.
fn body_for_ref<'a>(name: &str, bodies: &HashMap<&'a str, &'a GlyphBody>) -> Option<&'a GlyphBody> {
    if let Some(body) = bodies.get(name) {
        return Some(body);
    }
    let expanded = crate::ref_composite::parse_ref_pattern(name)?;
    bodies.get(expanded.get(0).as_str()).copied()
}

/// Does anything this glyph draws carry a colour of its own?
///
/// A `fill` or a `visibility` on one of the glyph's own `ref`s says so
/// outright, and so does a `ref` to a glyph that is itself coloured — that
/// colour now travels up. Names only, no geometry: this is the gate in front of
/// the colour path, and every mono glyph has to pass it cheaply.
fn glyph_is_colored(
    name: &str,
    bodies: &HashMap<&str, &GlyphBody>,
    alt_index: &HashMap<String, Vec<(String, Vec<GlyphPoint>)>>,
    memo: &mut HashMap<String, bool>,
) -> bool {
    if let Some(&known) = memo.get(name) {
        return known;
    }
    // Seeded `false` for the length of the walk, so a name that reaches itself
    // answers rather than recurses.
    memo.insert(name.to_string(), false);
    let colored = body_for_ref(name, bodies).is_some_and(|body| {
        body.refs.iter().any(|r| {
            r.fill.is_some()
                || r.visibility.is_some()
                || glyph_is_colored(&r.name, bodies, alt_index, memo)
                || alt_index.get(&r.name).is_some_and(|alts| {
                    alts.iter()
                        .any(|(n, _)| glyph_is_colored(n, bodies, alt_index, memo))
                })
        })
    });
    memo.insert(name.to_string(), colored);
    colored
}

/// The pieces a `ref` target hands its parent, or `None` when it has none to
/// hand over — nothing coloured to keep, no body to read them from, or a chain
/// too deep to follow. The parent then reads the target the way it always did,
/// as one flattened drawing.
fn color_pieces_for_name(
    name: &str,
    ctx: &PieceCtx,
    colored_memo: &mut HashMap<String, bool>,
    pieces_memo: &mut HashMap<String, Rc<Vec<ColorPiece>>>,
    depth: u32,
) -> Option<Rc<Vec<ColorPiece>>> {
    if depth >= COLOR_PIECE_DEPTH
        || !glyph_is_colored(name, ctx.bodies, ctx.alt_index, colored_memo)
    {
        return None;
    }
    if let Some(pieces) = pieces_memo.get(name) {
        return Some(pieces.clone());
    }
    let body = body_for_ref(name, ctx.bodies)?;
    let pieces = color_pieces_for_body(name, body, ctx, colored_memo, pieces_memo, depth);
    pieces_memo.insert(name.to_string(), pieces.clone());
    Some(pieces)
}

/// The one colour a group of pieces can be handed up as, once it is flattened
/// into a single drawing: a difference has no per-part colouring left to give
/// away, so only a colour every part already agrees on survives.
fn agreed_fill(pieces: &[ColorPiece]) -> Option<Option<Rgba>> {
    let mut parts = pieces.iter().filter(|p| !p.negated);
    let first = parts.next()?.fill.clone();
    parts.all(|p| p.fill == first).then_some(first).flatten()
}

/// Decompose one glyph body into the pieces its colour layers are built from.
///
/// The order is the drawing order — own pixels first, then each `ref` — because
/// a negated piece subtracts from what is above it and from nothing else.
fn color_pieces_for_body(
    name: &str,
    body: &GlyphBody,
    ctx: &PieceCtx,
    colored_memo: &mut HashMap<String, bool>,
    pieces_memo: &mut HashMap<String, Rc<Vec<ColorPiece>>>,
    depth: u32,
) -> Rc<Vec<ColorPiece>> {
    use crate::render::glyph_cache::CachedGlyphEntry;

    let mut out: Vec<ColorPiece> = Vec::new();
    // Same rules as the outline path: `vectoronly` picks the flavor this glyph
    // is drawn in, and a `desync` grid is ink for the bitmap build and geometry
    // for nobody.
    let bitmap = ctx.bitmap && (ctx.exempt.is_empty() || !ctx.exempt.contains(name));
    let own_pixels = body.pixels.as_ref().filter(|_| bitmap || !body.desync);
    if let Some(own) = own_pixels
        && !own.is_all_empty()
    {
        out.push(ColorPiece {
            fill: None,
            vis: LayerVisibility::Both,
            negated: false,
            path: Vec::new(),
            contours: track_contour(own, PX_SUBPIXEL),
            grid: Some((own.clone(), 0, 0)),
        });
    }

    let (effective_refs, _) = derive_effective_refs(
        &body.points,
        &body.refs,
        ctx.cache,
        ctx.alt_index,
        ctx.declared_anchors_map,
        ctx.aligns,
        body.scale,
    );
    let ps = body.scale.max(1);
    for (ri, eref) in effective_refs.iter().enumerate() {
        let orig = &body.refs[ri];
        let Some(cached) = resolve_cached_ref(&eref.name, ctx.cache) else {
            continue;
        };
        let rs = cached.scale.max(1);
        let rsf = ps as f32 / rs as f32;
        // Box coordinates in, grid coordinates out: the offset names the
        // target's box corner, where the target's own drawing sits in its grid.
        let (box_col, box_row) = cached.declared_origin();
        let (base_row, base_col) = (
            eref.row() as i32 - box_row as i32 * ps as i32,
            eref.col() as i32 - box_col as i32 * ps as i32,
        );
        let place = |contours: &[Vec<(f32, f32)>]| -> Vec<Vec<(f32, f32)>> {
            contours
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|&(x, y)| (x * rsf + base_col as f32, y * rsf + base_row as f32))
                        .collect()
                })
                .collect()
        };
        let rescaled = |grid: &PixelGrid| {
            if rs == ps {
                grid.clone()
            } else {
                grid.rescale(rs, ps)
            }
        };
        let vis = effective_visibility(orig.visibility, orig.fill.as_ref(), ctx.color_aliases);
        // A `fill` is a claim over everything the ref reaches, so it stops the
        // target's own colours from coming up; so does a negation, which is a
        // difference and has no parts to hand over (see `push_ref_components`
        // in `render::sample`, which splits a ref by the same rule).
        let sub = (orig.fill.is_none() && !orig.negated)
            .then(|| color_pieces_for_name(&eref.name, ctx, colored_memo, pieces_memo, depth + 1))
            .flatten();
        let splice = sub.as_ref().filter(|s| !s.iter().any(|p| p.negated));

        if let Some(sub) = splice {
            for p in sub.iter() {
                out.push(ColorPiece {
                    fill: p.fill.clone(),
                    // A `visibility` written on the ref is the ref's word for
                    // everything under it; a colour is not overridden the same
                    // way, because the ref wrote none.
                    vis: if orig.visibility.is_some() {
                        vis
                    } else {
                        p.vis
                    },
                    negated: false,
                    path: std::iter::once(ri as u16)
                        .chain(p.path.iter().copied())
                        .collect(),
                    contours: place(&p.contours),
                    grid: p.grid.as_ref().map(|(g, r, c)| {
                        (
                            rescaled(g),
                            base_row + (*r as f32 * rsf).round() as i32,
                            base_col + (*c as f32 * rsf).round() as i32,
                        )
                    }),
                });
            }
            continue;
        }

        let fill = match orig.fill.as_ref().filter(|f| f.color != "fg") {
            Some(f) => Some(resolve_fill_rgba(f, ctx.color_aliases)),
            // Nothing written: a target flattened into one drawing still hands
            // up the one colour its parts agree on, if they agree on one.
            None => sub.as_ref().and_then(|s| agreed_fill(s)),
        };
        out.push(ColorPiece {
            fill,
            vis,
            negated: orig.negated,
            path: vec![ri as u16],
            contours: place(&cached.contours),
            grid: cached.grid.as_ref().map(|g| {
                let (row, col) = cached.placed_at(eref.row() as i32, eref.col() as i32, ps);
                (rescaled(g), row, col)
            }),
        });
    }

    Rc::new(out)
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

    let exempt = vectoronly_closure(all_items, bitmap);

    let seed_timer = crate::startup::PerfStage::new("seed cache");
    let (mut cache, pending) = {
        let cc = &mut contour_cache;
        let exempt = &exempt;
        crate::render::glyph_cache::seed_cache(
            all_items,
            |name, pixels, desync| {
                // `vectoronly` and everything it reaches is traced the way the
                // vector build traces it, whichever face is being built.
                let flavor = bitmap && (exempt.is_empty() || !exempt.contains(name));
                // A `desync` grid is ink for the bitmap build and geometry for
                // nobody: the vector build keeps only the dimensions it
                // declares, so a blank grid of the same size stands in.
                if desync && !flavor {
                    let blank = PixelGrid::new(pixels.width, pixels.height);
                    CachedContours::from_grid(&blank, flavor, cc.as_deref_mut())
                } else {
                    CachedContours::from_grid(pixels, flavor, cc.as_deref_mut())
                }
            },
            CachedContours::empty,
            cancel,
        )
    };
    drop(seed_timer);
    {
        let _t = crate::startup::PerfStage::new("resolve composites");
        let mut builder = super::contours::ContourBuilder::new(bitmap, &exempt, contour_cache);
        crate::render::glyph_cache::resolve_pending(
            &mut cache,
            pending,
            &shared.anchor_aligns,
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
    let mut colored_memo: HashMap<String, bool> = HashMap::new();
    let mut pieces_memo: HashMap<String, Rc<Vec<ColorPiece>>> = HashMap::new();
    for g in &mut glyph_data {
        let name = g.name.clone();
        let Some(body) = glyph_bodies_map.get(name.as_str()).copied() else {
            continue;
        };
        if !glyph_is_colored(
            &name,
            &glyph_bodies_map,
            &color_alt_index,
            &mut colored_memo,
        ) {
            continue;
        }

        let color_glyph_scale = scale / body.scale as f32;
        let color_ascent = meta.ascent() * body.scale as u16;
        let g_meta = glyph_meta.get(&name);
        let left_offset = g_meta
            .and_then(|m| m.left)
            .map_or(0, |left| (left as f32 * scale).round() as i16);
        let top_offset = g_meta
            .and_then(|m| m.top)
            .map_or(0, |top| (top as f32 * scale).round() as i16);

        let ctx = PieceCtx {
            cache: &cache,
            alt_index: &color_alt_index,
            declared_anchors_map,
            aligns: &shared.anchor_aligns,
            color_aliases,
            bodies: &glyph_bodies_map,
            bitmap,
            exempt: &exempt,
        };
        let pieces =
            color_pieces_for_body(&name, body, &ctx, &mut colored_memo, &mut pieces_memo, 0);

        // A `negated` piece draws nothing of its own — it only removes area
        // from the pieces above it.  This path splits a composite into
        // per-layer contour sets, so each surviving piece has to be traced
        // against the negated pieces that follow it.  Cutting is per pass: a
        // monoonly negation cannot reach the coloronly layers, which are not
        // present when it is drawn.
        let has_negated = pieces.iter().any(|p| p.negated);
        // Negated pieces drawn after piece `from`, restricted to the pass that
        // `skip` selects.
        let negated_after = |from: usize, skip: LayerVisibility| {
            pieces[from + 1..]
                .iter()
                .filter(|p| p.negated && p.vis != skip)
                .filter_map(|p| p.grid.as_ref().map(|(g, r, c)| (g, *r, *c, true)))
                .collect::<Vec<_>>()
        };
        // Trace one positive piece minus the negated pieces that follow it.
        // `None` when nothing cuts it, so a piece no negation reaches keeps
        // its own exactly traced contours instead of being re-traced.
        let cut_contours = |piece: &ColorPiece, pi: usize, skip: LayerVisibility| {
            if !has_negated {
                return None;
            }
            let (grid, row, col) = piece.grid.as_ref()?;
            let negs = negated_after(pi, skip);
            if negs.is_empty() {
                return None;
            }
            let mut layers = vec![(grid, *row, *col, false)];
            layers.extend(negs);
            Some(track_contour_multi_diff_at(&layers, PX_SUBPIXEL))
        };

        // The colour pass: every piece with a fill of its own becomes a COLR
        // layer, and everything drawn in the text colour is one merged
        // foreground layer.
        let mut fg_contours: Vec<Vec<(i16, i16)>> = Vec::new();
        let mut color_layers: Vec<CollectedColorLayer> = Vec::new();
        for (pi, piece) in pieces.iter().enumerate() {
            if piece.negated || piece.vis == LayerVisibility::MonoOnly {
                continue;
            }
            let traced = cut_contours(piece, pi, LayerVisibility::MonoOnly);
            let logical = traced.as_ref().unwrap_or(&piece.contours);
            let layer_contours = scale_glyph_contours(
                logical,
                color_glyph_scale,
                color_ascent,
                left_offset,
                top_offset,
            );
            if layer_contours.is_empty() {
                continue;
            }
            match &piece.fill {
                None => fg_contours.extend(layer_contours),
                Some(rgba) => {
                    let palette_index = match rgba {
                        Some(rgba) => *color_to_index.entry(rgba.clone()).or_insert_with(|| {
                            let idx = palette_colors.len() as u16;
                            palette_colors.push(rgba.clone());
                            idx
                        }),
                        None => 0xFFFF,
                    };
                    color_layers.push(CollectedColorLayer {
                        contours: layer_contours,
                        palette_index,
                        source: ColorLayerSource::Ref(piece.path.clone()),
                    });
                }
            }
        }

        g.color_layers = color_layers;
        if !fg_contours.is_empty() {
            g.color_layers.insert(
                0,
                CollectedColorLayer {
                    contours: fg_contours,
                    palette_index: 0xFFFF,
                    source: ColorLayerSource::Foreground,
                },
            );
        }

        // Rebuild fallback contours: only non-coloronly pieces
        let mut fallback_contours: Vec<Vec<(i16, i16)>> = Vec::new();
        for (pi, piece) in pieces.iter().enumerate() {
            if piece.negated || piece.vis == LayerVisibility::ColorOnly {
                continue;
            }
            let traced = cut_contours(piece, pi, LayerVisibility::ColorOnly);
            let logical = traced.as_ref().unwrap_or(&piece.contours);
            fallback_contours.extend(scale_glyph_contours(
                logical,
                color_glyph_scale,
                color_ascent,
                left_offset,
                top_offset,
            ));
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

/// The shortest code point sequence that puts each glyph on the screen that a
/// `remap` produces and no `map` names — the flags, the composed jamo, and
/// everything else a specimen has no code point to key a cell on.
///
/// The expansion this reads is the one the GSUB tables are built from, so the
/// question costs a walk over data that already exists. What the walk is and
/// why it checks its answers rather than deriving them is
/// [`crate::render::reach`].
pub(super) fn remap_only_sequences(
    docs: &[&Document],
    face: &crate::faces::Face,
    expansion: Option<&super::expand::Expansion>,
    cancel: &crate::cancel::CancelToken,
) -> Option<RemapOnly> {
    use crate::render::reach::{Cascade, RemapLine};

    // Expanding is by far the larger half of this — 731 ms against 146 ms over
    // `font/` — so a caller that has an expansion lends it rather than paying
    // for a second one. See `ExpansionSource`.
    let source = expansion.map_or(ExpansionSource::Compute, ExpansionSource::Lent);
    let input = compute_face_input(docs, face, cancel, source)?;

    let mut cmap: Vec<(u32, String)> = Vec::new();
    for item in &input.all_items {
        let DocumentItem::Map {
            char_repr,
            selector,
            glyphs,
            ..
        } = item
        else {
            continue;
        };
        // A variation sequence claims no code point of its own; it reaches the
        // cascade as a pair instead, the same way it reaches cmap format 14.
        if selector.is_some() {
            continue;
        }
        let mut pairs = super::expand_map_pairs(char_repr, super::resolved_map_target(glyphs));
        input.glyph_aliases.canonicalize_pairs(&mut pairs);
        cmap.append(&mut pairs);
    }
    let uvs: Vec<(u32, u32, String)> =
        super::gsub::collect_uvs_pairs(&input.all_items, &input.glyph_aliases)
            .into_iter()
            .map(|pair| (pair.base, pair.selector, pair.glyph))
            .collect();

    // Only the groups some `feature` runs, and in lookup order — `remap_sets`
    // is keyed by name and so says nothing about order. Whether the tag is one
    // a shaper turns on by default is *not* checked; every `feature` in `font/`
    // is one (`ccmp`, `liga`, `calt`, `locl`, `ljmo`, `vjmo`, `tjmo`).
    let run: HashSet<&str> = input
        .gsub_data
        .features
        .iter()
        .flat_map(|(_, _, names)| names.iter().map(String::as_str))
        .collect();
    let groups: Vec<Vec<RemapLine<'_>>> = input
        .gsub_data
        .groups
        .order
        .iter()
        .filter(|name| run.contains(name.as_str()))
        .filter_map(|name| input.gsub_data.remap_sets.get(name))
        .map(|set| {
            set.iter()
                .map(|rule| RemapLine {
                    lookbehind: &rule.lookbehind,
                    source: &rule.source,
                    target: &rule.target,
                    lookahead: &rule.lookahead,
                })
                .collect()
        })
        .collect();

    let mapped: HashSet<&str> = cmap.iter().map(|(_, name)| name.as_str()).collect();
    let targets: std::collections::BTreeSet<&str> = input
        .gsub_data
        .remap_sets
        .values()
        .flatten()
        .flat_map(|rule| rule.target.iter().flatten())
        .map(String::as_str)
        .filter(|name| !name.is_empty() && !mapped.contains(name))
        .collect();

    let solved = Cascade::new(&cmap, &uvs, &groups).solve(&targets);
    Some(RemapOnly {
        unsolved: targets
            .iter()
            .filter(|name| !solved.contains_key(*name))
            .map(|name| name.to_string())
            .collect(),
        solved: solved
            .into_iter()
            .map(|(name, cps)| (name.to_string(), cps))
            .collect(),
    })
}
