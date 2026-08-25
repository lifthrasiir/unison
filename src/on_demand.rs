//! On-demand glyph synthesis: names nothing defines but that describe a shape.
//!
//! A name that no `glyph` block defines but that matches a synthesizable shape
//! is generated on the spot, and such a glyph is implicitly `inline`:
//!
//! - `[-|_]W[pArR]x[-|_]H[pBrR]` — the **declared box**: a filled rectangle,
//!   each dimension either a whole number of cells or `A + B/R`; e.g. `1p2r3x4`
//!   is 1⅔ × 4. See [`parse_on_demand_glyph`] for the exact constraints and
//!   [`BoxAlign`] for what a leading `-` or `_` aligns.
//! - the box with a `-ul`/`-ur`/`-dl`/`-dr` suffix — a right triangle.
//! - the box with `-circle` — the ellipse inscribed in it.
//! - the box with `-polyN[.MMM|rK][-cwR|-ccwR]` — a regular N-gon or a star
//!   inscribed in that ellipse.
//! - the box with `-xsN`/`-xzN`/`-ysN`/`-yzN` — the parallelogram whose two
//!   sheared edges sit `N` apart. See [`ShearSpec`].
//! - any of those with a trailing `:ceil`/`:floor`/`:zero` — the
//!   [`BitmapFill`] rule.
//! - `X` where `X` is undefined but both `X:mono` and `X:color` exist — picks
//!   by rendering mode ([`detect_color_mono_glyph`]).
//!
//! The grammar is parsed strictly left to right and must be matched in full:
//! there is exactly one shape suffix and at most one fill suffix, in that
//! order. Any leftover — a second shape word, an unknown `:`-suffix — makes the
//! name a non-match, so ordinary glyph names containing `-` or `:` fall
//! through to normal lookup untouched.
//!
//! # The declared box is the glyph's size
//!
//! Whatever the shape, the synthesized grid is `ceil(W) × ceil(H)` logical
//! pixels: the box fixes the glyph's extent, and the shape only decides which
//! part of it is inked. A fractional dimension does not fill its last cell, and
//! the sign on that dimension is what says where the leftover gap falls — at
//! the far end (no sign), at the near end (`-`) or split between the two (`_`).
//! The box is anchored to integer coordinates the same way for every shape.
//!
//! A shear leans the same way: the box stays `ceil(W) × ceil(H)` and the
//! parallelogram is inscribed in it, so `WxH-xsN` is a rectangle of
//! `(W - N) × H` with its two horizontal edges slid `N` apart, rather than a
//! `W × H` one sheared until it overhangs. `N` is therefore a *length* on the
//! same lattice as the box — the offset between the two edges, which is what
//! one looks at when drawing a stroke — and not the slope, which would put
//! every useful value below 1. The box a name states is what every reader of
//! it — the `:WxH-l` variant rule, the autocompletion of an IDC slot,
//! `declared_box`, the clearance an `InkProfile` measures — takes for the
//! glyph's size, and a shape that grew past its own name would be wrong for
//! all of them. A shear that eats the whole dimension (`N >= W`) encloses
//! nothing and is not a name at all, like a zero-width box.
//!
//! # Circles and polygons live in a square box first
//!
//! `-circle` and `-polyN` are defined in an auxiliary *square* box of side
//! `min(|W|, |H|)` sharing the real box's center, and the finished shape is
//! then mapped onto the real box by the affine transform that takes the
//! auxiliary box to it. So `2x1-circle` is the ellipse filling 2 × 1, and a
//! rotated polygon is rotated in the square and *then* stretched — the
//! rotation is not applied to the stretched shape, which is why `-cw`/`-ccw`
//! sit before the stretch in the pipeline and not after.
//!
//! A polygon's outer points sit on that inscribed circle. `.MMM` pulls the
//! inner points towards the center as a fraction of the way in from the edge
//! midpoints: `.000` (the default) leaves them *on* the edges, so the shape is
//! the plain regular N-gon — note that even then the inner points are nearer
//! the center than the outer ones, by `cos(pi/N)`. `rK` instead picks the inner
//! radius of the `{N/K}` star polygon. `-cwR`/`-ccwR` turn the shape by R
//! degrees about the shared center, from a default angle that puts an outer
//! point at the top of the box.
//!
//! # Names are normalized, so equal shapes share one cached grid
//!
//! Several spellings mean the same polygon — `poly6`, `poly6.000`, `poly6r1`,
//! `poly6-cw60`, `poly6-ccw0` are all one shape. [`PolySpec`] is therefore a
//! *normalized* form, not a transcript of the name: a zero inset and `r1`
//! collapse to [`PolyInset::None`], and the rotation is folded into the
//! shape's own N-fold symmetry and turned into a clockwise fraction of a full
//! turn. That fraction is kept as an exact reduced rational because the folded
//! angle is not a whole number of degrees unless N divides 360 — `poly7-cw100`
//! normalizes to 100/360 mod 1/7 of a turn, which no decimal degree spells.
//! `rK` does *not* normalize into `.MMM`: its inner radius is irrational, so
//! `poly5r2` and `poly5.528` are near-identical but genuinely distinct shapes.
//! A shear normalizes the other way round: `-xs0` and its three siblings all
//! mean the plain rectangle, and parse to [`OnDemandShape::Rect`] so that the
//! rectangle keeps one spelling in the cache and in every match on the shape.
//!
//! Equal specs are the cache key for [`make_on_demand_grid`], which memoizes
//! the curved shapes — they cost a per-cell exact clip, unlike a rectangle.
//!
//! # Curves are polylines, cut on a lattice both sides of a border agree on
//!
//! [`crate::detail::DetailRegion`] stores straight-edged rings on a rational
//! lattice, so a circle enters the grid as a polygon fine enough that the
//! difference is below that lattice ([`POLY_Q`]).
//!
//! Cutting it into cells is where the care goes. The outline is first split at
//! every cell border, so no edge interior ever leaves the cell its endpoints
//! are in; each cell then clips that ring against its own box. Because of the
//! split, a clip can only ever land on a vertex that is already there, so the
//! whole thing runs in plain integers and — the point of the exercise — two
//! cells sharing a border cut the same edge at the same point instead of each
//! rounding its own. Disagree there and the contour tracer sees a heap of
//! fragments rather than one outline.
//!
//! Note that the exact-rational machinery in [`crate::detail`] is *not* what
//! does this; see [`REGION_DEN`] for why a curve cannot be handed to it.
//!
//! # The bitmap build has to be told what to light
//!
//! The font is built twice — a vector build reading the geometry, and a bitmap
//! build that keeps only the [`crate::pixel::PX_FULL`] ink flag and squares
//! every lit cell off — so a synthesized shape has to decide which cells that
//! second build lights. [`BitmapFill`] is that decision, made **per logical
//! pixel** from the exact covered area. Two invariants hold it together and are
//! easy to break: the rule applies uniformly across a logical pixel's subcells
//! (see [`apply_bitmap_fill`]), and it moves no outline (see
//! [`make_on_demand_grid`]). The ½ tie is real — it is every 45° triangle edge
//! cell — so the area comparison stays exact through
//! [`crate::detail::DetailRegion::area_units_on`], which measures every subcell
//! of a logical pixel on one shared lattice so the total is an integer sum
//! rather than a running fraction. `area2` is the f64 test helper, not the
//! production path.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Mutex, OnceLock};

