//! The specimen panel: every mapped character and every remap-only glyph of the
//! built font, rasterized through the shared glyph cache.
//!
//! It draws the *built font bytes* against names the background pipeline
//! resolved, so its cached cell list is keyed on the generations of those two
//! background **results** — never on the build request, which is bumped the
//! moment a document changes. `SpecimenState::cached_gen` carries the whole
//! story; get that key wrong and a remap cell draws a stale gid against fresh
//! font bytes, i.e. simply the wrong glyph.
//!
//! # Three caches, one behind the other
//!
//! What the grid shows is built in three steps, each keyed on strictly more than
//! the last, because each is invalidated by something different:
//!
//! 1. **What the source says** — the cmap pairs, the variation sequences, the
//!    remap-only glyphs, the `prop` lines, the blocks and the
//!    `exclude-from-sample` set. Keyed on the
//!    two background generations ([`SpecimenState::cached_gen`]). This is the
//!    step that reads the source the way the build does rather than as written:
//!    an `exists` above a `map` unrolls it once per matched name, so a source
//!    that states its han characters as one search maps them all through a
//!    single line. Resolving those searches (and the merges the aliases rest on)
//!    is what this step costs — a few tens of milliseconds for a font this size,
//!    paid once per background result rather than per frame, which is why it is
//!    a step of its own and not part of the layout below.
//! 2. **Which cells exist, in which sections** — [`SpecimenState::rebuild_sections`].
//!    Keyed additionally on [`SpecimenOptions`], since "show undeclared
//!    characters" fills every block that has a mapped character out to its whole
//!    range: a few hundred cells become a few hundred thousand, which is not
//!    work for a frame. The heading's coverage fraction walks the same ranges
//!    either way — counting a block's characters costs one pass, where giving
//!    each of them a cell costs a `CharEntry`.
//! 3. **Which row each cell is on** — [`GridLayout`]. Keyed additionally on the
//!    column count, since a row is only hidden when *every* character on it is
//!    excluded from the sample, and that depends on where the row happens to
//!    break.
//!
//! Row hiding is what makes step 2 affordable to look at: a source that excludes
//! U+AC00..D7A3 says the 11,172 Hangul syllables are not worth 700 rows of
//! screen, and hiding by the *displayed* row rather than per cell keeps the grid
//! rectangular instead of leaving ragged holes in it. A run of hidden rows
//! leaves an ellipsis row behind, so an excluded range still reads differently
//! from a range the source never mentions.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use crate::document::{
    Document, DocumentItem, NamePartsMap, expand_name_element, substitute_name_parts,
};
use crate::editor::doc_links::LinkTargetKind;
use crate::glyph_flags::{GlyphFlag, GlyphFlags};
use crate::preview::rasterizer::GlyphCache;
use crate::render::ttf_builder::{
    decomposed_map_pairs, expand_map_pairs_per_alternative, expand_uvs_map_triples,
};
use crate::resolve::ItemRef;
use crate::ucd::{BlockMap, format_block_range, variation_selector_label};

/// One variation-sequence cell — a `map BASE SELECTOR = GLYPH`.
///
/// It sits immediately after the cell of its own base character, in selector
/// order, because that is where someone looking for it looks: a variant is read
/// against the character it varies, not as a section of its own. The base
/// always gets a cell even when nothing `map`s it on its own, so a sequence is
/// never listed with nothing to vary from.
struct UvsEntry {
    base: u32,
    selector: u32,
    glyph_name: String,
    /// See [`CharEntry::unresolved`].
    unresolved: bool,
}

struct RemapEntry {
    label: String,
    glyph_name: String,
    feature: String,
    gid: u16,
    cp_sequence: Option<Vec<u32>>,
}

pub struct SpecimenClick {
    pub name: String,
    pub kind: LinkTargetKind,
}

/// Which characters the specimen lists and what it draws on each cell — the
/// three toggles of the grid's context menu.
///
/// Plain `Copy` fields, and the whole struct is the cache key of everything
/// derived from it, so it round-trips through the settings as one value; see
/// `app/settings.rs`. `serde(default)` is what lets a toggle be added here
/// without a saved file from an older build failing to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SpecimenOptions {
    /// List every character of every block that has at least one mapped
    /// character, not just the mapped ones, so a hole in the coverage is a
    /// visible empty cell. Unassigned code points stay out regardless — a
    /// block's permanent holes are not holes in the font.
    pub show_undeclared: bool,
    /// Draw each cell's advance-by-ascent/descent crop marks.
    pub show_metric_marks: bool,
    /// Break the grid into one section per block, under a heading row.
    pub group_by_block: bool,
}

impl Default for SpecimenOptions {
    fn default() -> Self {
        Self {
            show_undeclared: false,
            // The headings are what makes a grid of a few thousand cells
            // navigable, so the grid opens grouped; the metric marks are a
            // detail to switch on for one look rather than the resting state.
            show_metric_marks: false,
            group_by_block: true,
        }
    }
}

/// One character cell of the grid.
struct CharEntry {
    cp: u32,
    /// The glyph the source maps `cp` to — `None` for a character the source
    /// declares nothing about, which only [`SpecimenOptions::show_undeclared`]
    /// puts on the grid. Such a cell draws no glyph (not even the UI font's,
    /// which would read as coverage the font does not have) and is not
    /// clickable, since there is nothing to jump to.
    glyph_name: Option<String>,
    /// The source maps this character, but the font has no glyph for it: none
    /// of the `map` line's alternatives named one. The cell is tinted like an
    /// error, because that is what it is — a character the font claims and
    /// cannot draw — and no [`crate::glyph_flags`] entry can say so, since a
    /// flag is per glyph *name* and the name here stands for nothing.
    unresolved: bool,
}

/// What one cell of a section draws.
#[derive(Clone, Copy)]
enum Item {
    Char(usize),
    /// Index into [`SpecimenState::uvs_entries`].
    Uvs(usize),
    Remap(usize),
}

/// A run of cells laid out on rows of its own, optionally under a heading.
struct Section {
    /// `None` only for the single unheaded section a grid with
    /// [`SpecimenOptions::group_by_block`] off consists of.
    heading: Option<String>,
    /// `(declared, total)` — how much of the block the source covers, drawn at
    /// the right end of the heading. `None` for a section that is not a block,
    /// so there is nothing to be a fraction of.
    coverage: Option<(usize, usize)>,
    /// Range into [`SpecimenState::items`].
    start: usize,
    len: usize,
}

/// One block's worth of cells on its way to becoming a [`Section`] — the
/// heading it would get, and what the grid draws under it.
struct Group {
    heading: Option<String>,
    coverage: Option<(usize, usize)>,
    cps: Vec<u32>,
}

/// Which cells sit on which row, for one column count. See the module docs for
/// why this is a cache of its own.
struct GridLayout {
    cols: usize,
    rows: Vec<Row>,
    /// `rows.len() + 1` y offsets from the grid origin, so a scroll clip rect
    /// turns into a row range by binary search even though a heading row is
    /// shorter than a cell row.
    row_y: Vec<f32>,
}

enum Row {
    /// Index into [`SpecimenState::sections`].
    Heading(usize),
    /// Range into [`SpecimenState::items`].
    Cells { start: usize, len: usize },
    /// One or more consecutive rows the exclusion rule hid, drawn as `…` so the
    /// grid says something was left out instead of quietly closing the gap.
    Ellipsis,
}

/// Cell size, the glyph size drawn in one, and the heights of the two rows that
/// hold no cells.
const CELL_W: f32 = 64.0;
const CELL_H: f32 = 80.0;
const HEADING_H: f32 = 24.0;
const ELLIPSIS_H: f32 = 18.0;
const PX_SIZE: f32 = 48.0;

const LABEL_COLOR: egui::Color32 = egui::Color32::from_gray(180);
const DIM_LABEL_COLOR: egui::Color32 = egui::Color32::from_gray(215);
/// How much of a glyph is left when its cell is dimmed. Only the UI-font
/// fallback needs it; the font's own glyph is never drawn in a dimmed cell,
/// there being nothing to draw.
const DIM_ALPHA: f32 = 0.35;
/// A heading row is drawn against the grid's own white background rather than
/// the app theme's, so its colors are fixed too.
const HEADING_BG: egui::Color32 = egui::Color32::from_gray(128);
const HEADING_FG: egui::Color32 = egui::Color32::WHITE;

/// The tint a cell gets when the report has something to say about the glyph it
/// draws — see [`crate::glyph_flags`], which is also where a composite inherits
/// its components' flags. Pale enough on the grid's white to leave the glyph
/// itself the thing being read; the hovered pair replaces the black a hovered
/// cell is otherwise drawn against, so hovering never hides the flag.
const WARNING_BG: egui::Color32 = egui::Color32::from_rgb(0xff, 0xf3, 0xbf);
const ERROR_BG: egui::Color32 = egui::Color32::from_rgb(0xff, 0xd5, 0xcc);
const WARNING_BG_HOVER: egui::Color32 = egui::Color32::from_rgb(0x46, 0x38, 0x00);
const ERROR_BG_HOVER: egui::Color32 = egui::Color32::from_rgb(0x4e, 0x11, 0x08);

