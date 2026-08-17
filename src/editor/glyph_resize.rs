//! Resize mode: dragging a glyph's boundary, and what that costs everything
//! else.
//!
//! # Two rectangles
//!
//! A glyph has two, and they are dragged from two different places:
//!
//! - **The declared box** — what the glyph *claims*, which is what every `ref`
//!   to it is measured against. `F2` over a pixel grid drags this one. Nothing
//!   the glyph draws moves; its `origin`/`advance`/`extent` change.
//! - **The pixel grid** — how much room the drawing has. Dragged from under the
//!   backreference shadow (`` ` `` twice), which is up exactly when the question
//!   "does my ink still fit where I am used?" is being asked, and which is
//!   bigger than the canvas nearly always, so the drag has room to preview in;
//!   it stays up for the drag it started. The session begins only once the drag
//!   has a whole logical pixel to show for itself, so the mode switch is
//!   something the user sees happen *because* of a change they made — see
//!   [`CanvasStart`]. Growing the canvas never moves what is already drawn.
//!
//! Arrow keys move an edge in either mode (a key moves the boundary *towards*
//! the direction it names, so `Shift+Up` pulls the bottom edge up and shrinks
//! the rectangle); Enter applies, Escape or losing the focus cancels. Nothing
//! is committed while the mode is live: the document is previewed by rewriting
//! the glyph's block from a pristine snapshot on every step, so what is on
//! screen is exactly what applying would produce, and cancelling is a single
//! splice back.
//!
//! # What each one costs
//!
//! **A canvas resize costs nothing outside the glyph.** Growing the grid to the
//! left or the top moves the ink's *grid* coordinates, so the block moves with
//! it — the pixels, the glyph's own `anchor` lines, its own explicitly-offset
//! `ref`s — and the header states the box that would otherwise have drifted:
//! an `origin` that keeps the box's corner on the same ink, and the width it
//! had, which was the grid's until the grid changed ([`canvas_box`]). The glyph
//! draws and measures exactly as before, so nothing that uses it moves. Room is
//! all that changed.
//!
//! **A box drag is what moves everything else.** It changes what the glyph
//! claims, and a `ref` offset names that claim's corner, so **every `ref` to
//! the glyph shifts by the near edges' movement, negated**: `ref foo 1 2`
//! becomes `ref foo -1 2` when `foo`'s box grows two columns to the left. A
//! negative offset is a bearing and is exactly the right answer (see
//! [`crate::ref_composite`]). The drawing itself does not move at all.
//!
//! Two things decide whether a `ref` line is touched at all:
//!
//! - **It must name the glyph outright** — the name as written (an `@…` form
//!   expands first), or an alias of it. A ref that only reaches the glyph
//!   through a name-part or pattern expansion is left alone, matching what
//!   Search reports as a reference.
//! - **It must not be anchor-placed.** An offset-less `ref` whose placement
//!   came from an anchor match already follows its target's anchors, which
//!   this resize moved in step, so writing an offset onto it would freeze it
//!   at the position it happened to have. An offset-less ref that matched
//!   *nothing*, though, is a (0, 0) fallback and does need the offset spelled
//!   out. [`crate::ref_composite::DeriveOutcome::anchor_placed`] is the only
//!   thing that can tell the two apart.
//!
//! # Units
//!
//! Sizes are dragged in **logical** pixels — the numbers the header states —
//! so a `scale N` glyph always moves a whole subcell block and there is no
//! such thing as an inexact resize. Everything downstream of the header is in
//! raster (subcell) coordinates: the grid, `anchor` positions and `ref`
//! offsets alike, each at *its own* glyph's scale. So the shift inside the
//! resized glyph is `delta * its own scale`, while the compensation applied to
//! a `ref` elsewhere is `delta * the referring glyph's scale` — one logical
//! pixel of the target is one logical pixel of whoever draws it, whatever
//! either scale is.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::document::{DocLine, Document, DocumentItem, GlyphBody, NamePartsMap, PixelGrid};
use crate::editor::pixel_interaction::layer_doc_line;
use crate::editor::undo::UndoOp;
use crate::editor::{EditMode, EditorState};
use crate::pixel::PixelShape;
use crate::ref_composite::{AlternativesIndex, ResolvedGlyph};

/// How far each edge of a glyph has moved outwards, in logical pixels. A
/// negative component pulls that edge inwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResizeDeltas {
    pub left: i16,
    pub right: i16,
    pub top: i16,
    pub bottom: i16,
}

impl ResizeDeltas {
    pub(crate) fn is_zero(&self) -> bool {
        *self == Self::default()
    }

    /// The `(dcol, drow)` the *content* moves by inside a grid of this scale.
    /// Only the left and top edges move it; growing to the right or downwards
    /// appends cells past the ink and leaves it where it was.
    fn content_shift(&self, scale: u8) -> (i32, i32) {
        let s = scale.max(1) as i32;
        (self.left as i32 * s, self.top as i32 * s)
    }
}

/// What a resize session is dragging.
///
/// The two are the same gesture over two different rectangles, and the
/// difference is what each one is *for*:
///
/// - [`Box`](ResizeKind::Box) drags the **declared box** — what the glyph
///   claims, which is what every `ref` to it is measured against. Nothing the
///   glyph draws moves; its header's `origin`/`advance`/`extent` change and
///   every parent follows.
/// - [`Canvas`](ResizeKind::Canvas) drags the **pixel grid** — how much room
///   the drawing has. The ink stays where it is on screen, so the glyph's own
///   anchors and refs shift inside the grown grid and the header takes an
///   `origin` to match; nothing outside the glyph changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeKind {
    Box,
    Canvas,
}