use crate::detail::{Frac64, clip_polygon_to_cell};
use crate::document::PixelGrid;
use crate::pixel::{PX_ALMOSTFULL, PixelShape};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// How far a polygon's inner points are pulled towards the center.
///
/// Normalized: the spellings that mean "no inset at all" (`polyN`,
/// `polyN.000`, `polyNr1`) all parse to [`PolyInset::None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolyInset {
    /// The plain regular N-gon: the inner points lie on its edge midpoints,
    /// at radius `cos(pi/N)`.
    None,
    /// `.MMM` — thousandths of the way in from the edge midpoints towards the
    /// center, `1..=999`; the inner radius is `cos(pi/N) · (1 - MMM/1000)`.
    Milli(u16),
    /// `rK` — the inner radius of the `{N/K}` star polygon,
    /// `cos(pi·K/N) / cos(pi·(K-1)/N)`, with `2 <= K <= N/2`.
    Star(u8),
}

/// A regular polygon or star, normalized (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PolySpec {
    /// Number of outer points, at least 3.
    pub n: u8,
    pub inset: PolyInset,
    /// Clockwise rotation as an exact reduced fraction of a full turn, already
    /// folded into the shape's N-fold symmetry: `0 <= rot_num/rot_den < 1/n`,
    /// and `(0, 1)` for no rotation.
    pub rot_num: u32,
    pub rot_den: u32,
}

impl PolySpec {
    /// Inner-point radius as a fraction of the outer radius. Zero means every
    /// spike reaches the center and the shape encloses no area.
    fn inner_ratio(&self) -> f64 {
        let n = self.n as f64;
        match self.inset {
            PolyInset::None => (PI / n).cos(),
            PolyInset::Milli(m) => (PI / n).cos() * (1.0 - m as f64 / 1000.0),
            PolyInset::Star(k) => (PI * k as f64 / n).cos() / (PI * (k as f64 - 1.0) / n).cos(),
        }
    }
}

/// Which coordinate a shear displaces, and so which pair of the box's edges
/// slides apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShearAxis {
    /// `xs`/`xz`: x is displaced along y, and the horizontal edges slide.
    X,
    /// `ys`/`yz`: y is displaced along x, and the vertical edges do.
    Y,
}

/// A shear, normalized: `num` is never zero, because a name that writes a zero
/// displacement is [`OnDemandShape::Rect`] instead.
///
/// The displacement is a length, not a slope: `8x4-xs1` slides the two
/// horizontal edges one pixel apart, whatever the four rows between them do.
///
/// The letter is the direction: `s` for the way a slash leans, `z` for its
/// mirror. On the y axis that is still read on screen — `ys` puts the left
/// edge low and the right edge high, which leans like a slash just as `xs`
/// does — rather than being the sign of the matrix entry, which the two axes
/// would disagree about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShearSpec {
    pub axis: ShearAxis,
    /// `z` rather than `s`: the shape leans like an antidiagonal.
    pub anti: bool,
    /// How far apart the two sheared edges sit, in subcells of the box's own
    /// [`OnDemandBox::scale`] — a length on the same lattice as the box, and
    /// always below the dimension it is taken out of.
    pub num: u16,
}

/// What is drawn inside the declared box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OnDemandShape {
    /// The box itself, filled.
    Rect,
    /// A right triangle with legs W × H, the right angle at the named corner
    /// (`-ul`/`-ur`/`-dl`/`-dr`).
    Tri(TriCorner),
    /// The inscribed ellipse (`-circle`).
    Circle,
    /// A regular polygon or star inscribed in that ellipse (`-polyN…`).
    Poly(PolySpec),
    /// The parallelogram a shear inscribes in the box (`-xsN`/`-xzN`/`-ysN`/
    /// `-yzN`): the box's rectangle, shortened on the sheared axis by `N` and
    /// its two remaining edges slid `N` apart.
    Shear(ShearSpec),
}

/// How a shape's exact coverage of a *logical* pixel is rounded into that
/// pixel's ink flag ([`crate::pixel::PX_FULL`]), which is what the bitmap
/// build of the font draws (`ttf_builder::contours::CachedContours::from_grid`). The
/// vector build reads the geometry instead, so this never moves an outline.
///
/// Whole-pixel shapes are covered 1/1 everywhere and so render identically
/// under every variant but [`BitmapFill::Zero`]; the choice only bites on
/// fractional rectangles and on the cells a curve or hypotenuse crosses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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

/// Where a fractional dimension's leftover — the part of the last cell the box
/// does not fill — sits on its axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BoxAlign {
    /// No sign: the box is flush against the near edge (left/top) and the
    /// whole leftover falls at the far end.
    #[default]
    Near,
    /// `-`: flush against the far edge (right/bottom).
    Far,
    /// `_`: centered, the leftover split evenly between the two ends — exactly
    /// halfway between what no sign and `-` give, including when that puts the
    /// box half a subcell in ([`center_multiplier`]).
    Center,
}