/// The alternative one cell takes, and whether it had to settle for a name the
/// font has no glyph for. `None` for a mapping that is not there at all.
///
/// `per_alt[k][i]` is alternative `k`'s expansion at position `i`; the
/// alternatives all expand over the same characters, so the position is the
/// character. A cell that matches nothing keeps the *first* name written, which
/// is the one the author most likely meant and so the one worth showing in the
/// status bar and jumping to on a click — unless the line ends in the empty
/// target, which says a character that matched nothing is simply not in the
/// font. Then there is no cell: the grid shows what the build produced, and the
/// build dropped the mapping.
fn pick_target<T>(
    per_alt: &[Vec<T>],
    i: usize,
    name_of: impl Fn(&T) -> &String,
    usable: &impl Fn(&str) -> bool,
    optional: bool,
) -> Option<(String, bool)> {
    let mut first = None;
    for alt in per_alt {
        let Some(entry) = alt.get(i) else {
            continue;
        };
        let name = name_of(entry);
        if name.is_empty() {
            continue;
        }
        if usable(name) {
            return Some((name.clone(), false));
        }
        first.get_or_insert_with(|| name.clone());
    }
    if optional {
        return None;
    }
    Some((first.unwrap_or_default(), true))
}

fn flag_bg(flag: GlyphFlag, is_hovered: bool) -> egui::Color32 {
    match (flag, is_hovered) {
        (GlyphFlag::Warning, false) => WARNING_BG,
        (GlyphFlag::Error, false) => ERROR_BG,
        (GlyphFlag::Warning, true) => WARNING_BG_HOVER,
        (GlyphFlag::Error, true) => ERROR_BG_HOVER,
    }
}

impl GridLayout {
    fn total_height(&self) -> f32 {
        self.row_y.last().copied().unwrap_or(0.0)
    }

    /// The top of row `idx`, or the grid's full height for `rows.len()`.
    fn row_top(&self, idx: usize) -> f32 {
        self.row_y
            .get(idx)
            .copied()
            .unwrap_or_else(|| self.total_height())
    }

    /// The row containing `y`, an offset from the grid origin.
    fn row_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        let idx = self.row_y.partition_point(|ry| *ry <= y).checked_sub(1)?;
        (idx < self.rows.len()).then_some(idx)
    }

    /// The rows overlapping the vertical band `top..bottom`, as offsets from the
    /// grid origin. A heading row is shorter than a cell row, so this is a
    /// binary search rather than a division.
    fn visible_rows(&self, top: f32, bottom: f32) -> std::ops::Range<usize> {
        let first = self
            .row_y
            .partition_point(|y| *y <= top)
            .saturating_sub(1)
            .min(self.rows.len());
        let last = self
            .row_y
            .partition_point(|y| *y < bottom)
            .min(self.rows.len());
        first..last.max(first)
    }
}

pub struct SpecimenState {
    pub options: SpecimenOptions,
    entries: Vec<CharEntry>,
    uvs_entries: Vec<UvsEntry>,
    remap_entries: Vec<RemapEntry>,
    /// Every cell of the grid in drawing order; the sections index into it.
    items: Vec<Item>,
    sections: Vec<Section>,
    /// The options `items`/`sections` were built for, `None` when step 1 landed
    /// something new and they have to be rebuilt.
    sections_key: Option<SpecimenOptions>,
    layout: Option<GridLayout>,
    /// Every character the source maps, and the glyph it maps to. Kept beside
    /// `entries` because filling a block asks it per code point, and because a
    /// change of options must not have to re-read the documents.
    /// Per character, the glyph the source maps it to and whether the font
    /// actually has that glyph — see [`CharEntry::unresolved`].
    declared: BTreeMap<u32, (String, bool)>,
    /// The variation sequences the source declares: base character, then
    /// selector, to the glyph each pair maps to. Two nested maps rather than a
    /// list, because a cell's place on the grid is *beside its base*, in
    /// selector order.
    uvs: BTreeMap<u32, BTreeMap<u32, (String, bool)>>,
    /// Which block every code point falls in, `prop block` claims included.
    blocks: BlockMap,
    /// The `exclude-from-sample` code points — the rows a filled grid drops.
    excluded: BTreeSet<u32>,
    /// `(font_data_gen, derived_gen)` — the generations of the *two* background
    /// results the rebuild reads, never the generation of the build *request*.
    /// A remap-only glyph is listed only if `name_to_gid` knows its (name-part
    /// expanded) name, so a rebuild keyed on the request would drop nearly all
    /// of them whenever the specimen is opened while a build is in flight — or
    /// at startup, where `name_parts` is empty until the first derive lands —
    /// and would then never run again to fix it.
    cached_gen: Option<(u64, u64)>,
    glyph_cache: GlyphCache,
    /// The `prop` lines of the source, as of the last rebuild. The hover status
    /// names a character through these, so a Private Use character the source
    /// named reads as that name here too — one rebuild behind an edit, like
    /// every other thing the specimen shows.
    char_props: crate::ucd::CharProps,
    /// Which glyphs the last derive's report faults, as of the same derive as
    /// everything else here. Read per cell while painting, so it is held rather
    /// than borrowed for the frame.
    glyph_flags: GlyphFlags,
    pub hover_status: Option<String>,
    /// What steps 2 and 3 cost in the frame that last re-ran them, for the
    /// rebuild report; `None` in a frame that reused them. Read and cleared by
    /// the panel that drew this.
    pub relayout_took: Option<std::time::Duration>,
}

impl SpecimenState {
    pub fn new() -> Self {
        Self {
            options: SpecimenOptions::default(),
            entries: Vec::new(),
            uvs_entries: Vec::new(),
            remap_entries: Vec::new(),
            items: Vec::new(),
            sections: Vec::new(),
            sections_key: None,
            layout: None,
            declared: BTreeMap::new(),
            uvs: BTreeMap::new(),
            blocks: BlockMap::default(),
            excluded: BTreeSet::new(),
            cached_gen: None,
            glyph_cache: GlyphCache::new(),
            char_props: crate::ucd::CharProps::default(),
            glyph_flags: GlyphFlags::default(),
            hover_status: None,
            relayout_took: None,
        }
    }

    pub fn needs_rebuild(&self, font_data_gen: u64, derived_gen: u64) -> bool {
        self.cached_gen != Some((font_data_gen, derived_gen))
    }

    /// Install what [`SpecimenData::collect`] found, for the two generations it
    /// was collected from. Steps 2 and 3 (see the module docs) rest on it, so
    /// both are invalidated.
    pub fn apply(&mut self, data: SpecimenData, font_data_gen: u64, derived_gen: u64) {
        self.cached_gen = Some((font_data_gen, derived_gen));
        self.sections_key = None;
        self.layout = None;
        self.declared = data.declared;
        self.uvs = data.uvs;
        self.remap_entries = data.remap_entries;
        self.blocks = data.blocks;
        self.excluded = data.excluded;
        self.char_props = data.char_props;
        self.glyph_flags = data.glyph_flags;
    }

    /// Collect and install in one go, for a caller with no background pipeline
    /// to collect on — which is the tests, and only the tests: the editor
    /// cannot afford this on the thread it draws with.
    #[cfg(test)]
    #[expect(clippy::too_many_arguments)]
    pub fn rebuild_if_needed(
        &mut self,
        docs: &[&Document],
        name_parts: &NamePartsMap,
        name_to_gid: &HashMap<String, u16>,
        face_id: Option<&str>,
        glyph_flags: &GlyphFlags,
        font_data_gen: u64,
        derived_gen: u64,
    ) {
        if !self.needs_rebuild(font_data_gen, derived_gen) {
            return;
        }
        let (exists, _) = crate::exists::resolve_scopes(docs, name_parts);
        let aliases = crate::alias::AliasMap::collect_with_merges(docs, name_parts, &exists);
        let data = SpecimenData::collect(
            docs,
            name_parts,
            &exists,
            &aliases,
            name_to_gid,
            face_id,
            glyph_flags,
        );
        self.apply(data, font_data_gen, derived_gen);
    }
}

/// What the specimen reads out of the documents: step 1 of its three, and the
/// only one that has to look at them at all.
///
/// Split off [`SpecimenState`] because it is a third full expansion of the
/// document set — the same order of work as the font build, and on a slow
/// machine over a second of it — and it used to run on the UI thread, where the
/// editor simply stopped for as long as it took. It depends on nothing the UI
/// knows: the options, the column count and the layout are steps 2 and 3, which
/// read only what this leaves behind. So the rebuild collects it beside
/// everything else and hands it over; see [`crate::app`]'s background pipeline.
pub struct SpecimenData {
    declared: BTreeMap<u32, (String, bool)>,
    uvs: BTreeMap<u32, BTreeMap<u32, (String, bool)>>,
    remap_entries: Vec<RemapEntry>,
    blocks: BlockMap,
    excluded: BTreeSet<u32>,
    char_props: crate::ucd::CharProps,
    glyph_flags: GlyphFlags,
}

