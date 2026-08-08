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
//! 1. **What the source says** — the cmap pairs, the remap-only glyphs, the
//!    `prop` lines, the blocks and the `exclude-from-sample` set. Keyed on the
//!    two background generations ([`SpecimenState::cached_gen`]).
//! 2. **Which cells exist, in which sections** — [`SpecimenState::rebuild_sections`].
//!    Keyed additionally on [`SpecimenOptions`], since "show undeclared
//!    characters" fills every block that has a mapped character out to its whole
//!    range: a few hundred cells become a few hundred thousand, which is not
//!    work for a frame.
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
use crate::preview::rasterizer::GlyphCache;
use crate::render::ttf_builder::{decomposed_map_pairs, expand_map_pairs};
use crate::ucd::{BlockMap, format_block_range};

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
/// derived from it, so persisting them later is a matter of round-tripping one
/// value; nothing persists them today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
            show_metric_marks: true,
            group_by_block: false,
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
}

/// What one cell of a section draws.
#[derive(Clone, Copy)]
enum Item {
    Char(usize),
    Remap(usize),
}

/// A run of cells laid out on rows of its own, optionally under a heading.
struct Section {
    /// `None` only for the single unheaded section a grid with
    /// [`SpecimenOptions::group_by_block`] off consists of.
    heading: Option<String>,
    /// Range into [`SpecimenState::items`].
    start: usize,
    len: usize,
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
const UNDECLARED_LABEL_COLOR: egui::Color32 = egui::Color32::from_gray(215);
/// A heading row is drawn against the grid's own white background rather than
/// the app theme's, so its colors are fixed too.
const HEADING_BG: egui::Color32 = egui::Color32::from_gray(128);
const HEADING_FG: egui::Color32 = egui::Color32::WHITE;

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
    declared: BTreeMap<u32, String>,
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
    pub hover_status: Option<String>,
}

impl SpecimenState {
    pub fn new() -> Self {
        Self {
            options: SpecimenOptions::default(),
            entries: Vec::new(),
            remap_entries: Vec::new(),
            items: Vec::new(),
            sections: Vec::new(),
            sections_key: None,
            layout: None,
            declared: BTreeMap::new(),
            blocks: BlockMap::default(),
            excluded: BTreeSet::new(),
            cached_gen: None,
            glyph_cache: GlyphCache::new(),
            char_props: crate::ucd::CharProps::default(),
            hover_status: None,
        }
    }

    pub fn needs_rebuild(&self, font_data_gen: u64, derived_gen: u64) -> bool {
        self.cached_gen != Some((font_data_gen, derived_gen))
    }

