//! Folding: collapsing a run of consecutive lines down to the first of them.
//!
//! # The model
//!
//! A [`FoldGroup`] is a *header* line — always visible, and the only line that
//! carries a marker in the gutter — followed by one or more member lines. A
//! collapsed group hides its members: they are dropped from the visual lines,
//! so nothing paints them, the minimap does not draw them and no caret can
//! land on one. The header is never hidden by its own group.
//!
//! Only one kind of group exists today, [`fold_groups`]'s glyph blocks (a
//! `glyph` header plus its grid and `ref`/`anchor` lines), but nothing below
//! that function assumes it: groups are returned sorted by header with the
//! *outer* one first at a tie, and every query here walks the list rather than
//! indexing it, so nested and interleaved kinds only need a longer
//! [`fold_groups`].
//!
//! # Why the group list is not recomputed every frame
//!
//! Groups come from the *derived* `Document` (`items` + `item_line_starts`),
//! not from the raw text, so the editor's deferred-reparse rule
//! (`document_view::changes::apply_pending_rederive`) decides when they change:
//! while the caret sits on a glyph header the document is not re-derived, so a
//! header edited into something that is no longer a header keeps its group —
//! and its fold — until the caret leaves the line or the editor loses focus.
//! That is precisely the behaviour folding wants, and [`FoldState::sync`] gets
//! it for free by keying off `Document::edit_gen`.
//!
//! # Why a tall glyph starts folded
//!
//! A `scale N` glyph's grid is stored in *subcells* — `document_io` multiplies
//! the declared dimensions by the scale — and the editor draws one cell per
//! subcell, so a `16 16 scale 4` block is 64 rows of grid where an ordinary
//! glyph is 16. A handful of those turn a file into something that has to be
//! scrolled past rather than read, so [`FoldState::apply_initial`] collapses
//! every glyph whose grid draws taller than twice the font height, once, when
//! the buffer is first laid out.
//!
//! The font height it measures against is whatever `meta` says on that first
//! frame, which before the background derive lands is
//! [`crate::meta::FontMetrics::DEFAULT_HEIGHT`]. That is deliberate: the
//! default is the height nearly every source states anyway, and waiting for the
//! derive would leave a file on a slow share unfolded for as long as the build
//! takes and then rearrange it under the reader. A source that declares an
//! unusual `meta height` and is opened before its first derive can therefore
//! fold by the default rather than by its own height — a fold the reader can
//! undo, against a rearranging page they cannot.
//!
//! # Why collapsed groups are re-found by header text
//!
//! [`FoldState`] stores line *indices*, because that is what everything
//! downstream — hit tests, caret snapping, the marker rects — asks about. Line
//! indices do not survive an edit above them, so each entry also remembers the
//! header's text and [`FoldState::sync`] re-finds it in the new group list,
//! nearest match first. An insertion or deletion elsewhere in the file
//! therefore carries the fold along, and a header rewritten into a different
//! header (or into no header at all) drops it — which is the same "the
//! grouping broke" outcome as the group disappearing outright.

use super::caret::{self, Caret};
use crate::document::{DocLine, Document, DocumentItem};

/// A foldable run of lines: `header` is shown always, `header + 1 .. end` are
/// what collapsing hides. `end` is always greater than `header + 1` — a group
/// with nothing to hide is not built in the first place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FoldGroup {
    pub(crate) header: usize,
    pub(crate) end: usize,
}

impl FoldGroup {
    /// Whether collapsing this group would hide `line`.
    pub(crate) fn hides(&self, line: usize) -> bool {
        line > self.header && line < self.end
    }

    /// Whether `line` belongs to this group at all, header included.
    pub(crate) fn contains(&self, line: usize) -> bool {
        line >= self.header && line < self.end
    }
}

/// Every group the document currently offers, sorted by header ascending and,
/// at equal headers, outermost first.
///
/// A `glyph` item owns the lines from its own start up to the next item's, so
/// its members are the optional grid line and the `ref`/`anchor` lines after
/// it. Items that are a single line are not foldable.
pub(crate) fn fold_groups(doc: &Document, lines: &[DocLine]) -> Vec<FoldGroup> {
    let starts = &doc.item_line_starts;
    let mut groups = Vec::new();
    for (idx, item) in doc.items.iter().enumerate() {
        if !matches!(item, DocumentItem::Glyph { .. }) {
            continue;
        }
        let Some(&header) = starts.get(idx) else {
            continue;
        };
        let end = starts
            .get(idx + 1)
            .copied()
            .unwrap_or(lines.len())
            .min(lines.len());
        if end > header + 1 {
            groups.push(FoldGroup { header, end });
        }
    }
    groups.sort_by(|a, b| a.header.cmp(&b.header).then(b.end.cmp(&a.end)));
    groups
}