impl SpecimenData {
    /// `exists` and `aliases` come from the caller because it already has them:
    /// the rebuild that collects this expanded the very same documents a moment
    /// earlier, and both cost as much as a fifth of an expansion to derive. A
    /// scope is keyed by *item index*, so they may only be handed in by a
    /// caller holding the same document set they were resolved over — which is
    /// the whole point of collecting this inside the rebuild rather than after
    /// it.
    pub fn collect(
        docs: &[&Document],
        name_parts: &NamePartsMap,
        exists: &crate::exists::ExistsScopes,
        aliases: &crate::alias::AliasMap,
        name_to_gid: &HashMap<String, u16>,
        face_id: Option<&str>,
        glyph_flags: &GlyphFlags,
    ) -> Self {
        let glyph_flags = glyph_flags.clone();
        let char_props = crate::ucd::CharProps::collect(docs);
        let blocks = BlockMap::collect(docs);
        let excluded = crate::document::excluded_from_sample(docs.iter().flat_map(|d| &d.items));

        // An `exists` above a line binds `$0`/`$N` over the names the source
        // declares and unrolls the line below it once per matched name. A
        // source that maps a few thousand han glyphs that way — `exists
        // han-([0-9a-f]{4,5})` / `map U+($1) = han-($1)` — has no literal `map`
        // line for any of them, so a pass that reads the items as written finds
        // no character at all. Hence `exists`, and hence `aliases` beside it:
        // `name_to_gid` comes from the built font, which knows a glyph only by
        // its canonical name, so a character mapped through an alias has to be
        // asked for under that name. Merged names count as aliases here for the
        // same reason they do in the expansion: the font carries one of them.

        // The specimen draws one face's font bytes, so it has to read the
        // source the way `expand_for` did when building them: a slice-qualified
        // line is stated once per slice the face includes, with *that slice's*
        // name parts in force. Substituting a `map wide|narrow : … = f($-half)`
        // with the unqualified parts leaves `$-half` in the glyph name, which
        // then names no glyph — no gid to draw, and nothing for a click to
        // link to.
        let faces = crate::faces::FaceSet::collect(docs);
        let face = face_id
            .and_then(|id| faces.faces.iter().find(|f| f.id == id))
            .unwrap_or_else(|| faces.primary());
        let scoped = crate::document::SliceNameParts::with_base(docs, name_parts.clone());

        // The oracle the ordered alternatives of a `map` line are asked
        // against, and the one the grid's error tint rests on: the *built
        // font*'s own glyph set. It answers the same question the build asked
        // (`resolve_map_alternatives`) without re-deriving it, and it is the
        // only honest answer for a cell — a name with no glyph id is a
        // character the font cannot draw, whatever the source says.
        //
        // A build that produced nothing at all leaves it empty; treating that
        // as "no glyph exists" would tint the entire grid red over a transient
        // state, so nothing is faulted until there is a font to fault against.
        let have_font = !name_to_gid.is_empty();
        let usable =
            |name: &str| !name.is_empty() && (!have_font || name_to_gid.contains_key(name));

        let mut map: BTreeMap<u32, (String, bool)> = BTreeMap::new();
        let mut uvs: BTreeMap<u32, BTreeMap<u32, (String, bool)>> = BTreeMap::new();
        // Only the names a `remap` targets are ever asked of `mapped_glyphs`
        // below, so only those are worth remembering: a `map` line with ordered
        // alternatives over a range names hundreds of thousands of glyphs, and
        // keeping every one of them was most of what this walk cost.
        let remap_targets: HashSet<String> = docs
            .iter()
            .flat_map(|d| &d.items)
            .filter_map(|item| match item {
                DocumentItem::Remap { target, .. } => Some(target),
                _ => None,
            })
            .flatten()
            .flat_map(|t| expand_name_element(t, name_parts))
            .collect();
        let mut mapped_glyphs: HashSet<String> = HashSet::new();
        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let origin = ItemRef::new(doc_idx, item_idx);
                // The `exists` line itself states nothing of its own, and a
                // search that found nothing leaves the line below it standing
                // for nothing.
                if exists.is_directive(origin) {
                    continue;
                }
                let scope = exists.scope(origin);
                if scope.is_some_and(|s| s.matches.is_empty()) {
                    continue;
                }
                let mut slices: Vec<Option<&str>> = match item.slice_qualifier() {
                    [] => vec![None],
                    qual => qual.iter().map(|s| Some(s.as_str())).collect(),
                };
                slices.retain(|s| face.includes(*s));
                for slice in slices {
                    let slice_parts = scoped.for_slice(slice);
                    // A scoped `map` unrolls to one mapping per match, with the
                    // code point *computed* from that match; an unscoped one is
                    // stated once, exactly as written. Same machinery as
                    // `expand_inner`.
                    // The base of the bindings is cloned once for the line,
                    // not once per match: a search over the han glyphs matches
                    // thousands of names, and the base is every `name-parts`
                    // the source declares.
                    let mut bound = scope.map(|_| slice_parts.clone());
                    for round in 0..scope.map_or(1, |s| s.matches.len()) {
                        let (parts, caps) = match (scope, &mut bound) {
                            (Some(scope), Some(bound)) => {
                                scope.rebind(bound, round);
                                (&*bound, Some(&scope.matches[round][..]))
                            }
                            _ => (slice_parts, None),
                        };
                        // A spelling `exists` cannot evaluate (`U+($9)` with no
                        // ninth group) fails every match the same way; it is
                        // reported by the build, and draws nothing here.
                        let evaluated = |spec: &str| match caps {
                            Some(caps) => crate::exists::eval_codepoint(spec, caps).ok(),
                            None => Some(spec.to_string()),
                        };
                        match item {
                            // A variation sequence gets a cell of its own, beside
                            // the base's. A malformed one (both halves varying, a
                            // selector that is not one) is left to `issues` to
                            // report and draws nothing here.
                            DocumentItem::Map {
                                char_repr,
                                selector: Some(selector),
                                glyphs,
                                ..
                            } => {
                                let (Some(char_repr), Some(selector)) =
                                    (evaluated(char_repr), evaluated(selector))
                                else {
                                    continue;
                                };
                                let per_alt: Vec<Vec<(u32, u32, String)>> = glyphs
                                    .iter()
                                    .map(|g| {
                                        let subst = substitute_name_parts(g, parts);
                                        let mut triples =
                                            expand_uvs_map_triples(&char_repr, &selector, &subst)
                                                .unwrap_or_default();
                                        for t in &mut triples {
                                            aliases.canonicalize(&mut t.2);
                                        }
                                        triples
                                    })
                                    .collect();
                                for t in per_alt.iter().flatten() {
                                    if remap_targets.contains(&t.2) {
                                        mapped_glyphs.insert(t.2.clone());
                                    }
                                }
                                let Some(first) = per_alt.first() else {
                                    continue;
                                };
                                let optional = glyphs.last().is_some_and(|g| g.is_empty());
                                for (i, &(base, sel, _)) in first.iter().enumerate() {
                                    let Some(target) =
                                        pick_target(&per_alt, i, |t| &t.2, &usable, optional)
                                    else {
                                        continue;
                                    };
                                    uvs.entry(base).or_default().entry(sel).or_insert(target);
                                }
                            }
                            DocumentItem::Map {
                                char_repr, glyphs, ..
                            } => {
                                let Some(char_repr) = evaluated(char_repr) else {
                                    continue;
                                };
                                // Expanded together rather than one alternative
                                // at a time: they range over the same
                                // characters, and a range line is thousands of
                                // them wide.
                                let substituted: Vec<String> = glyphs
                                    .iter()
                                    .map(|g| substitute_name_parts(g, parts))
                                    .collect();
                                let mut per_alt =
                                    expand_map_pairs_per_alternative(&char_repr, &substituted);
                                for alt in &mut per_alt {
                                    aliases.canonicalize_pairs(alt);
                                }
                                for p in per_alt.iter().flatten() {
                                    if remap_targets.contains(&p.1) {
                                        mapped_glyphs.insert(p.1.clone());
                                    }
                                }
                                let Some(first) = per_alt.first() else {
                                    continue;
                                };
                                let optional = glyphs.last().is_some_and(|g| g.is_empty());
                                for (i, &(cp, _)) in first.iter().enumerate() {
                                    if let std::collections::btree_map::Entry::Vacant(slot) =
                                        map.entry(cp)
                                        && let Some(target) =
                                            pick_target(&per_alt, i, |p| &p.1, &usable, optional)
                                    {
                                        slot.insert(target);
                                    }
                                }
                            }
                            DocumentItem::MapDecomposed {
                                char_repr, glyph, ..
                            } => {
                                let subst = glyph.as_ref().map(|g| substitute_name_parts(g, parts));
                                let Some(char_repr) = evaluated(char_repr) else {
                                    continue;
                                };
                                for (cp, name) in decomposed_map_pairs(&char_repr, subst.as_deref())
                                {
                                    if remap_targets.contains(&name) {
                                        mapped_glyphs.insert(name.clone());
                                    }
                                    let unresolved = !usable(&name);
                                    map.entry(cp).or_insert((name, unresolved));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        let declared = map;

        // Build reverse map: glyph_name → smallest codepoint.
        let mut glyph_to_cp: HashMap<&str, u32> = HashMap::new();
        for (cp, (glyph_name, _)) in &declared {
            glyph_to_cp.entry(glyph_name.as_str()).or_insert(*cp);
        }

        // Collect remap-only glyph names and their originating feature.
        let mut remap_only: BTreeSet<String> = BTreeSet::new();

        // Context-free remap rules (no lookbehind/lookahead) are eligible
        // for codepoint-sequence labels.
        struct RemapRule {
            source: Vec<String>,
            target: Vec<String>,
        }
        let mut ligature_rules: Vec<RemapRule> = Vec::new();
        // feature name for each remap-only glyph (first seen wins).
        let mut glyph_feature: HashMap<String, String> = HashMap::new();

        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Remap {
                    feature,
                    source,
                    target,
                    lookbehind,
                    lookahead,
                    ..
                } = item
                {
                    // `remap` takes no slice qualifier, so the unqualified name
                    // parts are the only ones in force here.
                    let tgt_expanded: Vec<Vec<String>> = target
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();
                    let src_expanded: Vec<Vec<String>> = source
                        .iter()
                        .map(|s| expand_name_element(s, name_parts))
                        .collect();

                    let max_len = src_expanded
                        .iter()
                        .chain(tgt_expanded.iter())
                        .map(|v| v.len())
                        .max()
                        .unwrap_or(1);

                    let has_context = !lookbehind.is_empty() || !lookahead.is_empty();

                    for i in 0..max_len {
                        let tgt: Vec<String> = tgt_expanded
                            .iter()
                            .map(|v| v[i % v.len()].clone())
                            .collect();
                        for name in &tgt {
                            if !mapped_glyphs.contains(name) {
                                remap_only.insert(name.clone());
                                glyph_feature
                                    .entry(name.clone())
                                    .or_insert_with(|| feature.clone());
                            }
                        }
                        if !has_context {
                            let src: Vec<String> = src_expanded
                                .iter()
                                .map(|v| v[i % v.len()].clone())
                                .collect();
                            ligature_rules.push(RemapRule {
                                source: src,
                                target: tgt,
                            });
                        }
                    }
                }
            }
        }

        // Build remap entries, trying to compute codepoint sequences.
        let mut with_cp: Vec<RemapEntry> = Vec::new();
        let mut without_cp: Vec<RemapEntry> = Vec::new();

        for glyph_name in remap_only {
            let Some(&gid) = name_to_gid.get(&glyph_name) else {
                continue;
            };
            let feature = glyph_feature.get(&glyph_name).cloned().unwrap_or_default();

            // Find a context-free remap rule where this glyph appears in
            // the target and all source glyphs have direct cmap mappings.
            let cp_seq = ligature_rules.iter().find_map(|rule| {
                if !rule.target.contains(&glyph_name) {
                    return None;
                }
                rule.source
                    .iter()
                    .map(|s| glyph_to_cp.get(s.as_str()).copied())
                    .collect::<Option<Vec<u32>>>()
            });

            let label = if let Some(ref cps) = cp_seq {
                let hex = cps
                    .iter()
                    .map(|cp| format!("{cp:04X}"))
                    .collect::<Vec<_>>()
                    .join("+");
                format!("{hex} ({glyph_name})")
            } else {
                glyph_name.clone()
            };

            let entry = RemapEntry {
                label,
                glyph_name,
                feature,
                gid,
                cp_sequence: cp_seq.clone(),
            };
            if cp_seq.is_some() {
                with_cp.push(entry);
            } else {
                without_cp.push(entry);
            }
        }

        // Sort ligature remaps by codepoint sequence, then append others
        // (already sorted by glyph name via BTreeSet).
        with_cp.sort_by(|a, b| a.cp_sequence.cmp(&b.cp_sequence));
        let mut remap_entries = with_cp;
        remap_entries.append(&mut without_cp);

        Self {
            declared,
            uvs,
            remap_entries,
            blocks,
            excluded,
            char_props,
            glyph_flags,
        }
    }
}

impl SpecimenState {
    /// Step 2: which cells the grid has, and how they are grouped. Reads only
    /// what step 1 left behind, so a change of options never re-reads the
    /// documents.
    fn rebuild_sections(&mut self) {
        self.sections_key = Some(self.options);
        self.layout = None;
        self.entries.clear();
        self.uvs_entries.clear();
        self.items.clear();
        self.sections.clear();

        // Group the mapped characters by block, in code point order. A code
        // point in no block at all — the UCD leaves gaps between them — goes
        // into one section at the end rather than having a range invented for
        // it, and is never filled.
        let mut by_block: BTreeMap<(u32, u32), (String, Vec<u32>)> = BTreeMap::new();
        let mut no_block: Vec<u32> = Vec::new();
        // A base that only variation sequences name still gets a cell — see
        // [`UvsEntry`] — so the grouping runs over both sets of code points.
        let cell_cps: BTreeSet<u32> = self
            .declared
            .keys()
            .chain(self.uvs.keys())
            .copied()
            .collect();
        for cp in cell_cps {
            match self.blocks.block_of(cp) {
                Some(b) => by_block
                    .entry((b.start, b.end))
                    .or_insert_with(|| (b.name.to_string(), Vec::new()))
                    .1
                    .push(cp),
                None => no_block.push(cp),
            }
        }

        let mut groups: Vec<Group> = Vec::new();
        for ((start, end), (name, cps)) in by_block {
            // The coverage is a count of *characters*, not of cells, so it
            // reads the same whether or not the grid is filled — and a block
            // with a glyph on a code point it has no character for states none
            // at all, the fraction being one that would read over 100%.
            let coverage = cps
                .iter()
                .all(|&cp| self.char_props.is_assigned(cp))
                .then(|| (cps.len(), self.block_total(start, end)));
            let cps = if self.options.show_undeclared {
                self.block_members(start, end).collect()
            } else {
                cps
            };
            let range = format_block_range(start, end);
            groups.push(Group {
                heading: Some(format!("{name}  {range}")),
                coverage,
                cps,
            });
        }
        if !no_block.is_empty() {
            groups.push(Group {
                heading: Some("No Block".to_string()),
                // A code point in no block has no range to be a fraction of.
                coverage: None,
                cps: no_block,
            });
        }

        let grouped = self.options.group_by_block;
        for Group {
            heading,
            coverage,
            cps,
        } in groups
        {
            let start = self.items.len();
            for cp in cps {
                let declared = self.declared.get(&cp).cloned();
                let unresolved = declared.as_ref().is_some_and(|(_, u)| *u);
                self.entries.push(CharEntry {
                    cp,
                    glyph_name: declared.map(|(n, _)| n),
                    unresolved,
                });
                self.items.push(Item::Char(self.entries.len() - 1));
                // `BTreeMap`, so the selectors come out in order.
                for (selector, (glyph_name, unresolved)) in
                    self.uvs.get(&cp).cloned().unwrap_or_default()
                {
                    self.uvs_entries.push(UvsEntry {
                        base: cp,
                        selector,
                        glyph_name,
                        unresolved,
                    });
                    self.items.push(Item::Uvs(self.uvs_entries.len() - 1));
                }
            }
            if grouped {
                let len = self.items.len() - start;
                self.sections.push(Section {
                    heading,
                    coverage,
                    start,
                    len,
                });
            }
        }

        // Remap-only glyphs come last: they have no code point to sort among
        // the blocks, so a grouped grid gives them a heading of their own.
        let remap_start = self.items.len();
        for i in 0..self.remap_entries.len() {
            self.items.push(Item::Remap(i));
        }
        if !grouped {
            self.sections.push(Section {
                heading: None,
                coverage: None,
                start: 0,
                len: self.items.len(),
            });
        } else if self.items.len() > remap_start {
            self.sections.push(Section {
                heading: Some("Remaps".to_string()),
                coverage: None,
                start: remap_start,
                len: self.items.len() - remap_start,
            });
        }
    }

    /// Every code point of one block, minus the ones a nested `prop block`
    /// claims — those belong to that block's own section.
    fn block_range(&self, start: u32, end: u32) -> impl Iterator<Item = u32> + '_ {
        // Both bounds, not just the start: a `prop block` claim at the very
        // beginning of a Private Use plane shares its start with the UCD block
        // it overrides, and comparing starts alone would then fill the claimed
        // code points into both sections.
        (start..=end).filter(move |&cp| {
            self.blocks
                .block_of(cp)
                .is_some_and(|b| (b.start, b.end) == (start, end))
        })
    }

    /// Every character of one block — the ones a filled grid gives a cell to:
    /// every one the source has ([`crate::ucd::CharProps::is_assigned`], which
    /// is the `prop` lines inside Private Use and the UCD outside it), plus the
    /// ones it draws. A block's permanent holes and its unassigned tail are not
    /// holes in the *font*, so they get no cell.
    fn block_members(&self, start: u32, end: u32) -> impl Iterator<Item = u32> + '_ {
        self.block_range(start, end).filter(move |&cp| {
            self.declared.contains_key(&cp)
                || self.uvs.contains_key(&cp)
                || self.char_props.is_assigned(cp)
        })
    }

    /// How many characters a block has — the denominator of the coverage its
    /// heading states, and the one thing there that is *not* a count of cells:
    /// a code point the source draws without stating is a cell but not a
    /// character.
    fn block_total(&self, start: u32, end: u32) -> usize {
        self.block_range(start, end)
            .filter(|&cp| self.char_props.is_assigned(cp))
            .count()
    }

    /// Step 3: which cells sit on which row, for `cols` columns.
    ///
    /// Every section contributes at least one row — a wholly hidden one is an
    /// ellipsis rather than nothing at all — so a grid with any cell in it never
    /// comes out zero-height.
    fn build_layout(&self, cols: usize) -> GridLayout {
        let mut rows: Vec<Row> = Vec::new();
        let mut row_y: Vec<f32> = vec![0.0];
        let mut y = 0.0_f32;
        for (si, sec) in self.sections.iter().enumerate() {
            let mut cell_rows: Vec<Row> = Vec::new();
            let mut i = sec.start;
            while i < sec.start + sec.len {
                let len = cols.min(sec.start + sec.len - i);
                if self.options.show_undeclared && self.row_is_all_excluded(i, len) {
                    // A run of hidden rows collapses to one ellipsis: dropping
                    // them silently makes an excluded range indistinguishable
                    // from a range the source never mentions.
                    if !matches!(cell_rows.last(), Some(Row::Ellipsis)) {
                        cell_rows.push(Row::Ellipsis);
                    }
                } else {
                    cell_rows.push(Row::Cells { start: i, len });
                }
                i += len;
            }
            if cell_rows.is_empty() {
                continue;
            }
            if sec.heading.is_some() {
                rows.push(Row::Heading(si));
                y += HEADING_H;
                row_y.push(y);
            }
            for row in cell_rows {
                y += match row {
                    Row::Ellipsis => ELLIPSIS_H,
                    _ => CELL_H,
                };
                rows.push(row);
                row_y.push(y);
            }
        }
        GridLayout { cols, rows, row_y }
    }

    /// Whether the vertical border *before* item `idx` separates a variation
    /// sequence from the cell it varies — its base, or an earlier sequence of
    /// the same base. Those borders are not drawn at all: the `n + 1` cells of
    /// one base are one open box, which reads as a run at a glance where a
    /// lighter or dashed rule of the same width does not.
    ///
    /// The question is asked of the item alone and not of the pair around it,
    /// so a run broken across two rows is left open at *both* ends of the
    /// break: the right edge of the row that fills up (the border before the
    /// item that did not fit) and the left edge of the row that continues it,
    /// which is what says the run goes on. `idx` past the last item — the
    /// right edge of the final row — is simply not a boundary.
    fn uvs_boundary(&self, idx: usize) -> bool {
        matches!(self.items.get(idx), Some(Item::Uvs(_)))
    }

    /// Whether every character on one row is excluded from the sample — the
    /// rule that hides the row. A remap cell is never excluded, so a row with
    /// one on it always stays.
    fn row_is_all_excluded(&self, start: usize, len: usize) -> bool {
        self.items[start..start + len]
            .iter()
            .all(|item| match item {
                Item::Char(i) => self.excluded.contains(&self.entries[*i].cp),
                // A variation sequence is excluded with its base: the two are
                // read together, so hiding one and not the other says nothing.
                Item::Uvs(i) => self.excluded.contains(&self.uvs_entries[*i].base),
                Item::Remap(_) => false,
            })
    }

    #[cfg(test)]
    fn glyph_for_cp(&self, cp: u32) -> Option<&str> {
        self.declared.get(&cp).map(|(n, _)| n.as_str())
    }

    /// The grid as `show` would lay it out at `cols` columns, one string per
    /// drawn row: `# HEADING` for a heading row, the cells' code points (or a
    /// remap glyph's name) for a cell row. A row the exclusion rule hid is
    /// simply not there.
    #[cfg(test)]
    fn row_summaries(&mut self, cols: usize) -> Vec<String> {
        if self.sections_key != Some(self.options) {
            self.rebuild_sections();
        }
        let layout = self.build_layout(cols);
        layout
            .rows
            .iter()
            .map(|row| match row {
                Row::Heading(si) => {
                    // `show` lays the coverage out at the right edge of the
                    // grid; two spaces is only this summary's way of saying so.
                    let sec = &self.sections[*si];
                    let cov = sec
                        .coverage
                        .map(|c| format!("  {}", format_coverage(c)))
                        .unwrap_or_default();
                    format!("# {}{cov}", sec.heading.as_deref().unwrap_or(""))
                }
                Row::Ellipsis => "\u{2026}".to_string(),
                Row::Cells { start, len } => self.items[*start..*start + *len]
                    .iter()
                    .map(|item| match item {
                        Item::Char(i) => format!("{:04X}", self.entries[*i].cp),
                        Item::Uvs(i) => uvs_label(&self.uvs_entries[*i]),
                        Item::Remap(ri) => self.remap_entries[*ri].glyph_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            })
            .collect()
    }

    /// Every character cell of the grid, in drawing order.
    #[cfg(test)]
    fn cell_cps(&mut self) -> Vec<u32> {
        if self.sections_key != Some(self.options) {
            self.rebuild_sections();
        }
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Char(i) => Some(self.entries[*i].cp),
                Item::Uvs(_) | Item::Remap(_) => None,
            })
            .collect()
    }

    #[cfg(test)]
    fn remap_glyph_names(&self) -> Vec<&str> {
        self.remap_entries
            .iter()
            .map(|e| e.glyph_name.as_str())
            .collect()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        font_data: Option<&(Vec<u8>, Vec<u8>)>,
        font_data_gen: u64,
    ) -> Option<SpecimenClick> {
        self.glyph_cache.invalidate_if_changed(font_data_gen);
        self.hover_status = None;

        // Steps 2 and 3, both on the UI thread and both re-run when step 1
        // lands something new — which is the frame after every rebuild that
        // collected it. Timed for the rebuild report, since a frame that does
        // them is the one frame an edit is visible in.
        let relayout = std::time::Instant::now();
        let mut relaid = false;
        if self.sections_key != Some(self.options) {
            self.rebuild_sections();
            relaid = true;
        }
        if self.items.is_empty() {
            ui.label("No cmap entries.");
            return None;
        }

        let mut clicked: Option<SpecimenClick> = None;
        let avail_width = ui.available_width();
        let cols = (avail_width / CELL_W).floor().max(1.0) as usize;
        if self.layout.as_ref().is_none_or(|l| l.cols != cols) {
            self.layout = Some(self.build_layout(cols));
            relaid = true;
        }
        self.relayout_took = relaid.then(|| relayout.elapsed());
        // Out of `self` for the frame: every draw call below wants `&mut self`
        // for the glyph cache, and the context menu wants `&mut self.options`.
        // Always `Some` — it was just filled in above.
        let layout = self.layout.take()?;
        let grid_width = cols as f32 * CELL_W;
        let total_height = layout.total_height();

        let label_font = crate::app::uniform_font_id(ui.ctx(), 16.0);
        let glyph_color = egui::Color32::BLACK;
        let bg_color = egui::Color32::WHITE;
        let border_stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);

        let raster_font = font_data.map(|p| &p.1);

        crate::editor::document_view::apply_scroll_physics(
            ui,
            1,
            egui::Id::new("specimen_scroll_accel"),
        );

        let hover_pointer = ui.input(|i| i.pointer.hover_pos());
        let ctrl_c = ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.modifiers.command && i.key_pressed(egui::Key::C))
        });

        egui::ScrollArea::vertical()
            .id_salt("specimen_scroll")
            .show(ui, |ui| {
                // Own the full width even though only `cols` boxes fit: the
                // painter's clip rect is the allocated area, and a hovered cell
                // deliberately overflows its neighbors, so a rect ending at
                // `grid_width` would cut the rightmost column's overflow off.
                // The slack to the right is filled with the same background.
                let (response, painter) = ui.allocate_painter(
                    egui::vec2(avail_width.max(grid_width), total_height),
                    egui::Sense::click(),
                );
                let origin = response.rect.min;

                let clip = painter.clip_rect();
                let visible = layout.visible_rows(clip.top() - origin.y, clip.bottom() - origin.y);

                let vis_rect = egui::Rect::from_min_max(
                    egui::pos2(origin.x, origin.y + layout.row_top(visible.start)),
                    egui::pos2(
                        response.rect.right(),
                        origin.y + layout.row_top(visible.end),
                    ),
                );
                painter.rect_filled(vis_rect, 0.0, bg_color);

                // A border stroke is centred on its line, so an outermost one
                // sitting exactly on the allocated rect's edge loses half its
                // width to the clip; inset those (and only those) inward.
                let half = border_stroke.width / 2.0;
                let clamp_x =
                    |x: f32| x.clamp(response.rect.left() + half, response.rect.right() - half);
                let clamp_y =
                    |y: f32| y.clamp(response.rect.top() + half, response.rect.bottom() - half);

                // `response.rect` is the *content* rect, which extends past the
                // scroll viewport on both sides once the grid is scrolled, so
                // it contains points that are over the editor above instead.
                // `contains_pointer` respects the clip rect and the layer
                // order, so the cell under the pointer is the one on screen.
                let cell_at = |pos: egui::Pos2| -> Option<(usize, egui::Pos2)> {
                    if !response.rect.contains(pos) {
                        return None;
                    }
                    let row_idx = layout.row_at(pos.y - origin.y)?;
                    let Row::Cells { start, len } = layout.rows[row_idx] else {
                        return None;
                    };
                    let col = (pos.x - origin.x) / CELL_W;
                    if col < 0.0 || col.floor() as usize >= len {
                        return None;
                    }
                    let col = col.floor() as usize;
                    Some((
                        start + col,
                        egui::pos2(
                            origin.x + col as f32 * CELL_W,
                            origin.y + layout.row_top(row_idx),
                        ),
                    ))
                };

                let hovered = hover_pointer
                    .filter(|_| response.contains_pointer())
                    .and_then(cell_at);

                let style = CellStyle {
                    px_size: PX_SIZE,
                    label_font: &label_font,
                    label_color: LABEL_COLOR,
                    dim_label_color: DIM_LABEL_COLOR,
                    show_metric_marks: self.options.show_metric_marks,
                    raster_font,
                    ctx: ui.ctx(),
                };

                for row_idx in visible.clone() {
                    let y0 = origin.y + layout.row_top(row_idx);
                    let y1 = origin.y + layout.row_top(row_idx + 1);
                    match layout.rows[row_idx] {
                        Row::Heading(section_idx) => {
                            let rect = egui::Rect::from_min_max(
                                egui::pos2(origin.x, y0),
                                egui::pos2(response.rect.right(), y1),
                            );
                            painter.rect_filled(rect, 0.0, HEADING_BG);
                            let Some(text) = self.sections[section_idx].heading.clone() else {
                                continue;
                            };
                            let galley =
                                painter.layout_no_wrap(text, label_font.clone(), HEADING_FG);
                            let ty = y0 + (HEADING_H - galley.size().y) / 2.0;
                            painter.galley(egui::pos2(origin.x + 4.0, ty), galley, HEADING_FG);
                            // The coverage sits at the far end of the grid, not
                            // of the allocated rect: the slack to the right of
                            // the last column is not part of the grid to read.
                            if let Some(cov) = self.sections[section_idx].coverage {
                                let galley = painter.layout_no_wrap(
                                    format_coverage(cov),
                                    label_font.clone(),
                                    HEADING_FG,
                                );
                                let x = origin.x + grid_width - 4.0 - galley.size().x;
                                let ty = y0 + (HEADING_H - galley.size().y) / 2.0;
                                painter.galley(egui::pos2(x, ty), galley, HEADING_FG);
                            }
                        }
                        Row::Ellipsis => {
                            let galley = painter.layout_no_wrap(
                                "\u{2026}".to_string(),
                                label_font.clone(),
                                LABEL_COLOR,
                            );
                            let ty = y0 + (ELLIPSIS_H - galley.size().y) / 2.0;
                            painter.galley(egui::pos2(origin.x + 4.0, ty), galley, LABEL_COLOR);
                        }
                        Row::Cells { start, len } => {
                            let ly0 = clamp_y(y0);
                            let ly1 = clamp_y(y1);
                            for col in 0..=len {
                                if self.uvs_boundary(start + col) {
                                    continue;
                                }
                                let x = clamp_x(origin.x + col as f32 * CELL_W);
                                painter.line_segment(
                                    [egui::pos2(x, ly0), egui::pos2(x, ly1)],
                                    border_stroke,
                                );
                            }
                            let x_end = clamp_x(origin.x + len as f32 * CELL_W);
                            for y in [ly0, ly1] {
                                painter.line_segment(
                                    [egui::pos2(clamp_x(origin.x), y), egui::pos2(x_end, y)],
                                    border_stroke,
                                );
                            }
                            for col in 0..len {
                                let idx = start + col;
                                // The hovered cell overflows its neighbors, so
                                // it is drawn after every other one.
                                if hovered.map(|(i, _)| i) == Some(idx) {
                                    continue;
                                }
                                let cell_min = egui::pos2(origin.x + col as f32 * CELL_W, y0);
                                self.draw_item(
                                    &painter,
                                    cell_min,
                                    self.items[idx],
                                    false,
                                    glyph_color,
                                    &style,
                                );
                            }
                        }
                    }
                }

                if let Some((idx, cell_min)) = hovered {
                    let item = self.items[idx];
                    // The hovered cell inverts, so a flag shows here as the
                    // dark end of its pair rather than as the pale tint the
                    // resting cells carry.
                    let hover_bg = match self.flag_for(item) {
                        Some(flag) => flag_bg(flag, true),
                        None => egui::Color32::BLACK,
                    };
                    painter.rect_filled(style.cell_rect(cell_min), 0.0, hover_bg);
                    if let Some(gr) = self.compute_glyph_rect(cell_min, item, &style) {
                        painter.rect_filled(gr.expand(4.0), 0.0, hover_bg);
                    }
                    // A remap or variation-sequence label is wider than a cell
                    // far more often than a `U+XXXX` one; give it a background
                    // of its own.
                    let wide_label = match item {
                        Item::Char(_) => None,
                        Item::Uvs(i) => Some(uvs_label(&self.uvs_entries[i])),
                        Item::Remap(ri) => Some(self.remap_entries[ri].label.clone()),
                    };
                    if let Some(text) = wide_label {
                        let label_galley =
                            painter.layout_no_wrap(text, label_font.clone(), LABEL_COLOR);
                        let lw = label_galley.size().x + 4.0;
                        if lw > CELL_W {
                            let label_bg = egui::Rect::from_min_size(
                                cell_min,
                                egui::vec2(lw, label_galley.size().y + 2.0),
                            );
                            painter.rect_filled(label_bg, 0.0, hover_bg);
                        }
                    }
                    self.draw_item(&painter, cell_min, item, true, egui::Color32::WHITE, &style);
                }

                if response.clicked()
                    && let Some(pos) = response.interact_pointer_pos()
                    && let Some((idx, _)) = cell_at(pos)
                {
                    clicked = match self.items[idx] {
                        // An undeclared character has nothing to jump to.
                        Item::Char(_) | Item::Uvs(_) => {
                            self.goto_target(self.items[idx]).map(|name| SpecimenClick {
                                name: name.to_string(),
                                kind: LinkTargetKind::Glyph,
                            })
                        }
                        // A remap cell jumps to the *feature*, which is what
                        // put the glyph on the grid; the glyph itself has no
                        // character to reach it by.
                        Item::Remap(ri) => Some(SpecimenClick {
                            name: self.remap_entries[ri].feature.clone(),
                            kind: LinkTargetKind::Remap,
                        }),
                    };
                }

                if let Some((idx, _)) = hovered {
                    self.hover_status = Some(self.status_for(self.items[idx]));
                    if ctrl_c && let Some(text) = self.copy_text(self.items[idx]) {
                        ui.ctx().copy_text(text);
                    }
                }

                response.context_menu(|ui| self.options_menu(ui));
            });

        self.layout = Some(layout);
        clicked
    }

    /// The grid's context menu. A toggle takes effect on the next frame, since
    /// the sections and the row layout are rebuilt at the top of `show`.
    ///
    /// Toggling closes the menu. A checkbox row normally stays open so several
    /// can be set at once, but this menu covers the grid it describes and there
    /// is no obvious empty space to click to dismiss it — one toggle, then out of
    /// the way, so the effect is visible.
    fn options_menu(&mut self, ui: &mut egui::Ui) {
        let toggled = ui
            .checkbox(
                &mut self.options.show_undeclared,
                "Show undeclared characters",
            )
            .clicked()
            | ui.checkbox(&mut self.options.show_metric_marks, "Show metric marks")
                .clicked()
            | ui.checkbox(&mut self.options.group_by_block, "Group by block")
                .clicked();
        if toggled {
            ui.close_menu();
        }
    }

    /// What the report says about the glyph one cell draws, if anything.
    ///
    /// A cell whose character maps to no glyph at all is an error of its own:
    /// see [`CharEntry::unresolved`] for why the flag map cannot carry it.
    fn flag_for(&self, item: Item) -> Option<GlyphFlag> {
        let unresolved = match item {
            Item::Char(i) => self.entries[i].unresolved,
            Item::Uvs(i) => self.uvs_entries[i].unresolved,
            Item::Remap(_) => false,
        };
        if unresolved {
            return Some(GlyphFlag::Error);
        }
        self.glyph_flags.get(self.glyph_of(item)?)
    }

    /// The glyph a cell draws, `None` for a character the source declares
    /// nothing about.
    fn glyph_of(&self, item: Item) -> Option<&str> {
        match item {
            Item::Char(i) => self.entries[i].glyph_name.as_deref(),
            Item::Uvs(i) => Some(self.uvs_entries[i].glyph_name.as_str()),
            Item::Remap(ri) => Some(self.remap_entries[ri].glyph_name.as_str()),
        }
    }

    /// Where a click on this cell lands: the glyph whose own line carries the
    /// fault, if any, and otherwise the glyph the cell draws.
    ///
    /// A cell tinted because something it is *built out of* is broken is a cell
    /// whose own declaration is fine — for a Han character that declaration is
    /// a pattern line covering a whole block, which is not a place anyone
    /// wants to be sent. See [`crate::glyph_flags`]. For a glyph faulted
    /// directly the two names are the same and this changes nothing.
    fn goto_target(&self, item: Item) -> Option<&str> {
        let name = self.glyph_of(item)?;
        Some(self.glyph_flags.source(name).unwrap_or(name))
    }

    /// The status-bar line for one cell, with what the report says about the
    /// glyph on the end of it. An inherited fault names the glyph it is really
    /// in, which is also where a click goes — see [`Self::goto_target`].
    fn status_for(&self, item: Item) -> String {
        let mut line = self.status_body(item);
        if let Some(name) = self.glyph_of(item)
            && let Some(flag) = self.glyph_flags.get(name)
        {
            let what = match flag {
                GlyphFlag::Warning => "warning",
                GlyphFlag::Error => "error",
            };
            line.push_str(&match self.glyph_flags.source(name) {
                Some(src) if src != name => format!(" \u{2014} {what} in '{src}'"),
                _ => format!(" \u{2014} {what}"),
            });
        }
        line
    }

    /// What Ctrl+C over one cell copies: the character, or the whole variation
    /// sequence — the two code points together are what a text field has to
    /// receive for the variant to show up in it.
    fn copy_text(&self, item: Item) -> Option<String> {
        match item {
            Item::Char(i) => char::from_u32(self.entries[i].cp).map(|c| c.to_string()),
            Item::Uvs(i) => {
                let entry = &self.uvs_entries[i];
                let text: String = [entry.base, entry.selector]
                    .iter()
                    .filter_map(|cp| char::from_u32(*cp))
                    .collect();
                (text.chars().count() == 2).then_some(text)
            }
            Item::Remap(_) => None,
        }
    }

    fn status_body(&self, item: Item) -> String {
        match item {
            Item::Char(i) => {
                let cp = self.entries[i].cp;
                let ch = char::from_u32(cp);
                let char_str = ch.map(|c| c.to_string()).unwrap_or_default();
                let char_name = self
                    .char_props
                    .name(cp)
                    .unwrap_or_else(|| "<unknown>".to_string());
                // Same brace group as the Ctrl+K popup, so one character reads
                // identically in either place.
                let props = ch
                    .map(|c| format!(" {}", self.char_props.property_summary(c)))
                    .unwrap_or_default();
                let tail = match &self.entries[i].glyph_name {
                    Some(name) => format!("({name})"),
                    None => "(undeclared)".to_string(),
                };
                format!("U+{cp:04X} {char_str} {char_name}{props} {tail}")
            }
            Item::Uvs(i) => {
                let entry = &self.uvs_entries[i];
                let base_name = self
                    .char_props
                    .name(entry.base)
                    .unwrap_or_else(|| "<unknown>".to_string());
                let text: String = [entry.base, entry.selector]
                    .iter()
                    .filter_map(|cp| char::from_u32(*cp))
                    .collect();
                format!(
                    "U+{:04X} U+{:04X} {} {} + {} ({})",
                    entry.base,
                    entry.selector,
                    text,
                    base_name,
                    selector_name(entry.selector),
                    entry.glyph_name
                )
            }
            Item::Remap(ri) => {
                let entry = &self.remap_entries[ri];
                match &entry.cp_sequence {
                    Some(cps) => {
                        let parts: Vec<String> = cps
                            .iter()
                            .map(|cp| {
                                let char_name = self
                                    .char_props
                                    .name(*cp)
                                    .unwrap_or_else(|| "<unknown>".to_string());
                                format!("U+{cp:04X} {char_name}")
                            })
                            .collect();
                        format!("{} ({})", parts.join(" + "), entry.glyph_name)
                    }
                    None => format!("{} (remap-only)", entry.glyph_name),
                }
            }
        }
    }

    fn draw_item(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        item: Item,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        // Painted under everything the cell draws. A hovered cell has already
        // had its (dark) background filled in by `show`, which owns the
        // overflow past the cell edge as well.
        if !is_hovered && let Some(flag) = self.flag_for(item) {
            // Inset by the border stroke, which is drawn before the cells and
            // is the grid the tint sits inside rather than over.
            painter.rect_filled(
                style.cell_rect(cell_min).shrink(1.0),
                0.0,
                flag_bg(flag, false),
            );
        }
        match item {
            Item::Char(i) => {
                let cp = self.entries[i].cp;
                let declared = self.entries[i].glyph_name.is_some();
                self.draw_cell(
                    painter,
                    cell_min,
                    cp,
                    declared,
                    is_hovered,
                    glyph_color,
                    style,
                );
            }
            Item::Uvs(i) => {
                self.draw_uvs_cell(painter, cell_min, i, is_hovered, glyph_color, style)
            }
            Item::Remap(ri) => {
                self.draw_remap_cell(painter, cell_min, ri, is_hovered, glyph_color, style)
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn draw_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        cp: u32,
        declared: bool,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let px_size = style.px_size;
        let ctx = style.ctx;

        let font = style.raster_font.and_then(|b| FontRef::new(b).ok());
        // Whether the *built font* has anything to show here, which is what
        // dims the cell. Before the first build there is no font to ask, so
        // what the source says stands in for it.
        let has_metrics = match &font {
            Some(font) => cp_has_metrics(font, cp),
            None => declared,
        };

        let hex = format!("{cp:04X}");
        let label_color = if has_metrics {
            style.label_color
        } else {
            style.dim_label_color
        };
        let label_galley = painter.layout_no_wrap(hex, style.label_font.clone(), label_color);
        painter.galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            label_color,
        );

        let Some(ch) = char::from_u32(cp) else { return };
        let center = style.cell_center(cell_min);

        let mut drawn_via_rasterizer = false;

        if let Some(font_bytes) = style.raster_font
            && let Some(font) = &font
            && let Some(gid) = font.charmap().map(ch)
        {
            drawn_via_rasterizer = self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                font,
                font_bytes,
                gid,
                is_hovered,
                glyph_color,
                style,
            );
        }

        // The fallback draws the character in the *editor's* UI font, which is
        // a reasonable stand-in for a glyph the build has not caught up with —
        // but for a character the source declares nothing about it would read
        // as coverage the font does not have, so an undeclared cell stays empty.
        // It is the font's glyph, not this one, that the cell claims to show,
        // so a dimmed cell dims it too.
        if !drawn_via_rasterizer && declared {
            let color = if has_metrics {
                glyph_color
            } else {
                glyph_color.gamma_multiply(DIM_ALPHA)
            };
            let glyph_font = crate::app::uniform_font_id(ctx, px_size);
            let glyph_galley = painter.layout_no_wrap(ch.to_string(), glyph_font, color);
            let glyph_size = glyph_galley.size();
            let pos = egui::pos2(center.0 - glyph_size.x / 2.0, center.1 - glyph_size.y / 2.0);
            cell_painter(painter, cell_rect, is_hovered).galley(pos, glyph_galley, color);
        }
    }

    /// A variation-sequence cell: the `+VS17` label, and whatever the built
    /// font's cmap format 14 maps the pair to.
    ///
    /// The glyph is looked up through the *font*, not through the source's
    /// glyph name, so the cell shows what a shaper would actually pick — a
    /// sequence the build dropped draws nothing and dims its label, exactly as
    /// a character whose glyph never made it does.
    fn draw_uvs_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        uvs_idx: usize,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let entry = &self.uvs_entries[uvs_idx];
        let label_text = uvs_label(entry);
        let (base, selector) = (entry.base, entry.selector);

        let font = style.raster_font.and_then(|b| FontRef::new(b).ok());
        let gid = font.as_ref().and_then(|f| variant_gid(f, base, selector));
        let label_color = if gid.is_some() {
            style.label_color
        } else {
            style.dim_label_color
        };
        let label_galley =
            painter.layout_no_wrap(label_text, style.label_font.clone(), label_color);
        cell_painter(painter, cell_rect, is_hovered).galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            label_color,
        );

        if let (Some(font_bytes), Some(font), Some(gid)) = (style.raster_font, &font, gid) {
            let center = style.cell_center(cell_min);
            self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                font,
                font_bytes,
                gid,
                is_hovered,
                glyph_color,
                style,
            );
        }
    }

    fn draw_remap_cell(
        &mut self,
        painter: &egui::Painter,
        cell_min: egui::Pos2,
        remap_idx: usize,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) {
        let cell_rect = style.cell_rect(cell_min);
        let entry = &self.remap_entries[remap_idx];
        let gid = entry.gid;
        let label_text = entry.label.clone();

        let label_galley =
            painter.layout_no_wrap(label_text, style.label_font.clone(), style.label_color);
        cell_painter(painter, cell_rect, is_hovered).galley(
            egui::pos2(cell_min.x + 2.0, cell_min.y + 1.0),
            label_galley,
            style.label_color,
        );

        if let Some(font_bytes) = style.raster_font
            && let Ok(font) = FontRef::new(font_bytes)
        {
            let center = style.cell_center(cell_min);
            self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                &font,
                font_bytes,
                skrifa::GlyphId::new(gid as u32),
                is_hovered,
                glyph_color,
                style,
            );
        }
    }

    /// Rasterizes `gid` and paints it centered on the cell baseline; returns
    /// false when the rasterizer produced nothing so the caller can fall back
    /// to text rendering.
    #[expect(clippy::too_many_arguments)]
    fn draw_rasterized_glyph(
        &mut self,
        painter: &egui::Painter,
        cell_rect: egui::Rect,
        center: (f32, f32),
        font: &FontRef,
        font_bytes: &[u8],
        gid: skrifa::GlyphId,
        is_hovered: bool,
        glyph_color: egui::Color32,
        style: &CellStyle<'_>,
    ) -> bool {
        let px_size = style.px_size;
        let Some(cached) = self.glyph_cache.get_or_rasterize(
            style.ctx,
            font_bytes,
            gid.to_u32() as u16,
            px_size,
            true,
            glyph_color,
        ) else {
            return false;
        };

        let m = cell_glyph_metrics(font, gid, px_size, center, cached.width);
        if style.show_metric_marks {
            draw_metric_marks(painter, cell_rect, &m, is_hovered, glyph_color);
        }
        let draw_rect = egui::Rect::from_min_size(
            egui::pos2(m.pen_x + cached.bearing_x, m.baseline_y - cached.bearing_y),
            egui::vec2(cached.width, cached.height),
        );
        let tint = if cached.is_color {
            egui::Color32::WHITE
        } else {
            glyph_color
        };
        cell_painter(painter, cell_rect, is_hovered).image(
            cached.texture.id(),
            draw_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
        true
    }

    /// The rect the hovered cell's glyph occupies, which the hover paints its
    /// black background over — `None` when the cell draws no glyph at all.
    fn compute_glyph_rect(
        &self,
        cell_min: egui::Pos2,
        item: Item,
        style: &CellStyle<'_>,
    ) -> Option<egui::Rect> {
        let center = style.cell_center(cell_min);
        let font = style.raster_font.and_then(|b| FontRef::new(b).ok());
        match item {
            Item::Char(i) => {
                let ch = char::from_u32(self.entries[i].cp)?;
                if let Some(font) = &font
                    && let Some(gid) = font.charmap().map(ch)
                {
                    return Some(raster_glyph_rect(font, gid, style.px_size, center));
                }
                // Whatever `draw_cell`'s UI-font fallback will draw — nothing,
                // for an undeclared character.
                self.entries[i].glyph_name.as_ref()?;
                let glyph_font = crate::app::uniform_font_id(style.ctx, style.px_size);
                let size = style
                    .ctx
                    .fonts(|f| f.layout_no_wrap(ch.to_string(), glyph_font, egui::Color32::WHITE))
                    .size();
                Some(egui::Rect::from_min_size(
                    egui::pos2(center.0 - size.x / 2.0, center.1 - size.y / 2.0),
                    size,
                ))
            }
            Item::Uvs(i) => {
                let entry = &self.uvs_entries[i];
                let font = font?;
                let gid = variant_gid(&font, entry.base, entry.selector)?;
                Some(raster_glyph_rect(&font, gid, style.px_size, center))
            }
            Item::Remap(ri) => Some(raster_glyph_rect(
                &font?,
                skrifa::GlyphId::new(self.remap_entries[ri].gid as u32),
                style.px_size,
                center,
            )),
        }
    }
}

