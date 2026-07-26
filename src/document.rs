use std::collections::{BTreeMap, HashMap};
use std::fmt;
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

    pub fn mirror_h(&self) -> Self {
        let mut out = Self::new(self.width, self.height);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                out.set(r, self.width - 1 - c, self.get(r, c).mirror_h());
            }
        }
        for (&(r, c), d) in &self.details {
            out.details.insert((r, self.width - 1 - c), d.mirror_h());
        }
        out
    }

    pub fn flip_v(&self) -> Self {
        let mut out = Self::new(self.width, self.height);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                out.set(self.height - 1 - r, c, self.get(r, c).flip_v());
            }
        }
        for (&(r, c), d) in &self.details {
            out.details.insert((self.height - 1 - r, c), d.flip_v());
        }
        out
    }

    pub fn rotate_cw(&self) -> Self {
        let mut out = Self::new(self.height, self.width);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                out.set(c, self.height - 1 - r, self.get(r, c).rotate_cw());
            }
        }
        for (&(r, c), d) in &self.details {
            out.details.insert((c, self.height - 1 - r), d.rotate_cw());
        }
        out
    }

    pub fn rotate_ccw(&self) -> Self {
        let mut out = Self::new(self.height, self.width);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                out.set(self.width - 1 - c, r, self.get(r, c).rotate_ccw());
            }
        }
        for (&(r, c), d) in &self.details {
            out.details.insert((self.width - 1 - c, r), d.rotate_ccw());
        }
        out
    }

    pub fn rotate_180(&self) -> Self {
        let mut out = Self::new(self.width, self.height);
        out.den = self.den;
        for r in 0..self.height {
            for c in 0..self.width {
                out.set(self.height - 1 - r, self.width - 1 - c, self.get(r, c).rotate_180());
            }
        }
        for (&(r, c), d) in &self.details {
            out.details.insert((self.height - 1 - r, self.width - 1 - c), d.rotate_180());
        }
        out
    }

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
    pub fill: Option<RefFill>,
    pub visibility: Option<LayerVisibility>,
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
        if let Some(ref fill) = self.fill {
            parts.push(format!("fill {}", quote_token(&fill.color)));
        }
        match self.visibility {
            Some(LayerVisibility::ColorOnly) => parts.push("coloronly".into()),
            Some(LayerVisibility::MonoOnly) => parts.push("monoonly".into()),
            Some(LayerVisibility::Both) | None => {}
        }
        parts.join(" ")
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
}

impl GlyphPoint {
    pub fn width(&self) -> u16 {
        (self.col_end - self.col + 1) as u16
    }

    pub fn height(&self) -> u16 {
        (self.row_end - self.row + 1) as u16
    }

    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    pub fn is_single_cell(&self) -> bool {
        self.col == self.col_end && self.row == self.row_end
    }

