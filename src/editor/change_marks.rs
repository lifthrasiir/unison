//! What the buffer changed since it was last saved, as marks in the gutter and
//! on the minimap.
//!
//! # A diff, not *the* diff
//!
//! A buffer and the file it was loaded from have many correct diffs, and the
//! marks only have to show one of them: every line marked unchanged really is
//! the saved line it is paired with, the pairing keeps both files' order, and
//! everything else is an addition, a modification or a deletion. Minimal is
//! not a requirement.
//!
//! So the pairing is not searched for. It is read off each line's
//! [`LineId`], which an edit already carries along with the line it rewrites
//! and an undo puts back: the line saved as id 7 is the line that has id 7 now, whatever it says. That
//! is the edit the user actually made — a line they typed into is *modified*,
//! not deleted and added again — and it costs one hash lookup per line where a
//! text diff would cost a quadratic worst case on every keystroke of a
//! twenty-thousand-line file.
//!
//! Two things keep the pairing a diff. An id pairing need not keep order
//! (nothing stops an edit from handing an id to a line elsewhere, or to two
//! lines), so only the
//! longest run of pairs increasing on both sides is kept; the rest count as
//! unpaired. And an unpaired stretch between two pairs — lines rebuilt from
//! text under fresh ids, say — is matched by content from both ends, and what
//! is left of it pairs up in order as modifications, the surplus of either side
//! being the additions or the one deletion. A buffer rewritten wholesale thus
//! still marks only the lines whose text moved.
//!
//! # The saved side
//!
//! What "saved" compares against is a copy of the lines the file holds, taken
//! when the buffer was loaded or reloaded, and when a write lands — the write's
//! own lines, taken when it was serialized, for the reason
//! `crate::app::save` credits a write to the revision it wrote. So the marks
//! say what the file on disk holds, not where the undo stack's saved point is:
//! a write whose redo branch was dropped still compares against what it wrote.
//!
//! # Drawing it
//!
//! A mark covers a buffer line, which the view may draw as several rows — a
//! wrapped line, a heading taller than a row, a grid as tall as its glyph — so
//! [`mark_spans`] walks the visual lines once and hands back each mark's
//! extent along them. The editor and the minimap both draw from it, each with
//! its own row heights.

use std::sync::Arc;

use crate::document::{DocLine, LineId};
use crate::hash::HashMap;

use super::colors::Palette;
use super::document_view::{VLineKind, VisualLine};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineChange {
    Unchanged,
    Added,
    Modified,
}

/// One buffer's changes against its saved lines.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ChangeMarks {
    /// One per buffer line.
    lines: Vec<LineChange>,
    /// Buffer lines that saved lines were deleted in front of, ascending;
    /// `lines.len()` for a deletion at the end of the file.
    deleted_before: Vec<usize>,
}

impl ChangeMarks {
    pub(crate) fn line(&self, line: usize) -> LineChange {
        self.lines
            .get(line)
            .copied()
            .unwrap_or(LineChange::Unchanged)
    }

    pub(crate) fn deleted_before(&self) -> &[usize] {
        &self.deleted_before
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.deleted_before.is_empty() && self.lines.iter().all(|&c| c == LineChange::Unchanged)
    }
}

/// `current` against `saved`; `index` maps each saved line's id to its index.
fn diff(saved: &[DocLine], index: &HashMap<LineId, usize>, current: &[DocLine]) -> ChangeMarks {
    let pairs: Vec<(usize, usize)> = current
        .iter()
        .enumerate()
        .filter_map(|(i, line)| index.get(&line.line_id()).map(|&j| (i, j)))
        .collect();
    let mut marks = ChangeMarks {
        lines: vec![LineChange::Unchanged; current.len()],
        deleted_before: Vec::new(),
    };
    let (mut ci, mut sj) = (0, 0);
    let end = (current.len(), saved.len());
    for (i, j) in increasing_pairs(pairs).into_iter().chain([end]) {
        marks.mark_gap(&saved[sj..j], &current[ci..i], ci);
        if i < current.len() && current[i] != saved[j] {
            marks.lines[i] = LineChange::Modified;
        }
        (ci, sj) = (i + 1, j + 1);
    }
    marks
}