/// Which edge a drag or a key is moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// The resize the editor wants carried out, once the user applies it. The
/// editor cannot do it itself: a `ref` to this glyph may live in any file of
/// the font, open or not, so only the host can complete it — the same split
/// as [`crate::app::rename`].
#[derive(Clone, Debug)]
pub struct ResizeAction {
    /// The document that defines the glyph.
    pub path: PathBuf,
    /// Its item index in that document, as of the frame the action was made.
    pub item_idx: usize,
    pub glyph_name: String,
    pub deltas: ResizeDeltas,
    pub(crate) kind: ResizeKind,
}

/// The live resize session: the glyph, the pristine text of its block, and
/// how far each edge has been moved so far.
///
/// Every step recomputes the preview from `orig_block`/`body` rather than
/// from the previewed document, so repeated nudges cannot compound rounding
/// or re-shift an already-shifted `anchor` line.
pub(crate) struct GlyphResize {
    pub kind: ResizeKind,
    /// The font's metrics, for the box's height when the header states none.
    /// Held rather than passed because the preview recomputes the box on every
    /// step, from the pristine body, exactly as applying it later will.
    meta: crate::meta::FontMetrics,
    pub item_idx: usize,
    pub name: String,
    pub header_line: usize,
    pub orig_block: Vec<DocLine>,
    /// Length of the block as it currently stands in `lines`.
    cur_len: usize,
    pub body: GlyphBody,
    /// Which of the glyph's own refs are anchor-placed, worked out once when
    /// the mode is entered: a resize moves anchors but never renames or
    /// resizes them, so the answer cannot change while the mode is live.
    own_anchor_placed: Vec<bool>,
    /// The mode to go back to when the session ends, whichever way it ends:
    /// F2 is reachable from a plain caret on the grid line as well as from
    /// either pixel mode, and leaving resize must not move the user somewhere
    /// they were not.
    pub return_mode: EditMode,
    pub deltas: ResizeDeltas,
}

impl GlyphResize {
    /// Logical size the dragged rectangle would have with the current deltas.
    pub(crate) fn preview_dims(&self) -> (i32, i32) {
        let (w, h) = match self.kind {
            ResizeKind::Box => {
                let (_, w, h) = declared_box_of(&self.body, self.meta);
                (w as i32, h as i32)
            }
            ResizeKind::Canvas => {
                let s = self.body.scale.max(1) as i32;
                match &self.body.pixels {
                    Some(g) => (g.width as i32 / s, g.height as i32 / s),
                    None => (0, 0),
                }
            }
        };
        (
            w + self.deltas.left as i32 + self.deltas.right as i32,
            h + self.deltas.top as i32 + self.deltas.bottom as i32,
        )
    }

    /// The declared box the current deltas would leave — see [`boxed_for`].
    fn boxed(&self) -> BoxFlags {
        boxed_for(&self.body, self.meta, self.deltas)
    }
}

/// The resolution tables a resize reads. Bundled because both the editor and
/// the host need the same three, and neither owns them.
#[derive(Clone, Copy)]
pub(crate) struct ResolveEnv<'a> {
    pub named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    pub name_parts: &'a NamePartsMap,
    pub alt_index: &'a AlternativesIndex,
}

/// Which of `body`'s refs got their placement from an anchor match.
fn anchor_placed_refs(body: &GlyphBody, env: ResolveEnv<'_>) -> Vec<bool> {
    if body.refs.is_empty() {
        return Vec::new();
    }
    // The same lookups `ref_composite::compute_composite` derives with, so
    // the editor's answer here cannot disagree with what it draws.
    crate::ref_composite::derive_ref_offsets_detailed(
        &body.points,
        &body.refs,
        body.scale,
        |name| {
            crate::ref_composite::resolve_ref_name_for_view(name, env.named_glyphs, env.name_parts)
                .map(|resolved| resolved.resolved_anchors.clone())
        },
        |name| env.alt_index.get(name).to_vec(),
        |name| {
            crate::ref_composite::resolve_ref_name_for_view(name, env.named_glyphs, env.name_parts)
                .map(|resolved| resolved.declared_anchors.clone())
        },
        |name| {
            crate::ref_composite::resolve_ref_name_for_view(name, env.named_glyphs, env.name_parts)
                .map_or((0, 0), |resolved| resolved.declared_origin)
        },
    )
    .anchor_placed
}

/// The lines one glyph owns: its header, its grid (when it has one) and the
/// `ref`/`anchor` lines that follow, which the parser accepts in any order but
/// never interleaved with anything else.
pub(crate) fn glyph_block_len(lines: &[DocLine], body: &GlyphBody, header_line: usize) -> usize {
    let base =
        header_line + 1 + usize::from(matches!(lines.get(header_line + 1), Some(DocLine::Grid(_))));
    let total = body.refs.len() + body.points.len();
    let mut n = 0usize;
    while n < total {
        match lines.get(base + n) {
            Some(DocLine::Text(t)) => match t.split_whitespace().next() {
                Some("ref") | Some("anchor") => n += 1,
                _ => break,
            },
            _ => break,
        }
    }
    base + n - header_line
}

