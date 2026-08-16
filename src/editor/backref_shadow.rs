//! The *backreference shadow*: every glyph that refers to the one being
//! edited, drawn underneath it, each placed so that its own copy of this glyph
//! lands exactly on this glyph.
//!
//! It answers the question the anchor shadow does not — "what am I used in, and
//! does my ink still fit there?" — and it answers it the same way: the *union*
//! of every referring glyph at once, through [`crate::editor::shadow`].
//!
//! **What counts as a backreference.** The referring glyphs are read off
//! [`ResolvedGlyph::inline_source`], which is a composite's *effective* refs —
//! an anchor-placed ref already carries the offset that was derived for it, and
//! the name of the alternative that was actually chosen. So the shadow shows
//! where each parent really puts this glyph rather than re-deriving a placement
//! that might disagree with the build. Refs are matched by name; a parent that
//! refers to this glyph twice contributes twice, and a subtracting (`-ref`)
//! parent contributes like any other — the shadow says *where the parent is*,
//! not what it does with the ink.
//!
//! **Placement.** In a parent `P`'s own coordinates this glyph's raster sits at
//! `ref offset + this glyph's origin` (`ref_effective_offset_scaled`), while
//! `P`'s raster sits at `P.origin`. Expressing the second relative to the first
//! and converting to this glyph's `scale` is the whole of [`compute`].
//!
//! **Cost.** This is O(document) in the glyph count *and* in how often the
//! edited glyph is used, so unlike the anchor shadow it is not on by default:
//! see [`crate::editor::EditMode::PixelSelect`], which the second `` ` ``
//! toggles it on within. A shadow is only rebuilt when the view cache is.

use std::collections::HashMap;

use crate::editor::ref_composite::ResolvedGlyph;
use crate::editor::shadow::{Shadow, ShadowBuilder, ShadowKind};

