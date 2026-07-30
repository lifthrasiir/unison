//! The `.unf` data model: [`Document`] and its [`DocumentItem`]s, [`DocLine`]
//! (the line-level model the editor actually edits), [`PixelGrid`] and the glyph
//! bodies hanging off them.
//!
//! The parser and serializer — and the reference for the surface syntax — are in
//! [`crate::document_io`]. Name expansion lives in [`crate::pattern`], whose API
//! this module re-exports for the legacy import paths that predate the split.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

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
            self.details.get(&(row, col)).cloned().unwrap_or(DetailRegion::EMPTY)
        } else {
            DetailRegion::from_shape(shape.shape_id())
        }
    }

    pub fn resize(&mut self, new_width: u16, new_height: u16) {
        if new_width == self.width && new_height == self.height {
            return;
        }
        let mut new_pixels =
            vec![PixelShape::EMPTY; new_width as usize * new_height as usize];
        let copy_w = self.width.min(new_width) as usize;
        let copy_h = self.height.min(new_height) as usize;
        for r in 0..copy_h {
            for c in 0..copy_w {
                new_pixels[r * new_width as usize + c] =
                    self.pixels[r * self.width as usize + c];
            }
        }
        self.width = new_width;
        self.height = new_height;
        self.pixels = new_pixels;
        self.details.retain(|&(r, c), _| r < new_height && c < new_width);
    }

    /// Exact geometric rescale between two pixel-subdivision scales. Every
    /// destination pixel receives the union of the source regions that
    /// overlap it, mapped exactly; results are re-encoded as plain shape
    /// codes when possible and stored as custom details otherwise.
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
        map.entry(key).or_default().push((
            self.clone(),
            old_scale,
            new_scale,
            out.clone(),
        ));
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
                    + if ((c as i64 + 1) * old_s).rem_euclid(new_s) != 0 { 1 } else { 0 };
                let sr0 = (r as i64 * old_s).div_euclid(new_s);
                let sr1 = ((r as i64 + 1) * old_s).div_euclid(new_s)
                    + if ((r as i64 + 1) * old_s).rem_euclid(new_s) != 0 { 1 } else { 0 };

                // Fast path: all overlapping source pixels are uniformly
                // full or uniformly empty — no geometry needed.
                let mut all_full = true;
                let mut all_empty = true;
                let mut any_filled = false;
                for sr in sr0..sr1 {
                    for sc in sc0..sc1 {
                        let s = self.get(sr as u16, sc as u16);
                        let id = s.shape_id();
                        if id != PX_ALMOSTFULL {
                            all_full = false;
                        }
                        if id != crate::pixel::PX_EMPTY {
                            all_empty = false;
                        }
                        any_filled |= s.is_filled();
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
                        if s.shape_id() == crate::pixel::PX_EMPTY {
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
                out.set_detail(r, c, &region, any_filled);
            }
        }
        out
    }

    pub fn is_all_empty(&self) -> bool {
        self.pixels.iter().all(|s| s.is_empty())
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
        self.map_coords(w, h, |r, c| (r, w - 1 - c), |s| s.mirror_h(), |d| d.mirror_h())
    }

    #[cfg(feature = "editor")]
    pub fn flip_v(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(w, h, |r, c| (h - 1 - r, c), |s| s.flip_v(), |d| d.flip_v())
    }

    #[cfg(feature = "editor")]
    pub fn rotate_cw(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(h, w, |r, c| (c, h - 1 - r), |s| s.rotate_cw(), |d| d.rotate_cw())
    }

    #[cfg(feature = "editor")]
    pub fn rotate_ccw(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(h, w, |r, c| (w - 1 - c, r), |s| s.rotate_ccw(), |d| d.rotate_ccw())
    }

    #[cfg(feature = "editor")]
    pub fn rotate_180(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.map_coords(w, h, |r, c| (h - 1 - r, w - 1 - c), |s| s.rotate_180(), |d| d.rotate_180())
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
            let filled = !self.get(r, c).is_filled();
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
                if shape.is_empty() {
                    continue;
                }
                let dr = off_r + r;
                let dc = off_c + c;
                if dr < 0 || dc < 0 || dr >= self.height as i32 || dc >= self.width as i32 {
                    continue;
                }
                let src_custom = shape.shape_id() == PX_CUSTOM;
                if negated {
                    let current = self.get(dr as u16, dc as u16);
                    if current.is_empty() {
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
                            current.is_filled(),
                        );
                        continue;
                    }
                    let cur_region = self.region_at(dr as u16, dc as u16);
                    let sub = src.region_at(r as u16, c as u16);
                    self.apply_classified(
                        dr as u16,
                        dc as u16,
                        detail::subtract_classified(&cur_region, &sub),
                        current.is_filled(),
                    );
                } else {
                    let current = self.get(dr as u16, dc as u16);
                    if current.is_empty() {
                        if src_custom {
                            let region = src.region_at(r as u16, c as u16);
                            self.set_detail(dr as u16, dc as u16, &region, shape.is_filled());
                        } else {
                            self.set(dr as u16, dc as u16, shape);
                        }
                    } else {
                        let cur_region = self.region_at(dr as u16, dc as u16);
                        let src_region = src.region_at(r as u16, c as u16);
                        self.apply_classified(
                            dr as u16,
                            dc as u16,
                            detail::bool_op(&cur_region, &src_region, detail::BoolOp::Union).classify(),
                            current.is_filled() || shape.is_filled(),
                        );
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerVisibility {
    Both,
    ColorOnly,
    MonoOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefFill {
    pub color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRef {
    pub name: String,
    /// `(col, row)` offset. `None` = auto-resolve from points (adjoin), defaulting to (0, 0).
    pub offset: Option<(i16, i16)>,
    pub negated: bool,
    /// Whether the composite exposes this target's surviving anchors (the ones
    /// not consumed inside the composite) as its own. Off by default: anchor
    /// inheritance is opt-in, so a digraph or a circled letter does not leak
    /// its components' attachment points. Attachment *inside* the composite
    /// (a sibling ref consuming this target's `+` anchors) works regardless.
    pub inherit: bool,
    pub fill: Option<RefFill>,
    pub visibility: Option<LayerVisibility>,
    /// Trailing `// …` comment of the `ref` line, without its marker.
    pub comment: Option<String>,
}

impl GlyphRef {
    pub fn row(&self) -> i16 {
        self.offset.map_or(0, |(_, r)| r)
    }

    pub fn col(&self) -> i16 {
        self.offset.map_or(0, |(c, _)| c)
    }

    /// Format as a `ref …` line. When `offset_override` is `Some`, that
    /// offset is written instead of `self.offset` (and is always explicit,
    /// even for `0 0`).
    #[cfg(any(feature = "editor", test))]
    pub fn format_line(&self, offset_override: Option<(i16, i16)>) -> String {
        use crate::document_io::quote_token;
        let rname = quote_token(&self.name);
        let mut parts = vec![format!("ref {rname}")];
        match offset_override {
            Some((c, r)) => parts.push(format!("{c} {r}")),
            None => {
                if let Some((c, r)) = self.offset {
                    parts.push(format!("{c} {r}"));
                }
            }
        }
        if self.negated {
            parts.push("negated".into());
        }
        if self.inherit {
            parts.push("inherit".into());
        }
        if let Some(ref fill) = self.fill {
            parts.push(format!("fill {}", quote_token(&fill.color)));
        }
        match self.visibility {
            Some(LayerVisibility::ColorOnly) => parts.push("coloronly".into()),
            Some(LayerVisibility::MonoOnly) => parts.push("monoonly".into()),
            Some(LayerVisibility::Both) | None => {}
        }
        format!(
            "{}{}",
            parts.join(" "),
            crate::document_io::comment_suffix(&self.comment),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphPoint {
    pub position: String,
    pub col: i16,
    pub row: i16,
    /// Inclusive end of the column range. Equal to `col` for single-cell anchors.
    pub col_end: i16,
    /// Inclusive end of the row range. Equal to `row` for single-cell anchors.
    pub row_end: i16,
    /// Trailing `// …` comment of the `point`/`anchor` line, without its marker.
    pub comment: Option<String>,
}

impl GlyphPoint {
    pub fn width(&self) -> u16 {
        (self.col_end - self.col + 1) as u16
    }

    pub fn height(&self) -> u16 {
        (self.row_end - self.row + 1) as u16
    }

    #[cfg(any(feature = "editor", test))]
    pub fn is_single_cell(&self) -> bool {
        self.col == self.col_end && self.row == self.row_end
    }

    pub fn size_matches(&self, other: &GlyphPoint) -> bool {
        self.width() == other.width() && self.height() == other.height()
    }

    /// The `anchor` line for this point, comment included. Single implementation
    /// shared by the serializer and by layer dragging in the editor.
    #[cfg(any(feature = "editor", test))]
    pub fn format_line(&self) -> String {
        let range = |start: i16, end: i16| {
            if start == end {
                format!("{start}")
            } else {
                format!("{start}..{end}")
            }
        };
        format!(
            "anchor {} {} {}{}",
            crate::document_io::quote_token(&self.position),
            range(self.col, self.col_end),
            range(self.row, self.row_end),
            crate::document_io::comment_suffix(&self.comment),
        )
    }

    /// A copy of this point moved by `(dcol, drow)` whole cells.
    #[cfg(feature = "editor")]
    pub fn shifted(&self, dcol: i16, drow: i16) -> GlyphPoint {
        GlyphPoint {
            col: self.col + dcol,
            col_end: self.col_end + dcol,
            row: self.row + drow,
            row_end: self.row_end + drow,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBody {
    pub pixels: Option<PixelGrid>,
    pub refs: Vec<GlyphRef>,
    pub points: Vec<GlyphPoint>,
    pub sticky: bool,
    pub inline: bool,
    pub mark: bool,
    pub advance: Option<u16>,
    pub left: Option<i16>,
    pub top: Option<i16>,
    pub scale: u8,
    /// Trailing `// …` comment of the `glyph` header line, without its marker.
    pub comment: Option<String>,
}

impl GlyphBody {
    pub fn new() -> Self {
        Self {
            pixels: None,
            refs: Vec::new(),
            points: Vec::new(),
            sticky: false,
            inline: false,
            mark: false,
            advance: None,
            left: None,
            top: None,
            scale: 1,
            comment: None,
        }
    }

    /// True if this body is a simple alias (`glyph NAME = ALIAS`): no pixel
    /// data, exactly one ref, with no positional offset.
    #[cfg(any(feature = "editor", test))]
    pub fn is_simple_alias(&self) -> bool {
        self.pixels.is_none()
            && self.refs.len() == 1
            && self.refs[0].offset.is_none()
            && !self.refs[0].negated
            && self.refs[0].fill.is_none()
            && self.refs[0].visibility.is_none()
            && self.points.is_empty()
            && !self.sticky
            && !self.inline
            && !self.mark
            && self.advance.is_none()
            && self.left.is_none()
            && self.top.is_none()
            && self.scale == 1
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphName(pub String);

impl GlyphName {
    pub fn display(&self) -> String {
        self.0.clone()
    }
}

/// What a [`DocumentItem::Directive`]'s raw text means.
///
/// `document_io` keeps directives that have no typed item as raw text, so
/// every consumer used to re-parse them with its own `strip_prefix` chain and
/// its own idea of which keywords are recognized — five copies that had to be
/// kept in sync with the parser by hand. This is that knowledge, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Directive<'a> {
    /// `exclude-from-sample NAME...` — the argument text.
    ExcludeFromSample(&'a str),
    /// `assume unused NAME...` — the argument text.
    AssumeUnused(&'a str),
    /// Blank or whitespace only.
    Empty,
    /// A keyword we do not know, or a known keyword whose arguments did not
    /// parse into a typed item.
    Unrecognized,
}

/// Note: this deliberately does *not* know about the directives that parse
/// into typed items (`name-parts`, `remap`, `feature`, `color`, `assert`).
/// Those only reach [`DocumentItem::Directive`] when malformed, and are
/// reported as unrecognized so the author hears about the typo.
pub fn classify_directive(text: &str) -> Directive<'_> {
    // Raw-text directives keep their `// …` comment inline, so it has to come
    // off before the arguments are read.
    let (text, _) = crate::document_io::split_comment(text);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Directive::Empty;
    }
    if let Some(rest) = trimmed.strip_prefix("exclude-from-sample ") {
        return Directive::ExcludeFromSample(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("assume unused ") {
        return Directive::AssumeUnused(rest);
    }
    // `assert` lines that parse become typed items, so an `assert` reaching
    // here is malformed and should be reported like any other unknown line.
    Directive::Unrecognized
}

#[derive(Clone, Debug, PartialEq)]
pub enum DocumentItem {
    Comment(String),
    BlankLine,
    Directive(String),
    FontMeta(String),
    Glyph {
        name: GlyphName,
        body: GlyphBody,
    },
    /// `map CHAR = GLYPH` — cmap mapping from a Unicode character to a glyph name.
    Map {
        char_repr: String,
        glyph: String,
        comment: Option<String>,
    },
    /// `map generate CHAR [= GLYPH]` — auto-decomposed cmap mapping. The glyph
    /// is synthesized from the character's Unicode canonical decomposition and
    /// named `uniXXXX` unless `glyph` names it.
    MapDecomposed {
        char_repr: String,
        glyph: Option<String>,
        comment: Option<String>,
    },
    /// `name-parts $NAME = token1 token2 $ref3 ...`
    NameParts {
        name: String,
        values: Vec<String>,
        comment: Option<String>,
    },
    /// `remap FEATURE : [LOOKBEHIND... :] SOURCE -> TARGET [: LOOKAHEAD...]`
    Remap {
        feature: String,
        lookbehind: Vec<String>,
        source: Vec<String>,
        target: Vec<String>,
        lookahead: Vec<String>,
        comment: Option<String>,
    },
    /// `feature NAME for SCRIPT... : REMAP_GROUP`
    Feature {
        name: String,
        scripts: Vec<String>,
        remap_group: String,
        comment: Option<String>,
    },
    /// `feature NAME for SCRIPT... : anchor ANCHOR_NAME`
    FeatureAnchor {
        name: String,
        scripts: Vec<String>,
        anchor: String,
        comment: Option<String>,
    },
    /// `color NAME = #xxxxxx[xx]|COLORNAME [coloronly|monoonly]`
    Color {
        name: String,
        value: String,
        visibility: Option<LayerVisibility>,
        comment: Option<String>,
    },
    /// `assert shape \`text\` [@lang] [+feat] [-feat] : glyph1 [advance N] [offset X Y] : glyph2 ...`
    AssertShape {
        text: String,
        features: Vec<ShapeFeatureFlag>,
        /// BCP 47 language the text is shaped as, from an `@tag` token.
        ///
        /// Deliberately *not* the `script/LANG` notation a `feature` directive
        /// uses: an assertion states the input a real client hands the shaper,
        /// and the OpenType language system is what the shaper is supposed to
        /// derive from it. Writing `@ROM` on both sides would make the two
        /// agree by construction and stop the assertion from noticing that
        /// Romanian does not resolve to the tag the font declared.
        language: Option<String>,
        expected: Vec<ExpectedGlyph>,
        comment: Option<String>,
    },
    /// `assert same GLYPH1 GLYPH2 ...`
    AssertSame {
        names: Vec<String>,
        comment: Option<String>,
    },
    /// `assert distinct GLYPH1 GLYPH2 ...`
    AssertDistinct {
        names: Vec<String>,
        comment: Option<String>,
    },
}

impl DocumentItem {
    /// The glyph names a `remap` rule names, in rule order. Empty for every
    /// other item. Enumerating the four operand lists by hand is easy to get
    /// subtly wrong — a forgotten `lookahead` silently narrows a check.
    pub fn remap_operands(&self) -> impl Iterator<Item = &String> {
        let lists: [&[String]; 4] = match self {
            DocumentItem::Remap { source, target, lookbehind, lookahead, .. } => {
                [source, target, lookbehind, lookahead]
            }
            _ => [&[], &[], &[], &[]],
        };
        lists.into_iter().flatten()
    }

    #[cfg(feature = "editor")]
    pub fn affects_font(&self) -> bool {
        !matches!(
            self,
            DocumentItem::Comment(_)
                | DocumentItem::BlankLine
                | DocumentItem::Directive(_)
                | DocumentItem::AssertShape { .. }
                | DocumentItem::AssertSame { .. }
                | DocumentItem::AssertDistinct { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeFeatureFlag {
    pub tag: String,
    pub enable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedGlyph {
    pub name: String,
    pub advance: Option<i32>,
    pub offset: Option<(i32, i32)>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct Document {
    pub items: Vec<DocumentItem>,
    pub item_line_starts: Vec<usize>,
    /// Maps each DocLine index to its 0-based file line number.
    pub docline_file_lines: Vec<usize>,
    pub path: PathBuf,
    pub dirty: bool,
    pub edit_gen: u64,
    pub pixel_gen: u64,
    /// Incremented only when `items` actually change (not on every keystroke).
    pub content_gen: u64,
}

impl Document {
    pub fn new(path: PathBuf) -> Self {
        Self {
            items: Vec::new(),
            item_line_starts: Vec::new(),
            docline_file_lines: Vec::new(),
            path,
            dirty: false,
            edit_gen: 0,
            pixel_gen: 0,
            content_gen: 0,
        }
    }

    /// 1-based line of `docline_idx` in the serialized file.
    pub fn docline_file_line(&self, docline_idx: usize) -> usize {
        self.docline_file_lines.get(docline_idx).copied().unwrap_or(docline_idx) + 1
    }

    /// `(docline, 1-based file line)` of item `item_idx`'s defining header.
    pub fn item_lines(&self, item_idx: usize) -> (usize, usize) {
        let line = self.item_line_starts.get(item_idx).copied().unwrap_or(0);
        (line, self.docline_file_line(line))
    }
}

pub fn compute_docline_file_lines(lines: &[DocLine]) -> Vec<usize> {
    let mut result = Vec::with_capacity(lines.len());
    let mut file_line = 0usize;
    for line in lines {
        result.push(file_line);
        match line {
            DocLine::Text(_) => file_line += 1,
            DocLine::Grid(grid) => {
                if !grid.is_all_empty() {
                    file_line += grid.height as usize;
                }
            }
        }
    }
    result
}

#[cfg(any(feature = "editor", test))]
use crate::document_io::comment_suffix as serialize_comment_suffix;

impl DocumentItem {
    /// Parse a structured directive from pre-tokenized tokens (the line's
    /// `// …` comment already split off and passed as `comment`).
    /// The first token is the keyword ("name-parts", "remap", or "feature").
    pub fn parse_directive(tokens: &[String], comment: Option<String>) -> DocumentItem {
        if tokens.is_empty() {
            return DocumentItem::Directive(String::new());
        }
        match tokens[0].as_str() {
            "name-parts" => {
                let rest = &tokens[1..];
                if rest.len() >= 3 && rest[1] == "=" {
                    return DocumentItem::NameParts {
                        name: rest[0].clone(),
                        values: rest[2..].to_vec(),
                        comment,
                    };
                }
            }
            "assert" => {
                if tokens.get(1).is_some_and(|t| t == "shape") {
                    if let Some(item) = Self::parse_assert_shape(&tokens[2..], comment.clone()) {
                        return item;
                    }
                }
                match tokens.get(1).map(|s| s.as_str()) {
                    Some("same") if tokens.len() >= 4 => {
                        return DocumentItem::AssertSame { names: tokens[2..].to_vec(), comment };
                    }
                    Some("distinct") if tokens.len() >= 4 => {
                        return DocumentItem::AssertDistinct { names: tokens[2..].to_vec(), comment };
                    }
                    _ => {}
                }
            }
            "remap" => {
                if let Some(item) = Self::parse_remap(&tokens[1..], comment.clone()) {
                    return item;
                }
            }
            "feature" => {
                let rest = &tokens[1..];
                // feature NAME for SCRIPT... : REMAP_GROUP
                // feature NAME for SCRIPT... : anchor ANCHOR_NAME
                if let Some(for_pos) = rest.iter().position(|t| t == "for")
                    && let Some(colon_pos) = rest.iter().position(|t| t == ":")
                        && for_pos == 1 && colon_pos > 2 && colon_pos + 1 < rest.len() {
                            if rest.get(colon_pos + 1).is_some_and(|t| t == "anchor")
                                && colon_pos + 2 < rest.len()
                            {
                                return DocumentItem::FeatureAnchor {
                                    name: rest[0].clone(),
                                    scripts: rest[2..colon_pos].to_vec(),
                                    anchor: rest[colon_pos + 2].clone(),
                                    comment,
                                };
                            }
                            return DocumentItem::Feature {
                                name: rest[0].clone(),
                                scripts: rest[2..colon_pos].to_vec(),
                                remap_group: rest[colon_pos + 1].clone(),
                                comment,
                            };
                        }
            }
            _ => {}
        }
        // Malformed: keep the line as raw text, comment included, so nothing
        // is lost on the way back out.
        let quoted: Vec<String> = tokens.iter().map(|t| crate::document_io::quote_token(t)).collect();
        let comment = match comment {
            Some(c) => format!(" // {c}"),
            None => String::new(),
        };
        DocumentItem::Directive(format!("{}{}", quoted.join(" "), comment))
    }

    fn parse_remap(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        let arrow_pos = tokens.iter().position(|t| t == "->")?;

        let colon_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| t.as_str() == ":")
            .map(|(i, _)| i)
            .collect();

        let first = tokens.first()?;
        let (feature, first_colon_after_feature) = if first.ends_with(':') && first.len() > 1 {
            (first.trim_end_matches(':').to_string(), 1)
        } else {
            let fc = colon_positions.iter().copied().find(|&p| p < arrow_pos)?;
            (first.clone(), fc + 1)
        };

        let last_colon_before_arrow = colon_positions
            .iter()
            .copied().rfind(|&p| p >= first_colon_after_feature && p < arrow_pos);

        let (lookbehind, source_start) = if let Some(lc) = last_colon_before_arrow {
            let lb: Vec<String> = tokens[first_colon_after_feature..lc].to_vec();
            (lb, lc + 1)
        } else {
            (Vec::new(), first_colon_after_feature)
        };

        let source = tokens[source_start..arrow_pos].to_vec();

        let after_arrow = arrow_pos + 1;
        let lookahead_colon = colon_positions.iter().copied().find(|&p| p > arrow_pos);

        let (target, lookahead) = if let Some(lc) = lookahead_colon {
            let target = tokens[after_arrow..lc].to_vec();
            let la: Vec<String> = tokens[lc + 1..].to_vec();
            (target, la)
        } else {
            (tokens[after_arrow..].to_vec(), Vec::new())
        };

        Some(DocumentItem::Remap {
            feature,
            lookbehind,
            source,
            target,
            lookahead,
            comment,
        })
    }

    fn parse_assert_shape(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        if tokens.is_empty() {
            return None;
        }
        let text = tokens[0].clone();

        let first_colon = tokens.iter().position(|t| t == ":")?;

        let mut features = Vec::new();
        let mut language = None;
        for tok in &tokens[1..first_colon] {
            if let Some(tag) = tok.strip_prefix('+') {
                features.push(ShapeFeatureFlag { tag: tag.to_string(), enable: true });
            } else if let Some(tag) = tok.strip_prefix('-') {
                features.push(ShapeFeatureFlag { tag: tag.to_string(), enable: false });
            } else if let Some(tag) = tok.strip_prefix('@')
                && !tag.is_empty()
                && language.is_none()
            {
                language = Some(tag.to_string());
            }
        }

        let glyph_tokens = &tokens[first_colon + 1..];
        let mut expected = Vec::new();
        let mut segments: Vec<&[String]> = Vec::new();

        let mut start = 0;
        for (i, tok) in glyph_tokens.iter().enumerate() {
            if tok == ":" && i > start {
                segments.push(&glyph_tokens[start..i]);
                start = i + 1;
            }
        }
        if start < glyph_tokens.len() {
            segments.push(&glyph_tokens[start..]);
        }

        for seg in segments {
            if seg.is_empty() {
                continue;
            }
            let name = seg[0].clone();
            let mut advance = None;
            let mut offset = None;
            let mut i = 1;
            while i < seg.len() {
                match seg[i].as_str() {
                    "advance" if i + 1 < seg.len() => {
                        advance = seg[i + 1].parse().ok();
                        i += 2;
                    }
                    "offset" if i + 2 < seg.len() => {
                        if let (Ok(x), Ok(y)) = (seg[i + 1].parse(), seg[i + 2].parse()) {
                            offset = Some((x, y));
                        }
                        i += 3;
                    }
                    _ => { i += 1; }
                }
            }
            expected.push(ExpectedGlyph { name, advance, offset });
        }

        if expected.is_empty() {
            return None;
        }

        Some(DocumentItem::AssertShape { text, features, language, expected, comment })
    }

    #[cfg(any(feature = "editor", test))]
    pub fn serialize_line(&self) -> Option<String> {
        use crate::document_io::quote_token;
        match self {
            DocumentItem::NameParts { name, values, comment } => {
                let qvals: Vec<String> = values.iter().map(|v| quote_token(v)).collect();
                Some(format!(
                    "name-parts {} = {}{}",
                    quote_token(name),
                    qvals.join(" "),
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::Remap {
                feature,
                lookbehind,
                source,
                target,
                lookahead,
                comment,
            } => {
                let mut parts = vec![format!("remap {} :", quote_token(feature))];
                if !lookbehind.is_empty() {
                    let lb: Vec<String> = lookbehind.iter().map(|s| quote_token(s)).collect();
                    parts.push(format!("{} :", lb.join(" ")));
                }
                let qs: Vec<String> = source.iter().map(|s| quote_token(s)).collect();
                let qt: Vec<String> = target.iter().map(|s| quote_token(s)).collect();
                parts.push(format!("{} -> {}", qs.join(" "), qt.join(" ")));
                if !lookahead.is_empty() {
                    let la: Vec<String> = lookahead.iter().map(|s| quote_token(s)).collect();
                    parts.push(format!(": {}", la.join(" ")));
                }
                Some(format!("{}{}", parts.join(" "), serialize_comment_suffix(comment)))
            }
            DocumentItem::Feature { name, scripts, remap_group, comment } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                Some(format!(
                    "feature {} for {} : {}{}",
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(remap_group),
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::FeatureAnchor { name, scripts, anchor, comment } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                Some(format!(
                    "feature {} for {} : anchor {}{}",
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(anchor),
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::Color { name, value, visibility, comment } => {
                let vis = match visibility {
                    Some(LayerVisibility::ColorOnly) => " coloronly",
                    Some(LayerVisibility::MonoOnly) => " monoonly",
                    _ => "",
                };
                Some(format!(
                    "color {} = {}{}{}",
                    quote_token(name),
                    quote_token(value),
                    vis,
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::AssertShape { text, features, language, expected, comment } => {
                let mut parts = vec![
                    "assert".to_string(),
                    "shape".to_string(),
                    quote_token(text),
                ];
                if let Some(lang) = language {
                    parts.push(format!("@{lang}"));
                }
                for f in features {
                    let prefix = if f.enable { "+" } else { "-" };
                    parts.push(format!("{prefix}{}", f.tag));
                }
                for (i, g) in expected.iter().enumerate() {
                    parts.push(":".to_string());
                    parts.push(quote_token(&g.name));
                    if let Some(adv) = g.advance {
                        parts.push("advance".to_string());
                        parts.push(adv.to_string());
                    }
                    if let Some((x, y)) = g.offset {
                        parts.push("offset".to_string());
                        parts.push(x.to_string());
                        parts.push(y.to_string());
                    }
                    let _ = i;
                }
                Some(format!("{}{}", parts.join(" "), serialize_comment_suffix(comment)))
            }
            DocumentItem::AssertSame { names, comment } => {
                let qnames: Vec<String> = names.iter().map(|n| quote_token(n)).collect();
                Some(format!("assert same {}{}", qnames.join(" "), serialize_comment_suffix(comment)))
            }
            DocumentItem::AssertDistinct { names, comment } => {
                let qnames: Vec<String> = names.iter().map(|n| quote_token(n)).collect();
                Some(format!("assert distinct {}{}", qnames.join(" "), serialize_comment_suffix(comment)))
            }
            _ => None,
        }
    }
}

// Name pattern parsing/expansion and `$var` substitution live in
// `crate::pattern`; re-exported here because most consumers reach them
// through `crate::document`.
pub use crate::pattern::{
    MAX_EXPANSION, NamePartsMap, NamePattern, expand_name_element, find_invalid_inline_ranges,
    has_top_level_pipe, is_name_pattern, parse_name_element, split_top_level_pipes,
    substitute_name_parts,
};

// ---------------------------------------------------------------------------
// Name-parts collection
// ---------------------------------------------------------------------------

pub fn collect_name_parts(docs: &[&Document]) -> NamePartsMap {
    let mut map = NamePartsMap::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::NameParts { name, values, .. } = item {
                let mut resolved = Vec::new();
                for token in values {
                    if token.starts_with('$') {
                        if let Some(referenced) = map.get(token.as_str()) {
                            if referenced.len() > MAX_EXPANSION.saturating_sub(resolved.len()) {
                                resolved.push(token.clone());
                            } else {
                                resolved.extend(referenced.iter().cloned());
                            }
                        } else {
                            resolved.push(token.clone());
                        }
                    } else {
                        for part in token.split('|') {
                            if part.is_empty() || part == "``" {
                                resolved.push(String::new());
                            } else if let Some((name_part, rep_str)) = part.rsplit_once('*') {
                                if let Ok(rep) = rep_str.parse::<usize>() {
                                    if rep > MAX_EXPANSION.saturating_sub(resolved.len()) {
                                        resolved.push(part.to_string());
                                    } else {
                                        for _ in 0..rep {
                                            resolved.push(name_part.to_string());
                                        }
                                    }
                                } else {
                                    resolved.push(part.to_string());
                                }
                            } else {
                                resolved.push(part.to_string());
                            }
                        }
                    }
                }
                map.insert(name.clone(), resolved);
            }
        }
    }
    map
}

pub fn parse_glyph_name(s: &str) -> GlyphName {
    GlyphName(s.trim().to_string())
}

/// Expand a ref-only glyph item (`glyph NAME` + `ref ...` lines, no pixel
/// data) whose name and/or ref targets carry alternation/range patterns,
/// directly from its in-memory `GlyphName`/`GlyphRef`s (no serialize/reparse
/// round-trip through `.unf` text).
///
/// Mirrors the historical behavior exactly: pixel data is not meaningful
/// for a batch of expanded ref-composites, so expanded items always come
/// out as `pixels: None` — this function is only ever called on an
/// already-pattern-named item, and `.unf` content never combines a
/// pattern name with pixel data on the same glyph (patterns are only
/// used for ref/composite batches).
pub fn expand_glyph_block(name: &GlyphName, refs: &[GlyphRef], scale: u8) -> Result<Vec<DocumentItem>, String> {
    let name_pattern = NamePattern::parse(&name.display()).map_err(|e| e.to_string())?;

    let mut parsed_refs: Vec<(NamePattern, &GlyphRef)> = Vec::new();
    for r in refs {
        let pattern = NamePattern::parse_segments(&r.name).map_err(|e| e.to_string())?;
        parsed_refs.push((pattern, r));
    }

    // The glyph-name pattern determines how many glyphs are declared. Each
    // ref pattern is consumed cyclically in lock-step with those names.
    let n = name_pattern.len();

    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let expanded_name = parse_glyph_name(&name_pattern.get(i));

        let expanded_refs: Vec<GlyphRef> = parsed_refs
            .iter()
            .map(|(pattern, r)| GlyphRef {
                comment: None,
                name: pattern.get(i),
                ..(*r).clone()
            })
            .collect();

        if expanded_refs.is_empty() {
            continue;
        }

        items.push(DocumentItem::Glyph {
            name: expanded_name,
            body: GlyphBody {
                refs: expanded_refs,
                scale,
                ..GlyphBody::new()
            },
        });
    }

    Ok(items)
}

// ---------------------------------------------------------------------------
// DocLine — the new ground truth for the editor
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum DocLine {
    Text(String),
    Grid(PixelGrid),
}

#[cfg(feature = "editor")]
impl DocLine {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            DocLine::Text(s) => Some(s),
            DocLine::Grid(_) => None,
        }
    }

    pub fn as_text_mut(&mut self) -> Option<&mut String> {
        match self {
            DocLine::Text(s) => Some(s),
            DocLine::Grid(_) => None,
        }
    }

    #[cfg(test)]
    pub fn as_grid(&self) -> Option<&PixelGrid> {
        match self {
            DocLine::Grid(g) => Some(g),
            DocLine::Text(_) => None,
        }
    }

    pub fn char_len(&self) -> usize {
        match self {
            DocLine::Text(s) => s.chars().count(),
            DocLine::Grid(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_ref(name: &str) -> GlyphRef {
        GlyphRef {
            comment: None,
            name: name.to_string(),
            offset: None,
            negated: false,
            inherit: false,
            fill: None,
            visibility: None,
        }
    }

    #[test]
    fn classify_directive_recognizes_exactly_the_untyped_directives() {
        use super::{Directive, classify_directive};
        assert_eq!(
            classify_directive("exclude-from-sample a b"),
            Directive::ExcludeFromSample("a b"),
        );
        assert_eq!(
            classify_directive("  assume unused foo  "),
            Directive::AssumeUnused("foo"),
        );
        assert_eq!(classify_directive("   "), Directive::Empty);
        // No arguments means no match: `assume unused` alone says nothing.
        assert_eq!(classify_directive("assume unused"), Directive::Unrecognized);
        assert_eq!(classify_directive("assume something"), Directive::Unrecognized);
        // Malformed forms of directives that normally parse into typed items
        // must still be reported rather than silently accepted.
        assert_eq!(classify_directive("assert bogus"), Directive::Unrecognized);
        assert_eq!(classify_directive("whatever"), Directive::Unrecognized);
    }

    #[test]
    fn collect_name_parts_decodes_empty_alternative() {
        let mut doc = Document::new("test.unf".into());
        doc.items.push(DocumentItem::NameParts {
            comment: None,
            name: "$part".to_string(),
            values: vec!["``|a".to_string()],
        });

        let parts = collect_name_parts(&[&doc]);
        assert_eq!(
            parts.get("$part"),
            Some(&vec![String::new(), "a".to_string()]),
        );
    }

    #[test]
    fn collect_name_parts_preserves_repeat_that_exceeds_cumulative_limit() {
        let mut doc = Document::new("test.unf".into());
        let oversized = format!("b*{}", MAX_EXPANSION);
        doc.items.push(DocumentItem::NameParts {
            comment: None,
            name: "$part".to_string(),
            values: vec!["a".to_string(), oversized.clone()],
        });

        let parts = collect_name_parts(&[&doc]);
        assert_eq!(
            parts.get("$part"),
            Some(&vec!["a".to_string(), oversized]),
        );
    }

    #[test]
    fn expand_glyph_block_rejects_zero_repeat_without_panicking() {
        let result = expand_glyph_block(
            &GlyphName("glyph*0".to_string()),
            &[pattern_ref("base")],
            1,
        );

        assert!(result.is_err());
    }

    #[test]
    fn expand_glyph_block_rejects_overflowing_codepoint_range() {
        let result = expand_glyph_block(
            &GlyphName("U+00000000..FFFFFFFF".to_string()),
            &[pattern_ref("base")],
            1,
        );

        assert!(result.is_err());
    }

    #[test]
    fn expand_glyph_block_expands_lowercase_codepoint_range() {
        let items = expand_glyph_block(
            &GlyphName("u+2800..2801".to_string()),
            &[pattern_ref("base")],
            1,
        )
        .unwrap();
        let names: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, .. } => name.display(),
                _ => unreachable!(),
            })
            .collect();

        assert_eq!(names, vec!["U+2800".to_string(), "U+2801".to_string()]);
    }

    #[test]
    fn glyph_name_count_drives_ref_pattern_expansion() {
        let items = expand_glyph_block(
            &GlyphName("out-(a|b)".to_string()),
            &[pattern_ref("dep-(1|2|3|4)")],
            1,
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        let expanded: Vec<(String, String)> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, body } => {
                    (name.display(), body.refs[0].name.clone())
                }
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            expanded,
            vec![
                ("out-a".to_string(), "dep-1".to_string()),
                ("out-b".to_string(), "dep-2".to_string()),
            ],
        );
    }

    #[test]
    fn glyph_block_group_mult() {
        let items = expand_glyph_block(
            &GlyphName("out-(a|b**3)".to_string()),
            &[pattern_ref("base")],
            1,
        )
        .unwrap();

        let names: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, .. } => name.display(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "out-a", "out-a", "out-a",
                "out-b", "out-b", "out-b",
            ],
        );
    }

    #[test]
    fn glyph_block_group_mult_with_individual_repeats() {
        let items = expand_glyph_block(
            &GlyphName("out-(a*2|b**3)".to_string()),
            &[pattern_ref("base")],
            1,
        )
        .unwrap();

        let names: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, .. } => name.display(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "out-a", "out-a", "out-a", "out-a", "out-a", "out-a",
                "out-b", "out-b", "out-b",
            ],
        );
    }

    #[test]
    fn glyph_block_uses_lcm_for_independent_alternation_groups() {
        let items = expand_glyph_block(
            &GlyphName("out-(a|b)-(1|2|3)".to_string()),
            &[pattern_ref("base")],
            1,
        )
        .unwrap();

        let names: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, .. } => name.display(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "out-a-1".to_string(),
                "out-b-2".to_string(),
                "out-a-3".to_string(),
                "out-b-1".to_string(),
                "out-a-2".to_string(),
                "out-b-3".to_string(),
            ],
        );
    }

    #[test]
    fn compute_docline_file_lines_skips_omitted_empty_grids() {
        use crate::document_io::serialize_doclines;
        use crate::pixel::{PX_ALMOSTFULL, PX_FULL, PixelShape};

        // "glyph a" has declared dims but an all-empty grid, which the
        // serializer omits entirely; lines after it must still map to their
        // real (post-omission) line numbers in the serialized file.
        let mut filled = PixelGrid::new(1, 1);
        filled.set(0, 0, PixelShape(PX_ALMOSTFULL | PX_FULL));

        let lines = vec![
            DocLine::Text("glyph a 2 2".to_string()),
            DocLine::Grid(PixelGrid::new(2, 2)),
            DocLine::Text("glyph b 1 1".to_string()),
            DocLine::Grid(filled),
            DocLine::Text("map A = b".to_string()),
        ];

        let file_lines = compute_docline_file_lines(&lines);
        assert_eq!(file_lines, vec![0, 1, 1, 2, 3]);

        // Cross-check against the actual serialized output.
        let mut buf = Vec::new();
        serialize_doclines(&lines, &mut buf).unwrap();
        let serialized = String::from_utf8(buf).unwrap();
        let serialized_lines: Vec<&str> = serialized.lines().collect();
        assert_eq!(serialized_lines.len(), 4);
        assert_eq!(serialized_lines[file_lines[0]], "glyph a 2 2");
        assert_eq!(serialized_lines[file_lines[2]], "glyph b 1 1");
        assert_eq!(serialized_lines[file_lines[4]], "map A = b");
    }

    #[test]
    fn rescale_up() {
        // 2×2 grid at scale 1, rescale to scale 2 → 4×4
        let mut g = PixelGrid::new(2, 2);
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        g.set(0, 0, full); // top-left filled
        g.set(1, 1, full); // bottom-right filled

        let r = g.rescale(1, 2);
        assert_eq!((r.width, r.height), (4, 4));
        // Each source pixel becomes a 2×2 block
        assert_eq!(r.get(0, 0), full);
        assert_eq!(r.get(0, 1), full);
        assert_eq!(r.get(1, 0), full);
        assert_eq!(r.get(1, 1), full);
        assert_eq!(r.get(0, 2), PixelShape::EMPTY);
        assert_eq!(r.get(2, 0), PixelShape::EMPTY);
        assert_eq!(r.get(2, 2), full);
        assert_eq!(r.get(2, 3), full);
        assert_eq!(r.get(3, 2), full);
        assert_eq!(r.get(3, 3), full);
    }

    #[test]
    fn rescale_down() {
        // 4×4 grid at scale 2, rescale to scale 1 → 2×2
        let mut g = PixelGrid::new(4, 4);
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        for r in 0..2 { for c in 0..2 { g.set(r, c, full); } }
        for r in 2..4 { for c in 2..4 { g.set(r, c, full); } }

        let r = g.rescale(2, 1);
        assert_eq!((r.width, r.height), (2, 2));
        assert_eq!(r.get(0, 0), full);
        assert_eq!(r.get(0, 1), PixelShape::EMPTY);
        assert_eq!(r.get(1, 0), PixelShape::EMPTY);
        assert_eq!(r.get(1, 1), full);
    }

    #[test]
    fn rescale_fractional_ratio() {
        // 6×3 grid at scale 3, rescale to scale 2 → 4×2
        let mut g = PixelGrid::new(6, 3);
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        for r in 0..3 { for c in 0..3 { g.set(r, c, full); } }

        let r = g.rescale(3, 2);
        assert_eq!((r.width, r.height), (4, 2));
        assert_eq!(r.get(0, 0), full);
        assert_eq!(r.get(0, 1), full);
        assert_eq!(r.get(0, 2), PixelShape::EMPTY);
        assert_eq!(r.get(1, 0), full);
    }

    #[test]
    fn rescale_up_subpixel_shape_exact() {
        // A HALF1 diagonal upscaled 3× must stay one straight diagonal:
        // cells on the diagonal become HALF1, cells below it full, cells
        // above it empty — all plain codes, no details. (The former
        // nearest-neighbor rescale duplicated the diagonal into every
        // cell, visibly snapping mixed-scale composites to one grid.)
        let mut g = PixelGrid::new(1, 1);
        g.set(0, 0, PixelShape::new(crate::pixel::PX_HALF1, true));
        let r = g.rescale(1, 3);
        assert_eq!((r.width, r.height), (3, 3));
        assert!(r.details.is_empty());
        for row in 0..3u16 {
            for col in 0..3u16 {
                let expected = if row == col {
                    crate::pixel::PX_HALF1
                } else if row > col {
                    PX_ALMOSTFULL
                } else {
                    crate::pixel::PX_EMPTY
                };
                assert_eq!(
                    r.get(row, col).shape_id(),
                    expected,
                    "cell ({row}, {col})"
                );
            }
        }
    }

    #[test]
    fn rescale_fractional_creates_exact_details() {
        // A logical pixel two-thirds covered (2 of 3 columns full at scale
        // 3) rescaled to scale 2: the filled region is 4/3 destination
        // pixels wide. The right column's sliver is not representable as a
        // plain code and must become an exact custom detail, and the
        // contour tracer must produce a single clean rectangle outline.
        let mut g = PixelGrid::new(3, 3);
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        for r in 0..3 { for c in 0..2 { g.set(r, c, full); } }

        let out = g.rescale(3, 2);
        assert_eq!((out.width, out.height), (2, 2));
        assert_eq!(out.get(0, 0).shape_id(), PX_ALMOSTFULL);
        assert_eq!(out.get(1, 0).shape_id(), PX_ALMOSTFULL);
        assert_eq!(out.get(0, 1).shape_id(), PX_CUSTOM);
        assert_eq!(out.get(1, 1).shape_id(), PX_CUSTOM);
        let d = out.details.get(&(0, 1)).unwrap();
        assert_eq!(d.den, 3);
        assert_eq!(d.area2(), 2.0 / 3.0);

        let paths =
            crate::render::contour::track_contour(&out, crate::pixel::PX_SUBPIXEL);
        assert_eq!(paths.len(), 1, "one rectangle outline, got {paths:?}");
        let mut pts = paths[0].clone();
        pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let expected = [(0.0f32, 0.0f32), (0.0, 2.0), (4.0 / 3.0, 0.0), (4.0 / 3.0, 2.0)];
        assert_eq!(pts.len(), 4, "rectangle has 4 corners: {pts:?}");
        for (p, e) in pts.iter().zip(expected.iter()) {
            assert!((p.0 - e.0).abs() < 1e-5 && (p.1 - e.1).abs() < 1e-5,
                "vertex {p:?} != {e:?} in {pts:?}");
        }
    }

    #[test]
    fn blit_negated_subtracts_exactly() {
        // Subtracting a third-of-a-pixel bar from a full pixel leaves an
        // exact custom remainder instead of a raster-snapped catalog shape.
        let mut dst = PixelGrid::new(1, 1);
        dst.set(0, 0, PixelShape::new(PX_ALMOSTFULL, true));

        let mut src = PixelGrid::new(1, 1);
        let bar = crate::detail::DetailRegion {
            den: 3,
            rings: vec![vec![(0, 0), (1, 0), (1, 3), (0, 3)]],
        };
        src.set_detail(0, 0, &bar, true);

        dst.blit(&src, 0, 0, true);
        assert_eq!(dst.get(0, 0).shape_id(), PX_CUSTOM);
        let d = dst.details.get(&(0, 0)).unwrap();
        assert_eq!(d.area2(), 2.0 * 2.0 / 3.0);
    }
}
