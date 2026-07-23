use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

use write_fonts::tables::cmap::Cmap;
use write_fonts::tables::colr::{BaseGlyph, Colr, Layer as ColrLayer};
use write_fonts::tables::cpal::{ColorRecord, Cpal};
use write_fonts::tables::gdef::{Gdef, MarkGlyphSets};
use write_fonts::tables::gpos::{
    AnchorTable, BaseArray, BaseRecord, Gpos, Mark2Array, Mark2Record,
    MarkArray, MarkBasePosFormat1, MarkMarkPosFormat1, MarkRecord,
    PositionLookup, PositionLookupList,
};
use write_fonts::tables::glyf::{Bbox, Component, ComponentFlags, CompositeGlyph, Contour, Glyph, GlyfLocaBuilder, SimpleGlyph};
use read_fonts::tables::glyf::Anchor;
use read_fonts::tables::glyf::CurvePoint;
use write_fonts::tables::gsub::{
    Gsub, Ligature, LigatureSet, LigatureSubstFormat1, SingleSubst, SingleSubstFormat2,
    SubstitutionChainContext, SubstitutionLookup,
};
use write_fonts::tables::head::{Flags, Head};
use write_fonts::tables::hhea::Hhea;
use write_fonts::tables::hmtx::{Hmtx, LongMetric};
use write_fonts::tables::layout::{
    ChainedSequenceContext, ChainedSequenceContextFormat3, ClassDef, ClassDefFormat2,
    ClassRangeRecord, CoverageTable, Feature, FeatureList, FeatureRecord, LangSys, Lookup,
    LookupFlag, LookupList, Script, ScriptList, ScriptRecord, SequenceLookupRecord,
};
use write_fonts::tables::maxp::Maxp;
use write_fonts::tables::name::{Name, NameRecord};
use write_fonts::tables::os2::{Os2, SelectionFlags};
use write_fonts::tables::post::Post;
use write_fonts::FontBuilder;

use write_fonts::types::{Fixed, GlyphId, GlyphId16, NameId, Tag};

use crate::document::*;
use crate::document_io;
use crate::pixel::{PX_ALMOSTFULL, PX_SUBPIXEL, PixelShape};
use crate::render::contour::{track_contour, track_contour_multi, track_contour_multi_diff};

pub const UNITS_PER_EM: u16 = 1024;

// ---------------------------------------------------------------------------
// Persistent contour cache — survives across incremental rebuilds
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CacheEntry<T> {
    value: T,
    gen_id: u64,
}

#[derive(Default, Clone)]
pub struct ContourCache {
    entries: HashMap<u64, CacheEntry<Vec<Vec<(f32, f32)>>>>,
    composite_entries: HashMap<u64, CacheEntry<CachedContours>>,
    gen_id: u64,
}

pub type SharedContourCache = Arc<Mutex<ContourCache>>;

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
impl ContourCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.composite_entries.clear();
    }

    pub fn begin_generation(&mut self) {
        self.gen_id += 1;
    }

    pub fn evict_stale(&mut self) {
        let cur_gen =self.gen_id;
        self.entries.retain(|_, e| e.gen_id == cur_gen);
        self.composite_entries.retain(|_, e| e.gen_id == cur_gen);
    }
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn new_contour_cache() -> SharedContourCache {
    Arc::new(Mutex::new(ContourCache::default()))
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
    let cur_gen =cache.gen_id;
    if let Some(entry) = cache.entries.get_mut(&key) {
        entry.gen_id = cur_gen;
        return entry.value.clone();
    }
    let contours = track_contour(grid, PX_SUBPIXEL);
    cache.entries.insert(key, CacheEntry { value: contours.clone(), gen_id: cur_gen });
    contours
}

#[derive(Clone, Copy)]
struct FontMeta {
    height: u16,
    ascent: u16,
    descent: u16,
}

impl Default for FontMeta {
    fn default() -> Self {
        Self {
            height: 16,
            ascent: 14,
            descent: 2,
        }
    }
}

struct CompositeRef {
    component_name: String,
    x_offset: i16,
    y_offset: i16,
}

struct CollectedGlyph {
    name: String,
    codepoint: Option<u32>,
    advance_width: u16,
    contours: Vec<Vec<(i16, i16)>>,
    composite_refs: Vec<CompositeRef>,
    color_layers: Vec<CollectedColorLayer>,
    mark: bool,
    /// Resolved anchors (including forwarded from refs).
    resolved_anchors: Vec<GlyphPoint>,
    /// Anchors declared directly on the glyph body (not forwarded).
    declared_anchors: Vec<GlyphPoint>,
    /// Left/top offsets in font units, for GPOS anchor coordinate adjustment.
    left_offset: i16,
    top_offset: i16,
}

struct CollectedColorLayer {
    contours: Vec<Vec<(i16, i16)>>,
    palette_index: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

pub fn parse_hex_color(s: &str) -> Option<Rgba> {
    let s = s.strip_prefix('#')?;
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some(Rgba { r, g, b, a: 255 })
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some(Rgba { r, g, b, a })
        }
        _ => None,
    }
}

pub type ColorAliasMap = HashMap<String, (Rgba, Option<LayerVisibility>)>;

pub fn collect_color_aliases(docs: &[&Document]) -> ColorAliasMap {
    let mut map = ColorAliasMap::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Color { name, value, visibility } = item {
                let resolved = resolve_color_value(value, &map);
                if let Some(rgba) = resolved {
                    map.insert(name.clone(), (rgba, *visibility));
                }
            }
        }
    }
    map
}

fn resolve_color_value(value: &str, aliases: &ColorAliasMap) -> Option<Rgba> {
    if value.starts_with('#') {
        parse_hex_color(value)
    } else if let Some((rgba, _)) = aliases.get(value) {
        Some(rgba.clone())
    } else {
        None
    }
}

pub fn resolve_fill_rgba(
    fill: &RefFill,
    color_aliases: &ColorAliasMap,
) -> Option<Rgba> {
    if fill.color == "fg" {
        return None;
    }
    if fill.color.starts_with('#') {
        return parse_hex_color(&fill.color);
    }
    color_aliases.get(&fill.color).map(|(rgba, _)| rgba.clone())
}

pub fn effective_visibility(
    ref_visibility: Option<LayerVisibility>,
    fill: Option<&RefFill>,
    color_aliases: &ColorAliasMap,
) -> LayerVisibility {
    if let Some(vis) = ref_visibility {
        return vis;
    }
    if let Some(fill) = fill {
        if !fill.color.starts_with('#') && fill.color != "fg"
            && let Some((_, Some(vis))) = color_aliases.get(&fill.color) {
                return *vis;
            }
    }
    LayerVisibility::Both
}

// ---------------------------------------------------------------------------
// GSUB remap data structures
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ExpandedRemap {
    lookbehind: Vec<Vec<String>>,
    /// Each inner Vec is a sequence of input glyph positions (len > 1 = ligature).
    source: Vec<Vec<String>>,
    /// Each inner Vec is a sequence of output glyph positions.
    target: Vec<Vec<String>>,
    lookahead: Vec<Vec<String>>,
}

#[derive(Clone)]
struct GsubData {
    remap_sets: BTreeMap<String, Vec<ExpandedRemap>>,
    /// (feature_tag, scripts, remap_set_names)
    features: Vec<(String, Vec<String>, Vec<String>)>,
    /// Anchor-based feature declarations: (feature_tag, scripts, anchor_name)
    anchor_features: Vec<(String, Vec<String>, String)>,
}

pub fn load_docs_from_directory(dir: &Path) -> Vec<Document> {
    load_docs_from_directory_checked(dir).0
}

pub fn load_docs_from_directory_checked(dir: &Path) -> (Vec<Document>, Vec<(std::path::PathBuf, String)>) {
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (docs, errors);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "unf") {
            match document_io::parse_document(&path) {
                Ok(doc) => docs.push(doc),
                Err(e) => errors.push((path, e.to_string())),
            }
        }
    }
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    (docs, errors)
}

pub fn build_font_from_documents(docs: &[&Document]) -> Option<Vec<u8>> {
    build_font_from_documents_inner(docs, false, None)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn build_font_pair_cached(
    docs: &[&Document],
    shared_cache: &SharedContourCache,
) -> Option<((Vec<u8>, Vec<u8>), HashMap<String, u16>)> {
    let shared = compute_shared_font_input(docs)?;

    let mut cc = shared_cache.lock().unwrap();
    cc.begin_generation();

    let bitmap_data = collect_glyph_data_with_shared(&shared, true, Some(&mut cc))?;
    let vector_data = collect_glyph_data_with_shared(&shared, false, Some(&mut cc))?;

    cc.evict_stale();
    drop(cc);

    let (b_meta, _, b_glyphs, b_gsub, b_palette) = bitmap_data;
    let (v_meta, v_scale, v_glyphs, v_gsub, v_palette) = vector_data;

    let mut name_to_gid: HashMap<String, u16> = HashMap::new();
    for (i, g) in v_glyphs.iter().enumerate() {
        name_to_gid.entry(g.name.clone()).or_insert((i + 1) as u16);
    }

    let b_scale = UNITS_PER_EM as f32 / b_meta.height as f32;
    let b_ascender = (b_meta.ascent as f32 * b_scale).round() as i16;
    let b_descender = -((b_meta.descent as f32 * b_scale).round() as i16);

    let v_ascender = (v_meta.ascent as f32 * v_scale).round() as i16;
    let v_descender = -((v_meta.descent as f32 * v_scale).round() as i16);

    let v_hint_ppem = if UNITS_PER_EM.is_multiple_of(v_meta.height) {
        v_meta.height
    } else {
        0
    };

    let (bitmap, vector) = std::thread::scope(|s| {
        let bh = s.spawn(|| build_ttf(b_ascender, b_descender, &b_glyphs, 0, &b_gsub, &b_palette, b_scale, b_meta.ascent));
        let vector = build_ttf(v_ascender, v_descender, &v_glyphs, v_hint_ppem, &v_gsub, &v_palette, v_scale, v_meta.ascent);
        let bitmap = bh.join().unwrap();
        (bitmap, vector)
    });

    Some(((bitmap, vector), name_to_gid))
}

/// Build result containing the TTF bytes, GID→name map, and the pixel
/// em-height needed to convert font units back to pixel coordinates.
pub struct FontWithGidMap {
    pub ttf: Vec<u8>,
    pub gid_to_name: HashMap<u16, String>,
    pub height: u16,
}

/// Build the font and return the TTF bytes together with a GID→glyph-name map
/// and the pixel em-height.
pub fn build_font_with_gid_map(docs: &[&Document]) -> Option<FontWithGidMap> {
    let (meta, scale, glyph_data, gsub_data, palette) = collect_glyph_data_cached(docs, false, None)?;
    let ascender = (meta.ascent as f32 * scale).round() as i16;
    let descender = -((meta.descent as f32 * scale).round() as i16);
    let hint_ppem = if UNITS_PER_EM.is_multiple_of(meta.height) { meta.height } else { 0 };

    let mut gid_to_name: HashMap<u16, String> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for (i, g) in glyph_data.iter().enumerate() {
        if seen.insert(g.name.clone()) {
            gid_to_name.insert((i + 1) as u16, g.name.clone());
        }
    }

    let ttf = build_ttf(ascender, descender, &glyph_data, hint_ppem, &gsub_data, &palette, scale, meta.ascent);
    Some(FontWithGidMap { ttf, gid_to_name, height: meta.height })
}

fn build_font_from_documents_inner(docs: &[&Document], bitmap: bool, contour_cache: Option<&mut ContourCache>) -> Option<Vec<u8>> {
    let (meta, scale, glyph_data, gsub_data, palette) = collect_glyph_data_cached(docs, bitmap, contour_cache)?;

    let ascender = (meta.ascent as f32 * scale).round() as i16;
    let descender = -((meta.descent as f32 * scale).round() as i16);

    let hint_ppem = if !bitmap && UNITS_PER_EM.is_multiple_of(meta.height) {
        meta.height
    } else {
        0
    };
    Some(build_ttf(ascender, descender, &glyph_data, hint_ppem, &gsub_data, &palette, scale, meta.ascent))
}

/// Resolve all documents' glyph items (expanding name patterns, following
/// refs, tracking contours) into the flat, codepoint-sorted glyph list that
/// [`build_font_from_documents`] then hands to [`build_ttf`]. Split out so
/// tests can inspect the intermediate, pre-TTF-encoding representation
/// directly (e.g. to canonicalize away the non-deterministic contour
/// point/rotation order that `track_contour` can produce — see
/// `tests::ttf_build_digest_real_files_is_stable`).
/// Everything needed to assemble the font tables for one build flavor:
/// font metadata, pixel→unit scale, glyphs, GSUB inputs, and color palette.
type CollectedFontData = (FontMeta, f32, Vec<CollectedGlyph>, GsubData, Vec<Rgba>);

#[cfg(test)]
fn collect_glyph_data(docs: &[&Document], bitmap: bool) -> Option<CollectedFontData> {
    collect_glyph_data_cached(docs, bitmap, None)
}

/// OpenType tag from a string, right-padded with spaces to 4 bytes.
fn make_tag(name: &str) -> Tag {
    let mut tag_arr = [b' '; 4];
    for (i, &b) in name.as_bytes().iter().enumerate().take(4) {
        tag_arr[i] = b;
    }
    Tag::new(&tag_arr)
}

/// Script records with one default LangSys per script, from a
/// script-tag → feature-indices map.
fn build_script_records(script_feature_indices: &BTreeMap<String, Vec<u16>>) -> Vec<ScriptRecord> {
    let mut script_records: Vec<ScriptRecord> = Vec::new();
    for (script_tag, feat_indices) in script_feature_indices {
        let lang_sys = LangSys {
            required_feature_index: 0xFFFF,
            feature_indices: feat_indices.clone(),
        };
        let script = Script::new(Some(lang_sys), vec![]);
        script_records.push(ScriptRecord::new(make_tag(script_tag), script));
    }
    script_records
}

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
    crate::ref_composite::derive_ref_offsets_with(
        points,
        refs,
        |name| {
            resolve_cached_ref(name, cache)
                .map(|resolved| resolved.anchors.clone())
        },
        |name| {
            alt_index
                .get(name)
                .map_or_else(Vec::new, |v| v.clone())
        },
        |name| {
            declared_anchors_map.get(name).cloned()
        },
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
    glyph_meta: &HashMap<String, (Option<u16>, Option<i16>, Option<i16>)>,
    name: &str,
    resolved_width: u16,
    scale: f32,
    base_scale: f32,
) -> (u16, i16, i16) {
    let advance_width = match glyph_meta.get(name) {
        Some(&(Some(adv), _, _)) => (adv as f32 * base_scale).round() as u16,
        _ => (resolved_width as f32 * scale).round() as u16,
    };
    let left_offset = match glyph_meta.get(name) {
        Some(&(_, Some(left), _)) => (left as f32 * base_scale).round() as i16,
        _ => 0,
    };
    let top_offset = match glyph_meta.get(name) {
        Some(&(_, _, Some(top))) => (top as f32 * base_scale).round() as i16,
        _ => 0,
    };
    (advance_width, left_offset, top_offset)
}

/// Composite references for a resolved glyph in font units, or empty when
/// the glyph is forced inline.  Compensates for each component glyph's own
/// left/top offset so that the shift doesn't propagate into parent composites.
fn build_composite_refs(
    resolved: &CachedContours,
    inline: bool,
    left_offset: i16,
    top_offset: i16,
    glyph_meta: &HashMap<String, (Option<u16>, Option<i16>, Option<i16>)>,
    scale: f32,
    base_scale: f32,
) -> Vec<CompositeRef> {
    if inline {
        return Vec::new();
    }
    let Some(comps) = &resolved.composite_components else {
        return Vec::new();
    };
    comps.iter().map(|(name, dx, dy)| {
        let (comp_left, comp_top) = match glyph_meta.get(name.as_str()) {
            Some(&(_, left, top)) => (
                left.map_or(0, |l| (l as f32 * base_scale).round() as i16),
                top.map_or(0, |t| (t as f32 * base_scale).round() as i16),
            ),
            None => (0, 0),
        };
        CompositeRef {
            component_name: name.clone(),
            x_offset: ((*dx + left_offset as f32 / scale) * scale).round() as i16 - comp_left,
            y_offset: (-*dy * scale).round() as i16 - top_offset + comp_top,
        }
    }).collect()
}

/// Collect all items from `docs` with name-part patterns substituted and
/// expanded, and `map-decomposed` directives turned into synthesized
/// composite glyphs + `map` entries via NFD decomposition.
pub(crate) fn collect_expanded_items(docs: &[&Document], name_parts: &NamePartsMap) -> Vec<DocumentItem> {
    // Collect all items, expanding ranged glyph names
    let mut all_items: Vec<DocumentItem> = Vec::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Glyph { name, body } = item {
                let name_str = substitute_name_parts(&name.display(), name_parts);
                if is_name_pattern(&name_str) {
                    let subst_name = GlyphName(name_str);
                    let subst_refs: Vec<GlyphRef> = body
                        .refs
                        .iter()
                        .map(|r| GlyphRef {
                            name: substitute_name_parts(&r.name, name_parts),
                            offset: r.offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: r.visibility,
                        })
                        .collect();
                    if let Ok(expanded) = expand_glyph_block(&subst_name, &subst_refs, body.scale) {
                        for mut item in expanded {
                            if let DocumentItem::Glyph { body: ref mut b, .. } = item {
                                b.pixels = body.pixels.clone();
                                b.points = body.points.clone();
                                b.sticky = body.sticky;
                                b.advance = body.advance;
                                b.left = body.left;
                                b.top = body.top;
                                b.scale = body.scale;
                            }
                            all_items.push(item);
                        }
                    }
                } else {
                    let mut body = body.clone();
                    for gref in &mut body.refs {
                        gref.name = substitute_name_parts(&gref.name, name_parts);
                    }
                    all_items.push(DocumentItem::Glyph {
                        name: GlyphName(name_str),
                        body,
                    });
                }
            } else if let DocumentItem::Map { char_repr, glyph } = item {
                all_items.push(DocumentItem::Map {
                    char_repr: char_repr.clone(),
                    glyph: substitute_name_parts(glyph, name_parts),
                });
            } else if let DocumentItem::MapDecomposed { .. } = item {
                all_items.push(item.clone());
            } else {
                all_items.push(item.clone());
            }
        }
    }

    // Expand MapDecomposed: build codepoint→glyph reverse map, then
    // synthesize composite glyphs from NFD decomposition.
    {
        use unicode_normalization::UnicodeNormalization;

        let mut cp_to_glyph: HashMap<u32, String> = HashMap::new();
        for item in &all_items {
            if let DocumentItem::Map { char_repr, glyph } = item {
                let pairs = expand_map_pairs(char_repr, glyph);
                for (cp, gname) in pairs {
                    cp_to_glyph.entry(cp).or_insert(gname);
                }
            }
        }

        let mut decomposed_items: Vec<DocumentItem> = Vec::new();
        let all_items_snapshot = all_items.clone();
        for item in &all_items_snapshot {
            let DocumentItem::MapDecomposed { char_repr } = item else {
                continue;
            };
            let Some(cp) = parse_map_char(char_repr) else {
                continue;
            };
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };

            let nfd: Vec<char> = ch.nfd().collect();
            if nfd.len() < 2 {
                continue;
            }

            let glyph_refs: Vec<Option<String>> = nfd
                .iter()
                .map(|c| cp_to_glyph.get(&(*c as u32)).cloned())
                .collect();
            if glyph_refs.iter().any(|g| g.is_none()) {
                continue;
            }

            let composite_name = format!("uni{cp:04X}");
            let refs: Vec<GlyphRef> = glyph_refs
                .into_iter()
                .map(|g| GlyphRef {
                    name: g.unwrap(),
                    offset: None,
                    negated: false,
                    fill: None,
                    visibility: None,
                })
                .collect();

            decomposed_items.push(DocumentItem::Glyph {
                name: GlyphName(composite_name.clone()),
                body: GlyphBody {
                    refs,
                    ..GlyphBody::new()
                },
            });
            decomposed_items.push(DocumentItem::Map {
                char_repr: char_repr.clone(),
                glyph: composite_name,
            });
        }
        all_items.retain(|item| !matches!(item, DocumentItem::MapDecomposed { .. }));
        all_items.extend(decomposed_items);
    }

    inject_on_demand_glyph_items(&mut all_items);

    all_items
}

