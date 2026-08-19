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
//! [`crate::on_demand`] on `BitmapFill` for what a synthesized shape has to
//! decide because of that.
//!
//! # `desync`: a grid the vector build does not read
//!
//! Normally the two builds draw the same shapes and differ only in how finely:
//! the grid is the drawing, and the bitmap build squares off whatever the
//! geometry says. A glyph flagged `desync` breaks that tie deliberately — its
//! own pixel grid is ink for the bitmap build and geometry for nobody, so the
//! vector build resolves it from its `ref`s alone. Together with a ref to an
//! on-demand `:zero` shape, which is the mirror case (geometry that lights no
//! pixel), the two faces become independent drawings of one glyph.
//!
//! The grid still declares the glyph's *dimensions* in both builds
//! (`glyph_cache::resolve_pending` re-applies them over whatever the composite
//! measured), so suppressing an outline never moves an advance. Every place
//! that reads own pixels for an outline has to honour the flag — `collect`'s
//! seed and composite closures and its COLR layer pass — because a grid that
//! slips back in produces a font that builds cleanly and draws the wrong thing
//! at one size only. `render/sample.rs` makes the same split for the same
//! reason: its small glyphs are the bitmap face and its scaled ones the vector
//! face.
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

use read_fonts::tables::glyf::Anchor;
use read_fonts::tables::glyf::CurvePoint;
use write_fonts::FontBuilder;
use write_fonts::tables::cmap::Cmap;
use write_fonts::tables::colr::{BaseGlyph, Colr, Layer as ColrLayer};
use write_fonts::tables::cpal::{ColorRecord, Cpal};
use write_fonts::tables::gasp::{Gasp, GaspRange, GaspRangeBehavior};
use write_fonts::tables::gdef::{Gdef, MarkGlyphSets};
use write_fonts::tables::glyf::{
    Bbox, Component, ComponentFlags, CompositeGlyph, Contour, GlyfLocaBuilder, Glyph, SimpleGlyph,
};
use write_fonts::tables::gpos::{
    AnchorTable, BaseArray, BaseRecord, Gpos, Mark2Array, Mark2Record, MarkArray,
    MarkBasePosFormat1, MarkMarkPosFormat1, MarkRecord, PositionLookup, PositionLookupList,
};
use write_fonts::tables::gsub::{
    Gsub, Ligature, LigatureSet, LigatureSubstFormat1, MultipleSubstFormat1,
    ReverseChainSingleSubstFormat1, Sequence, SingleSubst, SingleSubstFormat2,
    SubstitutionChainContext, SubstitutionLookup,
};
use write_fonts::tables::head::{Flags, Head, MacStyle};
use write_fonts::tables::hhea::Hhea;
use write_fonts::tables::hmtx::{Hmtx, LongMetric};
use write_fonts::tables::layout::{
    ChainedSequenceContext, ChainedSequenceContextFormat3, ClassDef, ClassDefFormat2,
    ClassRangeRecord, CoverageTable, Feature, FeatureList, FeatureRecord, LangSys, LangSysRecord,
    Lookup, LookupFlag, LookupList, Script, ScriptList, ScriptRecord, SequenceLookupRecord,
};
use write_fonts::tables::maxp::Maxp;
use write_fonts::tables::name::{Name, NameRecord};
use write_fonts::tables::os2::{Os2, SelectionFlags};
use write_fonts::tables::post::Post;

use write_fonts::types::{Fixed, GlyphId, GlyphId16, LongDateTime, NameId, Tag};

use crate::document::*;
use crate::document_io;
use crate::issues::Severity;
use crate::meta::FontMeta;
use crate::pixel::{PX_ALMOSTFULL, PX_CUSTOM, PX_SUBPIXEL, PixelShape};
use crate::render::contour::{track_contour, track_contour_multi_at, track_contour_multi_diff_at};
use crate::render::glyph_cache::{
    build_alt_index as build_cached_alternatives, resolve_cached as resolve_cached_ref,
};
use crate::resolve::{Diagnostic, ItemRef};

mod collect;
mod collection;
mod color;
mod contours;
mod expand;
mod gpos;
mod gsub;
mod hints;
mod os2_ranges;
mod outlines;
mod tables;