/// The declared box plus the shape drawn in it — everything a name says.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnDemandBox {
    pub w: u8,
    pub h: u8,
    pub w_frac: u8,
    pub h_frac: u8,
    pub scale: u8,
    pub align_w: BoxAlign,
    pub align_h: BoxAlign,
    pub shape: OnDemandShape,
    /// From the `:ceil`/`:floor`/`:zero` name suffix; [`BitmapFill::Round`]
    /// when absent.
    pub fill: BitmapFill,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnDemandGlyph {
    Shape(OnDemandBox),
    ColorMono { mono: String, color: String },
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Take a run of ASCII digits with no leading zero (a bare `0` is fine),
/// returning its value and the rest of the input.
fn take_uint(s: &str) -> Option<(u32, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 || end > 5 {
        return None;
    }
    let (digits, rest) = s.split_at(end);
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    Some((digits.parse().ok()?, rest))
}

/// Take one to three digits of a decimal fraction, as thousandths. Leading
/// zeros are meaningful here (`.05` is 50), and the digits are padded on the
/// right (`.5` is 500).
fn take_milli(s: &str) -> Option<(u16, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 || end > 3 {
        return None;
    }
    let (digits, rest) = s.split_at(end);
    let mut v: u16 = digits.parse().ok()?;
    for _ in digits.len()..3 {
        v *= 10;
    }
    Some((v, rest))
}

/// One parsed dimension of the declared box: `[-|_]A[pBrR]`.
struct BoxDim {
    align: BoxAlign,
    base: u8,
    /// `(frac, scale)` of the `pBrR` part, absent for a plain `A`.
    detail: Option<(u8, u8)>,
}

/// The optional `pBrR` tail of a written number: a box dimension's fraction,
/// and a shear amount's, which are one lattice and so one spelling.
fn take_detail(s: &str) -> Option<(Option<(u8, u8)>, &str)> {
    let Some(s) = s.strip_prefix('p') else {
        return Some((None, s));
    };
    let (frac, s) = take_uint(s)?;
    let s = s.strip_prefix('r')?;
    let (scale, s) = take_uint(s)?;
    Some((
        Some((u8::try_from(frac).ok()?, u8::try_from(scale).ok()?)),
        s,
    ))
}

fn take_box_dim(s: &str) -> Option<(BoxDim, &str)> {
    let (align, s) = match s.as_bytes().first() {
        Some(b'-') => (BoxAlign::Far, &s[1..]),
        Some(b'_') => (BoxAlign::Center, &s[1..]),
        _ => (BoxAlign::Near, s),
    };
    let (base, s) = take_uint(s)?;
    let base = u8::try_from(base).ok()?;
    let (detail, s) = take_detail(s)?;
    Some((
        BoxDim {
            align,
            base,
            detail,
        },
        s,
    ))
}

/// Fold a written rotation into the shape's N-fold symmetry and turn it into a
/// clockwise, exactly reduced fraction of a full turn.
fn normalize_rotation(n: u8, milli_deg: u32, ccw: bool) -> (u32, u32) {
    const TURN: u64 = 360_000; // milli-degrees in a full turn
    let written = milli_deg as u64 % TURN;
    let cw = if ccw {
        (TURN - written) % TURN
    } else {
        written
    };
    // cw/TURN turns, taken modulo the 1/n-turn symmetry period.
    let num = cw * n as u64 % TURN;
    if num == 0 {
        return (0, 1);
    }
    let den = TURN * n as u64;
    let g = crate::math::gcd_u64(num, den);
    ((num / g) as u32, (den / g) as u32)
}

/// A shape word as written. Everything but a shear is settled here; a shear's
/// amount is a fraction on the box's own lattice, so it waits for
/// [`parse_on_demand_glyph`] to settle the one scale the whole name shares.
enum ShapeWord {
    Fixed(OnDemandShape),
    Shear {
        axis: ShearAxis,
        anti: bool,
        base: u8,
        detail: Option<(u8, u8)>,
    },
}

/// Take a shape word, which must run to the end of the name or to the `:` of
/// the fill suffix.
fn take_shape(s: &str) -> Option<(ShapeWord, &str)> {
    let ends_here = |rest: &str| rest.is_empty() || rest.starts_with(':');
    for (word, corner) in [
        ("ul", TriCorner::Ul),
        ("ur", TriCorner::Ur),
        ("dl", TriCorner::Dl),
        ("dr", TriCorner::Dr),
    ] {
        if let Some(rest) = s.strip_prefix(word)
            && ends_here(rest)
        {
            return Some((ShapeWord::Fixed(OnDemandShape::Tri(corner)), rest));
        }
    }
    if let Some(rest) = s.strip_prefix("circle")
        && ends_here(rest)
    {
        return Some((ShapeWord::Fixed(OnDemandShape::Circle), rest));
    }
    for (word, axis, anti) in [
        ("xs", ShearAxis::X, false),
        ("xz", ShearAxis::X, true),
        ("ys", ShearAxis::Y, false),
        ("yz", ShearAxis::Y, true),
    ] {
        // Once the word matches, the rest of the name is the amount or the
        // name is no on-demand name at all.
        let Some(rest) = s.strip_prefix(word) else {
            continue;
        };
        let (base, rest) = take_uint(rest)?;
        let base = u8::try_from(base).ok()?;
        let (detail, rest) = take_detail(rest)?;
        if !ends_here(rest) {
            return None;
        }
        return Some((
            ShapeWord::Shear {
                axis,
                anti,
                base,
                detail,
            },
            rest,
        ));
    }

    let s = s.strip_prefix("poly")?;
    let (n, s) = take_uint(s)?;
    let n = u8::try_from(n).ok()?;
    if n < 3 {
        return None;
    }
    let (inset, s) = if let Some(rest) = s.strip_prefix('.') {
        let (milli, rest) = take_milli(rest)?;
        let inset = if milli == 0 {
            PolyInset::None
        } else {
            PolyInset::Milli(milli)
        };
        (inset, rest)
    } else if let Some(rest) = s.strip_prefix('r') {
        let (k, rest) = take_uint(rest)?;
        let k = u8::try_from(k).ok()?;
        if k == 0 || u16::from(k) * 2 > u16::from(n) {
            return None;
        }
        // {N/1} is the regular N-gon itself.
        let inset = if k == 1 {
            PolyInset::None
        } else {
            PolyInset::Star(k)
        };
        (inset, rest)
    } else {
        (PolyInset::None, s)
    };

    let (ccw, s) = match s.strip_prefix("-ccw") {
        Some(rest) => (true, rest),
        None => match s.strip_prefix("-cw") {
            Some(rest) => (false, rest),
            None => {
                if !ends_here(s) {
                    return None;
                }
                return Some((
                    ShapeWord::Fixed(OnDemandShape::Poly(PolySpec {
                        n,
                        inset,
                        rot_num: 0,
                        rot_den: 1,
                    })),
                    s,
                ));
            }
        },
    };
    let (deg, s) = take_uint(s)?;
    if deg >= 360 {
        return None;
    }
    let (frac, s) = match s.strip_prefix('.') {
        Some(rest) => take_milli(rest)?,
        None => (0, s),
    };
    if !ends_here(s) {
        return None;
    }
    let (rot_num, rot_den) = normalize_rotation(n, deg * 1000 + frac as u32, ccw);
    Some((
        ShapeWord::Fixed(OnDemandShape::Poly(PolySpec {
            n,
            inset,
            rot_num,
            rot_den,
        })),
        s,
    ))
}

/// Parse an on-demand glyph name.
///
/// ```text
/// name  := dim 'x' dim [ '-' shape ] [ ':' fill ]
/// dim   := ['-' | '_'] uint [ 'p' uint 'r' uint ]
/// shape := 'ul' | 'ur' | 'dl' | 'dr' | 'circle'
///        | 'poly' uint [ '.' digit{1,3} | 'r' uint ] [ ('-cw' | '-ccw') angle ]
///        | ('xs' | 'xz' | 'ys' | 'yz') amount
/// angle := uint [ '.' digit{1,3} ]            -- degrees, below 360
/// amount:= uint [ 'p' uint 'r' uint ]         -- a length, like a dimension
/// fill  := 'ceil' | 'floor' | 'zero'
/// ```
///
/// The declared box: a plain `WxH` must have both dimensions nonzero and takes
/// no alignment sign. The fractional form `A[pBrR]` needs `R >= 2` on both
/// sides, `0 <= B,D < R`, and a positive total on each axis; a leading `-`
/// there flushes the ink against the far edge of the cell the fraction does
/// not fill, and a leading `_` centers it on that axis instead ([`BoxAlign`]).
///
/// **One name, one lattice.** Every fraction the name writes shares a single
/// `R` — the two dimensions and a shear's amount alike — and any of the three
/// may be the one that states it, so `3x4-xs1p1r2` needs no `p0r2` padding on
/// the box to say what lattice it is on. A shear also has to leave something
/// to draw: `N` comes out of the dimension it shears and so must stay below
/// it, and a zero amount is the plain rectangle rather than a shear of
/// nothing.
///
/// Nothing is optional beyond what the grammar says and nothing may follow it:
/// a name is either matched in full or is not an on-demand name at all, which
/// is what keeps ordinary glyph names containing `-` or `:` — every
/// alternative form, and the `:mono`/`:color` pair handled by
/// [`detect_color_mono_glyph`] — out of this path.
pub fn parse_on_demand_glyph(name: &str) -> Option<OnDemandGlyph> {
    let (w_dim, rest) = take_box_dim(name)?;
    let rest = rest.strip_prefix('x')?;
    let (h_dim, rest) = take_box_dim(rest)?;

    let (word, rest) = match rest.strip_prefix('-') {
        Some(rest) => take_shape(rest)?,
        None => (ShapeWord::Fixed(OnDemandShape::Rect), rest),
    };
    let fill = match rest.strip_prefix(':') {
        Some("ceil") => BitmapFill::Ceil,
        Some("floor") => BitmapFill::Floor,
        Some("zero") => BitmapFill::Zero,
        Some(_) => return None,
        None => {
            if !rest.is_empty() {
                return None;
            }
            BitmapFill::Round
        }
    };

    let BoxDim {
        align: align_w,
        base: w,
        detail: w_detail,
    } = w_dim;
    let BoxDim {
        align: align_h,
        base: h,
        detail: h_detail,
    } = h_dim;

    // One lattice for the whole name: whichever of the three fractions state
    // one, they have to state the same one.
    let shear_detail = match word {
        ShapeWord::Shear { detail, .. } => detail,
        ShapeWord::Fixed(_) => None,
    };
    let mut written_scale = None;
    for (_, r) in [w_detail, h_detail, shear_detail].into_iter().flatten() {
        if written_scale.is_some_and(|s| s != r) {
            return None;
        }
        written_scale = Some(r);
    }
    let scale = match written_scale {
        // The whole-cell form has no leftover to place, so it takes no sign.
        None => {
            if align_w != BoxAlign::Near || align_h != BoxAlign::Near {
                return None;
            }
            1
        }
        Some(r) => {
            if r < 2 {
                return None;
            }
            r
        }
    };
    let w_frac = w_detail.map_or(0, |(f, _)| f);
    let h_frac = h_detail.map_or(0, |(f, _)| f);
    if w_frac >= scale || h_frac >= scale {
        return None;
    }
    if (w == 0 && w_frac == 0) || (h == 0 && h_frac == 0) {
        return None;
    }

    let shape = match word {
        ShapeWord::Fixed(shape) => shape,
        ShapeWord::Shear {
            axis,
            anti,
            base,
            detail,
        } => {
            let frac = detail.map_or(0, |(f, _)| f);
            if frac >= scale {
                return None;
            }
            let num = u16::from(base) * u16::from(scale) + u16::from(frac);
            if num == 0 {
                // Normalized: the rectangle keeps one spelling.
                OnDemandShape::Rect
            } else {
                // What is left of the sheared dimension once the displacement
                // has taken its share: nothing left, nothing to draw. The
                // other dimension has no say in it.
                let own = match axis {
                    ShearAxis::X => u32::from(w) * u32::from(scale) + u32::from(w_frac),
                    ShearAxis::Y => u32::from(h) * u32::from(scale) + u32::from(h_frac),
                };
                if u32::from(num) >= own {
                    return None;
                }
                OnDemandShape::Shear(ShearSpec { axis, anti, num })
            }
        }
    };

    Some(OnDemandGlyph::Shape(OnDemandBox {
        w,
        h,
        w_frac,
        h_frac,
        scale,
        align_w,
        align_h,
        shape,
        fill,
    }))
}

/// Detect an on-demand color/mono glyph: name X is not defined, but both
/// X:mono and X:color exist.  X itself must not contain `:mono` or `:color`.
pub fn detect_color_mono_glyph(
    name: &str,
    has_glyph: impl Fn(&str) -> bool,
) -> Option<OnDemandGlyph> {
    if name.contains(":mono") || name.contains(":color") {
        return None;
    }
    // One buffer, reused for the second probe and handed over as the answer:
    // this is asked of every name that is not a declared glyph, which for a
    // source with ordered `map` alternatives over a han range is millions of
    // names per build, and two `format!`s per miss was most of what that cost.
    let mut probe = String::with_capacity(name.len() + ":color".len());
    probe.push_str(name);
    probe.push_str(":mono");
    if !has_glyph(&probe) {
        return None;
    }
    let mono = probe.clone();
    probe.truncate(name.len());
    probe.push_str(":color");
    if !has_glyph(&probe) {
        return None;
    }
    Some(OnDemandGlyph::ColorMono { mono, color: probe })
}

pub fn detect_on_demand_glyph(
    name: &str,
    has_glyph: impl Fn(&str) -> bool,
) -> Option<OnDemandGlyph> {
    parse_on_demand_glyph(name).or_else(|| detect_color_mono_glyph(name, has_glyph))
}

// ---------------------------------------------------------------------------
// Bitmap ink flags
// ---------------------------------------------------------------------------

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
    // One lattice for every subcell area below. A cell is either a catalog
    // shape (den 2) or a custom detail, and `PixelGrid::den` is kept as the lcm
    // of the details it holds, so this is divisible by both.
    let Some(lat) = crate::detail::lcm_den(2, grid.den) else {
        return; // unreachable in practice: `set_detail` never stores a den this wide
    };
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
                // Exact covered area of the logical pixel, measured on `lat`
                // and so a plain integer sum: a fully covered pixel is `s²`
                // cells' worth.
                let mut area = 0i64;
                for dr in 0..s {
                    for dc in 0..s {
                        area += grid.region_at(lr * s + dr, lc * s + dc).area_units_on(lat);
                    }
                }
                let full =
                    crate::detail::DetailRegion::area_units_full(lat) * (s as i64) * (s as i64);
                match fill {
                    BitmapFill::Zero => false,
                    BitmapFill::Ceil => area > 0,
                    BitmapFill::Floor => area >= full,
                    BitmapFill::Round => 2 * area >= full,
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

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Vertex lattice of a synthesized curve: `1/POLY_Q` of a grid subcell.
/// A curve is approximated no finer than the lattice its vertices snap to.
const POLY_Q: i64 = 240;

/// Lattice each cell's finished [`crate::detail::DetailRegion`] is written on.
///
/// Coarser than [`POLY_Q`] on purpose, and not freely choosable — a curve is
/// the first thing in this codebase to hand [`crate::detail`] a region that is
/// neither catalog-shaped nor cut on a small lattice, and two of its limits
/// bite at once:
///
/// - everything a region then goes through — `canonical`, the boolean sweeps,
///   `rescale` — is exact rational arithmetic in `i64` whose intermediate
///   denominators grow with the cube of the region's, and a fine lattice with
///   slanted edges overflows them;
/// - `canonical` re-picks the smallest denominator that holds every vertex of
///   its own sweep exactly, and falls back to snapping at
///   [`crate::detail::MAX_DEN`]. A vertex on a cell border must survive that
///   snap unmoved or the two cells sharing the border disagree and the contour
///   tracer sees a break instead of one outline — which is why this divides
///   `MAX_DEN` rather than being merely small.
///
/// The geometry is still *computed* at [`POLY_Q`] and only quantized on the
/// way in, so the cost is one rounding at the last step rather than a coarse
/// construction throughout.
const REGION_DEN: i64 = 51;

/// The default angle: an outer point at the top of the box. Screen y points
/// down, so "up" is a negative angle and a *clockwise* turn on screen is the
/// increasing direction.
const TOP_ANGLE: f64 = -FRAC_PI_2;

/// How many segments approximate an ellipse with semi-axes `ax`, `ay` (in
/// subcells). A chord subtending `theta` misses the curve by about
/// `r·theta²/8`; hold that under one [`POLY_Q`] lattice step.
fn ellipse_segments(ax: f64, ay: f64) -> usize {
    let r = ax.max(ay).max(0.5);
    let n = (TAU * (r * POLY_Q as f64 / 8.0).sqrt()).ceil() as i64;
    // A multiple of 4 puts a vertex on each end of both axes.
    (n.clamp(32, 512) as usize).div_ceil(4) * 4
}

/// Vertices of a curved shape, in increasing angle about the center, as
/// numerators over [`POLY_Q`]. Empty when the shape encloses no area.
fn shape_vertices(shape: &OnDemandShape, cx: i64, cy: i64, ax: f64, ay: f64) -> Vec<(i64, i64)> {
    let q = POLY_Q as f64;
    let point = |theta: f64, rho: f64| -> (i64, i64) {
        (
            cx + (ax * rho * theta.cos() * q).round() as i64,
            cy + (ay * rho * theta.sin() * q).round() as i64,
        )
    };
    match shape {
        OnDemandShape::Circle => {
            let m = ellipse_segments(ax, ay);
            (0..m)
                .map(|i| point(TOP_ANGLE + TAU * i as f64 / m as f64, 1.0))
                .collect()
        }
        OnDemandShape::Poly(spec) => {
            let n = spec.n as usize;
            let base = TOP_ANGLE + TAU * spec.rot_num as f64 / spec.rot_den as f64;
            if spec.inset == PolyInset::None {
                return (0..n)
                    .map(|i| point(base + TAU * i as f64 / n as f64, 1.0))
                    .collect();
            }
            let rho = spec.inner_ratio();
            // `rK` with 2K = N sends every inner point to the center; the
            // spikes have no width and the shape holds no ink at all.
            if rho < 1e-6 {
                return Vec::new();
            }
            (0..2 * n)
                .map(|i| {
                    let theta = base + PI * i as f64 / n as f64;
                    point(theta, if i % 2 == 0 { 1.0 } else { rho })
                })
                .collect()
        }
        OnDemandShape::Rect | OnDemandShape::Tri(_) | OnDemandShape::Shear(_) => Vec::new(),
    }
}

/// Nearest integer to `n / d` (`d > 0`), rounding halves up.
fn div_round(n: i64, d: i64) -> i64 {
    (2 * n + d).div_euclid(2 * d)
}

/// Cut every edge where it crosses a cell border, so that no edge interior
/// ever leaves the cell its endpoints are in.
///
/// This is what lets the per-cell clip below run on plain integers: a clip
/// against a cell border can then only ever land on a vertex that is already
/// there, so it invents no coordinates, and two neighbouring cells cut the
/// same edge at the same point instead of each rounding its own.
fn subdivide_at_cell_borders(pts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(pts.len() * 3);
    for i in 0..pts.len() {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % pts.len()];
        out.push((x1, y1));
        // Each crossing carries where along the edge it sits, as `num/den`
        // with a positive denominator, so the two axes can be merged in order.
        let mut cuts: Vec<((i64, i64), (i64, i64))> = Vec::new();
        if x1 != x2 {
            let (lo, hi) = (x1.min(x2), x1.max(x2));
            for k in lo.div_euclid(POLY_Q) + 1..=(hi - 1).div_euclid(POLY_Q) {
                let x = k * POLY_Q;
                let y = y1 + div_round((y2 - y1) * (x - x1), x2 - x1);
                cuts.push((((x - x1) * (x2 - x1).signum(), (x2 - x1).abs()), (x, y)));
            }
        }
        if y1 != y2 {
            let (lo, hi) = (y1.min(y2), y1.max(y2));
            for k in lo.div_euclid(POLY_Q) + 1..=(hi - 1).div_euclid(POLY_Q) {
                let y = k * POLY_Q;
                let x = x1 + div_round((x2 - x1) * (y - y1), y2 - y1);
                cuts.push((((y - y1) * (y2 - y1).signum(), (y2 - y1).abs()), (x, y)));
            }
        }
        cuts.sort_by(|a, b| (a.0.0 * b.0.1).cmp(&(b.0.0 * a.0.1)));
        out.extend(cuts.into_iter().map(|(_, p)| p));
    }
    out.dedup();
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// One Sutherland–Hodgman pass against a half-plane of the cell box.
///
/// After [`subdivide_at_cell_borders`] no edge crosses `bound` other than at
/// an endpoint, so the "intersection" is that endpoint; the interpolation is
/// only a guard for the rounding slop of a crossing that landed a lattice step
/// off its border, and it reads the same from either side of the border.
fn clip_half_plane(
    ring: &[(i64, i64)],
    vertical: bool,
    bound: i64,
    keep_ge: bool,
) -> Vec<(i64, i64)> {
    let coord = |p: (i64, i64)| if vertical { p.0 } else { p.1 };
    let inside = |p: (i64, i64)| {
        if keep_ge {
            coord(p) >= bound
        } else {
            coord(p) <= bound
        }
    };
    let cut = |a: (i64, i64), b: (i64, i64)| -> (i64, i64) {
        if coord(a) == bound {
            return a;
        }
        if coord(b) == bound {
            return b;
        }
        let t = bound - coord(a);
        let span = coord(b) - coord(a);
        if vertical {
            (bound, a.1 + div_round((b.1 - a.1) * t, span))
        } else {
            (a.0 + div_round((b.0 - a.0) * t, span), bound)
        }
    };
    let n = ring.len();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(n + 4);
    for i in 0..n {
        let (cur, next) = (ring[i], ring[(i + 1) % n]);
        if inside(cur) {
            out.push(cur);
        }
        if inside(cur) != inside(next) {
            out.push(cut(cur, next));
        }
    }
    out.dedup();
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// The part of `ring` inside cell `(row, col)`, as a region on the [`POLY_Q`]
/// lattice in cell-local coordinates.
fn clip_ring_to_cell(ring: &[(i64, i64)], row: u16, col: u16) -> crate::detail::DetailRegion {
    let x0 = col as i64 * POLY_Q;
    let y0 = row as i64 * POLY_Q;
    let mut cur = clip_half_plane(ring, true, x0, true);
    for (vertical, bound, keep_ge) in [
        (true, x0 + POLY_Q, false),
        (false, y0, true),
        (false, y0 + POLY_Q, false),
    ] {
        if cur.len() < 3 {
            return crate::detail::DetailRegion::EMPTY;
        }
        cur = clip_half_plane(&cur, vertical, bound, keep_ge);
    }
    if cur.len() < 3 {
        return crate::detail::DetailRegion::EMPTY;
    }
    let ring: Vec<(u8, u8)> = drop_collinear(
        cur.into_iter()
            .map(|(x, y)| {
                (
                    div_round((x - x0) * REGION_DEN, POLY_Q).clamp(0, REGION_DEN) as u8,
                    div_round((y - y0) * REGION_DEN, POLY_Q).clamp(0, REGION_DEN) as u8,
                )
            })
            .collect(),
    );
    if ring.len() < 3 {
        return crate::detail::DetailRegion::EMPTY;
    }
    crate::detail::DetailRegion {
        den: REGION_DEN as u8,
        rings: vec![ring],
    }
}

/// Drop repeated and exactly collinear vertices. Quantizing onto
/// [`REGION_DEN`] flattens a run of arc vertices into a straight line often
/// enough that this is worth doing before the region reaches `canonical`.
fn drop_collinear(ring: Vec<(u8, u8)>) -> Vec<(u8, u8)> {
    let mut ring: Vec<(u8, u8)> = ring;
    ring.dedup();
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = ring[(i + n - 1) % n];
        let q = ring[i];
        let r = ring[(i + 1) % n];
        let cross = (q.0 as i32 - p.0 as i32) * (r.1 as i32 - p.1 as i32)
            - (q.1 as i32 - p.1 as i32) * (r.0 as i32 - p.0 as i32);
        if cross != 0 {
            out.push(q);
        }
    }
    out
}

/// Even-odd point-in-polygon test in exact integers.
fn point_in_polygon(pts: &[(i64, i64)], px: i64, py: i64) -> bool {
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % n];
        if (y1 > py) != (y2 > py) {
            let side = (px - x1) * (y2 - y1) - (x2 - x1) * (py - y1);
            if (if y2 > y1 { side } else { -side }) < 0 {
                inside = !inside;
            }
        }
    }
    inside
}