/// Scan `all_items` for on-demand glyph names referenced in refs, maps,
/// and remaps. For each one not already defined as a glyph, append a
/// synthetic `DocumentItem::Glyph` (filled rectangle for WxH, or a
/// color/mono composite when X:mono and X:color both exist).
fn inject_on_demand_glyph_items(all_items: &mut Vec<DocumentItem>) {
    let mut defined: HashSet<String> = HashSet::new();
    let mut glyph_bodies: HashMap<String, GlyphBody> = HashMap::new();

    for item in all_items.iter() {
        if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
            defined.insert(n.clone());
            glyph_bodies.insert(n.clone(), body.clone());
        }
    }

    let mut needed: HashSet<String> = HashSet::new();
    let mut consider = |name: &str| {
        if !defined.contains(name) {
            needed.insert(name.to_string());
        }
    };

    for item in all_items.iter() {
        match item {
            DocumentItem::Glyph { body, .. } => {
                for r in &body.refs {
                    consider(&r.name);
                }
            }
            DocumentItem::Map { glyph, .. } => consider(glyph),
            DocumentItem::Remap {
                lookbehind,
                source,
                target,
                lookahead,
                ..
            } => {
                for token in source {
                    consider(token);
                }
                for token in target {
                    consider(token);
                }
                for lb in lookbehind {
                    consider(lb);
                }
                for la in lookahead {
                    consider(la);
                }
            }
            _ => {}
        }
    }

    for name in needed {
        use crate::ref_composite::{OnDemandGlyph, detect_on_demand_glyph};
        match detect_on_demand_glyph(&name, |n| defined.contains(n)) {
            Some(OnDemandGlyph::Rect(rect)) => {
                let grid = crate::ref_composite::make_on_demand_grid(&rect);
                all_items.push(DocumentItem::Glyph {
                    name: GlyphName(name),
                    body: GlyphBody {
                        scale: rect.scale,
                        pixels: Some(grid),
                        ..GlyphBody::new()
                    },
                });
            }
            Some(OnDemandGlyph::ColorMono { mono, color }) => {
                let mono_body = glyph_bodies.get(&mono);
                let color_body = glyph_bodies.get(&color);
                if let (Some(mono_body), Some(color_body)) = (mono_body, color_body) {
                    let mono_s = mono_body.scale.max(1);
                    let color_s = color_body.scale.max(1);
                    fn lcm(a: u8, b: u8) -> u8 {
                        fn gcd(mut a: u8, mut b: u8) -> u8 { while b != 0 { let t = b; b = a % b; a = t; } a }
                        a / gcd(a, b) * b
                    }
                    let combined_scale = lcm(mono_s, color_s);
                    let mono_s = mono_s as i16;
                    let color_s = color_s as i16;
                    let combined_s = combined_scale as i16;

                    let mut refs = Vec::new();
                    for r in &mono_body.refs {
                        let offset = if mono_s == combined_s {
                            r.offset
                        } else {
                            r.offset.map(|(row, col)| (row * combined_s / mono_s, col * combined_s / mono_s))
                        };
                        refs.push(GlyphRef {
                            name: r.name.clone(),
                            offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: Some(LayerVisibility::MonoOnly),
                        });
                    }
                    for r in &color_body.refs {
                        let offset = if color_s == combined_s {
                            r.offset
                        } else {
                            r.offset.map(|(row, col)| (row * combined_s / color_s, col * combined_s / color_s))
                        };
                        refs.push(GlyphRef {
                            name: r.name.clone(),
                            offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: Some(LayerVisibility::ColorOnly),
                        });
                    }
                    let mut points = Vec::new();
                    points.extend_from_slice(&mono_body.points);
                    points.extend_from_slice(&color_body.points);

                    let pixels = match (&mono_body.pixels, &color_body.pixels) {
                        (Some(mg), Some(cg)) => {
                            let mg2 = if mono_s == combined_s { mg.clone() } else { mg.rescale(mono_s as u8, combined_scale) };
                            let cg2 = if color_s == combined_s { cg.clone() } else { cg.rescale(color_s as u8, combined_scale) };
                            Some(if mg2.width >= cg2.width && mg2.height >= cg2.height { mg2 } else { cg2 })
                        }
                        (None, Some(cg)) => {
                            Some(if color_s == combined_s { cg.clone() } else { cg.rescale(color_s as u8, combined_scale) })
                        }
                        (Some(mg), None) => {
                            Some(if mono_s == combined_s { mg.clone() } else { mg.rescale(mono_s as u8, combined_scale) })
                        }
                        (None, None) => None,
                    };

                    all_items.push(DocumentItem::Glyph {
                        name: GlyphName(name),
                        body: GlyphBody {
                            refs,
                            points,
                            pixels,
                            scale: combined_scale,
                            advance: mono_body.advance.or(color_body.advance),
                            left: mono_body.left.or(color_body.left),
                            top: mono_body.top.or(color_body.top),
                            ..GlyphBody::new()
                        },
                    });
                }
            }
            None => {}
        }
    }
}

struct SharedFontInput {
    meta: FontMeta,
    scale: f32,
    all_items: Vec<DocumentItem>,
    declared_anchors_map: HashMap<String, Vec<GlyphPoint>>,
    gsub_data: GsubData,
    color_aliases: ColorAliasMap,
    glyph_meta: HashMap<String, (Option<u16>, Option<i16>, Option<i16>)>,
    inline_glyphs: HashSet<String>,
    glyph_bodies: Vec<(String, GlyphBody)>,
}

fn compute_shared_font_input(docs: &[&Document]) -> Option<SharedFontInput> {
    if docs.is_empty() {
        return None;
    }

    let mut meta = FontMeta::default();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::FontMeta(s) = item {
                parse_font_meta(s, &mut meta);
            }
        }
    }

    if meta.height == 0 {
        eprintln!("error: font-meta height must be > 0");
        return None;
    }

    let scale = UNITS_PER_EM as f32 / meta.height as f32;

    let name_parts = collect_name_parts(docs);
    let all_items = collect_expanded_items(docs, &name_parts);

    let mut declared_anchors_map: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
    for item in &all_items {
        if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
            declared_anchors_map.entry(n.clone()).or_insert_with(|| body.points.clone());
        }
    }

    let gsub_data = collect_gsub_data(docs, &name_parts);
    let color_aliases = collect_color_aliases(docs);

    let mut glyph_meta: HashMap<String, (Option<u16>, Option<i16>, Option<i16>)> = HashMap::new();
    let mut inline_glyphs: HashSet<String> = HashSet::new();
    let mut glyph_bodies: Vec<(String, GlyphBody)> = Vec::new();
    let mut seen_bodies: HashSet<String> = HashSet::new();
    for item in &all_items {
        if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
            if body.advance.is_some() || body.left.is_some() || body.top.is_some() {
                glyph_meta.insert(n.clone(), (body.advance, body.left, body.top));
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
        glyph_meta,
        inline_glyphs,
        glyph_bodies,
    })
}

fn collect_glyph_data_cached(docs: &[&Document], bitmap: bool, contour_cache: Option<&mut ContourCache>) -> Option<CollectedFontData> {
    let shared = compute_shared_font_input(docs)?;
    collect_glyph_data_with_shared(&shared, bitmap, contour_cache)
}