/// A variation-sequence cell's label: `+VS17`, the selector's short name alone.
/// A selector outside the two `VS` ranges (and the Mongolian ones) has no such
/// name and falls back to its code point.
///
/// The base is not repeated here: the cell sits right after the one it varies
/// and shares an open box with it, so its code point is already on
/// screen, and the short form is the one that fits a cell. Where there is no
/// such neighbour to read it against — the status bar, the tooltip — the pair
/// is spelled out in full instead (`status_body`).
fn uvs_label(entry: &UvsEntry) -> String {
    format!("+{}", selector_name(entry.selector))
}

fn selector_name(selector: u32) -> String {
    variation_selector_label(selector).unwrap_or_else(|| format!("U+{selector:04X}"))
}

/// The glyph the built font maps `base` + `selector` to, following cmap format
/// 14's "use default" entries back to the base's own glyph.
fn variant_gid(font: &FontRef, base: u32, selector: u32) -> Option<skrifa::GlyphId> {
    let ch = char::from_u32(base)?;
    match font.charmap().map_variant(ch, char::from_u32(selector)?)? {
        skrifa::charmap::MapVariant::UseDefault => font.charmap().map(ch),
        skrifa::charmap::MapVariant::Variant(gid) => Some(gid),
    }
}

/// `12 / 128 (9.4%)` — how much of a block the source covers.
fn format_coverage((declared, total): (usize, usize)) -> String {
    let pct = if total == 0 {
        0.0
    } else {
        declared as f32 * 100.0 / total as f32
    };
    format!("{declared} / {total} ({pct:.1}%)")
}

