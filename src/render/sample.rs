//! What the specimen pages are told about a source: the characters it maps,
//! the features it carries, and the bodies of text it offers.
//!
//! The glyphs themselves are the *font's* to draw — [`crate::render::demo`]
//! embeds the built face rather than pictures of it — so what is collected
//! here is the cmap and nothing that draws. The cmap is still resolved
//! through the driver shared with the TTF builder
//! (`render/glyph_cache.rs`), because a mapping whose target never resolved
//! claims no code point in the font either, and a page listing a character
//! the font does not map is the old way the two came to disagree.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self};
use std::path::Path;

use crate::document::*;
use crate::render::ttf_builder::expand_map_pairs;
use crate::ucd::CharProps;

struct SampleData {
    /// codepoint → glyph name, sorted by codepoint
    cmap: BTreeMap<u32, String>,
    /// OpenType feature tags
    features: Vec<String>,
}

/// What a specimen page is told about a source.
///
/// Resolving the cmap is most of what collecting this costs, so a `build`
/// collects it once and lends it to whatever pages it writes.
pub struct SampleSource {
    data: SampleData,
    char_props: CharProps,
    samples: crate::samples::SampleSet,
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
/// the larger half of collecting the sample, so the two share the one rather
/// than doing it twice.
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

    // A source with no em height describes no font, and so no sample either.
    if crate::meta::FontMeta::collect(docs).height() == 0 {
        return None;
    }

    let glyph_aliases = &resolution.expansion.aliases;
    let all_items = || resolution.expansion.items();
    let face = resolution.faces.primary();

    // The resolution cache: what a glyph *is*, to the extent the cmap needs
    // it. Dimensions and anchors, because those decide which glyphs resolve
    // at all; no contours and no layers, because nothing here draws.
    struct CachedGlyph {
        /// Extent right of / below the glyph origin; area a negative ref
        /// offset puts *before* the origin is a bearing and is not counted.
        width: u16,
        height: u16,
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
        scale: u8,
    }

    impl CachedGlyph {
        fn empty() -> Self {
            Self {
                declared_origin: (0, 0),
                width: 0,
                height: 0,
                anchors: Vec::new(),
                grid: None,
                origin_row: 0,
                origin_col: 0,
                scale: 1,
            }
        }