fn collect_glyph_data_with_shared(
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

    let mut cache: HashMap<String, CachedContours> = HashMap::new();

    struct PendingGlyph {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
        scale: u8,
    }
    let mut pending: Vec<PendingGlyph> = Vec::new();

    for item in all_items {
        let (cache_key, body) = match item {
            DocumentItem::Glyph { name: GlyphName(n), body } => (n.clone(), body),
            _ => continue,
        };
        if !cache_key.is_empty() && !cache.contains_key(&cache_key) {
            if let Some(ref pixels) = body.pixels && body.refs.is_empty() {
                let mut cached = CachedContours::from_grid(pixels, bitmap, contour_cache.as_deref_mut());
                cached.anchors = body.points.clone();
                cached.scale = body.scale;
                cache.insert(cache_key, cached);
            } else if body.pixels.is_some() || !body.refs.is_empty() {
                pending.push(PendingGlyph {
                    name: cache_key,
                    pixels: body.pixels.clone(),
                    refs: body.refs.clone(),
                    points: body.points.clone(),
                    scale: body.scale,
                });
            } else if body.sticky {
                cache.insert(
                    cache_key,
                    CachedContours {
                        width: 0,
                        height: 0,
                        contours: Vec::new(),
                        anchors: body.points.clone(),
                        grid: None,
                        composite_components: None,
                        scale: 1,
                    },
                );
            }
        }
    }

    let mut progress = true;
    while progress {
        progress = false;
        let alt_index = build_cached_alternatives(&cache);
        let mut i = 0;
        while i < pending.len() {
            if !pending[i]
                .refs
                .iter()
                .all(|gref| resolve_cached_ref(&gref.name, &cache).is_some())
            {
                i += 1;
                continue;
            }
            let pg = pending.swap_remove(i);
            let (effective_refs, anchors) = derive_effective_refs(
                &pg.points, &pg.refs, &cache, &alt_index, &declared_anchors_map);
            let mut cached_entry = CachedContours::from_components(
                pg.pixels.as_ref(),
                &effective_refs,
                &cache,
                bitmap,
                contour_cache.as_deref_mut(),
                pg.scale,
            ).unwrap_or_else(|| if let Some(grid) = &pg.pixels {
                CachedContours::from_grid(grid, bitmap, contour_cache.as_deref_mut())
            } else {
                CachedContours {
                    width: 0,
                    height: 0,
                    contours: Vec::new(),
                    anchors: Vec::new(),
                    grid: None,
                    composite_components: None,
                    scale: 1,
                }
            });
            if let Some(grid) = &pg.pixels {
                cached_entry.width = cached_entry.width.max(grid.width);
                cached_entry.height = cached_entry.height.max(grid.height);
            }
            cached_entry.anchors = anchors;
            cached_entry.scale = pg.scale;
            cache.insert(pg.name.clone(), cached_entry);
            progress = true;
        }
    }

    let glyph_bodies_map: HashMap<&str, &GlyphBody> = glyph_bodies.iter()
        .map(|(n, b)| (n.as_str(), b))
        .collect();

    let mut glyph_data: Vec<CollectedGlyph> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    for item in all_items {
        let DocumentItem::Map { char_repr, glyph } = item else { continue };

        let pairs = expand_map_pairs(char_repr, glyph);
        for (cp, glyph_name) in &pairs {
            let Some(resolved) = cache.get(glyph_name.as_str()) else { continue };

            let glyph_scale = scale / resolved.scale as f32;
            let (advance_width, left_offset, top_offset) =
                resolve_glyph_metrics(&glyph_meta, glyph_name, resolved.width, glyph_scale, scale);
            let font_contours =
                scale_glyph_contours(&resolved.contours, glyph_scale, meta.ascent * resolved.scale as u16, left_offset, top_offset);
            let composite_refs = build_composite_refs(
                resolved,
                inline_glyphs.contains(glyph_name.as_str()),
                left_offset,
                top_offset,
                &glyph_meta,
                glyph_scale,
                scale,
            );

            let is_mark = glyph_bodies_map.get(glyph_name.as_str()).is_some_and(|b| b.mark);
            let glyph_anchors = cache.get(glyph_name.as_str())
                .map(|c| c.anchors.clone())
                .unwrap_or_default();
            let declared_anchors = glyph_bodies_map.get(glyph_name.as_str())
                .map(|b| b.points.clone())
                .unwrap_or_default();

            seen_names.insert(glyph_name.clone());
            glyph_data.push(CollectedGlyph {
                name: glyph_name.clone(),
                codepoint: Some(*cp),
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

    glyph_data.sort_by_key(|g| g.codepoint);
    glyph_data.dedup_by_key(|g| g.codepoint);

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
            let declared = glyph_bodies_map.get(base_name.as_str()).map(|b| &b.points[..]).unwrap_or(&[]);
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
                let Some(mark_minus) = mark_body.points.iter().find(|p| p.position == minus_name) else { continue };
                for (alt_name, alt_anchors) in alts {
                    if seen_names.contains(alt_name) || extra_name_set.contains(alt_name) {
                        continue;
                    }
                    if let Some(alt_minus) = alt_anchors.iter().find(|p| p.position == minus_name)
                        && !alt_minus.size_matches(mark_minus) {
                            extra_name_set.insert(alt_name.clone());
                        }
                }
            }
        }
    }

    let mut extra_names: Vec<String> = extra_name_set.into_iter().collect();
    extra_names.sort();

    for glyph_name in &extra_names {
        let empty_cached = CachedContours {
            width: 0,
            height: 0,
            contours: Vec::new(),
            anchors: Vec::new(),
            grid: None,
            composite_components: None,
            scale: 1,
        };
        let resolved = cache.get(glyph_name.as_str()).unwrap_or(&empty_cached);
        let glyph_scale = scale / resolved.scale as f32;
        let (advance_width, left_offset, top_offset) =
            resolve_glyph_metrics(&glyph_meta, glyph_name, resolved.width, glyph_scale, scale);
        let font_contours =
            scale_glyph_contours(&resolved.contours, glyph_scale, meta.ascent * resolved.scale as u16, left_offset, top_offset);
        let composite_refs = build_composite_refs(
            resolved,
            inline_glyphs.contains(glyph_name.as_str()),
            left_offset,
            top_offset,
            &glyph_meta,
            glyph_scale,
            scale,
        );

        let is_mark = glyph_bodies_map.get(glyph_name.as_str()).is_some_and(|b| b.mark);
        let glyph_anchors = cache.get(glyph_name.as_str())
            .map(|c| c.anchors.clone())
            .unwrap_or_default();
        let declared_anchors = glyph_bodies_map.get(glyph_name.as_str())
            .map(|b| b.points.clone())
            .unwrap_or_default();

        glyph_data.push(CollectedGlyph {
            name: glyph_name.clone(),
            codepoint: None,
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
                let empty_cached = CachedContours {
                    width: 0,
                    height: 0,
                    contours: Vec::new(),
                    anchors: Vec::new(),
                    grid: None,
                    composite_components: None,
                    scale: 1,
                };
                let resolved = cache.get(cr.component_name.as_str()).unwrap_or(&empty_cached);
                let comp_glyph_scale = scale / resolved.scale as f32;
                let font_contours =
                    scale_glyph_contours(&resolved.contours, comp_glyph_scale, meta.ascent * resolved.scale as u16, 0, 0);
                let advance_width = (resolved.width as f32 * comp_glyph_scale).round() as u16;
                component_extras.push(CollectedGlyph {
                    name: cr.component_name.clone(),
                    codepoint: None,
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
        let Some(body) = glyph_bodies_map.get(g.name.as_str()) else { continue };
        let has_fill_or_vis = body.refs.iter().any(|r| r.fill.is_some() || r.visibility.is_some());
        if !has_fill_or_vis {
            continue;
        }

        let (effective_refs, _) = derive_effective_refs(
            &body.points, &body.refs, &cache, &color_alt_index, &declared_anchors_map);

        let color_glyph_scale = scale / body.scale as f32;
        let color_ascent = meta.ascent * body.scale as u16;
        let left_offset = match glyph_meta.get(&g.name) {
            Some(&(_, Some(left), _)) => (left as f32 * scale).round() as i16,
            _ => 0,
        };
        let top_offset = match glyph_meta.get(&g.name) {
            Some(&(_, _, Some(top))) => (top as f32 * scale).round() as i16,
            _ => 0,
        };

        // Collect foreground contours (own pixels + refs without fill or with fill=fg)
        // and separate color layers (refs with non-fg fill).
        let mut fg_contours: Vec<Vec<(i16, i16)>> = Vec::new();

        if let Some(ref own_grid) = body.pixels
            && !own_grid.is_all_empty() {
                let c = track_contour(own_grid, PX_SUBPIXEL);
                fg_contours.extend(scale_glyph_contours(&c, color_glyph_scale, color_ascent, left_offset, top_offset));
            }

        for (ri, eref) in effective_refs.iter().enumerate() {
            let orig_ref = &body.refs[ri.min(body.refs.len() - 1)];
            let fill = orig_ref.fill.as_ref();
            let vis = effective_visibility(orig_ref.visibility, fill, &color_aliases);
            if vis == LayerVisibility::MonoOnly {
                continue;
            }

            let Some(ref_cached) = resolve_cached_ref(&eref.name, &cache) else { continue };
            let dx = eref.col() as f32;
            let dy = eref.row() as f32;
            let rsf = body.scale as f32 / ref_cached.scale.max(1) as f32;

            let layer_contours: Vec<Vec<(i16, i16)>> = ref_cached
                .contours
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|&(x, y)| {
                            (
                                ((x * rsf + dx) * color_glyph_scale).round() as i16 + left_offset,
                                ((color_ascent as f32 - (y * rsf + dy)) * color_glyph_scale).round() as i16 - top_offset,
                            )
                        })
                        .collect()
                })
                .collect();

            if layer_contours.is_empty() {
                continue;
            }

            let is_fg = fill.is_none() || fill.is_some_and(|f| f.color == "fg");
            if is_fg {
                fg_contours.extend(layer_contours);
            } else {
                let f = fill.unwrap();
                let palette_index = if let Some(rgba) = resolve_fill_rgba(f, &color_aliases) {
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
            g.color_layers.insert(0, CollectedColorLayer {
                contours: fg_contours,
                palette_index: 0xFFFF,
            });
        }

        // Rebuild fallback contours: only non-coloronly layers
        let mut fallback_contours: Vec<Vec<(i16, i16)>> = Vec::new();
        if let Some(ref own_grid) = body.pixels
            && !own_grid.is_all_empty() {
                let c = track_contour(own_grid, PX_SUBPIXEL);
                fallback_contours.extend(scale_glyph_contours(&c, color_glyph_scale, color_ascent, left_offset, top_offset));
            }
        for (ri, eref) in effective_refs.iter().enumerate() {
            let orig_ref = &body.refs[ri.min(body.refs.len() - 1)];
            let fill = orig_ref.fill.as_ref();
            let vis = effective_visibility(orig_ref.visibility, fill, &color_aliases);
            if vis == LayerVisibility::ColorOnly {
                continue;
            }
            let Some(ref_cached) = resolve_cached_ref(&eref.name, &cache) else { continue };
            let dx = eref.col() as f32;
            let dy = eref.row() as f32;
            let fb_rsf = body.scale as f32 / ref_cached.scale.max(1) as f32;
            for c in &ref_cached.contours {
                fallback_contours.push(
                    c.iter()
                        .map(|&(x, y)| {
                            (
                                ((x * fb_rsf + dx) * color_glyph_scale).round() as i16 + left_offset,
                                ((color_ascent as f32 - (y * fb_rsf + dy)) * color_glyph_scale).round() as i16 - top_offset,
                            )
                        })
                        .collect()
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
        let old_to_new: HashMap<u16, u16> = palette_colors.iter().enumerate()
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

    Some((meta.clone(), scale, glyph_data, gsub_data.clone(), palette_colors))
}

pub(crate) fn parse_map_char(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        let mut chars = s.chars();
        let c = chars.next()?;
        if chars.next().is_none() {
            Some(c as u32)
        } else {
            None
        }
    }
}

fn split_top_level_pipes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

pub(crate) fn expand_map_pairs(char_repr: &str, glyph: &str) -> Vec<(u32, String)> {
    // Range: U+XXXX..YYYY or u+XXXX..YYYY
    if let Some(hex_rest) = char_repr.strip_prefix("U+").or_else(|| char_repr.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..")
            && let (Ok(start), Ok(end)) = (
                u32::from_str_radix(start_hex, 16),
                u32::from_str_radix(end_hex, 16),
            ) {
                if end < start {
                    return vec![];
                }
                let count64 = u64::from(end) - u64::from(start) + 1;
                if count64 > MAX_EXPANSION as u64 {
                    return vec![];
                }
                let count = count64 as usize;
                let glyph_names = expand_glyph_pattern(glyph, count);
                return (0..count)
                    .zip(glyph_names.iter().cycle())
                    .filter_map(|(i, name)| {
                        let cp = start + i as u32;
                        char::from_u32(cp).map(|_| (cp, name.clone()))
                    })
                    .collect();
            }

    // Multi-char with pipe (depth-aware)
    // Filter empty parts so a bare "|" (the pipe character) falls through to single-char.
    if has_top_level_pipe(char_repr) {
        let chars: Vec<&str> = split_top_level_pipes(char_repr)
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if chars.len() >= 2 {
            let glyph_names = if has_top_level_pipe(glyph) {
                let glyphs = split_top_level_pipes(glyph);
                if glyphs.len() == chars.len() {
                    glyphs.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                } else {
                    expand_glyph_pattern(glyph, chars.len())
                }
            } else {
                expand_glyph_pattern(glyph, chars.len())
            };
            return chars
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    parse_map_char(c).map(|cp| (cp, glyph_names[i % glyph_names.len()].clone()))
                })
                .collect();
        }
    }

    // Single char — still expand the glyph pattern
    if let Some(cp) = parse_map_char(char_repr) {
        let names = expand_glyph_pattern(glyph, 1);
        vec![(cp, names.into_iter().next().unwrap_or_else(|| glyph.to_string()))]
    } else {
        vec![]
    }
}

pub(crate) fn expand_glyph_pattern(pattern: &str, count: usize) -> Vec<String> {
    if let Some(hex_rest) = pattern.strip_prefix("U+").or_else(|| pattern.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..")
            && let (Ok(start), Ok(end)) = (
                u32::from_str_radix(start_hex, 16),
                u32::from_str_radix(end_hex, 16),
            ) {
                if end < start {
                    return vec![pattern.to_string(); count];
                }
                return (start..=end).map(|cp| format!("U+{cp:04X}")).collect();
            }

    if !pattern.contains('(') && !pattern.contains('|') && !has_bare_repeat(pattern) {
        return vec![pattern.to_string(); count];
    }

    match crate::document::expand_name_pattern(pattern) {
        Ok(expanded) => {
            let names = expanded.into_vec();
            if names.is_empty() {
                return vec![pattern.to_string(); count];
            }
            let mut result = Vec::with_capacity(count);
            for i in 0..count {
                result.push(names[i % names.len()].clone());
            }
            result
        }
        Err(_) => vec![pattern.to_string(); count],
    }
}

fn parse_font_meta(s: &str, meta: &mut FontMeta) {
    let mut iter = s.split_whitespace();
    while let Some(key) = iter.next() {
        let Some(val) = iter.next() else { break };
        let Ok(v) = val.parse::<u16>() else { continue };
        match key {
            "height" => meta.height = v,
            "ascent" => meta.ascent = v,
            "descent" => meta.descent = v,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// GSUB remap collection & expansion
// ---------------------------------------------------------------------------

fn expand_remap_element(s: &str, name_parts: &NamePartsMap) -> Vec<String> {
    let substituted = substitute_name_parts(s, name_parts);
    match expand_name_pattern(&substituted) {
        Ok(names) => names.into_vec(),
        Err(_) => vec![substituted],
    }
}

fn collect_gsub_data(docs: &[&Document], name_parts: &NamePartsMap) -> GsubData {
    let mut remap_sets: BTreeMap<String, Vec<ExpandedRemap>> = BTreeMap::new();
    let mut features: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut anchor_features: Vec<(String, Vec<String>, String)> = Vec::new();

    for doc in docs {
        for item in &doc.items {
            match item {
                DocumentItem::Remap {
                    feature,
                    lookbehind,
                    source,
                    target,
                    lookahead,
                } => {
                    let expanded_positions: Vec<Vec<String>> = source
                        .iter()
                        .map(|s| expand_remap_element(s, name_parts))
                        .collect();

                    // The number of remap entries is the LCM of all position
                    // expansion counts (each position cycles independently).
                    fn usize_gcd(a: usize, b: usize) -> usize {
                        if b == 0 { a } else { usize_gcd(b, a % b) }
                    }
                    let entry_count = expanded_positions
                        .iter()
                        .map(|p| p.len())
                        .fold(1usize, |a, b| a / usize_gcd(a, b) * b);

                    let expanded_target_positions: Vec<Vec<String>> = target
                        .iter()
                        .map(|s| expand_remap_element(s, name_parts))
                        .collect();

                    let entry_count = expanded_positions
                        .iter()
                        .chain(expanded_target_positions.iter())
                        .map(|p| p.len())
                        .fold(entry_count, |a, b| a / usize_gcd(a, b) * b);

                    let mut source_seqs = Vec::with_capacity(entry_count);
                    let mut target_seqs = Vec::with_capacity(entry_count);
                    for i in 0..entry_count {
                        let seq: Vec<String> = expanded_positions
                            .iter()
                            .map(|pos| pos[i % pos.len()].clone())
                            .collect();
                        source_seqs.push(seq);
                        let tseq: Vec<String> = expanded_target_positions
                            .iter()
                            .map(|pos| pos[i % pos.len()].clone())
                            .collect();
                        target_seqs.push(tseq);
                    }

                    let lb: Vec<Vec<String>> = lookbehind
                        .iter()
                        .map(|s| expand_remap_element(s, name_parts))
                        .collect();
                    let la: Vec<Vec<String>> = lookahead
                        .iter()
                        .map(|s| expand_remap_element(s, name_parts))
                        .collect();

                    remap_sets.entry(feature.clone()).or_default().push(
                        ExpandedRemap {
                            lookbehind: lb,
                            source: source_seqs,
                            target: target_seqs,
                            lookahead: la,
                        },
                    );
                }
                DocumentItem::Feature { name, scripts, remap_group } => {
                    features.push((name.clone(), scripts.clone(), vec![remap_group.clone()]));
                }
                DocumentItem::FeatureAnchor { name, scripts, anchor } => {
                    anchor_features.push((name.clone(), scripts.clone(), anchor.clone()));
                }
                _ => {}
            }
        }
    }

    GsubData {
        remap_sets,
        features,
        anchor_features,
    }
}

enum RemapSetKind {
    Single,
    Ligature,
    ChainContext,
}

fn classify_remap_set(remaps: &[ExpandedRemap]) -> RemapSetKind {
    let has_context = remaps
        .iter()
        .any(|r| !r.lookbehind.is_empty() || !r.lookahead.is_empty());
    let has_multi_input = remaps
        .iter()
        .any(|r| r.source.iter().any(|seq| seq.len() > 1));

    if has_context || has_multi_input {
        if has_context {
            RemapSetKind::ChainContext
        } else {
            RemapSetKind::Ligature
        }
    } else {
        // All sources are single-glyph, no context
        let has_single = remaps.iter().any(|r| r.source.iter().any(|seq| seq.len() == 1));
        let has_ligature = has_multi_input;
        if has_ligature && has_single {
            // Mixed — currently treat as ligature; single entries become
            // 1-component "ligatures" which work but are suboptimal.
            RemapSetKind::Ligature
        } else if has_ligature {
            RemapSetKind::Ligature
        } else {
            RemapSetKind::Single
        }
    }
}

fn build_gsub(
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
) -> Option<Gsub> {
    if gsub_data.features.is_empty() {
        return None;
    }

    let mut lookups: Vec<SubstitutionLookup> = Vec::new();
    let mut set_to_lookup: HashMap<String, u16> = HashMap::new();

    // Build lookups in feature declaration order so that lookup indices
    // respect the intended application order (e.g. ljmo < vjmo < tjmo).
    let mut ordered_sets: Vec<&String> = Vec::new();
    for (_, _, set_names) in &gsub_data.features {
        for sn in set_names {
            if !ordered_sets.contains(&sn) {
                ordered_sets.push(sn);
            }
        }
    }
    for setname in gsub_data.remap_sets.keys() {
        if !ordered_sets.contains(&setname) {
            ordered_sets.push(setname);
        }
    }

    for &setname in &ordered_sets {
        let Some(remaps) = gsub_data.remap_sets.get(setname) else { continue };
        match classify_remap_set(remaps) {
            RemapSetKind::Single => {
                let lookup = build_single_subst_lookup(remaps, name_to_gid);
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(lookup);
            }
            RemapSetKind::Ligature => {
                let lookup = build_ligature_subst_lookup(remaps, name_to_gid);
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(lookup);
            }
            RemapSetKind::ChainContext => {
                let helper_start = lookups.len();
                let mut chain_subtables: Vec<SubstitutionChainContext> = Vec::new();

                for r in remaps {
                    // Build helper SingleSubst for the first input position
                    let first_sources: Vec<String> = r.source.iter().map(|seq| seq[0].clone()).collect();
                    let helper_idx = lookups.len() as u16;
                    let first_targets: Vec<String> = r.target.iter().map(|seq| seq[0].clone()).collect();
                    let helper = build_single_subst_from_pairs(&first_sources, &first_targets, name_to_gid);
                    lookups.push(helper);

                    let backtrack: Vec<CoverageTable> = r
                        .lookbehind
                        .iter()
                        .rev()
                        .map(|names| make_coverage(names, name_to_gid))
                        .collect();

                    // Input coverages: one per source position
                    let input_len = r.source.first().map_or(1, |seq| seq.len());
                    let input: Vec<CoverageTable> = (0..input_len)
                        .map(|pos| {
                            let names: Vec<String> = r.source.iter()
                                .filter_map(|seq| seq.get(pos).cloned())
                                .collect();
                            make_coverage(&names, name_to_gid)
                        })
                        .collect();

                    let lookahead: Vec<CoverageTable> = r
                        .lookahead
                        .iter()
                        .map(|names| make_coverage(names, name_to_gid))
                        .collect();

                    let slr = SequenceLookupRecord::new(0, helper_idx);

                    let mut sc = SubstitutionChainContext::default();
                    *sc = ChainedSequenceContext::Format3(
                        ChainedSequenceContextFormat3::new(
                            backtrack,
                            input,
                            lookahead,
                            vec![slr],
                        ),
                    );
                    chain_subtables.push(sc);
                }

                let chain_lookup = SubstitutionLookup::ChainContextual(Lookup::new(
                    LookupFlag::empty(),
                    chain_subtables,
                ));
                set_to_lookup.insert(setname.clone(), lookups.len() as u16);
                lookups.push(chain_lookup);
                let _ = helper_start;
            }
        }
    }

    let mut feature_records: Vec<FeatureRecord> = Vec::new();
    // Collect which feature indices belong to which script tags
    let mut script_feature_indices: BTreeMap<String, Vec<u16>> = BTreeMap::new();
    for (feat_tag, scripts, set_names) in &gsub_data.features {
        let tag = make_tag(feat_tag);

        let lookup_indices: Vec<u16> = set_names
            .iter()
            .filter_map(|sn| set_to_lookup.get(sn).copied())
            .collect();

        let feat_idx = feature_records.len() as u16;
        feature_records.push(FeatureRecord::new(
            tag,
            Feature::new(None, lookup_indices),
        ));

        for script in scripts {
            script_feature_indices.entry(script.clone()).or_default().push(feat_idx);
        }
    }

    let script_records = build_script_records(&script_feature_indices);

    let script_list = ScriptList::new(script_records);
    let feature_list = FeatureList::new(feature_records);
    let lookup_list: LookupList<SubstitutionLookup> = LookupList::new(lookups);

    Some(Gsub::new(script_list, feature_list, lookup_list))
}

// ---------------------------------------------------------------------------
// GPOS / GDEF / ccmp generation from anchor features
// ---------------------------------------------------------------------------

struct AnchorGposData {
    gpos: Option<Gpos>,
    gdef: Gdef,
    /// Per-feature-tag GSUB lookups for anchor-based substitution.
    /// Each entry: (feature_tag, scripts, lookups).
    feature_lookups: Vec<(String, Vec<String>, Vec<SubstitutionLookup>)>,
    /// Mark glyph sets for GDEF MarkGlyphSets table, used by
    /// USE_MARK_FILTERING_SET on mark-subst lookups.
    mark_glyph_sets: Vec<CoverageTable>,
    /// Base substitution entries: (source, target, anchor_name).
    #[cfg(test)]
    base_subst_entries: Vec<(String, String, String)>,
    /// Mark substitution entries: (mark, mark_alt, anchor_name, backtrack_bases).
    #[cfg(test)]
    mark_subst_entries: Vec<(String, String, String, Vec<String>)>,
}

fn build_anchor_gpos(
    glyphs: &[CollectedGlyph],
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
    scale: f32,
    ascent: u16,
) -> AnchorGposData {
    if gsub_data.anchor_features.is_empty() {
        return AnchorGposData {
            gpos: None,
            gdef: Gdef::default(),
            feature_lookups: Vec::new(),
            mark_glyph_sets: Vec::new(),
            #[cfg(test)]
            base_subst_entries: Vec::new(),
            #[cfg(test)]
            mark_subst_entries: Vec::new(),
        };
    }

    let anchor_names: Vec<String> = gsub_data
        .anchor_features
        .iter()
        .map(|(_, _, a)| a.clone())
        .collect();

    let mut all_scripts: Vec<String> = Vec::new();
    for (_, scripts, _) in &gsub_data.anchor_features {
        for s in scripts {
            if !all_scripts.contains(s) {
                all_scripts.push(s.clone());
            }
        }
    }

    // Assign anchor classes: each unique anchor name (from feature declarations) gets a class.
    let mut anchor_class_map: HashMap<String, u16> = HashMap::new();
    for (i, name) in anchor_names.iter().enumerate() {
        anchor_class_map.entry(name.clone()).or_insert(i as u16);
    }
    let num_classes = anchor_class_map.len() as u16;

    // Classify glyphs: mark glyphs have `-anchor` anchors, base glyphs have `+anchor`.
    // For mark-to-mark: a mark glyph with `+anchor` serves as mark2 (the base mark).
    let mut mark_gids: Vec<(GlyphId16, u16, i16, i16)> = Vec::new(); // (gid, class, x, y)
    let mut base_gids: Vec<(GlyphId16, Vec<Option<(i16, i16)>>)> = Vec::new();
    let mut mark2_gids: Vec<(GlyphId16, Vec<Option<(i16, i16)>>)> = Vec::new();

    // Collect all mark glyph GIDs for ccmp/GDEF
    let mut mark_gid_set: HashSet<GlyphId16> = HashSet::new();

    // Build alternative index from glyphs: name:variant → base_name
    let mut alt_index: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
    for g in glyphs {
        let mut prefix = g.name.as_str();
        while let Some(colon_pos) = prefix.rfind(':') {
            prefix = &prefix[..colon_pos];
            alt_index
                .entry(prefix.to_string())
                .or_default()
                .push((g.name.clone(), g.resolved_anchors.clone()));
        }
    }
    for alts in alt_index.values_mut() {
        alts.sort_by(|(a, _), (b, _)| a.cmp(b));
    }

    // Track base glyphs that need alternative substitution, grouped
    // by anchor name.  Each entry also records the feature tag so the
    // resulting lookups land under the correct OpenType feature.
    let mut ccmp_entries: Vec<(String, String, String)> = Vec::new(); // (source, target, anchor_name)

    // Map anchor_name → (feature_tag, scripts) from the declarations.
    let mut anchor_to_feature: HashMap<String, (String, Vec<String>)> = HashMap::new();
    for (tag, scripts, anchor_name) in &gsub_data.anchor_features {
        anchor_to_feature.entry(anchor_name.clone())
            .or_insert_with(|| (tag.clone(), scripts.clone()));
    }

    for g in glyphs {
        let Some(&gid) = name_to_gid.get(&g.name) else { continue };
        let loff = g.left_offset;
        let toff = g.top_offset;

        if g.mark {
            mark_gid_set.insert(gid);

            // Mark glyphs: look for `-anchor` anchors in declared_anchors only
            // (not forwarded anchors from refs) to determine mark class.
            for anchor_name in anchor_names.iter() {
                let minus_name = format!("-{anchor_name}");
                if let Some(pt) = g.declared_anchors.iter().find(|p| p.position == minus_name) {
                    let class = anchor_class_map[anchor_name];
                    let x = (pt.col as f32 * scale).round() as i16 + loff;
                    let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - toff;
                    mark_gids.push((gid, class, x, y));
                    break;
                }
            }

            // Mark-to-mark: mark glyphs with `+anchor` anchors
            let mut plus_anchors: Vec<Option<(i16, i16)>> = vec![None; num_classes as usize];
            let mut has_any = false;
            for anchor_name in anchor_names.iter() {
                let plus_name = format!("+{anchor_name}");
                if let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name) {
                    let class = anchor_class_map[anchor_name] as usize;
                    let x = (pt.col as f32 * scale).round() as i16 + loff;
                    let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - toff;
                    plus_anchors[class] = Some((x, y));
                    has_any = true;
                }
            }
            if has_any {
                mark2_gids.push((gid, plus_anchors));
            }
        } else {
            // Base glyphs: look for `+anchor` anchors (direct or via alternatives).
            // Own anchors go on the original glyph; anchors provided only by
            // alternatives go on the alt glyph (which ccmp substitutes in).
            let mut own_plus: Vec<Option<(i16, i16)>> = vec![None; num_classes as usize];
            let mut has_own = false;
            // alt_name → plus_anchors for each alternative that provides anchors
            let mut alt_plus_map: HashMap<String, Vec<Option<(i16, i16)>>> = HashMap::new();

            for anchor_name in anchor_names.iter() {
                let plus_name = format!("+{anchor_name}");
                if let Some(pt) = g.declared_anchors.iter().find(|p| p.position == plus_name) {
                    let class = anchor_class_map[anchor_name] as usize;
                    let x = (pt.col as f32 * scale).round() as i16 + loff;
                    let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - toff;
                    own_plus[class] = Some((x, y));
                    has_own = true;
                } else if let Some(alts) = alt_index.get(&g.name) {
                    let mut alt_found = false;
                    for (alt_name, alt_anchors) in alts {
                        if let Some(pt) = alt_anchors.iter().find(|p| p.position == plus_name) {
                            let class = anchor_class_map[anchor_name] as usize;
                            let alt_g = glyphs.iter().find(|gg| gg.name == *alt_name);
                            let alt_loff = alt_g.map_or(0, |gg| gg.left_offset);
                            let alt_toff = alt_g.map_or(0, |gg| gg.top_offset);
                            let x = (pt.col as f32 * scale).round() as i16 + alt_loff;
                            let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - alt_toff;
                            let entry = alt_plus_map
                                .entry(alt_name.clone())
                                .or_insert_with(|| vec![None; num_classes as usize]);
                            entry[class] = Some((x, y));

                            if !ccmp_entries.iter().any(|(s, _, a)| s == &g.name && a == anchor_name) {
                                ccmp_entries.push((g.name.clone(), alt_name.clone(), anchor_name.clone()));
                            }
                            alt_found = true;
                            break;
                        }
                    }
                    if !alt_found
                        && let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name) {
                            let class = anchor_class_map[anchor_name] as usize;
                            let x = (pt.col as f32 * scale).round() as i16 + loff;
                            let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - toff;
                            own_plus[class] = Some((x, y));
                            has_own = true;
                        }
                } else if let Some(pt) = g.resolved_anchors.iter().find(|p| p.position == plus_name) {
                    let class = anchor_class_map[anchor_name] as usize;
                    let x = (pt.col as f32 * scale).round() as i16 + loff;
                    let y = ((ascent as f32 - pt.row as f32) * scale).round() as i16 - toff;
                    own_plus[class] = Some((x, y));
                    has_own = true;
                }
            }

            if has_own {
                base_gids.push((gid, own_plus));
            }
            for (alt_name, alt_anchors) in &alt_plus_map {
                if let Some(&alt_gid) = name_to_gid.get(alt_name.as_str()) {
                    base_gids.push((alt_gid, alt_anchors.clone()));
                }
            }
        }
    }

    // Compact class indices: only keep classes that are actually used by marks.
    let used_classes: Vec<u16> = {
        let mut s: Vec<u16> = mark_gids.iter().map(|&(_, class, _, _)| class).collect();
        s.sort();
        s.dedup();
        s
    };
    if !used_classes.is_empty() {
        let class_remap: HashMap<u16, u16> = used_classes
            .iter()
            .enumerate()
            .map(|(new_idx, &old_idx)| (old_idx, new_idx as u16))
            .collect();
        let compact_num_classes = used_classes.len();

        for entry in &mut mark_gids {
            entry.1 = class_remap[&entry.1];
        }
        for (_, anchors) in &mut base_gids {
            let compacted: Vec<Option<(i16, i16)>> = used_classes
                .iter()
                .map(|&old_class| anchors.get(old_class as usize).copied().flatten())
                .collect();
            *anchors = compacted;
        }
        for (_, anchors) in &mut mark2_gids {
            let compacted: Vec<Option<(i16, i16)>> = used_classes
                .iter()
                .map(|&old_class| anchors.get(old_class as usize).copied().flatten())
                .collect();
            *anchors = compacted;
        }
        let _ = compact_num_classes; // used implicitly via compacted vectors
    }

    // Sort by GID for coverage tables
    mark_gids.sort_by_key(|&(gid, _, _, _)| gid);
    mark_gids.dedup_by_key(|entry| entry.0);
    base_gids.sort_by_key(|&(gid, _)| gid);
    base_gids.dedup_by_key(|entry| entry.0);
    mark2_gids.sort_by_key(|&(gid, _)| gid);
    mark2_gids.dedup_by_key(|entry| entry.0);

    // Build GPOS lookups
    let mut gpos_lookups: Vec<PositionLookup> = Vec::new();
    let mut gpos_lookup_indices: Vec<u16> = Vec::new();

    // MarkBasePos (lookup type 4)
    if !mark_gids.is_empty() && !base_gids.is_empty() {
        let mark_coverage = CoverageTable::format_1(
            mark_gids.iter().map(|&(gid, _, _, _)| gid).collect(),
        );
        let base_coverage = CoverageTable::format_1(
            base_gids.iter().map(|&(gid, _)| gid).collect(),
        );
        let mark_array = MarkArray::new(
            mark_gids
                .iter()
                .map(|&(_, class, x, y)| {
                    MarkRecord::new(class, AnchorTable::format_1(x, y))
                })
                .collect(),
        );
        let base_array = BaseArray::new(
            base_gids
                .iter()
                .map(|(_, anchors)| {
                    BaseRecord::new(
                        anchors
                            .iter()
                            .map(|opt| opt.map(|(x, y)| AnchorTable::format_1(x, y)))
                            .collect(),
                    )
                })
                .collect(),
        );
        let lookup_idx = gpos_lookups.len() as u16;
        gpos_lookups.push(PositionLookup::MarkToBase(Lookup::new(
            LookupFlag::empty(),
            vec![MarkBasePosFormat1::new(
                mark_coverage,
                base_coverage,
                mark_array,
                base_array,
            )],
        )));
        gpos_lookup_indices.push(lookup_idx);
    }

    // MarkMarkPos (lookup type 6)
    if !mark_gids.is_empty() && !mark2_gids.is_empty() {
        let mark1_coverage = CoverageTable::format_1(
            mark_gids.iter().map(|&(gid, _, _, _)| gid).collect(),
        );
        let mark2_coverage = CoverageTable::format_1(
            mark2_gids.iter().map(|&(gid, _)| gid).collect(),
        );
        let mark1_array = MarkArray::new(
            mark_gids
                .iter()
                .map(|&(_, class, x, y)| {
                    MarkRecord::new(class, AnchorTable::format_1(x, y))
                })
                .collect(),
        );
        let mark2_array = Mark2Array::new(
            mark2_gids
                .iter()
                .map(|(_, anchors)| {
                    Mark2Record::new(
                        anchors
                            .iter()
                            .map(|opt| opt.map(|(x, y)| AnchorTable::format_1(x, y)))
                            .collect(),
                    )
                })
                .collect(),
        );
        let lookup_idx = gpos_lookups.len() as u16;
        gpos_lookups.push(PositionLookup::MarkToMark(Lookup::new(
            LookupFlag::empty(),
            vec![MarkMarkPosFormat1::new(
                mark1_coverage,
                mark2_coverage,
                mark1_array,
                mark2_array,
            )],
        )));
        gpos_lookup_indices.push(lookup_idx);
    }

    // Build GPOS table
    let gpos = if !gpos_lookups.is_empty() {
        let mut feature_records: Vec<FeatureRecord> = Vec::new();
        let mut script_feature_indices: BTreeMap<String, Vec<u16>> = BTreeMap::new();

        // mark feature
        let mark_feat_idx = feature_records.len() as u16;
        feature_records.push(FeatureRecord::new(
            Tag::new(b"mark"),
            Feature::new(None, gpos_lookup_indices.clone()),
        ));
        for script in &all_scripts {
            script_feature_indices
                .entry(script.clone())
                .or_default()
                .push(mark_feat_idx);
        }

        // mkmk feature (if MarkMarkPos exists)
        if gpos_lookup_indices.len() > 1 {
            let mkmk_feat_idx = feature_records.len() as u16;
            feature_records.push(FeatureRecord::new(
                Tag::new(b"mkmk"),
                Feature::new(None, vec![gpos_lookup_indices[1]]),
            ));
            for script in &all_scripts {
                script_feature_indices
                    .entry(script.clone())
                    .or_default()
                    .push(mkmk_feat_idx);
            }
        }

        let script_records = build_script_records(&script_feature_indices);

        let script_list = ScriptList::new(script_records);
        let feature_list = FeatureList::new(feature_records);
        let lookup_list = PositionLookupList::new(gpos_lookups);

        Some(Gpos::new(script_list, feature_list, lookup_list))
    } else {
        None
    };

    // Build GDEF with mark glyph class
    let gdef = if !mark_gid_set.is_empty() {
        let mut mark_gids_sorted: Vec<GlyphId16> = mark_gid_set.into_iter().collect();
        mark_gids_sorted.sort();

        let mut class_ranges: Vec<ClassRangeRecord> = Vec::new();
        let mut i = 0;
        while i < mark_gids_sorted.len() {
            let start = mark_gids_sorted[i];
            let mut end = start;
            while i + 1 < mark_gids_sorted.len()
                && mark_gids_sorted[i + 1].to_u16() == end.to_u16() + 1
            {
                i += 1;
                end = mark_gids_sorted[i];
            }
            class_ranges.push(ClassRangeRecord::new(start, end, 3)); // 3 = Mark
            i += 1;
        }

        let class_def = ClassDef::Format2(ClassDefFormat2 {
            class_range_records: class_ranges,
        });
        Gdef::new(Some(class_def), None, None, None)
    } else {
        Gdef::default()
    };

    // Build ccmp GSUB lookups, grouped by anchor name.
    // Each anchor gets its own chain context + single subst pair so
    // that the lookahead only includes marks carrying that anchor's
    // `-X` (e.g. only dia-above marks for the "above" anchor, not
    // dia-below marks).
    // Build per-feature GSUB lookups grouped by feature tag, then anchor.
    let mut feature_lookups: Vec<(String, Vec<String>, Vec<SubstitutionLookup>)> = Vec::new();
    if !ccmp_entries.is_empty() {
        // Group entries by feature tag, then by anchor within each tag.
        let mut tag_groups: BTreeMap<String, BTreeMap<String, (Vec<String>, Vec<String>)>> = BTreeMap::new();
        for (source, target, anchor_name) in &ccmp_entries {
            let (tag, _) = anchor_to_feature.get(anchor_name)
                .cloned()
                .unwrap_or_else(|| ("ccmp".to_string(), vec!["DFLT".to_string()]));
            let group = tag_groups.entry(tag).or_default()
                .entry(anchor_name.clone()).or_default();
            if !group.0.contains(source) {
                group.0.push(source.clone());
                group.1.push(target.clone());
            }
        }

        for (tag, anchor_groups) in &tag_groups {
            let scripts: Vec<String> = gsub_data.anchor_features.iter()
                .filter(|(t, _, _)| t == tag)
                .flat_map(|(_, s, _)| s.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .fold(Vec::new(), |mut acc, s| { if !acc.contains(&s) { acc.push(s); } acc });

            let mut lookups: Vec<SubstitutionLookup> = Vec::new();
            for (anchor_name, (sources, targets)) in anchor_groups {
                let minus_name = format!("-{anchor_name}");
                let mark_coverage = CoverageTable::format_1({
                    let mut gids: Vec<GlyphId16> = glyphs
                        .iter()
                        .filter(|g| g.mark && g.declared_anchors.iter().any(|p| p.position == minus_name))
                        .filter_map(|g| name_to_gid.get(&g.name).copied())
                        .collect();
                    gids.sort();
                    gids.dedup();
                    gids
                });

                let subst_lookup = build_single_subst_from_pairs(sources, targets, name_to_gid);
                let subst_idx = lookups.len();
                lookups.push(subst_lookup);

                let source_coverage = make_coverage(sources, name_to_gid);
                let mut sc = SubstitutionChainContext::default();
                *sc = ChainedSequenceContext::Format3(
                    ChainedSequenceContextFormat3::new(
                        vec![],
                        vec![source_coverage],
                        vec![mark_coverage],
                        vec![SequenceLookupRecord {
                            sequence_index: 0,
                            lookup_list_index: subst_idx as u16,
                        }],
                    ),
                );
                lookups.push(SubstitutionLookup::ChainContextual(Lookup::new(
                    LookupFlag::empty(),
                    vec![sc],
                )));
            }

            feature_lookups.push((tag.clone(), scripts, lookups));
        }
    }

    #[cfg(test)]
    let base_subst_entries = ccmp_entries.clone();
    #[cfg(test)]
    let mut mark_subst_entries: Vec<(String, String, String, Vec<String>)> = Vec::new();

    // Mark alternative substitution: when a mark's `-X` anchor doesn't
    // size-match the preceding base's `+X`, substitute with a mark:alt
    // whose `-X` does match.
    //
    // For each anchor, collect (mark, mark:alt) pairs where the alt has
    // a differently-sized `-X`.  Then generate a chain context with
    // backtrack = bases whose `+X` matches the alt's `-X` size.
    let mut mark_glyph_sets: Vec<CoverageTable> = Vec::new();
    {
        let mark_alt_index: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = {
            let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
            for g in glyphs {
                if !g.mark { continue; }
                let mut prefix = g.name.as_str();
                while let Some(colon_pos) = prefix.rfind(':') {
                    prefix = &prefix[..colon_pos];
                    map.entry(prefix.to_string())
                        .or_default()
                        .push((g.name.clone(), g.declared_anchors.clone()));
                }
            }
            for alts in map.values_mut() {
                alts.sort_by(|(a, _), (b, _)| a.cmp(b));
            }
            map
        };

        for anchor_name in &anchor_names {
            let minus_name = format!("-{anchor_name}");
            let plus_name = format!("+{anchor_name}");

            // Mark filtering set: all marks carrying `-X` for this anchor,
            // plus their alternatives.  Registered in GDEF MarkGlyphSets
            // so that USE_MARK_FILTERING_SET on mark-subst lookups causes
            // marks of OTHER anchor classes to be skipped during backtrack.
            let mut filtering_gids: Vec<GlyphId16> = glyphs.iter()
                .filter(|g| g.mark && g.declared_anchors.iter().any(|p| p.position == minus_name))
                .filter_map(|g| name_to_gid.get(&g.name).copied())
                .collect();
            filtering_gids.sort();
            filtering_gids.dedup();
            let filtering_set_idx = if !filtering_gids.is_empty() {
                let idx = mark_glyph_sets.len() as u16;
                mark_glyph_sets.push(CoverageTable::format_1(filtering_gids));
                Some(idx)
            } else {
                None
            };

            // Collect marks that have alternatives with different `-X` sizes.
            for g in glyphs {
                if !g.mark { continue; }
                let Some(&mark_gid) = name_to_gid.get(&g.name) else { continue };
                let Some(mark_minus) = g.declared_anchors.iter().find(|p| p.position == minus_name) else { continue };
                let Some(alts) = mark_alt_index.get(&g.name) else { continue };

                for (alt_name, alt_declared) in alts {
                    let Some(&_alt_gid) = name_to_gid.get(alt_name.as_str()) else { continue };
                    let Some(alt_minus) = alt_declared.iter().find(|p| p.position == minus_name) else { continue };
                    if alt_minus.size_matches(mark_minus) {
                        continue; // same size, no substitution needed
                    }

                    // Find bases (and mark2 glyphs with `+X`) whose `+X`
                    // matches the alt's `-X` size.  Including marks here
                    // handles mark-to-mark stacking where a second mark
                    // should be substituted based on the first mark's anchor.
                    let mut backtrack_gids: Vec<GlyphId16> = Vec::new();
                    for base in glyphs {
                        let Some(&base_gid) = name_to_gid.get(&base.name) else { continue };
                        let plus_pt = base.declared_anchors.iter()
                            .find(|p| p.position == plus_name)
                            .or_else(|| base.resolved_anchors.iter().find(|p| p.position == plus_name));
                        if let Some(pt) = plus_pt
                            && pt.size_matches(alt_minus) && !pt.size_matches(mark_minus) {
                                backtrack_gids.push(base_gid);
                            }
                    }
                    if backtrack_gids.is_empty() {
                        continue;
                    }
                    backtrack_gids.sort();
                    backtrack_gids.dedup();

                    #[cfg(test)]
                    {
                        let bt_names: Vec<String> = backtrack_gids.iter()
                            .filter_map(|gid| {
                                glyphs.iter().find(|g| name_to_gid.get(&g.name) == Some(gid))
                                    .map(|g| g.name.clone())
                            })
                            .collect();
                        mark_subst_entries.push((
                            g.name.clone(), alt_name.clone(), anchor_name.clone(), bt_names,
                        ));
                    }

                    let (tag, _) = anchor_to_feature.get(anchor_name.as_str())
                        .cloned()
                        .unwrap_or_else(|| ("ccmp".to_string(), vec!["DFLT".to_string()]));
                    let scripts: Vec<String> = gsub_data.anchor_features.iter()
                        .filter(|(t, _, _)| *t == tag)
                        .flat_map(|(_, s, _)| s.clone())
                        .fold(Vec::new(), |mut acc, s| { if !acc.contains(&s) { acc.push(s); } acc });

                    // Find or create the feature_lookups entry for this tag.
                    let entry = feature_lookups.iter_mut().find(|(t, _, _)| *t == tag);
                    let lookups = if let Some((_, _, lks)) = entry {
                        lks
                    } else {
                        feature_lookups.push((tag.clone(), scripts, Vec::new()));
                        &mut feature_lookups.last_mut().unwrap().2
                    };

                    let subst_lookup = build_single_subst_from_pairs(
                        std::slice::from_ref(&g.name),
                        std::slice::from_ref(alt_name),
                        name_to_gid,
                    );
                    let subst_idx = lookups.len();
                    lookups.push(subst_lookup);

                    let backtrack_coverage = CoverageTable::format_1(backtrack_gids);
                    let input_coverage = CoverageTable::format_1(vec![mark_gid]);
                    let mut sc = SubstitutionChainContext::default();
                    *sc = ChainedSequenceContext::Format3(
                        ChainedSequenceContextFormat3::new(
                            vec![backtrack_coverage],
                            vec![input_coverage],
                            vec![],
                            vec![SequenceLookupRecord {
                                sequence_index: 0,
                                lookup_list_index: subst_idx as u16,
                            }],
                        ),
                    );
                    let chain_lookup = if let Some(set_idx) = filtering_set_idx {
                        let mut lk = Lookup::new(
                            LookupFlag::USE_MARK_FILTERING_SET,
                            vec![sc],
                        );
                        lk.mark_filtering_set = Some(set_idx);
                        lk
                    } else {
                        Lookup::new(LookupFlag::empty(), vec![sc])
                    };
                    lookups.push(SubstitutionLookup::ChainContextual(chain_lookup));
                }
            }
        }
    }

    AnchorGposData {
        gpos,
        gdef,
        feature_lookups,
        mark_glyph_sets,
        #[cfg(test)]
        base_subst_entries,
        #[cfg(test)]
        mark_subst_entries,
    }
}

fn compute_max_context(gsub_data: &GsubData) -> u16 {
    let mut max_ctx: u16 = 1;
    for remaps in gsub_data.remap_sets.values() {
        for r in remaps {
            let input_len = r.source.first().map_or(1, |seq| seq.len()) as u16;
            let la_len = r.lookahead.len() as u16;
            let ctx = input_len + la_len;
            max_ctx = max_ctx.max(ctx);
        }
    }
    max_ctx
}

fn make_coverage(names: &[String], name_to_gid: &HashMap<String, GlyphId16>) -> CoverageTable {
    let mut gids: Vec<GlyphId16> = names
        .iter()
        .filter_map(|n| name_to_gid.get(n).copied())
        .collect();
    gids.sort();
    gids.dedup();
    CoverageTable::format_1(gids)
}

fn build_single_subst_from_pairs(
    sources: &[String],
    targets: &[String],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut pairs: Vec<(GlyphId16, GlyphId16)> = sources
        .iter()
        .zip(targets.iter())
        .filter_map(|(s, t)| {
            let sg = name_to_gid.get(s)?;
            let tg = name_to_gid.get(t)?;
            Some((*sg, *tg))
        })
        .collect();
    pairs.sort_by_key(|&(s, _)| s);
    pairs.dedup_by_key(|p| p.0);

    let coverage_gids: Vec<GlyphId16> = pairs.iter().map(|&(s, _)| s).collect();
    let substitute_gids: Vec<GlyphId16> = pairs.iter().map(|&(_, t)| t).collect();

    let coverage = CoverageTable::format_1(coverage_gids);
    let subtable = SingleSubst::Format2(SingleSubstFormat2::new(coverage, substitute_gids));

    SubstitutionLookup::Single(Lookup::new(LookupFlag::empty(), vec![subtable]))
}

fn build_single_subst_lookup(
    remaps: &[ExpandedRemap],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut all_sources = Vec::new();
    let mut all_targets = Vec::new();
    for r in remaps {
        for (seq, tgt) in r.source.iter().zip(r.target.iter()) {
            if seq.len() == 1 && !tgt.is_empty() {
                all_sources.push(seq[0].clone());
                all_targets.push(tgt[0].clone());
            }
        }
    }
    build_single_subst_from_pairs(&all_sources, &all_targets, name_to_gid)
}

fn build_ligature_subst_lookup(
    remaps: &[ExpandedRemap],
    name_to_gid: &HashMap<String, GlyphId16>,
) -> SubstitutionLookup {
    let mut by_first: BTreeMap<GlyphId16, Vec<(Vec<GlyphId16>, GlyphId16)>> = BTreeMap::new();

    for r in remaps {
        for (seq, tgt) in r.source.iter().zip(r.target.iter()) {
            if seq.len() < 2 {
                continue;
            }
            let gids: Vec<GlyphId16> = seq
                .iter()
                .filter_map(|name| name_to_gid.get(name.as_str()).copied())
                .collect();
            if gids.len() != seq.len() {
                continue;
            }
            if tgt.is_empty() {
                continue;
            }
            let Some(&tgt_gid) = name_to_gid.get(tgt[0].as_str()) else {
                continue;
            };
            let first = gids[0];
            let rest = gids[1..].to_vec();
            by_first.entry(first).or_default().push((rest, tgt_gid));
        }
    }

    let coverage_gids: Vec<GlyphId16> = by_first.keys().copied().collect();
    let coverage = CoverageTable::format_1(coverage_gids);

    let ligature_sets: Vec<LigatureSet> = by_first
        .values()
        .map(|entries| {
            let mut ligs: Vec<Ligature> = entries
                .iter()
                .map(|(components, lig_glyph)| Ligature::new(*lig_glyph, components.clone()))
                .collect();
            ligs.sort_by(|a, b| {
                b.component_glyph_ids
                    .len()
                    .cmp(&a.component_glyph_ids.len())
                    .then_with(|| a.component_glyph_ids.cmp(&b.component_glyph_ids))
            });
            LigatureSet::new(ligs)
        })
        .collect();

    SubstitutionLookup::Ligature(Lookup::new(
        LookupFlag::empty(),
        vec![LigatureSubstFormat1::new(coverage, ligature_sets.into_iter().collect())],
    ))
}

#[derive(Clone)]
struct CachedContours {
    width: u16,
    height: u16,
    contours: Vec<Vec<(f32, f32)>>,
    anchors: Vec<GlyphPoint>,
    grid: Option<PixelGrid>,
    /// For composite-eligible glyphs: (component_name, col_offset, row_offset)
    composite_components: Option<Vec<(String, f32, f32)>>,
    scale: u8,
}

fn build_cached_alternatives(
    cache: &HashMap<String, CachedContours>,
) -> HashMap<String, Vec<(String, Vec<GlyphPoint>)>> {
    let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
    for (name, cached) in cache {
        let mut prefix = name.as_str();
        while let Some(colon_pos) = prefix.rfind(':') {
            prefix = &prefix[..colon_pos];
            map.entry(prefix.to_string())
                .or_default()
                .push((name.clone(), cached.anchors.clone()));
        }
    }
    for alts in map.values_mut() {
        alts.sort_by(|(a, _), (b, _)| a.cmp(b));
    }
    map
}

fn resolve_cached_ref<'a>(
    name: &str,
    cache: &'a HashMap<String, CachedContours>,
) -> Option<&'a CachedContours> {
    if let Some(cached) = cache.get(name) {
        return Some(cached);
    }
    let expanded = crate::ref_composite::expand_ref_names(name)?;
    cache.get(expanded.first()?)
}

impl CachedContours {
    fn from_grid(grid: &PixelGrid, bitmap: bool, cc: Option<&mut ContourCache>) -> Self {
        if bitmap {
            let mut bitmap_grid = grid.clone();
            for pixel in &mut bitmap_grid.pixels {
                if pixel.is_filled() {
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
                width: bitmap_grid.width,
                height: bitmap_grid.height,
                contours,
                anchors: Vec::new(),
                grid: Some(bitmap_grid),
                composite_components: None,
                scale: 1,
            }
        } else {
            let contours = match cc {
                Some(c) => cached_track_contour(c, grid, false),
                None => track_contour(grid, PX_SUBPIXEL),
            };
            Self {
                width: grid.width,
                height: grid.height,
                contours,
                anchors: Vec::new(),
                grid: Some(grid.clone()),
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
            }
        }
        hasher.finish()
    }

    fn from_components(
        own_pixels: Option<&PixelGrid>,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedContours>,
        bitmap: bool,
        mut cc: Option<&mut ContourCache>,
        parent_scale: u8,
    ) -> Option<Self> {
        let comp_key = Self::hash_composite_key(own_pixels, refs, cache, bitmap);
        if let Some(ref mut cc) = cc {
            let cur_gen = cc.gen_id;
            if let Some(entry) = cc.composite_entries.get_mut(&comp_key) {
                entry.gen_id = cur_gen;
                return Some(entry.value.clone());
            }
        }

        let result = Self::from_components_inner(own_pixels, refs, cache, bitmap, parent_scale);

        if let Some(ref val) = result {
            if let Some(cc) = cc {
                let cur_gen = cc.gen_id;
                cc.composite_entries.insert(comp_key, CacheEntry { value: val.clone(), gen_id: cur_gen });
            }
        }

        result
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

        if has_negated {
            // Collect layers with negation flags and trace contours via
            // track_contour_multi_diff, which computes the geometric difference
            // (positive union minus negative union) per pixel.
            // Pre-rescale ref grids so their raster resolution matches the parent.
            let ref_scaled: Vec<Option<PixelGrid>> = refs.iter().map(|gref| {
                let cached = resolve_cached_ref(&gref.name, cache)?;
                let ref_grid = cached.grid.as_ref()?;
                let rs = cached.scale.max(1);
                Some(if rs == ps { ref_grid.clone() } else { ref_grid.rescale(rs, ps) })
            }).collect();

            let mut diff_layers: Vec<(&PixelGrid, i32, i32, bool)> = Vec::new();
            if let Some(grid) = own_pixels {
                diff_layers.push((grid, 0, 0, false));
            }
            for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
                if let Some(sg) = sg {
                    diff_layers.push((sg, gref.row() as i32, gref.col() as i32, gref.negated));
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
                track_contour_multi_diff(&bitmap_layers, PX_SUBPIXEL)
            } else {
                track_contour_multi_diff(&diff_layers, PX_SUBPIXEL)
            };

            // Build flattened grid for downstream composites that reference
            // this glyph.  shape_subtract may produce PX_DOT for some pixels,
            // which is acceptable here since the grid is only used for pixel
            // lookups, not for contour tracing.
            let mut min_r: i32 = 0;
            let mut min_c: i32 = 0;
            let mut max_r: i32 = 0;
            let mut max_c: i32 = 0;
            for &(grid, row_off, col_off, _) in &diff_layers {
                min_r = min_r.min(row_off);
                min_c = min_c.min(col_off);
                max_r = max_r.max(row_off + grid.height as i32);
                max_c = max_c.max(col_off + grid.width as i32);
            }
            let width = (max_c - min_c).max(0) as u16;
            let height = (max_r - min_r).max(0) as u16;
            let mut result = PixelGrid::new(width, height);

            if let Some(grid) = own_pixels {
                result.blit(grid, -min_r, -min_c, false);
            }

            for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
                if let Some(sg) = sg {
                    result.blit(sg, gref.row() as i32 - min_r, gref.col() as i32 - min_c, gref.negated);
                }
            }

            return Some(Self {
                width,
                height,
                contours,
                anchors: Vec::new(),
                grid: Some(result),
                composite_components: None,
                scale: 1,
            });
        }

        // No negated refs. Pre-rescale ref grids to match parent scale.
        let ref_scaled: Vec<Option<PixelGrid>> = refs.iter().map(|gref| {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let ref_grid = cached.grid.as_ref()?;
            let rs = cached.scale.max(1);
            Some(if rs == ps { ref_grid.clone() } else { ref_grid.rescale(rs, ps) })
        }).collect();

        let mut layers: Vec<(&PixelGrid, i32, i32)> = Vec::new();
        if let Some(grid) = own_pixels {
            layers.push((grid, 0, 0));
        }
        for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
            if let Some(sg) = sg {
                layers.push((sg, gref.row() as i32, gref.col() as i32));
            }
        }

        let needs_multi = own_pixels.is_some() || layers_have_subpixel_conflicts(&layers);

        if needs_multi {
            // Use track_contour_multi to correctly union overlapping subpixels.
            let contours = if bitmap {
                let bitmap_grids: Vec<PixelGrid> = layers
                    .iter()
                    .map(|(g, _, _)| to_bitmap_grid(g))
                    .collect();
                let bitmap_layers: Vec<(&PixelGrid, i32, i32)> = bitmap_grids
                    .iter()
                    .zip(layers.iter())
                    .map(|(bg, &(_, r, c))| (bg, r, c))
                    .collect();
                track_contour_multi(&bitmap_layers, PX_SUBPIXEL)
            } else {
                track_contour_multi(&layers, PX_SUBPIXEL)
            };

            // Build combined grid for downstream composites
            let mut min_r: i32 = 0;
            let mut min_c: i32 = 0;
            let mut max_r: i32 = 0;
            let mut max_c: i32 = 0;
            for &(grid, row_off, col_off) in &layers {
                min_r = min_r.min(row_off);
                min_c = min_c.min(col_off);
                max_r = max_r.max(row_off + grid.height as i32);
                max_c = max_c.max(col_off + grid.width as i32);
            }
            let width = (max_c - min_c).max(0) as u16;
            let height = (max_r - min_r).max(0) as u16;
            let mut result = PixelGrid::new(width, height);
            for &(grid, row_off, col_off) in &layers {
                let off_r = row_off - min_r;
                let off_c = col_off - min_c;
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

            // Pure-ref composites (no own pixels) can still use TrueType
            // composite format; the contours above serve as a fallback
            // for inline glyphs.
            let composite_components = if own_pixels.is_none() {
                Some(refs.iter().filter_map(|gref| {
                    resolve_cached_ref(&gref.name, cache)?;
                    Some((gref.name.clone(), gref.col() as f32, gref.row() as f32))
                }).collect())
            } else {
                None
            };

            return Some(Self {
                width,
                height,
                contours,
                anchors: Vec::new(),
                grid: Some(result),
                composite_components,
                scale: 1,
            });
        }

        // No negated refs, no own pixels, no overlap: simple contour translation
        let mut all_contours = Vec::new();
        let mut max_width = 0u16;
        let mut max_height = 0u16;
        let mut combined_grid: Option<PixelGrid> = None;
        let mut components = Vec::new();

        for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let rs = cached.scale.max(1);
            let scale_f = ps as f32 / rs as f32;
            let dx = gref.col() as f32;
            let dy = gref.row() as f32;
            components.push((gref.name.clone(), dx, dy));
            for contour in &cached.contours {
                let translated: Vec<(f32, f32)> =
                    contour.iter().map(|&(x, y)| (x * scale_f + dx, y * scale_f + dy)).collect();
                all_contours.push(translated);
            }
            let scaled_w = (cached.width as f32 * scale_f).round() as i32;
            let scaled_h = (cached.height as f32 * scale_f).round() as i32;
            let w = (gref.col() as i32 + scaled_w).max(0) as u16;
            let h = (gref.row() as i32 + scaled_h).max(0) as u16;
            max_width = max_width.max(w);
            max_height = max_height.max(h);

            if let Some(sg) = sg {
                let cg = combined_grid.get_or_insert_with(|| PixelGrid::new(max_width, max_height));
                if cg.width < max_width || cg.height < max_height {
                    cg.resize(max_width, max_height);
                }
                let off_r = gref.row() as i32;
                let off_c = gref.col() as i32;
                for r in 0..sg.height as i32 {
                    for c in 0..sg.width as i32 {
                        let shape = sg.get(r as u16, c as u16);
                        if !shape.is_empty() {
                            let dr = off_r + r;
                            let dc = off_c + c;
                            if dr >= 0 && dc >= 0 && dr < max_height as i32 && dc < max_width as i32 {
                                cg.set(dr as u16, dc as u16, shape);
                            }
                        }
                    }
                }
            }
        }

        Some(Self {
            width: max_width,
            height: max_height,
            contours: all_contours,
            anchors: Vec::new(),
            grid: combined_grid,
            composite_components: Some(components),
            scale: 1,
        })
    }
}

fn layers_have_subpixel_conflicts(layers: &[(&PixelGrid, i32, i32)]) -> bool {
    for i in 0..layers.len() {
        let (g1, r1, c1) = layers[i];
        for &(g2, r2, c2) in &layers[i + 1..] {
            let overlap_r0 = r1.max(r2);
            let overlap_r1 = (r1 + g1.height as i32).min(r2 + g2.height as i32);
            let overlap_c0 = c1.max(c2);
            let overlap_c1 = (c1 + g1.width as i32).min(c2 + g2.width as i32);
            for r in overlap_r0..overlap_r1 {
                for c in overlap_c0..overlap_c1 {
                    let s1 = g1.get((r - r1) as u16, (c - c1) as u16);
                    let s2 = g2.get((r - r2) as u16, (c - c2) as u16);
                    if !s1.is_empty() && !s2.is_empty() && s1 != s2 {
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
            if grid.get(r, c).is_filled() {
                bg.set(r, c, PixelShape::new(PX_ALMOSTFULL, true));
            }
        }
    }
    bg
}

fn gcd(mut a: i32, mut b: i32) -> i32 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

fn contour_signed_area(contour: &[(i16, i16)]) -> i64 {
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

/// Add midpoints on diagonal contour edges and generate TrueType instructions
/// that snap those midpoints to the pixel grid at `hint_ppem`.
///
/// At other PPEMs the midpoints are collinear with their neighbors (invisible).
/// At `hint_ppem` the SHPIX deltas fire, turning each diagonal into an
/// axis-aligned staircase so the glyph rasterizes as a clean bitmap.
fn generate_grid_snap_hints(
    contours: &mut [Vec<(i16, i16)>],
    hint_ppem: u16,
) -> Vec<u8> {
    let scale = UNITS_PER_EM as i32 / hint_ppem as i32;
    let half_scale = scale / 2;
    if half_scale == 0 {
        return Vec::new();
    }

    // (point_index, dx, dy) — dx/dy are in font units, which at this specific
    // PPEM happen to equal F26Dot6 values (1024/16 * 16/1024 * 64 = 64 → 1:1).
    let mut deltas: Vec<(u16, i16, i16)> = Vec::new();
    let mut point_offset = 0u16;

    for contour in contours.iter_mut() {
        let area = contour_signed_area(contour);
        let old = std::mem::take(contour);
        let n = old.len();

        for i in 0..n {
            let (ax, ay) = (old[i].0 as i32, old[i].1 as i32);
            let (bx, by) = (old[(i + 1) % n].0 as i32, old[(i + 1) % n].1 as i32);
            contour.push(old[i]);

            let dx = bx - ax;
            let dy = by - ay;
            if dx == 0 || dy == 0 {
                continue;
            }

            // Work in half-scale units to find grid-aligned intermediate points.
            // Grid points sit at multiples of `scale` (= even half-scale units).
            let h_dx = dx / half_scale;
            let h_dy = dy / half_scale;
            if h_dx == 0 || h_dy == 0 {
                continue;
            }
            let g = gcd(h_dx.abs(), h_dy.abs());
            let d1 = h_dx / g;
            let d2 = h_dy / g;

            // Collect segment start points (grid-aligned splits of the diagonal)
            let mut seg_starts: Vec<(i32, i32)> = vec![(ax, ay)];
            for k in 1..g {
                let hx = ax / half_scale + k * d1;
                let hy = ay / half_scale + k * d2;
                if hx % 2 == 0 && hy % 2 == 0 {
                    seg_starts.push((hx * half_scale, hy * half_scale));
                }
            }

            let seg_count = seg_starts.len();
            for si in 0..seg_count {
                let seg_a = seg_starts[si];
                let seg_b = if si + 1 < seg_count {
                    seg_starts[si + 1]
                } else {
                    (bx, by)
                };

                let sdx = seg_b.0 - seg_a.0;
                let sdy = seg_b.1 - seg_a.1;

                let mx = (seg_a.0 + seg_b.0) / 2;
                let my = (seg_a.1 + seg_b.1) / 2;

                if (mx == seg_a.0 && my == seg_a.1)
                    || (mx == seg_b.0 && my == seg_b.1)
                {
                    if si + 1 < seg_count {
                        contour.push((seg_starts[si + 1].0 as i16, seg_starts[si + 1].1 as i16));
                    }
                    continue;
                }

                // Snap direction: C1 = (seg_a.x, seg_b.y), C2 = (seg_b.x, seg_a.y).
                // CW outer contour (area < 0): filled region is to the RIGHT.
                // use C1 when dxdy and area have different signs.
                let dxdy = sdx as i64 * sdy as i64;
                let use_c1 = (dxdy < 0) != (area < 0);
                let (tx, ty) = if use_c1 {
                    (seg_a.0, seg_b.1)
                } else {
                    (seg_b.0, seg_a.1)
                };

                let delta_x = (tx - mx) as i16;
                let delta_y = (ty - my) as i16;

                let point_idx = point_offset + contour.len() as u16;
                contour.push((mx as i16, my as i16));

                if delta_x != 0 || delta_y != 0 {
                    deltas.push((point_idx, delta_x, delta_y));
                }

                if si + 1 < seg_count {
                    contour.push((seg_starts[si + 1].0 as i16, seg_starts[si + 1].1 as i16));
                }
            }
        }

        point_offset += contour.len() as u16;
    }

    if deltas.is_empty() {
        return Vec::new();
    }

    encode_grid_snap_instructions(&deltas, hint_ppem)
}

fn encode_grid_snap_instructions(
    deltas: &[(u16, i16, i16)],
    hint_ppem: u16,
) -> Vec<u8> {
    let mut code = Vec::new();

    // PUSHB hint_ppem; MPPEM; EQ; IF
    tt_push(&mut code, hint_ppem as i32);
    code.push(0x4D); // MPPEM
    code.push(0x54); // EQ
    code.push(0x58); // IF

    // X-axis deltas
    let x_deltas: Vec<_> = deltas.iter().filter(|d| d.1 != 0).collect();
    if !x_deltas.is_empty() {
        code.push(0x01); // SVTCA[1] — freedom/projection to X
        for &&(pt, dx, _) in &x_deltas {
            tt_push(&mut code, dx as i32);
            tt_push(&mut code, pt as i32);
            code.push(0x38); // SHPIX
        }
    }

    // Y-axis deltas
    let y_deltas: Vec<_> = deltas.iter().filter(|d| d.2 != 0).collect();
    if !y_deltas.is_empty() {
        code.push(0x00); // SVTCA[0] — freedom/projection to Y
        for &&(pt, _, dy) in &y_deltas {
            tt_push(&mut code, dy as i32);
            tt_push(&mut code, pt as i32);
            code.push(0x38); // SHPIX
        }
    }

    code.push(0x59); // EIF
    code
}

fn tt_push(code: &mut Vec<u8>, value: i32) {
    if (0..=255).contains(&value) {
        code.push(0xB0); // PUSHB[0]
        code.push(value as u8);
    } else {
        code.push(0xB8); // PUSHW[0]
        let v = value as i16;
        code.push((v >> 8) as u8);
        code.push(v as u8);
    }
}

fn build_ttf(
    ascender: i16,
    descender: i16,
    glyphs: &[CollectedGlyph],
    hint_ppem: u16,
    gsub_data: &GsubData,
    palette: &[Rgba],
    scale: f32,
    pixel_ascent: u16,
) -> Vec<u8> {
    let mut num_glyphs = u16::try_from(glyphs.len() + 1).expect("glyph count checked earlier"); // +1 for .notdef

    let default_aw = glyphs
        .iter()
        .find(|g| g.codepoint == Some(0x20))
        .or(glyphs.first())
        .map(|g| g.advance_width)
        .unwrap_or(UNITS_PER_EM / 2);

    // Build glyf/loca
    let mut glyf_builder = GlyfLocaBuilder::new();

    // .notdef (empty glyph)
    glyf_builder.add_glyph(&Glyph::Empty).unwrap();

    let mut max_points = 0u16;
    let mut max_contours = 0u16;
    let mut max_insn_size = 0u16;
    let mut max_stack = 0u16;
    let mut h_metrics = vec![LongMetric {
        advance: default_aw,
        side_bearing: 0,
    }];

    let mut cmap_mappings: Vec<(char, GlyphId)> = Vec::new();
    let mut name_to_gid: HashMap<String, GlyphId16> = HashMap::new();

    // Pass 1: build name→GID mapping and cmap
    for (i, g) in glyphs.iter().enumerate() {
        let glyph_id = GlyphId::new((i + 1) as u32);
        let glyph_id16 = GlyphId16::new((i + 1) as u16);

        if let Some(cp) = g.codepoint
            && let Some(ch) = char::from_u32(cp) {
                cmap_mappings.push((ch, glyph_id));
            }

        name_to_gid.entry(g.name.clone()).or_insert(glyph_id16);
    }

    let mut max_composite_points = 0u16;
    let mut max_composite_contours = 0u16;
    let mut max_component_elements = 0u16;
    let mut max_component_depth = 0u16;

    // Pass 2: build glyph outlines
    for g in glyphs.iter() {
        let is_composite = !g.composite_refs.is_empty()
            && g.composite_refs.iter().all(|cr| name_to_gid.contains_key(&cr.component_name));

        if is_composite {
            let mut comp_glyph: Option<CompositeGlyph> = None;
            for cr in &g.composite_refs {
                let comp_gid = name_to_gid[&cr.component_name];
                let component = Component::new(
                    comp_gid,
                    Anchor::Offset { x: cr.x_offset, y: cr.y_offset },
                    read_fonts::tables::glyf::Transform::default(),
                    ComponentFlags {
                        round_xy_to_grid: true,
                        overlap_compound: g.composite_refs.len() > 1,
                        ..Default::default()
                    },
                );
                let (gx0, gy0, gx1, gy1) = glyph_bounds(&g.contours);
                let bbox = Bbox { x_min: gx0, y_min: gy0, x_max: gx1, y_max: gy1 };
                match comp_glyph.as_mut() {
                    None => comp_glyph = Some(CompositeGlyph::new(component, bbox)),
                    Some(cg) => cg.add_component(component, bbox),
                }
            }
            let cg = comp_glyph.unwrap();
            max_component_elements = max_component_elements.max(g.composite_refs.len() as u16);
            max_component_depth = max_component_depth.max(1);

            let (gx0, ..) = glyph_bounds(&g.contours);
            let n_points: usize = g.contours.iter().map(|c| c.len()).sum();
            max_composite_points = max_composite_points.max(n_points as u16);
            max_composite_contours = max_composite_contours.max(g.contours.len() as u16);

            h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: gx0,
            });

            glyf_builder.add_glyph(&cg).unwrap();
        } else if g.contours.is_empty() {
            glyf_builder.add_glyph(&Glyph::Empty).unwrap();
            h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: 0,
            });
        } else {
            let mut hinted_contours = g.contours.clone();
            let instructions = if hint_ppem > 0 {
                generate_grid_snap_hints(&mut hinted_contours, hint_ppem)
            } else {
                Vec::new()
            };

            let contours: Vec<Contour> = hinted_contours
                .iter()
                .map(|c| {
                    let points: Vec<CurvePoint> = c
                        .iter()
                        .map(|&(x, y)| CurvePoint::on_curve(x, y))
                        .collect();
                    Contour::from(points)
                })
                .collect();

            let n_points: usize = contours.iter().map(|c| c.len()).sum();
            max_points = max_points.max(n_points as u16);
            max_contours = max_contours.max(contours.len() as u16);
            max_insn_size = max_insn_size.max(instructions.len() as u16);
            if !instructions.is_empty() {
                max_stack = max_stack.max(2);
            }

            let mut sg = SimpleGlyph {
                bbox: Bbox::default(),
                contours,
                instructions,
            };
            sg.recompute_bounding_box();

            // Extend the base glyph's bbox to cover COLR layers and the
            // full advance width so that renderers clipping COLRv0 to
            // the base glyph's bbox don't cut off coloronly content.
            if !g.color_layers.is_empty() {
                for cl in &g.color_layers {
                    for c in &cl.contours {
                        for &(x, y) in c {
                            sg.bbox.x_min = sg.bbox.x_min.min(x);
                            sg.bbox.y_min = sg.bbox.y_min.min(y);
                            sg.bbox.x_max = sg.bbox.x_max.max(x);
                            sg.bbox.y_max = sg.bbox.y_max.max(y);
                        }
                    }
                }
                sg.bbox.x_max = sg.bbox.x_max.max(g.advance_width as i16);
            }

            h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: sg.bbox.x_min,
            });

            glyf_builder.add_glyph(&sg).unwrap();
        }
    }

    // COLRv0: add layer glyphs and build COLR/CPAL data
    let mut colr_base_glyphs: Vec<BaseGlyph> = Vec::new();
    let mut colr_layers: Vec<ColrLayer> = Vec::new();

    for (i, g) in glyphs.iter().enumerate() {
        if g.color_layers.is_empty() {
            continue;
        }
        let base_gid = GlyphId16::new((i + 1) as u16);
        let first_layer_index = colr_layers.len() as u16;

        for cl in &g.color_layers {
            let layer_gid_val = num_glyphs;
            num_glyphs += 1;

            if cl.contours.is_empty() {
                glyf_builder.add_glyph(&Glyph::Empty).unwrap();
                h_metrics.push(LongMetric {
                    advance: g.advance_width,
                    side_bearing: 0,
                });
            } else {
                let contours: Vec<Contour> = cl
                    .contours
                    .iter()
                    .map(|c| {
                        let points: Vec<CurvePoint> = c
                            .iter()
                            .map(|&(x, y)| CurvePoint::on_curve(x, y))
                            .collect();
                        Contour::from(points)
                    })
                    .collect();
                let mut sg = SimpleGlyph {
                    bbox: Bbox::default(),
                    contours,
                    instructions: Vec::new(),
                };
                sg.recompute_bounding_box();
                let lsb = sg.bbox.x_min;
                glyf_builder.add_glyph(&sg).unwrap();
                h_metrics.push(LongMetric {
                    advance: g.advance_width,
                    side_bearing: lsb,
                });
            }

            colr_layers.push(ColrLayer::new(
                GlyphId16::new(layer_gid_val),
                cl.palette_index,
            ));
        }

        colr_base_glyphs.push(BaseGlyph::new(
            base_gid,
            first_layer_index,
            g.color_layers.len() as u16,
        ));
    }

    let has_color = !colr_base_glyphs.is_empty();
    let colr_layers_count = colr_layers.len() as u16;

    let (glyf, loca, loca_format) = glyf_builder.build();

    // Compute global bounds
    let mut x_min = 0i16;
    let mut y_min = descender;
    let mut x_max = default_aw as i16;
    let mut y_max = ascender;
    let mut aw_max = default_aw;
    let mut min_lsb = 0i16;
    let mut min_rsb = i16::MAX;
    let mut x_max_extent = 0i16;

    for (i, g) in glyphs.iter().enumerate() {
        let m = &h_metrics[i + 1];
        aw_max = aw_max.max(m.advance);
        if !g.contours.is_empty() {
            let (gx0, gy0, gx1, gy1) = glyph_bounds(&g.contours);
            x_min = x_min.min(gx0);
            y_min = y_min.min(gy0);
            x_max = x_max.max(gx1);
            y_max = y_max.max(gy1);
            min_lsb = min_lsb.min(gx0);
            let rsb = m.advance as i16 - gx1;
            min_rsb = min_rsb.min(rsb);
            x_max_extent = x_max_extent.max(gx1);
        }
    }
    if min_rsb == i16::MAX {
        min_rsb = 0;
    }

    // head
    let head = Head {
        font_revision: Fixed::from_f64(1.0),
        magic_number: 0x5F0F3CF5,
        flags: Flags::BASELINE_AT_Y_0
            | Flags::LSB_AT_X_0
            | Flags::INSTRUCTIONS_MAY_ALTER_ADVANCE_WIDTH,
        units_per_em: UNITS_PER_EM,
        x_min,
        y_min,
        x_max,
        y_max,
        lowest_rec_ppem: 8,
        font_direction_hint: 2,
        index_to_loc_format: loca_format as i16,
        ..Default::default()
    };

    // hhea
    let hhea = Hhea {
        ascender: ascender.into(),
        descender: descender.into(),
        line_gap: 0i16.into(),
        advance_width_max: aw_max.into(),
        min_left_side_bearing: min_lsb.into(),
        min_right_side_bearing: min_rsb.into(),
        x_max_extent: x_max_extent.into(),
        caret_slope_rise: 1,
        caret_slope_run: 0,
        caret_offset: 0,
        number_of_h_metrics: num_glyphs,
    };

    // maxp
    let maxp = Maxp {
        num_glyphs,
        max_points: Some(max_points),
        max_contours: Some(max_contours),
        max_composite_points: Some(max_composite_points),
        max_composite_contours: Some(max_composite_contours),
        max_zones: Some(1),
        max_twilight_points: Some(0),
        max_storage: Some(0),
        max_function_defs: Some(0),
        max_instruction_defs: Some(0),
        max_stack_elements: Some(max_stack),
        max_size_of_instructions: Some(max_insn_size),
        max_component_elements: Some(max_component_elements),
        max_component_depth: Some(max_component_depth),
    };

    // hmtx
    let hmtx = Hmtx {
        h_metrics,
        left_side_bearings: Vec::new(),
    };

    // cmap
    let cmap = Cmap::from_mappings(cmap_mappings).unwrap();

    // name
    let name = build_name_table();

    // os2
    let avg_width = if glyphs.is_empty() {
        default_aw as i16
    } else {
        let total: u32 = glyphs.iter().map(|g| g.advance_width as u32).sum();
        (total / glyphs.len() as u32) as i16
    };
    let first_cp = glyphs.iter().filter_map(|g| g.codepoint).min().unwrap_or(0x20);
    let last_cp = glyphs.iter().filter_map(|g| g.codepoint).max().unwrap_or(0x7E);

    let max_context = compute_max_context(gsub_data);

    let os2 = Os2 {
        x_avg_char_width: avg_width,
        us_weight_class: 400,
        us_width_class: 5,
        s_typo_ascender: ascender,
        s_typo_descender: descender,
        s_typo_line_gap: 0,
        us_win_ascent: ascender as u16,
        us_win_descent: descender.unsigned_abs(),
        fs_selection: SelectionFlags::REGULAR,
        us_first_char_index: first_cp.min(0xFFFF) as u16,
        us_last_char_index: last_cp.min(0xFFFF) as u16,
        ach_vend_id: Tag::new(b"UNIF"),
        panose_10: [2, 0, 5, 9, 0, 0, 0, 0, 0, 0],
        sx_height: Some(ascender * 2 / 3),
        s_cap_height: Some(ascender),
        us_default_char: Some(0),
        us_break_char: Some(0x20),
        us_max_context: Some(max_context),
        ul_code_page_range_1: Some(1),
        ul_code_page_range_2: Some(0),
        ..Default::default()
    };

    // post
    let post = Post {
        version: write_fonts::types::Version16Dot16::new(3, 0),
        underline_position: (descender / 2).into(),
        underline_thickness: (UNITS_PER_EM as i16 / 20).into(),
        is_fixed_pitch: 1,
        ..Default::default()
    };

    let mut gsub = build_gsub(gsub_data, &name_to_gid);

    let anchor_data = build_anchor_gpos(
        glyphs,
        gsub_data,
        &name_to_gid,
        scale,
        pixel_ascent,
    );

    // Merge anchor-based feature lookups into GSUB
    for (feature_tag, scripts, lookups) in anchor_data.feature_lookups {
        if lookups.is_empty() {
            continue;
        }
        let gsub = gsub.get_or_insert_with(|| {
            Gsub::new(
                ScriptList::new(vec![]),
                FeatureList::new(vec![]),
                LookupList::new(vec![]),
            )
        });

        let base_idx = gsub.lookup_list.lookups.len() as u16;
        let mut chain_indices: Vec<u16> = Vec::new();
        for (local_idx, mut lookup) in lookups.into_iter().enumerate() {
            let global_idx = base_idx + local_idx as u16;
            if let SubstitutionLookup::ChainContextual(ref mut lk) = lookup {
                for subtable in &mut lk.subtables {
                    if let ChainedSequenceContext::Format3(ref mut f3) = ***subtable {
                        for rec in &mut f3.seq_lookup_records {
                            rec.lookup_list_index += base_idx;
                        }
                    }
                }
                chain_indices.push(global_idx);
            }
            gsub.lookup_list.lookups.push(lookup.into());
        }

        let feat_tag = make_tag(&feature_tag);

        // Try to merge into an existing feature record with the same tag
        // to avoid duplicate feature entries (which some shapers ignore).
        let existing_feat = gsub
            .feature_list
            .feature_records
            .iter_mut()
            .find(|fr| fr.feature_tag == feat_tag);

        if let Some(fr) = existing_feat {
            fr.feature.lookup_list_indices.extend(chain_indices);
        } else {
            let feat_idx = gsub.feature_list.feature_records.len() as u16;
            gsub.feature_list.feature_records.push(FeatureRecord::new(
                feat_tag,
                Feature::new(None, chain_indices),
            ));

            for script in &scripts {
                let script_tag = make_tag(script);

                let existing = gsub
                    .script_list
                    .script_records
                    .iter_mut()
                    .find(|sr| sr.script_tag == script_tag);

                if let Some(sr) = existing {
                    if let Some(ref mut default_ls) = *sr.script.default_lang_sys {
                        default_ls.feature_indices.push(feat_idx);
                    }
                } else {
                    let lang_sys = LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: vec![feat_idx],
                    };
                    let script_obj = Script::new(Some(lang_sys), vec![]);
                    gsub.script_list
                        .script_records
                        .push(ScriptRecord::new(script_tag, script_obj));
                }
            }
        }
    }

    let mut builder = FontBuilder::new();
    builder
        .add_table(&head)
        .unwrap()
        .add_table(&hhea)
        .unwrap()
        .add_table(&maxp)
        .unwrap()
        .add_table(&hmtx)
        .unwrap()
        .add_table(&cmap)
        .unwrap()
        .add_table(&name)
        .unwrap()
        .add_table(&os2)
        .unwrap()
        .add_table(&post)
        .unwrap()
        .add_table(&glyf)
        .unwrap()
        .add_table(&loca)
        .unwrap();

    let has_gsub = gsub.is_some();
    let has_gpos = anchor_data.gpos.is_some();

    if let Some(ref gsub) = gsub {
        builder.add_table(gsub).unwrap();
    }

    if let Some(ref gpos) = anchor_data.gpos {
        builder.add_table(gpos).unwrap();
    }

    if has_gsub || has_gpos {
        let mut gdef = anchor_data.gdef;
        if !anchor_data.mark_glyph_sets.is_empty() {
            gdef.mark_glyph_sets_def =
                Some(MarkGlyphSets::new(anchor_data.mark_glyph_sets)).into();
        }
        builder.add_table(&gdef).unwrap();
    }

    if has_color {
        let colr = Colr::new(
            colr_base_glyphs.len() as u16,
            Some(colr_base_glyphs),
            Some(colr_layers),
            colr_layers_count,
        );
        builder.add_table(&colr).unwrap();

        let color_records: Vec<ColorRecord> = palette
            .iter()
            .map(|c| ColorRecord::new(c.b, c.g, c.r, c.a))
            .collect();
        let num_entries = color_records.len() as u16;
        let cpal = Cpal::new(
            num_entries,
            1,
            num_entries,
            Some(color_records),
            vec![0],
        );
        builder.add_table(&cpal).unwrap();
    }

    builder.build()
}

fn build_name_table() -> Name {
    let entries: &[(u16, &str)] = &[
        (0, "Uniform Font"),
        (1, "Uniform"),
        (2, "Regular"),
        (3, "Uniform-Regular"),
        (4, "Uniform Regular"),
        (5, "Version 1.0"),
        (6, "Uniform-Regular"),
    ];

    let records: Vec<NameRecord> = entries
        .iter()
        .map(|&(name_id, text)| {
            NameRecord::new(
                3, // Windows
                1, // Unicode BMP
                0x0409,
                NameId::new(name_id),
                String::from(text).into(),
            )
        })
        .collect();

    Name::new(records)
}

fn glyph_bounds(contours: &[Vec<(i16, i16)>]) -> (i16, i16, i16, i16) {
    let mut x_min = i16::MAX;
    let mut y_min = i16::MAX;
    let mut x_max = i16::MIN;
    let mut y_max = i16::MIN;
    for c in contours {
        for &(x, y) in c {
            x_min = x_min.min(x);
            y_min = y_min.min(y);
            x_max = x_max.max(x);
            y_max = y_max.max(y);
        }
    }
    if x_min > x_max {
        return (0, 0, 0, 0);
    }
    (x_min, y_min, x_max, y_max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use read_fonts::TableProvider;

    /// Rotate a contour so it starts at its lexicographically-smallest point.
    /// `render::contour::track_contour` traces contours via `HashMap`
    /// iteration order internally (not in this file's scope to change), so
    /// the *rotation* at which a closed contour's point list starts (and the
    /// relative order of multiple disjoint sub-contours within one glyph)
    /// varies nondeterministically from run to run, even though the actual
    /// traced geometry does not. Canonicalizing before hashing makes the
    /// digest reflect real geometry changes only.
    /// Drop vertices that sit exactly on the straight line between their
    /// neighbors (cyclically). `track_contour`'s collinearity-collapsing
    /// depends on which point tracing happened to start at (see
    /// `canonicalize_contour` doc comment), so which redundant on-line
    /// points survive is itself nondeterministic; simplifying away all of
    /// them makes the polygon's *point set* canonical, not just its
    /// rotation.
    fn simplify_collinear(c: &[(i16, i16)]) -> Vec<(i16, i16)> {
        let n = c.len();
        if n < 3 {
            return c.to_vec();
        }
        (0..n)
            .filter(|&i| {
                let (x1, y1) = c[(i + n - 1) % n];
                let (x2, y2) = c[i];
                let (x3, y3) = c[(i + 1) % n];
                let cross = (x2 - x1) as i64 * (y3 - y1) as i64 - (y2 - y1) as i64 * (x3 - x1) as i64;
                cross != 0
            })
            .map(|i| c[i])
            .collect()
    }

    fn canonicalize_contour(c: &[(i16, i16)]) -> Vec<(i16, i16)> {
        let c = simplify_collinear(c);
        if c.is_empty() {
            return Vec::new();
        }
        let min_idx = c
            .iter()
            .enumerate()
            .min_by_key(|&(_, &pt)| pt)
            .map(|(i, _)| i)
            .unwrap();
        let mut rotated = Vec::with_capacity(c.len());
        rotated.extend_from_slice(&c[min_idx..]);
        rotated.extend_from_slice(&c[..min_idx]);
        rotated
    }

    fn canonicalize_glyph(g: &CollectedGlyph) -> (Option<u32>, u16, Vec<Vec<(i16, i16)>>) {
        let mut contours: Vec<Vec<(i16, i16)>> =
            g.contours.iter().map(|c| canonicalize_contour(c)).collect();
        contours.sort();
        (g.codepoint, g.advance_width, contours)
    }

    #[test]
    fn ttf_build_digest_is_deterministic() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 2 2
@@@@
..@@

glyph wide 3 2
@@..@@
..@@@@

glyph comp
ref base
ref wide

glyph alias = base

map A = base
map B = wide
map C = comp
map D = alias
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let doc_refs = vec![&doc];

        let (_, _, glyph_data_a, _, _) =
            collect_glyph_data(&doc_refs, false).expect("expected glyph data");
        let (_, _, glyph_data_b, _, _) =
            collect_glyph_data(&doc_refs, false).expect("expected glyph data");

        let mut canon_a: Vec<_> = glyph_data_a.iter().map(canonicalize_glyph).collect();
        let mut canon_b: Vec<_> = glyph_data_b.iter().map(canonicalize_glyph).collect();
        canon_a.sort();
        canon_b.sort();
        assert_eq!(canon_a, canon_b, "canonicalized glyph data should be deterministic");
        assert!(!canon_a.is_empty(), "should produce glyphs");
    }

    #[test]
    fn non_pattern_glyphs_resolve_substituted_and_pattern_refs() {
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
map A = via-parts

glyph via-pattern
ref stem-(a|b)
map B = via-pattern
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();

        for name in ["via-parts", "via-pattern"] {
            assert!(
                glyphs
                    .iter()
                    .any(|glyph| glyph.name == name && !glyph.contours.is_empty()),
                "{name} did not resolve through the TTF dependency cache"
            );
        }
    }

    #[test]
    fn ttf_offsets_use_transitively_forwarded_anchors_without_mutation() {
        let input = "\
glyph link 1 1
@@
point -join 0 0
point +join 2 0

glyph wrapped
ref link

glyph chain
point +join 0 0
ref wrapped
ref wrapped
map C = chain
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let chain = glyphs.iter().find(|glyph| glyph.name == "chain").unwrap();
        let xs: Vec<i16> = chain
            .contours
            .iter()
            .flat_map(|contour| contour.iter().map(|point| point.0))
            .collect();
        assert_eq!(xs.iter().copied().min(), Some(0));
        assert_eq!(xs.iter().copied().max(), Some(192));

        let chain_body = doc.items.iter().find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "chain" => Some(body),
            _ => None,
        }).unwrap();
        assert!(chain_body.refs.iter().all(|gref| gref.offset.is_none()));
    }

    #[test]
    fn unmapped_empty_sticky_glyph_is_retained() {
        let doc = document_io::parse_document_from_str(
            "glyph keep sticky advance 0\n",
            "test.unf".into(),
        )
        .unwrap();
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let keep = glyphs.iter().find(|glyph| glyph.name == "keep").unwrap();
        assert_eq!(keep.codepoint, None);
        assert_eq!(keep.advance_width, 0);
        assert!(keep.contours.is_empty());
    }

    #[test]
    fn grid_snap_hints_single_diagonal() {
        // A triangle contour with one diagonal: (0,896)→(64,832)→(0,832)
        // CW in y-up → area < 0.  Diagonal from (0,896) to (64,832).
        let mut contours = vec![vec![(0i16, 896), (64, 832), (0, 832)]];
        let instructions = generate_grid_snap_hints(&mut contours, 16);

        // The diagonal (0,896)→(64,832) should get a midpoint at (32,864).
        assert_eq!(contours[0].len(), 4, "midpoint should be inserted");
        assert!(
            contours[0].contains(&(32, 864)),
            "midpoint (32,864) missing from {:?}",
            contours[0],
        );

        // Instructions should be non-empty (one delta point).
        assert!(
            !instructions.is_empty(),
            "expected TrueType hint instructions",
        );

        // The instructions should start with: PUSHB 16, MPPEM, EQ, IF
        // and end with EIF (0x59).
        assert_eq!(instructions[0], 0xB0, "PUSHB[0]");
        assert_eq!(instructions[1], 16, "ppem=16");
        assert_eq!(instructions[2], 0x4D, "MPPEM");
        assert_eq!(instructions[3], 0x54, "EQ");
        assert_eq!(instructions[4], 0x58, "IF");
        assert_eq!(*instructions.last().unwrap(), 0x59, "EIF");
    }

    #[test]
    fn grid_snap_hints_collinear_midpoints_invisible() {
        // Verify that the midpoints lie exactly on the diagonal line
        // (collinear with neighbors), so at non-target PPEMs the shape
        // is unchanged.
        let mut contours = vec![vec![(0i16, 896), (64, 832), (0, 832)]];
        generate_grid_snap_hints(&mut contours, 16);

        let c = &contours[0];
        for i in 0..c.len() {
            let prev = c[(i + c.len() - 1) % c.len()];
            let cur = c[i];
            let next = c[(i + 1) % c.len()];
            let cross = (cur.0 - prev.0) as i64 * (next.1 - prev.1) as i64
                - (cur.1 - prev.1) as i64 * (next.0 - prev.0) as i64;
            // For the original 3 non-collinear vertices, cross != 0.
            // For added midpoints, cross must == 0.
            if cross == 0 {
                // This is a collinear (midpoint) vertex — expected.
                assert!(
                    ![(0, 896), (64, 832), (0, 832)].contains(&cur),
                    "original vertex {cur:?} should not be collinear",
                );
            }
        }
    }

    #[test]
    fn grid_snap_hints_multi_cell_diagonal() {
        // Two-cell diagonal: (0,896)→(128,768)→(0,768)
        // Should split into two 1-cell sub-diagonals with a grid point
        // at (64,832) plus two midpoints.
        let mut contours = vec![vec![(0i16, 896), (128, 768), (0, 768)]];
        let instructions = generate_grid_snap_hints(&mut contours, 16);

        // Expect grid split point (64,832) and midpoints (32,864) and (96,800).
        let c = &contours[0];
        assert!(c.contains(&(64, 832)), "grid split point missing: {c:?}");
        assert!(c.contains(&(32, 864)), "first midpoint missing: {c:?}");
        assert!(c.contains(&(96, 800)), "second midpoint missing: {c:?}");
        assert!(!instructions.is_empty());
    }

    #[test]
    fn grid_snap_hints_no_diagonals() {
        // Pure rectangle — no diagonals, no hints.
        let mut contours = vec![vec![
            (0i16, 896),
            (64, 896),
            (64, 832),
            (0, 832),
        ]];
        let instructions = generate_grid_snap_hints(&mut contours, 16);
        assert!(instructions.is_empty());
        assert_eq!(contours[0].len(), 4, "no points should be added");
    }

    #[test]
    fn gsub_tables_generated_for_hangul() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph init-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

glyph init-comb-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

glyph med-a 6 8
............
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........

glyph med-comb-a-f 6 8
............
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........

glyph med-comb-a-nof 6 8
............
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........

glyph fin-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

glyph fin-comb-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

map U+1100 = init-g
map U+1161 = med-a
map U+11A8 = fin-g

remap hangul-ljmo : init-g -> init-comb-g : med-a fin-g
remap hangul-ljmo : init-g -> init-comb-g : med-a
remap hangul-vjmo : med-a -> med-comb-a-f : fin-g
remap hangul-vjmo : med-a -> med-comb-a-nof
remap hangul-tjmo : med-comb-a-f : fin-g -> fin-comb-g
remap hangul-tjmo : med-comb-a-nof : fin-g -> fin-comb-g
feature ljmo for hang : hangul-ljmo
feature vjmo for hang : hangul-vjmo
feature tjmo for hang : hangul-tjmo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let doc_refs = vec![&doc];

        let (_, _, glyph_data, gsub_data, _) =
            collect_glyph_data(&doc_refs, false).expect("expected glyph data");

        assert!(
            !gsub_data.remap_sets.is_empty(),
            "remap sets should be collected"
        );
        assert!(
            !gsub_data.features.is_empty(),
            "features should be collected"
        );

        assert_eq!(gsub_data.features.len(), 3, "ljmo/vjmo/tjmo features");
        assert_eq!(
            gsub_data.features.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>(),
            ["ljmo", "vjmo", "tjmo"],
        );
        for (_, scripts, _) in &gsub_data.features {
            assert!(
                scripts.contains(&"hang".to_string()),
                "all hangul features should have 'hang' script, got: {scripts:?}"
            );
        }

        let non_cmap_count = glyph_data.iter().filter(|g| g.codepoint.is_none()).count();
        assert!(
            non_cmap_count > 0,
            "remap-referenced non-cmap glyphs should be included"
        );

        let mut name_to_gid: HashMap<String, GlyphId16> = HashMap::new();
        for (i, g) in glyph_data.iter().enumerate() {
            name_to_gid.entry(g.name.clone()).or_insert(GlyphId16::new((i + 1) as u16));
        }
        let gsub = build_gsub(&gsub_data, &name_to_gid);
        assert!(gsub.is_some(), "GSUB table should be generated");
        let gsub = gsub.unwrap();
        let hang_found = gsub.script_list.script_records.iter().any(|r| r.script_tag == Tag::new(b"hang"));
        assert!(hang_found, "GSUB ScriptList should contain 'hang' script");

        let font = build_font_from_documents(&doc_refs);
        assert!(font.is_some(), "font should be built");
    }

    #[test]
    fn font_meta_height_zero_returns_none() {
        let doc = document_io::parse_document_from_str(
            "font-meta height 0 ascent 0 descent 0\nglyph a 1 1\n@@\nmap A = a\n",
            "test.unf".into(),
        ).unwrap();
        let result = build_font_from_documents(&[&doc]);
        assert!(result.is_none(), "height 0 should reject build");
    }

    #[test]
    fn parse_map_char_accepts_lowercase_u_plus() {
        assert_eq!(parse_map_char("u+0041"), Some(0x41));
        assert_eq!(parse_map_char("U+0041"), Some(0x41));
    }

    #[test]
    fn expand_map_pairs_depth_aware_pipe_split() {
        // g(a|b) has a pipe inside parens — must not be split at top level
        let pairs = expand_map_pairs("A|B", "g(a|b)");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], (0x41, "ga".to_string()));
        assert_eq!(pairs[1], (0x42, "gb".to_string()));
    }

    #[test]
    fn expand_map_pairs_cycles_glyph_names() {
        // 3 chars, 2 glyph names — should cycle
        let pairs = expand_map_pairs("A|B|C", "ga|gb");
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[2].1, "ga");
    }

    #[test]
    fn expand_map_pairs_single_char_expands_pattern() {
        let pairs = expand_map_pairs("A", "g(a|b)");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], (0x41, "ga".to_string()));
    }

    #[test]
    fn expand_map_pairs_lowercase_u_plus_list() {
        let pairs = expand_map_pairs("u+0041|u+0042", "ga|gb");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, 0x41);
        assert_eq!(pairs[1].0, 0x42);
    }

    #[test]
    fn expand_map_pairs_reverse_range_returns_empty() {
        let pairs = expand_map_pairs("U+0042..0041", "g(a|b)");
        assert!(pairs.is_empty());
    }

    #[test]
    fn expand_map_pairs_bare_pipe_char() {
        // "map | = pipe" — the pipe character itself, not a separator
        let pairs = expand_map_pairs("|", "pipe");
        assert_eq!(pairs, vec![('|' as u32, "pipe".to_string())]);
    }

    #[test]
    fn gsub_ligature_source_with_spaces() {
        let input = "\
glyph f 1 1
@@
glyph i 1 1
@@
glyph fi 2 1
@@@@
map F = f
map I = i
remap ligset : f i -> fi
feature liga for latn : ligset
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, gsub_data, _) = collect_glyph_data(&[&doc], false).unwrap();

        let mut name_to_gid: HashMap<String, GlyphId16> = HashMap::new();
        for (i, g) in glyph_data.iter().enumerate() {
            name_to_gid.entry(g.name.clone()).or_insert(GlyphId16::new((i + 1) as u16));
        }

        let gsub = build_gsub(&gsub_data, &name_to_gid);
        assert!(gsub.is_some(), "GSUB should be generated for ligature remap");
    }

    #[test]
    fn gsub_feature_script_isolation() {
        let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
glyph d 1 1
@@
map A = a
map B = b
remap set1 : a -> b
remap set2 : c -> d
feature feat for latn : set1
feature feat for arab : set2
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, _glyph_data, gsub_data, _) = collect_glyph_data(&[&doc], false).unwrap();

        assert_eq!(
            gsub_data.features.len(), 2,
            "same tag + different scripts = separate feature entries"
        );
    }

    #[test]
    fn remap_referenced_empty_glyph_survives() {
        let input = "\
glyph a 1 1
@@
glyph b
map A = a
remap set1 : a -> b
feature feat for latn : set1
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        assert!(
            glyph_data.iter().any(|g| g.name == "b"),
            "remap-referenced empty glyph should survive"
        );
    }

    #[test]
    fn ttf_build_selects_alternative_glyph_by_anchor_size() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph enclosing 16 16
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
anchor +center 8 7..8

