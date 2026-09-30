//! `InkProfile`: where a glyph's ink starts and stops on every line of the grid, and the faces every gap measurement reads.

use crate::detail::DetailRegion;
use crate::document::PixelGrid;

/// Where a glyph's ink starts and stops on every line of the grid, in the
/// glyph's *declared* units — the same units the IDC layout is in, so a part
/// drawn at `scale 2` measures the same as one drawn at `scale 1`.
///
/// Both axes are kept because one part can be a component of a ⿰ line and a ⿱
/// line both, and the profile is built once per part per expansion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InkProfile {
    /// Per row, read left to right, or `None` for a row that draws nothing.
    pub rows: Vec<Option<InkLine>>,
    /// Per column, read top to bottom.
    pub cols: Vec<Option<InkLine>>,
    /// Per row, what its two ink frontiers actually cover of the boundary they
    /// face. Parallel to [`Self::rows`].
    pub row_edges: Vec<EdgeCover>,
    /// Per column, the same. Parallel to [`Self::cols`].
    pub col_edges: Vec<EdgeCover>,
    /// The lattice the covers are counted over: one declared cell across a line
    /// is `edge_den`. Catalog geometry lies on the half lattice, so this is
    /// twice the grid's `scale` and every endpoint is exact on it.
    pub edge_den: u16,
}

/// How much of the boundary a line's frontier faces its ink actually covers, as
/// sorted disjoint intervals over [`InkProfile::edge_den`], measured across the
/// line (down a row, along a column).
///
/// A frontier is a *cell*, and a cell is inked long before its ink reaches the
/// side of it that faces the neighbour: a diagonal ending in a corner inks the
/// cell and covers none of the edge, or a sliver of it. That difference is the
/// whole reason this is kept — [`contact_run`](super::gap::contact_run) asks whether two parts really
/// run together, and the cells alone answer a coarser question, always the
/// stricter way round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeCover {
    /// What the near frontier covers of its near side.
    pub near: Vec<(u16, u16)>,
    /// What the far frontier covers of its far side.
    pub far: Vec<(u16, u16)>,
    /// What the *first run's* far frontier covers of its far side — a wall's
    /// inner face, the one a cavity sees. Identical to [`Self::far`] on a line
    /// that is one run, which is every line of a part that does not enclose.
    pub first_far: Vec<(u16, u16)>,
    /// The same for the last run's near frontier. Identical to [`Self::near`]
    /// on a one-run line.
    pub last_near: Vec<(u16, u16)>,
}

/// Whether two covers, each over its own lattice, share any length at all.
pub(super) fn covers_meet(a: &[(u16, u16)], a_den: u16, b: &[(u16, u16)], b_den: u16) -> bool {
    let (a_den, b_den) = (a_den.max(1) as i64, b_den.max(1) as i64);
    // Cross-multiplied onto the shared lattice, which needs no gcd: the
    // comparison is all anyone wants of it.
    a.iter().any(|&(p, q)| {
        let (p, q) = (p as i64 * b_den, q as i64 * b_den);
        b.iter().any(|&(r, t)| {
            let (r, t) = (r as i64 * a_den, t as i64 * a_den);
            p.max(r) < q.min(t)
        })
    })
}

/// Sorted, with everything that meets or overlaps run together.
fn merge_cover(list: &mut Vec<(u16, u16)>) {
    list.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::with_capacity(list.len());
    for &(s, e) in list.iter() {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    *list = out;
}

/// What one line of a grid occupies along an axis: the two frontiers, plus how
/// far a hardblank run reaches in from each of them.
///
/// The runs are what lets two facing parts share space. A hardblank draws
/// nothing yet occupies its cell (it is space the source *means*), so it holds
/// the frontier out where an empty cell would not; but where the two parts'
/// runs face each other, the same nothing is written twice, and the shared
/// depth is clearance rather than a part's own extent — see [`facing_offset`](super::gap::facing_offset).
///
/// The two frontiers are in the box's coordinates and are *not* bounded by it:
/// a part that draws outside what it declared is measured where it draws, so
/// `near` may be negative and `far` may reach past the extent. See
/// [`InkProfile::of`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InkLine {
    /// The lowest occupied coordinate on this line.
    pub near: i32,
    /// The highest occupied coordinate on this line.
    pub far: i32,
    /// Hardblank cells running inward from `near`, `near` included.
    pub near_hardblanks: u16,
    /// Hardblank cells running inward from `far`, `far` included.
    pub far_hardblanks: u16,
    /// The run this line presents to a cavity from the **low** side, looking
    /// toward higher coordinates. `None` when the line draws nothing on that
    /// side and so has no wall there to be measured against.
    pub low_wall: Option<WallFace>,
    /// The same from the **high** side, looking toward lower coordinates.
    pub high_wall: Option<WallFace>,
}

