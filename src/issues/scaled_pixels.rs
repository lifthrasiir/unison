//! Logical pixels of a `scale N` grid whose cells do not agree on what the
//! bitmap build should make of them.
//!
//! The bitmap build has no sub-pixels: it lights a logical pixel whole if any
//! of its N × N cells is lit (`render::ttf_builder::contours`), and a pixel it
//! does not light is a hardblank if any of its cells is one — a lit pixel is
//! never one, so a `$$` in it is only a spelling, kept for consistency with
//! the claims around it. Each cell is one of four things —
//! *blank* (clear), *hardblank*, *lit* (the bitmap fill bit) or *unlit* (drawn
//! geometry without it) — and two mixes leave that "any" a guess rather than a
//! decision: **lit with unlit**, where the source drew ink in the pixel and
//! said both that the bitmap has it and that it does not, and **blank with
//! hardblank** in a pixel nothing lights, where it claimed part of the pixel
//! and left the rest open.
//! Every other mix is a decision, so only those two are reported.
//!
//! Only a glyph's own grid is checked. What a composite settles into depends
//! on where its components land and what each overlaps, which no single line
//! of the source states, so a fault found there would have nowhere to point.
//!
//! A `vectoronly` glyph is skipped: the bitmap build draws its geometry as the
//! vector build does, so its cells are never squared off into pixels at all.

use crate::document::{DocumentItem, PixelGrid};

use super::{Cx, Issue, Severity, issue_at};

/// A logical pixel, as `(row, column)`.
type Pixel = (usize, usize);

/// How many pixels of each kind a finding spells out before it only counts.
const LISTED: usize = 3;

pub(super) fn check_scaled_pixels(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    for (doc_idx, doc) in cx.docs.iter().enumerate() {
        for (item_idx, item) in cx.source_items(doc_idx) {
            let DocumentItem::Glyph { name, body } = item else {
                continue;
            };
            let Some(pixels) = body.pixels.as_ref() else {
                continue;
            };
            if body.scale <= 1 || body.vectoronly {
                continue;
            }
            let (lit_unlit, blank_hardblank) = ambiguous_pixels(pixels, body.scale as usize);
            let mut parts = Vec::new();
            if !lit_unlit.is_empty() {
                parts.push(summarize(&lit_unlit, "lit and unlit"));
            }
            if !blank_hardblank.is_empty() {
                parts.push(summarize(&blank_hardblank, "blank and hardblank"));
            }
            if parts.is_empty() {
                continue;
            }
            issues.push(issue_at(
                doc,
                item_idx,
                Severity::Warning,
                format!(
                    "glyph `{}` (scale {}) is ambiguous in the bitmap build: {}",
                    name.0,
                    body.scale,
                    parts.join("; "),
                ),
            ));
        }
    }
}

/// The logical pixels, in row-major order, whose cells mix lit with unlit, and
/// those with no lit cell whose cells mix blank with hardblank. A pixel the
/// grid only partly covers is judged on the cells it has.
fn ambiguous_pixels(grid: &PixelGrid, scale: usize) -> (Vec<Pixel>, Vec<Pixel>) {
    let (width, height) = (grid.width as usize, grid.height as usize);
    let (mut lit_unlit, mut blank_hardblank) = (Vec::new(), Vec::new());
    for pr in 0..height.div_ceil(scale) {
        for pc in 0..width.div_ceil(scale) {
            let (mut blank, mut hardblank, mut lit, mut unlit) = (false, false, false, false);
            for r in pr * scale..((pr + 1) * scale).min(height) {
                for c in pc * scale..((pc + 1) * scale).min(width) {
                    let s = grid.get(r as u16, c as u16);
                    if s.is_bitmap_filled() {
                        lit = true;
                    } else if s.is_hardblank() {
                        hardblank = true;
                    } else if s.is_clear() {
                        blank = true;
                    } else {
                        unlit = true;
                    }
                }
            }
            if lit && unlit {
                lit_unlit.push((pr, pc));
            }
            if blank && hardblank && !lit {
                blank_hardblank.push((pr, pc));
            }
        }
    }
    (lit_unlit, blank_hardblank)
}

/// "N pixels mix WHAT cells (at row R, column C, …, and M more)".
fn summarize(pixels: &[Pixel], what: &str) -> String {
    let count = pixels.len();
    let mut at = pixels
        .iter()
        .take(LISTED)
        .map(|(r, c)| format!("row {r}, column {c}"))
        .collect::<Vec<_>>()
        .join("; ");
    if count > LISTED {
        at.push_str(&format!("; and {} more", count - LISTED));
    }
    let verb = if count == 1 {
        "pixel mixes"
    } else {
        "pixels mix"
    };
    format!("{count} {verb} {what} cells (at {at})")
}