/// Ink a curved shape into the grid, cell by cell: whole cells by a
/// point-in-polygon test, and the cells the outline passes through by clipping
/// the outline against the cell.
fn draw_curved_shape(
    grid: &mut PixelGrid,
    shape: &OnDemandShape,
    off_r: u16,
    off_c: u16,
    rect_w: u16,
    rect_h: u16,
) {
    // POLY_Q is even, so a box of a whole number of subcells has its center on
    // the lattice exactly.
    let cx = off_c as i64 * POLY_Q + rect_w as i64 * POLY_Q / 2;
    let cy = off_r as i64 * POLY_Q + rect_h as i64 * POLY_Q / 2;
    let pts = shape_vertices(shape, cx, cy, rect_w as f64 / 2.0, rect_h as f64 / 2.0);
    if pts.len() < 3 {
        return;
    }
    let ring = subdivide_at_cell_borders(&pts);
    if ring.len() < 3 {
        return;
    }

    // Cells any edge can reach, found through each edge's bounding box. Over-
    // marking only costs a clip that would have come out whole or empty.
    let width = grid.width as usize;
    let mut on_outline = vec![false; width * grid.height as usize];
    let cell_of = |v: i64, limit: u16| v.div_euclid(POLY_Q).clamp(0, limit as i64 - 1);
    for i in 0..ring.len() {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % ring.len()];
        for r in cell_of(y1.min(y2), grid.height)..=cell_of(y1.max(y2), grid.height) {
            for c in cell_of(x1.min(x2), grid.width)..=cell_of(x1.max(x2), grid.width) {
                on_outline[r as usize * width + c as usize] = true;
            }
        }
    }

    for row in off_r..off_r + rect_h {
        for col in off_c..off_c + rect_w {
            if on_outline[row as usize * width + col as usize] {
                grid.set_detail(row, col, &clip_ring_to_cell(&ring, row, col), true);
            } else {
                let px = col as i64 * POLY_Q + POLY_Q / 2;
                let py = row as i64 * POLY_Q + POLY_Q / 2;
                if point_in_polygon(&ring, px, py) {
                    grid.set(row, col, PixelShape::new(PX_ALMOSTFULL, true));
                }
            }
        }
    }
}