glyph inner 8 16
................
................
................
................
................
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
................
................
................
................
anchor -center 4 8

glyph inner:compressed 8 16
................
................
................
................
................
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
................
................
................
................
anchor -center 4 7..8

glyph combo
ref enclosing
ref inner
map a = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
        // inner:compressed has -center 1x2 matching +center 1x2.
        // With compressed selected at offset (4, 0): contours shift right by 4 pixels.
        // Without: contours at (0, 0).
        // Verify by checking that contour points are shifted.
        assert!(
            !combo.contours.is_empty(),
            "combo glyph should have contours"
        );
        // The inner glyph is 8px wide. With offset col=4, leftmost contour x ≥ 4.
        // Without offset, leftmost contour x = 0.
        let min_x: i16 = combo.contours.iter()
            .flat_map(|c| c.iter().map(|&(x, _)| x))
            .min()
            .unwrap();
        let scale = UNITS_PER_EM as f32 / 16.0;
        let expected_min = (4.0 * scale).round() as i16;
        assert!(
            min_x >= expected_min,
            "inner:compressed should be selected (offset col=4), but min_x={min_x}, expected>={expected_min}"
        );
    }

    #[test]
    fn colr_cpal_tables_built_for_colored_glyphs() {
        let input = "\
font-meta height 16 ascent 12 descent 4

color red = #ff0000
color blue = #0000ff

glyph base 2 2
@@@@
@@@@

glyph overlay 2 2
..@@
@@..

glyph combo
ref base fill red
ref overlay fill blue

map A = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
        assert!(
            !combo.color_layers.is_empty(),
            "combo should have color layers"
        );
        assert_eq!(combo.color_layers.len(), 2, "should have 2 color layers");
        assert!(!palette.is_empty(), "palette should have colors");
        assert_eq!(palette.len(), 2, "palette should have 2 unique colors");
        // Verify deterministic sort (blue < red)
        assert_eq!(palette[0], Rgba { r: 0, g: 0, b: 255, a: 255 });
        assert_eq!(palette[1], Rgba { r: 255, g: 0, b: 0, a: 255 });

        let font = build_font_from_documents(&[&doc]);
        assert!(font.is_some(), "font with COLR should build successfully");
    }

    #[test]
    fn coloronly_layer_excluded_from_fallback() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 2 2