impl ChangeMarks {
    /// The unpaired stretch between two pairs: `current` stands where `saved`
    /// stood, and starts at buffer line `at`. See the module docs.
    fn mark_gap(&mut self, saved: &[DocLine], current: &[DocLine], at: usize) {
        let prefix = saved
            .iter()
            .zip(current)
            .take_while(|(a, b)| a == b)
            .count();
        let (saved, current) = (&saved[prefix..], &current[prefix..]);
        let suffix = saved
            .iter()
            .rev()
            .zip(current.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let (saved, current) = (
            &saved[..saved.len() - suffix],
            &current[..current.len() - suffix],
        );
        let at = at + prefix;
        let paired = saved.len().min(current.len());
        for k in 0..current.len() {
            self.lines[at + k] = if k < paired {
                LineChange::Modified
            } else {
                LineChange::Added
            };
        }
        if saved.len() > paired {
            self.deleted_before.push(at + paired);
        }
    }
}

/// The longest run of `pairs` (already increasing in the first index) that
/// also strictly increases in the second. The buffer's lines are nearly always
/// in their saved order, so that case is checked for first and costs one pass.
fn increasing_pairs(pairs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if pairs.windows(2).all(|w| w[0].1 < w[1].1) {
        return pairs;
    }
    // Patience sorting: `tails[k]` is the pair ending the best run of length
    // `k + 1` found so far, `prev` the pair before each one in its run.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];
    for (k, &(_, j)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| pairs[t].1 < j);
        // Of two lines under one saved id, the first stays the saved one and
        // the copy is what was added.
        if tails.get(pos).is_some_and(|&t| pairs[t].1 == j) {
            continue;
        }
        prev[k] = pos.checked_sub(1).map(|p| tails[p]);
        if pos == tails.len() {
            tails.push(k);
        } else {
            tails[pos] = k;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(k) = at {
        run.push(pairs[k]);
        at = prev[k];
    }
    run.reverse();
    run
}

/// What the marks were computed from: the saved lines (replaced wholesale, so
/// they drop the cache themselves) and, standing in for the buffer, every
/// counter an edit of it steps. The undo stack's revision is the one a run of
/// typing moves; the document's generations cover what reaches the buffer
/// through a rederive instead, such as a resize preview.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct BufferRevision {
    pub(crate) undo: u64,
    pub(crate) edit_gen: u64,
    pub(crate) pixel_gen: u64,
    pub(crate) len: usize,
}

struct Saved {
    lines: Arc<[DocLine]>,
    /// Built on the first diff rather than on every load and save.
    index: Option<HashMap<LineId, usize>>,
}

/// An editor's saved lines and the marks last computed against them.
#[derive(Default)]
pub(crate) struct ChangeTracker {
    saved: Option<Saved>,
    cache: Option<(BufferRevision, Arc<ChangeMarks>)>,
}

impl ChangeTracker {
    /// Records `lines` as what the file holds now.
    pub(crate) fn set_saved(&mut self, lines: Arc<[DocLine]>) {
        self.saved = Some(Saved { lines, index: None });
        self.cache = None;
    }