/// Ink the parallelogram of a shear into the grid.
///
/// The box's rectangle is shortened on the sheared axis by the displacement
/// and its two remaining edges are slid apart by it, so the finished shape
/// spans the whole box on both axes — the declared box is the bbox, not the
/// rectangle the shear started from.
///
/// `d` is the displacement in the build's own subcells, which may be finer
/// than the declared ones (see [`center_multiplier`]) but never coarser, so
/// every vertex here is a whole subcell and the geometry is exact — this is a
/// triangle's path, not a curve's.
fn draw_shear(
    grid: &mut PixelGrid,
    spec: &ShearSpec,
    d: i64,
    off_r: u16,
    off_c: u16,
    rect_w: u16,
    rect_h: u16,
) {
    let (x0, y0) = (off_c as i64, off_r as i64);
    let (w, h) = (rect_w as i64, rect_h as i64);
    // Clockwise on screen, like every other ring here.
    let pts: [(i64, i64); 4] = match (spec.axis, spec.anti) {
        // `s`: the top edge sits to the right of the bottom one, so the shape
        // leans the way a slash does.
        (ShearAxis::X, false) => [
            (x0 + d, y0),
            (x0 + w, y0),
            (x0 + w - d, y0 + h),
            (x0, y0 + h),
        ],
        (ShearAxis::X, true) => [
            (x0, y0),
            (x0 + w - d, y0),
            (x0 + w, y0 + h),
            (x0 + d, y0 + h),
        ],
        // `s` again: the left edge sits below the right one.
        (ShearAxis::Y, false) => [
            (x0, y0 + d),
            (x0 + w, y0),
            (x0 + w, y0 + h - d),
            (x0, y0 + h),
        ],
        (ShearAxis::Y, true) => [
            (x0, y0),
            (x0 + w, y0 + d),
            (x0 + w, y0 + h),
            (x0, y0 + h - d),
        ],
    };
    draw_convex_polygon(grid, &pts, off_r, off_c, rect_w, rect_h);
}