/// Build the shadow of every glyph referring to `self_name`, at the edited
/// glyph's `scale`.
pub(crate) fn compute(
    self_name: &str,
    scale: u8,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
) -> Option<Shadow> {
    let ss = scale.max(1) as i32;
    // Where this glyph's own raster starts, in its own grid coordinates. A
    // plain pixel glyph has no offset at all; a composite's raster reaches
    // above/left of its own pixels exactly as far as `origin` says.
    let (self_origin_row, self_origin_col) = named_glyphs
        .get(self_name)
        .map_or((0, 0), |g| (g.origin_row, g.origin_col));

    // Collected by name and sorted before the union, for the same reason the
    // anchor shadow does it: a `HashMap` iterates in an arbitrary order, and
    // while the union itself is order-independent, the tests should not have
    // to be.
    let mut parents: Vec<(&str, &ResolvedGlyph)> = named_glyphs
        .iter()
        .filter(|(name, parent)| {
            name.as_str() != self_name
                && parent.grid.width != 0
                && parent.grid.height != 0
                && parent
                    .inline_source
                    .as_ref()
                    .is_some_and(|src| src.refs.iter().any(|r| r.name == self_name))
        })
        .map(|(name, parent)| (name.as_str(), parent))
        .collect();
    if parents.is_empty() {
        return None;
    }
    parents.sort_by_key(|(name, _)| *name);

    let mut builder = ShadowBuilder::new(ShadowKind::Backref);
    for (_, parent) in parents {
        let ps = parent.scale.max(1) as i32;
        let Some(src) = &parent.inline_source else {
            continue;
        };
        for gref in src.refs.iter().filter(|r| r.name == self_name) {
            // This glyph's raster top-left, in the parent's coordinates…
            let rr = gref.row() as i32 + self_origin_row * ps / ss;
            let rc = gref.col() as i32 + self_origin_col * ps / ss;
            // …and the parent's own raster top-left, back in this glyph's.
            let row = self_origin_row + (parent.origin_row - rr) * ss / ps;
            let col = self_origin_col + (parent.origin_col - rc) * ss / ps;
            let grid = if ps == ss {
                parent.grid.clone()
            } else {
                parent.grid.rescale(ps as u8, ss as u8)
            };
            builder.push(grid, row, col);
        }
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{GlyphRef, PixelGrid};
    use crate::pixel::{PX_ALMOSTFULL, PX_HALF1, PX_HALF2, PixelShape};
    use crate::ref_composite::InlineSource;

    fn gref(name: &str, col: i16, row: i16) -> GlyphRef {
        GlyphRef {
            name: name.to_string(),
            raw_name: None,
            offset: Some((col, row)),
            negated: false,
            inherit: false,
            if_exists: false,
            fill: None,
            visibility: None,
            comment: None,
        }
    }

    /// A `w`x`h` glyph of `shape`, whose raster starts at `origin`, referring
    /// to each of `refs`.
    fn glyph(w: u16, h: u16, shape: u8, origin: (i32, i32), refs: Vec<GlyphRef>) -> ResolvedGlyph {
        let mut grid = PixelGrid::new(w, h);
        for r in 0..h {
            for c in 0..w {
                grid.set(r, c, PixelShape::new(shape, true));
            }
        }
        ResolvedGlyph {
            grid,
            origin_row: origin.0,
            origin_col: origin.1,
            resolved_anchors: Vec::new(),
            declared_anchors: Vec::new(),
            scale: 1,
            declared_box: None,
            inline_source: (!refs.is_empty())
                .then(|| std::sync::Arc::new(InlineSource { refs, pixels: None })),
        }
    }

    fn named(glyphs: Vec<(&str, ResolvedGlyph)>) -> HashMap<String, ResolvedGlyph> {
        glyphs
            .into_iter()
            .map(|(n, g)| (n.to_string(), g))
            .collect()
    }

    /// A glyph nothing refers to casts no shadow.
    #[test]
    fn no_referrer_no_shadow() {
        let glyphs = named(vec![
            ("a", glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![])),
            (
                "b",
                glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![gref("c", 0, 0)]),
            ),
        ]);
        assert!(compute("a", 1, &glyphs).is_none());
    }

    /// The parent lands so that its copy of us sits on us: a parent that draws
    /// us three columns in is drawn three columns *left* of our own grid.
    #[test]
    fn the_parent_is_placed_by_its_own_ref_offset() {
        let glyphs = named(vec![
            ("a", glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![])),
            (
                "ab",
                glyph(4, 2, PX_ALMOSTFULL, (0, 0), vec![gref("a", 3, 1)]),
            ),
        ]);
        let shadow = compute("a", 1, &glyphs).unwrap();
        assert_eq!((shadow.col, shadow.row), (-3, -1));
        assert_eq!((shadow.grid.width, shadow.grid.height), (4, 2));
        assert_eq!(shadow.count, 1);
        assert_eq!(shadow.kind, ShadowKind::Backref);
    }

    /// A composite whose raster reaches left of its own pixels is placed by
    /// that raster, not by the `ref` offset alone.
    #[test]
    fn our_own_origin_is_taken_out_of_the_offset() {
        // "a" rasterizes from one column left of its own origin, so the parent
        // ref at column 3 puts our *raster* at column 2.
        let glyphs = named(vec![
            ("a", glyph(2, 1, PX_ALMOSTFULL, (0, -1), vec![])),
            (
                "ab",
                glyph(4, 1, PX_ALMOSTFULL, (0, 0), vec![gref("a", 3, 0)]),
            ),
        ]);
        let shadow = compute("a", 1, &glyphs).unwrap();
        assert_eq!((shadow.col, shadow.row), (-3, 0));
    }

    /// Two refs to the same glyph in one parent are two placements, and every
    /// referring glyph is unioned exactly — two complementary halves in one
    /// cell make a full pixel.
    #[test]
    fn every_reference_counts_and_the_union_is_exact() {
        let glyphs = named(vec![
            ("a", glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![])),
            (
                "aa",
                glyph(
                    1,
                    1,
                    PX_HALF1,
                    (0, 0),
                    vec![gref("a", 0, 0), gref("a", 0, 0)],
                ),
            ),
            ("ax", glyph(1, 1, PX_HALF2, (0, 0), vec![gref("a", 0, 0)])),
        ]);
        let shadow = compute("a", 1, &glyphs).unwrap();
        assert_eq!(shadow.count, 3);
        assert_eq!((shadow.grid.width, shadow.grid.height), (1, 1));
        assert_eq!(shadow.grid.get(0, 0).shape_id(), PX_ALMOSTFULL);
    }

    /// A glyph referring to itself — which resolution never produces, but a
    /// half-typed source can ask for — is not its own shadow.
    #[test]
    fn the_edited_glyph_is_not_its_own_shadow() {
        let glyphs = named(vec![(
            "a",
            glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![gref("a", 1, 0)]),
        )]);
        assert!(compute("a", 1, &glyphs).is_none());
    }

    /// A parent that writes hardblanks (`$$`) where this glyph sits draws
    /// nothing there, so it shadows nothing: a shadow is read off ink, not off
    /// a cell having been written.
    #[test]
    fn a_parent_that_draws_nothing_casts_no_shadow() {
        let glyphs = named(vec![
            ("a", glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![])),
            (
                "blank",
                glyph(
                    4,
                    1,
                    crate::pixel::PX_HARDBLANK,
                    (0, 0),
                    vec![gref("a", 0, 0)],
                ),
            ),
        ]);
        assert!(compute("a", 1, &glyphs).is_none());
    }

    /// The blank cells of a parent that *does* draw are left out of the union,
    /// so the shadow dims only what the parent actually inks.
    #[test]
    fn blank_cells_of_a_referring_glyph_are_not_shadowed() {
        let mut parent = glyph(2, 1, PX_ALMOSTFULL, (0, 0), vec![gref("a", 1, 0)]);
        parent
            .grid
            .set(0, 0, PixelShape::new(crate::pixel::PX_HARDBLANK, false));
        let glyphs = named(vec![
            ("a", glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![])),
            ("parent", parent),
        ]);
        let shadow = compute("a", 1, &glyphs).unwrap();
        assert!(
            shadow.grid.get(0, 0).is_empty(),
            "the parent's hardblank must not be carried into the shadow at all"
        );
        assert!(!shadow.grid.get(0, 1).is_blank(), "its ink must not");
    }

    /// A parent placed absurdly far away is dropped rather than stretching the
    /// shadow grid to reach it ([`crate::editor::shadow::MAX_EXTENT`]).
    #[test]
    fn a_misplaced_parent_is_dropped() {
        let glyphs = named(vec![
            ("a", glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![])),
            (
                "far",
                glyph(1, 1, PX_ALMOSTFULL, (0, 0), vec![gref("a", 30000, 0)]),
            ),
        ]);
        assert!(compute("a", 1, &glyphs).is_none());
    }
}
