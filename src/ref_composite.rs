use std::collections::HashMap;

use crate::document::{
    Document, DocumentItem, GlyphName, GlyphPoint, GlyphRef, NamePartsMap, PixelGrid,
    substitute_name_parts,
};
use crate::pattern::NamePattern;
// Only the composite helpers and their tests need this.
#[cfg(any(feature = "editor", test))]
use crate::document::GlyphBody;

#[cfg(feature = "editor")]
const PHI: f64 = 1.618033988749895;

#[cfg(feature = "editor")]
pub fn ref_color_sv(s: f32, v: f32, index: usize) -> egui::Color32 {
    let hue = ((index + 1) as f64 / PHI % 1.0 * 360.0) as f32;
    hsv_to_rgb(hue, s, v)
}

#[cfg(feature = "editor")]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    egui::Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub struct ResolvedGlyph {
    pub grid: PixelGrid,
    /// Logical coordinate represented by raster cell `(0, 0)`. Keeping this
    /// separate from the raster is essential for nested refs whose bounds
    /// extend left/up from the glyph origin.
    pub(crate) origin_row: i32,
    pub(crate) origin_col: i32,
    pub(crate) resolved_anchors: Vec<GlyphPoint>,
    /// The glyph body's own declared anchor/point lines (not forwarded
    /// from refs).  Used by look-ahead alternative selection.
    pub(crate) declared_anchors: Vec<GlyphPoint>,
    pub scale: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriCorner {
    /// Right angle at the upper-left corner.
    Ul,
    /// Right angle at the upper-right corner.
    Ur,
    /// Right angle at the lower ("down") left corner.
    Dl,
    /// Right angle at the lower ("down") right corner.
    Dr,
}