/// The declared box a glyph starts a drag from, in logical pixels.
///
/// The width is what the header states, or what is left of the grid right of
/// the origin when it states nothing (the unstated box ends where the raster
/// does — [`GlyphBody::declared_extent`]); the height is the em box unless
/// `extent` says otherwise. This is the rectangle
/// [`crate::editor::document_view::glyph_metrics`] draws, and it has to be,
/// since that is what the pointer is grabbing.
fn declared_box_of(body: &GlyphBody, meta: crate::meta::FontMetrics) -> ((i16, i16), u16, u16) {
    let s = body.scale.max(1) as u16;
    let grid = body.pixels.as_ref();
    let origin = body.declared_origin();
    let width = body
        .stated_advance()
        .or_else(|| grid.map(|g| ((g.width / s) as i32 - origin.0 as i32).max(0) as u16))
        .unwrap_or(0);
    let height = match body.extent {
        Some((_, h)) => h,
        None => meta.ascent() + meta.descent(),
    };
    (origin, width, height)
}

/// What a header should state about its box, as the three flags that state it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoxFlags {
    origin: (i16, i16),
    advance: Option<u16>,
    extent: Option<(u16, u16)>,
}

/// The box a *canvas* resize leaves: the same rectangle it started as.
///
/// The ink's grid coordinates move when the grid grows at the left or the top,
/// so the box's corner moves with them — that automatic `origin` is what keeps
/// the drawing where it was drawn. The width has to be pinned along with it
/// whenever the header left it to the grid, since the grid is what just
/// changed; a width the header already stated is already pinned, and a height
/// is unaffected either way (the box's own is the em box, and a stated one
/// stays stated).
///
/// What that implicit width *was* is the grid's right edge measured from the
/// old origin, not the whole grid: pinning the grid's width would widen the box
/// of every glyph that already carried an origin.
fn canvas_box(body: &GlyphBody, deltas: ResizeDeltas) -> BoxFlags {
    let (oc, or) = body.declared_origin();
    let origin = (oc + deltas.left, or + deltas.top);
    if body.extent.is_some() {
        return BoxFlags {
            origin,
            advance: None,
            extent: body.extent,
        };
    }
    let s = body.scale.max(1) as u16;
    let width_moved = deltas.left != 0 || deltas.right != 0;
    let advance = body.advance.or_else(|| {
        width_moved.then(|| {
            body.pixels
                .as_ref()
                .map_or(0, |g| ((g.width / s) as i32 - oc as i32).max(0) as u16)
        })
    });
    BoxFlags {
        origin,
        advance,
        extent: None,
    }
}

/// The declared box `deltas` leave, as the header should state it.
///
/// A width equal to what the glyph resolves to is still written: the drag said
/// where the edge goes, and leaving it implicit would let it drift with the
/// ink. A height is written only once one is *asked* for — a vertical drag, or
/// a header that already stated one — so the em box stays the answer for every
/// glyph that never had an opinion about it.
fn boxed_for(body: &GlyphBody, meta: crate::meta::FontMetrics, deltas: ResizeDeltas) -> BoxFlags {
    let ((oc, or), w, h) = declared_box_of(body, meta);
    let origin = (oc - deltas.left, or - deltas.top);
    let w = (w as i32 + deltas.left as i32 + deltas.right as i32).max(0) as u16;
    let h = (h as i32 + deltas.top as i32 + deltas.bottom as i32).max(0) as u16;
    if body.extent.is_some() || deltas.top != 0 || deltas.bottom != 0 {
        BoxFlags {
            origin,
            advance: None,
            extent: Some((w, h)),
        }
    } else {
        BoxFlags {
            origin,
            advance: Some(w),
            extent: None,
        }
    }
}

/// The glyph's header, rewritten for a box that has moved. Nothing else in the
/// block changes: the drawing stays exactly where it is, and only what the
/// glyph *claims* about it moves.
fn rebox_block(block: &[DocLine], boxed: BoxFlags) -> Option<Vec<DocLine>> {
    let DocLine::Text(header) = block.first()? else {
        return None;
    };
    let new_header = crate::document_io::replace_glyph_box_flags(
        header,
        (boxed.origin != (0, 0)).then_some(boxed.origin),
        boxed.advance,
        boxed.extent,
    )?;
    let mut out = block.to_vec();
    out[0] = DocLine::Text(new_header);
    Some(out)
}