/// A wall's cavity-facing end on one line: which run it is, and what the run's
/// claim beyond it comes to.
///
/// **Which run** is the whole question, and the answer is *the run nearest the
/// box's middle*. Taking the line's first and last runs would be the obvious
/// rule and it is wrong on real drawings: every han part writes its side
/// bearing as a detached hardblank column at the box's edge, so the first run
/// of nearly every line is a bearing rather than a wall, and a cavity measured
/// against it would swallow the wall itself. The middle is where a cavity is,
/// so the runs on either side of it are what bound one; a run that *contains*
/// the middle fills the line and presents its two far ends, which reads as the
/// overlap it is.
///
/// What this cannot see is a mark the drawing leaves inside its own cavity —
/// the run adjacent to the middle is the only one either side offers. That is
/// deliberate: choosing by where the inner part actually sits would make the
/// measurement depend on the placement, and with it the axis totals that
/// [`measure_enclosure_clearances`](super::enclosure::measure_enclosure_clearances) telescopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallFace {
    /// The run's cavity-facing cell.
    pub at: i32,
    /// Hardblank cells running back from `at` into the run.
    pub hardblanks: u16,
    /// How long the run is, so that a claim which has eaten it whole reads as
    /// no ink at all.
    pub run: u16,
}

/// One boundary of one line, as everything that measures a gap sees it: where
/// the material stops, how deep the source's claim on the space beyond runs,
/// and which way the boundary looks.
///
/// The four boundaries a line has are the two it presents to the world
/// ([`InkLine::near`] and [`InkLine::far`]) and the two it presents to a cavity
/// between its first and last run. A part that does not enclose draws one run
/// per line and its four boundaries collapse onto two, which is why every
/// one-dimensional measurement reads exactly the numbers it always did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    /// The occupied cell the boundary is at.
    pub at: i32,
    /// Hardblank cells running back from `at` into the run behind it.
    pub hardblanks: u16,
    /// How long that run is, in cells — what says whether the claim has eaten
    /// it whole.
    pub run: u16,
    /// `+1` when the boundary looks toward higher coordinates, `-1` toward
    /// lower ones. Every "pull the claim back" and "step past the frontier"
    /// below is written once, with this as its sign.
    pub toward: i32,
}

impl Face {
    /// Where the run's **ink** stops on this side: `at` pulled back by the
    /// hardblank claim, or `None` when the claim is the whole run and there is
    /// no ink here to touch anything.
    ///
    /// This is the reason [`contact_run`](super::gap::contact_run) needs no hardblank term of its own: a
    /// claim parts two parts by holding their *ink* apart, and ink that is
    /// already apart touches over no lines.
    pub fn ink(self) -> Option<i32> {
        (self.hardblanks < self.run).then(|| self.at - self.toward * self.hardblanks as i32)
    }
}

impl InkLine {
    /// One line of a grid, from the runs of occupied cells it holds and the
    /// coordinate a cavity on it would be around ([`WallFace`]).
    ///
    /// `runs` is in ascending order and non-empty; each is `(near, far,
    /// hardblanks running in from near, hardblanks running in from far)`.
    pub fn from_runs(runs: &[(i32, i32, u16, u16)], pivot: i32) -> Option<Self> {
        let first = runs.first()?;
        let last = runs.last()?;
        let len = |r: &(i32, i32, u16, u16)| (r.1 - r.0 + 1).clamp(0, u16::MAX as i32) as u16;
        // A run straddling the middle fills the line: it is the wall on both
        // sides at once, and presents each of its own far ends.
        let straddling = runs.iter().find(|r| r.0 <= pivot && pivot <= r.1);
        let low = straddling.or_else(|| runs.iter().rev().find(|r| r.1 < pivot));
        let high = straddling.or_else(|| runs.iter().find(|r| r.0 > pivot));
        Some(Self {
            near: first.0,
            far: last.1,
            near_hardblanks: first.2,
            far_hardblanks: last.3,
            low_wall: low.map(|r| WallFace {
                at: r.1,
                hardblanks: r.3,
                run: len(r),
            }),
            high_wall: high.map(|r| WallFace {
                at: r.0,
                hardblanks: r.2,
                run: len(r),
            }),
        })
    }

