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
//!    remap-only glyphs, the `prop` lines and the blocks. Keyed on the
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
//!    column count, since a long section is folded by *rows* and how many rows
//!    it has depends on where they break.
//!
//! The fold is what makes step 2 affordable to look at: filling every block out
//! to its whole range puts 700 rows of Hangul syllables between one block and
//! the next, so a section past [`FOLD_EDGE_ROWS`] rows at each end keeps its two
//! ends and puts everything between them one click away. It is `demo.html`'s
//! rule (`demo.js`, `FOLD_OVER`) rather than one of the panel's own, and the
//! source has no say in it. Only the filled grid folds: with undeclared
//! characters hidden, every row is a glyph the source drew.

#[cfg(test)]
use crate::hash::HashMap;
use crate::hash::HashSet;
use std::collections::BTreeMap;

#[cfg(test)]
use crate::document::{Document, NamePartsMap};
use crate::editor::doc_links::LinkTargetKind;
use crate::glyph_flags::GlyphFlags;
use crate::preview::rasterizer::GlyphCache;
use crate::ucd::BlockMap;

mod collect;
mod layout;
mod paint;
mod status;

use paint::{CellStyle, flag_bg};
use status::{CopyKey, format_coverage, uvs_label};

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
    /// The middle of a folded section: the rows between its two ends, drawn as
    /// one `…` line saying how many they are. A click opens the section for
    /// good — see [`SpecimenState::unfolded`].
    Fold { section: usize, hidden: usize },
}

/// Cell size, the glyph size drawn in one, and the heights of the two rows that
/// hold no cells.
const CELL_W: f32 = 64.0;
const CELL_H: f32 = 80.0;
const HEADING_H: f32 = 24.0;
const ELLIPSIS_H: f32 = 18.0;
/// How many rows of a folded section stay showing at each end — `demo.html`'s
/// `FOLD_EDGE`, so the two pages fold to the same shape.
const FOLD_EDGE_ROWS: usize = 8;
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
    /// The sections a reader has opened out of their fold, by index into
    /// `sections`. Layout state and nothing more: it is cleared whenever the
    /// sections are rebuilt, since an index means something else afterwards.
    unfolded: HashSet<usize>,
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
    /// Keeps a held Ctrl/Cmd+C from copying once per key repeat.
    copy_key: CopyKey,
    /// What steps 2 and 3 cost in the frame that last re-ran them, for the
    /// rebuild report; `None` in a frame that reused them. Read and cleared by
    /// the panel that drew this.
    pub relayout_took: Option<std::time::Duration>,
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
    char_props: crate::ucd::CharProps,
    glyph_flags: GlyphFlags,
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
            unfolded: HashSet::default(),
            cached_gen: None,
            glyph_cache: GlyphCache::new(),
            char_props: crate::ucd::CharProps::default(),
            glyph_flags: GlyphFlags::default(),
            hover_status: None,
            copy_key: CopyKey::default(),
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
            &crate::cancel::CancelToken::never(),
        );
        self.apply(data, font_data_gen, derived_gen);
    }
}

impl SpecimenState {
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
        let mut unfolded_now = false;
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
        // The cell that was under the pointer when the shortcut was *pressed*
        // is the one to copy, so a run of key repeats counts once: see
        // [`CopyKey`].
        let (ctrl_c, copy_key_up) = ui.input(|i| {
            let trigger = i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.modifiers.command && i.key_pressed(egui::Key::C));
            (
                trigger,
                !i.modifiers.command || i.key_released(egui::Key::C),
            )
        });
        let ctrl_c = self
            .copy_key
            .accept(ctrl_c, copy_key_up, std::time::Instant::now());

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
                // The fold is the one row that is not a cell and still answers
                // a click, so it is the one that says so under the pointer.
                if let Some(pos) = hover_pointer.filter(|_| response.contains_pointer())
                    && response.rect.contains(pos)
                    && let Some(row_idx) = layout.row_at(pos.y - origin.y)
                    && matches!(layout.rows.get(row_idx), Some(Row::Fold { .. }))
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

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
                        Row::Fold { hidden, .. } => {
                            // What is behind the fold and how to open it: a
                            // bare `…` reads as a gap in the chart rather than
                            // as something the panel is holding back.
                            let galley = painter.layout_no_wrap(
                                format!("\u{2026} {hidden} more rows — click to show"),
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
                        Item::Uvs(i) => Some(uvs_label(&self.char_props, &self.uvs_entries[i])),
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

                // A click on a fold opens that section and nothing else: the
                // layout is what folded it, so dropping the layout is the whole
                // change, and the next frame lays the section out in full.
                if response.clicked()
                    && let Some(pos) = response.interact_pointer_pos()
                    && response.rect.contains(pos)
                    && let Some(row_idx) = layout.row_at(pos.y - origin.y)
                    && let Row::Fold { section, .. } = layout.rows[row_idx]
                {
                    self.unfolded.insert(section);
                    unfolded_now = true;
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

        // An opened fold invalidates the layout it was part of.
        self.layout = (!unfolded_now).then_some(layout);
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
}

#[cfg(test)]
#[path = "../specimen_tests.rs"]
mod tests;
