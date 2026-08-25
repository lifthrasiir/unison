//! [`PixelGrid`]: the rectangle of [`PixelShape`](crate::pixel::PixelShape)
//! cells a glyph draws, and everything that reshapes one — cropping, resizing,
//! rescaling and the exact sub-pixel geometry that rides along in `details`.

use std::collections::{BTreeMap, HashMap};

use crate::detail::{self, Classified, DetailRegion, Frac64};
use crate::pixel::{PX_ALMOSTFULL, PX_CUSTOM, PixelShape};

#[derive(Clone, Debug, PartialEq)]
pub struct PixelGrid {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<PixelShape>,
    /// Common lattice denominator for `details`; every stored region's
    /// denominator divides this.
    pub den: u8,
    /// Custom geometry (in canonical form) for pixels whose shape id is
    /// [`PX_CUSTOM`]. Only derived grids carry entries; document source
    /// grids never do.
    pub details: BTreeMap<(u16, u16), DetailRegion>,
}

impl PixelGrid {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![PixelShape::EMPTY; width as usize * height as usize],
            den: 1,
            details: BTreeMap::new(),
        }
    }

    pub fn get(&self, row: u16, col: u16) -> PixelShape {
        if row < self.height && col < self.width {
            self.pixels[row as usize * self.width as usize + col as usize]
        } else {
            PixelShape::EMPTY
        }
    }

    /// Set a pixel's shape, discarding any custom detail stored for the
    /// cell. Assigning a [`PX_CUSTOM`] shape is allowed but leaves the cell
    /// without geometry until the caller inserts the matching detail; use
    /// [`PixelGrid::set_detail`] unless doing exactly that.
    pub fn set(&mut self, row: u16, col: u16, shape: PixelShape) {
        if row < self.height && col < self.width {
            self.pixels[row as usize * self.width as usize + col as usize] = shape;
            if !self.details.is_empty() {
                self.details.remove(&(row, col));
            }
        }
    }

    /// Set a pixel's ink flag ([`crate::pixel::PX_FULL`]) without touching its
    /// geometry, keeping any custom detail intact — unlike [`PixelGrid::set`],
    /// which discards it.
    pub fn set_filled(&mut self, row: u16, col: u16, filled: bool) {
        if row < self.height && col < self.width {
            let px = &mut self.pixels[row as usize * self.width as usize + col as usize];
            *px = PixelShape::new(px.shape_id(), filled);
        }
    }

    /// Set a pixel from an arbitrary region: re-encoded as a plain shape
    /// code whenever the geometry allows, stored as a custom detail
    /// otherwise (growing the grid's common denominator as needed).
    pub fn set_detail(&mut self, row: u16, col: u16, region: &DetailRegion, filled: bool) {
        if row >= self.height || col >= self.width {
            return;
        }
        match region.classify() {
            Classified::Empty => self.set(row, col, PixelShape::EMPTY),
            Classified::Full => self.set(row, col, PixelShape::new(PX_ALMOSTFULL, filled)),
            Classified::Shape(id) => self.set(row, col, PixelShape::new(id, filled)),
            Classified::Custom(canon) => {
                let canon = match detail::lcm_den(self.den, canon.den) {
                    Some(l) => {
                        self.den = l;
                        canon
                    }
                    // The common denominator would overflow; degrade this
                    // cell to the existing grid lattice.
                    None => match canon.snap_to_den(self.den).classify() {
                        Classified::Empty => {
                            self.set(row, col, PixelShape::EMPTY);
                            return;
                        }
                        Classified::Full => {
                            self.set(row, col, PixelShape::new(PX_ALMOSTFULL, filled));
                            return;
                        }
                        Classified::Shape(id) => {
                            self.set(row, col, PixelShape::new(id, filled));
                            return;
                        }
                        Classified::Custom(snapped) => snapped,
                    },
                };
                self.pixels[row as usize * self.width as usize + col as usize] =
                    PixelShape::new(PX_CUSTOM, filled);
                self.details.insert((row, col), canon);
            }
        }
    }

    /// Apply an already-classified region to a pixel.
    fn apply_classified(&mut self, row: u16, col: u16, classified: Classified, filled: bool) {
        match classified {
            Classified::Empty => self.set(row, col, PixelShape::EMPTY),
            Classified::Full => self.set(row, col, PixelShape::new(PX_ALMOSTFULL, filled)),
            Classified::Shape(id) => self.set(row, col, PixelShape::new(id, filled)),
            Classified::Custom(region) => {
                // Route through set_detail for denominator bookkeeping (the
                // region is already canonical, so classification is a cheap
                // table hit).
                self.set_detail(row, col, &region, filled);
            }
        }
    }

    /// The exact filled region of a pixel, whether plain or custom.
    pub fn region_at(&self, row: u16, col: u16) -> DetailRegion {
        let shape = self.get(row, col);
        if shape.shape_id() == PX_CUSTOM {
            self.details
                .get(&(row, col))
                .cloned()
                .unwrap_or(DetailRegion::EMPTY)
        } else {
            DetailRegion::from_shape(shape.shape_id())
        }
    }

    #[cfg(feature = "editor")]
    pub fn resize(&mut self, new_width: u16, new_height: u16) {
        if new_width == self.width && new_height == self.height {
            return;
        }
        let mut new_pixels = vec![PixelShape::EMPTY; new_width as usize * new_height as usize];
        let copy_w = self.width.min(new_width) as usize;
        let copy_h = self.height.min(new_height) as usize;
        for r in 0..copy_h {
            for c in 0..copy_w {
                new_pixels[r * new_width as usize + c] = self.pixels[r * self.width as usize + c];
            }
        }
        self.width = new_width;
        self.height = new_height;
        self.pixels = new_pixels;
        self.details
            .retain(|&(r, c), _| r < new_height && c < new_width);
    }

    /// Exact geometric rescale between two pixel-subdivision scales. Every
    /// destination pixel receives the union of the source regions that
    /// overlap it, mapped exactly; results are re-encoded as plain shape
    /// codes when possible and stored as custom details otherwise.
    ///
    /// What a cell holds *besides* geometry rides along on its own level, since
    /// the sweep can only see what is drawn: a destination cell covering a
    /// hardblank is a hardblank, and one covering an inked cell keeps the ink
    /// flag even where no geometry landed. Ink outranks a claim, the same order
    /// [`crate::pixel::blank_op`] gives a merge.
    ///
    /// Results are memoized by content: the same source grid is typically
    /// rescaled once per referencing glyph, which made this the dominant
    /// cost of resolving a font before caching.
    pub fn rescale(&self, old_scale: u8, new_scale: u8) -> Self {
        if old_scale.max(1) == new_scale.max(1) {
            return self.clone();
        }
        use std::sync::Mutex;
        type CacheEntry = (PixelGrid, u8, u8, PixelGrid);
        static CACHE: Mutex<Option<HashMap<u64, Vec<CacheEntry>>>> = Mutex::new(None);

        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.width.hash(&mut h);
            self.height.hash(&mut h);
            for px in &self.pixels {
                px.0.hash(&mut h);
            }
            self.den.hash(&mut h);
            self.details.hash(&mut h);
            old_scale.hash(&mut h);
            new_scale.hash(&mut h);
            h.finish()
        };
        {
            let mut cache = CACHE.lock().unwrap();
            if let Some(entries) = cache.get_or_insert_with(HashMap::new).get(&key) {
                for (src, o, n, out) in entries {
                    if *o == old_scale && *n == new_scale && src == self {
                        return out.clone();
                    }
                }
            }
        }
        let out = self.rescale_uncached(old_scale, new_scale);
        let mut cache = CACHE.lock().unwrap();
        let map = cache.get_or_insert_with(HashMap::new);
        // Crude bound: drop everything when the cache grows unreasonable
        // (long editor sessions keep mutating grids).
        if map.len() > 4096 {
            map.clear();
        }
        map.entry(key)
            .or_default()
            .push((self.clone(), old_scale, new_scale, out.clone()));
        out
    }

    fn rescale_uncached(&self, old_scale: u8, new_scale: u8) -> Self {
        // A merged rectangle of uniformly full source cells, as one piece
        // for the destination cell's union sweep.
        fn full_run_piece(
            run: (i64, i64, i64, i64),
            cell_x0: &impl Fn(i64) -> Frac64,
            cell_y0: &impl Fn(i64) -> Frac64,
            old_s: i64,
            new_s: i64,
        ) -> (DetailRegion, Frac64, Frac64, Frac64, Frac64) {
            let (sc_start, sc_end, sr_start, sr_end) = run;
            (
                DetailRegion::full(),
                cell_x0(sc_start),
                cell_y0(sr_start),
                Frac64::new((sc_end - sc_start + 1) * new_s, old_s),
                Frac64::new((sr_end - sr_start + 1) * new_s, old_s),
            )
        }
        let old_s = old_scale.max(1) as i64;
        let new_s = new_scale.max(1) as i64;
        if old_s == new_s {
            return self.clone();
        }
        let logical_w = self.width / old_s as u16;
        let logical_h = self.height / old_s as u16;
        let new_width = logical_w * new_s as u16;
        let new_height = logical_h * new_s as u16;
        let mut out = Self::new(new_width, new_height);
        for r in 0..new_height {
            for c in 0..new_width {
                // The destination pixel covers source-space
                // [c·old/new, (c+1)·old/new) × [r·old/new, (r+1)·old/new).
                let sc0 = (c as i64 * old_s).div_euclid(new_s);
                let sc1 = ((c as i64 + 1) * old_s).div_euclid(new_s)
                    + if ((c as i64 + 1) * old_s).rem_euclid(new_s) != 0 {
                        1
                    } else {
                        0
                    };
                let sr0 = (r as i64 * old_s).div_euclid(new_s);
                let sr1 = ((r as i64 + 1) * old_s).div_euclid(new_s)
                    + if ((r as i64 + 1) * old_s).rem_euclid(new_s) != 0 {
                        1
                    } else {
                        0
                    };

                // Fast path: all overlapping source pixels are uniformly
                // full or uniformly clear — no geometry needed. "Clear" is
                // the bottom of the `CLEAR`/`HARDBLANK`/`INK` ladder rather
                // than the empty *shape*: a hardblank and a geometry-less ink
                // flag both draw nothing, so the geometry sweep below cannot
                // see them and they are carried by hand afterwards.
                let mut all_full = true;
                let mut all_empty = true;
                let mut any_filled = false;
                let mut any_hardblank = false;
                for sr in sr0..sr1 {
                    for sc in sc0..sc1 {
                        let s = self.get(sr as u16, sc as u16);
                        if s.shape_id() != PX_ALMOSTFULL {
                            all_full = false;
                        }
                        if !s.is_clear() {
                            all_empty = false;
                        }
                        any_filled |= s.is_bitmap_filled();
                        any_hardblank |= s.is_hardblank();
                    }
                }
                if all_empty {
                    continue;
                }
                if all_full {
                    out.set(r, c, PixelShape::new(PX_ALMOSTFULL, any_filled));
                    continue;
                }

                // Source pixel (sr, sc) spans destination-local x
                // [(sc·new − c·old)/old, (sc·new − c·old)/old + new/old]
                // (and likewise for y), so all pieces have disjoint
                // interiors and can be unioned in a single sweep.
                let cell_x0 = |sc: i64| Frac64::new(sc * new_s - c as i64 * old_s, old_s);
                let cell_y0 = |sr: i64| Frac64::new(sr * new_s - r as i64 * old_s, old_s);
                let cell_span = Frac64::new(new_s, old_s);
                let mut pieces: Vec<(DetailRegion, Frac64, Frac64, Frac64, Frac64)> = Vec::new();

                // Merge runs of uniformly full source cells into single
                // rectangles (row runs, then equal runs of consecutive
                // rows) so a mostly-full block contributes a handful of
                // edges to the sweep instead of four per cell.
                // Open run rectangles: (sc_start, sc_end, sr_start, sr_end).
                let mut open_runs: Vec<(i64, i64, i64, i64)> = Vec::new();
                for sr in sr0..sr1 {
                    let mut row_runs: Vec<(i64, i64)> = Vec::new();
                    let mut run_start: Option<i64> = None;
                    for sc in sc0..sc1 {
                        let s = self.get(sr as u16, sc as u16);
                        if s.shape_id() == PX_ALMOSTFULL {
                            run_start.get_or_insert(sc);
                            continue;
                        }
                        if let Some(start) = run_start.take() {
                            row_runs.push((start, sc - 1));
                        }
                        // Nothing to sweep: an empty cell, a hardblank, or a
                        // cell carrying only an ink flag — all three fold to
                        // the empty shape here, and the latter two are picked
                        // up from `any_hardblank`/`any_filled` below.
                        if s.catalog_shape_id() == crate::pixel::PX_EMPTY {
                            continue;
                        }
                        pieces.push((
                            self.region_at(sr as u16, sc as u16),
                            cell_x0(sc),
                            cell_y0(sr),
                            cell_span,
                            cell_span,
                        ));
                    }
                    if let Some(start) = run_start.take() {
                        row_runs.push((start, sc1 - 1));
                    }
                    let mut next_open: Vec<(i64, i64, i64, i64)> = Vec::new();
                    for (start, end) in row_runs {
                        if let Some(pos) = open_runs
                            .iter()
                            .position(|&(s, e, _, er)| s == start && e == end && er == sr - 1)
                        {
                            let (s, e, sr_start, _) = open_runs.swap_remove(pos);
                            next_open.push((s, e, sr_start, sr));
                        } else {
                            next_open.push((start, end, sr, sr));
                        }
                    }
                    for run in open_runs.drain(..) {
                        pieces.push(full_run_piece(run, &cell_x0, &cell_y0, old_s, new_s));
                    }
                    open_runs = next_open;
                }
                for run in open_runs.drain(..) {
                    pieces.push(full_run_piece(run, &cell_x0, &cell_y0, old_s, new_s));
                }

                let region = detail::union_disjoint_transformed(&pieces);
                if region.is_empty() {
                    // Nothing was drawn into this cell, but something was
                    // claimed: carry the claim on its own level, where ink
                    // outranks a hardblank exactly as in [`pixel::blank_op`].
                    // (`set_detail` would classify the empty region as the
                    // clear cell and drop both.)
                    let shape = if any_filled {
                        PixelShape::new(crate::pixel::PX_EMPTY, true)
                    } else if any_hardblank {
                        PixelShape::new(crate::pixel::PX_HARDBLANK, false)
                    } else {
                        PixelShape::EMPTY
                    };
                    out.set(r, c, shape);
                    continue;
                }
                out.set_detail(r, c, &region, any_filled);
            }
        }
        out
    }

    /// Replace every custom detail with the closest catalog shape
    /// ([`DetailRegion::nearest_shape`]), leaving a grid that `.unf` can spell.
    ///
    /// A grid on its way back into a document must go through this: the format
    /// writes one shape code per cell and has no syntax for exact geometry, so
    /// a `PX_CUSTOM` cell would serialize as the unknown-code `??`. Only the
    /// editor needs it — resolution and the builder consume grids in memory,
    /// where the exact regions are the point.
    #[cfg(feature = "editor")]
    pub fn snap_details_to_catalog(&mut self) {
        if self.details.is_empty() && self.den == 1 {
            return;
        }
        for r in 0..self.height {
            for c in 0..self.width {
                if self.get(r, c).shape_id() != PX_CUSTOM {
                    continue;
                }
                let filled = self.get(r, c).is_bitmap_filled();
                let id = self.region_at(r, c).nearest_shape();
                let shape = if id == crate::pixel::PX_EMPTY {
                    PixelShape::EMPTY
                } else {
                    PixelShape::new(id, filled)
                };
                self.set(r, c, shape);
            }
        }
        self.details.clear();
        self.den = 1;
    }

    pub fn is_all_empty(&self) -> bool {
        self.pixels.iter().all(|s| s.is_clear())
    }

    /// Geometric-transform skeleton shared by the five transforms below,
    /// mirroring `DetailRegion::map_lattice` one layer down: `coord` maps a
    /// source cell to its destination, while `shape`/`detail` transform the
    /// cell contents to match.
    #[cfg(feature = "editor")]
    fn map_coords(
        &self,
        new_w: u16,
        new_h: u16,
        coord: impl Fn(u16, u16) -> (u16, u16),
        shape: impl Fn(PixelShape) -> PixelShape,
        detail: impl Fn(&detail::DetailRegion) -> detail::DetailRegion,
    ) -> Self {
        let mut out = Self::new(new_w, new_h);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                let (nr, nc) = coord(r, c);
                out.set(nr, nc, shape(self.get(r, c)));
            }
        }
        for (&(r, c), d) in &self.details {
            out.details.insert(coord(r, c), detail(d));
        }
        out
    }

    #[cfg(feature = "editor")]
    pub fn mirror_h(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(
            w,
            h,
            |r, c| (r, w - 1 - c),
            |s| s.mirror_h(),
            |d| d.mirror_h(),
        )
    }

    #[cfg(feature = "editor")]
    pub fn flip_v(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(w, h, |r, c| (h - 1 - r, c), |s| s.flip_v(), |d| d.flip_v())
    }

    /// A copy of this grid with its contents moved by `(dcol, drow)` whole
    /// cells. The grid keeps its size, so whatever crosses an edge is dropped.
    #[cfg(feature = "editor")]
    pub fn shifted(&self, dcol: i16, drow: i16) -> Self {
        let mut out = Self::new(self.width, self.height);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                let nr = r as i32 + drow as i32;
                let nc = c as i32 + dcol as i32;
                if nr < 0 || nc < 0 || nr >= self.height as i32 || nc >= self.width as i32 {
                    continue;
                }
                let (nr, nc) = (nr as u16, nc as u16);
                out.set(nr, nc, self.get(r, c));
                if let Some(region) = self.details.get(&(r, c)) {
                    out.details.insert((nr, nc), region.clone());
                }
            }
        }
        out
    }

    #[cfg(feature = "editor")]
    pub fn rotate_cw(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(
            h,
            w,
            |r, c| (c, h - 1 - r),
            |s| s.rotate_cw(),
            |d| d.rotate_cw(),
        )
    }

    #[cfg(feature = "editor")]
    pub fn rotate_ccw(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(
            h,
            w,
            |r, c| (w - 1 - c, r),
            |s| s.rotate_ccw(),
            |d| d.rotate_ccw(),
        )
    }

    #[cfg(feature = "editor")]
    pub fn rotate_180(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(
            w,
            h,
            |r, c| (h - 1 - r, w - 1 - c),
            |s| s.rotate_180(),
            |d| d.rotate_180(),
        )
    }

    #[cfg(feature = "editor")]
    pub fn opposite(&self) -> Self {
        let mut out = self.clone();
        for px in &mut out.pixels {
            *px = px.opposite();
        }
        // Custom cells: PixelShape::opposite mangles the sentinel id, so
        // complement the stored geometry instead.
        for (&(r, c), region) in &self.details {
            let filled = !self.get(r, c).is_bitmap_filled();
            out.set_detail(r, c, &region.complement(), filled);
        }
        out
    }

    #[cfg(feature = "editor")]
    pub fn opposite_bitmap(&self) -> Self {
        let mut out = self.clone();
        for px in &mut out.pixels {
            *px = px.opposite_bitmap();
        }
        out
    }

    /// Blit `src` into `self` with its top-left at `(off_r, off_c)`,
    /// overwriting the destination wherever `src` has a non-empty shape.
    /// When `negated`, `src` regions are instead subtracted exactly from
    /// non-empty destination pixels (re-encoded as plain shape codes when
    /// possible, custom details otherwise).
    pub fn blit(&mut self, src: &PixelGrid, off_r: i32, off_c: i32, negated: bool) {
        for r in 0..src.height as i32 {
            for c in 0..src.width as i32 {
                let shape = src.get(r as u16, c as u16);
                if shape.is_clear() {
                    continue;
                }
                let dr = off_r + r;
                let dc = off_c + c;
                if dr < 0 || dc < 0 || dr >= self.height as i32 || dc >= self.width as i32 {
                    continue;
                }
                let current = self.get(dr as u16, dc as u16);
                // A hardblank is a claim rather than geometry, so a pair
                // involving one is settled before the region layer, which can
                // only see the nothing it draws. See [`pixel::blank_op`].
                if let Some(blank) = crate::pixel::blank_op(current, shape, negated) {
                    if blank.0 != current.0 {
                        self.set(dr as u16, dc as u16, blank);
                    }
                    continue;
                }
                let src_custom = shape.shape_id() == PX_CUSTOM;
                if negated {
                    if current.is_clear() {
                        continue;
                    }
                    let cur_custom = current.shape_id() == PX_CUSTOM;
                    if !cur_custom && !src_custom {
                        // Plain − plain: the result depends only on the two
                        // catalog ids — use the memoized table.
                        self.apply_classified(
                            dr as u16,
                            dc as u16,
                            detail::catalog_subtract(current.shape_id(), shape.shape_id()),
                            current.is_bitmap_filled(),
                        );
                        continue;
                    }
                    let cur_region = self.region_at(dr as u16, dc as u16);
                    let sub = src.region_at(r as u16, c as u16);
                    self.apply_classified(
                        dr as u16,
                        dc as u16,
                        detail::subtract_classified(&cur_region, &sub),
                        current.is_bitmap_filled(),
                    );
                } else {
                    if current.is_clear() {
                        if src_custom {
                            let region = src.region_at(r as u16, c as u16);
                            self.set_detail(
                                dr as u16,
                                dc as u16,
                                &region,
                                shape.is_bitmap_filled(),
                            );
                        } else {
                            self.set(dr as u16, dc as u16, shape);
                        }
                    } else {
                        let filled = current.is_bitmap_filled() || shape.is_bitmap_filled();
                        // The sweep is the general answer, but not every pair
                        // needs one: a cell either side already covers whole
                        // is that cell, and two equal shape ids describe the
                        // same region — except `PX_CUSTOM`, the one id many
                        // regions share. Union is on the hot path of every
                        // composite flattening, so these three cost nothing
                        // and skip most of the work a han glyph would do.
                        let (cur_id, src_id) = (current.shape_id(), shape.shape_id());
                        if src_id == PX_ALMOSTFULL {
                            self.set(dr as u16, dc as u16, PixelShape::new(PX_ALMOSTFULL, filled));
                            continue;
                        }
                        if cur_id == PX_ALMOSTFULL {
                            self.set_filled(dr as u16, dc as u16, filled);
                            continue;
                        }
                        if cur_id == src_id && cur_id != PX_CUSTOM {
                            self.set_filled(dr as u16, dc as u16, filled);
                            continue;
                        }
                        let cur_region = self.region_at(dr as u16, dc as u16);
                        let src_region = src.region_at(r as u16, c as u16);
                        self.apply_classified(
                            dr as u16,
                            dc as u16,
                            detail::bool_op(&cur_region, &src_region, detail::BoolOp::Union)
                                .classify(),
                            filled,
                        );
                    }
                }
            }
        }
    }
}