#[derive(Clone)]
struct Collapsed {
    group: FoldGroup,
    /// The header line's text when the fold was made, so [`FoldState::sync`]
    /// can find the same header again after lines shifted around it.
    text: String,
}

/// Which of a document's [`FoldGroup`]s are collapsed, in one editor.
///
/// Per-pane state, like the caret: two editors over the same file fold
/// independently, and nothing here is persisted between runs.
#[derive(Default)]
pub(crate) struct FoldState {
    collapsed: Vec<Collapsed>,
    /// The groups `collapsed` was last reconciled against.
    groups: Vec<FoldGroup>,
    /// `Document::edit_gen` the group list was built from. `None` forces the
    /// next [`FoldState::sync`] to rebuild.
    synced_edit_gen: Option<u64>,
    /// Bumped whenever the *visible* set of lines changes, so the view cache
    /// (which is keyed on it) rebuilds.
    visible_gen: u64,
    /// Whether [`FoldState::apply_initial`] has run for this buffer. Reset by
    /// [`FoldState::clear`], so a buffer replaced wholesale is folded afresh.
    initial_applied: bool,
}

/// How tall a glyph's own grid may draw, as a multiple of the font height,
/// before [`FoldState::apply_initial`] starts it collapsed.
const INITIAL_FOLD_HEIGHT_RATIO: usize = 2;

impl FoldState {
    /// Re-derives the group list when the document changed under it, and
    /// carries the collapsed set over to the new lines.
    ///
    /// Cheap and idempotent: while `edit_gen` stands still — which includes
    /// every frame of a deferred reparse — this does nothing at all.
    pub(crate) fn sync(&mut self, doc: &Document, lines: &[DocLine]) {
        if self.synced_edit_gen == Some(doc.edit_gen) {
            return;
        }
        self.synced_edit_gen = Some(doc.edit_gen);
        self.groups = fold_groups(doc, lines);
        if self.collapsed.is_empty() {
            return;
        }

        let before: Vec<FoldGroup> = self.collapsed.iter().map(|c| c.group).collect();
        let mut taken = vec![false; self.groups.len()];
        let mut kept = Vec::with_capacity(self.collapsed.len());
        for entry in std::mem::take(&mut self.collapsed) {
            let best = self
                .groups
                .iter()
                .enumerate()
                .filter(|(i, g)| {
                    !taken[*i]
                        && lines.get(g.header).and_then(DocLine::as_text)
                            == Some(entry.text.as_str())
                })
                .min_by_key(|(_, g)| g.header.abs_diff(entry.group.header));
            if let Some((i, &group)) = best {
                taken[i] = true;
                kept.push(Collapsed { group, ..entry });
            }
        }
        kept.sort_by_key(|c| c.group.header);
        self.collapsed = kept;
        if before.len() != self.collapsed.len()
            || before
                .iter()
                .zip(self.collapsed.iter())
                .any(|(a, b)| *a != b.group)
        {
            self.visible_gen += 1;
        }
    }