/// Ink a right triangle with legs `rect_w × rect_h` into the grid.
fn draw_triangle(
    grid: &mut PixelGrid,
    corner: TriCorner,
    off_r: u16,
    off_c: u16,
    rect_w: u16,
    rect_h: u16,
) {
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
    draw_convex_polygon(grid, &tri, off_r, off_c, rect_w, rect_h);
}

/// Ink a convex polygon whose vertices are whole subcells into the grid, cell
/// by cell: a cell every edge keeps whole is inked as one, a cell some edge
/// drops entirely is skipped, and the rest are cut exactly.
///
/// Straight edges on the grid's own lattice are what separate this from
/// [`draw_curved_shape`]: the clip is exact rational arithmetic
/// ([`clip_polygon_to_cell`]) rather than a polyline quantized onto
/// [`REGION_DEN`], so two cells sharing a border cut a slanted edge at the
/// same point because the arithmetic says so, not because the outline was
/// pre-split. Both the triangles and the shears go through it.
fn draw_convex_polygon(
    grid: &mut PixelGrid,
    pts: &[(i64, i64)],
    off_r: u16,
    off_c: u16,
    rect_w: u16,
    rect_h: u16,
) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    // Which sign of the cross product is the polygon's inside, once for every
    // edge below.
    let turn = |a: (i64, i64), b: (i64, i64), p: (i64, i64)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let sign = (0..n)
        .map(|i| turn((0, 0), pts[i], pts[(i + 1) % n]))
        .sum::<i64>()
        .signum();
    if sign == 0 {
        return;
    }

    for r in off_r..(off_r + rect_h) {
        for c in off_c..(off_c + rect_w) {
            let corners = [
                (c as i64, r as i64),
                (c as i64 + 1, r as i64),
                (c as i64, r as i64 + 1),
                (c as i64 + 1, r as i64 + 1),
            ];
            // Classify the cell's corners against every edge. Convexity is
            // what makes the two verdicts sound: all four corners inside every
            // edge means the whole cell is, and all four outside any one edge
            // means none of it is.
            let mut whole = true;
            let mut empty = false;
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                let outside = corners
                    .iter()
                    .filter(|&&p| turn(a, b, p) * sign < 0)
                    .count();
                if outside == corners.len() {
                    empty = true;
                    break;
                }
                whole &= outside == 0;
            }
            if empty {
                continue;
            }
            if whole {
                grid.set(r, c, PixelShape::new(PX_ALMOSTFULL, true));
                continue;
            }
            let local: Vec<(Frac64, Frac64)> = pts
                .iter()
                .map(|&(x, y)| (Frac64::new(x - c as i64, 1), Frac64::new(y - r as i64, 1)))
                .collect();
            let region = clip_polygon_to_cell(&local);
            // An edge the cell only touches leaves nothing behind.
            if !region.is_empty() {
                grid.set_detail(r, c, &region, true);
            }
        }
    }
}

