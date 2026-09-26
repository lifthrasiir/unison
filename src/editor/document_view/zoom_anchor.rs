//! Where a zoom leaves the page.
//!
//! A zoom picks one point on the page as its anchor and keeps it at the same
//! height on the screen:
//!
//! 1. the pointer, while it is over the editor;
//! 2. otherwise the middle of the caret's segment, while that is on screen;
//! 3. otherwise the middle of the viewport.
//!
//! The anchor is not carried across as a document y scaled by the zoom ratio.
//! The page does not scale linearly: a reference strip keeps one height at
//! every level, headings grow by steps rather than by a factor, text rewraps
//! against a width that did not grow with it, and the glyph being edited is
//! padded to the height of its side panel. Each of those, summed over
//! everything above the anchor, moved it — the further down the file, the
//! further. So the anchor is recorded as *what* it is on — a grid row, a strip,
//! a text line (all of its wrapped segments together, since the segments are
//! exactly what a zoom redraws), or the caret — and how far down that thing it
//! sits, and is looked up again in the new layout.
//!
//! The old layout is the one the previous frame painted: the view cache still
//! holds it when this runs, and the scroll offset of the previous frame is an
//! offset into it. A zoom can take two layouts to settle — the gutter's width
//! is estimated from the scroll offset the zoom is about to move — so a
//! relayout on the frame right after one is anchored the same way
//! (`show_document`).

use super::layout::{VLineKind, VisualLine, doc_line_to_y};
use crate::editor::caret::Caret;

/// What the anchor sits on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Spot {
    GridRow {
        line: usize,
        row: i16,
    },
    RefImage {
        line: usize,
    },
    /// All text segments of one document line, taken together.
    Text {
        line: usize,
    },
    /// The caret's own segment, found again from the caret.
    Caret,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ZoomAnchor {
    spot: Spot,
    /// How far down `spot` the anchor is, as a fraction of its height.
    frac: f32,
    /// How far down the viewport the anchor was, as a fraction of its height.
    screen_frac: f32,
}

/// The segment of `vlines` the caret is drawn on — the same test the painter
/// applies — with its top.
fn caret_segment(vlines: &[VisualLine], row_h: f32, cell: f32, caret: Caret) -> Option<(f32, f32)> {
    let mut y = 0.0;
    for vl in vlines {
        let h = vl.height(row_h, cell);
        if vl.doc_line == caret.line
            && let VLineKind::Text(text) = &vl.kind
            && caret.col >= vl.col_offset
            && caret.col <= vl.col_offset + text.chars().count()
        {
            return Some((y, h));
        }
        if vl.doc_line > caret.line {
            break;
        }
        y += h;
    }
    None
}

/// The top and height of `spot` in `vlines`, or `None` when the layout holds
/// no such thing.
fn locate(
    vlines: &[VisualLine],
    row_h: f32,
    cell: f32,
    spot: Spot,
    caret: Caret,
) -> Option<(f32, f32)> {
    let line = match spot {
        Spot::Caret => return caret_segment(vlines, row_h, cell, caret),
        Spot::GridRow { line, .. } | Spot::RefImage { line } | Spot::Text { line } => line,
    };
    let is_part = |vl: &VisualLine| match (spot, &vl.kind) {
        (Spot::GridRow { row, .. }, VLineKind::GridRow { row: r, .. }) => *r == row,
        (Spot::RefImage { .. }, VLineKind::RefImage { .. }) => true,
        (Spot::Text { .. }, VLineKind::Text(_)) => true,
        _ => false,
    };
    let mut y = 0.0;
    let mut found: Option<(f32, f32)> = None;
    for vl in vlines {
        if vl.doc_line > line {
            break;
        }
        let h = vl.height(row_h, cell);
        if vl.doc_line == line && is_part(vl) {
            // A text line's segments are contiguous, so they add up to one
            // span; every other spot is a single visual line.
            match &mut found {
                Some((_, total)) => *total += h,
                None => found = Some((y, h)),
            }
        } else if found.is_some() {
            break;
        }
        y += h;
    }
    found
}

/// Records the anchor on the page the previous frame painted: `vlines` laid out
/// with `row_h` and `cell`, scrolled to `scroll_y` in a viewport `viewport_h`
/// tall. `pointer_offset` is the pointer's distance below the viewport's top,
/// when it is over the editor at all; `caret_shown` is whether the caret is
/// drawn in the current mode.
#[expect(clippy::too_many_arguments)]
pub(super) fn capture(
    vlines: &[VisualLine],
    row_h: f32,
    cell: f32,
    scroll_y: f32,
    viewport_h: f32,
    pointer_offset: Option<f32>,
    caret: Caret,
    caret_shown: bool,
) -> Option<ZoomAnchor> {
    if viewport_h <= 0.0 {
        return None;
    }
    let caret_mid = caret_shown
        .then(|| caret_segment(vlines, row_h, cell, caret))
        .flatten()
        .map(|(y, h)| y + h * 0.5 - scroll_y)
        .filter(|&off| (0.0..=viewport_h).contains(&off));
    let offset = match (pointer_offset, caret_mid) {
        (Some(off), _) => off,
        (None, Some(off)) => {
            return Some(ZoomAnchor {
                spot: Spot::Caret,
                frac: 0.5,
                screen_frac: off / viewport_h,
            });
        }
        (None, None) => viewport_h * 0.5,
    };
    let doc_y = scroll_y + offset;
    let mut y = 0.0;
    let mut hit = None;
    for vl in vlines {
        let h = vl.height(row_h, cell);
        if doc_y < y + h {
            hit = Some((vl, y, h));
            break;
        }
        y += h;
    }
    let (vl, y, h) = hit?;
    let spot = match &vl.kind {
        VLineKind::GridRow { row, .. } => Spot::GridRow {
            line: vl.doc_line,
            row: *row,
        },
        VLineKind::RefImage { .. } => Spot::RefImage { line: vl.doc_line },
        VLineKind::Text(_) => Spot::Text { line: vl.doc_line },
    };
    // A text line's fraction is of all its segments, measured the same way
    // `locate` will measure them again.
    let (top, height) = match spot {
        Spot::Text { .. } => locate(vlines, row_h, cell, spot, caret).unwrap_or((y, h)),
        _ => (y, h),
    };
    Some(ZoomAnchor {
        spot,
        frac: if height > 0.0 {
            ((doc_y - top) / height).clamp(0.0, 1.0)
        } else {
            0.0
        },
        screen_frac: offset / viewport_h,
    })
}

impl ZoomAnchor {
    /// The scroll offset that puts the anchor back at its height on the screen
    /// in the new layout. A grid row that is gone — the padding under the glyph
    /// being edited shrinks as the zoom goes down — falls back to the top of
    /// the line the anchor was on.
    pub(super) fn scroll_in(
        &self,
        vlines: &[VisualLine],
        row_h: f32,
        cell: f32,
        viewport_h: f32,
        caret: Caret,
    ) -> Option<f32> {
        let doc_y = match locate(vlines, row_h, cell, self.spot, caret) {
            Some((top, h)) => top + h * self.frac,
            None => match self.spot {
                Spot::GridRow { line, .. } | Spot::RefImage { line } | Spot::Text { line } => {
                    doc_line_to_y(vlines, row_h, cell, line)
                }
                Spot::Caret => return None,
            },
        };
        Some(doc_y - self.screen_frac * viewport_h)
    }
}