/// The glyph's own block, rewritten for `deltas`: the header's dimensions,
/// the grid's contents, and every line positioned against the grid.
///
/// `None` when the resize would leave no glyph at all (less than one logical
/// pixel either way) or when the header owns no grid to resize.
pub(crate) fn resize_block(
    block: &[DocLine],
    body: &GlyphBody,
    deltas: ResizeDeltas,
    own_anchor_placed: &[bool],
) -> Option<Vec<DocLine>> {
    let grid = body.pixels.as_ref()?;
    let boxed = canvas_box(body, deltas);
    let s = body.scale.max(1) as i32;
    let new_w = grid.width as i32 + (deltas.left as i32 + deltas.right as i32) * s;
    let new_h = grid.height as i32 + (deltas.top as i32 + deltas.bottom as i32) * s;
    if new_w < s || new_h < s || new_w > u16::MAX as i32 || new_h > u16::MAX as i32 {
        return None;
    }
    let (dcol, drow) = deltas.content_shift(body.scale);

    let mut out: Vec<DocLine> = block.to_vec();
    let DocLine::Text(header) = out.first()? else {
        return None;
    };
    let new_header = crate::document_io::replace_glyph_header_dims(
        header,
        (new_w / s) as u16,
        (new_h / s) as u16,
    )?;
    // …and the box the wider grid would otherwise have moved. Growing the
    // canvas is a change of *room*: the drawing does not move, so neither may
    // what the glyph claims about it.
    let new_header = crate::document_io::replace_glyph_box_flags(
        &new_header,
        (boxed.origin != (0, 0)).then_some(boxed.origin),
        boxed.advance,
        boxed.extent,
    )?;
    out[0] = DocLine::Text(new_header);

    let new_grid = shifted_grid(grid, dcol, drow, new_w as u16, new_h as u16);
    match out.get(1) {
        Some(DocLine::Grid(_)) => out[1] = DocLine::Grid(new_grid),
        // An all-empty grid is written as a bare header, and `reconcile`
        // materializes the `DocLine::Grid` for it. Do the same rather than
        // leave the header describing a grid that is not there.
        _ => out.insert(1, DocLine::Grid(new_grid)),
    }

    if dcol != 0 || drow != 0 {
        let (dcol, drow) = (dcol as i16, drow as i16);
        let (mut ref_i, mut point_i) = (0usize, 0usize);
        for line in out.iter_mut().skip(2) {
            let DocLine::Text(text) = line else { continue };
            match text.split_whitespace().next() {
                Some("ref") => {
                    let idx = ref_i;
                    ref_i += 1;
                    let Some(gref) = body.refs.get(idx) else {
                        continue;
                    };
                    // An anchor-placed ref follows the anchors, which moved
                    // with everything else; leaving it alone is what keeps it
                    // attached.
                    if own_anchor_placed.get(idx).copied().unwrap_or(false) {
                        continue;
                    }
                    *line = DocLine::Text(
                        gref.format_line(Some((gref.col() + dcol, gref.row() + drow))),
                    );
                }
                Some("anchor") => {
                    let idx = point_i;
                    point_i += 1;
                    if let Some(point) = body.points.get(idx) {
                        *line = DocLine::Text(point.shifted(dcol, drow).format_line());
                    }
                }
                _ => break,
            }
        }
    }

    Some(out)
}

/// `grid` in a `new_w`×`new_h` box with its contents moved by `(dcol, drow)`,
/// whatever falls outside cropped away.
fn shifted_grid(grid: &PixelGrid, dcol: i32, drow: i32, new_w: u16, new_h: u16) -> PixelGrid {
    let mut out = PixelGrid::new(new_w, new_h);
    for r in 0..grid.height {
        let nr = r as i32 + drow;
        if nr < 0 || nr >= new_h as i32 {
            continue;
        }
        for c in 0..grid.width {
            let nc = c as i32 + dcol;
            if nc < 0 || nc >= new_w as i32 {
                continue;
            }
            let shape = grid.get(r, c);
            if shape == PixelShape::EMPTY {
                continue;
            }
            out.set(nr as u16, nc as u16, shape);
            if let Some(region) = grid.details.get(&(r, c)) {
                out.details.insert((nr as u16, nc as u16), region.clone());
            }
        }
    }
    if !out.details.is_empty() {
        out.den = grid.den;
    }
    out
}

/// One rewritten line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LineEdit {
    pub line: usize,
    pub old: String,
    pub new: String,
}

/// Every name that means the resized glyph: its own, plus every alias that
/// resolves to it. A `ref` written with an alias reaches the same glyph id, so
/// it is offset by the same amount.
pub(crate) fn target_names(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    glyph_name: &str,
) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    names.insert(glyph_name.to_string());
    let aliases = crate::alias::AliasMap::collect(docs, name_parts);
    for (name, target) in aliases.entries() {
        if target == glyph_name {
            names.insert(name.clone());
        }
    }
    names
}

/// The `ref` lines in one document that name the resized glyph and have to
/// move with it.
pub(crate) fn adjust_refs_in_doc(
    doc: &Document,
    lines: &[DocLine],
    names: &HashSet<String>,
    deltas: ResizeDeltas,
    env: ResolveEnv<'_>,
) -> Vec<LineEdit> {
    let mut edits = Vec::new();
    if deltas.left == 0 && deltas.top == 0 {
        // Only the right and bottom edges moved: nothing that refers to this
        // glyph has to move at all.
        return edits;
    }
    for (item_idx, item) in doc.items.iter().enumerate() {
        let DocumentItem::Glyph { body, .. } = item else {
            continue;
        };
        if !body.refs.iter().any(|r| names.contains(&r.name)) {
            continue;
        }
        let anchor_placed = anchor_placed_refs(body, env);
        // A referring glyph counts the target's logical pixels in its own
        // raster cells, which is what its scale converts.
        let ps = body.scale.max(1) as i16;
        let (dcol, drow) = (deltas.left * ps, deltas.top * ps);
        let header_line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
        for (ref_idx, gref) in body.refs.iter().enumerate() {
            if !names.contains(&gref.name) {
                continue;
            }
            if anchor_placed.get(ref_idx).copied().unwrap_or(false) {
                continue;
            }
            let new_offset = (gref.col() - dcol, gref.row() - drow);
            if gref.offset == Some(new_offset) {
                continue;
            }
            let line = layer_doc_line(lines, body, header_line, ref_idx);
            let Some(DocLine::Text(old)) = lines.get(line) else {
                continue;
            };
            let new = gref.format_line(Some(new_offset));
            if *old != new {
                edits.push(LineEdit {
                    line,
                    old: old.clone(),
                    new,
                });
            }
        }
    }
    edits
}