    pub fn size_matches(&self, other: &GlyphPoint) -> bool {
        self.width() == other.width() && self.height() == other.height()
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
        }
    }

    /// True if this body is a simple alias (`glyph NAME = ALIAS`): no pixel
    /// data, exactly one ref, with no positional offset.
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
    },
    /// `map CHAR` — auto-decomposed cmap mapping. The glyph is synthesized from
    /// the character's Unicode canonical decomposition.
    MapDecomposed {
        char_repr: String,
    },
    /// `name-parts $NAME = token1 token2 $ref3 ...`
    NameParts {
        name: String,
        values: Vec<String>,
    },
    /// `remap FEATURE : [LOOKBEHIND... :] SOURCE -> TARGET [: LOOKAHEAD...]`
    Remap {
        feature: String,
        lookbehind: Vec<String>,
        source: Vec<String>,
        target: Vec<String>,
        lookahead: Vec<String>,
    },
    /// `feature NAME for SCRIPT... : REMAP_GROUP`
    Feature {
        name: String,
        scripts: Vec<String>,
        remap_group: String,
    },
    /// `feature NAME for SCRIPT... : anchor ANCHOR_NAME`
    FeatureAnchor {
        name: String,
        scripts: Vec<String>,
        anchor: String,
    },
    /// `color NAME = #xxxxxx[xx]|COLORNAME [coloronly|monoonly]`
    Color {
        name: String,
        value: String,
        visibility: Option<LayerVisibility>,
    },
    /// `assert shape \`text\` [+feat] [-feat] : glyph1 [advance N] [offset X Y] : glyph2 ...`
    AssertShape {
        text: String,
        features: Vec<ShapeFeatureFlag>,
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

fn split_inline_comment(tokens: &[String]) -> (Vec<String>, Option<String>) {
    if let Some(pos) = tokens.iter().position(|t| t == "//") {
        let body = tokens[..pos].to_vec();
        let comment_parts: Vec<&str> = tokens[pos + 1..].iter().map(|s| s.as_str()).collect();
        let comment = if comment_parts.is_empty() {
            None
        } else {
            Some(comment_parts.join(" "))
        };
        (body, comment)
    } else {
        (tokens.to_vec(), None)
    }
}

fn serialize_comment_suffix(comment: &Option<String>) -> String {
    match comment {
        Some(c) => format!(" // {c}"),
        None => String::new(),
    }
}

impl DocumentItem {
    /// Parse a structured directive from pre-tokenized tokens.
    /// The first token is the keyword ("name-parts", "remap", or "feature").
    pub fn parse_directive(tokens: &[String]) -> DocumentItem {
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
                    };
                }
            }
            "assert" => {
                let (tokens, comment) = split_inline_comment(tokens);
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
                if let Some(item) = Self::parse_remap(&tokens[1..]) {
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
                                };
                            }
                            return DocumentItem::Feature {
                                name: rest[0].clone(),
                                scripts: rest[2..colon_pos].to_vec(),
                                remap_group: rest[colon_pos + 1].clone(),
                            };
                        }
            }
            _ => {}
        }
        let quoted: Vec<String> = tokens.iter().map(|t| crate::document_io::quote_token(t)).collect();
        DocumentItem::Directive(quoted.join(" "))
    }

    fn parse_remap(tokens: &[String]) -> Option<DocumentItem> {
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
        })
    }

    fn parse_assert_shape(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        if tokens.is_empty() {
            return None;
        }
        let text = tokens[0].clone();

        let first_colon = tokens.iter().position(|t| t == ":")?;

        let mut features = Vec::new();
        for tok in &tokens[1..first_colon] {
            if let Some(tag) = tok.strip_prefix('+') {
                features.push(ShapeFeatureFlag { tag: tag.to_string(), enable: true });
            } else if let Some(tag) = tok.strip_prefix('-') {
                features.push(ShapeFeatureFlag { tag: tag.to_string(), enable: false });
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

        Some(DocumentItem::AssertShape { text, features, expected, comment })
    }

    pub fn serialize_line(&self) -> Option<String> {
        use crate::document_io::quote_token;
        match self {
            DocumentItem::NameParts { name, values } => {
                let qvals: Vec<String> = values.iter().map(|v| quote_token(v)).collect();
                Some(format!("name-parts {} = {}", quote_token(name), qvals.join(" ")))
            }
            DocumentItem::Remap {
                feature,
                lookbehind,
                source,
                target,
                lookahead,
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
                Some(parts.join(" "))
            }
            DocumentItem::Feature { name, scripts, remap_group } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                Some(format!(
                    "feature {} for {} : {}",
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(remap_group),
                ))
            }
            DocumentItem::FeatureAnchor { name, scripts, anchor } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                Some(format!(
                    "feature {} for {} : anchor {}",
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(anchor),
                ))
            }
            DocumentItem::Color { name, value, visibility } => {
                let vis = match visibility {
                    Some(LayerVisibility::ColorOnly) => " coloronly",
                    Some(LayerVisibility::MonoOnly) => " monoonly",
                    _ => "",
                };
                Some(format!("color {} = {}{}", quote_token(name), quote_token(value), vis))
            }
            DocumentItem::AssertShape { text, features, expected, comment } => {
                let mut parts = vec![
                    "assert".to_string(),
                    "shape".to_string(),
                    quote_token(text),
                ];
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

// ---------------------------------------------------------------------------
// Name pattern expansion: `foo-(a|b|c)` → ["foo-a", "foo-b", "foo-c"]
// ---------------------------------------------------------------------------

pub const MAX_EXPANSION: usize = 1 << 16;

#[derive(Clone, Debug)]
pub struct ExpandedNames {
    names: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum NamePatternError {
    TooManyExpansions(usize),
    Syntax(String),
}

impl fmt::Display for NamePatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NamePatternError::TooManyExpansions(n) => {
                write!(f, "name pattern expands to {n} names (max {MAX_EXPANSION})")
            }
            NamePatternError::Syntax(msg) => write!(f, "name pattern syntax error: {msg}"),
        }
    }
}

impl std::error::Error for NamePatternError {}

impl ExpandedNames {
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(|s| s.as_str())
    }

    pub fn into_vec(self) -> Vec<String> {
        self.names
    }
}

