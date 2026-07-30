//! The *anchor shadow*: while an `anchor` layer is the one selected in
//! the subglyph palette, the editor draws every glyph that could attach at that
//! anchor underneath the glyph being edited.
//!
//! Attachment is symmetric, so this makes no distinction between the two signs:
//! a `+above` shows the marks that carry `-above`, a `-above` shows the bases
//! that carry `+above`, and both are found the same way. What is drawn is the
//! *union* of all of them — a cell is inked ([`crate::pixel::PX_FULL`]) when any
//! candidate inks it, and its geometry is the exact union of every candidate's
//! geometry there, which is what [`PixelGrid::blit`] already computes (a cell
//! whose union is no catalog shape becomes a `PX_CUSTOM` detail).
//!
//! Placement mirrors composition exactly — [`crate::ref_composite`]'s
//! `try_match_minus_plus` (anchor delta, *not* scale-converted) plus
//! `ref_effective_offset_scaled` (the target's origin, scale-converted) — so
//! the shadow lands where the glyph really would. Candidates are subject to the
//! same `size_matches` rule composition applies.
//!
//! The shadow is part of `ViewData`, so `ViewCacheKey` carries the selected
//! *anchor* layer. Ref layers are deliberately left out of that key: cycling
//! through them changes nothing the view is built from, and rebuilding it is
//! O(document).

use std::collections::HashMap;

use crate::document::{GlyphPoint, PixelGrid};
use crate::editor::ref_composite::ResolvedGlyph;
use crate::pixel::{PX_ALMOSTFULL, PX_CUSTOM};

/// Rows/columns beyond which a candidate is taken to be misplaced rather than
/// merely far away. Without a bound one stray anchor pair could ask for a grid
/// of millions of cells, all of it off screen.
const MAX_EXTENT: i32 = 1024;

/// The union of every glyph that can attach at the selected anchor, in the
/// edited glyph's own grid coordinates.
#[derive(Clone)]
pub(crate) struct AnchorShadow {
    pub(crate) grid: PixelGrid,
    /// Grid coordinate of the shadow's raster cell `(0, 0)`. Negative when the
    /// shadow reaches above/left of the edited glyph's own grid, exactly as a
    /// composite's `own_offset_*` allows.
    pub(crate) row: i16,
    pub(crate) col: i16,
    /// How many glyphs went into the union. Zero never reaches here — no
    /// candidate means no shadow at all.
    pub(crate) count: usize,
}

/// The anchor on the other side of `position`: `+x` ↔ `-x`. `None` for a name
/// carrying neither sign, which cannot take part in attachment.
pub(crate) fn counterpart_position(position: &str) -> Option<String> {
    if let Some(base) = position.strip_prefix('+') {
        Some(format!("-{base}"))
    } else {
        position.strip_prefix('-').map(|base| format!("+{base}"))
    }
}

/// Build the shadow for `point` of the glyph named `self_name` (excluded from
/// its own shadow), at the edited glyph's `scale`.
pub(crate) fn compute(
    self_name: Option<&str>,
    point: &GlyphPoint,
    scale: u8,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
) -> Option<AnchorShadow> {
    let counterpart = counterpart_position(&point.position)?;
    let ps = scale.max(1) as i32;

    // Collected by name and sorted before blitting: a `HashMap` iterates in an
    // arbitrary order, and while the union itself is order-independent, the
    // tests should not have to be.
    let mut candidates: Vec<(&str, &ResolvedGlyph, &GlyphPoint)> = Vec::new();
    for (name, resolved) in named_glyphs {
        if Some(name.as_str()) == self_name {
            continue;
        }
        if resolved.grid.width == 0 || resolved.grid.height == 0 {
            continue;
        }
        let Some(anchor) = resolved
            .resolved_anchors
            .iter()
            .find(|a| a.position == counterpart && a.size_matches(point))
        else {
            continue;
        };
        candidates.push((name.as_str(), resolved, anchor));
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|(name, _, _)| *name);

    let mut placements: Vec<(PixelGrid, i32, i32)> = Vec::new();
    let (mut min_r, mut min_c) = (i32::MAX, i32::MAX);
    let (mut max_r, mut max_c) = (i32::MIN, i32::MIN);
    for (_, resolved, anchor) in candidates {
        let rs = resolved.scale.max(1) as i32;
        let row = point.row as i32 - anchor.row as i32 + resolved.origin_row * ps / rs;
        let col = point.col as i32 - anchor.col as i32 + resolved.origin_col * ps / rs;
        let grid = if rs == ps {
            resolved.grid.clone()
        } else {
            resolved.grid.rescale(rs as u8, ps as u8)
        };
        let (h, w) = (grid.height as i32, grid.width as i32);
        if row < -MAX_EXTENT || col < -MAX_EXTENT || row + h > MAX_EXTENT || col + w > MAX_EXTENT {
            continue;
        }
        min_r = min_r.min(row);
        min_c = min_c.min(col);
        max_r = max_r.max(row + h);
        max_c = max_c.max(col + w);
        placements.push((grid, row, col));
    }
    if placements.is_empty() {
        return None;
    }

    let mut grid = PixelGrid::new((max_c - min_c) as u16, (max_r - min_r) as u16);
    for (src, row, col) in &placements {
        union_into(&mut grid, src, row - min_r, col - min_c);
    }

    Some(AnchorShadow {
        grid,
        row: min_r as i16,
        col: min_c as i16,
        count: placements.len(),
    })
}