/// Everything one document changes for a resize, as `(at, old, new)` blocks
/// ready to splice and to record as a single undo entry.
///
/// `define_item` is the glyph's own item index when this document is the one
/// that defines it; the other documents only ever get `ref` line edits.
#[expect(clippy::too_many_arguments)]
pub(crate) fn plan_document_resize(
    doc: &Document,
    lines: &[DocLine],
    names: &HashSet<String>,
    deltas: ResizeDeltas,
    define_item: Option<usize>,
    env: ResolveEnv<'_>,
    kind: ResizeKind,
    meta: crate::meta::FontMetrics,
) -> Vec<(usize, Vec<DocLine>, Vec<DocLine>)> {
    let mut plan: Vec<(usize, Vec<DocLine>, Vec<DocLine>)> = Vec::new();
    if let Some(item_idx) = define_item
        && let Some(DocumentItem::Glyph { body, .. }) = doc.items.get(item_idx)
    {
        let header_line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
        let len = glyph_block_len(lines, body, header_line);
        let old: Vec<DocLine> = lines[header_line..header_line + len].to_vec();
        // The same two rewrites the preview used, for the same reason: what a
        // box drag changes is the header, and what a canvas drag changes is
        // everything positioned against the grid.
        let rewritten = match kind {
            ResizeKind::Box => rebox_block(&old, boxed_for(body, meta, deltas)),
            ResizeKind::Canvas => resize_block(&old, body, deltas, &anchor_placed_refs(body, env)),
        };
        if let Some(new) = rewritten
            && new != old
        {
            plan.push((header_line, old, new));
        }
    }
    // Only a box drag moves what uses the glyph. A canvas drag keeps the ink
    // exactly where it was relative to the box (see [`canvas_box`]), so every
    // `ref` to it already points at the right place.
    let ref_edits = match kind {
        ResizeKind::Box => adjust_refs_in_doc(doc, lines, names, deltas, env),
        ResizeKind::Canvas => Vec::new(),
    };
    for edit in ref_edits {
        plan.push((
            edit.line,
            vec![DocLine::Text(edit.old)],
            vec![DocLine::Text(edit.new)],
        ));
    }
    // Splicing back to front keeps the earlier positions valid even when a
    // block changes length.
    plan.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    plan
}

/// Apply a plan to `lines`, returning the undo ops that take it back. The
/// caller records them as *one* entry: a resize is one action however many
/// lines it touched.
pub(crate) fn apply_plan(
    lines: &mut Vec<DocLine>,
    plan: Vec<(usize, Vec<DocLine>, Vec<DocLine>)>,
) -> Vec<UndoOp> {
    let mut ops = Vec::with_capacity(plan.len());
    for (at, old, new) in plan {
        if at + old.len() > lines.len() {
            continue;
        }
        lines.splice(at..at + old.len(), new.iter().cloned());
        ops.push(UndoOp::Lines { at, old, new });
    }
    ops
}

// ---------------------------------------------------------------------------
// The editor-side session
// ---------------------------------------------------------------------------

/// Enter resize mode on the glyph at `item_idx`.
///
/// A canvas resize needs a grid to resize; a box drag does not, but it does
/// need the font's em height, which is the box's own whenever the header does
/// not say otherwise.
pub(crate) fn begin(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    item_idx: usize,
    env: ResolveEnv<'_>,
    kind: ResizeKind,
    meta: crate::meta::FontMetrics,
) -> bool {
    let Some(DocumentItem::Glyph { name, body }) = doc.items.get(item_idx) else {
        return false;
    };
    if body.pixels.is_none() && kind == ResizeKind::Canvas {
        return false;
    }
    let header_line = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
    if !matches!(lines.get(header_line), Some(DocLine::Text(_))) {
        return false;
    }
    // A floating pixel selection is an edit the user is still holding; land
    // it before the block is snapshotted, or cancelling the resize would put
    // back a block that never had those pixels in it.
    if let Some(sel) = state
        .pixel_selection
        .clone()
        .filter(crate::editor::pixel_selection::PixelSelection::is_floating)
    {
        crate::editor::pixel_selection::commit_and_clear(doc, lines, state, &sel);
    }
    state.pixel_selection = None;

    let len = glyph_block_len(lines, body, header_line);
    state.resize = Some(GlyphResize {
        kind,
        meta,
        item_idx,
        name: name.0.clone(),
        header_line,
        orig_block: lines[header_line..header_line + len].to_vec(),
        cur_len: len,
        body: body.clone(),
        own_anchor_placed: anchor_placed_refs(body, env),
        return_mode: state.mode.clone(),
        deltas: ResizeDeltas::default(),
    });
    state.mode = EditMode::GlyphResize { item_idx };
    true
}