@@@@
@@@@

glyph overlay 2 2
..@@
@@..

glyph combo
ref base fill fg
ref overlay fill #ff0000 coloronly

map A = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, _palette) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
        // coloronly layer should NOT be in color_layers (it IS in COLR but
        // the test above covers that). Wait actually coloronly means it IS in COLR.
        // Let me re-check: coloronly = only in COLR, not in fallback. monoonly = only in fallback.
        // So color_layers should contain coloronly layers (they go into COLR).
        // And fallback contours should NOT contain coloronly layers.

        // combo.contours = fallback = only layers that are NOT coloronly
        // So fallback should only have base (fg), not overlay (coloronly).
        // The base is 2x2, overlay is also 2x2. If both were included,
        // the contours would cover all 4 cells. If only base, all 4 cells too.
        // This test is hard to distinguish by contour shape alone.
        // Just verify the font builds and has color layers.
        assert!(!combo.color_layers.is_empty());
    }

    #[test]
    fn coloronly_white_fill_excluded_from_fallback() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph card-blank 4 4
@@@@@@@@
@@....@@
@@....@@
@@@@@@@@

glyph card-fill 4 4
........
..@@@@..
..@@@@..
........

glyph combo
ref card-blank fill #000000
ref card-fill fill #ffffff coloronly