    /// Collapses the glyph blocks that are too tall to start open, once per
    /// buffer. Reports whether anything was collapsed.
    ///
    /// Must run after [`FoldState::sync`], whose group list it picks from.
    /// `opened_at` is a line the editor was deliberately opened *at* (a search
    /// hit, a followed link): the group holding it is left open, because a fold
    /// over the very thing the user asked to see is not a saving.
    pub(crate) fn apply_initial(
        &mut self,
        doc: &Document,
        lines: &[DocLine],
        meta: crate::meta::FontMetrics,
        opened_at: Option<usize>,
    ) -> bool {
        if self.initial_applied {
            return false;
        }
        self.initial_applied = true;
        let limit = meta.height() as usize * INITIAL_FOLD_HEIGHT_RATIO;
        let tall: Vec<FoldGroup> = doc
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| grid_display_height(item) > limit)
            .filter_map(|(idx, _)| doc.item_line_starts.get(idx))
            .filter_map(|&header| self.groups.iter().find(|g| g.header == header).copied())
            .filter(|g| opened_at.is_none_or(|line| !g.contains(line)))
            .collect();
        for group in &tall {
            self.toggle(lines, *group);
        }
        !tall.is_empty()
    }

    /// The groups of the document as last synced.
    pub(crate) fn groups(&self) -> &[FoldGroup] {
        &self.groups
    }

    /// A counter that changes exactly when the set of visible lines does.
    pub(crate) fn visible_gen(&self) -> u64 {
        self.visible_gen
    }

    pub(crate) fn is_collapsed(&self, header: usize) -> bool {
        self.collapsed.iter().any(|c| c.group.header == header)
    }

    /// Whether `line` is hidden by some collapsed group.
    pub(crate) fn is_hidden(&self, line: usize) -> bool {
        self.collapsed.iter().any(|c| c.group.hides(line))
    }

    /// The outermost collapsed group hiding `line`.
    fn hiding(&self, line: usize) -> Option<FoldGroup> {
        self.collapsed
            .iter()
            .filter(|c| c.group.hides(line))
            .map(|c| c.group)
            .min_by_key(|g| g.header)
    }

    /// The first visible line at or after `line`. Loops because an outer
    /// group may hide the line an inner one ends on.
    pub(crate) fn snap_down(&self, line: usize) -> usize {
        let mut at = line;
        for _ in 0..=self.collapsed.len() {
            match self.hiding(at) {
                Some(g) => at = g.end,
                None => break,
            }
        }
        at
    }

    /// The first visible line at or before `line`.
    pub(crate) fn snap_up(&self, line: usize) -> usize {
        let mut at = line;
        for _ in 0..=self.collapsed.len() {
            match self.hiding(at) {
                Some(g) => at = g.header,
                None => break,
            }
        }
        at
    }

    /// Pulls a caret that landed on a hidden line onto the nearest visible one
    /// in the direction the move was going, keeping the column where that
    /// makes sense (a vertical move) and going to the line's edge where it
    /// does not (a horizontal one).
    pub(crate) fn snap_caret(&self, lines: &[DocLine], c: Caret, dir: Snap) -> Caret {
        if !self.is_hidden(c.line) {
            return c;
        }
        match dir {
            Snap::Up => {
                let line = self.snap_up(c.line);
                Caret::new(line, c.col.min(caret::line_char_len(lines, line)))
            }
            Snap::Down => {
                let line = self.snap_down(c.line).min(lines.len().saturating_sub(1));
                Caret::new(line, c.col.min(caret::line_char_len(lines, line)))
            }
            Snap::Backward => {
                let line = self.snap_up(c.line);
                Caret::new(line, caret::line_char_len(lines, line))
            }
            Snap::Forward => {
                let line = self.snap_down(c.line);
                if line >= lines.len() {
                    let last = self.snap_up(lines.len().saturating_sub(1));
                    Caret::new(last, caret::line_char_len(lines, last))
                } else {
                    Caret::new(line, 0)
                }
            }
        }
    }

    /// The innermost group `line` belongs to — the fold the toggle shortcut
    /// acts on. A collapsed group counts even from its header alone.
    pub(crate) fn innermost_at(&self, line: usize) -> Option<FoldGroup> {
        self.groups
            .iter()
            .copied()
            .filter(|g| g.contains(line) || (self.is_collapsed(g.header) && g.header == line))
            .max_by_key(|g| g.header)
    }

    /// Collapses or expands `group`, reporting what it became. `None` when
    /// the group is not one this document offers.
    pub(crate) fn toggle(&mut self, lines: &[DocLine], group: FoldGroup) -> Option<bool> {
        if !self.groups.contains(&group) {
            return None;
        }
        self.visible_gen += 1;
        if let Some(pos) = self.collapsed.iter().position(|c| c.group == group) {
            self.collapsed.remove(pos);
            Some(false)
        } else {
            let text = lines
                .get(group.header)
                .and_then(DocLine::as_text)
                .unwrap_or_default()
                .to_string();
            self.collapsed.push(Collapsed { group, text });
            self.collapsed.sort_by_key(|c| c.group.header);
            Some(true)
        }
    }

    /// Expands every collapsed group hiding `line`, so a jump from outside the
    /// editor (a Ctrl-click, a search hit) lands somewhere it can be seen.
    /// Reports whether anything was expanded.
    pub(crate) fn expand_containing(&mut self, line: usize) -> bool {
        let before = self.collapsed.len();
        self.collapsed.retain(|c| !c.group.hides(line));
        if self.collapsed.len() != before {
            self.visible_gen += 1;
            true
        } else {
            false
        }
    }

    /// Drops every fold, for a buffer that was replaced wholesale.
    pub(crate) fn clear(&mut self) {
        if !self.collapsed.is_empty() {
            self.visible_gen += 1;
        }
        self.collapsed.clear();
        self.groups.clear();
        self.synced_edit_gen = None;
        self.initial_applied = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::{derive_document, parse_doclines};

    /// A glyph block whose grid is `h` subcell rows tall, written the way the
    /// parser wants it: two characters per subcell, and `scale` folded into the
    /// stored dimensions by `derive_document`.
    fn glyph(name: &str, w: u16, h: u16, scale: u16) -> String {
        let mut out = format!("glyph {name} {w} {h}");
        if scale > 1 {
            out.push_str(&format!(" scale {scale}"));
        }
        out.push('\n');
        for _ in 0..h * scale {
            out.push_str(&".".repeat(w as usize * scale as usize * 2));
            out.push('\n');
        }
        out
    }

    fn state_for(source: &str) -> (Document, Vec<DocLine>, FoldState) {
        let lines = parse_doclines(source);
        let (doc, _) = derive_document(&lines, "test.unf".into()).expect("derive");
        let mut folds = FoldState::default();
        folds.sync(&doc, &lines);
        (doc, lines, folds)
    }

    /// Default metrics: height 16, so the limit is 32 subcell rows.
    fn meta() -> crate::meta::FontMetrics {
        crate::meta::FontMetrics::default()
    }

    #[test]
    fn a_grid_taller_than_twice_the_font_height_starts_collapsed() {
        // A grid is one `DocLine`, so the headers are lines 0, 2 and 4.
        let src = glyph("small", 2, 16, 1) + &glyph("limit", 2, 16, 2) + &glyph("tall", 2, 16, 4);
        let (doc, lines, mut folds) = state_for(&src);
        assert!(folds.apply_initial(&doc, &lines, meta(), None));
        assert!(!folds.is_collapsed(0), "an ordinary glyph stays open");
        assert!(
            !folds.is_collapsed(2),
            "twice the font height is not more than it"
        );
        assert!(folds.is_collapsed(4));
    }

    #[test]
    fn the_initial_fold_runs_once_and_a_reopened_group_stays_open() {
        let src = glyph("tall", 2, 16, 4);
        let (doc, lines, mut folds) = state_for(&src);
        assert!(folds.apply_initial(&doc, &lines, meta(), None));
        let group = folds.innermost_at(0).expect("group");
        folds.toggle(&lines, group);
        assert!(!folds.is_collapsed(0));

        assert!(!folds.apply_initial(&doc, &lines, meta(), None), "once");
        assert!(!folds.is_collapsed(0));

        // A buffer replaced wholesale is folded afresh.
        folds.clear();
        folds.sync(&doc, &lines);
        assert!(folds.apply_initial(&doc, &lines, meta(), None));
        assert!(folds.is_collapsed(0));
    }

    #[test]
    fn the_group_the_editor_was_opened_at_is_left_open() {
        let src = glyph("tall", 2, 16, 4) + &glyph("tall2", 2, 16, 4);
        let (doc, lines, mut folds) = state_for(&src);
        // Line 1 is the first glyph's grid; 2 is the second one's header.
        assert!(folds.apply_initial(&doc, &lines, meta(), Some(1)));
        assert!(!folds.is_collapsed(0), "the glyph the jump landed in");
        assert!(folds.is_collapsed(2));
    }

    #[test]
    fn a_taller_font_folds_a_grid_that_a_shorter_one_would_not() {
        let src = glyph("tall", 2, 16, 4);
        let big = crate::meta::FontMetrics {
            height: Some(32),
            ..Default::default()
        };
        let (doc, lines, mut folds) = state_for(&src);
        assert!(
            !folds.apply_initial(&doc, &lines, big, None),
            "64 rows is exactly twice a 32-pixel font height"
        );
    }
}