impl<'a> IntoIterator for &'a ExpandedNames {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;
    fn into_iter(self) -> Self::IntoIter {
        self.names.iter()
    }
}

impl IntoIterator for ExpandedNames {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;
    fn into_iter(self) -> Self::IntoIter {
        self.names.into_iter()
    }
}

/// Returns true if the name string looks like a multi-glyph pattern
/// (contains alternation `|`, grouping `(`, or range `..`).
/// Single-character names are never patterns even if the character is `(` or `|`.
pub fn has_bare_repeat(s: &str) -> bool {
    if let Some((_, count_str)) = s.rsplit_once('*') {
        !count_str.is_empty() && count_str.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    }
}

pub fn is_name_pattern(s: &str) -> bool {
    s.chars().count() > 1
        && (s.contains('(') || s.contains('|') || s.contains("..") || has_bare_repeat(s))
}

pub fn expand_name_pattern(s: &str) -> Result<ExpandedNames, NamePatternError> {
    if let Some(hex_rest) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..") {
            let start = u32::from_str_radix(start_hex, 16).map_err(|_| {
                NamePatternError::Syntax(format!("bad range start: {start_hex}"))
            })?;
            let end = u32::from_str_radix(end_hex, 16).map_err(|_| {
                NamePatternError::Syntax(format!("bad range end: {end_hex}"))
            })?;
            if end < start {
                return Err(NamePatternError::Syntax("range end < start".into()));
            }
            let count = u64::from(end) - u64::from(start) + 1;
            if count > MAX_EXPANSION as u64 {
                return Err(NamePatternError::TooManyExpansions(
                    usize::try_from(count).unwrap_or(usize::MAX),
                ));
            }
            let names = (start..=end).map(|cp| format!("U+{cp:04X}")).collect();
            return Ok(ExpandedNames { names });
        }

    if s.chars().count() <= 1 || (!s.contains('(') && !s.contains('|') && !s.contains('*')) {
        return Ok(ExpandedNames {
            names: vec![s.to_string()],
        });
    }

    let normalized = if !s.contains('(') {
        format!("({s})")
    } else {
        s.to_string()
    };

    let normalized_replaced = normalized.replace('(', ")");
    let raw_parts: Vec<&str> = normalized_replaced.split(')').collect();

    if raw_parts.len().is_multiple_of(2) {
        return Err(NamePatternError::Syntax("unmatched parentheses".into()));
    }

    enum Part {
        Fixed(String),
        Alternation(Vec<String>),
    }

    let mut parts: Vec<Part> = Vec::new();
    for (i, part) in raw_parts.iter().enumerate() {
        if i % 2 == 0 {
            parts.push(Part::Fixed(part.to_string()));
        } else {
            let (part, group_mult) = extract_group_mult(part).map_err(|e| {
                NamePatternError::Syntax(e)
            })?;
            let mut alts = Vec::new();
            for alt in part.split('|') {
                if let Some((name, rep_str)) = alt.rsplit_once('*') {
                    let rep: usize = rep_str.parse().map_err(|_| {
                        NamePatternError::Syntax(format!("invalid repeat count: {rep_str}"))
                    })?;
                    let expanded_count = alts
                        .len()
                        .checked_add(rep)
                        .ok_or(NamePatternError::TooManyExpansions(usize::MAX))?;
                    if expanded_count > MAX_EXPANSION {
                        return Err(NamePatternError::TooManyExpansions(expanded_count));
                    }
                    for _ in 0..rep {
                        alts.push(name.to_string());
                    }
                } else {
                    alts.push(alt.to_string());
                }
            }
            if alts.is_empty() {
                return Err(NamePatternError::Syntax("empty alternation group".into()));
            }
            if group_mult > 1 {
                let base = alts;
                let total = base
                    .len()
                    .checked_mul(group_mult)
                    .ok_or(NamePatternError::TooManyExpansions(usize::MAX))?;
                if total > MAX_EXPANSION {
                    return Err(NamePatternError::TooManyExpansions(total));
                }
                alts = Vec::with_capacity(total);
                for name in &base {
                    for _ in 0..group_mult {
                        alts.push(name.clone());
                    }
                }
            }
            parts.push(Part::Alternation(alts));
        }
    }

    let mut count: usize = 1;
    for part in &parts {
        if let Part::Alternation(alts) = part {
            count = lcm(count, alts.len());
            if count > MAX_EXPANSION {
                return Err(NamePatternError::TooManyExpansions(count));
            }
        }
    }

    let mut names = Vec::with_capacity(count);
    for k in 0..count {
        let mut name = String::new();
        for part in &parts {
            match part {
                Part::Fixed(s) => name.push_str(s),
                Part::Alternation(alts) => {
                    name.push_str(&alts[k % alts.len()]);
                }
            }
        }
        names.push(name);
    }

    Ok(ExpandedNames { names })
}