/// Build the pixel grid of an on-demand shape. The grid is at subpixel
/// resolution `spec.scale` and is always `ceil(W) × ceil(H)` logical pixels;
/// anything but a whole-cell rectangle gets exact per-pixel geometry,
/// re-encoded as plain shape codes wherever possible.
///
/// Geometry is laid down first and the ink flags ([`crate::pixel::PX_FULL`])
/// are decided afterwards by [`apply_bitmap_fill`], so `spec.fill` changes
/// only what the bitmap build draws, never an outline.
///
/// Memoized on the box, which is the whole input: the same handful of names is
/// rebuilt on every font build, and the editor runs two of those per edit — one
/// for the font, one for the derived data — on top of each other.
///
/// A curved shape costs an exact clip per outline cell and was the first to be
/// kept; a whole-cell rectangle costs almost nothing and does not need to be.
/// Everything in between — a rectangle with a fractional edge, a triangle, a
/// shear — is exact geometry like a curve, so the line is drawn at "does this
/// box place anything but whole cells" rather than at the shape.
pub fn make_on_demand_grid(spec: &OnDemandBox) -> PixelGrid {
    let whole_cells = spec.shape == OnDemandShape::Rect
        && spec.w_frac == 0
        && spec.h_frac == 0
        && spec.scale == 1;
    if whole_cells {
        return build_on_demand_grid(spec);
    }
    static CACHE: OnceLock<Mutex<HashMap<OnDemandBox, PixelGrid>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(grid) = cache.lock().unwrap().get(spec) {
        return grid.clone();
    }
    let grid = build_on_demand_grid(spec);
    cache.lock().unwrap().insert(spec.clone(), grid.clone());
    grid
}