/// How many rows of grid a glyph item draws, in the subcells the editor gives
/// a cell each. Zero for anything that is not a glyph with a grid of its own.
fn grid_display_height(item: &DocumentItem) -> usize {
    match item {
        DocumentItem::Glyph { body, .. } => body.pixels.as_ref().map_or(0, |g| g.height as usize),
        _ => 0,
    }
}

/// Folds or unfolds the innermost group `line` belongs to, and resolves
/// everything else a fold implies. Reports whether `lines` changed.
///
/// A fold is only ever asked for from plain text editing, so any pixel mode is
/// left *first* rather than being taught to survive one: a live resize preview
/// is uncommitted text and is cancelled here, and a floating pixel selection is
/// landed by `pixel_selection::reconcile` on the next frame, exactly as it is
/// for every other way out of an edit mode.
///
/// Collapsing then has to move whatever the fold would have swallowed:
///
/// * A selection with an endpoint on a line that is about to be hidden is
///   dropped — an endpoint must stay somewhere the user can see it. A
///   selection merely *spanning* the group is kept, hidden lines and all,
///   which is what makes select-all-then-fold a no-op.
/// * A caret on a hidden line moves to the header at the same column, clamped.
/// * The header is queued to be scrolled to the top of the viewport, which
///   [`FoldState`]'s consumer applies only if it is off screen.
///
/// Expanding moves nothing and scrolls nothing: the lines appear *below* the
/// header, so leaving the scroll offset alone already keeps the header where
/// it was.
pub(crate) fn toggle_at(
    lines: &mut Vec<DocLine>,
    state: &mut super::EditorState,
    line: usize,
) -> bool {
    let Some(group) = state.folds.innermost_at(line) else {
        return false;
    };

    let mut changed_lines = false;
    if state.resize.is_some() {
        changed_lines |= super::glyph_resize::cancel(lines, state);
    }
    state.mode = super::EditMode::Normal;

    if state.folds.toggle(lines, group) != Some(true) {
        state.fold_scroll = Some(FoldScroll::Hold);
        return changed_lines;
    }

    if let Some((lo, hi)) = state.selection_range()
        && (state.folds.is_hidden(lo.line) || state.folds.is_hidden(hi.line))
    {
        state.selection_anchor = None;
    }
    if state.folds.is_hidden(state.cursor.line) {
        state.cursor = state.folds.snap_caret(lines, state.cursor, Snap::Up);
    }
    state.fold_scroll = Some(FoldScroll::HeaderToTop(group.header));
    changed_lines
}