pub use color::parse_hex_color;
pub use color::{
    ColorAliasMap, Rgba, collect_color_aliases, effective_visibility, resolve_fill_rgba,
};
pub use contours::ContourCache;
#[cfg(feature = "editor")]
pub use contours::{SharedContourCache, new_contour_cache};
pub(crate) use expand::{
    Expansion, UvsExpandError, decomposed_map_pairs, expand_documents, expand_documents_for,
    expand_map_codepoints, expand_map_pairs, expand_uvs_map_triples, glyph_name_exists,
    parse_map_char,
};
pub(crate) use gsub::{remap_rule_kind, shadowed_single_subst_rules};

#[cfg(any(feature = "editor", test))]
use collect::collect_glyph_data_cached;
#[cfg(feature = "editor")]
use collect::collect_glyph_data_with_shared;
use tables::build_ttf;

pub const UNITS_PER_EM: u16 = 1024;

/// The glyph TrueType reserves GID 0 for, drawn for any character the font does
/// not cover. `collect` puts it at the head of the collected glyphs — moving
/// the one the source draws there, or inserting an empty stand-in — so from
/// there on a glyph's GID is simply its index.
pub(crate) const NOTDEF: &str = ".notdef";

#[derive(Clone)]
struct CompositeRef {
    component_name: String,
    x_offset: i16,
    y_offset: i16,
}

#[derive(Clone)]
struct CollectedGlyph {
    name: String,
    /// Every character that reaches this glyph *in the face being built*, in
    /// order. One glyph is one glyph however many characters reach it: an entry
    /// per `(codepoint, glyph)` pair would store the outline once per character
    /// and, worse, would make the glyph order depend on the cmap — which is
    /// what stops two faces of a collection from sharing the glyph store.
    codepoints: Vec<u32>,
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

#[derive(Clone)]
struct CollectedColorLayer {
    contours: Vec<Vec<(i16, i16)>>,
    palette_index: u16,
}

#[derive(Clone)]
struct ExpandedRemap {
    /// The `remap` line this came from, for the one check that reports
    /// against a rule rather than building one — see
    /// [`gsub::shadowed_single_subst_rules`].
    origin: Option<ItemRef>,
    lookbehind: Vec<Vec<String>>,
    /// Each inner Vec is a sequence of input glyph positions (len > 1 = ligature).
    source: Vec<Vec<String>>,
    /// Each inner Vec is a sequence of output glyph positions.
    target: Vec<Vec<String>>,
    lookahead: Vec<Vec<String>>,
}

/// One `map BASE SELECTOR = GLYPH`, resolved to codepoints and a glyph name.
///
/// Feeds two unrelated outputs from one declaration: a cmap format 14 entry,
/// which is how every conforming shaper reads a variation sequence, and a GSUB
/// ligature rule, which is the fallback for one that does not. The pair is
/// stated once because the two must not be able to disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct UvsPair {
    base: u32,
    selector: u32,
    glyph: String,
}

/// The name of the synthesized glyph a variation selector is mapped to.
///
/// The `@` makes the name unwritable rather than merely unusual, so there is no
/// reserved-name rule to state or enforce: `@` is not in the glyph-name
/// character set, and a source that writes one has it expanded against the
/// enclosing [`expand_at_name`] base into some *other* name — or, with no base
/// to expand against, rejected as an invalid name. Either way a source cannot
/// land on this one and steal the glyph the fallback lookup is written against.
///
/// Nothing downstream minds, because the name never leaves the build: `post` is
/// version 3.0, which stores no glyph names at all.
fn vs_glyph_name(selector: u32) -> String {
    format!("@vs-{selector:04X}")
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
    /// Variation sequences this *face* states, for both cmap 14 and the
    /// fallback lookup. Filled from the face-expanded items, not from the raw
    /// documents, so a slice-qualified pair reaches only the faces that include
    /// it.
    uvs_pairs: Vec<UvsPair>,
    /// Every selector any slice mentions, ascending. Deliberately *not* limited
    /// to the face being built: the synthesized selector glyphs are appended in
    /// this order, and `collect::build_faces` needs the glyph order to be the
    /// same for every face of a collection.
    uvs_selectors: Vec<u32>,
}

/// A file that failed to parse, and why.
pub type ParseError = (std::path::PathBuf, String);
/// The bytes one document was parsed from.
pub type DocSource = (std::path::PathBuf, Vec<u8>);
/// A directory's `.unf` files as loaded: what parsed, what did not, and the
/// bytes behind what did.
pub type LoadedDir = (Vec<Document>, Vec<ParseError>, Vec<DocSource>);