    /// The boundary looking toward **higher** coordinates: the line's own far
    /// end, or — with `inner` — the low-side wall's cavity face.
    pub fn upper(self, inner: bool) -> Option<Face> {
        let (at, hardblanks, run) = match inner {
            false => (
                self.far,
                self.far_hardblanks,
                (self.far - self.near + 1).clamp(0, u16::MAX as i32) as u16,
            ),
            true => {
                let w = self.low_wall?;
                (w.at, w.hardblanks, w.run)
            }
        };
        Some(Face {
            at,
            hardblanks,
            run,
            toward: 1,
        })
    }

    /// The boundary looking toward **lower** coordinates: the line's own near
    /// end, or — with `inner` — the high-side wall's cavity face.
    pub fn lower(self, inner: bool) -> Option<Face> {
        let (at, hardblanks, run) = match inner {
            false => (
                self.near,
                self.near_hardblanks,
                (self.far - self.near + 1).clamp(0, u16::MAX as i32) as u16,
            ),
            true => {
                let w = self.high_wall?;
                (w.at, w.hardblanks, w.run)
            }
        };
        Some(Face {
            at,
            hardblanks,
            run,
            toward: -1,
        })
    }
}

impl InkProfile {
    /// Read a part's frontiers over its **declared box**: `origin` is where the
    /// box's corner sits in the grid and `extent` is its size, both in declared
    /// cells, so the profile is indexed the way the IDC layout is.
    ///
    /// A cell counts as occupied when the source put *something* there, which
    /// includes a hardblank — see
    /// [`PixelShape::is_clear`](crate::pixel::PixelShape::is_clear). This is
    /// where the `CLEAR` / `HARDBLANK` / `INK` ladder those predicates are
    /// named for is actually read: a declared cell is hardblank when the source
    /// wrote hardblanks there and no ink, so a `scale 2` part is read on the
    /// same units its layout is in.
    ///
    /// **Along** the line being read, a cell outside the box keeps the
    /// coordinate it is drawn at, negative or past the extent as the case may
    /// be. That is the whole point on this axis: a hardblank drawn beyond the
    /// box is a claim on the neighbour's space (it is how a part writes a side
    /// bearing), and one folded onto the box's edge would be lost the moment
    /// that edge held ink. Ink out there is read the same way — a part drawing
    /// where it said it would not can only cost its neighbour room, never gain
    /// any.
    ///
    /// **Across** it the cell folds into the nearest line instead: the lines of
    /// two parts of one IDC line are matched by index, so a profile that is not
    /// exactly the box's size across cannot be measured against anything. A box
    /// reaching past the grid is simply clear out there — there is nothing
    /// drawn to report.
    /// `raster` is the raster coordinate `(col, row)` of grid cell `(0, 0)`:
    /// `(0, 0)` for a glyph's own pixels, and the flattening's own origin
    /// ([`ResolvedGlyph::origin_col`](crate::ref_composite::ResolvedGlyph) and
    /// `origin_row`) for a part that is a composite. A ref reaching left of or
    /// above the origin makes it negative, and the ink out there is exactly the
    /// ink this profile may not lose (see "along the line" below). It is in
    /// *raster* cells, not declared ones, because a `ref` offset is: it need
    /// not be a multiple of `scale`, so it cannot be folded into `origin`.
    pub fn of(
        grid: &PixelGrid,
        scale: u8,
        raster: (i32, i32),
        origin: (i16, i16),
        extent: (u16, u16),
    ) -> Self {
        let s = scale.max(1) as u16;
        let (w, h) = extent;
        // Raster → box coordinate, on the one axis at a time the callers below
        // read it: floor division, since a grid starting left of the origin has
        // negative raster coordinates and `-1 / 2` is not the cell `-1` is in.
        let box_of = |raster: i32, origin: i16| raster.div_euclid(s as i32) - origin as i32;
        // Per declared cell: nothing / hardblank only / ink, whichever is
        // greatest over the sub-cells, so any ink makes the cell ink.
        const CLEAR: u8 = 0;
        const HARDBLANK: u8 = 1;
        const INK: u8 = 2;
        // The box coordinates the grid reaches on each axis, box and grid
        // together, so a cell outside the box still has a place to be counted.
        let span = |extent: u16, len: u16, origin: i16, base: i32| -> (i32, i32) {
            if extent == 0 || len == 0 {
                return (0, extent as i32);
            }
            let (lo, hi) = (box_of(base, origin), box_of(base + len as i32 - 1, origin));
            (lo.min(0), hi.max(extent as i32 - 1) + 1)
        };
        let (col_lo, col_hi) = span(w, grid.width, origin.0, raster.0);
        let (row_lo, row_hi) = span(h, grid.height, origin.1, raster.1);
        let (cols_wide, rows_tall) = ((col_hi - col_lo) as usize, (row_hi - row_lo) as usize);
        // One map per axis, since which coordinate folds depends on which way
        // the line is read: `by_row` keeps the column exact and `by_col` the row.
        let mut by_row = vec![CLEAR; cols_wide * h as usize];
        let mut by_col = vec![CLEAR; rows_tall * w as usize];
        if w > 0 && h > 0 {
            for row in 0..grid.height {
                for col in 0..grid.width {
                    let px = grid.get(row, col);
                    if px.is_clear() {
                        continue;
                    }
                    let level = if px.is_hardblank() { HARDBLANK } else { INK };
                    let box_r = box_of(raster.1 + row as i32, origin.1);
                    let box_c = box_of(raster.0 + col as i32, origin.0);
                    let folded_r = box_r.clamp(0, h as i32 - 1) as usize;
                    let folded_c = box_c.clamp(0, w as i32 - 1) as usize;
                    let at = &mut by_row[folded_r * cols_wide + (box_c - col_lo) as usize];
                    *at = (*at).max(level);
                    let at = &mut by_col[folded_c * rows_tall + (box_r - row_lo) as usize];
                    *at = (*at).max(level);
                }
            }
        }
        // Every run of occupied cells on one line, which is what a cavity
        // between two of them is bounded by. A part that does not enclose draws
        // one run per line and its walls coincide with `near`/`far`, so every
        // one-dimensional measurement reads what it always did.
        let scan = |lo: i32, len: usize, pivot: i32, at: &dyn Fn(usize) -> u8| -> Option<InkLine> {
            let blanks_from = |from: usize, step: isize| -> u16 {
                let mut n = 0u16;
                let mut i = from as isize;
                while i >= 0 && (i as usize) < len && at(i as usize) == HARDBLANK {
                    n += 1;
                    i += step;
                }
                n
            };
            let mut runs: Vec<(i32, i32, u16, u16)> = Vec::new();
            let mut i = 0usize;
            while i < len {
                if at(i) == CLEAR {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < len && at(i) != CLEAR {
                    i += 1;
                }
                let end = i - 1;
                runs.push((
                    lo + start as i32,
                    lo + end as i32,
                    blanks_from(start, 1),
                    blanks_from(end, -1),
                ));
            }
            InkLine::from_runs(&runs, pivot)
        };
        // The coordinate a cavity on the line would be around: the middle of
        // the *declared box*, which is the rectangle an enclosure lays out in.
        // See [`WallFace`] for why the walls are chosen by it.
        let (col_pivot, row_pivot) = (w as i32 / 2, h as i32 / 2);
        let rows: Vec<Option<InkLine>> = (0..h as usize)
            .map(|r| scan(col_lo, cols_wide, col_pivot, &|c| by_row[r * cols_wide + c]))
            .collect();
        let cols: Vec<Option<InkLine>> = (0..w as usize)
            .map(|c| scan(row_lo, rows_tall, row_pivot, &|r| by_col[c * rows_tall + r]))
            .collect();

        // A second pass, now that the frontiers are known: what each of them
        // covers of the boundary it faces. Only the outermost sub-cell of a
        // declared cell touches that boundary, which is why `scale` shows up
        // here as a position and not only as a divisor.
        let mut row_edges = vec![EdgeCover::default(); h as usize];
        let mut col_edges = vec![EdgeCover::default(); w as usize];
        let push = |out: &mut Vec<(u16, u16)>, list: &[(u8, u8)], sub: u16, den: u8| {
            let mul = 2 / den.max(1) as u16;
            out.extend(
                list.iter()
                    .map(|&(a, b)| (sub * 2 + a as u16 * mul, sub * 2 + b as u16 * mul)),
            );
        };
        for row in 0..grid.height {
            for col in 0..grid.width {
                let px = grid.get(row, col);
                // A hardblank draws nothing, so it covers nothing — the same
                // statement [`InkLine::ink`] makes about the frontiers.
                if px.is_clear() || px.is_hardblank() {
                    continue;
                }
                let id = px.catalog_shape_id();
                let region = match id {
                    crate::pixel::PX_CUSTOM => grid.details.get(&(row, col)).cloned(),
                    _ => Some(DetailRegion::from_shape(id)),
                };
                let Some(region) = region else { continue };
                let cov = region.edge_coverage();
                let (abs_r, abs_c) = (raster.1 + row as i32, raster.0 + col as i32);
                let (box_r, box_c) = (box_of(abs_r, origin.1), box_of(abs_c, origin.0));
                let (sub_j, sub_i) = (
                    abs_r.rem_euclid(s as i32) as u16,
                    abs_c.rem_euclid(s as i32) as u16,
                );
                let fr = box_r.clamp(0, h as i32 - 1) as usize;
                let fc = box_c.clamp(0, w as i32 - 1) as usize;
                // Four boundaries per line, not two: the outward pair and the
                // pair a cavity between the line's first and last run sees. On
                // a one-run line the two pairs are the same cells and the same
                // covers are collected twice, which is what makes an enclosure
                // measurement read a non-enclosing part correctly.
                if let Some(line) = rows.get(fr).copied().flatten() {
                    let mut at = |inner: bool| {
                        if line.upper(inner).and_then(Face::ink) == Some(box_c) && sub_i == s - 1 {
                            let out = match inner {
                                false => &mut row_edges[fr].far,
                                true => &mut row_edges[fr].first_far,
                            };
                            push(out, &cov.right, sub_j, cov.den);
                        }
                        if line.lower(inner).and_then(Face::ink) == Some(box_c) && sub_i == 0 {
                            let out = match inner {
                                false => &mut row_edges[fr].near,
                                true => &mut row_edges[fr].last_near,
                            };
                            push(out, &cov.left, sub_j, cov.den);
                        }
                    };
                    at(false);
                    at(true);
                }
                if let Some(line) = cols.get(fc).copied().flatten() {
                    let mut at = |inner: bool| {
                        if line.upper(inner).and_then(Face::ink) == Some(box_r) && sub_j == s - 1 {
                            let out = match inner {
                                false => &mut col_edges[fc].far,
                                true => &mut col_edges[fc].first_far,
                            };
                            push(out, &cov.bottom, sub_i, cov.den);
                        }
                        if line.lower(inner).and_then(Face::ink) == Some(box_r) && sub_j == 0 {
                            let out = match inner {
                                false => &mut col_edges[fc].near,
                                true => &mut col_edges[fc].last_near,
                            };
                            push(out, &cov.top, sub_i, cov.den);
                        }
                    };
                    at(false);
                    at(true);
                }
            }
        }
        for e in row_edges.iter_mut().chain(col_edges.iter_mut()) {
            merge_cover(&mut e.near);
            merge_cover(&mut e.far);
            merge_cover(&mut e.first_far);
            merge_cover(&mut e.last_near);
        }
        Self {
            rows,
            cols,
            row_edges,
            col_edges,
            edge_den: 2 * s,
        }
    }

    /// The covers of the lines that face along the split axis, indexed like
    /// [`Self::along`].
    pub(super) fn along_edges(&self, horizontal: bool) -> &[EdgeCover] {
        if horizontal {
            &self.row_edges
        } else {
            &self.col_edges
        }
    }

    /// The lines that face along the split axis: rows for a horizontal split
    /// (each gives the leftmost and rightmost column), columns for a vertical
    /// one. Indexed by position across the axis, so two parts of one line index
    /// alike.
    pub(super) fn along(&self, horizontal: bool) -> &[Option<InkLine>] {
        if horizontal { &self.rows } else { &self.cols }
    }

    /// How far the ink reaches towards each end of the axis, in the part's own
    /// coordinates, or `None` for a part that draws nothing at all.
    ///
    /// These are the two numbers a clearance against a *parent edge* is
    /// arithmetic on: the near edge measures against the smallest near
    /// frontier and the far edge against the largest far one, since a
    /// clearance is the smallest distance over all the lines.
    ///
    /// A parent edge is taken to be hardblank all the way out — there is
    /// nothing beyond it for a part to keep clear of — so a line's facing
    /// hardblank run collapses into the edge entirely, and the frontier the
    /// edge sees is the first cell past that run. A line that is nothing but
    /// hardblanks therefore constrains neither edge, which is the same
    /// statement: it draws nothing to be clear of.
    pub fn frontier(&self, horizontal: bool) -> Option<AxisFrontier> {
        let lines = self.along(horizontal);
        Some(AxisFrontier {
            near: lines
                .iter()
                .filter_map(|l| l.map(|l| l.near + l.near_hardblanks as i32))
                .min()?,
            far: lines
                .iter()
                .filter_map(|l| l.map(|l| l.far - l.far_hardblanks as i32))
                .max()?,
        })
    }
}

/// How far a part's ink reaches towards each end of the split axis, as an edge
/// sees it — the hardblanks facing the edge already collapsed into it (see
/// [`InkProfile::frontier`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisFrontier {
    /// The lowest coordinate any line's ink starts at.
    pub near: i32,
    /// The highest coordinate any line's ink ends at.
    pub far: i32,
}