        /// `desync`: the grid is the glyph's bitmap and not its outline, so
        /// the raster grid a parent composes against is a blank of the same
        /// size — the glyph still bounds itself, and contributes no ink.
        fn from_grid(grid: &PixelGrid, desync: bool) -> Self {
            Self {
                declared_origin: (0, 0),
                width: grid.width,
                height: grid.height,
                anchors: Vec::new(),
                grid: Some(if desync {
                    PixelGrid::new(grid.width, grid.height)
                } else {
                    grid.clone()
                }),
                origin_row: 0,
                origin_col: 0,
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
    for item in all_items() {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        {
            glyph_declared_anchors
                .entry(n.clone())
                .or_insert_with(|| body.points.clone());
        }
    }

    let (mut cache, pending) = crate::render::glyph_cache::seed_cache(
        all_items(),
        |_, grid, desync| CachedGlyph::from_grid(grid, desync),
        CachedGlyph::empty,
        &crate::cancel::CancelToken::never(),
    );
    crate::render::glyph_cache::resolve_pending(
        &mut cache,
        pending,
        &crate::document::collect_anchor_aligns(all_items()),
        |name| glyph_declared_anchors.get(name).cloned(),
        &mut crate::render::glyph_cache::FnBuilder(
            |pg: &crate::render::glyph_cache::PendingGlyph,
             effective_refs: &[GlyphRef],
             cache: &_| {
                composite_glyph(pg.pixels.as_ref(), pg.desync, effective_refs, cache, pg.scale)
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

    fn composite_glyph(
        own_pixels: Option<&PixelGrid>,
        desync: bool,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedGlyph>,
        parent_scale: u8,
    ) -> Option<CachedGlyph> {
        let has_negated = refs.iter().any(|r| r.negated);
        // An all-empty own grid only declares dimensions; treating it as a
        // real layer would pin the composite's origin to (0, 0) and shift
        // refs placed at negative offsets into positive territory.  The
        // declared dims are re-applied by the caller.  Mirrors
        // `CachedContours::from_components_inner`.
        let own_pixels = own_pixels.filter(|g| !g.is_all_empty());
        // A `desync` grid still bounds the glyph, but it is not part of the
        // outline — so it is kept out of the raster a parent composes
        // against, exactly as in the TTF builder's vector pass.
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

            if let Some(grid) = own_outline {
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

            return Some(CachedGlyph {
                declared_origin: (0, 0),
                width: (min_c + raster_w).max(0) as u16,
                height: (min_r + raster_h).max(0) as u16,
                anchors: Vec::new(),
                grid: Some(result),
                origin_row,
                origin_col,
                scale: ps,
            });
        }

        // A composite of nothing but refs: its extent is the reach of what it
        // places, and its raster is those grids put side by side.
        let mut max_width = 0i32;
        let mut max_height = 0i32;
        let mut min_r = 0i32;
        let mut min_c = 0i32;

        for (gref, sg) in refs.iter().zip(ref_scaled.iter()) {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let (box_col, box_row) = cached.declared_origin;
            let dx = gref.col() as f32 - box_col as f32 * ps as f32;
            let dy = gref.row() as f32 - box_row as f32 * ps as f32;
            let rs = cached.scale.max(1);
            let rsf = ps as f32 / rs as f32;
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
            anchors: Vec::new(),
            grid: combined_grid,
            origin_row,
            origin_col,
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

    Some(SampleData { cmap, features })
}

impl SampleSource {
    /// Every character the primary face maps, and the glyph name it maps to.
    pub fn cmap(&self) -> &BTreeMap<u32, String> {
        &self.data.cmap
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

    /// The [`sample`](crate::samples) lines of the source.
    ///
    /// Collected here rather than by the page for itself, so that a page
    /// handed no documents can still ask which of the generated bodies of
    /// text this source asked for.
    pub fn samples(&self) -> &crate::samples::SampleSet {
        &self.samples
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
            samples: crate::samples::SampleSet::collect(
                docs.iter().flat_map(|doc| doc.items.iter()),
            ),
        })
    }
}

/// One language's Article 1, as `udhr-article1.json` writes it.
///
/// `lang` is the UDHR's own key for the translation — an ISO 639-3 code where
/// the data has one (`eng`, `kor`), sometimes with a variant suffix
/// (`aka_asante`), and a bare number where it has none.
///
/// `name` is the UDHR's own name for the translation, which is a *fallback*
/// and not the first answer: `demo.html` asks the browser
/// (`Intl.DisplayNames`) about the key first, because CLDR names a language
/// the way a reader expects it named where this table names it the UDHR's way
/// (`Crioulo, Upper Guinea (008)`). It is carried at all because the browser
/// has no name for the numeric keys, which is what a page falls back to
/// printing outright.
pub(crate) struct UdhrEntry {
    pub lang: String,
    pub name: String,
    pub text: String,
}

/// The translations of Article 1 a font can actually show, in the order the
/// pages list them.
///
/// Two filters, in this order. First a translation whose text holds one
/// character the font does not map is dropped outright — a sample with a
/// notdef box in it says nothing about the font. Then the survivors are taken
/// greedily in file order, keeping only one that draws a code point no kept
/// translation drew before it: five hundred translations of one paragraph are
/// mostly the same Latin letters over again, and what a reader of a specimen
/// wants is the ones that are *not*. The file's own order is what decides ties,
/// which is why it opens with English, Russian and Korean.
///
/// Read by the demo page's sample panel; see [`crate::render::demo`].
pub(crate) fn udhr_selection(
    data_dir: &Path,
    cmap: &BTreeMap<u32, String>,
) -> io::Result<Vec<UdhrEntry>> {
    #[derive(serde::Deserialize)]
    struct RawEntry {
        lang: String,
        name: String,
        text: String,
    }

    let path = data_dir.join("udhr-article1.json");
    let content = std::fs::read_to_string(&path)?;
    let entries: Vec<RawEntry> = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let cmap_set: HashSet<u32> = cmap.keys().copied().collect();
    let mut covered: HashSet<u32> = HashSet::new();
    let mut selected: Vec<UdhrEntry> = Vec::new();
    for entry in entries {
        if entry
            .text
            .chars()
            .any(|ch| !cmap_set.contains(&(ch as u32)))
        {
            continue;
        }
        if !entry.text.chars().any(|ch| !covered.contains(&(ch as u32))) {
            continue;
        }
        covered.extend(entry.text.chars().map(|ch| ch as u32));
        selected.push(UdhrEntry {
            lang: entry.lang,
            name: entry.name,
            text: entry.text,
        });
    }
    Ok(selected)
}

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

/// The CLDR subdivision containment data as one text: a line per region,
/// naming it and then the emoji tag sequence of each of its subdivisions.
///
/// This is what a [`subdivision-flags`](crate::samples::SampleMode) sample
/// stands for. The regions come out in the file's key order, which is a
/// `BTreeMap`'s and so alphabetical.
pub(crate) fn subdivision_flags_text(path: &Path) -> io::Result<String> {
    #[derive(serde::Deserialize)]
    struct SubdivisionFile {
        subdivisions: BTreeMap<String, Vec<String>>,
    }

    let content = std::fs::read_to_string(path)?;
    let parsed: SubdivisionFile = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut out = String::new();
    for (region, codes) in &parsed.subdivisions {
        let seqs: Vec<String> = codes
            .iter()
            .filter_map(|c| subdivision_flag_seq(c))
            .collect();
        if seqs.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(region);
        out.push(' ');
        out.extend(seqs);
    }
    Ok(out)
}

/// The CLDR subdivision containment file of a data directory, if it has one:
/// `cldr-subdivisions-48.2.0.json` and the like. The version is part of the
/// file name, so it is matched by prefix rather than pinned here.
pub(crate) fn subdivisions_path(data_dir: &Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(data_dir).ok().and_then(|entries| {
        entries.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("cldr-subdivisions-") && n.ends_with(".json"))
        })
    })
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