/// Rewrite the glyph's block for the session's current deltas. Always starts
/// from the pristine snapshot, so the deltas never compound.
fn refresh_preview(lines: &mut Vec<DocLine>, state: &mut EditorState) -> bool {
    let Some(session) = state.resize.as_mut() else {
        return false;
    };
    let new_block = if session.deltas.is_zero() {
        session.orig_block.clone()
    } else {
        let rewritten = match session.kind {
            ResizeKind::Box => rebox_block(&session.orig_block, session.boxed()),
            ResizeKind::Canvas => resize_block(
                &session.orig_block,
                &session.body,
                session.deltas,
                &session.own_anchor_placed,
            ),
        };
        match rewritten {
            Some(block) => block,
            None => return false,
        }
    };
    let start = session.header_line;
    let end = start + session.cur_len;
    if end > lines.len() {
        return false;
    }
    if lines[start..end] == new_block[..] {
        return false;
    }
    session.cur_len = new_block.len();
    lines.splice(start..end, new_block);
    true
}

/// Move one edge by `steps` logical pixels, positive being outwards. Rejects a
/// step that would leave the glyph smaller than one logical pixel.
pub(crate) fn nudge(
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    side: ResizeSide,
    steps: i16,
) -> bool {
    let Some(session) = state.resize.as_mut() else {
        return false;
    };
    let mut deltas = session.deltas;
    match side {
        ResizeSide::Left => deltas.left += steps,
        ResizeSide::Right => deltas.right += steps,
        ResizeSide::Top => deltas.top += steps,
        ResizeSide::Bottom => deltas.bottom += steps,
    }
    let prev = std::mem::replace(&mut session.deltas, deltas);
    // A canvas needs a cell to be a canvas; a box may be empty — `advance 0` is
    // what every combining mark says — but it may not be inside out.
    let floor = match session.kind {
        ResizeKind::Box => 0,
        ResizeKind::Canvas => 1,
    };
    if session.preview_dims().0 < floor || session.preview_dims().1 < floor {
        session.deltas = prev;
        return false;
    }
    refresh_preview(lines, state)
}

/// Take the previewed resize back out of the document and end the session.
///
/// The mode goes back to the one `F2` was pressed in — unless something else
/// has already moved it, which is the case this is also the safety net for:
/// the preview is uncommitted text no undo entry describes, so anything that
/// switches the mode behind the editor's back, or that is about to take these
/// lines as the file's content, has to drop it here rather than save it by
/// accident.
pub(crate) fn cancel(lines: &mut Vec<DocLine>, state: &mut EditorState) -> bool {
    let Some(session) = state.resize.as_mut() else {
        return false;
    };
    session.deltas = ResizeDeltas::default();
    let return_mode = session.return_mode.clone();
    let changed = refresh_preview(lines, state);
    state.resize = None;
    if matches!(state.mode, EditMode::GlyphResize { .. }) {
        state.mode = return_mode;
    }
    changed
}

/// Leave the mode and hand the resize over to the host. The preview is rolled
/// back first: the host redoes it as part of the one edit it records, so that
/// the glyph's own block and every `ref` that follows it land in a single undo
/// entry.
pub(crate) fn finish(
    doc: &Document,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
) -> Option<ResizeAction> {
    let deltas = state.resize.as_ref()?.deltas;
    let (item_idx, name, kind) = {
        let session = state.resize.as_ref()?;
        (session.item_idx, session.name.clone(), session.kind)
    };
    cancel(lines, state);
    if deltas.is_zero() {
        return None;
    }
    Some(ResizeAction {
        path: doc.path.clone(),
        item_idx,
        glyph_name: name,
        deltas,
        kind,
    })
}

/// Drop the session without touching the document: the lines it snapshotted
/// are no longer the lines it would put back.
pub(crate) fn abandon(state: &mut EditorState) {
    if let Some(session) = state.resize.take() {
        state.mode = session.return_mode;
    }
}

/// The resize a document did *not* ask for: the session's glyph is gone (the
/// document was reloaded or edited out from under it), so the mode has to go
/// with it.
pub(crate) fn still_valid(doc: &Document, state: &EditorState) -> bool {
    let Some(session) = &state.resize else {
        return true;
    };
    matches!(
        doc.items.get(session.item_idx),
        Some(DocumentItem::Glyph { name, .. }) if name.0 == session.name
    )
}

// ---------------------------------------------------------------------------
// Painting and pointer interaction
// ---------------------------------------------------------------------------

/// Half-width of the band around an edge that a press grabs it in, unzoomed.
const GRAB: f32 = 5.0;

/// The handle drawn at the middle of each edge, unzoomed.
const HANDLE: f32 = 7.0;

/// The grab handle of each edge, drawn just *inside* the glyph's box.
///
/// Inside rather than centred on the edge: the grid band is clipped to exactly
/// where the grid starts, so a handle straddling the left edge of a glyph at
/// column 0 loses its outer half to the clip and the sliver that survives is
/// covered by the border stroke. The pointer is tested against a band around
/// the edge either way ([`grabbed_side`]), so the handle only has to be seen.
pub(crate) fn handle_rects(rect: egui::Rect, zoom: f32) -> [(ResizeSide, egui::Rect); 4] {
    // Small glyphs are the norm here, and a handle wider than the glyph would
    // hide it entirely.
    let size = (HANDLE * zoom)
        .min(rect.width() / 3.0)
        .min(rect.height() / 3.0)
        .max(1.0);
    let half = size / 2.0;
    let at =
        |x: f32, y: f32| egui::Rect::from_center_size(egui::pos2(x, y), egui::Vec2::splat(size));
    [
        (ResizeSide::Left, at(rect.left() + half, rect.center().y)),
        (ResizeSide::Right, at(rect.right() - half, rect.center().y)),
        (ResizeSide::Top, at(rect.center().x, rect.top() + half)),
        (
            ResizeSide::Bottom,
            at(rect.center().x, rect.bottom() - half),
        ),
    ]
}