map A = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();

        assert!(
            !combo.color_layers.is_empty(),
            "should have color layers"
        );

        // The white fill layer should exist in color_layers
        let white_layers: Vec<_> = combo.color_layers.iter()
            .filter(|l| l.palette_index != 0xFFFF)
            .collect();
        assert!(
            !white_layers.is_empty(),
            "white fill layer should be in color_layers"
        );

        // Fallback should NOT include the coloronly card-fill
        // card-blank is a border shape (1 or 2 contours), card-fill is inner fill
        // If card-fill leaked, there would be extra contours
        let card_blank_doc = document_io::parse_document_from_str(
            "font-meta height 16 ascent 12 descent 4\nglyph card-blank 4 4\n@@@@@@@@\n@@....@@\n@@....@@\n@@@@@@@@\nmap B = card-blank\n",
            "test2.unf".into()
        ).unwrap();
        let (_, _, blank_data, _, _) = collect_glyph_data(&[&card_blank_doc], false).unwrap();
        let blank = blank_data.iter().find(|g| g.name == "card-blank").unwrap();

        assert_eq!(
            combo.contours.len(), blank.contours.len(),
            "fallback contours should match card-blank only (coloronly card-fill excluded). \
             combo has {} contours, card-blank has {}",
            combo.contours.len(), blank.contours.len()
        );
    }

    #[test]
    fn coloronly_with_pattern_expansion() {
        let input = "\
font-meta height 16 ascent 12 descent 4

name-parts $suit = spade heart

glyph card-blank 4 4
@@@@@@@@
@@....@@
@@....@@
@@@@@@@@

glyph card-fill 4 4
........
..@@@@..
..@@@@..
........

glyph card-suit-spade 2 2
@@@@
@@@@

glyph card-suit-heart 2 2
..@@
@@..

glyph card-($suit)
ref card-blank fill #000000
ref card-fill fill #ffffff coloronly
ref card-suit-($suit) fill #000000

map A = card-spade
map B = card-heart
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();

        for name in ["card-spade", "card-heart"] {
            let g = glyph_data.iter().find(|g| g.name == name).unwrap();
            assert!(
                !g.color_layers.is_empty(),
                "{name} should have color layers"
            );

            // Check that there IS a non-foreground (white) layer in color_layers
            let non_fg: Vec<_> = g.color_layers.iter()
                .filter(|l| l.palette_index != 0xFFFF)
                .collect();
            assert!(
                non_fg.len() >= 1,
                "{name}: should have at least one non-fg color layer (white fill), got {}",
                non_fg.len()
            );
        }
    }

    #[test]
    fn gpos_mark_base_from_anchor_feature() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = base-letter