/// The editor's build product for one face.
#[cfg(feature = "editor")]
pub struct BuiltFontPair {
    /// The bitmap face's TTF: hinted to the pixel grid.
    pub bitmap: Vec<u8>,
    /// The vector face's TTF.
    pub vector: Vec<u8>,
    /// Glyph name → GID in the vector face.
    pub name_to_gid: HashMap<String, u16>,
}

pub fn load_docs_from_directory_checked(dir: &Path) -> (Vec<Document>, Vec<ParseError>) {
    let (docs, errors, _) = load_docs_from_directory_with_sources(dir);
    (docs, errors)
}

/// The same load, keeping the bytes each document was parsed from.
///
/// The editor holds on to them: a directory snapshot is the only complete
/// picture of the font it has, and every consumer that would otherwise read
/// those files a second time (the search pane, opening a file into a pane) is
/// on a click's critical path. On a network volume — where this editor is
/// routinely used — one `stat` per file is already a visible stall, so those
/// consumers go to memory instead. The caller keeps the sources for exactly as
/// long as it keeps the documents; nothing else refreshes them.
///
/// The files are read concurrently. On a share each one costs a round trip that
/// dominates its transfer — measured at ~185 ms of fixed cost against ~6 MB/s,
/// so 44 files were 7.3 of the 18 seconds a cold start took (`startup.rs`) — and
/// overlapping those waits is the whole of the fix. The thread count is
/// therefore about latency in flight, not about cores.
pub fn load_docs_from_directory_with_sources(dir: &Path) -> LoadedDir {
    // Timed because this is the prime suspect for a slow start on a network
    // share; `startup` ignores everything after the first frame, so the
    // rebuilds that also come through here cost one atomic load.
    let scan_t0 = std::time::Instant::now();
    let entries = std::fs::read_dir(dir);
    crate::startup::record_dir_scan(dir, scan_t0.elapsed());
    let Ok(entries) = entries else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| document_io::is_source_file(p))
        .collect();
    // Sorted here rather than sorting the documents afterwards: the workers
    // write their results by index, so one sort fixes the order of all three
    // outputs — including `sources`, which nothing sorted before.
    paths.sort();

    /// What one file turned into. Kept per index so the merge below is a
    /// straight walk rather than a sort by path.
    enum Loaded {
        Parsed(Box<Document>, Vec<u8>),
        Failed(String),
    }

    let load_one = |path: &Path| -> Loaded {
        let read_t0 = std::time::Instant::now();
        let read = std::fs::read(path);
        let read_elapsed = read_t0.elapsed();
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(e) => return Loaded::Failed(format!("reading: {e}")),
        };
        let parse_t0 = std::time::Instant::now();
        let content = String::from_utf8_lossy(&bytes);
        let parsed = document_io::parse_document_from_str(&content, path.to_path_buf());
        crate::startup::record_file(path, bytes.len(), read_elapsed, parse_t0.elapsed());
        match parsed {
            Ok(doc) => Loaded::Parsed(Box::new(doc), bytes),
            Err(e) => Loaded::Failed(e.to_string()),
        }
    };

    // More threads than cores on purpose: a worker spends nearly all its time
    // waiting on the share, and the parse it does between waits is a fraction
    // of that. Capped so that a directory of hundreds of files does not open
    // hundreds of handles at once.
    let workers = paths.len().min(16);
    let mut loaded: Vec<Option<Loaded>> = Vec::new();
    if workers <= 1 {
        loaded.extend(paths.iter().map(|p| Some(load_one(p))));
    } else {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let paths = &paths;
        let load_one = &load_one;
        let chunks: Vec<Vec<(usize, Loaded)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(path) = paths.get(i) else { break };
                            out.push((i, load_one(path)));
                        }
                        out
                    })
                })
                .collect();
            // A worker that panics would poison nothing — its files simply have
            // no result, and the merge below reports them as failures rather
            // than bringing the whole load down with it.
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        loaded.resize_with(paths.len(), || None);
        for (i, item) in chunks.into_iter().flatten() {
            loaded[i] = Some(item);
        }
    }

    let mut docs = Vec::new();
    let mut errors = Vec::new();
    let mut sources = Vec::new();
    for (path, item) in paths.into_iter().zip(loaded) {
        match item {
            Some(Loaded::Parsed(doc, bytes)) => {
                docs.push(*doc);
                sources.push((path, bytes));
            }
            // A file that does not parse contributes no document, so nothing
            // can search or open it either; its bytes are not kept.
            Some(Loaded::Failed(msg)) => errors.push((path, msg)),
            None => errors.push((path, "reading: the loader thread died".to_string())),
        }
    }
    (docs, errors, sources)
}