/// How a shape's exact coverage of a *logical* pixel is rounded into that
/// pixel's ink flag ([`crate::pixel::PX_FULL`]), which is what the bitmap
/// build of the font draws (`ttf_builder::contours::CachedContours::from_grid`). The
/// vector build reads the geometry instead, so this never moves an outline.
///
/// Whole-pixel shapes are covered 1/1 everywhere and so render identically
/// under every variant but [`BitmapFill::Zero`]; the choice only bites on
/// fractional rectangles and on the cells a triangle's hypotenuse crosses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BitmapFill {
    /// Coverage of at least half a pixel lights it, ties included. The
    /// default, and the only one that needs no name suffix.
    #[default]
    Round,
    /// Any coverage at all lights the pixel (`:ceil`).
    Ceil,
    /// Only fully covered pixels are lit (`:floor`).
    Floor,
    /// Nothing is ever lit (`:zero`): the shape exists for the vector build
    /// only and contributes no bitmap ink.
    Zero,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnDemandRect {
    pub w: u8,
    pub h: u8,
    pub w_frac: u8,
    pub h_frac: u8,
    pub scale: u8,
    pub neg_w: bool,
    pub neg_h: bool,
    /// `Some` makes this a right triangle with legs `w × h`, the right
    /// angle sitting at the given corner of the bounding rectangle
    /// (`-ul`/`-ur`/`-dl`/`-dr` name suffixes).
    pub corner: Option<TriCorner>,
    /// From the `:ceil`/`:floor`/`:zero` name suffix; [`BitmapFill::Round`]
    /// when absent.
    pub fill: BitmapFill,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnDemandGlyph {
    Rect(OnDemandRect),
    ColorMono { mono: String, color: String },
}

fn parse_uint_no_leading_zero(s: &str) -> Option<u8> {
    if s.is_empty() {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

/// Parse one dimension: `[-]A[pBrR]`.
/// Returns `(negated, base, Option<(frac, scale)>)`.
fn parse_rect_dim(s: &str) -> Option<(bool, u8, Option<(u8, u8)>)> {
    let (neg, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };
    if let Some(p_pos) = s.find('p') {
        let base_str = &s[..p_pos];
        let rest = &s[p_pos + 1..];
        let (frac_str, scale_str) = rest.split_once('r')?;
        let base = parse_uint_no_leading_zero(base_str)?;
        let frac = parse_uint_no_leading_zero(frac_str)?;
        let scale = parse_uint_no_leading_zero(scale_str)?;
        Some((neg, base, Some((frac, scale))))
    } else {
        let base = parse_uint_no_leading_zero(s)?;
        Some((neg, base, None))
    }
}

/// Parse an on-demand glyph name of the form `WxH` (e.g. `3x5`) or the
/// extended form `[-]A[pBrR]x[-]C[pDrR]` (e.g. `1p2r3x4`, `-3p1r4x-2p3r4`).
///
/// Simple `WxH`: both W and H must be non-zero.
/// Extended form: R >= 2 on both sides (must match), 0 <= B,D < R, and the
/// total dimension A+B/R must be positive on each axis.
///
/// A `-ul`/`-ur`/`-dl`/`-dr` suffix turns the rectangle into a right
/// triangle with legs W × H, the right angle at the named corner
/// (u = up, d = down).
///
/// A trailing `:ceil`/`:floor`/`:zero` picks the [`BitmapFill`] rule; it comes
/// after the corner suffix (`8x16-ul:floor`). Any other `:`-suffix makes the
/// whole name a non-match, so ordinary glyph names that happen to contain a
/// colon — and the `:mono`/`:color` pair handled by
/// [`detect_color_mono_glyph`] — fall through untouched.
pub fn parse_on_demand_glyph(name: &str) -> Option<OnDemandGlyph> {
    let (name, fill) = match name.rsplit_once(':') {
        Some((rest, "ceil")) => (rest, BitmapFill::Ceil),
        Some((rest, "floor")) => (rest, BitmapFill::Floor),
        Some((rest, "zero")) => (rest, BitmapFill::Zero),
        Some(_) => return None,
        None => (name, BitmapFill::Round),
    };
    let (name, corner) = if let Some(rest) = name.strip_suffix("-ul") {
        (rest, Some(TriCorner::Ul))
    } else if let Some(rest) = name.strip_suffix("-ur") {
        (rest, Some(TriCorner::Ur))
    } else if let Some(rest) = name.strip_suffix("-dl") {
        (rest, Some(TriCorner::Dl))
    } else if let Some(rest) = name.strip_suffix("-dr") {
        (rest, Some(TriCorner::Dr))
    } else {
        (name, None)
    };
    let (w_str, h_str) = name.split_once('x')?;
    let (neg_w, w, w_detail) = parse_rect_dim(w_str)?;
    let (neg_h, h, h_detail) = parse_rect_dim(h_str)?;

    match (w_detail, h_detail) {
        (None, None) => {
            if w == 0 || h == 0 || neg_w || neg_h {
                return None;
            }
            Some(OnDemandGlyph::Rect(OnDemandRect {
                w,
                h,
                w_frac: 0,
                h_frac: 0,
                scale: 1,
                neg_w: false,
                neg_h: false,
                corner,
                fill,
            }))
        }
        (Some((wf, ws)), Some((hf, hs))) => {
            if ws != hs || ws < 2 {
                return None;
            }
            if wf >= ws || hf >= hs {
                return None;
            }
            if w == 0 && wf == 0 {
                return None;
            }
            if h == 0 && hf == 0 {
                return None;
            }
            Some(OnDemandGlyph::Rect(OnDemandRect {
                w,
                h,
                w_frac: wf,
                h_frac: hf,
                scale: ws,
                neg_w,
                neg_h,
                corner,
                fill,
            }))
        }
        (Some((wf, ws)), None) => {
            if ws < 2 || wf >= ws {
                return None;
            }
            if w == 0 && wf == 0 {
                return None;
            }
            if h == 0 {
                return None;
            }
            Some(OnDemandGlyph::Rect(OnDemandRect {
                w,
                h,
                w_frac: wf,
                h_frac: 0,
                scale: ws,
                neg_w,
                neg_h,
                corner,
                fill,
            }))
        }
        (None, Some((hf, hs))) => {
            if hs < 2 || hf >= hs {
                return None;
            }
            if w == 0 {
                return None;
            }
            if h == 0 && hf == 0 {
                return None;
            }
            Some(OnDemandGlyph::Rect(OnDemandRect {
                w,
                h,
                w_frac: 0,
                h_frac: hf,
                scale: hs,
                neg_w,
                neg_h,
                corner,
                fill,
            }))
        }
    }
}

/// Detect an on-demand color/mono glyph: name X is not defined, but both
/// X:mono and X:color exist.  X itself must not contain `:mono` or `:color`.
pub fn detect_color_mono_glyph(name: &str, has_glyph: impl Fn(&str) -> bool) -> Option<OnDemandGlyph> {
    if name.contains(":mono") || name.contains(":color") {
        return None;
    }
    let mono = format!("{name}:mono");
    let color = format!("{name}:color");
    if has_glyph(&mono) && has_glyph(&color) {
        Some(OnDemandGlyph::ColorMono { mono, color })
    } else {
        None
    }
}

/// Stamp each logical pixel's ink flag from its exact covered area, applying
/// the flag uniformly across the pixel's `s × s` subcells.
///
/// The uniformity is what carries the decision into a parent of any scale:
/// [`PixelGrid::rescale`] ORs the ink flags of the source subcells a
/// destination subcell covers, and — because it preserves logical dimensions —
/// that OR never reaches across a logical pixel boundary. Deciding per subcell
/// instead would be undone by exactly that OR, which is itself a `Ceil`.
fn apply_bitmap_fill(grid: &mut PixelGrid, s: u16, fill: BitmapFill) {
    if s == 0 {
        return;
    }
    for lr in 0..grid.height / s {
        for lc in 0..grid.width / s {
            // Uniformly full or uniformly empty logical pixels — every pixel
            // of a whole-pixel rectangle, and the bulk of everything else —
            // need no geometry, only their shape ids.
            let mut all_full = true;
            let mut all_empty = true;
            for dr in 0..s {
                for dc in 0..s {
                    match grid.get(lr * s + dr, lc * s + dc).shape_id() {
                        crate::pixel::PX_ALMOSTFULL => all_empty = false,
                        crate::pixel::PX_EMPTY => all_full = false,
                        _ => {
                            all_full = false;
                            all_empty = false;
                        }
                    }
                }
            }
            let filled = if all_empty {
                false
            } else if all_full {
                fill != BitmapFill::Zero
            } else {
                // Exact covered area of the logical pixel, as `num/den` in
                // subcell units; a fully covered pixel is `s²`.
                let (mut num, mut den) = (0i128, 1i128);
                for dr in 0..s {
                    for dc in 0..s {
                        let (n, d) = grid.region_at(lr * s + dr, lc * s + dc).area_exact();
                        let (n, d) = (n as i128, d as i128);
                        num = num * d + n * den;
                        den *= d;
                        let g = gcd_i128(num.abs(), den);
                        if g > 1 {
                            num /= g;
                            den /= g;
                        }
                    }
                }
                let full = den * (s as i128) * (s as i128);
                match fill {
                    BitmapFill::Zero => false,
                    BitmapFill::Ceil => num > 0,
                    BitmapFill::Floor => num >= full,
                    BitmapFill::Round => 2 * num >= full,
                }
            };
            for dr in 0..s {
                for dc in 0..s {
                    grid.set_filled(lr * s + dr, lc * s + dc, filled);
                }
            }
        }
    }
}

fn gcd_i128(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Build the pixel grid of an on-demand rectangle or triangle. The grid is
/// at subpixel resolution `rect.scale`; triangles get exact per-pixel
/// geometry, re-encoded as plain shape codes wherever possible.
///
/// Geometry is laid down first and the ink flags ([`crate::pixel::PX_FULL`])
/// are decided afterwards by [`apply_bitmap_fill`], so `rect.fill` changes
/// only what the bitmap build draws, never an outline.
pub fn make_on_demand_grid(rect: &OnDemandRect) -> PixelGrid {
    let s = rect.scale.max(1) as u16;
    let rect_w = rect.w as u16 * s + rect.w_frac as u16;
    let rect_h = rect.h as u16 * s + rect.h_frac as u16;
    let extent_w = (rect_w + s - 1) / s;
    let extent_h = (rect_h + s - 1) / s;
    let grid_w = extent_w * s;
    let grid_h = extent_h * s;
    let off_c = if rect.neg_w { grid_w - rect_w } else { 0 };
    let off_r = if rect.neg_h { grid_h - rect_h } else { 0 };

    let mut grid = PixelGrid::new(grid_w, grid_h);
    let Some(corner) = rect.corner else {
        for r in off_r..(off_r + rect_h) {
            for c in off_c..(off_c + rect_w) {
                grid.set(r, c, crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true));
            }
        }
        apply_bitmap_fill(&mut grid, s, rect.fill);
        return grid;
    };

    // Triangle vertices in grid subpixel units: the right-angle corner and
    // the two leg endpoints.
    let (x0, y0) = (off_c as i64, off_r as i64);
    let (w, h) = (rect_w as i64, rect_h as i64);
    let tri: [(i64, i64); 3] = match corner {
        TriCorner::Ul => [(x0, y0), (x0 + w, y0), (x0, y0 + h)],
        TriCorner::Ur => [(x0 + w, y0), (x0 + w, y0 + h), (x0, y0)],
        TriCorner::Dl => [(x0, y0 + h), (x0, y0), (x0 + w, y0 + h)],
        TriCorner::Dr => [(x0 + w, y0 + h), (x0 + w, y0), (x0, y0 + h)],
    };
    // The hypotenuse connects tri[1] and tri[2]; tri[0] is the right angle.
    let (hx1, hy1) = tri[1];
    let (hx2, hy2) = tri[2];
    let inside_sign = {
        let c = (hx2 - hx1) * (tri[0].1 - hy1) - (hy2 - hy1) * (tri[0].0 - hx1);
        c.signum()
    };

    for r in off_r..(off_r + rect_h) {
        for c in off_c..(off_c + rect_w) {
            // Classify the pixel's corners against the hypotenuse.
            let mut inside = 0;
            let mut outside = 0;
            for (px, py) in [
                (c as i64, r as i64),
                (c as i64 + 1, r as i64),
                (c as i64, r as i64 + 1),
                (c as i64 + 1, r as i64 + 1),
            ] {
                let cr = (hx2 - hx1) * (py - hy1) - (hy2 - hy1) * (px - hx1);
                match (cr * inside_sign as i64).signum() {
                    1 => inside += 1,
                    -1 => outside += 1,
                    _ => {}
                }
            }
            if outside == 0 {
                // Fully inside the triangle (the pixel is already within
                // the leg bounding box).
                grid.set(r, c, crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true));
                continue;
            }
            if inside == 0 {
                continue;
            }
            let local: Vec<(crate::detail::Frac64, crate::detail::Frac64)> = tri
                .iter()
                .map(|&(tx, ty)| {
                    (
                        crate::detail::Frac64::new(tx - c as i64, 1),
                        crate::detail::Frac64::new(ty - r as i64, 1),
                    )
                })
                .collect();
            let piece = crate::detail::clip_polygon_to_cell(&local);
            grid.set_detail(r, c, &piece, true);
        }
    }
    apply_bitmap_fill(&mut grid, s, rect.fill);
    grid
}

fn saturating_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn raster_dimension(min: i32, max: i32) -> u16 {
    max.saturating_sub(min).clamp(0, u16::MAX as i32) as u16
}

/// Pattern expansion used by runtime ref lookup and dependency resolution.
pub(crate) fn parse_ref_pattern(name: &str) -> Option<NamePattern> {
    NamePattern::parse_element(name).ok()
}

/// A problem found while deriving a composite's offsets and anchors. Carried
/// as data rather than a formatted string so each consumer can attach the
/// glyph name and provenance it knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeriveIssue {
    /// Two surviving anchors with the same name would both be exposed. The
    /// composite exposes neither: a digraph must not pick one side's
    /// attachment point silently. Declare the anchor explicitly instead.
    DuplicateExposed { position: String },
    /// A `-` anchor found more than one size-matching `+` anchor to attach
    /// to. Nothing is attached or consumed.
    AmbiguousAttachment { position: String, ref_name: String },
    /// A `-` anchor found a same-name `+` of a different size (`(w, h)` in
    /// cells) and therefore attached to nothing. A near-miss like this almost
    /// always means the wrong `:narrow`/`:wide` variant — a `-` with no
    /// same-name `+` at all is ordinary forwarding and stays quiet.
    SizeMismatchedAttachment {
        position: String,
        ref_name: String,
        minus: (u16, u16),
        plus: (u16, u16),
    },
}

impl DeriveIssue {
    pub(crate) fn message(&self, glyph: &str) -> String {
        match self {
            DeriveIssue::DuplicateExposed { position } => format!(
                "glyph '{glyph}' would expose anchor '{position}' from more than one \
                 source; it exposes neither — declare the anchor explicitly",
            ),
            DeriveIssue::AmbiguousAttachment { position, ref_name } => format!(
                "glyph '{glyph}': ref '{ref_name}' finds more than one '{}{}' anchor \
                 to attach its '{position}' to; nothing is attached",
                "+",
                position.strip_prefix('-').unwrap_or(position),
            ),
            DeriveIssue::SizeMismatchedAttachment { position, ref_name, minus, plus } => {
                format!(
                    "glyph '{glyph}': ref '{ref_name}' has '{position}' ({}x{}) matching \
                     a '+{}' only by name ({}x{}); not attached — check the other variants",
                    minus.0,
                    minus.1,
                    position.strip_prefix('-').unwrap_or(position),
                    plus.0,
                    plus.1,
                )
            }
        }
    }

    /// Whether this finding invalidates the anchors involved (error) or only
    /// flags a near-miss the composite still resolves around (warning).
    pub(crate) fn is_error(&self) -> bool {
        !matches!(self, DeriveIssue::SizeMismatchedAttachment { .. })
    }
}

/// An anchor in the derivation pool: the point (already translated into the
/// composite's coordinates) and where it came from — `None` for the
/// composite's own declared anchors, `Some(i)` for one published by ref `i`.
type PoolAnchor = (GlyphPoint, Option<usize>);

/// Derive effective ref offsets and the anchors exposed by the resulting
/// composite without changing the source refs.  A target's `-name` anchors
/// consume matching `+name` anchors that are already available, then the
/// target's `+name` anchors are published for following refs.
///
/// What the composite *exposes* is opt-in: its own declared anchors, plus the
/// surviving anchors (published `+`, unconsumed `-`) of refs marked
/// `inherit`.  Attachment between sibling refs works regardless of the flag.
/// If two survivors of the exposed set share a name, the composite exposes
/// neither and a [`DeriveIssue::DuplicateExposed`] is reported; likewise a
/// `-` anchor with more than one size-matching `+` candidate attaches to
/// nothing and reports [`DeriveIssue::AmbiguousAttachment`].
///
/// Ref order does not matter: when a target carries `-name` and some other
/// still-unresolved ref could yet publish `+name`, resolution is deferred
/// until it does.  A minus anchor no remaining ref can satisfy does *not*
/// defer — deferral would let explicit-offset sibling refs commit first and
/// miss their consumption.  Refs that remain unresolved after the fixpoint
/// fall back to `(0, 0)`.
///
/// `lookup_alternatives` returns sorted alternative glyph names for a base
/// name (e.g. for "foo" it returns ["foo:bar", "foo:baz"]).  When the
/// primary ref's anchors don't size-match, alternatives are tried in order.
///
/// `lookup_declared_anchors` returns a ref target's own declared anchors
/// (not forwarded from its refs).  This enables look-ahead alternative
/// selection: if a later ref would consume `+X` via `-X` and the current
/// ref's declared anchors lack `+X` (it is only forwarded from sub-refs),
/// an alternative that declares `+X` directly is preferred.
pub(crate) fn derive_ref_offsets_with(
    declared_anchors: &[GlyphPoint],
    refs: &[GlyphRef],
    mut lookup_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut lookup_alternatives: impl FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
    mut lookup_declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
) -> (Vec<GlyphRef>, Vec<PoolAnchor>, Vec<DeriveIssue>) {
    let mut issues: Vec<DeriveIssue> = Vec::new();
    let mut survived_minus: Vec<PoolAnchor> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .map(|p| (p.clone(), None))
        .collect();
    let mut available_plus: Vec<PoolAnchor> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .map(|p| (p.clone(), None))
        .collect();

    let target_anchors_list: Vec<Option<Vec<GlyphPoint>>> = refs
        .iter()
        .map(|gref| lookup_anchors(&gref.name))
        .collect();

    let target_declared_anchors_list: Vec<Option<Vec<GlyphPoint>>> = refs
        .iter()
        .map(|gref| lookup_declared_anchors(&gref.name))
        .collect();

    let alternatives_list: Vec<Vec<(String, Vec<GlyphPoint>)>> = refs
        .iter()
        .map(|gref| {
            if gref.offset.is_some() {
                Vec::new()
            } else {
                lookup_alternatives(&gref.name)
            }
        })
        .collect();

    let n = refs.len();
    let mut effective_refs: Vec<Option<GlyphRef>> = vec![None; n];

    loop {
        let mut progress = false;
        for (i, gref) in refs.iter().enumerate() {
            if effective_refs[i].is_some() {
                continue;
            }

            let Some(ref target_anchors) = target_anchors_list[i] else {
                effective_refs[i] = Some(gref.clone());
                progress = true;
                continue;
            };

            if let Some(offset) = gref.offset {
                commit_ref(
                    gref,
                    i,
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            match try_match_minus_plus(target_anchors, &available_plus) {
                MatchOutcome::Unique(offset) => {
                    commit_ref(
                        gref,
                        i,
                        offset,
                        target_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    progress = true;
                    continue;
                }
                MatchOutcome::Ambiguous => {
                    // The attachment is ill-defined; commit unattached and be
                    // loud (commit_ref reports it) rather than silently
                    // swapping in an alternative or picking one candidate.
                    commit_ref(
                        gref,
                        i,
                        (0, 0),
                        target_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    progress = true;
                    continue;
                }
                MatchOutcome::NoMatch => {}
            }

            // Try alternatives when primary doesn't size-match.
            let mut alt_matched = false;
            for (alt_name, alt_anchors) in &alternatives_list[i] {
                if let MatchOutcome::Unique(offset) =
                    try_match_minus_plus(alt_anchors, &available_plus)
                {
                    let alt_gref = GlyphRef {
                        comment: None,
                        name: alt_name.clone(),
                        offset: None,
                        negated: gref.negated,
                        inherit: gref.inherit,
                        fill: gref.fill.clone(),
                        visibility: gref.visibility,
                    };
                    commit_ref(
                        &alt_gref,
                        i,
                        offset,
                        alt_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    alt_matched = true;
                    progress = true;
                    break;
                }
            }
            if alt_matched {
                continue;
            }

            // Defer only while some other still-unresolved ref could publish a
            // `+` this candidate's `-` might match. A minus nothing remaining
            // can satisfy (a base's own `-center`, say) must not wait: waiting
            // would let explicit-offset sibling refs commit first and miss the
            // consumption of this ref's `+` anchors.
            let wanted: Vec<&str> = target_anchors
                .iter()
                .chain(alternatives_list[i].iter().flat_map(|(_, a)| a.iter()))
                .filter_map(|p| p.position.strip_prefix('-'))
                .collect();
            let could_match_later = !wanted.is_empty()
                && (0..n).any(|j| {
                    j != i
                        && effective_refs[j].is_none()
                        && target_anchors_list[j]
                            .iter()
                            .flatten()
                            .chain(
                                alternatives_list[j].iter().flat_map(|(_, a)| a.iter()),
                            )
                            .filter_map(|p| p.position.strip_prefix('+'))
                            .any(|base| wanted.contains(&base))
                });
            if could_match_later {
                continue;
            }

            // Look-ahead substitution: if this ref publishes +anchor
            // that a subsequent unresolved ref would consume via -anchor,
            // prefer an alternative that provides +anchor directly.
            let alt_found = try_lookahead_alt(
                i, n, target_anchors,
                target_declared_anchors_list[i].as_deref(),
                &available_plus,
                &alternatives_list[i], &effective_refs, &target_anchors_list,
            );
            if let Some((alt_name, alt_anchors)) = alt_found {
                let alt_gref = GlyphRef {
                    comment: None,
                    name: alt_name,
                    offset: None,
                    negated: gref.negated,
                    inherit: gref.inherit,
                    fill: gref.fill.clone(),
                    visibility: gref.visibility,
                };
                commit_ref(
                    &alt_gref,
                    i,
                    (0, 0),
                    alt_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
            } else {
                commit_ref(
                    gref,
                    i,
                    (0, 0),
                    target_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
            }
            progress = true;
        }
        if !progress {
            break;
        }
    }

    for (i, gref) in refs.iter().enumerate() {
        if effective_refs[i].is_some() {
            continue;
        }
        let target_anchors = target_anchors_list[i].as_deref().unwrap_or(&[]);
        let (resolved_name, offset, used_anchors) = if let Some(offset) = gref.offset {
            (gref.name.clone(), offset, target_anchors)
        } else {
            match try_match_minus_plus(target_anchors, &available_plus) {
                MatchOutcome::Unique(offset) => (gref.name.clone(), offset, target_anchors),
                // Ambiguity is reported by commit_ref below; commit unattached.
                MatchOutcome::Ambiguous => (gref.name.clone(), (0, 0), target_anchors),
                MatchOutcome::NoMatch => {
                    let mut found = None;
                    for (alt_name, alt_anchors) in &alternatives_list[i] {
                        if let MatchOutcome::Unique(offset) =
                            try_match_minus_plus(alt_anchors, &available_plus)
                        {
                            found = Some((alt_name.clone(), offset, alt_anchors.as_slice()));
                            break;
                        }
                    }
                    if found.is_none()
                        && let Some((alt_name, alt_anchors)) = try_lookahead_alt(
                            i, n, target_anchors,
                            target_declared_anchors_list[i].as_deref(),
                            &available_plus,
                            &alternatives_list[i], &effective_refs, &target_anchors_list,
                        ) {
                            found = Some((alt_name.clone(), (0, 0), alt_anchors));
                        }
                    found.unwrap_or_else(|| (gref.name.clone(), (0, 0), target_anchors))
                }
            }
        };
        let resolved_gref = GlyphRef {
            comment: None,
            name: resolved_name,
            offset: gref.offset,
            negated: gref.negated,
            inherit: gref.inherit,
            fill: gref.fill.clone(),
            visibility: gref.visibility,
        };
        commit_ref(
            &resolved_gref,
            i,
            offset,
            used_anchors,
            &mut available_plus,
            &mut survived_minus,
            &mut issues,
            &mut effective_refs[i],
        );
    }

    let effective_refs: Vec<GlyphRef> =
        effective_refs.into_iter().map(Option::unwrap).collect();

    // Exposure is opt-in: declared anchors always pass, a ref's survivors only
    // through its `inherit` flag. Survivors sharing a name are then dropped
    // together — the composite must not pick one of them silently. Each
    // exposed anchor keeps its source (`None` = declared, `Some(i)` = ref
    // `i`), which is how the palette colors an inherited anchor like the
    // subglyph it came from.
    let mut exposed: Vec<PoolAnchor> = survived_minus
        .into_iter()
        .chain(available_plus)
        .filter(|(_, source)| source.is_none_or(|i| refs[i].inherit))
        .collect();
    let mut duplicated: Vec<String> = Vec::new();
    for (i, (p, _)) in exposed.iter().enumerate() {
        if exposed[..i].iter().any(|(o, _)| o.position == p.position)
            && !duplicated.contains(&p.position)
        {
            duplicated.push(p.position.clone());
        }
    }
    for position in duplicated {
        exposed.retain(|(p, _)| p.position != position);
        issues.push(DeriveIssue::DuplicateExposed { position });
    }

    (effective_refs, exposed, issues)
}

/// Look-ahead alternative selection: when a ref at index `i` publishes
/// `+anchor` that a subsequent unresolved ref would consume via `-anchor`,
/// AND the ref's own declared points do NOT include that `+anchor` (it is
/// only forwarded from the ref's own sub-refs), prefer an alternative that
/// provides `+anchor` directly.
///
/// This handles cases like `i-lower` (which forwards `+above` from
/// `ref i-lower:dotless`) followed by `dia-above` (which needs `-above`):
/// `i-lower:dotless` should be used because it is the correct visual form
/// when the above-anchor is consumed (the dot would conflict).  But when
/// followed by `dia-below`, no substitution occurs because `i-lower` has
/// `+below` as its own declared anchor.
fn try_lookahead_alt<'a>(
    i: usize,
    n: usize,
    target_anchors: &[GlyphPoint],
    target_declared_anchors: Option<&[GlyphPoint]>,
    available_plus: &[PoolAnchor],
    alternatives: &'a [(String, Vec<GlyphPoint>)],
    effective_refs: &[Option<GlyphRef>],
    target_anchors_list: &[Option<Vec<GlyphPoint>>],
) -> Option<(String, &'a [GlyphPoint])> {
    let declared = target_declared_anchors?;
    let plus_anchors: Vec<&GlyphPoint> = target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .filter(|p| !available_plus.iter().any(|(a, _)| a.position == p.position))
        .filter(|p| !declared.iter().any(|o| o.position == p.position))
        .collect();
    if plus_anchors.is_empty() {
        return None;
    }
    for (alt_name, alt_anchors) in alternatives {
        for plus in &plus_anchors {
            let base = plus.position.strip_prefix('+')?;
            let minus_name = format!("-{base}");
            let needed_by_later = (i + 1..n).any(|j| {
                effective_refs[j].is_none()
                    && target_anchors_list[j]
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .any(|p| p.position == minus_name)
            });
            if needed_by_later
                && alt_anchors.iter().any(|a| a.position == plus.position)
            {
                return Some((alt_name.clone(), alt_anchors.as_slice()));
            }
        }
    }
    None
}

/// What matching a target's `-` anchors against the pool produced.
enum MatchOutcome {
    NoMatch,
    Unique((i16, i16)),
    /// The first `-` anchor with any candidate had more than one — the
    /// attachment is ill-defined, and no other anchor is tried instead.
    Ambiguous,
}

fn try_match_minus_plus(
    target_anchors: &[GlyphPoint],
    available_plus: &[PoolAnchor],
) -> MatchOutcome {
    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        let mut candidates = available_plus.iter().filter(|(p, _)| {
            p.position.strip_prefix('+') == Some(base) && p.size_matches(minus)
        });
        let Some((plus, _)) = candidates.next() else {
            continue;
        };
        if candidates.next().is_some() {
            return MatchOutcome::Ambiguous;
        }
        return MatchOutcome::Unique((
            saturating_i16(plus.col as i32 - minus.col as i32),
            saturating_i16(plus.row as i32 - minus.row as i32),
        ));
    }
    MatchOutcome::NoMatch
}

fn translate_point(p: &GlyphPoint, off_col: i16, off_row: i16) -> GlyphPoint {
    GlyphPoint {
        comment: None,
        position: p.position.clone(),
        col: saturating_i16(p.col as i32 + off_col as i32),
        row: saturating_i16(p.row as i32 + off_row as i32),
        col_end: saturating_i16(p.col_end as i32 + off_col as i32),
        row_end: saturating_i16(p.row_end as i32 + off_row as i32),
    }
}

fn commit_ref(
    gref: &GlyphRef,
    ref_idx: usize,
    offset: (i16, i16),
    target_anchors: &[GlyphPoint],
    available_plus: &mut Vec<PoolAnchor>,
    survived_minus: &mut Vec<PoolAnchor>,
    issues: &mut Vec<DeriveIssue>,
    out: &mut Option<GlyphRef>,
) {
    let effective = GlyphRef {
        comment: None,
        name: gref.name.clone(),
        offset: Some(offset),
        negated: gref.negated,
        inherit: gref.inherit,
        fill: gref.fill.clone(),
        visibility: gref.visibility,
    };
    let off_col = effective.col();
    let off_row = effective.row();

    // Consume before publishing. In particular, a component carrying
    // both `-join` and `+join` must publish its outgoing anchor rather
    // than immediately deleting it again. Consumption needs a *unique*
    // size-matching `+`: more than one means the attachment is ill-defined,
    // so nothing is consumed (and nothing survives) — loudly.
    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        let matching: Vec<usize> = available_plus
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| {
                p.position.strip_prefix('+') == Some(base) && p.size_matches(minus)
            })
            .map(|(i, _)| i)
            .collect();
        match matching.len() {
            0 => {
                // A same-name `+` of the wrong size is a near-miss worth
                // flagging; no same-name `+` at all is plain forwarding.
                if let Some((near, _)) = available_plus
                    .iter()
                    .find(|(p, _)| p.position.strip_prefix('+') == Some(base))
                {
                    issues.push(DeriveIssue::SizeMismatchedAttachment {
                        position: minus.position.clone(),
                        ref_name: gref.name.clone(),
                        minus: (minus.width(), minus.height()),
                        plus: (near.width(), near.height()),
                    });
                }
                survived_minus
                    .push((translate_point(minus, off_col, off_row), Some(ref_idx)));
            }
            1 => {
                available_plus.remove(matching[0]);
            }
            _ => issues.push(DeriveIssue::AmbiguousAttachment {
                position: minus.position.clone(),
                ref_name: gref.name.clone(),
            }),
        }
    }
    for plus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
    {
        available_plus.push((translate_point(plus, off_col, off_row), Some(ref_idx)));
    }

    *out = Some(effective);
}

/// Detect any on-demand glyph for `name`: first tries WxH rect, then
/// color/mono composite (checking whether X:mono and X:color exist).
pub fn detect_on_demand_glyph(name: &str, has_glyph: impl Fn(&str) -> bool) -> Option<OnDemandGlyph> {
    parse_on_demand_glyph(name).or_else(|| detect_color_mono_glyph(name, has_glyph))
}

pub fn resolve_named_glyphs_with_parts(
    docs: &[&Document],
    name_parts: &NamePartsMap,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    let expansion = crate::render::ttf_builder::expand_documents(docs, name_parts);
    resolve_expansion(expansion, name_parts)
}

/// Compose every glyph in an already-expanded document set.
///
/// Name-part substitution, pattern expansion and on-demand/decomposed-map
/// synthesis all happen in [`crate::render::ttf_builder::expand_documents`],
/// so this function starts from the same glyph set the font build sees. It
/// used to redo all of that itself, which is how the editor and the built
/// font could disagree about which glyphs exist.
pub fn resolve_expansion(
    expansion: crate::render::ttf_builder::Expansion,
    name_parts: &NamePartsMap,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();

    struct Pending {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
        scale: u8,
    }

    let mut pending: Vec<Pending> = Vec::new();
    // Mirrors `pending` names for O(1) duplicate checks; a linear scan here
    // is quadratic over the whole font (~18k glyphs).
    let mut pending_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    // The expansion is consumed, not borrowed: it already owns a full copy of
    // every glyph body, and cloning it a second time into `pending` cost more
    // than sharing the expansion saved.
    for expanded in expansion.items {
        let DocumentItem::Glyph { name: GlyphName(key), body } = expanded.item else {
            continue;
        };
        // First definition wins, matching the font build.
        if cache.contains_key(&key) || pending_names.contains(&key) {
            continue;
        }
        if body.refs.is_empty() {
            cache.insert(
                key,
                ResolvedGlyph {
                    grid: body.pixels.unwrap_or_else(|| PixelGrid::new(0, 0)),
                    origin_row: 0,
                    origin_col: 0,
                    resolved_anchors: body.points.clone(),
                    declared_anchors: body.points,
                    scale: body.scale,
                },
            );
        } else {
            pending_names.insert(key.clone());
            pending.push(Pending {
                name: key,
                pixels: body.pixels,
                refs: body.refs,
                points: body.points,
                scale: body.scale,
            });
        }
    }

    // Built once and grown incrementally: a full rebuild clones every name
    // and anchor list in the cache, which is too expensive to repeat per
    // fixpoint round.
    let mut alt_index = AlternativesIndex::build(&cache);

    // How many alternatives of each base name are still unresolved. A composite
    // must not be derived while one of them is pending: the alternative would be
    // missing from `alt_index`, so a ref whose anchors only size-match *that*
    // alternative silently falls back to offset (0, 0) instead. `i-upper` +
    // `acute-above` did exactly that — serving `i-upper`'s two-cell `+above` is
    // the whole reason `acute-above:wide` exists.
    let mut pending_alts: HashMap<String, usize> = HashMap::new();
    for pg in &pending {
        for prefix in alternative_prefixes(&pg.name) {
            *pending_alts.entry(prefix.to_string()).or_default() += 1;
        }
    }

    // Dropped for one round whenever that guard is what blocks every remaining
    // glyph (a reference cycle running through an alternative), so resolution
    // still terminates with the fallbacks it produced before.
    let mut relaxed = false;
    loop {
        let mut progress = false;
        for pg in std::mem::take(&mut pending) {
            if !pg
                .refs
                .iter()
                .all(|r| resolve_ref_name_with_parts(&r.name, &cache, name_parts).is_some())
                || (!relaxed
                    && pg
                        .refs
                        .iter()
                        .any(|r| r.offset.is_none() && pending_alts.contains_key(&r.name)))
            {
                pending.push(pg);
                continue;
            }
            let (effective_refs, exposed, _issues) =
                derive_ref_offsets_with(
                    &pg.points,
                    &pg.refs,
                    |name| {
                        resolve_ref_name_with_parts(name, &cache, name_parts)
                            .map(|resolved| resolved.resolved_anchors.clone())
                    },
                    |name| alt_index.get(name).to_vec(),
                    |name| {
                        resolve_ref_name_with_parts(name, &cache, name_parts)
                            .map(|resolved| resolved.declared_anchors.clone())
                    },
                );
            let anchors: Vec<GlyphPoint> =
                exposed.into_iter().map(|(p, _)| p).collect();
            let layout = resolve_composite_layout(
                pg.pixels.as_ref(),
                &effective_refs,
                &cache,
                name_parts,
                pg.scale,
            );
            let (min_r, min_c) = (layout.min_r, layout.min_c);
            let grid = layout.to_grid(pg.pixels.as_ref(), pg.scale);
            for prefix in alternative_prefixes(&pg.name) {
                if let Some(count) = pending_alts.get_mut(prefix) {
                    *count -= 1;
                    if *count == 0 {
                        pending_alts.remove(prefix);
                    }
                }
            }
            // Merged right away rather than at round end: a composite later in
            // the same round has to see the alternatives resolved before it.
            alt_index.extend([(pg.name.clone(), anchors.clone())]);
            cache.insert(
                pg.name.clone(),
                ResolvedGlyph {
                    grid,
                    origin_row: min_r,
                    origin_col: min_c,
                    resolved_anchors: anchors,
                    declared_anchors: pg.points.clone(),
                    scale: pg.scale,
                },
            );
            progress = true;
        }
        if pending.is_empty() {
            break;
        }
        if progress {
            relaxed = false;
        } else if relaxed {
            break;
        } else {
            relaxed = true;
        }
    }

    (cache, alt_index)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct GlyphComposite {
    pub width: u16,
    pub height: u16,
    pub own_offset_row: i16,
    pub own_offset_col: i16,
    pub layers: Vec<CompositeLayer>,
    /// Anchors the composite exposes through `inherit` refs rather than its
    /// own `anchor` lines, with the source ref's index. The subglyph palette
    /// lists them after the declared points, colored like the source
    /// subglyph; they have no document line, so they cannot be moved or
    /// renamed.
    pub inherited_anchors: Vec<(GlyphPoint, usize)>,
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
impl GlyphComposite {
    pub fn any_layer_filled_at(&self, composite_row: i16, composite_col: i16) -> bool {
        let mut filled = false;
        for layer in &self.layers {
            let lr = composite_row - layer.offset_row;
            let lc = composite_col - layer.offset_col;
            if lr >= 0
                && lr < layer.grid.height as i16
                && lc >= 0
                && lc < layer.grid.width as i16
                && layer.grid.get(lr as u16, lc as u16).is_filled()
            {
                filled = !layer.negated;
            }
        }
        filled
    }
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct CompositeLayer {
    pub ref_idx: usize,
    /// The resolved name (may differ from the source ref if an alternative was chosen).
    pub resolved_name: String,
    pub grid: PixelGrid,
    pub offset_row: i16,
    pub offset_col: i16,
    /// The ref placement in the owning glyph's logical coordinate space.
    /// This differs from `offset_*` when the resolved target has a negative
    /// logical origin.
    pub logical_offset_row: i16,
    pub logical_offset_col: i16,
    pub negated: bool,
    #[cfg(feature = "editor")]
    pub fill_color: Option<egui::Color32>,
}

pub fn resolve_ref_name_with_parts<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> Option<&'a ResolvedGlyph> {
    if let Some(resolved) = named_glyphs.get(name) {
        return Some(resolved);
    }
    let subst = substitute_name_parts(name, name_parts);
    if let Some(resolved) = named_glyphs.get(&subst) {
        return Some(resolved);
    }
    if let Some(expanded) = parse_ref_pattern(&subst) {
        return named_glyphs.get(&expanded.get(0));
    }
    None
}

/// Check that a ref name resolves to valid glyphs. For pattern refs, ALL
/// expansions must exist; returns false if any expansion is missing.
#[cfg(any(feature = "editor", test))]
pub fn is_ref_valid(
    name: &str,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    if named_glyphs.contains_key(name) {
        return true;
    }
    if parse_on_demand_glyph(name).is_some() {
        return true;
    }
    let subst = substitute_name_parts(name, name_parts);
    if named_glyphs.contains_key(&subst) {
        return true;
    }
    if parse_on_demand_glyph(&subst).is_some() {
        return true;
    }
    if let Some(expanded) = parse_ref_pattern(&subst) {
        return expanded
            .iter()
            .all(|n| named_glyphs.contains_key(&n) || parse_on_demand_glyph(&n).is_some());
    }
    false
}

/// Iterator over the base prefixes an alternative name registers under:
/// `foo:bar:baz` yields `foo:bar`, then `foo`.
pub(crate) fn alternative_prefixes(name: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(
        name.rfind(':').map(|p| &name[..p]),
        |prefix| prefix.rfind(':').map(|p| &prefix[..p]),
    )
}

/// Pre-built index mapping each base name to its sorted alternatives.
/// For glyph "foo:bar", entries are added under base "foo".
/// For "foo:bar:baz", entries are added under "foo" and "foo:bar".
#[derive(Clone, Debug, Default)]
pub struct AlternativesIndex {
    map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>>,
}

impl AlternativesIndex {
    pub fn build(named_glyphs: &HashMap<String, ResolvedGlyph>) -> Self {
        let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
        for (name, resolved) in named_glyphs {
            for prefix in alternative_prefixes(name) {
                map.entry(prefix.to_string())
                    .or_default()
                    .push((name.clone(), resolved.resolved_anchors.clone()));
            }
        }
        for alts in map.values_mut() {
            alts.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        Self { map }
    }

    /// Add entries for newly resolved glyphs, keeping each alternative list
    /// sorted (the same order [`Self::build`] produces). Lets the resolve
    /// fixpoint grow one index instead of rebuilding it from the whole cache
    /// every round.
    fn extend(&mut self, entries: impl IntoIterator<Item = (String, Vec<GlyphPoint>)>) {
        for (name, anchors) in entries {
            for prefix in alternative_prefixes(&name) {
                let alts = self.map.entry(prefix.to_string()).or_default();
                match alts.binary_search_by(|(a, _)| a.as_str().cmp(&name)) {
                    // Same glyph resolved again (cache overwrite): keep the
                    // list free of duplicate names, like a full rebuild would.
                    Ok(pos) => alts[pos].1 = anchors.clone(),
                    Err(pos) => alts.insert(pos, (name.clone(), anchors.clone())),
                }
            }
        }
    }

    pub fn get(&self, base_name: &str) -> &[(String, Vec<GlyphPoint>)] {
        self.map.get(base_name).map_or(&[], |v| v.as_slice())
    }
}

fn ref_effective_offset_scaled(
    gref: &GlyphRef,
    resolved: &ResolvedGlyph,
    parent_scale: u8,
) -> (i32, i32) {
    let ps = parent_scale as i32;
    let rs = resolved.scale.max(1) as i32;
    (
        gref.row() as i32 + resolved.origin_row * ps / rs,
        gref.col() as i32 + resolved.origin_col * ps / rs,
    )
}

fn ref_grid_scaled(grid: &PixelGrid, ref_scale: u8, parent_scale: u8) -> PixelGrid {
    if ref_scale == parent_scale {
        grid.clone()
    } else {
        grid.rescale(ref_scale.max(1), parent_scale.max(1))
    }
}

/// One ref resolved to its target and raster placement.
struct ResolvedLayer<'a> {
    #[cfg_attr(not(any(feature = "editor", test)), expect(dead_code))]
    ref_idx: usize,
    gref: &'a GlyphRef,
    resolved: &'a ResolvedGlyph,
    raster_row: i32,
    raster_col: i32,
}

/// All refs of a composite resolved once, with the resulting bounding box.
///
/// This is the single resolution pass shared by the build cache
/// (`resolve_expansion` → [`CompositeLayout::to_grid`]), the editor's live
/// composite ([`compute_composite`]) and bounds queries
/// ([`composite_bounds`]).  They must agree on how every ref resolves —
/// a historical divergence here made the editor and the flattened font
/// disagree on pattern refs — so none of them resolves refs on its own.
struct CompositeLayout<'a> {
    layers: Vec<ResolvedLayer<'a>>,
    min_r: i32,
    min_c: i32,
    max_r: i32,
    max_c: i32,
}

fn resolve_composite_layout<'a>(
    own_pixels: Option<&PixelGrid>,
    refs: &'a [GlyphRef],
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    parent_scale: u8,
) -> CompositeLayout<'a> {
    let mut min_r: i32 = 0;
    let mut min_c: i32 = 0;
    let mut max_r: i32 = 0;
    let mut max_c: i32 = 0;

    if let Some(grid) = own_pixels {
        max_r = grid.height as i32;
        max_c = grid.width as i32;
    }

    let ps = parent_scale as i32;
    let mut layers = Vec::new();
    for (ref_idx, gref) in refs.iter().enumerate() {
        let Some(resolved) = resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts)
        else {
            continue;
        };
        let rs = resolved.scale.max(1) as i32;
        let (raster_row, raster_col) = ref_effective_offset_scaled(gref, resolved, parent_scale);
        if resolved.grid.width != 0 && resolved.grid.height != 0 {
            let scaled_h = resolved.grid.height as i32 * ps / rs;
            let scaled_w = resolved.grid.width as i32 * ps / rs;
            min_r = min_r.min(raster_row);
            min_c = min_c.min(raster_col);
            max_r = max_r.max(raster_row + scaled_h);
            max_c = max_c.max(raster_col + scaled_w);
        }
        layers.push(ResolvedLayer { ref_idx, gref, resolved, raster_row, raster_col });
    }

    CompositeLayout { layers, min_r, min_c, max_r, max_c }
}

impl CompositeLayout<'_> {
    #[cfg(any(feature = "editor", test))]
    fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.min_r, self.min_c, self.max_r, self.max_c)
    }

    /// Flatten into one grid: own pixels plus each layer blitted at its
    /// raster placement (negated refs subtract).
    fn to_grid(&self, own_pixels: Option<&PixelGrid>, parent_scale: u8) -> PixelGrid {
        let width = raster_dimension(self.min_c, self.max_c);
        let height = raster_dimension(self.min_r, self.max_r);
        let mut result = PixelGrid::new(width, height);

        if let Some(grid) = own_pixels {
            result.blit(grid, -self.min_r, -self.min_c, false);
        }

        for layer in &self.layers {
            let scaled = ref_grid_scaled(&layer.resolved.grid, layer.resolved.scale, parent_scale);
            result.blit(
                &scaled,
                layer.raster_row - self.min_r,
                layer.raster_col - self.min_c,
                layer.gref.negated,
            );
        }

        result
    }
}