/// Union `src` into `dst` at `(off_r, off_c)` — the same rule
/// [`PixelGrid::blit`] applies, with the two cases a shadow is mostly made of
/// taken first. A shadow is every attachable glyph at once, so its cells
/// overlap far more than a composite's do, and going through the exact sweep
/// for each of them costs more than the whole rest of the view rebuild.
fn union_into(dst: &mut PixelGrid, src: &PixelGrid, off_r: i32, off_c: i32) {
    for r in 0..src.height as i32 {
        for c in 0..src.width as i32 {
            let shape = src.get(r as u16, c as u16);
            if shape.is_empty() {
                continue;
            }
            let (dr, dc) = (off_r + r, off_c + c);
            if dr < 0 || dc < 0 || dr >= dst.height as i32 || dc >= dst.width as i32 {
                continue;
            }
            let (dr, dc) = (dr as u16, dc as u16);
            let current = dst.get(dr, dc);
            // Already a whole inked pixel: nothing unions into more than that.
            if current.shape_id() == PX_ALMOSTFULL && current.is_filled() {
                continue;
            }
            let filled = current.is_filled() || shape.is_filled();
            if current.is_empty() {
                if shape.shape_id() == PX_CUSTOM {
                    dst.set_detail(dr, dc, &src.region_at(r as u16, c as u16), filled);
                } else {
                    dst.set(dr, dc, shape);
                }
                continue;
            }
            // The same geometry twice — only the ink flag can still change.
            if current.shape_id() == shape.shape_id() && shape.shape_id() != PX_CUSTOM {
                dst.set_filled(dr, dc, filled);
                continue;
            }
            let union = crate::detail::bool_op(
                &dst.region_at(dr, dc),
                &src.region_at(r as u16, c as u16),
                crate::detail::BoolOp::Union,
            );
            dst.set_detail(dr, dc, &union, filled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::{PX_ALMOSTFULL, PX_HALF1, PX_HALF2, PixelShape};

    fn point(position: &str, col: i16, row: i16) -> GlyphPoint {
        GlyphPoint {
            comment: None,
            position: position.to_string(),
            col,
            row,
            col_end: col,
            row_end: row,
        }
    }

    fn wide_point(position: &str, col: i16, row: i16, width: i16) -> GlyphPoint {
        GlyphPoint {
            col_end: col + width - 1,
            ..point(position, col, row)
        }
    }

    /// A 1x1 glyph whose single cell carries `shape`, anchored at `anchor`.
    fn glyph(shape: u8, filled: bool, anchor: GlyphPoint) -> ResolvedGlyph {
        let mut grid = PixelGrid::new(1, 1);
        grid.set(0, 0, PixelShape::new(shape, filled));
        ResolvedGlyph {
            grid,
            origin_row: 0,
            origin_col: 0,
            resolved_anchors: vec![anchor.clone()],
            declared_anchors: vec![anchor],
            scale: 1,
        }
    }

    fn named(glyphs: Vec<(&str, ResolvedGlyph)>) -> HashMap<String, ResolvedGlyph> {
        glyphs
            .into_iter()
            .map(|(n, g)| (n.to_string(), g))
            .collect()
    }

    #[test]
    fn counterpart_flips_the_sign_only() {
        assert_eq!(counterpart_position("+above").as_deref(), Some("-above"));
        assert_eq!(counterpart_position("-above").as_deref(), Some("+above"));
        assert_eq!(counterpart_position("above"), None);
    }

    /// The two sides are found the same way: a `+` anchor collects the `-`
    /// glyphs and a `-` anchor collects the `+` ones, both aligned so the
    /// anchors coincide.
    #[test]
    fn either_sign_finds_the_other_side() {
        let marks = named(vec![("acute", glyph(PX_ALMOSTFULL, true, point("-above", 0, 0)))]);
        let shadow = compute(Some("a"), &point("+above", 3, 5), 1, &marks).unwrap();
        assert_eq!((shadow.col, shadow.row), (3, 5));
        assert_eq!(shadow.count, 1);

        let bases = named(vec![("a", glyph(PX_ALMOSTFULL, true, point("+above", 2, 1)))]);
        let shadow = compute(Some("acute"), &point("-above", 0, 0), 1, &bases).unwrap();
        // The base is placed so its `+above` lands on our `-above` at (0, 0).
        assert_eq!((shadow.col, shadow.row), (-2, -1));
    }

    /// Ink is OR'd and geometry is unioned exactly: two complementary halves
    /// in the same cell make a full pixel, not one half winning.
    #[test]
    fn overlapping_candidates_are_unioned() {
        let glyphs = named(vec![
            ("m1", glyph(PX_HALF1, false, point("-above", 0, 0))),
            ("m2", glyph(PX_HALF2, true, point("-above", 0, 0))),
        ]);
        let shadow = compute(None, &point("+above", 0, 0), 1, &glyphs).unwrap();
        assert_eq!(shadow.count, 2);
        assert_eq!((shadow.grid.width, shadow.grid.height), (1, 1));
        let cell = shadow.grid.get(0, 0);
        assert_eq!(cell.shape_id(), PX_ALMOSTFULL);
        assert!(cell.is_filled(), "ink from either candidate lights the cell");
    }

    /// Candidates that do not size-match are not attachable, so they are not
    /// shadowed either — the same rule composition applies.
    #[test]
    fn size_mismatched_anchors_are_skipped() {
        let glyphs = named(vec![(
            "wide",
            glyph(PX_ALMOSTFULL, true, wide_point("-above", 0, 0, 2)),
        )]);
        assert!(compute(None, &point("+above", 0, 0), 1, &glyphs).is_none());
        assert!(compute(None, &wide_point("+above", 0, 0, 2), 1, &glyphs).is_some());
    }

    /// A glyph carrying both signs of an anchor would otherwise shadow itself.
    #[test]
    fn the_edited_glyph_is_not_its_own_shadow() {
        let mut both = glyph(PX_ALMOSTFULL, true, point("-join", 0, 0));
        both.resolved_anchors.push(point("+join", 0, 0));
        let glyphs = named(vec![("chain", both)]);
        assert!(compute(Some("chain"), &point("+join", 0, 0), 1, &glyphs).is_none());
        assert!(compute(None, &point("+join", 0, 0), 1, &glyphs).is_some());
    }

    /// The union spans every candidate, so the shadow's own grid starts at the
    /// left/topmost one and can reach outside the edited glyph.
    #[test]
    fn the_union_spans_every_candidate() {
        let glyphs = named(vec![
            ("left", glyph(PX_ALMOSTFULL, true, point("-above", 2, 0))),
            ("right", glyph(PX_ALMOSTFULL, true, point("-above", -1, 1))),
        ]);
        let shadow = compute(None, &point("+above", 0, 0), 1, &glyphs).unwrap();
        // "left" lands at col -2, "right" at col 1 row -1.
        assert_eq!((shadow.col, shadow.row), (-2, -1));
        assert_eq!((shadow.grid.width, shadow.grid.height), (4, 2));
        assert!(shadow.grid.get(1, 0).is_filled());
        assert!(shadow.grid.get(0, 3).is_filled());
    }
}
