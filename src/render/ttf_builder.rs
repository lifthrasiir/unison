use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

use write_fonts::tables::cmap::Cmap;
use write_fonts::tables::gdef::Gdef;
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
    ChainedSequenceContext, ChainedSequenceContextFormat3, CoverageTable, Feature, FeatureList,
    FeatureRecord, LangSys, Lookup, LookupFlag, LookupList, Script, ScriptList, ScriptRecord,
    SequenceLookupRecord,
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
use crate::render::contour::track_contour;

const UNITS_PER_EM: u16 = 1024;

// ---------------------------------------------------------------------------
// Persistent contour cache — survives across incremental rebuilds
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct ContourCache {
    entries: HashMap<u64, Vec<Vec<(f32, f32)>>>,
}

pub type SharedContourCache = Arc<Mutex<ContourCache>>;

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
impl ContourCache {
    pub fn clear(&mut self) {
        self.entries.clear();
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
    bitmap.hash(&mut hasher);
    hasher.finish()
}

fn cached_track_contour(
    cache: &mut ContourCache,
    grid: &PixelGrid,
    bitmap: bool,
) -> Vec<Vec<(f32, f32)>> {
    let key = hash_grid_for_cache(grid, bitmap);
    if let Some(cached) = cache.entries.get(&key) {
        return cached.clone();
    }
    let contours = track_contour(grid, PX_SUBPIXEL);
    cache.entries.insert(key, contours.clone());
    contours
}

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
}

// ---------------------------------------------------------------------------
// GSUB remap data structures
// ---------------------------------------------------------------------------

struct ExpandedRemap {
    lookbehind: Vec<Vec<String>>,
    /// Each inner Vec is a sequence of input glyph positions (len > 1 = ligature).
    source: Vec<Vec<String>>,
    target: Vec<String>,
    lookahead: Vec<Vec<String>>,
}

struct GsubData {
    remap_sets: BTreeMap<String, Vec<ExpandedRemap>>,
    /// (feature_tag, scripts, remap_set_names)
    features: Vec<(String, Vec<String>, Vec<String>)>,
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

#[allow(dead_code)]
pub fn build_bitmap_font_from_documents(docs: &[&Document]) -> Option<Vec<u8>> {
    build_font_from_documents_inner(docs, true, None)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn build_font_pair_cached(
    docs: &[&Document],
    shared_cache: &SharedContourCache,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut cc = shared_cache.lock().unwrap();

    let bitmap_data = collect_glyph_data_cached(docs, true, Some(&mut cc))?;
    let vector_data = collect_glyph_data_cached(docs, false, Some(&mut cc))?;

    drop(cc);

    let (b_meta, _, b_glyphs, b_gsub) = bitmap_data;
    let (v_meta, v_scale, v_glyphs, v_gsub) = vector_data;

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
        let bh = s.spawn(|| build_ttf(b_ascender, b_descender, &b_glyphs, 0, &b_gsub));
        let vector = build_ttf(v_ascender, v_descender, &v_glyphs, v_hint_ppem, &v_gsub);
        let bitmap = bh.join().unwrap();
        (bitmap, vector)
    });

    Some((bitmap, vector))
}

fn build_font_from_documents_inner(docs: &[&Document], bitmap: bool, contour_cache: Option<&mut ContourCache>) -> Option<Vec<u8>> {
    let (meta, scale, glyph_data, gsub_data) = collect_glyph_data_cached(docs, bitmap, contour_cache)?;

    let ascender = (meta.ascent as f32 * scale).round() as i16;
    let descender = -((meta.descent as f32 * scale).round() as i16);

    let hint_ppem = if !bitmap && UNITS_PER_EM.is_multiple_of(meta.height) {
        meta.height
    } else {
        0
    };
    Some(build_ttf(ascender, descender, &glyph_data, hint_ppem, &gsub_data))
}

/// Resolve all documents' glyph items (expanding name patterns, following
/// refs, tracking contours) into the flat, codepoint-sorted glyph list that
/// [`build_font_from_documents`] then hands to [`build_ttf`]. Split out so
/// tests can inspect the intermediate, pre-TTF-encoding representation
/// directly (e.g. to canonicalize away the non-deterministic contour
/// point/rotation order that `track_contour` can produce — see
/// `tests::ttf_build_digest_real_files_is_stable`).
#[cfg(test)]
fn collect_glyph_data(docs: &[&Document], bitmap: bool) -> Option<(FontMeta, f32, Vec<CollectedGlyph>, GsubData)> {
    collect_glyph_data_cached(docs, bitmap, None)
}