// ---------------------------------------------------------------------------
// Name-parts collection and $var substitution
// ---------------------------------------------------------------------------

pub type NamePartsMap = HashMap<String, Vec<String>>;

pub fn collect_name_parts(docs: &[&Document]) -> NamePartsMap {
    let mut map = NamePartsMap::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::NameParts { name, values } = item {
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

/// Try to parse an inline numeric range at `chars[start]` (which must be `$`).
///
/// Syntax: `$DIGITS..DIGITS` (decimal) or `$#HEX..HEX` (hexadecimal, lowercase).
/// Returns `Some((end_pos, expanded))` where `expanded` is `v1|v2|...|vn`.
/// The minimum output width is determined by the number of digits in the start
/// number, so `$00..09` produces `00|01|...|09`.
/// Returns `None` if the text doesn't match the range syntax at all.
/// Returns an empty expansion string if end < start (caller should leave as-is
/// or flag an error).
fn try_expand_inline_range(chars: &[char], start: usize) -> Option<(usize, String)> {
    let mut i = start + 1; // skip '$'

    let is_hex = i < chars.len() && chars[i] == '#';
    if is_hex {
        i += 1;
    }

    let digit_pred: fn(char) -> bool = if is_hex {
        |c: char| c.is_ascii_hexdigit()
    } else {
        |c: char| c.is_ascii_digit()
    };

    let num_start = i;
    while i < chars.len() && digit_pred(chars[i]) {
        i += 1;
    }
    if i == num_start {
        return None;
    }
    let start_str: String = chars[num_start..i].iter().collect();

    if i + 1 >= chars.len() || chars[i] != '.' || chars[i + 1] != '.' {
        return None;
    }
    i += 2;

    let end_start = i;
    while i < chars.len() && digit_pred(chars[i]) {
        i += 1;
    }
    if i == end_start {
        return None;
    }
    let end_str: String = chars[end_start..i].iter().collect();

    let min_width = start_str.len();
    let radix = if is_hex { 16 } else { 10 };

    let start_val = u64::from_str_radix(&start_str, radix).ok()?;
    let end_val = u64::from_str_radix(&end_str, radix).ok()?;
    if end_val < start_val {
        return Some((i, String::new()));
    }
    let count = end_val - start_val + 1;
    if count > MAX_EXPANSION as u64 {
        return Some((i, String::new()));
    }

    let parts: Vec<String> = if is_hex {
        (start_val..=end_val)
            .map(|v| format!("{v:0>width$x}", width = min_width))
            .collect()
    } else {
        (start_val..=end_val)
            .map(|v| format!("{v:0>width$}", width = min_width))
            .collect()
    };
    Some((i, parts.join("|")))
}

/// Replace `$var` tokens inside `(...)` groups with `val1|val2|...` from name-parts.
/// E.g. `hangul-init-($init)-l-f` with `$init = [g, gg, n]`
/// becomes `hangul-init-(g|gg|n)-l-f`.
///
/// Also expands inline numeric ranges: `($0..9)` → `(0|1|...|9)`,
/// `($#a0..af)` → `(a0|a1|...|af)`.
pub fn substitute_name_parts(s: &str, parts: &NamePartsMap) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            if let Some((end_pos, expanded)) = try_expand_inline_range(&chars, i) {
                if expanded.is_empty() {
                    let orig: String = chars[i..end_pos].iter().collect();
                    result.push_str(&orig);
                } else {
                    result.push_str(&expanded);
                }
                i = end_pos;
                continue;
            }
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            let var: String = chars[start..i].iter().collect();
            // Check for a `**N` group-multiplier suffix.
            let suffix_start = i;
            let mut group_mult = String::new();
            if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
                let star = i;
                i += 2;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i > star + 2 {
                    group_mult = chars[star..i].iter().collect();
                } else {
                    i = suffix_start;
                }
            }
            if let Some(values) = parts.get(&var) {
                result.push_str(&values.join("|"));
                result.push_str(&group_mult);
            } else {
                result.push_str(&var);
                result.push_str(&group_mult);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Check a string for invalid inline numeric ranges (`$end..start` where
/// end < start, or ranges exceeding `MAX_EXPANSION`). Returns descriptions
/// of each invalid range found.
pub fn find_invalid_inline_ranges(s: &str) -> Vec<String> {
    if !s.contains('$') {
        return Vec::new();
    }
    let mut errors = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            if let Some((end_pos, expanded)) = try_expand_inline_range(&chars, i) {
                if expanded.is_empty() {
                    let orig: String = chars[i..end_pos].iter().collect();
                    errors.push(orig);
                }
                i = end_pos;
                continue;
            }
        }
        i += 1;
    }
    errors
}

// ---------------------------------------------------------------------------
// Glyph-block pattern expansion: the richer engine used when *building* a
// font from a whole `glyph NAME\nref ...\nref ...` item (as opposed to
// `expand_name_pattern` above, which only expands a single name string).
//
// This additionally supports `U+XXXX..YYYY` codepoint ranges and top-level
// `name1|name2|...` lists (no enclosing parens) on the glyph name, and lets
// each `ref` line's target name carry its own `(a|b|c)` alternation that is
// expanded in lock-step with the name pattern.
//
// Multiple alternation groups use the same LCM/cyclic-repeat semantics as
// `expand_name_pattern`; this richer engine additionally handles ranges,
// top-level lists, and ref patterns.
enum Segment {
    Literal(String),
    Alts(Vec<String>),
}

fn extract_group_mult(content: &str) -> Result<(&str, usize), String> {
    let last_pipe = content.rfind('|').map_or(0, |p| p + 1);
    let last_alt = &content[last_pipe..];
    if let Some(pos) = last_alt.rfind("**") {
        let after = &last_alt[pos + 2..];
        if !after.is_empty() && after.bytes().all(|b| b.is_ascii_digit()) {
            let k: usize = after
                .parse()
                .map_err(|_| format!("invalid group multiplier: {after}"))?;
            return Ok((&content[..last_pipe + pos], k));
        }
    }
    Ok((content, 1))
}

fn parse_alt_content(content: &str) -> Result<Vec<String>, String> {
    let (content, group_mult) = extract_group_mult(content)?;

    let mut alts = Vec::new();
    for part in content.split('|') {
        if let Some((name, count_str)) = part.rsplit_once('*') {
            let n: usize = count_str
                .parse()
                .map_err(|_| format!("invalid repeat count: {count_str}"))?;
            if n > MAX_EXPANSION.saturating_sub(alts.len()) {
                return Err("alternation too large".into());
            }
            for _ in 0..n {
                alts.push(name.to_string());
            }
        } else {
            alts.push(part.to_string());
        }
    }
    if alts.is_empty() {
        return Err("alternation must contain at least one value".into());
    }

    if group_mult > 1 {
        let base = alts;
        let total = base
            .len()
            .checked_mul(group_mult)
            .ok_or_else(|| "group multiplier too large".to_string())?;
        if total > MAX_EXPANSION {
            return Err(format!("alternation too large after group multiply: {total}"));
        }
        alts = Vec::with_capacity(total);
        for name in &base {
            for _ in 0..group_mult {
                alts.push(name.clone());
            }
        }
    }

    Ok(alts)
}

/// Parse `(a|b|c)` groups in a string. Returns (expansion_count, segments).
/// Group counts combine by least common multiple (cyclic repeat).
fn parse_line_segments(s: &str) -> Result<(usize, Vec<Segment>), String> {
    let mut segments = Vec::new();
    let bytes = s.as_bytes();
    let mut pos = 0;
    let mut lit_start = 0;
    let mut group_counts: Vec<usize> = Vec::new();

    while pos < bytes.len() {
        if bytes[pos] == b'(' {
            if pos > lit_start {
                segments.push(Segment::Literal(s[lit_start..pos].to_string()));
            }
            let open = pos;
            let mut depth = 1;
            pos += 1;
            while pos < bytes.len() && depth > 0 {
                if bytes[pos] == b'(' {
                    depth += 1;
                }
                if bytes[pos] == b')' {
                    depth -= 1;
                }
                pos += 1;
            }
            if depth != 0 {
                return Err(format!("unmatched '(' in: {s}"));
            }
            let content = &s[open + 1..pos - 1];
            let alts = parse_alt_content(content)?;
            let n = alts.len();
            if n > 1 {
                group_counts.push(n);
            }
            segments.push(Segment::Alts(alts));
            lit_start = pos;
        } else {
            pos += 1;
        }
    }
    if lit_start < s.len() {
        segments.push(Segment::Literal(s[lit_start..].to_string()));
    }

    // Bare `foo*N` (no parentheses) → treat as `(foo*N)`.
    if group_counts.is_empty() && segments.len() == 1
        && let Segment::Literal(ref lit) = segments[0]
            && has_bare_repeat(lit) {
                let alts = parse_alt_content(lit)?;
                let n = alts.len();
                segments = vec![Segment::Alts(alts)];
                if n > 1 {
                    group_counts.push(n);
                }
            }

    let mut count = 1usize;
    for group_count in group_counts {
        count = (count / gcd(count, group_count))
            .checked_mul(group_count)
            .ok_or_else(|| "expansion too large".to_string())?;
        if count > MAX_EXPANSION {
            return Err(format!("expansion too large: {count}"));
        }
    }
    Ok((count, segments))
}

fn expand_segments_at(segments: &[Segment], i: usize) -> String {
    let mut out = String::new();
    for seg in segments {
        match seg {
            Segment::Literal(s) => out.push_str(s),
            Segment::Alts(alts) => {
                if alts.len() == 1 {
                    out.push_str(&alts[0]);
                } else {
                    out.push_str(&alts[i % alts.len()]);
                }
            }
        }
    }
    out
}

pub fn has_top_level_pipe(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

pub fn parse_glyph_name(s: &str) -> GlyphName {
    GlyphName(s.trim().to_string())
}

/// Parse the glyph name pattern. Returns (parsed segments/names, count).
/// Handles U+XXXX..YYYY ranges, top-level pipes, and (a|b|c) groups.
fn parse_name_pattern(s: &str) -> Result<(NamePattern, usize), String> {
    // U+XXXX..YYYY codepoint range
    if let Some(hex_rest) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..") {
            let start = u32::from_str_radix(start_hex, 16)
                .map_err(|_| format!("bad range start: {start_hex}"))?;
            let end = u32::from_str_radix(end_hex, 16)
                .map_err(|_| format!("bad range end: {end_hex}"))?;
            if end < start {
                return Err("range end < start".into());
            }
            let count = u64::from(end) - u64::from(start) + 1;
            if count > MAX_EXPANSION as u64 {
                return Err(format!("codepoint range too large: {count}"));
            }
            let count = count as usize;
            return Ok((NamePattern::Range(start), count));
        }

    // Top-level pipe: name1|name2|...
    if has_top_level_pipe(s) {
        let names: Vec<String> = s.split('|').map(|p| p.trim().to_string()).collect();
        let count = names.len();
        if count > MAX_EXPANSION {
            return Err(format!("pipe list too large: {count}"));
        }
        return Ok((NamePattern::List(names), count));
    }

    // (a|b|c) alternation
    let (count, segments) = parse_line_segments(s)?;
    Ok((NamePattern::Segments(segments), count))
}

enum NamePattern {
    Range(u32),
    List(Vec<String>),
    Segments(Vec<Segment>),
}

fn expand_name_at(pattern: &NamePattern, count: usize, i: usize) -> GlyphName {
    match pattern {
        NamePattern::Range(start) => {
            let cp = start + (i % count) as u32;
            GlyphName(format!("U+{cp:04X}"))
        }
        NamePattern::List(names) => GlyphName(names[i % names.len()].clone()),
        NamePattern::Segments(segs) => parse_glyph_name(&expand_segments_at(segs, i)),
    }
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
    let name_str = name.display();
    let (name_pattern, name_count) = parse_name_pattern(&name_str)?;

    let mut parsed_refs: Vec<(Vec<Segment>, Option<(i16, i16)>, bool, Option<RefFill>, Option<LayerVisibility>)> = Vec::new();
    for r in refs {
        let (_, segs) = parse_line_segments(&r.name)?;
        parsed_refs.push((segs, r.offset, r.negated, r.fill.clone(), r.visibility));
    }

    // The glyph-name pattern determines how many glyphs are declared. Each
    // ref pattern is consumed cyclically in lock-step with those names.
    let n = name_count;
    if n > MAX_EXPANSION {
        return Err(format!("expansion too large: {n}"));
    }

    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let expanded_name = expand_name_at(&name_pattern, name_count, i);

        let expanded_refs: Vec<GlyphRef> = parsed_refs
            .iter()
            .map(|(segs, offset, negated, fill, visibility)| GlyphRef {
                name: expand_segments_at(segs, i),
                offset: *offset,
                negated: *negated,
                fill: fill.clone(),
                visibility: *visibility,
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

#[cfg(any(feature = "editor", test))]
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

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b) * b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_ref(name: &str) -> GlyphRef {
        GlyphRef {
            name: name.to_string(),
            offset: None,
            negated: false,
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
    fn expand_name_pattern_expands_codepoint_ranges() {
        assert_eq!(
            expand_name_pattern("U+2800..2802").unwrap().into_vec(),
            vec![
                "U+2800".to_string(),
                "U+2801".to_string(),
                "U+2802".to_string(),
            ],
        );
        assert_eq!(
            expand_name_pattern("u+00fe..0100").unwrap().into_vec(),
            vec![
                "U+00FE".to_string(),
                "U+00FF".to_string(),
                "U+0100".to_string(),
            ],
        );
    }

    #[test]
    fn expand_name_pattern_rejects_invalid_or_oversized_ranges() {
        assert!(matches!(
            expand_name_pattern("U+2802..2800"),
            Err(NamePatternError::Syntax(_)),
        ));
        assert!(matches!(
            expand_name_pattern("U+00000000..FFFFFFFF"),
            Err(NamePatternError::TooManyExpansions(_)),
        ));
    }

    #[test]
    fn expand_name_pattern_rejects_oversized_repeat_before_materializing_it() {
        let pattern = format!("(name*{})", MAX_EXPANSION + 1);
        assert!(matches!(
            expand_name_pattern(&pattern),
            Err(NamePatternError::TooManyExpansions(_)),
        ));
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
    fn inline_range_decimal() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($0..9)", &parts),
            "(0|1|2|3|4|5|6|7|8|9)",
        );
    }

    #[test]
    fn inline_range_decimal_zero_padded() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($00..12)", &parts),
            "(00|01|02|03|04|05|06|07|08|09|10|11|12)",
        );
    }

    #[test]
    fn inline_range_decimal_mixed_width() {
        let parts = NamePartsMap::new();
        let result = substitute_name_parts("($0..11)", &parts);
        assert!(result.starts_with("(0|1|2|"));
        assert!(result.contains("|9|10|11)"));
    }

    #[test]
    fn inline_range_hex() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($#a..f)", &parts),
            "(a|b|c|d|e|f)",
        );
    }

    #[test]
    fn inline_range_hex_zero_padded() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($#0a..0c)", &parts),
            "(0a|0b|0c)",
        );
    }

    #[test]
    fn inline_range_reversed_leaves_as_is() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($3..2)", &parts),
            "($3..2)",
        );
    }

    #[test]
    fn inline_range_in_glyph_name() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("sup-($0..9)", &parts),
            "sup-(0|1|2|3|4|5|6|7|8|9)",
        );
    }

    #[test]
    fn inline_range_find_invalid() {
        assert_eq!(
            find_invalid_inline_ranges("($3..2)"),
            vec!["$3..2"],
        );
        assert!(find_invalid_inline_ranges("($0..9)").is_empty());
    }

    #[test]
    fn substitute_name_parts_with_group_mult_suffix() {
        let mut parts = NamePartsMap::new();
        parts.insert(
            "$foo".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        assert_eq!(
            substitute_name_parts("($foo**3)", &parts),
            "(a|b|c**3)",
        );
        // Without suffix, normal expansion.
        assert_eq!(
            substitute_name_parts("($foo)", &parts),
            "(a|b|c)",
        );
        // Unknown var keeps suffix verbatim.
        assert_eq!(
            substitute_name_parts("($bar**2)", &NamePartsMap::new()),
            "($bar**2)",
        );
    }

    #[test]
    fn expand_name_pattern_group_mult() {
        assert_eq!(
            expand_name_pattern("(a|b**3)").unwrap().into_vec(),
            vec!["a", "a", "a", "b", "b", "b"],
        );
        assert_eq!(
            expand_name_pattern("(a*2|b**3)").unwrap().into_vec(),
            vec!["a", "a", "a", "a", "a", "a", "b", "b", "b"],
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