/// Whether a key pressed this frame moves the caret off the line it is on.
fn leaves_the_line(ui: &egui::Ui) -> bool {
    use egui::Key::*;
    ui.input(|i| {
        [ArrowUp, ArrowDown, PageUp, PageDown]
            .iter()
            .any(|&k| i.key_pressed(k))
            || (i.modifiers.command
                && [ArrowLeft, ArrowRight, Home, End]
                    .iter()
                    .any(|&k| i.key_pressed(k)))
            || [ArrowLeft, ArrowRight].iter().any(|&k| i.key_pressed(k))
    })
}

/// Settles a fold whose header was edited, before the key that leaves the
/// header is acted on.
///
/// A collapsed header keeps its fold while it is being typed in, because the
/// document is not re-derived until the caret leaves the line
/// (`document_view::changes::apply_pending_rederive`) and the group list rides
/// on that. The key that leaves has to see the *new* grouping, though: arrowing
/// down off a `glyph a` just mistyped into `lyph a` must land on the `ref` line
/// the group used to hide, not step over a group that no longer exists. So the
/// pending reparse is flushed here, one stage ahead of the motion, and only in
/// the case that needs it — a caret sitting on a header that is shut.
pub(crate) fn settle_edited_header(
    ui: &egui::Ui,
    doc: &mut Document,
    lines: &mut Vec<DocLine>,
    state: &mut super::EditorState,
) {
    if state.pending_reparse_line != Some(state.cursor.line)
        || !state.folds.is_collapsed(state.cursor.line)
        || !leaves_the_line(ui)
    {
        return;
    }
    super::document_view::flush_document_changes(lines, doc, state);
    state.folds.sync(doc, lines);
}

/// What a fold asks of the scroll offset on the frame after it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldScroll {
    /// A group opened: hold the offset exactly. The rows appear *below* the
    /// header, so holding is all it takes to keep the header where it was —
    /// but the document just got taller, which the saved-fraction restore
    /// would otherwise read as a page that had drifted.
    Hold,
    /// A group closed: bring this line to the top of the page, but only if it
    /// is not on the page already.
    HeaderToTop(usize),
}

/// Which way a caret was moving when it landed on a hidden line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Snap {
    /// A vertical move upward: keep the column, land on the header.
    Up,
    /// A vertical move downward: keep the column, land past the group.
    Down,
    /// A horizontal move leftward: land at the end of the header.
    Backward,
    /// A horizontal move rightward: land at the start of the line past it.
    Forward,
}