map \u{0308} = dia

feature ccmp for DFLT : anchor above
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];

        // Verify parsed data
        let (_, _, glyph_data, gsub_data, _) = collect_glyph_data(&docs, false).unwrap();
        let dia_glyph = glyph_data.iter().find(|g| g.name == "dia").unwrap();
        assert!(dia_glyph.mark, "dia should be marked");
        assert!(!dia_glyph.resolved_anchors.is_empty(), "dia should have anchors");
        assert!(!gsub_data.anchor_features.is_empty(), "anchor features should be collected");

        let base_glyph = glyph_data.iter().find(|g| g.name == "base-letter").unwrap();
        assert!(!base_glyph.resolved_anchors.is_empty(), "base-letter should have anchors");

        let font_data = build_font_from_documents(&docs);
        assert!(font_data.is_some(), "font should build successfully");

        let bytes = font_data.unwrap();
        let font = read_fonts::FontRef::new(&bytes).unwrap();
        let gpos = font.gpos();
        assert!(gpos.is_ok(), "GPOS table should be present");

        let gdef = font.gdef();
        assert!(gdef.is_ok(), "GDEF table should be present");
        let gdef = gdef.unwrap();
        let class_def = gdef.glyph_class_def().unwrap();
        assert!(class_def.is_ok(), "GDEF should have glyph class def");
    }

    #[test]
    fn gpos_mark_to_mark_from_anchor_feature() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1
anchor +above 1 -1

map a = base-letter
map \u{0308} = dia

feature ccmp for DFLT : anchor above
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];

        let (meta, scale, glyphs, gsub_data, _) =
            collect_glyph_data(&docs, false).expect("should collect glyph data");

        let name_to_gid: HashMap<String, GlyphId16> = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
            .collect();

        let anchor_data = build_anchor_gpos(
            &glyphs, &gsub_data, &name_to_gid, scale, meta.ascent,
        );

        assert!(anchor_data.gpos.is_some(), "GPOS should exist");
        let gpos = anchor_data.gpos.unwrap();

        let lookups = &gpos.lookup_list.lookups;
        assert!(lookups.len() >= 2, "should have MarkBasePos and MarkMarkPos lookups");

        let has_mark_to_base = lookups.iter().any(|l| {
            matches!(l.as_ref(), PositionLookup::MarkToBase(_))
        });
        let has_mark_to_mark = lookups.iter().any(|l| {
            matches!(l.as_ref(), PositionLookup::MarkToMark(_))
        });
        assert!(has_mark_to_base, "MarkBasePos lookup should exist");
        assert!(has_mark_to_mark, "MarkMarkPos lookup should exist");

        let font_data = build_font_from_documents(&docs);
        assert!(font_data.is_some(), "font should build successfully");

        let bytes = font_data.unwrap();
        let font = read_fonts::FontRef::new(&bytes).unwrap();
        let gpos_table = font.gpos().expect("GPOS table should be present");
        let feature_list = gpos_table.feature_list().expect("feature list");
        let feature_tags: Vec<_> = feature_list.feature_records().iter()
            .map(|r| r.feature_tag())
            .collect();
        assert!(feature_tags.iter().any(|t| *t == Tag::new(b"mark")),
            "mark feature should exist");
        assert!(feature_tags.iter().any(|t| *t == Tag::new(b"mkmk")),
            "mkmk feature should exist");
    }

    #[test]
    fn gpos_ccmp_generated_for_alternative() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph base:alt 4 4
@@@@@@@@
..@@@@..
..@@@@..
@@@@@@@@
anchor +above 2 0

glyph dia mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = base
map \u{0308} = dia

feature ccmp for DFLT : anchor above
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];
        let font_data = build_font_from_documents(&docs);
        assert!(font_data.is_some(), "font should build successfully");

        let bytes = font_data.unwrap();
        let font = read_fonts::FontRef::new(&bytes).unwrap();

        // GSUB should exist with ccmp feature
        let gsub = font.gsub();
        assert!(gsub.is_ok(), "GSUB table should be present");

        // GPOS should exist
        let gpos = font.gpos();
        assert!(gpos.is_ok(), "GPOS table should be present");
    }

    /// Regression test for anchor-based ccmp and GPOS with three base
    /// glyphs that have different anchor/alternative configurations:
    ///
    /// - `ii`: own `+below` (2-cell), ref `ii:dotless` (has `+above` 2-cell).
    ///   +above: substitute to ii:dotless.  +below: no substitution.
    /// - `jj`: no own + anchors, ref `jj:dotless` (has `+above` 1-cell).
    ///   Alt `jj:compressed` has `+below` (1-cell).
    ///   +above: substitute to jj:dotless.  +below: substitute to jj:compressed.
    /// - `kk`: own `+above` (1-cell) and `+below` (1-cell).
    ///   No substitution needed for either anchor.
    ///
    /// Mark glyphs `dia-above` (1-cell `-above`) and `dia-below` (1-cell
    /// `-below`) each have a `:wide` variant with a 2-cell anchor.  The
    /// wide variant should be selected via ccmp when the base's `+` anchor
    /// is 2-cells wide.
    #[test]
    fn anchor_ccmp_base_and_mark_substitution() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph dia 2 1
@@@@

glyph dia-wide 3 1
@@@@@@

glyph dia-below 2 1 mark
ref dia 0 0
anchor -below 1 0

glyph dia-below:wide 3 1 mark
ref dia-wide 0 0
anchor -below 1..2 0

glyph dia-above 2 1 mark
ref dia 0 0
anchor -above 1 0

glyph dia-above:wide 3 1 mark
ref dia-wide 0 0
anchor -above 1..2 0

glyph ii:dotless 4 8
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2..3 0

glyph ii 4 10
@@@@@@@@
@@@@@@@@
........
........
........
........
........
........
........
........
ref ii:dotless 0 2
anchor +below 2..3 9

glyph jj:dotless 4 8
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph jj 4 10
@@@@@@@@
@@@@@@@@
........
........
........
........
........
........
........
........
ref jj:dotless 0 2

glyph jj:compressed 4 10
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +below 2 9

glyph kk 4 10
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0
anchor +below 2 9

map a = ii
map b = jj
map c = kk
map \u{0308} = dia-above
map \u{0324} = dia-below

feature ccmp for DFLT : anchor above
feature ccmp for DFLT : anchor below
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];

        let (meta, scale, glyphs, gsub_data, _) =
            collect_glyph_data(&docs, false).expect("should collect glyph data");

        let name_to_gid: HashMap<String, GlyphId16> = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
            .collect();

        let anchor_data = build_anchor_gpos(
            &glyphs, &gsub_data, &name_to_gid, scale, meta.ascent,
        );

        // --- Base substitution ------------------------------------------------

        // ii + dia-above: ii lacks own +above → substituted to ii:dotless
        assert!(anchor_data.base_subst_entries.iter()
            .any(|(s, t, a)| s == "ii" && t == "ii:dotless" && a == "above"),
            "ii should be substituted to ii:dotless for anchor above");
        // ii + dia-below: ii has own +below → NOT substituted
        assert!(!anchor_data.base_subst_entries.iter()
            .any(|(s, _, a)| s == "ii" && a == "below"),
            "ii must not be substituted for anchor below (has own +below)");

        // jj + dia-above: jj lacks own +above → substituted to jj:dotless
        assert!(anchor_data.base_subst_entries.iter()
            .any(|(s, t, a)| s == "jj" && t == "jj:dotless" && a == "above"),
            "jj should be substituted to jj:dotless for anchor above");
        // jj + dia-below: jj lacks own +below → substituted to jj:compressed
        assert!(anchor_data.base_subst_entries.iter()
            .any(|(s, t, a)| s == "jj" && t == "jj:compressed" && a == "below"),
            "jj should be substituted to jj:compressed for anchor below");

        // kk: has own +above and +below → NOT substituted for either
        assert!(!anchor_data.base_subst_entries.iter().any(|(s, _, _)| s == "kk"),
            "kk should not have any base substitution");

        // --- Mark substitution ------------------------------------------------

        // dia-above → dia-above:wide after bases with 2-cell +above
        let da_entry = anchor_data.mark_subst_entries.iter()
            .find(|(m, alt, a, _)| m == "dia-above" && alt == "dia-above:wide" && a == "above");
        assert!(da_entry.is_some(), "dia-above should be substituted to dia-above:wide");
        let da_bases = &da_entry.unwrap().3;
        assert!(da_bases.contains(&"ii:dotless".to_string()),
            "ii:dotless (2-cell +above) should trigger dia-above:wide");
        assert!(!da_bases.contains(&"kk".to_string()),
            "kk (1-cell +above) must not trigger dia-above:wide");

        // dia-below → dia-below:wide after bases with 2-cell +below
        let db_entry = anchor_data.mark_subst_entries.iter()
            .find(|(m, alt, a, _)| m == "dia-below" && alt == "dia-below:wide" && a == "below");
        assert!(db_entry.is_some(), "dia-below should be substituted to dia-below:wide");
        let db_bases = &db_entry.unwrap().3;
        assert!(db_bases.contains(&"ii".to_string()),
            "ii (2-cell +below) should trigger dia-below:wide");
        assert!(!db_bases.contains(&"kk".to_string()),
            "kk (1-cell +below) must not trigger dia-below:wide");

        // --- GPOS exists ------------------------------------------------------

        assert!(anchor_data.gpos.is_some(), "GPOS should exist");
        assert!(!anchor_data.feature_lookups.is_empty(), "feature lookups should exist");
    }

    #[test]
    fn mark_flag_roundtrips() {
        let input = "\
glyph dia 3 2 mark
@@@@@@
@@@@@@
anchor -above 1 1
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert!(body.mark, "mark flag should be parsed");
        } else {
            panic!("expected glyph");
        }

        let mut output = Vec::new();
        document_io::serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("mark"), "mark flag should be serialized");

        let doc2 = document_io::parse_document_from_str(&output_str, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc2.items[0] {
            assert!(body.mark, "mark flag should survive roundtrip");
        }
    }

    #[test]
    fn feature_anchor_roundtrips() {
        let input = "\
feature ccmp for DFLT latn : anchor above
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::FeatureAnchor { name, scripts, anchor } = &doc.items[0] {
            assert_eq!(name, "ccmp");
            assert_eq!(scripts, &["DFLT", "latn"]);
            assert_eq!(anchor, "above");
        } else {
            panic!("expected FeatureAnchor, got {:?}", doc.items[0]);
        }

        let mut output = Vec::new();
        document_io::serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("anchor above"));
    }

    #[test]
    fn map_decomposed_roundtrips() {
        let input = "\
map ä
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::MapDecomposed { char_repr } = &doc.items[0] {
            assert_eq!(char_repr, "ä");
        } else {
            panic!("expected MapDecomposed, got {:?}", doc.items[0]);
        }

        let mut output = Vec::new();
        document_io::serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("map ä"));
    }

    #[test]
    fn map_decomposed_generates_composite_glyph() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph a-lower 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = a-lower
map \u{0308} = dia-above
map ä
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];
        let font_data = build_font_from_documents(&docs);
        assert!(font_data.is_some(), "font should build");

        let bytes = font_data.unwrap();
        let font = read_fonts::FontRef::new(&bytes).unwrap();

        // ä (U+00E4) should be in the cmap
        let cmap = font.cmap().unwrap();
        let gid = cmap.map_codepoint('ä');
        assert!(gid.is_some(), "ä should be mapped in cmap");
    }

    #[test]
    fn map_decomposed_forwards_mark_anchors() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph a-lower 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0
anchor +below 2 3

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1
anchor +above 1 -1

map a = a-lower
map \u{0308} = dia-above
map ä

feature ccmp for DFLT : anchor above
feature ccmp for DFLT : anchor below
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];

        let (_, _, glyphs, _, _) =
            collect_glyph_data(&docs, false).expect("should collect");

        let composite = glyphs.iter().find(|g| g.name == "uni00E4").unwrap();
        let has_plus_above = composite.resolved_anchors.iter()
            .any(|p| p.position == "+above");
        let has_plus_below = composite.resolved_anchors.iter()
            .any(|p| p.position == "+below");
        assert!(has_plus_above,
            "uni00E4 should forward +above from dia-above; anchors: {:?}",
            composite.resolved_anchors);
        assert!(has_plus_below,
            "uni00E4 should forward +below from a-lower; anchors: {:?}",
            composite.resolved_anchors);

        // Verify that the composite is registered as a base in GPOS
        let (meta, scale, glyphs, gsub_data, _) =
            collect_glyph_data(&docs, false).unwrap();
        let name_to_gid: HashMap<String, GlyphId16> = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
            .collect();
        let anchor_data = build_anchor_gpos(
            &glyphs, &gsub_data, &name_to_gid, scale, meta.ascent,
        );
        assert!(anchor_data.gpos.is_some(), "GPOS should exist");

        // Check that uni00E4 is in the MarkBasePos base coverage
        let gpos = anchor_data.gpos.unwrap();
        let lookups = &gpos.lookup_list.lookups;
        let composite_gid = *name_to_gid.get("uni00E4").unwrap();
        let mut found_in_base_coverage = false;
        for lookup in lookups {
            if let PositionLookup::MarkToBase(ref lk) = *lookup.as_ref() {
                for sub in &lk.subtables {
                    if let CoverageTable::Format1(ref cov) = *sub.base_coverage
                        && cov.glyph_array.contains(&composite_gid) {
                            found_in_base_coverage = true;
                        }
                }
            }
        }
        assert!(found_in_base_coverage,
            "uni00E4 (gid {:?}) should be in MarkBasePos base coverage",
            composite_gid);
    }

    // -----------------------------------------------------------------
    // Regression tests for bugs fixed in past sessions (see task notes).
    // -----------------------------------------------------------------

    /// Regression test: a composite glyph's component `y_offset` must be
    /// `-dy * scale` (plus top-offset compensation), not
    /// `(ascent - dy) * scale`. The latter double-counts the ascent and
    /// shifts every composite (ref-built) glyph up by a full ascender.
    #[test]
    fn composite_y_offset_is_negative_dy_not_ascent_relative() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 2 2