/// Where the box starts on an axis whose extent leaves `gap` subcells over.
///
/// [`BoxAlign::Center`] halves the gap exactly. An odd `gap` therefore starts
/// the box mid-subcell, which the declared `1/R` lattice cannot express — see
/// [`center_multiplier`] for the finer lattice the build then runs on.
fn align_offset(align: BoxAlign, gap: u16) -> u16 {
    match align {
        BoxAlign::Near => 0,
        BoxAlign::Far => gap,
        BoxAlign::Center => gap / 2,
    }
}

/// Widest grid, in subcells, the doubled-lattice centering below will build.
/// Well under the sweep's coordinate budget, and far past any real name.
const MAX_FINE_EXTENT: u16 = 4096;

/// How many fine subcells the build puts in one declared subcell.
///
/// A centered axis whose leftover gap is an odd number of subcells wants the
/// box half a subcell in, which the declared `1/R` lattice cannot express. So
/// the whole shape is drawn on a `1/2R` lattice instead and folded back down
/// by [`PixelGrid::rescale`], which turns the half-subcell edges into
/// sub-pixel geometry — the same thing the file format stores for any other
/// off-lattice edge. Anything else builds on the declared lattice as written;
/// so does a name too large for the doubled one to stay within
/// [`MAX_FINE_EXTENT`] and a `u8` scale, which then rounds as it always did.
fn center_multiplier(spec: &OnDemandBox, s: u16, gap_w: u16, gap_h: u16) -> u16 {
    let odd = |align, gap: u16| align == BoxAlign::Center && gap % 2 == 1;
    if !odd(spec.align_w, gap_w) && !odd(spec.align_h, gap_h) {
        return 1;
    }
    let extent_w = (gap_w + spec.w as u16 * s + spec.w_frac as u16) / s;
    let extent_h = (gap_h + spec.h as u16 * s + spec.h_frac as u16) / s;
    let fits =
        2 * s <= u8::MAX as u16 && extent_w.max(extent_h).saturating_mul(2 * s) <= MAX_FINE_EXTENT;
    if fits { 2 } else { 1 }
}

fn build_on_demand_grid(spec: &OnDemandBox) -> PixelGrid {
    let s = spec.scale.max(1) as u16;
    let extent_w = (spec.w as u16 * s + spec.w_frac as u16).div_ceil(s);
    let extent_h = (spec.h as u16 * s + spec.h_frac as u16).div_ceil(s);
    let m = center_multiplier(
        spec,
        s,
        extent_w * s - (spec.w as u16 * s + spec.w_frac as u16),
        extent_h * s - (spec.h as u16 * s + spec.h_frac as u16),
    );
    // Everything from here on is in fine subcells, `m` of them per declared one.
    let fine = s * m;
    let rect_w = (spec.w as u16 * s + spec.w_frac as u16) * m;
    let rect_h = (spec.h as u16 * s + spec.h_frac as u16) * m;
    let grid_w = extent_w * fine;
    let grid_h = extent_h * fine;
    let off_c = align_offset(spec.align_w, grid_w - rect_w);
    let off_r = align_offset(spec.align_h, grid_h - rect_h);

    let mut grid = PixelGrid::new(grid_w, grid_h);
    match spec.shape {
        OnDemandShape::Rect => {
            for r in off_r..(off_r + rect_h) {
                for c in off_c..(off_c + rect_w) {
                    grid.set(r, c, PixelShape::new(PX_ALMOSTFULL, true));
                }
            }
        }
        OnDemandShape::Tri(corner) => {
            draw_triangle(&mut grid, corner, off_r, off_c, rect_w, rect_h)
        }
        OnDemandShape::Shear(shear) => {
            // The amount is written on the declared lattice; the build may be
            // running on a finer one.
            draw_shear(
                &mut grid,
                &shear,
                shear.num as i64 * m as i64,
                off_r,
                off_c,
                rect_w,
                rect_h,
            )
        }
        OnDemandShape::Circle | OnDemandShape::Poly(_) => {
            draw_curved_shape(&mut grid, &spec.shape, off_r, off_c, rect_w, rect_h)
        }
    }
    if m != 1 {
        grid = grid.rescale(fine as u8, s as u8);
    }
    apply_bitmap_fill(&mut grid, s, spec.fill);
    grid
}

#[cfg(test)]
#[path = "on_demand_tests.rs"]
mod tests;