    /// The buffer's marks as of `revision`. An editor that was never told what
    /// its file holds takes the first buffer it is shown as saved, which is
    /// what an editor built straight from text (a test, a scratch pane) means.
    pub(crate) fn marks(
        &mut self,
        lines: &[DocLine],
        revision: BufferRevision,
    ) -> Arc<ChangeMarks> {
        if let Some((key, marks)) = &self.cache
            && *key == revision
        {
            return Arc::clone(marks);
        }
        let saved = self.saved.get_or_insert_with(|| Saved {
            lines: lines.into(),
            index: None,
        });
        let index = saved.index.get_or_insert_with(|| {
            let mut index =
                HashMap::with_capacity_and_hasher(saved.lines.len(), Default::default());
            for (j, line) in saved.lines.iter().enumerate() {
                index.insert(line.line_id(), j);
            }
            index
        });
        let marks = Arc::new(diff(&saved.lines, index, lines));
        self.cache = Some((revision, Arc::clone(&marks)));
        marks
    }
}

/// The buffer line a visual line draws, if it draws one: a text line's own, a
/// grid row's grid. A ref-only glyph's composite rows draw no line of the
/// buffer, and neither does a reference chart strip.
fn source_line(vl: &VisualLine, lines: &[DocLine]) -> Option<usize> {
    match &vl.kind {
        VLineKind::Text(_) => Some(vl.doc_line),
        VLineKind::GridRow { grid_doc_line, .. } => {
            matches!(lines.get(*grid_doc_line), Some(DocLine::Grid(_))).then_some(*grid_doc_line)
        }
        VLineKind::RefImage { .. } => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkKind {
    Added,
    Modified,
    Deleted,
}

/// One mark's extent along the visual lines, from the top of the first.
/// A deletion has none: `y0 == y1` is the boundary it sits on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MarkSpan {
    pub(crate) kind: MarkKind,
    pub(crate) y0: f32,
    pub(crate) y1: f32,
}

/// Where `marks` fall along `vlines`, each visual line `height(i, vl)` tall.
///
/// A changed line covers every row that draws it, and neighbouring lines with
/// the same change are one span. A deletion sits on the top edge of the first
/// row drawing a line at or after the one it was deleted in front of — below a
/// chart strip, which belongs to no line, and past a folded group, whose
/// lines are not drawn — or on the bottom of the last row at the end.
pub(crate) fn mark_spans(
    vlines: &[VisualLine],
    lines: &[DocLine],
    marks: &ChangeMarks,
    mut height: impl FnMut(usize, &VisualLine) -> f32,
) -> Vec<MarkSpan> {
    let mut spans: Vec<MarkSpan> = Vec::new();
    if marks.is_clean() {
        return spans;
    }
    let deleted = marks.deleted_before();
    let mut next_deleted = 0;
    let mut y = 0.0f32;
    let mut end = 0.0f32;
    for (i, vl) in vlines.iter().enumerate() {
        let h = height(i, vl);
        if let Some(line) = source_line(vl, lines) {
            while deleted.get(next_deleted).is_some_and(|&b| b <= line) {
                spans.push(MarkSpan {
                    kind: MarkKind::Deleted,
                    y0: y,
                    y1: y,
                });
                next_deleted += 1;
            }
            let kind = match marks.line(line) {
                LineChange::Unchanged => None,
                LineChange::Added => Some(MarkKind::Added),
                LineChange::Modified => Some(MarkKind::Modified),
            };
            if let Some(kind) = kind {
                match spans.last_mut() {
                    Some(last) if last.kind == kind && last.y1 == y => last.y1 = y + h,
                    _ => spans.push(MarkSpan {
                        kind,
                        y0: y,
                        y1: y + h,
                    }),
                }
            }
            end = y + h;
        }
        y += h;
    }
    if next_deleted < deleted.len() {
        spans.push(MarkSpan {
            kind: MarkKind::Deleted,
            y0: end,
            y1: end,
        });
    }
    spans
}

/// Paints `marks` in the gap between the line numbers and the text, `gap`
/// being its `(left, right)`, for a document whose top is at `top` and which
/// is `total_height` tall. Returns what was painted, for the harness.
///
/// A changed line is a bar down the middle third of the gap, as tall as the
/// rows drawing it. A deletion is a triangle pointing at the text, half the
/// gap wide and flush with its right edge, centered on the boundary it sits
/// on — held inside the document at either end, where half of it would
/// otherwise be clipped away.
///
/// `spans` are [`mark_spans`] of the view, which the caller keeps for as long
/// as neither the view nor the marks change: working them out walks every
/// visual line, and this runs every frame.
pub(crate) fn paint_gutter_marks(
    painter: &egui::Painter,
    clip: egui::Rect,
    spans: &[MarkSpan],
    (left, right): (f32, f32),
    top: f32,
    total_height: f32,
    pal: &Palette,
) -> Vec<(MarkKind, egui::Rect)> {
    let ppp = painter.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let width = right - left;
    let (bar_left, bar_right) = (snap(left + width / 3.0), snap(left + width * 2.0 / 3.0));
    let bar_right = bar_right.max(bar_left + 1.0 / ppp);
    let (tri_w, tri_h) = (width / 2.0, width);
    let mut painted = Vec::new();
    for span in spans {
        let rect = match span.kind {
            MarkKind::Added | MarkKind::Modified => egui::Rect::from_x_y_ranges(
                bar_left..=bar_right,
                snap(top + span.y0)..=snap(top + span.y1),
            ),
            MarkKind::Deleted => {
                let cy = if total_height >= tri_h {
                    span.y0.clamp(tri_h / 2.0, total_height - tri_h / 2.0)
                } else {
                    span.y0
                };
                egui::Rect::from_center_size(
                    egui::pos2(right - tri_w / 2.0, top + cy),
                    egui::vec2(tri_w, tri_h),
                )
            }
        };
        if !rect.intersects(clip) {
            continue;
        }
        painter.add(match span.kind {
            MarkKind::Added => egui::Shape::rect_filled(rect, 0.0, pal.change_added),
            MarkKind::Modified => egui::Shape::rect_filled(rect, 0.0, pal.change_modified),
            MarkKind::Deleted => egui::Shape::convex_polygon(
                vec![
                    rect.left_top(),
                    egui::pos2(rect.right(), rect.center().y),
                    rect.left_bottom(),
                ],
                pal.change_deleted,
                egui::Stroke::NONE,
            ),
        });
        painted.push((span.kind, rect));
    }
    painted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::parse_doclines;

    fn text(s: &str) -> DocLine {
        DocLine::text(s.to_string())
    }

    fn marks_of(saved: &[DocLine], current: &[DocLine]) -> Arc<ChangeMarks> {
        let mut tracker = ChangeTracker::default();
        tracker.set_saved(saved.into());
        let revision = BufferRevision {
            undo: 0,
            edit_gen: 0,
            pixel_gen: 0,
            len: current.len(),
        };
        tracker.marks(current, revision)
    }

    fn changes(marks: &ChangeMarks) -> Vec<LineChange> {
        marks.lines.clone()
    }

    use LineChange::{Added as A, Modified as M, Unchanged as U};

    #[test]
    fn an_untouched_buffer_is_clean() {
        let saved = parse_doclines("a\nb\nc\n");
        assert!(marks_of(&saved, &saved).is_clean());
    }

    #[test]
    fn a_line_typed_into_is_modified_not_replaced() {
        let saved = parse_doclines("a\nb\nc\n");
        let mut current = saved.clone();
        current[1].as_text_mut().unwrap().push('!');
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [U, M, U]);
        assert!(marks.deleted_before().is_empty());
    }

    #[test]
    fn a_line_typed_back_to_what_was_saved_is_unchanged() {
        let saved = parse_doclines("a\nb\n");
        let mut current = saved.clone();
        current[1].as_text_mut().unwrap().push('!');
        current[1].as_text_mut().unwrap().pop();
        assert!(marks_of(&saved, &current).is_clean());
    }

    #[test]
    fn inserted_and_deleted_lines() {
        let saved = parse_doclines("a\nb\nc\nd\n");
        let mut current = saved.clone();
        current.remove(2); // c
        current.insert(1, text("new"));
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [U, A, U, U]);
        // `c` went from between `b` (now 2) and `d` (now 3).
        assert_eq!(marks.deleted_before(), [3]);
    }