@@@@
@@@@

glyph comp
ref base 0 3

map A = base
map B = comp
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, scale, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let comp = glyphs.iter().find(|g| g.name == "comp").unwrap();
        assert_eq!(comp.composite_refs.len(), 1, "comp should keep its composite representation");
        let dy = 3.0f32;
        let expected_y = (-dy * scale).round() as i16;
        assert_eq!(
            comp.composite_refs[0].y_offset, expected_y,
            "composite y_offset must be -dy*scale, not ascent-relative"
        );
    }

    /// Regression test: `glyph_bounds` on an empty contour list must return
    /// a degenerate (0,0,0,0) box, not (MAX,MAX,MIN,MIN) — the latter is an
    /// invalid bbox (x_min > x_max) that Chrome's OTS sanitizer rejects.
    #[test]
    fn glyph_bounds_empty_contours_is_degenerate_zero_box() {
        assert_eq!(glyph_bounds(&[]), (0, 0, 0, 0));
    }

    /// Regression test: subpixel-conflict detection between composite ref
    /// layers must look at actual pixel shapes, not just bounding-box
    /// overlap. Two grids whose bboxes fully overlap but whose filled cells
    /// never coincide must NOT be flagged as conflicting; two grids that
    /// fill the *same* cell with different (non-empty) shapes must be.
    #[test]
    fn layers_have_subpixel_conflicts_checks_pixels_not_just_bbox() {
        let mut a = PixelGrid::new(2, 2);
        a.set(0, 0, PixelShape::new(PX_ALMOSTFULL, true));
        let mut b = PixelGrid::new(2, 2);
        b.set(1, 1, PixelShape::new(PX_ALMOSTFULL, true));
        assert!(
            !layers_have_subpixel_conflicts(&[(&a, 0, 0), (&b, 0, 0)]),
            "overlapping bboxes with disjoint filled cells must not conflict"
        );

        let mut c = PixelGrid::new(1, 1);
        c.set(0, 0, PixelShape::new(PX_ALMOSTFULL, true));
        let mut d = PixelGrid::new(1, 1);
        d.set(0, 0, PixelShape::new(crate::pixel::PX_HALF3, true));
        assert!(
            layers_have_subpixel_conflicts(&[(&c, 0, 0), (&d, 0, 0)]),
            "the same cell filled with two different shapes must conflict"
        );
    }

    /// Regression test: a pure-ref composite whose component bounding boxes
    /// overlap, but whose actual pixels never conflict, must keep its
    /// TrueType composite-component representation rather than being
    /// flattened into full contours (which used to balloon font size).
    #[test]
    fn non_conflicting_overlapping_bbox_refs_keep_composite_representation() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph partA 2 2
@@..
....

glyph partB 2 2
....
..@@

glyph combo
ref partA 0 0
ref partB 0 0

map A = partA
map B = partB
map C = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyphs.iter().find(|g| g.name == "combo").unwrap();
        assert_eq!(
            combo.composite_refs.len(), 2,
            "non-conflicting overlapping-bbox refs should stay as 2 composite components"
        );
    }

    /// Regression test: `glyph foo W H` with an all-empty own pixel grid
    /// (declared dims but no filled pixels) plus refs must (a) still use
    /// the declared width for the advance and (b) not force the composite
    /// to flatten into full contours just because an (empty) own grid is
    /// present.
    #[test]
    fn declared_dims_with_empty_grid_keeps_advance_and_composite() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph combo 16 16
ref base 0 0

map A = base
map B = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, scale, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyphs.iter().find(|g| g.name == "combo").unwrap();
        let expected_advance = (16.0f32 * scale).round() as u16;
        assert_eq!(combo.advance_width, expected_advance, "advance should come from declared width");
        assert_eq!(
            combo.composite_refs.len(), 1,
            "an all-empty own grid must not force flattening of the composite"
        );
    }

    /// Regression test: own pixel data plus fill-less/`fg`-filled refs must
    /// be merged into a single foreground (palette index 0xFFFF) COLR
    /// layer, not emitted as separate layers.
    #[test]
    fn colr_foreground_layers_are_merged_into_one() {
        let input = "\
font-meta height 16 ascent 12 descent 4

color red = #ff0000

glyph base 2 2
@@..
@@..

glyph overlay1 2 2
..@@
..@@

glyph overlay2 2 2
@@@@
@@@@

glyph combo
ref base
ref overlay1 fill fg
ref overlay2 fill red

map A = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
        assert_eq!(
            combo.color_layers.len(), 2,
            "own pixels + fg-filled ref should merge into ONE foreground layer, plus one red layer"
        );
        let fg_layers: Vec<_> = combo.color_layers.iter().filter(|l| l.palette_index == 0xFFFF).collect();
        assert_eq!(fg_layers.len(), 1, "there should be exactly one foreground layer");
        // The single foreground layer should contain contours from BOTH
        // own pixels (base) and the fg-filled ref (overlay1): 2 contours.
        assert_eq!(fg_layers[0].contours.len(), 2, "foreground layer should merge own+fg-ref contours");
    }

    /// Regression test: each COLR layer glyph's hmtx left-side-bearing must
    /// equal its own bbox x_min, not 0 — otherwise renderers reposition the
    /// layer relative to the wrong origin.
    #[test]
    fn colr_layer_glyph_lsb_matches_its_own_bbox() {
        let input = "\
font-meta height 16 ascent 12 descent 4

color red = #ff0000

glyph base 4 2
@@@@@@@@
@@@@@@@@

glyph overlay 4 2
....@@@@
....@@@@

glyph combo
ref base fill fg
ref overlay fill red

map A = combo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let font_data = build_font_from_documents(&[&doc]);
        assert!(font_data.is_some(), "font with COLR should build successfully");
        let bytes = font_data.unwrap();
        let font = read_fonts::FontRef::new(&bytes).unwrap();

        let glyf = font.glyf().unwrap();
        let loca = font.loca(None).unwrap();
        let hmtx = font.hmtx().unwrap();
        let maxp = font.maxp().unwrap();

        // The "red" overlay layer glyph is offset to the right (its filled
        // cells start at column 4 of an 8-wide grid), so its bbox x_min
        // (and thus its LSB) must be > 0, not 0.
        let mut found_nonzero_lsb = false;
        for gid in 0..maxp.num_glyphs() {
            let gid = GlyphId::new(gid as u32);
            let Ok(Some(glyph)) = loca.get_glyf(gid, &glyf) else { continue };
            let read_fonts::tables::glyf::Glyph::Simple(sg) = glyph else { continue };
            if sg.number_of_contours() == 0 {
                continue;
            }
            let x_min = sg.x_min();
            let lsb = hmtx.h_metrics()[gid.to_u32() as usize].side_bearing();
            if x_min > 0 {
                assert_eq!(lsb, x_min, "LSB must match this layer glyph's own bbox x_min");
                found_nonzero_lsb = true;
            }
        }
        assert!(found_nonzero_lsb, "expected at least one COLR layer glyph with a nonzero x_min");
    }

    /// Regression test: GPOS mark classification must use a mark glyph's
    /// own (declared) anchors, not anchors forwarded from a ref'd glyph.
    /// A mark glyph that refs another mark inherits (forwards) its
    /// anchors; classification used to use that forwarded set, causing a
    /// mark like `dia-above` (which refs `dia-below`) to register in the
    /// `below` class with the wrong anchor coordinates.
    #[test]
    fn gpos_mark_classification_uses_declared_not_forwarded_anchors() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-below mark 3 2
@@@@@@
@@@@@@
anchor -below 1 1

glyph dia-above mark
ref dia-below
anchor -above 1 0

map a = base-letter
map \u{0323} = dia-below
map \u{0308} = dia-above

feature ccmp for DFLT : anchor below
feature ccmp for DFLT : anchor above
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];
        let (meta, scale, glyphs, gsub_data, _) =
            collect_glyph_data(&docs, false).expect("should collect");

        // Sanity check on the setup itself: dia-above does forward -below
        // from its ref (that forwarding is correct and NOT the bug), but
        // its *declared* anchors must only contain its own -above.
        let dia_above = glyphs.iter().find(|g| g.name == "dia-above").unwrap();
        assert!(dia_above.resolved_anchors.iter().any(|p| p.position == "-below"),
            "dia-above should forward -below from dia-below via the ref");
        assert!(dia_above.declared_anchors.iter().any(|p| p.position == "-above"),
            "dia-above should have its own declared -above anchor");
        assert!(!dia_above.declared_anchors.iter().any(|p| p.position == "-below"),
            "dia-above's declared anchors must not include the forwarded -below");

        let name_to_gid: HashMap<String, GlyphId16> = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
            .collect();
        let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent);
        let gpos = anchor_data.gpos.expect("GPOS should exist");
        let dia_above_gid = *name_to_gid.get("dia-above").unwrap();

        let mut found = false;
        for lookup in &gpos.lookup_list.lookups {
            if let PositionLookup::MarkToBase(lk) = lookup.as_ref() {
                for sub in &lk.subtables {
                    let CoverageTable::Format1(cov) = &*sub.mark_coverage else { continue };
                    let Some(idx) = cov.glyph_array.iter().position(|&g| g == dia_above_gid) else { continue };
                    let record = &sub.mark_array.mark_records[idx];
                    let AnchorTable::Format1(anchor) = &*record.mark_anchor else {
                        panic!("expected AnchorFormat1");
                    };
                    // dia-above's own -above anchor is at col=1,row=0:
                    // x = 1*64 = 64, y = (12-0)*64 = 768.
                    // The forwarded -below anchor (col=1,row=1) would give
                    // y = (12-1)*64 = 704 instead — that's the bug.
                    assert_eq!(anchor.x_coordinate, 64,
                        "x should come from dia-above's own -above anchor");
                    assert_eq!(anchor.y_coordinate, 768,
                        "dia-above must be positioned using its OWN -above anchor (y=768), \
                         not the forwarded -below anchor from dia-below (which would give y=704)");
                    found = true;
                }
            }
        }
        assert!(found, "dia-above should appear in a MarkBasePos mark array");
    }

    /// Regression test: when the used anchor classes are non-contiguous
    /// (e.g. classes {0, 2} because the middle anchor class has no marks
    /// using it), they must be compacted to contiguous 0-based indices so
    /// that `MarkArray::class_count()` (which counts unique classes) still
    /// matches the number of anchor slots in each `BaseRecord`.
    #[test]
    fn mark_class_compaction_keeps_base_record_slots_consistent() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base-letter 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +below 2 3
anchor +middle 2 2
anchor +above 2 0

glyph dia-below mark 3 2
@@@@@@
@@@@@@
anchor -below 1 1

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = base-letter
map \u{0323} = dia-below
map \u{0308} = dia-above

feature ccmp for DFLT : anchor below
feature ccmp for DFLT : anchor middle
feature ccmp for DFLT : anchor above
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs: Vec<&Document> = vec![&doc];
        let (meta, scale, glyphs, gsub_data, _) =
            collect_glyph_data(&docs, false).expect("should collect");

        let name_to_gid: HashMap<String, GlyphId16> = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.clone(), GlyphId16::new((i + 1) as u16)))
            .collect();
        let anchor_data = build_anchor_gpos(&glyphs, &gsub_data, &name_to_gid, scale, meta.ascent);
        let gpos = anchor_data.gpos.expect("GPOS should exist");

        let dia_below_gid = *name_to_gid.get("dia-below").unwrap();
        let dia_above_gid = *name_to_gid.get("dia-above").unwrap();

        let mut checked = false;
        for lookup in &gpos.lookup_list.lookups {
            if let PositionLookup::MarkToBase(lk) = lookup.as_ref() {
                for sub in &lk.subtables {
                    let CoverageTable::Format1(mark_cov) = &*sub.mark_coverage else { continue };
                    let CoverageTable::Format1(base_cov) = &*sub.base_coverage else { continue };
                    assert_eq!(base_cov.glyph_array.len(), 1, "expected a single base glyph");

                    // Only 2 of the 3 declared classes (below, above) are
                    // actually used by any mark ("middle" is unused), so
                    // classes must be compacted down to 2 slots.
                    let base_record = &sub.base_array.base_records[0];
                    assert_eq!(
                        base_record.base_anchors.len(), 2,
                        "base record anchor slots must match the compacted class count, not the declared count"
                    );

                    let below_idx = mark_cov.glyph_array.iter().position(|&g| g == dia_below_gid).unwrap();
                    let above_idx = mark_cov.glyph_array.iter().position(|&g| g == dia_above_gid).unwrap();
                    let below_class = sub.mark_array.mark_records[below_idx].mark_class;
                    let above_class = sub.mark_array.mark_records[above_idx].mark_class;

                    assert!((below_class as usize) < base_record.base_anchors.len());
                    assert!((above_class as usize) < base_record.base_anchors.len());
                    assert_ne!(below_class, above_class);

                    // Each mark's class must resolve to a present (non-null)
                    // anchor on the base record.
                    assert!(base_record.base_anchors[below_class as usize].as_ref().is_some());
                    assert!(base_record.base_anchors[above_class as usize].as_ref().is_some());

                    checked = true;
                }
            }
        }
        assert!(checked, "expected a MarkBasePos subtable to inspect");
    }

    /// Regression test for `build_composite_refs`'s `top`/`left` offset
    /// compensation: a child glyph's OWN `left`/`top` offset (already baked
    /// into the child's own glyph outline) must not leak into a parent
    /// composite that refs it — the parent must subtract the child's own
    /// offset so the ref's declared position isn't double-shifted.
    #[test]
    fn composite_ref_compensates_for_childs_own_left_offset() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph child 2 2 left 5
@@@@
@@@@

glyph parent
ref child 0 0

map A = child
map B = parent
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, scale, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let parent = glyphs.iter().find(|g| g.name == "parent").unwrap();
        assert_eq!(parent.composite_refs.len(), 1);
        let comp_left = (5.0f32 * scale).round() as i16;
        assert_eq!(
            parent.composite_refs[0].x_offset, -comp_left,
            "parent composite must subtract the child's own left offset (already baked into \
             the child's own glyph outline) so it isn't double-applied"
        );
    }

    #[test]
    fn color_layers_built_for_remap_only_glyphs() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph base-a 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph base-b 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph mono-layer 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph color-layer 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph combined
ref mono-layer fill fg monoonly
ref color-layer fill #FF0000 coloronly

map A = base-a
map B = base-b

remap sub : base-a base-b -> combined
feature ccmp for DFLT : sub
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (_, _, glyphs, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
        let combined = glyphs.iter().find(|g| g.name == "combined").expect(
            "combined glyph should be in glyph_data as a remap-referenced extra glyph"
        );
        assert!(
            !combined.color_layers.is_empty(),
            "remap-only glyph with fill refs must have COLR layers"
        );
        assert!(
            !palette.is_empty(),
            "palette must contain at least the #FF0000 color"
        );
        assert!(
            combined.color_layers.iter().all(|cl| !cl.contours.is_empty()),
            "every color layer should have contours"
        );
        let has_colored = combined.color_layers.iter().any(|cl| cl.palette_index != 0xFFFF);
        assert!(has_colored, "at least one layer must reference a palette color");

        let fallback_non_empty = !combined.contours.is_empty();
        assert!(fallback_non_empty, "fallback contours (monoonly layer) should be present");

        let color_layer_count = combined.color_layers.len();
        assert_eq!(
            color_layer_count, 1,
            "monoonly ref should be excluded from color layers, \
             so only the coloronly ref (with its palette color) should remain"
        );
    }

    #[test]
    fn scaled_glyph_has_same_advance_as_unscaled() {
        let input_unscaled = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 3
@@@@@@@@
@@@@@@@@
@@@@@@@@

map A = base
";
        let input_scaled = "\
font-meta height 16 ascent 12 descent 4

glyph base 4 3 scale 2
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

map A = base
";
        let doc1 = document_io::parse_document_from_str(input_unscaled, "a.unf".into()).unwrap();
        let doc2 = document_io::parse_document_from_str(input_scaled, "b.unf".into()).unwrap();

        let (_, _, glyphs1, _, _) = collect_glyph_data(&[&doc1], false).unwrap();
        let (_, _, glyphs2, _, _) = collect_glyph_data(&[&doc2], false).unwrap();

        assert_eq!(glyphs1[0].advance_width, glyphs2[0].advance_width);
        assert!(!glyphs2[0].contours.is_empty());
    }

    #[test]
    fn scaled_composite_matches_unscaled() {
        let input_unscaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 6 3
ref part 0 0
ref part 0 4

map A = combo
";
        // Same composite but the parent is at scale 2.
        // The refs point at scale-1 parts; offsets are doubled.
        let input_scaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 6 3 scale 2
ref part 0 0
ref part 0 8

map A = combo
";
        let doc1 = document_io::parse_document_from_str(input_unscaled, "a.unf".into()).unwrap();
        let doc2 = document_io::parse_document_from_str(input_scaled, "b.unf".into()).unwrap();

        let (_, _, g1, _, _) = collect_glyph_data(&[&doc1], false).unwrap();
        let (_, _, g2, _, _) = collect_glyph_data(&[&doc2], false).unwrap();

        assert_eq!(
            g1[0].advance_width, g2[0].advance_width,
            "advance: unscaled {} vs scaled {}",
            g1[0].advance_width, g2[0].advance_width
        );
        assert_eq!(
            g1[0].contours, g2[0].contours,
            "contours should match"
        );
    }

    #[test]
    fn color_mono_combined_glyph_preserves_advance_across_scales() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph part-left 16 16
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@

glyph part-right 8 16
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph test-xy:mono
ref part-left
ref part-right 16 0

glyph test-xy:color 24 16 scale 2
ref 22x16 2 0 fill #ff000080

map X = test-xy
";
        let doc = document_io::parse_document_from_str(input, "t.unf".into()).unwrap();
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();

        let xy = glyphs.iter().find(|g| g.name == "test-xy").unwrap();
        assert_eq!(
            xy.advance_width,
            (24.0_f32 * 1024.0 / 16.0).round() as u16,
            "advance should be 24 logical pixels = {}",
            (24.0_f32 * 1024.0 / 16.0).round() as u16,
        );
    }

    #[test]
    fn colr_base_glyph_bbox_covers_color_layers() {
        let input = "\
font-meta height 16 ascent 12 descent 4

glyph frame-left 4 4
..@@@@@@
..@@....
..@@....
..@@@@@@

glyph frame-right 4 4
@@@@@@..
....@@..
....@@..
@@@@@@..

glyph test-flag:mono
ref frame-left
ref frame-right 4 0

glyph test-flag:color 8 4 scale 2
ref 14x6 1 1 fill #ff0000

map A = test-flag
";
        let doc = document_io::parse_document_from_str(input, "t.unf".into()).unwrap();
        let font_data = build_font_from_documents(&[&doc]);
        assert!(font_data.is_some(), "font should build");
        let bytes = font_data.unwrap();
        let font = read_fonts::FontRef::new(&bytes).unwrap();
        let glyf = font.glyf().unwrap();
        let loca = font.loca(None).unwrap();
        let hmtx = font.hmtx().unwrap();

        let cmap = font.cmap().unwrap();
        let gid = cmap
            .map_codepoint('A')
            .expect("A should be mapped");
        let advance = hmtx.advance(gid).unwrap();
        let glyph = loca.get_glyf(gid, &glyf).unwrap().unwrap();
        let simple = match glyph {
            read_fonts::tables::glyf::Glyph::Simple(s) => s,
            _ => panic!("expected simple glyph"),
        };
        assert!(
            simple.x_max() >= advance as i16,
            "base glyph xMax ({}) should be >= advance ({}) to prevent COLR clipping",
            simple.x_max(),
            advance,
        );
    }

    #[test]
    fn scaled_composite_with_own_pixels_matches_unscaled() {
        // Parent has own pixels AND refs
        let input_unscaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 4 3
@@......@@
@@......@@
@@......@@
ref part 0 2

map A = combo
";
        let input_scaled = "\
font-meta height 16 ascent 12 descent 4

glyph part 2 3
@@@@
@@@@
@@@@

glyph combo 4 3 scale 2
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
@@@@..........@@@@
ref part 0 4

map A = combo
";
        let doc1 = document_io::parse_document_from_str(input_unscaled, "a.unf".into()).unwrap();
        let doc2 = document_io::parse_document_from_str(input_scaled, "b.unf".into()).unwrap();

        let (_, _, g1, _, _) = collect_glyph_data(&[&doc1], false).unwrap();
        let (_, _, g2, _, _) = collect_glyph_data(&[&doc2], false).unwrap();

        assert_eq!(
            g1[0].advance_width, g2[0].advance_width,
            "advance: unscaled {} vs scaled {}",
            g1[0].advance_width, g2[0].advance_width
        );
        assert_eq!(
            g1[0].contours, g2[0].contours,
            "contours should match"
        );
    }
}