/// The glyph's boundary, with a handle on each edge.
pub(crate) fn draw_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    zoom: f32,
    pal: &crate::editor::colors::Palette,
) {
    let color = pal.pixel_selection;
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(2.0 * zoom, color),
        // Inside, for the same reason the handles are: an outside stroke on
        // the left edge lands entirely in the clipped half of the band.
        egui::epaint::StrokeKind::Inside,
    );
    draw_handles(painter, rect, zoom, color);
}

/// The handles alone, for a boundary that is grabbable but not being dragged.
///
/// This is the whole affordance the backreference shadow has: without it the
/// grid's edge says only that the glyph ends there, and a resize nobody knows
/// how to start is a resize that is not there. Drawn dimmed, since the shadow
/// is up to be *looked* at and four solid squares in the selection colour would
/// read as a selection.
pub(crate) fn draw_grab_hint(
    painter: &egui::Painter,
    rect: egui::Rect,
    zoom: f32,
    pal: &crate::editor::colors::Palette,
) {
    draw_handles(
        painter,
        rect,
        zoom,
        pal.pixel_selection.gamma_multiply(0.55),
    );
}

fn draw_handles(painter: &egui::Painter, rect: egui::Rect, zoom: f32, color: egui::Color32) {
    for (_, handle) in handle_rects(rect, zoom) {
        painter.rect_filled(handle, 0.0, color);
    }
}

/// The cursor a pointer over `rect`'s edges should take, if any: the affordance
/// that says *which way* the edge under it moves.
pub(crate) fn grab_cursor(
    rect: egui::Rect,
    pos: egui::Pos2,
    zoom: f32,
) -> Option<egui::CursorIcon> {
    Some(match grabbed_side(rect, pos, zoom)? {
        ResizeSide::Left | ResizeSide::Right => egui::CursorIcon::ResizeHorizontal,
        ResizeSide::Top | ResizeSide::Bottom => egui::CursorIcon::ResizeVertical,
    })
}

/// The edge a press at `pos` grabs, if any: the nearest one within the grab
/// band, measured only along the axis that edge moves in.
fn grabbed_side(rect: egui::Rect, pos: egui::Pos2, zoom: f32) -> Option<ResizeSide> {
    let grab = GRAB * zoom;
    let outset = rect.expand(grab);
    if !outset.contains(pos) {
        return None;
    }
    let mut best: Option<(ResizeSide, f32)> = None;
    for side in [
        ResizeSide::Left,
        ResizeSide::Right,
        ResizeSide::Top,
        ResizeSide::Bottom,
    ] {
        let d = match side {
            ResizeSide::Left => (pos.x - rect.left()).abs(),
            ResizeSide::Right => (pos.x - rect.right()).abs(),
            ResizeSide::Top => (pos.y - rect.top()).abs(),
            ResizeSide::Bottom => (pos.y - rect.bottom()).abs(),
        };
        if d <= grab && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((side, d));
        }
    }
    best.map(|(side, _)| side)
}

/// What a drag with no session yet would start, and what it needs to do it.
///
/// This is how the *canvas* is resized now that `F2` drags the box: the grid's
/// edges are grabbable while the backreference shadow is up, and the session
/// only begins once the drag has a whole logical pixel to show for itself — so
/// a stray press on the border leaves the mode it was in, and the mode switch
/// is something the user sees happen because of a change they made.
pub(crate) struct CanvasStart<'a> {
    pub doc: &'a Document,
    pub env: ResolveEnv<'a>,
    pub meta: crate::meta::FontMetrics,
    pub item_idx: usize,
}