fn collect_glyph_data_cached(docs: &[&Document], bitmap: bool, mut contour_cache: Option<&mut ContourCache>) -> Option<(FontMeta, f32, Vec<CollectedGlyph>, GsubData)> {
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

    // Collect all items, expanding ranged glyph names
    let mut all_items: Vec<DocumentItem> = Vec::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Glyph { name, body } = item {
                let name_str = substitute_name_parts(&name.display(), &name_parts);
                if is_name_pattern(&name_str) {
                    let subst_name = GlyphName(name_str);
                    let subst_refs: Vec<GlyphRef> = body
                        .refs
                        .iter()
                        .map(|r| GlyphRef {
                            name: substitute_name_parts(&r.name, &name_parts),
                            offset: r.offset,
                            negated: r.negated,
                        })
                        .collect();
                    if let Ok(expanded) = expand_glyph_block(&subst_name, &subst_refs) {
                        for mut item in expanded {
                            if let DocumentItem::Glyph { body: ref mut b, .. } = item {
                                b.pixels = body.pixels.clone();
                                b.points = body.points.clone();
                                b.sticky = body.sticky;
                                b.advance = body.advance;
                                b.left = body.left;
                            }
                            all_items.push(item);
                        }
                    }
                } else {
                    let mut body = body.clone();
                    for gref in &mut body.refs {
                        gref.name = substitute_name_parts(&gref.name, &name_parts);
                    }
                    all_items.push(DocumentItem::Glyph {
                        name: GlyphName(name_str),
                        body,
                    });
                }
            } else if let DocumentItem::Map { char_repr, glyph } = item {
                all_items.push(DocumentItem::Map {
                    char_repr: char_repr.clone(),
                    glyph: substitute_name_parts(glyph, &name_parts),
                });
            } else {
                all_items.push(item.clone());
            }
        }
    }

    // Contour cache: compute track_contour once per unique named glyph,
    // then composite glyphs just translate and concatenate cached contours.
    let mut cache: HashMap<String, CachedContours> = HashMap::new();

    struct PendingGlyph {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
    }
    let mut pending: Vec<PendingGlyph> = Vec::new();

    for item in &all_items {
        let (cache_key, body) = match item {
            DocumentItem::Glyph { name: GlyphName(n), body } => (n.clone(), body),
            _ => continue,
        };
        if !cache_key.is_empty() && !cache.contains_key(&cache_key) {
            if let Some(ref pixels) = body.pixels && body.refs.is_empty() {
                let mut cached = CachedContours::from_grid(pixels, bitmap, contour_cache.as_deref_mut());
                cached.anchors = body.points.clone();
                cache.insert(cache_key, cached);
            } else if body.pixels.is_some() || !body.refs.is_empty() {
                pending.push(PendingGlyph {
                    name: cache_key,
                    pixels: body.pixels.clone(),
                    refs: body.refs.clone(),
                    points: body.points.clone(),
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
                    },
                );
            }
        }
    }

    // Iteratively resolve named glyphs that depend on other named glyphs
    let mut progress = true;
    while progress {
        progress = false;
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
            let (effective_refs, anchors) =
                crate::ref_composite::derive_ref_offsets_with(
                    &pg.points,
                    &pg.refs,
                    |name| {
                        resolve_cached_ref(name, &cache)
                            .map(|resolved| resolved.anchors.clone())
                    },
                );
            let mut cached_entry = CachedContours::from_components(
                pg.pixels.as_ref(),
                &effective_refs,
                &cache,
                bitmap,
                contour_cache.as_deref_mut(),
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
                }
            });
            if let Some(grid) = &pg.pixels {
                cached_entry.width = cached_entry.width.max(grid.width);
                cached_entry.height = cached_entry.height.max(grid.height);
            }
            cached_entry.anchors = anchors;
            cache.insert(pg.name.clone(), cached_entry);
            progress = true;
        }
    }

    let gsub_data = collect_gsub_data(docs, &name_parts);

    let mut glyph_meta: HashMap<String, (Option<u16>, Option<i16>)> = HashMap::new();
    let mut inline_glyphs: HashSet<String> = HashSet::new();
    for item in &all_items {
        if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
            if body.advance.is_some() || body.left.is_some() {
                glyph_meta.insert(n.clone(), (body.advance, body.left));
            }
            if body.inline {
                inline_glyphs.insert(n.clone());
            }
        }
    }

    let mut glyph_data: Vec<CollectedGlyph> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    for item in &all_items {
        let DocumentItem::Map { char_repr, glyph } = item else { continue };

        let pairs = expand_map_pairs(char_repr, glyph);
        for (cp, glyph_name) in &pairs {
            let Some(resolved) = cache.get(glyph_name.as_str()) else { continue };

            let font_contours: Vec<Vec<(i16, i16)>> = resolved
                .contours
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|&(x, y)| {
                            (
                                (x * scale).round() as i16,
                                ((meta.ascent as f32 - y) * scale).round() as i16,
                            )
                        })
                        .collect()
                })
                .collect();

            let advance_width = match glyph_meta.get(glyph_name.as_str()) {
                Some(&(Some(adv), _)) => (adv as f32 * scale).round() as u16,
                _ => (resolved.width as f32 * scale).round() as u16,
            };
            let left_offset = match glyph_meta.get(glyph_name.as_str()) {
                Some(&(_, Some(left))) => (left as f32 * scale).round() as i16,
                _ => 0,
            };
            let font_contours = if left_offset != 0 {
                font_contours.into_iter().map(|c| {
                    c.into_iter().map(|(x, y)| (x + left_offset, y)).collect()
                }).collect()
            } else {
                font_contours
            };

            let composite_refs = if !inline_glyphs.contains(glyph_name.as_str()) {
                if let Some(comps) = &resolved.composite_components {
                    comps.iter().map(|(name, dx, dy)| {
                        CompositeRef {
                            component_name: name.clone(),
                            x_offset: ((*dx + left_offset as f32 / scale) * scale).round() as i16,
                            y_offset: (-*dy * scale).round() as i16,
                        }
                    }).collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            seen_names.insert(glyph_name.clone());
            glyph_data.push(CollectedGlyph {
                name: glyph_name.clone(),
                codepoint: Some(*cp),
                advance_width,
                contours: font_contours,
                composite_refs,
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
            for name in &r.target {
                remap_referenced.insert(name.as_str());
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
    for item in &all_items {
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
        };
        let resolved = cache.get(glyph_name.as_str()).unwrap_or(&empty_cached);
        let font_contours: Vec<Vec<(i16, i16)>> = resolved
            .contours
            .iter()
            .map(|c| {
                c.iter()
                    .map(|&(x, y)| {
                        (
                            (x * scale).round() as i16,
                            ((meta.ascent as f32 - y) * scale).round() as i16,
                        )
                    })
                    .collect()
            })
            .collect();

        let advance_width = match glyph_meta.get(glyph_name.as_str()) {
            Some(&(Some(adv), _)) => (adv as f32 * scale).round() as u16,
            _ => (resolved.width as f32 * scale).round() as u16,
        };
        let left_offset = match glyph_meta.get(glyph_name.as_str()) {
            Some(&(_, Some(left))) => (left as f32 * scale).round() as i16,
            _ => 0,
        };
        let font_contours = if left_offset != 0 {
            font_contours.into_iter().map(|c| {
                c.into_iter().map(|(x, y)| (x + left_offset, y)).collect()
            }).collect()
        } else {
            font_contours
        };

        let composite_refs = if !inline_glyphs.contains(glyph_name.as_str()) {
            if let Some(comps) = &resolved.composite_components {
                comps.iter().map(|(name, dx, dy)| {
                    CompositeRef {
                        component_name: name.clone(),
                        x_offset: ((*dx + left_offset as f32 / scale) * scale).round() as i16,
                        y_offset: (-*dy * scale).round() as i16,
                    }
                }).collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        glyph_data.push(CollectedGlyph {
            name: glyph_name.clone(),
            codepoint: None,
            advance_width,
            contours: font_contours,
            composite_refs,
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
                };
                let resolved = cache.get(cr.component_name.as_str()).unwrap_or(&empty_cached);
                let font_contours: Vec<Vec<(i16, i16)>> = resolved
                    .contours
                    .iter()
                    .map(|c| {
                        c.iter()
                            .map(|&(x, y)| {
                                (
                                    (x * scale).round() as i16,
                                    ((meta.ascent as f32 - y) * scale).round() as i16,
                                )
                            })
                            .collect()
                    })
                    .collect();
                let advance_width = (resolved.width as f32 * scale).round() as u16;
                component_extras.push(CollectedGlyph {
                    name: cr.component_name.clone(),
                    codepoint: None,
                    advance_width,
                    contours: font_contours,
                    composite_refs: Vec::new(),
                });
            }
        }
    }
    glyph_data.append(&mut component_extras);

    if glyph_data.is_empty() {
        return None;
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

    Some((meta, scale, glyph_data, gsub_data))
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
                    let source_positions: Vec<&str> = source.split_whitespace().collect();
                    let expanded_positions: Vec<Vec<String>> = source_positions
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

                    let tgt_names = expand_remap_element(target, name_parts);

                    let mut source_seqs = Vec::with_capacity(entry_count);
                    let mut target_expanded = Vec::with_capacity(entry_count);
                    for i in 0..entry_count {
                        let seq: Vec<String> = expanded_positions
                            .iter()
                            .map(|pos| pos[i % pos.len()].clone())
                            .collect();
                        source_seqs.push(seq);
                        target_expanded.push(tgt_names[i % tgt_names.len()].clone());
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
                            target: target_expanded,
                            lookahead: la,
                        },
                    );
                }
                DocumentItem::Feature { name, scripts, remap_group } => {
                    features.push((name.clone(), scripts.clone(), vec![remap_group.clone()]));
                }
                _ => {}
            }
        }
    }

    GsubData {
        remap_sets,
        features,
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
                    let helper = build_single_subst_from_pairs(&first_sources, &r.target, name_to_gid);
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
        let tag_bytes = feat_tag.as_bytes();
        let mut tag_arr = [b' '; 4];
        for (i, &b) in tag_bytes.iter().enumerate().take(4) {
            tag_arr[i] = b;
        }
        let tag = Tag::new(&tag_arr);

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

    let mut script_records: Vec<ScriptRecord> = Vec::new();
    for (script_tag, feat_indices) in &script_feature_indices {
        let tag_bytes = script_tag.as_bytes();
        let mut tag_arr = [b' '; 4];
        for (i, &b) in tag_bytes.iter().enumerate().take(4) {
            tag_arr[i] = b;
        }
        let lang_sys = LangSys {
            required_feature_index: 0xFFFF,
            feature_indices: feat_indices.clone(),
        };
        let script = Script::new(Some(lang_sys), vec![]);
        script_records.push(ScriptRecord::new(Tag::new(&tag_arr), script));
    }

    let script_list = ScriptList::new(script_records);
    let feature_list = FeatureList::new(feature_records);
    let lookup_list: LookupList<SubstitutionLookup> = LookupList::new(lookups);

    Some(Gsub::new(script_list, feature_list, lookup_list))
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
            if seq.len() == 1 {
                all_sources.push(seq[0].clone());
                all_targets.push(tgt.clone());
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
            let Some(&tgt_gid) = name_to_gid.get(tgt.as_str()) else {
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

struct CachedContours {
    width: u16,
    height: u16,
    contours: Vec<Vec<(f32, f32)>>,
    anchors: Vec<GlyphPoint>,
    grid: Option<PixelGrid>,
    /// For composite-eligible glyphs: (component_name, col_offset, row_offset)
    composite_components: Option<Vec<(String, f32, f32)>>,
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
            }
        }
    }

    fn from_components(
        own_pixels: Option<&PixelGrid>,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedContours>,
        bitmap: bool,
        cc: Option<&mut ContourCache>,
    ) -> Option<Self> {
        let has_negated = refs.iter().any(|r| r.negated);
        let own_pixels = own_pixels.filter(|g| !g.is_all_empty());

        if has_negated || own_pixels.is_some() {
            // Use pixel-level composition (same semantics as the editor)
            // to correctly handle negated refs and own-pixel interaction.
            let mut min_r: i32 = 0;
            let mut min_c: i32 = 0;
            let mut max_r: i32 = 0;
            let mut max_c: i32 = 0;
            if let Some(grid) = own_pixels {
                max_r = grid.height as i32;
                max_c = grid.width as i32;
            }
            for gref in refs {
                if let Some(cached) = resolve_cached_ref(&gref.name, cache) {
                    let eff_r = gref.row() as i32;
                    let eff_c = gref.col() as i32;
                    if cached.width != 0 && cached.height != 0 {
                        min_r = min_r.min(eff_r);
                        min_c = min_c.min(eff_c);
                        max_r = max_r.max(eff_r + cached.height as i32);
                        max_c = max_c.max(eff_c + cached.width as i32);
                    }
                }
            }
            let width = (max_c - min_c).max(0) as u16;
            let height = (max_r - min_r).max(0) as u16;
            let mut result = PixelGrid::new(width, height);

            // Paint own pixels first
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

            // Apply refs in order (same as editor's composite_to_grid)
            for gref in refs {
                let Some(cached) = resolve_cached_ref(&gref.name, cache) else { continue };
                let Some(ref_grid) = &cached.grid else { continue };
                let off_r = gref.row() as i32 - min_r;
                let off_c = gref.col() as i32 - min_c;
                for r in 0..ref_grid.height as i32 {
                    for c in 0..ref_grid.width as i32 {
                        let shape = ref_grid.get(r as u16, c as u16);
                        if !shape.is_empty() {
                            let dr = off_r + r;
                            let dc = off_c + c;
                            if dr >= 0 && dc >= 0 && dr < height as i32 && dc < width as i32 {
                                if gref.negated {
                                    let current = result.get(dr as u16, dc as u16);
                                    if !current.is_empty() {
                                        let out = if shape.shape_id() == 0 && shape.is_filled() {
                                            PixelShape::EMPTY
                                        } else if current.shape_id() == 0 && current.is_filled() {
                                            shape.negated()
                                        } else if current == shape {
                                            PixelShape::EMPTY
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

            let cached = Self::from_grid(&result, bitmap, cc);
            return Some(Self {
                width,
                height,
                contours: cached.contours,
                anchors: Vec::new(),
                grid: Some(result),
                composite_components: None,
            });
        }

        // No negated refs and no own pixels: simple contour translation
        let mut all_contours = Vec::new();
        let mut max_width = 0u16;
        let mut max_height = 0u16;
        let mut combined_grid: Option<PixelGrid> = None;
        let mut components = Vec::new();

        for gref in refs {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let dx = gref.col() as f32;
            let dy = gref.row() as f32;
            components.push((gref.name.clone(), dx, dy));
            for contour in &cached.contours {
                let translated: Vec<(f32, f32)> =
                    contour.iter().map(|&(x, y)| (x + dx, y + dy)).collect();
                all_contours.push(translated);
            }
            let w = (gref.col() as i32 + cached.width as i32).max(0) as u16;
            let h = (gref.row() as i32 + cached.height as i32).max(0) as u16;
            max_width = max_width.max(w);
            max_height = max_height.max(h);

            // Combine grids for downstream composites
            if let Some(ref_grid) = &cached.grid {
                let cg = combined_grid.get_or_insert_with(|| PixelGrid::new(max_width, max_height));
                if cg.width < max_width || cg.height < max_height {
                    cg.resize(max_width, max_height);
                }
                let off_r = gref.row() as i32;
                let off_c = gref.col() as i32;
                for r in 0..ref_grid.height as i32 {
                    for c in 0..ref_grid.width as i32 {
                        let shape = ref_grid.get(r as u16, c as u16);
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
        })
    }
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
) -> Vec<u8> {
    let num_glyphs = u16::try_from(glyphs.len() + 1).expect("glyph count checked earlier"); // +1 for .notdef

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
    for (_i, g) in glyphs.iter().enumerate() {
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

            h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: sg.bbox.x_min,
            });

            glyf_builder.add_glyph(&sg).unwrap();
        }
    }

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

    // GSUB / GDEF
    let gsub = build_gsub(gsub_data, &name_to_gid);

    // Assemble
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

    if let Some(ref gsub) = gsub {
        builder.add_table(gsub).unwrap();
        let gdef = Gdef::default();
        builder.add_table(&gdef).unwrap();
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

        let (_, _, glyph_data_a, _) =
            collect_glyph_data(&doc_refs, false).expect("expected glyph data");
        let (_, _, glyph_data_b, _) =
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
        let (_, _, glyphs, _) = collect_glyph_data(&[&doc], false).unwrap();

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
        let (_, _, glyphs, _) = collect_glyph_data(&[&doc], false).unwrap();
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
        let (_, _, glyphs, _) = collect_glyph_data(&[&doc], false).unwrap();
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

        let (_, _, glyph_data, gsub_data) =
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
        let (_, _, glyph_data, gsub_data) = collect_glyph_data(&[&doc], false).unwrap();

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
        let (_, _, _glyph_data, gsub_data) = collect_glyph_data(&[&doc], false).unwrap();

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
        let (_, _, glyph_data, _) = collect_glyph_data(&[&doc], false).unwrap();
        assert!(
            glyph_data.iter().any(|g| g.name == "b"),
            "remap-referenced empty glyph should survive"
        );
    }

}