#[cfg(any(feature = "editor", test))]
pub fn build_font_from_documents(docs: &[&Document]) -> Option<Vec<u8>> {
    build_font_from_documents_inner(docs, false, None)
}

#[cfg(feature = "editor")]
pub fn build_font_pair_cached(
    docs: &[&Document],
    shared_cache: &SharedContourCache,
) -> Option<BuiltFontPair> {
    build_font_pair_cached_for(
        docs,
        shared_cache,
        None,
        &crate::cancel::CancelToken::never(),
    )
}

/// The editor's font pair for one named face, or for the primary face when
/// `face_id` is `None` or names a face the source no longer declares — the
/// editor's selection outlives the edit that deletes the `face` line, and a
/// preview that goes blank until it is re-picked would be worse than one that
/// falls back.
#[cfg(feature = "editor")]
pub fn build_font_pair_cached_for(
    docs: &[&Document],
    shared_cache: &SharedContourCache,
    face_id: Option<&str>,
    cancel: &crate::cancel::CancelToken,
) -> Option<BuiltFontPair> {
    let faces = crate::faces::FaceSet::collect(docs);
    let face = face_id
        .and_then(|id| faces.faces.iter().find(|f| f.id == id))
        .unwrap_or_else(|| faces.primary());
    let shared = collect::compute_shared_font_input_for(docs, face, cancel)?;

    // The lock is held across both flavors, so a build that gives up here also
    // frees the cache for the build that replaces it. Bailing between
    // `begin_generation` and `evict_stale` is safe by construction: the
    // generation counter only ever moves forward and entries are pruned by the
    // next *completed* build, which refreshes everything it still reads. An
    // aborted build therefore leaves the cache holding more than it needs, and
    // nothing else.
    let mut cc = shared_cache.lock().unwrap();
    cc.begin_generation();

    // The two flavors keep separate caches (see `ContourCaches`), so there is
    // nothing left for the second to wait on: both collections trace at once.
    let (bitmap_cache, vector_cache) = cc.split();
    let (bitmap_data, vector_data) = std::thread::scope(|s| {
        let bh =
            s.spawn(|| collect_glyph_data_with_shared(&shared, true, Some(bitmap_cache), cancel));
        let vector_data =
            collect_glyph_data_with_shared(&shared, false, Some(vector_cache), cancel);
        (bh.join().unwrap(), vector_data)
    });
    let (bitmap_data, vector_data) = (bitmap_data?, vector_data?);

    cc.evict_stale();
    drop(cc);

    let (b_meta, _, b_glyphs, b_gsub, b_palette) = bitmap_data;
    let (v_meta, v_scale, v_glyphs, v_gsub, v_palette) = vector_data;

    let mut name_to_gid: HashMap<String, u16> = HashMap::new();
    for (i, g) in v_glyphs.iter().enumerate() {
        name_to_gid.entry(g.name.clone()).or_insert(i as u16);
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
        let bh = s.spawn(|| {
            build_ttf(
                b_ascender,
                b_descender,
                &b_glyphs,
                0,
                &b_gsub,
                &b_palette,
                b_scale,
                &b_meta,
            )
        });
        let vector = build_ttf(
            v_ascender,
            v_descender,
            &v_glyphs,
            v_hint_ppem,
            &v_gsub,
            &v_palette,
            v_scale,
            &v_meta,
        );
        let bitmap = bh.join().unwrap();
        (bitmap, vector)
    });

    Some(BuiltFontPair {
        bitmap,
        vector,
        name_to_gid,
    })
}