/// Drag one edge of the glyph. The grabbed edge is latched at press time: the
/// boundary follows the pointer, so by the next frame the pointer is no longer
/// on the edge it grabbed.
#[expect(clippy::too_many_arguments)]
pub(crate) fn handle_drag(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    needs_rederive: &mut bool,
    rect: egui::Rect,
    grid_cell: f32,
    zoom: f32,
    start: Option<CanvasStart<'_>>,
) {
    let side_id = state.key(crate::editor::Slot::ResizeDragSide);
    let accum_id = state.key(crate::editor::Slot::ResizeDragAccum);
    if !ui.input(|i| i.pointer.primary_down()) {
        ui.ctx().data_mut(|d| {
            d.remove::<ResizeSide>(side_id);
            d.remove::<egui::Vec2>(accum_id);
        });
        return;
    }
    if ui.input(|i| i.pointer.primary_pressed())
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        match grabbed_side(rect, origin, zoom) {
            Some(side) => {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(side_id, side);
                    d.insert_temp(accum_id, egui::Vec2::ZERO);
                });
                // The frame that presses also carries the pointer's travel
                // *to* the edge, which is not travel of the edge.
                return;
            }
            None => return,
        }
    }
    let Some(side) = ui.ctx().data(|d| d.get_temp::<ResizeSide>(side_id)) else {
        return;
    };

    // One logical pixel on screen: the grid draws raster cells, and a resize
    // moves whole logical ones. Before the session exists the scale comes from
    // the glyph the drag is about to start on, or the first step would be a
    // subcell on a `scale N` glyph.
    let scale = match (&state.resize, &start) {
        (Some(session), _) => session.body.scale.max(1),
        (None, Some(start)) => match start.doc.items.get(start.item_idx) {
            Some(DocumentItem::Glyph { body, .. }) => body.scale.max(1),
            _ => 1,
        },
        (None, None) => 1,
    } as f32;
    let step_px = grid_cell * scale;
    let mut accum = ui
        .ctx()
        .data(|d| d.get_temp::<egui::Vec2>(accum_id))
        .unwrap_or_default();
    accum += ui.input(|i| i.pointer.delta());

    let along = match side {
        ResizeSide::Left | ResizeSide::Right => accum.x,
        ResizeSide::Top | ResizeSide::Bottom => accum.y,
    };
    let cells = (along / step_px).trunc() as i16;
    if cells == 0 {
        ui.ctx().data_mut(|d| d.insert_temp(accum_id, accum));
        return;
    }
    // An edge grows outwards when it moves away from the glyph's middle.
    let steps = match side {
        ResizeSide::Left | ResizeSide::Top => -cells,
        ResizeSide::Right | ResizeSide::Bottom => cells,
    };
    // The first step is what starts a canvas session — see [`CanvasStart`].
    if let Some(start) = start
        && state.resize.is_none()
        && !begin(
            start.doc,
            lines,
            state,
            start.item_idx,
            start.env,
            ResizeKind::Canvas,
            start.meta,
        )
    {
        return;
    }
    let moved = nudge(lines, state, side, steps);
    if moved {
        *needs_rederive = true;
        ui.ctx().request_repaint();
    }
    // Consume the whole cells whether or not the nudge was allowed: a step
    // rejected at the minimum size must not build up a debt that jumps the
    // edge the moment the pointer turns around.
    let consumed = cells as f32 * step_px;
    match side {
        ResizeSide::Left | ResizeSide::Right => accum.x -= consumed,
        ResizeSide::Top | ResizeSide::Bottom => accum.y -= consumed,
    }
    ui.ctx().data_mut(|d| d.insert_temp(accum_id, accum));
}

/// What a click on the resize panel asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PanelAction {
    Apply,
    Cancel,
}

/// The inline tool panel while resizing: Apply and Cancel, and nothing else —
/// no shape palette or layer row, since neither acts on anything here.
pub(crate) fn draw_panel(
    ui: &egui::Ui,
    painter: &egui::Painter,
    panel_x: f32,
    panel_y: f32,
    state: &EditorState,
    click_pos: Option<egui::Pos2>,
    zoom: f32,
) -> (Option<PanelAction>, bool) {
    // Ordinary buttons, so they take their colours from `egui`'s widget
    // visuals rather than from [`crate::editor::colors::Palette`]. The
    // palette's panel colours are deliberately dark in *both* themes — they
    // back glyph swatches drawn over the dark grid — while its text colour
    // follows the theme, and the pair reads as dark-on-dark in light mode.
    let visuals = ui.visuals();
    let font = egui::FontId::proportional(12.0 * zoom);
    let w = 72.0 * zoom;
    let h = 22.0 * zoom;
    let gap = 6.0 * zoom;
    let rounding = 3.0 * zoom;

    let mut action = None;
    let mut consumed = false;
    let mut rects = Vec::new();
    for (i, (label, this)) in [
        ("Apply", PanelAction::Apply),
        ("Cancel", PanelAction::Cancel),
    ]
    .into_iter()
    .enumerate()
    {
        let rect = egui::Rect::from_min_size(
            egui::pos2(panel_x, panel_y + i as f32 * (h + gap)),
            egui::vec2(w, h),
        );
        let hovered = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|p| rect.contains(p));
        // `weak_bg_fill` is what an idle `egui::Button` paints; `bg_fill` is
        // the stronger fill it takes once the pointer is over it.
        let (widget, fill) = if hovered {
            (&visuals.widgets.hovered, visuals.widgets.hovered.bg_fill)
        } else {
            (
                &visuals.widgets.inactive,
                visuals.widgets.inactive.weak_bg_fill,
            )
        };
        rects.push((this, rect, fill, widget.fg_stroke.color));
        painter.rect_filled(rect, rounding, fill);
        painter.rect_stroke(
            rect,
            rounding,
            widget.bg_stroke,
            egui::epaint::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            widget.fg_stroke.color,
        );
        if let Some(cp) = click_pos
            && rect.contains(cp)
        {
            action = Some(this);
            consumed = true;
        }
    }

    // The size the dragged rectangle would end up with, under the buttons, and
    // which rectangle that is — the two are dragged the same way and a glyph
    // where they coincide would otherwise give no clue which one is live.
    if let Some(session) = &state.resize {
        let (w_new, h_new) = session.preview_dims();
        let what = match session.kind {
            ResizeKind::Box => "box",
            ResizeKind::Canvas => "canvas",
        };
        painter.text(
            egui::pos2(panel_x, panel_y + 2.0 * (h + gap)),
            egui::Align2::LEFT_TOP,
            format!("{what} {w_new} × {h_new}"),
            font,
            visuals.weak_text_color(),
        );
    }

    #[cfg(test)]
    crate::editor::harness::capture_resize_buttons(ui.ctx(), state.id(), &rects);

    (action, consumed)
}

#[cfg(test)]
#[path = "glyph_resize_tests.rs"]
mod tests;