    pub fn rebuild_if_needed(
        &mut self,
        docs: &[&Document],
        name_parts: &NamePartsMap,
        name_to_gid: &HashMap<String, u16>,
        face_id: Option<&str>,
        font_data_gen: u64,
        derived_gen: u64,
    ) {
        if !self.needs_rebuild(font_data_gen, derived_gen) {
            return;
        }
        self.cached_gen = Some((font_data_gen, derived_gen));
        // Steps 2 and 3 (see the module docs) rest on what this collects.
        self.sections_key = None;
        self.layout = None;
        self.char_props = crate::ucd::CharProps::collect(docs);
        self.blocks = BlockMap::collect(docs);
        self.excluded = crate::document::excluded_from_sample(docs.iter().flat_map(|d| &d.items));

        // `name_to_gid` comes from the built font, which knows a glyph only by
        // its canonical name, so a character mapped through an alias has to be
        // asked for under that name.
        let aliases = crate::alias::AliasMap::collect(docs, name_parts);

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

        let mut map: BTreeMap<u32, String> = BTreeMap::new();
        let mut mapped_glyphs: HashSet<String> = HashSet::new();
        for doc in docs {
            for item in &doc.items {
                let mut slices: Vec<Option<&str>> = match item.slice_qualifier() {
                    [] => vec![None],
                    qual => qual.iter().map(|s| Some(s.as_str())).collect(),
                };
                slices.retain(|s| face.includes(*s));
                for slice in slices {
                    let parts = scoped.for_slice(slice);
                    match item {
                        DocumentItem::Map {
                            char_repr, glyph, ..
                        } => {
                            let subst_glyph = substitute_name_parts(glyph, parts);
                            let mut pairs = expand_map_pairs(char_repr, &subst_glyph);
                            aliases.canonicalize_pairs(&mut pairs);
                            for (cp, glyph_name) in pairs {
                                mapped_glyphs.insert(glyph_name.clone());
                                map.entry(cp).or_insert(glyph_name);
                            }
                        }
                        DocumentItem::MapDecomposed {
                            char_repr, glyph, ..
                        } => {
                            let subst = glyph.as_ref().map(|g| substitute_name_parts(g, parts));
                            for (cp, name) in decomposed_map_pairs(char_repr, subst.as_deref()) {
                                mapped_glyphs.insert(name.clone());
                                map.entry(cp).or_insert(name);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        self.declared = map;

        // Build reverse map: glyph_name → smallest codepoint.
        let mut glyph_to_cp: HashMap<&str, u32> = HashMap::new();
        for (cp, glyph_name) in &self.declared {
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
        self.remap_entries = with_cp;
        self.remap_entries.append(&mut without_cp);
    }

    /// Step 2: which cells the grid has, and how they are grouped. Reads only
    /// what step 1 left behind, so a change of options never re-reads the
    /// documents.
    fn rebuild_sections(&mut self) {
        self.sections_key = Some(self.options);
        self.layout = None;
        self.entries.clear();
        self.items.clear();
        self.sections.clear();

        // Group the mapped characters by block, in code point order. A code
        // point in no block at all — the UCD leaves gaps between them — goes
        // into one section at the end rather than having a range invented for
        // it, and is never filled.
        let mut by_block: BTreeMap<(u32, u32), (String, bool, Vec<u32>)> = BTreeMap::new();
        let mut no_block: Vec<u32> = Vec::new();
        for &cp in self.declared.keys() {
            match self.blocks.block_of(cp) {
                Some(b) => by_block
                    .entry((b.start, b.end))
                    .or_insert_with(|| (b.name.to_string(), b.stated, Vec::new()))
                    .2
                    .push(cp),
                None => no_block.push(cp),
            }
        }

        let mut groups: Vec<(Option<String>, Vec<u32>)> = Vec::new();
        for ((start, end), (name, stated, cps)) in by_block {
            let cps = if self.options.show_undeclared {
                self.fill_block(start, end, stated)
            } else {
                cps
            };
            let range = format_block_range(start, end);
            groups.push((Some(format!("{name}  {range}")), cps));
        }
        if !no_block.is_empty() {
            groups.push((Some("No Block".to_string()), no_block));
        }

        let grouped = self.options.group_by_block;
        for (heading, cps) in groups {
            let start = self.items.len();
            for cp in cps {
                let glyph_name = self.declared.get(&cp).cloned();
                self.entries.push(CharEntry { cp, glyph_name });
                self.items.push(Item::Char(self.entries.len() - 1));
            }
            if grouped {
                let len = self.items.len() - start;
                self.sections.push(Section {
                    heading,
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
                start: 0,
                len: self.items.len(),
            });
        } else if self.items.len() > remap_start {
            self.sections.push(Section {
                heading: Some("Remaps".to_string()),
                start: remap_start,
                len: self.items.len() - remap_start,
            });
        }
    }

    /// Every code point of one block worth a cell: the ones the source maps or
    /// a `prop` line covers, plus every assigned one — a block's permanent holes
    /// and its unassigned tail are not holes in the *font*, so they get no cell.
    ///
    /// A Private Use code point is the exception, and `stated` — whether a
    /// `prop block` line is what named this block — is what decides it. The UCD
    /// assigns all 137,468 of them and says nothing about any, so filling from
    /// `is_assigned` alone would answer "where are this font's holes?" with
    /// 65,536 empty cells per Private Use plane. Inside an area the source
    /// claimed with `prop block`, on the other hand, the claim *is* the
    /// statement of what should be there, and a gap in it is exactly the hole
    /// worth seeing.
    ///
    /// Code points a nested `prop block` claims belong to that block's own
    /// section, not to this one.
    fn fill_block(&self, start: u32, end: u32, stated: bool) -> Vec<u32> {
        (start..=end)
            // Both bounds, not just the start: a `prop block` claim at the very
            // beginning of a Private Use plane shares its start with the UCD
            // block it overrides, and comparing starts alone would then fill the
            // claimed code points into both sections.
            .filter(|&cp| {
                self.blocks
                    .block_of(cp)
                    .is_some_and(|b| (b.start, b.end) == (start, end))
            })
            .filter(|&cp| {
                self.declared.contains_key(&cp)
                    || self.char_props.stated(cp).is_some()
                    || (crate::ucd::is_assigned(cp) && (stated || !crate::ucd::is_private_use(cp)))
            })
            .collect()
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

    /// Whether every character on one row is excluded from the sample — the
    /// rule that hides the row. A remap cell is never excluded, so a row with
    /// one on it always stays.
    fn row_is_all_excluded(&self, start: usize, len: usize) -> bool {
        self.items[start..start + len]
            .iter()
            .all(|item| match item {
                Item::Char(i) => self.excluded.contains(&self.entries[*i].cp),
                Item::Remap(_) => false,
            })
    }

    #[cfg(test)]
    fn glyph_for_cp(&self, cp: u32) -> Option<&str> {
        self.declared.get(&cp).map(|n| n.as_str())
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
                    format!("# {}", self.sections[*si].heading.as_deref().unwrap_or(""))
                }
                Row::Ellipsis => "\u{2026}".to_string(),
                Row::Cells { start, len } => self.items[*start..*start + *len]
                    .iter()
                    .map(|item| match item {
                        Item::Char(i) => format!("{:04X}", self.entries[*i].cp),
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
                Item::Remap(_) => None,
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

        if self.sections_key != Some(self.options) {
            self.rebuild_sections();
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
        }
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
                    undeclared_label_color: UNDECLARED_LABEL_COLOR,
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
                    painter.rect_filled(style.cell_rect(cell_min), 0.0, egui::Color32::BLACK);
                    if let Some(gr) = self.compute_glyph_rect(cell_min, item, &style) {
                        painter.rect_filled(gr.expand(4.0), 0.0, egui::Color32::BLACK);
                    }
                    // A remap label is wider than a cell far more often than a
                    // `U+XXXX` one; give it a background of its own.
                    if let Item::Remap(ri) = item {
                        let label_galley = painter.layout_no_wrap(
                            self.remap_entries[ri].label.clone(),
                            label_font.clone(),
                            LABEL_COLOR,
                        );
                        let lw = label_galley.size().x + 4.0;
                        if lw > CELL_W {
                            let label_bg = egui::Rect::from_min_size(
                                cell_min,
                                egui::vec2(lw, label_galley.size().y + 2.0),
                            );
                            painter.rect_filled(label_bg, 0.0, egui::Color32::BLACK);
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
                        Item::Char(i) => {
                            self.entries[i]
                                .glyph_name
                                .clone()
                                .map(|name| SpecimenClick {
                                    name,
                                    kind: LinkTargetKind::Glyph,
                                })
                        }
                        Item::Remap(ri) => Some(SpecimenClick {
                            name: self.remap_entries[ri].feature.clone(),
                            kind: LinkTargetKind::Remap,
                        }),
                    };
                }

                if let Some((idx, _)) = hovered {
                    self.hover_status = Some(self.status_for(self.items[idx]));
                    if ctrl_c
                        && let Item::Char(i) = self.items[idx]
                        && let Some(ch) = char::from_u32(self.entries[i].cp)
                    {
                        ui.ctx().copy_text(ch.to_string());
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

    /// The status-bar line for one cell.
    fn status_for(&self, item: Item) -> String {
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
        let hex = format!("{cp:04X}");
        let label_color = if declared {
            style.label_color
        } else {
            style.undeclared_label_color
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
            && let Ok(font) = FontRef::new(font_bytes)
            && let Some(gid) = font.charmap().map(ch)
        {
            drawn_via_rasterizer = self.draw_rasterized_glyph(
                painter,
                cell_rect,
                center,
                &font,
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
        if !drawn_via_rasterizer && declared {
            let glyph_font = crate::app::uniform_font_id(ctx, px_size);
            let glyph_galley = painter.layout_no_wrap(ch.to_string(), glyph_font, glyph_color);
            let glyph_size = glyph_galley.size();
            let pos = egui::pos2(center.0 - glyph_size.x / 2.0, center.1 - glyph_size.y / 2.0);
            cell_painter(painter, cell_rect, is_hovered).galley(pos, glyph_galley, glyph_color);
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
            Item::Remap(ri) => Some(raster_glyph_rect(
                &font?,
                skrifa::GlyphId::new(self.remap_entries[ri].gid as u32),
                style.px_size,
                center,
            )),
        }
    }
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
    /// The label color of a character the source declares nothing about: dimmer,
    /// since the cell is there to show the *hole*, not the character.
    undeclared_label_color: egui::Color32,
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
mod tests {
    use super::*;

    fn doc(src: &str) -> Document {
        crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap()
    }

    const SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
name-parts $l = a b
glyph sq 1 1
@@
glyph a-lig
ref sq
glyph b-lig
ref sq
map U+0061 = sq
remap liga : sq -> ($l)-lig
";

    /// The gid map and `name_parts` arrive from *background* work, so the
    /// specimen can be opened while both are still the previous build's (or,
    /// at startup, empty). Keying its cache on the build request would then
    /// freeze that half-built state in place forever.
    #[test]
    fn rebuilds_when_name_parts_and_gids_arrive_late() {
        let d = doc(SRC);
        let docs = [&d];
        let name_parts = crate::document::collect_name_parts(&docs);
        let gids: HashMap<String, u16> = [
            ("sq".to_string(), 1u16),
            ("a-lig".to_string(), 2),
            ("b-lig".to_string(), 3),
        ]
        .into_iter()
        .collect();

        let mut state = SpecimenState::new();

        // Frame 1: opened before the background build landed — no name parts,
        // no gid map yet.
        assert!(state.needs_rebuild(0, 0));
        state.rebuild_if_needed(&docs, &NamePartsMap::new(), &HashMap::new(), None, 0, 0);
        assert!(state.remap_glyph_names().is_empty());

        // Frame 2: the build and the derived data have landed.
        assert!(state.needs_rebuild(1, 1));
        state.rebuild_if_needed(&docs, &name_parts, &gids, None, 1, 1);
        assert_eq!(state.remap_glyph_names(), vec!["a-lig", "b-lig"]);

        // Nothing new: the cache holds.
        assert!(!state.needs_rebuild(1, 1));
    }

    /// The `prop` lines reach the hover status through the same rebuild as
    /// everything else — one generation behind an edit, never stale after it.
    #[test]
    fn a_rebuild_picks_up_the_prop_lines() {
        let d = doc(concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph logo 1 1\n",
            "@@\n",
            "map U+E000 = logo\n",
            "prop U+E000 = `UNISON LOGO` gc So eaw W\n",
        ));
        let docs = [&d];
        let mut state = SpecimenState::new();
        assert_eq!(state.char_props.name(0xE000), None);

        state.rebuild_if_needed(&docs, &NamePartsMap::new(), &HashMap::new(), None, 1, 1);
        assert_eq!(
            state.char_props.name(0xE000).as_deref(),
            Some("UNISON LOGO")
        );
        assert_eq!(
            state.char_props.property_summary('\u{E000}'),
            "{gc=So eaw=W}"
        );
    }

    const SLICED_SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
face regular : wide
face term : narrow
slice narrow
slice wide
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph star 1 1
@@
glyph star-half 1 1
@@
map wide|narrow : U+2042 = star($-half)
";

    /// Builds a state over `src` as if the background pipeline had landed,
    /// which is all the grid layout needs — it reads no font bytes.
    fn state(src: &str) -> SpecimenState {
        let d = doc(src);
        let docs = [&d];
        let name_parts = crate::document::collect_name_parts(&docs);
        let mut state = SpecimenState::new();
        state.rebuild_if_needed(&docs, &name_parts, &HashMap::new(), None, 1, 1);
        state
    }

    const BLOCKS_SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
glyph sq 1 1
@@
map U+0041 = sq
map U+0042 = sq
map U+2200 = sq
";

    /// Off, the grid is one unheaded run of every mapped character; on, it is
    /// one section per block, in code point order.
    #[test]
    fn grouping_gives_each_block_a_heading_row() {
        let mut state = state(BLOCKS_SRC);
        assert_eq!(state.row_summaries(2), vec!["0041 0042", "2200"]);

        state.options.group_by_block = true;
        assert_eq!(
            state.row_summaries(2),
            vec![
                "# Basic Latin  U+0000..007F",
                "0041 0042",
                "# Mathematical Operators  U+2200..22FF",
                "2200",
            ]
        );
    }

    /// Remap-only glyphs have no code point to sort among the blocks, so they
    /// come last — under a heading of their own once the grid is grouped.
    #[test]
    fn remap_only_glyphs_are_a_section_of_their_own() {
        let mut state = SpecimenState::new();
        let d = doc(SRC);
        let docs = [&d];
        let name_parts = crate::document::collect_name_parts(&docs);
        let gids: HashMap<String, u16> = [("a-lig".to_string(), 2u16), ("b-lig".to_string(), 3)]
            .into_iter()
            .collect();
        state.rebuild_if_needed(&docs, &name_parts, &gids, None, 1, 1);

        assert_eq!(state.row_summaries(4), vec!["0061 a-lig b-lig"]);
        state.options.group_by_block = true;
        assert_eq!(
            state.row_summaries(4),
            vec![
                "# Basic Latin  U+0000..007F",
                "0061",
                "# Remaps",
                "a-lig b-lig",
            ]
        );
    }

    /// A `prop block` claim is what names an area of the Private Use planes;
    /// the UCD calls all of it "Supplementary Private Use Area-A".
    #[test]
    fn a_stated_block_names_its_own_section() {
        let mut state = state(concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph logo 1 1\n",
            "@@\n",
            "prop block `Unison Symbols` = U+F0000..F000F\n",
            "map U+F0000 = logo\n",
        ));
        state.options.group_by_block = true;
        assert_eq!(
            state.row_summaries(4),
            vec!["# Unison Symbols  U+F0000..F000F", "F0000"]
        );
    }

    /// Filling a Private Use area from `is_assigned` would call all 65,536 code
    /// points of a plane present, since the UCD assigns every one of them and
    /// says nothing about any. Only a `prop block` claim, or a `prop` line, is a
    /// statement that a Private Use character should be there.
    #[test]
    fn filling_stops_at_the_edge_of_a_claimed_private_use_area() {
        let mut state = state(concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph logo 1 1\n",
            "@@\n",
            // A claim of four code points, one drawn…
            "prop block `Unison Symbols` = U+F0000..F0003\n",
            "map U+F0000 = logo\n",
            // …and two more characters out in unclaimed Private Use: one mapped,
            // one only named.
            "map U+F1000 = logo\n",
            "prop U+F1001 = `UNISON SPARE`\n",
        ));
        state.options.show_undeclared = true;
        // The claimed area fills, holes and all; the plane around it does not.
        assert_eq!(
            state.cell_cps(),
            vec![0xF0000, 0xF0001, 0xF0002, 0xF0003, 0xF1000, 0xF1001]
        );
    }

    /// "Show undeclared characters" fills every block that has a mapped
    /// character out to its whole range — but only with code points the UCD
    /// assigns, since a block's permanent holes are not holes in the font.
    #[test]
    fn filling_a_block_skips_the_code_points_nothing_assigns() {
        // U+0370 GREEK CAPITAL LETTER HETA is in Greek and Coptic
        // (U+0370..03FF), whose U+0378 and U+0379 are permanent holes.
        let mut state = state(concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph sq 1 1\n",
            "@@\n",
            "map U+0370 = sq\n",
        ));
        assert_eq!(state.cell_cps(), vec![0x370]);

        state.options.show_undeclared = true;
        let cps = state.cell_cps();
        assert_eq!(cps.first(), Some(&0x370));
        assert_eq!(cps.last(), Some(&0x3FF));
        assert!(cps.contains(&0x377));
        assert!(!cps.contains(&0x378));
        assert!(!cps.contains(&0x379));
        assert!(cps.contains(&0x37A));
        // The rest of the code space is untouched: no other block has a mapped
        // character to fill from.
        assert!(cps.iter().all(|cp| (0x370..=0x3FF).contains(cp)));
    }

    /// The row-hiding rule, both halves of it: a filled grid drops a row whose
    /// every character is excluded from the sample, and keeps one where any
    /// single character is not.
    #[test]
    fn an_entirely_excluded_row_is_hidden_only_while_filling() {
        let src = concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph sq 1 1\n",
            "@@\n",
            "map U+0041 = sq\n",
            "exclude-from-sample U+0042..0043\n",
        );
        // Without filling, an excluded character that is *mapped* still shows:
        // hiding exists to make a filled grid readable, and the grid is not
        // filled here.
        let mut state = state(src);
        state.options.show_undeclared = false;
        assert_eq!(state.row_summaries(2), vec!["0041"]);

        state.options.show_undeclared = true;
        let rows = state.row_summaries(2);
        // U+0042..0043 is exactly one row at two columns, and it is gone — but
        // an ellipsis stands where it was. The rows on either side, each with an
        // unexcluded character, are untouched.
        let hidden = rows.iter().position(|r| r == "\u{2026}").unwrap();
        assert_eq!(rows[hidden - 1], "0040 0041");
        assert_eq!(rows[hidden + 1], "0044 0045");
        assert!(!rows.contains(&"0042 0043".to_string()));
        // At three columns the same characters fall on rows that also carry an
        // unexcluded one, so nothing is hidden at all.
        let rows = state.row_summaries(3);
        assert!(rows.contains(&"0042 0043 0044".to_string()));
        assert!(!rows.contains(&"\u{2026}".to_string()));
    }

    /// A run of hidden rows is one ellipsis, not one per row: the point is to
    /// say something was left out, and the excluded ranges are the bulk ones.
    #[test]
    fn consecutive_hidden_rows_collapse_into_one_ellipsis() {
        let mut state = state(concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph sq 1 1\n",
            "@@\n",
            "map U+0041 = sq\n",
            "exclude-from-sample U+0042..0047\n",
        ));
        state.options.show_undeclared = true;
        let rows = state.row_summaries(2);
        assert_eq!(rows.iter().filter(|r| *r == "\u{2026}").count(), 1);
        assert!(rows.contains(&"0048 0049".to_string()));
    }

    /// A section every row of which was hidden still keeps its heading, with the
    /// ellipsis under it — a block that is entirely excluded is a thing worth
    /// seeing the name of.
    #[test]
    fn a_wholly_excluded_block_keeps_its_heading_and_an_ellipsis() {
        let mut state = state(concat!(
            "meta height 16\n",
            "meta ascent 14\n",
            "meta descent 2\n",
            "glyph sq 1 1\n",
            "@@\n",
            "map U+0041 = sq\n",
            "map U+2200 = sq\n",
            "exclude-from-sample U+2200..22FF\n",
        ));
        state.options.group_by_block = true;
        state.options.show_undeclared = true;
        let rows = state.row_summaries(8);
        assert_eq!(
            &rows[rows.len() - 2..],
            ["# Mathematical Operators  U+2200..22FF", "\u{2026}"]
        );
    }

    /// Only the mapped characters are clickable, so the fill has to record
    /// which cells are which.
    #[test]
    fn a_filled_cell_knows_it_has_no_glyph() {
        let mut state = state(BLOCKS_SRC);
        state.options.show_undeclared = true;
        state.rebuild_sections();
        let by_cp: HashMap<u32, bool> = state
            .entries
            .iter()
            .map(|e| (e.cp, e.glyph_name.is_some()))
            .collect();
        assert!(by_cp[&0x41]);
        assert!(!by_cp[&0x40]);
    }

    /// A slice-qualified `map` substitutes with *that slice's* name parts, so
    /// the specimen has to expand it per slice like the builder does — and pick
    /// the slice the face it is drawing actually includes. Expanding it with the
    /// unqualified parts left `$-half` verbatim in the glyph name, which then
    /// matched no glyph and made the cell unclickable.
    #[test]
    fn slice_scoped_name_parts_expand_per_face() {
        let d = doc(SLICED_SRC);
        let docs = [&d];
        let name_parts = crate::document::collect_name_parts(&docs);
        let gids: HashMap<String, u16> = [("star".to_string(), 1u16), ("star-half".to_string(), 2)]
            .into_iter()
            .collect();

        let mut state = SpecimenState::new();
        state.rebuild_if_needed(&docs, &name_parts, &gids, Some("regular"), 1, 1);
        assert_eq!(state.glyph_for_cp(0x2042), Some("star"));

        state.rebuild_if_needed(&docs, &name_parts, &gids, Some("term"), 2, 1);
        assert_eq!(state.glyph_for_cp(0x2042), Some("star-half"));
    }
}