/// Whether the built font has metrics for `cp` — an advance to occupy, or an
/// outline to draw. False for a character the font has no glyph for at all
/// (undeclared, or `map`ped to a glyph that does not exist) and for one whose
/// glyph is an empty grid.
///
/// A blank glyph *with* an advance — a space — has metrics: it is a character
/// the font has, and the cell says so.
fn cp_has_metrics(font: &FontRef, cp: u32) -> bool {
    let Some(gid) = char::from_u32(cp).and_then(|ch| font.charmap().map(ch)) else {
        return false;
    };
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    metrics.advance_width(gid).is_some_and(|w| w > 0.0)
        || metrics
            .bounds(gid)
            .is_some_and(|b| b.x_max > b.x_min && b.y_max > b.y_min)
}

/// The glyph anchor point of a specimen cell: horizontally centered, nudged
/// below center to leave room for the codepoint label.
fn cell_center(cell_min: egui::Pos2) -> (f32, f32) {
    (cell_min.x + CELL_W / 2.0, cell_min.y + CELL_H / 2.0 + 8.0)
}

/// Everything a specimen cell is drawn with that is the same for every cell of
/// one grid: glyph size, label style, whether the metric marks are on, and the
/// font to raster from.
struct CellStyle<'a> {
    px_size: f32,
    label_font: &'a egui::FontId,
    label_color: egui::Color32,
    /// The label color of a cell the built font has no metrics for — an empty
    /// glyph, a `map` to a glyph that does not exist, or a character the source
    /// declares nothing about. Dimmer, since such a cell is there to show the
    /// *hole*, not the character.
    dim_label_color: egui::Color32,
    show_metric_marks: bool,
    raster_font: Option<&'a Vec<u8>>,
    ctx: &'a egui::Context,
}

