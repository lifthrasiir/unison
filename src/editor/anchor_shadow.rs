//! The *anchor shadow*: while an `anchor` layer is the one selected in
//! the subglyph palette, the editor draws every glyph that could attach at that
//! anchor underneath the glyph being edited.
//!
//! Attachment is symmetric, so this makes no distinction between the two signs:
//! a `+above` shows the marks that carry `-above`, a `-above` shows the bases
//! that carry `+above`, and both are found the same way. The union rule and the
//! grid the result is carried in are [`crate::editor::shadow`]'s, shared with
//! the backreference shadow.
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

use crate::document::GlyphPoint;
use crate::editor::ref_composite::ResolvedGlyph;
use crate::editor::shadow::{Shadow, ShadowBuilder, ShadowKind};

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
) -> Option<Shadow> {
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

    let mut builder = ShadowBuilder::new(ShadowKind::Anchor);
    for (_, resolved, anchor) in candidates {
        let rs = resolved.scale.max(1) as i32;
        let row = point.row as i32 - anchor.row as i32 + resolved.origin_row * ps / rs;
        let col = point.col as i32 - anchor.col as i32 + resolved.origin_col * ps / rs;
        let grid = if rs == ps {
            resolved.grid.clone()
        } else {
            resolved.grid.rescale(rs as u8, ps as u8)
        };
        builder.push(grid, row, col);
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PixelGrid;
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
            declared_box: None,
            inline_source: None,
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
        let marks = named(vec![(
            "acute",
            glyph(PX_ALMOSTFULL, true, point("-above", 0, 0)),
        )]);
        let shadow = compute(Some("a"), &point("+above", 3, 5), 1, &marks).unwrap();
        assert_eq!((shadow.col, shadow.row), (3, 5));
        assert_eq!(shadow.count, 1);

        let bases = named(vec![(
            "a",
            glyph(PX_ALMOSTFULL, true, point("+above", 2, 1)),
        )]);
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
        assert!(
            cell.is_filled(),
            "ink from either candidate lights the cell"
        );
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