/// Build every declared face, in declaration order, as `(face id, TTF bytes)`.
///
/// A source declaring no `face` yields one entry with an empty id — the same
/// font `build_font_from_documents` returns, so the two paths cannot drift.
pub fn build_faces(docs: &[&Document]) -> Option<Vec<(String, Vec<u8>)>> {
    let faces = crate::faces::FaceSet::collect(docs);

    // The glyph store is built once, for a synthetic face that includes every
    // declared slice — so it is the union of what any face can reach, in an
    // order no single face's cmap decided. That is what lets the faces of a
    // collection share `glyf`, `loca`, `hmtx` and `maxp`: identical bytes are
    // stored once, and bytes are only identical if the glyph order is.
    let union_face = crate::faces::Face {
        id: String::new(),
        slices: faces.declared.keys().cloned().collect(),
        origin: None,
    };
    let never = crate::cancel::CancelToken::never();

    // The union is traced once and is the only tracing this build does: the
    // glyph set is face-independent (`expand_for` filters maps by slice, never
    // glyphs), so a per-face trace would reproduce it outline for outline. Each
    // face therefore only expands — for its own `meta`, its own GSUB and above
    // all its own cmap — and reads its glyphs out of the union store.
    //
    // They still run at once rather than one after another: the union's trace
    // dominates, and a face's expansion is free beside it.
    let collect_union = || {
        let expand = crate::startup::PerfStage::new("expand");
        let shared = collect::compute_shared_font_input_for(docs, &union_face, &never)?;
        drop(expand);
        let _collect = crate::startup::PerfStage::new("collect");
        collect::collect_glyph_data_with_shared(&shared, false, None, &never)
    };
    let collect_face = |face: &crate::faces::Face| {
        let _expand = crate::startup::PerfStage::new("expand face");
        collect::collect_face_cmap(docs, face, &never)
    };
    let (union_collected, per_face) = std::thread::scope(|scope| {
        let union = scope.spawn(collect_union);
        let per_face: Vec<_> = faces
            .faces
            .iter()
            .map(|face| scope.spawn(move || collect_face(face)))
            .collect();
        (
            union.join().unwrap(),
            per_face
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>(),
        )
    });
    // The palette comes from the union along with the glyphs, and has to: a
    // glyph's `color_layers` index into it, and those are the union's.
    let (_, _, union_glyphs, _, palette) = union_collected?;

    let mut out = Vec::new();
    for (face, collected) in faces.faces.iter().zip(per_face) {
        // Expanded for this face, for its own `meta`, its own GSUB, and above
        // all its own cmap. Only the *codepoints* come from it; the glyphs, and
        // therefore every glyph id, come from the shared store.
        let collect::FaceCmap {
            meta,
            scale,
            per_name,
            gsub_data,
        } = collected?;

        let glyphs: Vec<CollectedGlyph> = union_glyphs
            .iter()
            .map(|g| {
                let mut g = g.clone();
                g.codepoints = per_name.get(g.name.as_str()).cloned().unwrap_or_default();
                g
            })
            .collect();

        let ascender = (meta.ascent() as f32 * scale).round() as i16;
        let descender = -((meta.descent() as f32 * scale).round() as i16);
        let hint_ppem = if UNITS_PER_EM.is_multiple_of(meta.height()) {
            meta.height()
        } else {
            0
        };
        out.push((
            face.id.clone(),
            tables::build_ttf(
                ascender, descender, &glyphs, hint_ppem, &gsub_data, &palette, scale, &meta,
            ),
        ));
    }
    Some(out)
}

/// Assemble already-built faces into a TrueType Collection.
pub fn build_collection(fonts: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    collection::build_collection(fonts)
}

/// Build result containing the TTF bytes, GID→name map, and the pixel
/// em-height needed to convert font units back to pixel coordinates.
pub struct FontWithGidMap {
    pub ttf: Vec<u8>,
    pub gid_to_name: HashMap<u16, String>,
    pub height: u16,
}

/// Build the primary face and return the TTF bytes together with a GID→glyph-name
/// map and the pixel em-height.
///
/// Test-only: shipping code names the face it wants. The assertion runner used
/// to take the primary face's build as a parameter and then build the *same*
/// face again by name whenever a `for SLICE` assertion reached it; it now asks
/// for faces one by one through [`build_font_with_gid_map_for`], so no caller
/// outside the tests still means "just the primary one".
#[cfg(test)]
pub fn build_font_with_gid_map(docs: &[&Document]) -> Option<FontWithGidMap> {
    build_with_gid_map(collect_glyph_data_cached(docs, false, None)?)
}