/// Bounding box (min_row, min_col, max_row, max_col) of a composite made of
/// `own_pixels` (if any) plus `refs`, each resolved against `named_glyphs`
/// via [`resolve_ref_name_with_parts`] (which falls back to pattern expansion
/// when a ref name isn't a direct cache key).
#[cfg(any(feature = "editor", test))]
pub(crate) fn composite_bounds(
    own_pixels: Option<&PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    parent_scale: u8,
) -> (i32, i32, i32, i32) {
    resolve_composite_layout(own_pixels, refs, named_glyphs, name_parts, parent_scale).bounds()
}

#[cfg(feature = "editor")]
fn resolve_fill_display_color(
    fill: &crate::document::RefFill,
    aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<egui::Color32> {
    if fill.color == "fg" {
        return None;
    }
    if fill.color.starts_with('#') {
        let rgba = crate::render::ttf_builder::parse_hex_color(&fill.color)?;
        return Some(egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a));
    }
    let (rgba, _) = aliases.get(&fill.color)?;
    Some(egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a))
}

#[cfg(any(feature = "editor", test))]
pub fn compute_composite(
    body: &GlyphBody,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &AlternativesIndex,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<GlyphComposite> {
    if body.refs.is_empty() {
        return None;
    }

    let (effective_refs, exposed, _) = derive_ref_offsets_with(
        &body.points,
        &body.refs,
        |name| {
            resolve_ref_name_with_parts(name, named_glyphs, name_parts)
                .map(|resolved| resolved.resolved_anchors.clone())
        },
        |name| alt_index.get(name).to_vec(),
        |name| {
            resolve_ref_name_with_parts(name, named_glyphs, name_parts)
                .map(|resolved| resolved.declared_anchors.clone())
        },
    );
    let inherited_anchors: Vec<(GlyphPoint, usize)> = exposed
        .into_iter()
        .filter_map(|(p, source)| source.map(|ref_idx| (p, ref_idx)))
        .collect();

    let layout = resolve_composite_layout(
        body.pixels.as_ref(),
        &effective_refs,
        named_glyphs,
        name_parts,
        body.scale,
    );
    let (min_r, min_c, max_r, max_c) = layout.bounds();

    let width = raster_dimension(min_c, max_c).max(1);
    let height = raster_dimension(min_r, max_r).max(1);

    let mut layers = Vec::new();
    for layer in &layout.layers {
        let scaled_grid = ref_grid_scaled(&layer.resolved.grid, layer.resolved.scale, body.scale);
        let orig_ref = &body.refs[layer.ref_idx.min(body.refs.len() - 1)];
        #[cfg(feature = "editor")]
        let fill_color = orig_ref.fill.as_ref().and_then(|f| resolve_fill_display_color(f, color_aliases));
        #[cfg(not(feature = "editor"))]
        {
            let _ = color_aliases;
            let _ = &orig_ref.fill;
        }
        layers.push(CompositeLayer {
            ref_idx: layer.ref_idx,
            resolved_name: layer.gref.name.clone(),
            grid: scaled_grid,
            offset_row: saturating_i16(layer.raster_row - min_r),
            offset_col: saturating_i16(layer.raster_col - min_c),
            logical_offset_row: layer.gref.row(),
            logical_offset_col: layer.gref.col(),
            negated: layer.gref.negated,
            #[cfg(feature = "editor")]
            fill_color,
        });
    }

    Some(GlyphComposite {
        width,
        height,
        own_offset_row: saturating_i16(-min_r),
        own_offset_col: saturating_i16(-min_c),
        layers,
        inherited_anchors,
    })
}

#[cfg(test)]
fn composite_to_grid(
    own_pixels: &Option<PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    parent_scale: u8,
) -> PixelGrid {
    resolve_composite_layout(own_pixels.as_ref(), refs, named_glyphs, name_parts, parent_scale)
        .to_grid(own_pixels.as_ref(), parent_scale)
}

#[cfg(test)]
#[path = "ref_composite_tests.rs"]
mod tests;