    #[test]
    fn deletions_at_either_end() {
        let saved = parse_doclines("a\nb\nc\n");
        let current = saved[1..2].to_vec();
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [U]);
        assert_eq!(marks.deleted_before(), [0, 1]);
    }

    /// A line rebuilt from text under a fresh id still pairs with the saved
    /// line it stands in for, by content or else in order.
    #[test]
    fn fresh_ids_pair_by_content_then_in_order() {
        let saved = parse_doclines("a\nb\nc\nd\ne\n");
        let mut current = saved.clone();
        current[1] = text("b");
        current[2] = text("C");
        current[3] = text("D");
        current.insert(4, text("extra"));
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [U, U, M, M, A, U]);
        assert!(marks.deleted_before().is_empty());

        let current = vec![saved[0].clone(), text("X"), saved[4].clone()];
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [U, M, U]);
        assert_eq!(marks.deleted_before(), [2]);
    }

    /// A line moved up under its own id breaks the order; only one side of
    /// the move can stay paired, and the other shows as the move it is.
    #[test]
    fn a_moved_line_is_a_deletion_and_an_addition() {
        let saved = parse_doclines("a\nb\nc\nd\n");
        let current = vec![
            saved[3].clone(),
            saved[0].clone(),
            saved[1].clone(),
            saved[2].clone(),
        ];
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [A, U, U, U]);
        assert_eq!(marks.deleted_before(), [4]);
    }

    #[test]
    fn a_duplicated_id_pairs_once() {
        let saved = parse_doclines("a\nb\n");
        let current = vec![saved[0].clone(), saved[0].clone(), saved[1].clone()];
        let marks = marks_of(&saved, &current);
        assert_eq!(changes(&marks), [U, A, U]);
    }

    #[test]
    fn increasing_pairs_takes_the_longest_run() {
        let pairs = vec![(0, 3), (1, 0), (2, 1), (3, 4), (4, 2), (5, 5)];
        assert_eq!(increasing_pairs(pairs), [(1, 0), (2, 1), (4, 2), (5, 5)]);
        assert_eq!(increasing_pairs(Vec::new()), []);
    }

    #[test]
    fn the_cache_follows_the_revision_and_the_saved_lines() {
        let saved: Vec<DocLine> = parse_doclines("a\nb\n");
        let mut tracker = ChangeTracker::default();
        tracker.set_saved(saved.clone().into());
        let rev = |undo| BufferRevision {
            undo,
            edit_gen: 0,
            pixel_gen: 0,
            len: 2,
        };
        let mut current = saved.clone();
        assert!(tracker.marks(&current, rev(0)).is_clean());
        current[0].as_text_mut().unwrap().push('!');
        assert!(
            tracker.marks(&current, rev(0)).is_clean(),
            "same revision, same answer"
        );
        assert!(!tracker.marks(&current, rev(1)).is_clean());
        tracker.set_saved(current.clone().into());
        assert!(tracker.marks(&current, rev(1)).is_clean());
    }
}