/// The same, for one named face rather than the primary one. The glyph order is
/// that face's own — unlike [`build_faces`], nothing here is shared between
/// faces, so the returned GID→name map is only valid for these bytes.
pub fn build_font_with_gid_map_for(
    docs: &[&Document],
    face: &crate::faces::Face,
) -> Option<FontWithGidMap> {
    let never = crate::cancel::CancelToken::never();
    let shared = collect::compute_shared_font_input_for(docs, face, &never)?;
    build_with_gid_map(collect::collect_glyph_data_with_shared(
        &shared, false, None, &never,
    )?)
}

/// The same again, tracing contours through the editor's shared cache.
///
/// Contour tracing is ~90% of a face build, and the editor has already paid for
/// most of it: its own font build fills the very same cache with the vector
/// variant of every glyph. Running the assertions off it turns a full second of
/// rebuilding into a lookup.
///
/// Unlike [`build_font_pair_cached_for`] this neither begins a generation nor
/// evicts: a face built only to check an assertion must not age out the entries
/// the *displayed* font is built from, nor push its own into the set that
/// build's next eviction pass keeps. It only reads what is there and adds what
/// is missing.
#[cfg(feature = "editor")]
pub fn build_font_with_gid_map_for_cached(
    docs: &[&Document],
    face: &crate::faces::Face,
    shared_cache: &SharedContourCache,
) -> Option<FontWithGidMap> {
    let never = crate::cancel::CancelToken::never();
    let shared = collect::compute_shared_font_input_for(docs, face, &never)?;
    let data = {
        let mut cc = shared_cache.lock().unwrap();
        collect::collect_glyph_data_with_shared(&shared, false, Some(cc.vector()), &never)?
    };
    build_with_gid_map(data)
}

fn build_with_gid_map(
    (meta, scale, glyph_data, gsub_data, palette): CollectedFontData,
) -> Option<FontWithGidMap> {
    let ascender = (meta.ascent() as f32 * scale).round() as i16;
    let descender = -((meta.descent() as f32 * scale).round() as i16);
    let hint_ppem = if UNITS_PER_EM.is_multiple_of(meta.height()) {
        meta.height()
    } else {
        0
    };

    let mut gid_to_name: HashMap<u16, String> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for (i, g) in glyph_data.iter().enumerate() {
        if seen.insert(g.name.clone()) {
            gid_to_name.insert(i as u16, g.name.clone());
        }
    }

    let ttf = build_ttf(
        ascender,
        descender,
        &glyph_data,
        hint_ppem,
        &gsub_data,
        &palette,
        scale,
        &meta,
    );
    Some(FontWithGidMap {
        ttf,
        gid_to_name,
        height: meta.height(),
    })
}

#[cfg(any(feature = "editor", test))]
fn build_font_from_documents_inner(
    docs: &[&Document],
    bitmap: bool,
    contour_cache: Option<&mut ContourCache>,
) -> Option<Vec<u8>> {
    let (meta, scale, glyph_data, gsub_data, palette) =
        collect_glyph_data_cached(docs, bitmap, contour_cache)?;

    let ascender = (meta.ascent() as f32 * scale).round() as i16;
    let descender = -((meta.descent() as f32 * scale).round() as i16);

    let hint_ppem = if !bitmap && UNITS_PER_EM.is_multiple_of(meta.height()) {
        meta.height()
    } else {
        0
    };
    Some(build_ttf(
        ascender,
        descender,
        &glyph_data,
        hint_ppem,
        &gsub_data,
        &palette,
        scale,
        &meta,
    ))
}

/// Everything needed to assemble the font tables for one build flavor:
/// font metadata, pixel→unit scale, glyphs, GSUB inputs, and color palette.
type CollectedFontData = (FontMeta, f32, Vec<CollectedGlyph>, GsubData, Vec<Rgba>);

/// Resolve all documents' glyph items (expanding name patterns, following
/// refs, tracking contours) into the flat, codepoint-sorted glyph list that
/// [`build_font_from_documents`] then hands to [`build_ttf`]. Split out so
/// tests can inspect the intermediate, pre-TTF-encoding representation
/// directly (e.g. to canonicalize away the non-deterministic contour
/// point/rotation order that `track_contour` can produce — see
/// `tests::ttf_build_digest_real_files_is_stable`).
#[cfg(test)]
fn collect_glyph_data(docs: &[&Document], bitmap: bool) -> Option<CollectedFontData> {
    collect_glyph_data_cached(docs, bitmap, None)
}

#[cfg(test)]
#[path = "../ttf_tests/mod.rs"]
mod tests;
