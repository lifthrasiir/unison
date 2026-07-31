//! The TrueType/OpenType font builder.
//!
//! [`build_font_from_documents`] and friends are the entry points; this module
//! holds the shared vocabulary (the collected-glyph types and the build
//! drivers) and delegates each stage to a submodule: `expand` (pattern
//! expansion, on-demand and decomposed-`map` item synthesis), `collect`
//! (per-glyph refs, metrics, traced contours), `contours` (the contour cache and
//! `CachedContours`), `color`, `gsub`, `gpos`, `hints`, `outlines`
//! (glyf/metrics/cmap emission) and `tables` (final table assembly). The tests
//! are in `render/ttf_tests/`.
//!
//! [`UNITS_PER_EM`] is 1024. The font is built twice — once reading the exact
//! geometry, once keeping only the ink flag for a bitmap-style outline; see
//! [`crate::ref_composite`] on `BitmapFill` for what a synthesized shape has to
//! decide because of that.
//!
//! This module and [`crate::render::contour`] are where most of the fixes land,
//! and the theme is always the same: sub-pixel and on-demand shapes seen through
//! a *composite* rather than on their own (fractional on-demand glyphs, triangle
//! subglyphs, an `xMax` underestimated when `coloronly`/`monoonly` layers mix,
//! panics from multi-part shapes). Check contour output at composite level.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
#[cfg(feature = "editor")]
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
use write_fonts::tables::glyf::{
    Bbox, Component, ComponentFlags, CompositeGlyph, Contour, Glyph, GlyfLocaBuilder,
    SimpleGlyph
};
use read_fonts::tables::glyf::Anchor;
use read_fonts::tables::glyf::CurvePoint;
use write_fonts::tables::gsub::{
    Gsub, Ligature, LigatureSet, LigatureSubstFormat1, MultipleSubstFormat1,
    ReverseChainSingleSubstFormat1, Sequence, SingleSubst, SingleSubstFormat2,
    SubstitutionChainContext, SubstitutionLookup,
};
use write_fonts::tables::head::{Flags, Head};
use write_fonts::tables::hhea::Hhea;
use write_fonts::tables::hmtx::{Hmtx, LongMetric};
use write_fonts::tables::layout::{
    ChainedSequenceContext, ChainedSequenceContextFormat3, ClassDef, ClassDefFormat2,
    ClassRangeRecord, CoverageTable, Feature, FeatureList, FeatureRecord, LangSys, LangSysRecord,
    Lookup,
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
use crate::issues::Severity;
use crate::resolve::{Diagnostic, FontMeta, ItemRef};
use crate::pixel::{PX_ALMOSTFULL, PX_CUSTOM, PX_SUBPIXEL, PixelShape};
use crate::render::contour::{track_contour, track_contour_multi_at, track_contour_multi_diff_at};
use crate::render::glyph_cache::{
    build_alt_index as build_cached_alternatives, resolve_cached as resolve_cached_ref,
};

mod collect;
mod color;
mod contours;
mod expand;
mod gpos;
mod gsub;
mod hints;
mod outlines;
mod tables;

pub use color::{
    ColorAliasMap, Rgba, collect_color_aliases, effective_visibility, resolve_fill_rgba
};
#[cfg(any(feature = "editor", test))]
pub use color::parse_hex_color;
pub use contours::ContourCache;
#[cfg(feature = "editor")]
pub use contours::{SharedContourCache, new_contour_cache};
pub(crate) use expand::{
    Expansion, collect_expanded_items, decomposed_map_pairs, expand_documents, expand_map_pairs, parse_map_char
};
pub(crate) use gsub::remap_rule_kind;

use collect::collect_glyph_data_cached;
#[cfg(feature = "editor")]
use collect::{collect_glyph_data_with_shared, compute_shared_font_input};
use tables::build_ttf;

pub const UNITS_PER_EM: u16 = 1024;

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
    /// Lookup order and per-group properties. `remap_sets` is keyed by name and
    /// so says nothing about order; this is the only thing that does.
    groups: crate::document::RemapGroupOrder,
    /// (feature_tag, scripts, remap_set_names)
    features: Vec<(String, Vec<String>, Vec<String>)>,
    /// Anchor-based feature declarations: (feature_tag, scripts, anchor_name)
    anchor_features: Vec<(String, Vec<String>, String)>,
}

pub fn load_docs_from_directory_checked(dir: &Path) -> (Vec<Document>, Vec<(std::path::PathBuf, String)>) {
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (docs, errors);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if document_io::is_source_file(&path) {
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

#[cfg(feature = "editor")]
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

    let b_scale = UNITS_PER_EM as f32 / b_meta.height() as f32;
    let b_ascender = (b_meta.ascent() as f32 * b_scale).round() as i16;
    let b_descender = -((b_meta.descent() as f32 * b_scale).round() as i16);

    let v_ascender = (v_meta.ascent() as f32 * v_scale).round() as i16;
    let v_descender = -((v_meta.descent() as f32 * v_scale).round() as i16);

    let v_hint_ppem = if UNITS_PER_EM.is_multiple_of(v_meta.height()) {
        v_meta.height()
    } else {
        0
    };

    let (bitmap, vector) = std::thread::scope(|s| {
        let bh = s.spawn(|| build_ttf(b_ascender, b_descender, &b_glyphs, 0, &b_gsub, &b_palette, b_scale, b_meta.ascent()));
        let vector = build_ttf(v_ascender, v_descender, &v_glyphs, v_hint_ppem, &v_gsub, &v_palette, v_scale, v_meta.ascent());
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
    let ascender = (meta.ascent() as f32 * scale).round() as i16;
    let descender = -((meta.descent() as f32 * scale).round() as i16);
    let hint_ppem = if UNITS_PER_EM.is_multiple_of(meta.height()) { meta.height() } else { 0 };

    let mut gid_to_name: HashMap<u16, String> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for (i, g) in glyph_data.iter().enumerate() {
        if seen.insert(g.name.clone()) {
            gid_to_name.insert((i + 1) as u16, g.name.clone());
        }
    }

    let ttf = build_ttf(ascender, descender, &glyph_data, hint_ppem, &gsub_data, &palette, scale, meta.ascent());
    Some(FontWithGidMap { ttf, gid_to_name, height: meta.height() })
}

fn build_font_from_documents_inner(docs: &[&Document], bitmap: bool, contour_cache: Option<&mut ContourCache>) -> Option<Vec<u8>> {
    let (meta, scale, glyph_data, gsub_data, palette) = collect_glyph_data_cached(docs, bitmap, contour_cache)?;

    let ascender = (meta.ascent() as f32 * scale).round() as i16;
    let descender = -((meta.descent() as f32 * scale).round() as i16);

    let hint_ppem = if !bitmap && UNITS_PER_EM.is_multiple_of(meta.height()) {
        meta.height()
    } else {
        0
    };
    Some(build_ttf(ascender, descender, &glyph_data, hint_ppem, &gsub_data, &palette, scale, meta.ascent()))
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

#[cfg(test)]
#[path = "../ttf_tests/mod.rs"]
mod tests;