impl CellStyle<'_> {
    fn cell_rect(&self, cell_min: egui::Pos2) -> egui::Rect {
        egui::Rect::from_min_size(cell_min, egui::vec2(CELL_W, CELL_H))
    }

    fn cell_center(&self, cell_min: egui::Pos2) -> (f32, f32) {
        cell_center(cell_min)
    }
}

/// A painter that clips to the cell unless the cell is hovered (hovered
/// cells intentionally overflow their neighbors).
fn cell_painter(painter: &egui::Painter, cell_rect: egui::Rect, is_hovered: bool) -> egui::Painter {
    if is_hovered {
        painter.clone()
    } else {
        painter.with_clip_rect(cell_rect)
    }
}

/// Length of one arm of a metric corner mark, in points.
const METRIC_MARK_LEN: f32 = 5.0;

/// Paints the corners of a cell's metric box — the advance width by the
/// ascent-to-descent band — as crop marks: two segments per corner, each
/// pointing *away* from the box, so the marks say where the metrics are
/// without drawing a frame over the glyph.
fn draw_metric_marks(
    painter: &egui::Painter,
    cell_rect: egui::Rect,
    m: &CellGlyphMetrics,
    is_hovered: bool,
    glyph_color: egui::Color32,
) {
    let left = m.pen_x;
    let right = m.pen_x + m.advance_w;
    let top = m.baseline_y - m.ascent;
    let bottom = m.baseline_y - m.descent;
    if !(left.is_finite() && right.is_finite() && top.is_finite() && bottom.is_finite()) {
        return;
    }

    let stroke = egui::Stroke::new(1.0, glyph_color.gamma_multiply(0.3));
    let painter = cell_painter(painter, cell_rect, is_hovered);
    // (x, y, x-arm direction, y-arm direction) per corner; a zero-advance glyph
    // collapses the two columns onto each other, which is what it looks like.
    for (x, y, dx, dy) in [
        (left, top, -1.0, -1.0),
        (right, top, 1.0, -1.0),
        (left, bottom, -1.0, 1.0),
        (right, bottom, 1.0, 1.0),
    ] {
        let corner = egui::pos2(x, y);
        painter.line_segment(
            [corner, corner + egui::vec2(dx * METRIC_MARK_LEN, 0.0)],
            stroke,
        );
        painter.line_segment(
            [corner, corner + egui::vec2(0.0, dy * METRIC_MARK_LEN)],
            stroke,
        );
    }
}

struct CellGlyphMetrics {
    advance_w: f32,
    ascent: f32,
    descent: f32,
    baseline_y: f32,
    pen_x: f32,
}

/// Baseline/pen placement centering a glyph's advance in a cell.
fn cell_glyph_metrics(
    font: &FontRef,
    gid: skrifa::GlyphId,
    px_size: f32,
    center: (f32, f32),
    fallback_advance: f32,
) -> CellGlyphMetrics {
    let font_metrics = font.metrics(Size::new(px_size), LocationRef::default());
    let glyph_metrics = font.glyph_metrics(Size::new(px_size), LocationRef::default());
    let advance_w = glyph_metrics.advance_width(gid).unwrap_or(fallback_advance);
    let ascent = font_metrics.ascent;
    let descent = font_metrics.descent;
    CellGlyphMetrics {
        advance_w,
        ascent,
        descent,
        baseline_y: center.1 + (ascent + descent) / 2.0,
        pen_x: center.0 - advance_w / 2.0,
    }
}

/// The rect a rasterized glyph's advance/extent occupies in a cell.
fn raster_glyph_rect(
    font: &FontRef,
    gid: skrifa::GlyphId,
    px_size: f32,
    center: (f32, f32),
) -> egui::Rect {
    let m = cell_glyph_metrics(font, gid, px_size, center, 0.0);
    egui::Rect::from_min_size(
        egui::pos2(m.pen_x, m.baseline_y - m.ascent),
        egui::vec2(m.advance_w, m.ascent - m.descent),
    )
}

#[cfg(test)]
#[path = "specimen_tests.rs"]
mod tests;
