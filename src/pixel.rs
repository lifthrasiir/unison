//! The pixel model: [`PixelShape`] shape codes, the `PX_*` catalog below, and
//! the boolean operations and rescaling over a [`crate::document::PixelGrid`].
//!
//! A cell is one shape code plus an ink flag. The catalog covers the sub-pixel
//! shapes a glyph can be drawn with — `PX_HALF*`, `PX_QUAD*`, `PX_SLANT*`,
//! `PX_CONE*`, `PX_CORNER*`, `PX_HQUAD`/`PX_VQUAD`, `PX_DOT`, … — and each has
//! its complement one `^ PX_SUBPIXEL` away, which is what lets a serialized
//! character pair name either side of a diagonal.
//!
//! [`PX_HARDBLANK`] is the one code that is neither ink nor the empty cell: it
//! draws nothing, but a document that writes it (`$$`) gets it back.
//!
//! [`PX_CUSTOM`] is the one code with no fixed geometry: it is a sentinel
//! meaning "this cell's geometry is a [`crate::detail::DetailRegion`] in the
//! grid's detail table". It appears only in *derived* grids — composition,
//! rescaling, on-demand synthesis, the anchor shadow — and is **never
//! serialized**, so anything writing a document has to have re-encoded it as a
//! catalog shape (or lost nothing by dropping it).

use std::fmt;

pub const PX_SUBPIXEL: u8 = 0x7f;
pub const PX_FULL: u8 = 0x80;

pub const PX_EMPTY: u8 = 0;
pub const PX_ALMOSTFULL: u8 = PX_SUBPIXEL;
pub const PX_HALF1: u8 = 1; //               |\ (b filled, \ unfilled)
pub const PX_HALF2: u8 = 1 ^ PX_SUBPIXEL; // \| (9 filled, \ unfilled)
pub const PX_HALF3: u8 = 2; //               |/ (P filled, / unfilled)
pub const PX_HALF4: u8 = 2 ^ PX_SUBPIXEL; // /| (d filled, / unfilled)
pub const PX_QUAD1: u8 = 3; //               |> () filled, > unfilled)
pub const PX_QUAD2: u8 = 4; //               v  (u filled, v unfilled)
pub const PX_QUAD3: u8 = 5; //               <| (( filled, < unfilled)
pub const PX_QUAD4: u8 = 6; //               ^  (n filled, ^ unfilled)
pub const PX_INVQUAD1: u8 = 3 ^ PX_SUBPIXEL;
pub const PX_INVQUAD2: u8 = 4 ^ PX_SUBPIXEL;
pub const PX_INVQUAD3: u8 = 5 ^ PX_SUBPIXEL;
pub const PX_INVQUAD4: u8 = 6 ^ PX_SUBPIXEL;
pub const PX_SLANT1H: u8 = 7;
pub const PX_SLANT2H: u8 = 8;
pub const PX_SLANT3H: u8 = 9;
pub const PX_SLANT4H: u8 = 10;
pub const PX_SLANT1V: u8 = 11;
pub const PX_SLANT2V: u8 = 12;
pub const PX_SLANT3V: u8 = 13;
pub const PX_SLANT4V: u8 = 14;
pub const PX_HALFSLANT1H: u8 = 8 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT2H: u8 = 7 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT3H: u8 = 10 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT4H: u8 = 9 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT1V: u8 = 12 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT2V: u8 = 11 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT3V: u8 = 14 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT4V: u8 = 13 ^ PX_SUBPIXEL;
pub const PX_DOT: u8 = 15;
pub const PX_CONE1: u8 = 16;
pub const PX_CONE2: u8 = 17;
pub const PX_CONE3: u8 = 18;
pub const PX_CONE4: u8 = 19;
pub const PX_INVCONE1: u8 = 16 ^ PX_SUBPIXEL;
pub const PX_INVCONE2: u8 = 17 ^ PX_SUBPIXEL;
pub const PX_INVCONE3: u8 = 18 ^ PX_SUBPIXEL;
pub const PX_INVCONE4: u8 = 19 ^ PX_SUBPIXEL;
pub const PX_HQUAD: u8 = 20; //              >< (left+right quads bowtie)
pub const PX_VQUAD: u8 = 20 ^ PX_SUBPIXEL; // top+bottom quads bowtie
pub const PX_CORNER1: u8 = 21; //            BL corner triangle
pub const PX_CORNER2: u8 = 22; //            TR corner triangle
pub const PX_CORNER3: u8 = 23; //            TL corner triangle
pub const PX_CORNER4: u8 = 24; //            BR corner triangle
pub const PX_INVCORNER1: u8 = 21 ^ PX_SUBPIXEL;
pub const PX_INVCORNER2: u8 = 22 ^ PX_SUBPIXEL;
pub const PX_INVCORNER3: u8 = 23 ^ PX_SUBPIXEL;
pub const PX_INVCORNER4: u8 = 24 ^ PX_SUBPIXEL;
// `PX_DOT` plus two corner triangles: the diamond and the four corners tile
// the cell exactly, so each of these is "everything but two corners", and its
// complement is exactly the two corners that were left out. Two opposite
// corners read as a thick diagonal stroke (`SLASH`/`BACKSLASH`); two adjacent
// ones as a pentagon with a flat side and an apex opposite it (`HOUSE*`,
// numbered by the apex direction: 1 right, 2 down, 3 left, 4 up, the same
// convention as `PX_CONE*`).
pub const PX_SLASH: u8 = 25; //              DOT + CORNER1 (BL) + CORNER2 (TR)
pub const PX_BACKSLASH: u8 = 26; //          DOT + CORNER3 (TL) + CORNER4 (BR)
pub const PX_HOUSE1: u8 = 27; //             DOT + CORNER3 (TL) + CORNER1 (BL)
pub const PX_HOUSE2: u8 = 28; //             DOT + CORNER3 (TL) + CORNER2 (TR)
pub const PX_HOUSE3: u8 = 29; //             DOT + CORNER2 (TR) + CORNER4 (BR)
pub const PX_HOUSE4: u8 = 30; //             DOT + CORNER1 (BL) + CORNER4 (BR)
pub const PX_INVSLASH: u8 = 25 ^ PX_SUBPIXEL; //     CORNER3 + CORNER4
pub const PX_INVBACKSLASH: u8 = 26 ^ PX_SUBPIXEL; // CORNER1 + CORNER2
pub const PX_INVHOUSE1: u8 = 27 ^ PX_SUBPIXEL; //    CORNER2 + CORNER4
pub const PX_INVHOUSE2: u8 = 28 ^ PX_SUBPIXEL; //    CORNER1 + CORNER4
pub const PX_INVHOUSE3: u8 = 29 ^ PX_SUBPIXEL; //    CORNER3 + CORNER1
pub const PX_INVHOUSE4: u8 = 30 ^ PX_SUBPIXEL; //    CORNER3 + CORNER2
/// Sentinel id: the pixel's geometry is a custom [`crate::detail::DetailRegion`]
/// stored in the owning grid's detail table. Only ever appears in derived
/// (resolved/composited) grids, never in document source grids, and is
/// never serialized. It is kept last in the *catalog* range so the catalog can
/// grow below it (ids above it are not catalog shapes — see [`PX_HARDBLANK`]);
/// its complement id (96) is intentionally left unused, because negation
/// involving custom pixels is resolved eagerly into a new region.
pub const PX_CUSTOM: u8 = 31;

/// A *hardblank*: written `$$`, it draws exactly the nothing [`PX_EMPTY`] draws
/// and is kept apart from it only so a source can mark a blank as deliberate —
/// a cell kerning and the like may one day read. Nothing in the build pipeline
/// treats it as ink.
///
/// It sits *outside* the catalog band (`0..=30` and their complements
/// `97..=127`), so every geometry table — rasters, adjacency, edge coverage,
/// [`crate::detail::DetailRegion::from_shape`] — gives it the empty entry an
/// unassigned id gets, and nothing had to grow a case for it.
///
/// It is a shape id rather than the otherwise unused `PX_EMPTY | PX_FULL`
/// because that combination is *not* unused: `BitmapFill` writes it for a
/// subcell with no geometry inside a logical pixel the bitmap face inks (see
/// [`crate::on_demand`]). Being an id of its own, though, means it must never
/// be inverted into its unused complement (95) or its unwritable filled form —
/// so [`PixelShape::opposite`], [`PixelShape::opposite_bitmap`] and
/// [`PixelShape::with_fill_toggled`] all map a hardblank to itself. A blank has
/// no other side. Mirrors and rotations already do, through
/// [`transform_shape`]'s identity fallback for non-catalog ids.
///
/// The two predicates carry the distinction: the fill bit is unset, so
/// [`PixelShape::is_filled`] is `false` and nothing renders, rasterizes or
/// traces it; but [`PixelShape::is_empty`] is `false` too, so the cell counts as
/// *occupied* — the editor draws it and the serializer writes it back out.
pub const PX_HARDBLANK: u8 = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PixelShape(pub u8);

impl PixelShape {
    pub const EMPTY: Self = Self(PX_EMPTY);

    pub fn new(shape_id: u8, filled: bool) -> Self {
        debug_assert!(shape_id < 128);
        Self(shape_id | if filled { PX_FULL } else { 0 })
    }

    pub fn shape_id(self) -> u8 {
        self.0 & PX_SUBPIXEL
    }

    pub fn is_filled(self) -> bool {
        self.0 & PX_FULL != 0
    }

    /// Is this cell unwritten? A hardblank is not: it draws the same nothing,
    /// but it is something the source says.
    pub fn is_empty(self) -> bool {
        self.shape_id() == PX_EMPTY && !self.is_filled()
    }

    pub fn is_hardblank(self) -> bool {
        self.shape_id() == PX_HARDBLANK
    }

    /// Nothing is drawn here: an empty cell, or a hardblank, which is the same
    /// nothing under a name.
    pub fn is_blank(self) -> bool {
        self.is_empty() || self.is_hardblank()
    }

    /// The shape id this cell contributes to an outline — a hardblank
    /// contributes none, so it reads as [`PX_EMPTY`] to everything tracing ink.
    pub fn ink_shape_id(self) -> u8 {
        if self.is_hardblank() {
            PX_EMPTY
        } else {
            self.shape_id()
        }
    }

    #[cfg(feature = "editor")]
    pub fn with_fill_toggled(self) -> Self {
        if self.is_hardblank() {
            return self; // a blank has no filled form
        }
        Self(self.0 ^ PX_FULL)
    }

    #[cfg(feature = "editor")]
    pub fn is_slant_pair(self) -> bool {
        let id = self.shape_id();
        let base = id.min(id ^ PX_SUBPIXEL);
        (7..=14).contains(&base)
    }

    #[cfg(feature = "editor")]
    pub fn slant_direction_pair(self) -> Self {
        let id = self.shape_id();
        let pair_id = match id {
            PX_HALFSLANT1H => PX_SLANT1H,
            PX_HALFSLANT2H => PX_SLANT2H,
            PX_HALFSLANT3H => PX_SLANT3H,
            PX_HALFSLANT4H => PX_SLANT4H,
            PX_HALFSLANT1V => PX_SLANT1V,
            PX_HALFSLANT2V => PX_SLANT2V,
            PX_HALFSLANT3V => PX_SLANT3V,
            PX_HALFSLANT4V => PX_SLANT4V,
            PX_SLANT1H => PX_HALFSLANT1H,
            PX_SLANT2H => PX_HALFSLANT2H,
            PX_SLANT3H => PX_HALFSLANT3H,
            PX_SLANT4H => PX_HALFSLANT4H,
            PX_SLANT1V => PX_HALFSLANT1V,
            PX_SLANT2V => PX_HALFSLANT2V,
            PX_SLANT3V => PX_HALFSLANT3V,
            PX_SLANT4V => PX_HALFSLANT4V,
            _ => return self,
        };
        Self::new(pair_id, !self.is_filled())
    }

    #[cfg(any(feature = "editor", test))]
    pub fn mirror_h(self) -> Self {
        Self(transform_shape(self.0, MIRROR_H_TABLE))
    }

    #[cfg(any(feature = "editor", test))]
    pub fn flip_v(self) -> Self {
        Self(transform_shape(self.0, FLIP_V_TABLE))
    }

    #[cfg(any(feature = "editor", test))]
    pub fn rotate_cw(self) -> Self {
        Self(transform_shape(self.0, ROTATE_CW_TABLE))
    }

    #[cfg(any(feature = "editor", test))]
    pub fn rotate_ccw(self) -> Self {
        Self(transform_shape(self.0, ROTATE_CCW_TABLE))
    }

    #[cfg(any(feature = "editor", test))]
    pub fn rotate_180(self) -> Self {
        Self(transform_shape(self.0, ROTATE_180_TABLE))
    }

    #[cfg(feature = "editor")]
    pub fn opposite(self) -> Self {
        if self.is_hardblank() {
            return self; // inverting the bits would land on the unused id 95
        }
        Self(!self.0)
    }

    #[cfg(feature = "editor")]
    pub fn opposite_bitmap(self) -> Self {
        if self.shape_id() == PX_EMPTY || self.is_hardblank() {
            self
        } else {
            self.with_fill_toggled()
        }
    }
}

// Transform lookup tables: map shape_id → transformed shape_id for base shapes (0-31).
// Complement shapes (id ≥ 96) use: transform(id ^ 127) ^ 127.
// The fill bit (PX_FULL) passes through unchanged. Index 31 is PX_CUSTOM,
// which has no catalog geometry and maps to itself.
#[rustfmt::skip]
#[cfg(any(feature = "editor", test))]
const MIRROR_H_TABLE: [u8; 32] = [
    0,   // EMPTY → EMPTY
    125, // HALF1 (\ BL) → HALF4 (/ BR)
    126, // HALF3 (/ TL) → HALF2 (\ TR)
    5,   // QUAD1 (left) → QUAD3 (right)
    4,   // QUAD2 (top) → QUAD2 (top)
    3,   // QUAD3 (right) → QUAD1 (left)
    6,   // QUAD4 (bottom) → QUAD4 (bottom)
    10,  // SLANT1H → SLANT4H
    9,   // SLANT2H → SLANT3H
    8,   // SLANT3H → SLANT2H
    7,   // SLANT4H → SLANT1H
    14,  // SLANT1V → SLANT4V
    13,  // SLANT2V → SLANT3V
    12,  // SLANT3V → SLANT2V
    11,  // SLANT4V → SLANT1V
    15,  // DOT → DOT
    18,  // CONE1 (left) → CONE3 (right)
    17,  // CONE2 (top) → CONE2 (top)
    16,  // CONE3 (right) → CONE1 (left)
    19,  // CONE4 (bottom) → CONE4 (bottom)
    20,  // HQUAD → HQUAD (symmetric)
    24,  // CORNER1 (BL) → CORNER4 (BR)
    23,  // CORNER2 (TR) → CORNER3 (TL)
    22,  // CORNER3 (TL) → CORNER2 (TR)
    21,  // CORNER4 (BR) → CORNER1 (BL)
    26,  // SLASH → BACKSLASH
    25,  // BACKSLASH → SLASH
    29,  // HOUSE1 (right) → HOUSE3 (left)
    28,  // HOUSE2 (down) → HOUSE2 (down)
    27,  // HOUSE3 (left) → HOUSE1 (right)
    30,  // HOUSE4 (up) → HOUSE4 (up)
    31,  // CUSTOM → CUSTOM
];

#[rustfmt::skip]
#[cfg(any(feature = "editor", test))]
const FLIP_V_TABLE: [u8; 32] = [
    0,   // EMPTY → EMPTY
    2,   // HALF1 (\ BL) → HALF3 (/ TL)
    1,   // HALF3 (/ TL) → HALF1 (\ BL)
    3,   // QUAD1 (left) → QUAD1 (left)
    6,   // QUAD2 (top) → QUAD4 (bottom)
    5,   // QUAD3 (right) → QUAD3 (right)
    4,   // QUAD4 (bottom) → QUAD2 (top)
    9,   // SLANT1H → SLANT3H
    10,  // SLANT2H → SLANT4H
    7,   // SLANT3H → SLANT1H
    8,   // SLANT4H → SLANT2H
    13,  // SLANT1V → SLANT3V
    14,  // SLANT2V → SLANT4V
    11,  // SLANT3V → SLANT1V
    12,  // SLANT4V → SLANT2V
    15,  // DOT → DOT
    16,  // CONE1 (left) → CONE1 (left)
    19,  // CONE2 (top) → CONE4 (bottom)
    18,  // CONE3 (right) → CONE3 (right)
    17,  // CONE4 (bottom) → CONE2 (top)
    20,  // HQUAD → HQUAD (symmetric)
    23,  // CORNER1 (BL) → CORNER3 (TL)
    24,  // CORNER2 (TR) → CORNER4 (BR)
    21,  // CORNER3 (TL) → CORNER1 (BL)
    22,  // CORNER4 (BR) → CORNER2 (TR)
    26,  // SLASH → BACKSLASH
    25,  // BACKSLASH → SLASH
    27,  // HOUSE1 (right) → HOUSE1 (right)
    30,  // HOUSE2 (down) → HOUSE4 (up)
    29,  // HOUSE3 (left) → HOUSE3 (left)
    28,  // HOUSE4 (up) → HOUSE2 (down)
    31,  // CUSTOM → CUSTOM
];

#[rustfmt::skip]
#[cfg(any(feature = "editor", test))]
const ROTATE_CW_TABLE: [u8; 32] = [
    0,   // EMPTY → EMPTY
    2,   // HALF1 (\ BL) → HALF3 (/ TL)
    126, // HALF3 (/ TL) → HALF2 (\ TR)
    4,   // QUAD1 (left) → QUAD2 (top)
    5,   // QUAD2 (top) → QUAD3 (right)
    6,   // QUAD3 (right) → QUAD4 (bottom)
    3,   // QUAD4 (bottom) → QUAD1 (left)
    13,  // SLANT1H → SLANT3V
    14,  // SLANT2H → SLANT4V
    12,  // SLANT3H → SLANT2V
    11,  // SLANT4H → SLANT1V
    9,   // SLANT1V → SLANT3H
    10,  // SLANT2V → SLANT4H
    8,   // SLANT3V → SLANT2H
    7,   // SLANT4V → SLANT1H
    15,  // DOT → DOT
    17,  // CONE1 (left) → CONE2 (top)
    18,  // CONE2 (top) → CONE3 (right)
    19,  // CONE3 (right) → CONE4 (bottom)
    16,  // CONE4 (bottom) → CONE1 (left)
    107, // HQUAD → VQUAD
    23,  // CORNER1 (BL) → CORNER3 (TL)
    24,  // CORNER2 (TR) → CORNER4 (BR)
    22,  // CORNER3 (TL) → CORNER2 (TR)
    21,  // CORNER4 (BR) → CORNER1 (BL)
    26,  // SLASH → BACKSLASH
    25,  // BACKSLASH → SLASH
    28,  // HOUSE1 (right) → HOUSE2 (down)
    29,  // HOUSE2 (down) → HOUSE3 (left)
    30,  // HOUSE3 (left) → HOUSE4 (up)
    27,  // HOUSE4 (up) → HOUSE1 (right)
    31,  // CUSTOM → CUSTOM
];

#[rustfmt::skip]
#[cfg(any(feature = "editor", test))]
const ROTATE_CCW_TABLE: [u8; 32] = [
    0,   // EMPTY → EMPTY
    125, // HALF1 (\ BL) → HALF4 (/ BR)
    1,   // HALF3 (/ TL) → HALF1 (\ BL)
    6,   // QUAD1 (left) → QUAD4 (bottom)
    3,   // QUAD2 (top) → QUAD1 (left)
    4,   // QUAD3 (right) → QUAD2 (top)
    5,   // QUAD4 (bottom) → QUAD3 (right)
    14,  // SLANT1H → SLANT4V
    13,  // SLANT2H → SLANT3V
    11,  // SLANT3H → SLANT1V
    12,  // SLANT4H → SLANT2V
    10,  // SLANT1V → SLANT4H
    9,   // SLANT2V → SLANT3H
    7,   // SLANT3V → SLANT1H
    8,   // SLANT4V → SLANT2H
    15,  // DOT → DOT
    19,  // CONE1 (left) → CONE4 (bottom)
    16,  // CONE2 (top) → CONE1 (left)
    17,  // CONE3 (right) → CONE2 (top)
    18,  // CONE4 (bottom) → CONE3 (right)
    107, // HQUAD → VQUAD
    24,  // CORNER1 (BL) → CORNER4 (BR)
    23,  // CORNER2 (TR) → CORNER3 (TL)
    21,  // CORNER3 (TL) → CORNER1 (BL)
    22,  // CORNER4 (BR) → CORNER2 (TR)
    26,  // SLASH → BACKSLASH
    25,  // BACKSLASH → SLASH
    30,  // HOUSE1 (right) → HOUSE4 (up)
    27,  // HOUSE2 (down) → HOUSE1 (right)
    28,  // HOUSE3 (left) → HOUSE2 (down)
    29,  // HOUSE4 (up) → HOUSE3 (left)
    31,  // CUSTOM → CUSTOM
];

#[rustfmt::skip]
#[cfg(any(feature = "editor", test))]
const ROTATE_180_TABLE: [u8; 32] = [
    0,   // EMPTY → EMPTY
    126, // HALF1 (\ BL) → HALF2 (\ TR)
    125, // HALF3 (/ TL) → HALF4 (/ BR)
    5,   // QUAD1 (left) → QUAD3 (right)
    6,   // QUAD2 (top) → QUAD4 (bottom)
    3,   // QUAD3 (right) → QUAD1 (left)
    4,   // QUAD4 (bottom) → QUAD2 (top)
    8,   // SLANT1H → SLANT2H
    7,   // SLANT2H → SLANT1H
    10,  // SLANT3H → SLANT4H
    9,   // SLANT4H → SLANT3H
    12,  // SLANT1V → SLANT2V
    11,  // SLANT2V → SLANT1V
    14,  // SLANT3V → SLANT4V
    13,  // SLANT4V → SLANT3V
    15,  // DOT → DOT
    18,  // CONE1 (left) → CONE3 (right)
    19,  // CONE2 (top) → CONE4 (bottom)
    16,  // CONE3 (right) → CONE1 (left)
    17,  // CONE4 (bottom) → CONE2 (top)
    20,  // HQUAD → HQUAD (symmetric)
    22,  // CORNER1 (BL) → CORNER2 (TR)
    21,  // CORNER2 (TR) → CORNER1 (BL)
    24,  // CORNER3 (TL) → CORNER4 (BR)
    23,  // CORNER4 (BR) → CORNER3 (TL)
    25,  // SLASH → SLASH
    26,  // BACKSLASH → BACKSLASH
    29,  // HOUSE1 (right) → HOUSE3 (left)
    30,  // HOUSE2 (down) → HOUSE4 (up)
    27,  // HOUSE3 (left) → HOUSE1 (right)
    28,  // HOUSE4 (up) → HOUSE2 (down)
    31,  // CUSTOM → CUSTOM
];

#[cfg(any(feature = "editor", test))]
fn transform_shape(raw: u8, table: [u8; 32]) -> u8 {
    let fill = raw & PX_FULL;
    let id = raw & PX_SUBPIXEL;
    let new_id = if id <= 31 {
        table[id as usize]
    } else if id == PX_ALMOSTFULL {
        PX_ALMOSTFULL
    } else if id >= 96 {
        let base = id ^ PX_SUBPIXEL;
        if base <= 31 {
            table[base as usize] ^ PX_SUBPIXEL
        } else {
            id
        }
    } else {
        id
    };
    new_id | fill
}

impl fmt::Debug for PixelShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PixelShape({}, filled={})",
            self.shape_id(),
            self.is_filled()
        )
    }
}

type Seg = (f32, f32, f32, f32);

const ADJACENCY_MAP: &[(u8, u8, &[Seg])] = &[
    //    a   b
    //   +--+--+
    // h |     | c
    //   +     +
    // g |     | d
    //   +--+--+
    //    f   e     abcdefgh
    (PX_EMPTY, 0b00000000, &[]),
    (PX_HALF1, 0b00001111, &[(0.0, 0.0, 1.0, 1.0)]),
    (PX_HALF3, 0b11000011, &[(0.0, 1.0, 1.0, 0.0)]),
    (
        PX_QUAD1,
        0b00000011,
        &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 0.0, 1.0)],
    ),
    (
        PX_QUAD2,
        0b11000000,
        &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 1.0, 0.0)],
    ),
    (
        PX_QUAD3,
        0b00110000,
        &[(1.0, 0.0, 0.5, 0.5), (0.5, 0.5, 1.0, 1.0)],
    ),
    (
        PX_QUAD4,
        0b00001100,
        &[(0.0, 1.0, 0.5, 0.5), (0.5, 0.5, 1.0, 1.0)],
    ),
    (PX_SLANT1H, 0b00000111, &[(0.0, 0.0, 0.5, 1.0)]),
    (PX_SLANT2H, 0b01110000, &[(0.5, 0.0, 1.0, 1.0)]),
    (PX_SLANT3H, 0b10000011, &[(0.0, 1.0, 0.5, 0.0)]),
    (PX_SLANT4H, 0b00111000, &[(0.5, 1.0, 1.0, 0.0)]),
    (PX_SLANT1V, 0b00001110, &[(0.0, 0.5, 1.0, 1.0)]),
    (PX_SLANT2V, 0b11100000, &[(0.0, 0.0, 1.0, 0.5)]),
    (PX_SLANT3V, 0b11000001, &[(0.0, 0.5, 1.0, 0.0)]),
    (PX_SLANT4V, 0b00011100, &[(0.0, 1.0, 1.0, 0.5)]),
    (
        PX_DOT,
        0b00000000,
        &[
            (0.0, 0.5, 0.5, 0.0),
            (0.5, 0.0, 1.0, 0.5),
            (1.0, 0.5, 0.5, 1.0),
            (0.5, 1.0, 0.0, 0.5),
        ],
    ),
    (
        PX_CONE1,
        0b00000011,
        &[(0.0, 0.0, 1.0, 0.5), (1.0, 0.5, 0.0, 1.0)],
    ),
    (
        PX_CONE2,
        0b11000000,
        &[(0.0, 0.0, 0.5, 1.0), (0.5, 1.0, 1.0, 0.0)],
    ),
    (
        PX_CONE3,
        0b00110000,
        &[(1.0, 0.0, 0.0, 0.5), (0.0, 0.5, 1.0, 1.0)],
    ),
    (
        PX_CONE4,
        0b00001100,
        &[(0.0, 1.0, 0.5, 0.0), (0.5, 0.0, 1.0, 1.0)],
    ),
    (
        PX_HQUAD,
        0b00110011,
        &[
            (0.0, 0.0, 0.5, 0.5),
            (0.5, 0.5, 0.0, 1.0),
            (1.0, 0.0, 0.5, 0.5),
            (0.5, 0.5, 1.0, 1.0),
        ],
    ),
    (PX_CORNER1, 0b00000110, &[(0.0, 0.5, 0.5, 1.0)]),
    (PX_CORNER2, 0b01100000, &[(1.0, 0.5, 0.5, 0.0)]),
    (PX_CORNER3, 0b10000001, &[(0.5, 0.0, 0.0, 0.5)]),
    (PX_CORNER4, 0b00011000, &[(0.5, 1.0, 1.0, 0.5)]),
    // DOT + two corners: the two diamond edges facing the corners that were
    // left out are the only boundary away from the cell edges.
    (
        PX_SLASH,
        0b01100110,
        &[(1.0, 0.5, 0.5, 1.0), (0.0, 0.5, 0.5, 0.0)],
    ),
    (
        PX_BACKSLASH,
        0b10011001,
        &[(0.5, 0.0, 1.0, 0.5), (0.5, 1.0, 0.0, 0.5)],
    ),
    (
        PX_HOUSE1,
        0b10000111,
        &[(0.5, 0.0, 1.0, 0.5), (1.0, 0.5, 0.5, 1.0)],
    ),
    (
        PX_HOUSE2,
        0b11100001,
        &[(1.0, 0.5, 0.5, 1.0), (0.5, 1.0, 0.0, 0.5)],
    ),
    (
        PX_HOUSE3,
        0b01111000,
        &[(0.5, 1.0, 0.0, 0.5), (0.0, 0.5, 0.5, 0.0)],
    ),
    (
        PX_HOUSE4,
        0b00011110,
        &[(0.5, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 0.0)],
    ),
];

#[rustfmt::skip]
const ADJACENCY_BITS: [u8; 129] = [
    0x00, 0x0F, 0xC3, 0x03, 0xC0, 0x30, 0x0C, 0x07, // 0-7
    0x70, 0x83, 0x38, 0x0E, 0xE0, 0xC1, 0x1C, 0x00, // 8-15
    0x03, 0xC0, 0x30, 0x0C, 0x33, 0x06, 0x60, 0x81, // 16-23
    0x18, 0x66, 0x99, 0x87, 0xE1, 0x78, 0x1E, 0x00, // 24-31
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 32-39
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 40-47
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 48-55
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 56-63
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 64-71
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 72-79
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 80-87
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 88-95
    0x00, 0xE1, 0x87, 0x1E, 0x78, 0x66, 0x99, 0xE7, // 96-103
    0x7E, 0x9F, 0xF9, 0xCC, 0xF3, 0xCF, 0x3F, 0xFC, // 104-111
    0xFF, 0xE3, 0x3E, 0x1F, 0xF1, 0xC7, 0x7C, 0x8F, // 112-119
    0xF8, 0xF3, 0xCF, 0x3F, 0xFC, 0x3C, 0xF0, 0xFF, // 120-127
    0xFF, // 128 (clamped alias for ALMOSTFULL)
];

pub fn adjacency(shape_id: u8) -> (u8, &'static [(f32, f32, f32, f32)]) {
    let idx = shape_id.min(128) as usize;
    let bits = ADJACENCY_BITS[idx];
    // 0..=30 are the catalog shapes in `ADJACENCY_MAP` order; 97..=127 are
    // their complements, which share the same gap segments. PX_CUSTOM (31)
    // and its unused complement (96) have no catalog geometry.
    let map_idx = if shape_id <= 30 {
        shape_id as usize
    } else if shape_id >= 97 {
        (127 - shape_id.min(127)) as usize
    } else {
        return (bits, &[]);
    };
    (bits, ADJACENCY_MAP[map_idx].2)
}

#[cfg(any(feature = "editor", test))]
#[derive(Clone, Copy, Debug)]
pub struct EdgeInterval {
    pub start: f32,
    pub end: f32,
}

#[cfg(any(feature = "editor", test))]
impl EdgeInterval {
    pub const EMPTY: Self = Self {
        start: 0.0,
        end: 0.0,
    };

    pub fn is_empty(self) -> bool {
        self.end <= self.start + 1e-6
    }

    pub fn intersect(self, other: Self) -> Self {
        let s = self.start.max(other.start);
        let e = self.end.min(other.end);
        if e > s + 1e-6 {
            Self { start: s, end: e }
        } else {
            Self::EMPTY
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg(any(feature = "editor", test))]
pub struct ShapeEdgeCoverage {
    pub top: EdgeInterval,
    pub bottom: EdgeInterval,
    pub left: EdgeInterval,
    pub right: EdgeInterval,
}

#[cfg(any(feature = "editor", test))]
const EC_Z: ShapeEdgeCoverage = ShapeEdgeCoverage {
    top: EdgeInterval::EMPTY,
    bottom: EdgeInterval::EMPTY,
    left: EdgeInterval::EMPTY,
    right: EdgeInterval::EMPTY,
};
#[cfg(any(feature = "editor", test))]
const EC_F: ShapeEdgeCoverage = ShapeEdgeCoverage {
    top: EdgeInterval {
        start: 0.0,
        end: 1.0,
    },
    bottom: EdgeInterval {
        start: 0.0,
        end: 1.0,
    },
    left: EdgeInterval {
        start: 0.0,
        end: 1.0,
    },
    right: EdgeInterval {
        start: 0.0,
        end: 1.0,
    },
};

// Eight edge coordinates: naming them individually is the point.
#[cfg(any(feature = "editor", test))]
#[expect(clippy::too_many_arguments)]
const fn ec(
    ts: f32,
    te: f32,
    bs: f32,
    be: f32,
    ls: f32,
    le: f32,
    rs: f32,
    re: f32,
) -> ShapeEdgeCoverage {
    ShapeEdgeCoverage {
        top: EdgeInterval { start: ts, end: te },
        bottom: EdgeInterval { start: bs, end: be },
        left: EdgeInterval { start: ls, end: le },
        right: EdgeInterval { start: rs, end: re },
    }
}

#[rustfmt::skip]
#[cfg(any(feature = "editor", test))]
const EDGE_COVERAGE_TABLE: [ShapeEdgeCoverage; 128] = {
    let mut t = [EC_Z; 128];
    // 0: empty → all zero (default)
    t[ 1] = ec(0.0,0.0, 0.0,1.0, 0.0,1.0, 0.0,0.0); // HALF1
    t[ 2] = ec(0.0,1.0, 0.0,0.0, 0.0,1.0, 0.0,0.0); // HALF3
    t[ 3] = ec(0.0,0.0, 0.0,0.0, 0.0,1.0, 0.0,0.0); // QUAD1
    t[ 4] = ec(0.0,1.0, 0.0,0.0, 0.0,0.0, 0.0,0.0); // QUAD2
    t[ 5] = ec(0.0,0.0, 0.0,0.0, 0.0,0.0, 0.0,1.0); // QUAD3
    t[ 6] = ec(0.0,0.0, 0.0,1.0, 0.0,0.0, 0.0,0.0); // QUAD4
    t[ 7] = ec(0.0,0.0, 0.0,0.5, 0.0,1.0, 0.0,0.0); // SLANT1H
    t[ 8] = ec(0.5,1.0, 0.0,0.0, 0.0,0.0, 0.0,1.0); // SLANT2H
    t[ 9] = ec(0.0,0.5, 0.0,0.0, 0.0,1.0, 0.0,0.0); // SLANT3H
    t[10] = ec(0.0,0.0, 0.5,1.0, 0.0,0.0, 0.0,1.0); // SLANT4H
    t[11] = ec(0.0,0.0, 0.0,1.0, 0.5,1.0, 0.0,0.0); // SLANT1V
    t[12] = ec(0.0,1.0, 0.0,0.0, 0.0,0.0, 0.0,0.5); // SLANT2V
    t[13] = ec(0.0,1.0, 0.0,0.0, 0.0,0.5, 0.0,0.0); // SLANT3V
    t[14] = ec(0.0,0.0, 0.0,1.0, 0.0,0.0, 0.5,1.0); // SLANT4V
    // 15: DOT → all zero
    t[16] = ec(0.0,0.0, 0.0,0.0, 0.0,1.0, 0.0,0.0); // CONE1
    t[17] = ec(0.0,1.0, 0.0,0.0, 0.0,0.0, 0.0,0.0); // CONE2
    t[18] = ec(0.0,0.0, 0.0,0.0, 0.0,0.0, 0.0,1.0); // CONE3
    t[19] = ec(0.0,0.0, 0.0,1.0, 0.0,0.0, 0.0,0.0); // CONE4
    t[20] = ec(0.0,0.0, 0.0,0.0, 0.0,1.0, 0.0,1.0); // HQUAD (left+right bowtie)
    t[21] = ec(0.0,0.0, 0.0,0.5, 0.5,1.0, 0.0,0.0); // CORNER1
    t[22] = ec(0.5,1.0, 0.0,0.0, 0.0,0.0, 0.0,0.5); // CORNER2
    t[23] = ec(0.0,0.5, 0.0,0.0, 0.0,0.5, 0.0,0.0); // CORNER3
    t[24] = ec(0.0,0.0, 0.5,1.0, 0.0,0.0, 0.5,1.0); // CORNER4
    // DOT + two corners: the diamond touches no edge, so the coverage is
    // exactly that of the two corners; the complement is the other two.
    t[25] = ec(0.5,1.0, 0.0,0.5, 0.5,1.0, 0.0,0.5); // SLASH (CORNER1+CORNER2)
    t[26] = ec(0.0,0.5, 0.5,1.0, 0.0,0.5, 0.5,1.0); // BACKSLASH (CORNER3+CORNER4)
    t[27] = ec(0.0,0.5, 0.0,0.5, 0.0,1.0, 0.0,0.0); // HOUSE1 (CORNER3+CORNER1)
    t[28] = ec(0.0,1.0, 0.0,0.0, 0.0,0.5, 0.0,0.5); // HOUSE2 (CORNER3+CORNER2)
    t[29] = ec(0.5,1.0, 0.5,1.0, 0.0,0.0, 0.0,1.0); // HOUSE3 (CORNER2+CORNER4)
    t[30] = ec(0.0,0.0, 0.0,1.0, 0.5,1.0, 0.5,1.0); // HOUSE4 (CORNER1+CORNER4)
    t[97] = ec(0.0,1.0, 0.0,0.0, 0.0,0.5, 0.0,0.5); // INVHOUSE4 (CORNER3+CORNER2)
    t[98] = ec(0.0,0.5, 0.0,0.5, 0.0,1.0, 0.0,0.0); // INVHOUSE3 (CORNER3+CORNER1)
    t[99] = ec(0.0,0.0, 0.0,1.0, 0.5,1.0, 0.5,1.0); // INVHOUSE2 (CORNER1+CORNER4)
    t[100] = ec(0.5,1.0, 0.5,1.0, 0.0,0.0, 0.0,1.0); // INVHOUSE1 (CORNER2+CORNER4)
    t[101] = ec(0.5,1.0, 0.0,0.5, 0.5,1.0, 0.0,0.5); // INVBACKSLASH (CORNER1+CORNER2)
    t[102] = ec(0.0,0.5, 0.5,1.0, 0.0,0.5, 0.5,1.0); // INVSLASH (CORNER3+CORNER4)
    t[103] = ec(0.0,1.0, 0.0,0.5, 0.0,1.0, 0.0,0.5); // INVCORNER4
    t[104] = ec(0.5,1.0, 0.0,1.0, 0.5,1.0, 0.0,1.0); // INVCORNER3
    t[105] = ec(0.0,0.5, 0.0,1.0, 0.0,1.0, 0.5,1.0); // INVCORNER2
    t[106] = ec(0.0,1.0, 0.5,1.0, 0.0,0.5, 0.0,1.0); // INVCORNER1
    t[107] = ec(0.0,1.0, 0.0,1.0, 0.0,0.0, 0.0,0.0); // VQUAD (top+bottom bowtie)
    // 108-111: inverted cones — the edge the cone's base sits on is owned
    // by the cone, the other three are fully covered by the complement.
    t[108] = ec(0.0,1.0, 0.0,0.0, 0.0,1.0, 0.0,1.0); // INVCONE4 (base at bottom)
    t[109] = ec(0.0,1.0, 0.0,1.0, 0.0,1.0, 0.0,0.0); // INVCONE3 (base at right)
    t[110] = ec(0.0,0.0, 0.0,1.0, 0.0,1.0, 0.0,1.0); // INVCONE2 (base at top)
    t[111] = ec(0.0,1.0, 0.0,1.0, 0.0,0.0, 0.0,1.0); // INVCONE1 (base at left)
    t[112] = EC_F; // ALMOSTFULL complement (DOT inv)
    t[113] = ec(0.0,1.0, 0.0,0.0, 0.0,1.0, 0.0,0.5); // HALFSLANT1V (inv)
    t[114] = ec(0.0,0.0, 0.0,1.0, 0.5,1.0, 0.0,1.0); // HALFSLANT2V (inv)
    t[115] = ec(0.0,0.0, 0.0,1.0, 0.0,1.0, 0.5,1.0); // HALFSLANT3V (inv)
    t[116] = ec(0.0,1.0, 0.0,0.0, 0.0,0.5, 0.0,1.0); // HALFSLANT4V (inv)
    t[117] = ec(0.0,1.0, 0.0,0.5, 0.0,1.0, 0.0,0.0); // HALFSLANT1H
    t[118] = ec(0.5,1.0, 0.0,1.0, 0.0,0.0, 0.0,1.0); // HALFSLANT2H
    t[119] = ec(0.0,0.5, 0.0,1.0, 0.0,1.0, 0.0,0.0); // HALFSLANT3H
    t[120] = ec(0.0,1.0, 0.5,1.0, 0.0,0.0, 0.0,1.0); // HALFSLANT4H
    // Inverted quadrants: the edge the quadrant's base sits on is owned by
    // the quadrant, the other three are fully covered by the complement.
    t[121] = ec(0.0,1.0, 0.0,0.0, 0.0,1.0, 0.0,1.0); // INVQUAD4 (base at bottom)
    t[122] = ec(0.0,1.0, 0.0,1.0, 0.0,1.0, 0.0,0.0); // INVQUAD3 (base at right)
    t[123] = ec(0.0,0.0, 0.0,1.0, 0.0,1.0, 0.0,1.0); // INVQUAD2 (base at top)
    t[124] = ec(0.0,1.0, 0.0,1.0, 0.0,0.0, 0.0,1.0); // INVQUAD1 (base at left)
    t[125] = ec(0.0,0.0, 0.0,1.0, 0.0,0.0, 0.0,1.0); // INVCONE1
    t[126] = ec(0.0,1.0, 0.0,0.0, 0.0,0.0, 0.0,1.0); // INVCONE2
    t[127] = EC_F; // ALMOSTFULL
    t
};

#[cfg(any(feature = "editor", test))]
pub fn edge_coverage(shape_id: u8) -> &'static ShapeEdgeCoverage {
    &EDGE_COVERAGE_TABLE[shape_id.min(127) as usize]
}

/// The unit-square outline polygon of a catalog shape. All vertices lie on
/// the half lattice. Empty for `PX_EMPTY` and unassigned ids. Note that the
/// chained outline is not a faithful boundary for multi-part shapes
/// (HQUAD/VQUAD) or hole-carrying complements.
#[cfg(test)]
pub fn unit_polygon(shape_id: u8) -> Vec<(f32, f32)> {
    build_unit_polygon(shape_id)
}

fn build_unit_polygon(shape_id: u8) -> Vec<(f32, f32)> {
    let (adj_bits, gap_segs) = adjacency(shape_id);
    if adj_bits == 0 && gap_segs.is_empty() {
        return vec![];
    }
    polygon_from_adjacency(adj_bits, gap_segs)
}

/// Chain a shape's boundary half-edges and gap segments into a single
/// closed polygon in unit-square coordinates.
pub(crate) fn polygon_from_adjacency(
    adj_bits: u8,
    gap_segs: &[(f32, f32, f32, f32)],
) -> Vec<(f32, f32)> {
    if adj_bits == 0xFF {
        return vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    }

    let boundary: [(u8, [f32; 4]); 8] = [
        (7, [0.0, 0.0, 0.5, 0.0]), // a: TL→TM
        (6, [0.5, 0.0, 1.0, 0.0]), // b: TM→TR
        (5, [1.0, 0.0, 1.0, 0.5]), // c: TR→RM
        (4, [1.0, 0.5, 1.0, 1.0]), // d: RM→BR
        (3, [1.0, 1.0, 0.5, 1.0]), // e: BR→BM
        (2, [0.5, 1.0, 0.0, 1.0]), // f: BM→BL
        (1, [0.0, 1.0, 0.0, 0.5]), // g: BL→LM
        (0, [0.0, 0.5, 0.0, 0.0]), // h: LM→TL
    ];

    let mut edges: Vec<[f32; 4]> = Vec::new();
    for &(bit, seg) in &boundary {
        if adj_bits & (1 << bit) != 0 {
            edges.push(seg);
        }
    }
    for &(x1, y1, x2, y2) in gap_segs {
        edges.push([x1, y1, x2, y2]);
        edges.push([x2, y2, x1, y1]);
    }
    if edges.is_empty() {
        return vec![];
    }

    let mut used = vec![false; edges.len()];
    used[0] = true;
    mark_reverse_local(&edges, &mut used, 0);
    let mut polygon = vec![(edges[0][0], edges[0][1])];
    let mut cur = (edges[0][2], edges[0][3]);

    for _ in 0..edges.len() {
        if near_f(cur, polygon[0]) {
            break;
        }
        let mut found = false;
        for (i, e) in edges.iter().enumerate() {
            if !used[i] && near_f((e[0], e[1]), cur) {
                used[i] = true;
                mark_reverse_local(&edges, &mut used, i);
                polygon.push((e[0], e[1]));
                cur = (e[2], e[3]);
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }

    // Splice in sub-loops from unused edges whose start coincides with
    // a vertex already in the polygon (handles pinch-point shapes like
    // inv-cone where the apex sits on a boundary midpoint).
    loop {
        let mut spliced = false;
        for (i, e) in edges.iter().enumerate() {
            if used[i] {
                continue;
            }
            if let Some(insert_at) = polygon.iter().position(|&v| near_f(v, (e[0], e[1]))) {
                used[i] = true;
                mark_reverse_local(&edges, &mut used, i);
                let mut sub = vec![(e[0], e[1])];
                let mut sub_cur = (e[2], e[3]);
                for _ in 0..edges.len() {
                    if near_f(sub_cur, sub[0]) {
                        break;
                    }
                    let mut found = false;
                    for (j, e2) in edges.iter().enumerate() {
                        if !used[j] && near_f((e2[0], e2[1]), sub_cur) {
                            used[j] = true;
                            mark_reverse_local(&edges, &mut used, j);
                            sub.push((e2[0], e2[1]));
                            sub_cur = (e2[2], e2[3]);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        break;
                    }
                }
                // Insert the sub-loop vertices after the matching position
                let splice: Vec<_> = sub.into_iter().skip(1).collect();
                for (k, v) in splice.into_iter().enumerate() {
                    polygon.insert(insert_at + 1 + k, v);
                }
                spliced = true;
                break;
            }
        }
        if !spliced {
            break;
        }
    }

    polygon.dedup_by(|a, b| near_f(*a, *b));
    polygon
}

fn near_f(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.001 && (a.1 - b.1).abs() < 0.001
}

fn mark_reverse_local(edges: &[[f32; 4]], used: &mut [bool], idx: usize) {
    let e = edges[idx];
    for (j, other) in edges.iter().enumerate() {
        if !used[j]
            && near_f((other[0], other[1]), (e[2], e[3]))
            && near_f((other[2], other[3]), (e[0], e[1]))
        {
            used[j] = true;
            return;
        }
    }
}

const SHAPE_TO_CHARS: [[u8; 2]; 256] = {
    let mut table = [*b"??"; 256];
    // Unfilled shapes (PX_FULL = 0)
    table[PX_EMPTY as usize] = *b"..";
    table[PX_HALF1 as usize] = *b"0\\";
    table[PX_HALF3 as usize] = *b"0/";
    table[PX_QUAD1 as usize] = *b"0>";
    table[PX_QUAD2 as usize] = *b"0P";
    table[PX_QUAD3 as usize] = *b"<0";
    table[PX_QUAD4 as usize] = *b"d0";
    table[PX_SLANT1H as usize] = *b"v.";
    table[PX_SLANT2H as usize] = *b"`v";
    table[PX_SLANT3H as usize] = *b"v'";
    table[PX_SLANT4H as usize] = *b".v";
    table[PX_SLANT1V as usize] = *b"h\\";
    table[PX_SLANT2V as usize] = *b"\\h";
    table[PX_SLANT3V as usize] = *b"h/";
    table[PX_SLANT4V as usize] = *b"/h";
    table[PX_DOT as usize] = *b"<>";
    table[PX_CONE1 as usize] = *b"2>";
    table[PX_CONE2 as usize] = *b"2P";
    table[PX_CONE3 as usize] = *b"<2";
    table[PX_CONE4 as usize] = *b"d2";
    table[PX_HQUAD as usize] = *b"><";
    table[PX_CORNER1 as usize] = *b"\\.";
    table[PX_CORNER2 as usize] = *b".\\";
    table[PX_CORNER3 as usize] = *b"/.";
    table[PX_CORNER4 as usize] = *b"./";
    table[PX_VQUAD as usize] = *b"0B";
    table[PX_INVCORNER1 as usize] = *b"\\@";
    table[PX_INVCORNER2 as usize] = *b"@\\";
    table[PX_INVCORNER3 as usize] = *b"/@";
    table[PX_INVCORNER4 as usize] = *b"@/";
    table[PX_INVCONE4 as usize] = *b"P2";
    table[PX_INVCONE3 as usize] = *b"2<";
    table[PX_INVCONE2 as usize] = *b"2d";
    table[PX_INVCONE1 as usize] = *b">2";
    table[PX_HALFSLANT3V as usize] = *b"h_";
    table[PX_HALFSLANT4V as usize] = *b"~h";
    table[PX_HALFSLANT1V as usize] = *b"h~";
    table[PX_HALFSLANT2V as usize] = *b"_h";
    table[PX_HALFSLANT3H as usize] = *b"v/";
    table[PX_HALFSLANT4H as usize] = *b"/v";
    table[PX_HALFSLANT1H as usize] = *b"v\\";
    table[PX_HALFSLANT2H as usize] = *b"\\v";
    table[PX_INVQUAD4 as usize] = *b"P0";
    table[PX_INVQUAD3 as usize] = *b"0<";
    table[PX_INVQUAD2 as usize] = *b"0d";
    table[PX_INVQUAD1 as usize] = *b">0";
    table[PX_HALF4 as usize] = *b"/0";
    table[PX_HALF2 as usize] = *b"\\0";
    table[PX_SLASH as usize] = *b"//";
    table[PX_BACKSLASH as usize] = *b"\\\\";
    table[PX_HOUSE1 as usize] = *b"0D";
    table[PX_HOUSE2 as usize] = *b"0v";
    table[PX_HOUSE3 as usize] = *b"C0";
    table[PX_HOUSE4 as usize] = *b"^0";
    table[PX_INVSLASH as usize] = *b"'.";
    table[PX_INVBACKSLASH as usize] = *b".'";
    table[PX_INVHOUSE1 as usize] = *b".>";
    table[PX_INVHOUSE2 as usize] = *b"M0";
    table[PX_INVHOUSE3 as usize] = *b"<.";
    table[PX_INVHOUSE4 as usize] = *b"0W";
    table[PX_ALMOSTFULL as usize] = *b"88"; // unfilled almostfull — rare
    table[PX_HARDBLANK as usize] = *b"$$"; // a blank the source means

    // Filled shapes (PX_FULL = 0x80, offset by 128)
    table[128 + PX_EMPTY as usize] = *b"__"; // filled empty — unlikely but define
    table[128 + PX_HALF1 as usize] = *b"1\\";
    table[128 + PX_HALF3 as usize] = *b"1/";
    table[128 + PX_QUAD1 as usize] = *b"1>";
    table[128 + PX_QUAD2 as usize] = *b"1P";
    table[128 + PX_QUAD3 as usize] = *b"<1";
    table[128 + PX_QUAD4 as usize] = *b"d1";
    table[128 + PX_SLANT1H as usize] = *b"V.";
    table[128 + PX_SLANT2H as usize] = *b"`V";
    table[128 + PX_SLANT3H as usize] = *b"V'";
    table[128 + PX_SLANT4H as usize] = *b".V";
    table[128 + PX_SLANT1V as usize] = *b"H\\";
    table[128 + PX_SLANT2V as usize] = *b"\\H";
    table[128 + PX_SLANT3V as usize] = *b"H/";
    table[128 + PX_SLANT4V as usize] = *b"/H";
    table[128 + PX_DOT as usize] = *b"{}"; // filled dot
    table[128 + PX_CONE1 as usize] = *b"3>";
    table[128 + PX_CONE2 as usize] = *b"3P";
    table[128 + PX_CONE3 as usize] = *b"<3";
    table[128 + PX_CONE4 as usize] = *b"d3";
    table[128 + PX_HQUAD as usize] = *b")(";
    table[128 + PX_CORNER1 as usize] = *b"b.";
    table[128 + PX_CORNER2 as usize] = *b".9";
    table[128 + PX_CORNER3 as usize] = *b"P.";
    table[128 + PX_CORNER4 as usize] = *b".d";
    table[128 + PX_VQUAD as usize] = *b"1B";
    table[128 + PX_INVCORNER1 as usize] = *b"9@";
    table[128 + PX_INVCORNER2 as usize] = *b"@b";
    table[128 + PX_INVCORNER3 as usize] = *b"d@";
    table[128 + PX_INVCORNER4 as usize] = *b"@P";
    table[128 + PX_INVCONE4 as usize] = *b"P3";
    table[128 + PX_INVCONE3 as usize] = *b"3<";
    table[128 + PX_INVCONE2 as usize] = *b"3d";
    table[128 + PX_INVCONE1 as usize] = *b">3";
    table[128 + PX_HALFSLANT3V as usize] = *b"H_";
    table[128 + PX_HALFSLANT4V as usize] = *b"~H";
    table[128 + PX_HALFSLANT1V as usize] = *b"H~";
    table[128 + PX_HALFSLANT2V as usize] = *b"_H";
    table[128 + PX_HALFSLANT3H as usize] = *b"V/";
    table[128 + PX_HALFSLANT4H as usize] = *b"/V";
    table[128 + PX_HALFSLANT1H as usize] = *b"V\\";
    table[128 + PX_HALFSLANT2H as usize] = *b"\\V";
    table[128 + PX_INVQUAD4 as usize] = *b"P1";
    table[128 + PX_INVQUAD3 as usize] = *b"1<";
    table[128 + PX_INVQUAD2 as usize] = *b"1d";
    table[128 + PX_INVQUAD1 as usize] = *b">1";
    table[128 + PX_HALF4 as usize] = *b"/1";
    table[128 + PX_HALF2 as usize] = *b"\\1";
    table[128 + PX_SLASH as usize] = *b"d/";
    table[128 + PX_BACKSLASH as usize] = *b"\\b";
    table[128 + PX_HOUSE1 as usize] = *b"1D";
    table[128 + PX_HOUSE2 as usize] = *b"1v";
    table[128 + PX_HOUSE3 as usize] = *b"C1";
    table[128 + PX_HOUSE4 as usize] = *b"^1";
    table[128 + PX_INVSLASH as usize] = *b"~_";
    table[128 + PX_INVBACKSLASH as usize] = *b"_~";
    table[128 + PX_INVHOUSE1 as usize] = *b".)";
    table[128 + PX_INVHOUSE2 as usize] = *b"M1";
    table[128 + PX_INVHOUSE3 as usize] = *b"(.";
    table[128 + PX_INVHOUSE4 as usize] = *b"1W";
    table[128 + PX_ALMOSTFULL as usize] = *b"@@"; // the standard filled pixel

    table
};

#[cfg(any(feature = "editor", test))]
pub fn shape_to_chars(shape: PixelShape) -> [char; 2] {
    let [c1, c2] = SHAPE_TO_CHARS[shape.0 as usize];
    [c1 as char, c2 as char]
}

pub fn chars_to_shape(c1: char, c2: char) -> Option<PixelShape> {
    let b1 = c1 as u8;
    let b2 = c2 as u8;
    SHAPE_TO_CHARS
        .iter()
        .enumerate()
        .find(|&(_, &[a, b])| a == b1 && b == b2)
        .map(|(i, _)| PixelShape(i as u8))
}

// ---------------------------------------------------------------------------
// Shape combine (union / subtract) via precomputed rasters
// ---------------------------------------------------------------------------

const RASTER_N: usize = 10;
const RASTER_BITS: usize = RASTER_N * RASTER_N;
const FULL_RASTER: u128 = (1u128 << RASTER_BITS) - 1;

#[rustfmt::skip]
const SHAPE_RASTERS: [u128; 128] = {
    let mut r = [0u128; 128];
    r[  0] = 0x0000000000000000000000000;
    r[  1] = 0x7FCFF1FC3F07C0F01C0300400;
    r[  2] = 0x0040301C0F07C3F1FCFF7FFFF;
    r[  3] = 0x0040301C0F07C0F01C0300400;
    r[  4] = 0x0000000000000301E0FC7FBFF;
    r[  5] = 0x80300E03C0F83C0E030080000;
    r[  6] = 0x7F8FC1E030000000000000000;
    r[  7] = 0x07C0F03C0701C0300C0100400;
    r[  8] = 0x0020080300C0380E03C0F03E0;
    r[  9] = 0x000010040300C0701C0F03C1F;
    r[ 10] = 0xF83C0F0380E0300C020080000;
    r[ 11] = 0x7FC7F07C07004000000000000;
    r[ 12] = 0x000000000000200E03E0FE3FE;
    r[ 13] = 0x00000000000000101C1F1FDFF;
    r[ 14] = 0xFFBF8F8380800000000000000;
    r[ 15] = 0x0C0783F1FEFFDFE3F0780C000;
    r[ 16] = 0x0040707C7F7FDFF1FC1F01C01;
    r[ 17] = 0x000300C0781E0FC3F1FE7FBFF;
    r[ 18] = 0x80380F83F8FFBFEFE3E0E0200;
    r[ 19] = 0xFFDFE7F8FC3F0781E0300C000;
    r[ 20] = 0x80703E1FCFFFFFEFE3E0E0200; // HQUAD
    r[ 21] = 0x03C0700C01000000000000000; // CORNER1
    r[ 22] = 0x000000000000200C0380F03E0; // CORNER2
    r[ 23] = 0x00000000000000100C0703C1F; // CORNER3
    r[ 24] = 0xF0380C0200000000000000000; // CORNER4
    r[ 25] = 0x0FC7F3FDFFFFFFEFF3F8FC3E0; // SLASH        = DOT|CORNER1|CORNER2
    r[ 26] = 0xFC3F8FF3FEFFDFF3FC7F0FC1F; // BACKSLASH    = DOT|CORNER3|CORNER4
    r[ 27] = 0x0FC7F3FDFFFFDFF3FC7F0FC1F; // HOUSE1       = DOT|CORNER3|CORNER1
    r[ 28] = 0x0C0783F1FEFFFFFFFFFFFFFFF; // HOUSE2       = DOT|CORNER3|CORNER2
    r[ 29] = 0xFC3F8FF3FEFFFFEFF3F8FC3E0; // HOUSE3       = DOT|CORNER2|CORNER4
    r[ 30] = 0xFFFFFFFFFFFFDFE3F0780C000; // HOUSE4       = DOT|CORNER1|CORNER4
    r[ 97] = 0x000000000000201C0F87F3FFF; // INVHOUSE4    = CORNER3|CORNER2
    r[ 98] = 0x03C0700C010000100C0703C1F; // INVHOUSE3    = CORNER3|CORNER1
    r[ 99] = 0xF3F87C0E01000000000000000; // INVHOUSE2    = CORNER1|CORNER4
    r[100] = 0xF0380C020000200C0380F03E0; // INVHOUSE1    = CORNER2|CORNER4
    r[101] = 0x03C0700C0100200C0380F03E0; // INVBACKSLASH = CORNER1|CORNER2
    r[102] = 0xF0380C02000000100C0703C1F; // INVSLASH     = CORNER3|CORNER4
    r[103] = 0x0FC7F3FDFFFFFFFFFFFFFFFFF; // INVCORNER4
    r[104] = 0xFFFFFFFFFFFFFFEFF3F8FC3E0; // INVCORNER3
    r[105] = 0xFFFFFFFFFFFFDFF3FC7F0FC1F; // INVCORNER2
    r[106] = 0xFC3F8FF3FEFFFFFFFFFFFFFFF; // INVCORNER1
    r[107] = 0x780F01C0380603C1F0FE7FBFF; // VQUAD
    r[108] = 0x0020180703C0F87E1FCFF3FFF;
    r[109] = 0x7FC7F07C070040101C1F1FDFF;
    r[110] = 0xFFFCFF3F87E1F03C0E0180400;
    r[111] = 0xFFBF8F838080200E03E0FE3FE;
    r[112] = FULL_RASTER;
    r[113] = 0x0040707C7F7FFFFFFFFFFFFFF;
    r[114] = 0xFFFFFFFFFFFFFFEFE3E0E0200;
    r[115] = 0xFFFFFFFFFFFFDFF1FC1F01C01;
    r[116] = 0x80380F83F8FFBFFFFFFFFFFFF;
    r[117] = 0x07C3F0FC7F1FCFF3FDFF7FFFF;
    r[118] = 0xFFFFEFFBFCFF3F8FE3F0FC3E0;
    r[119] = 0xFFDFF7FCFF3FC7F1FC3F0FC1F;
    r[120] = 0xF83F0FC3F8FE3FCFF3FEFFBFF;
    r[121] = 0x80703E1FCFFFFFFFFFFFFFFFF;
    r[122] = 0x7FCFF1FC3F07C3F1FCFF7FFFF;
    r[123] = 0xFFFFFFFFFFFFFCFE1F0380400;
    r[124] = 0xFFBFCFE3F0F83F0FE3FCFFBFF;
    r[125] = 0xFFBFCFE3F0F83C0E030080000;
    r[126] = 0x80300E03C0F83F0FE3FCFFBFF;
    r[127] = FULL_RASTER;
    r
};

#[cfg(any(feature = "editor", test))]
fn raster_to_shape_id(raster: u128) -> u8 {
    if raster == 0 {
        return PX_EMPTY;
    }
    if raster == FULL_RASTER {
        return PX_ALMOSTFULL;
    }
    for i in 0u8..31 {
        if SHAPE_RASTERS[i as usize] == raster {
            return i;
        }
    }
    for i in 97u8..128 {
        if SHAPE_RASTERS[i as usize] == raster {
            return i;
        }
    }
    PX_DOT
}

#[cfg(any(feature = "editor", test))]
pub fn shape_union(a: PixelShape, b: PixelShape) -> PixelShape {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    // A hardblank carries no geometry, so it unions like the empty cell it is:
    // whatever is drawn over it wins, and it survives only where nothing else
    // is (the two `is_empty` returns above keep it over a truly empty cell).
    if a.is_blank() {
        return b;
    }
    if b.is_blank() {
        return a;
    }
    let ur = SHAPE_RASTERS[a.shape_id() as usize] | SHAPE_RASTERS[b.shape_id() as usize];
    let result_id = raster_to_shape_id(ur);
    PixelShape::new(result_id, a.is_filled() || b.is_filled())
}

#[cfg(any(feature = "editor", test))]
pub fn shape_subtract(a: PixelShape, b: PixelShape) -> PixelShape {
    if a.is_blank() || b.is_blank() {
        return a;
    }
    let sr = SHAPE_RASTERS[a.shape_id() as usize]
        & (!SHAPE_RASTERS[b.shape_id() as usize] & FULL_RASTER);
    let result_id = raster_to_shape_id(sr);
    if result_id == PX_EMPTY {
        PixelShape::EMPTY
    } else {
        PixelShape::new(result_id, a.is_filled())
    }
}

// ---------------------------------------------------------------------------
// Multi-shape adjacency (union of overlapping subpixels within one pixel)
// ---------------------------------------------------------------------------

/// The catalog shapes a *disconnected* shape is made of, or an empty slice
/// for the shapes that are one connected piece. [`polygon_from_adjacency`]
/// chains a single ring, so anything that needs a faithful outline — clipping
/// below, filling in the editor — has to ask for the parts and take them one
/// at a time. The bowties meet at the cell center and the two-corner
/// complements at a cell edge midpoint (or not at all).
pub fn shape_parts(shape_id: u8) -> &'static [u8] {
    match shape_id {
        PX_HQUAD => &[PX_QUAD1, PX_QUAD3],
        PX_VQUAD => &[PX_QUAD2, PX_QUAD4],
        PX_INVSLASH => &[PX_CORNER3, PX_CORNER4],
        PX_INVBACKSLASH => &[PX_CORNER1, PX_CORNER2],
        PX_INVHOUSE1 => &[PX_CORNER2, PX_CORNER4],
        PX_INVHOUSE2 => &[PX_CORNER1, PX_CORNER4],
        PX_INVHOUSE3 => &[PX_CORNER3, PX_CORNER1],
        PX_INVHOUSE4 => &[PX_CORNER3, PX_CORNER2],
        _ => &[],
    }
}

/// Compute adjacency bits and gap segments for the union of multiple shapes
/// within a single pixel cell. Returns `(combined_adj_bits, gap_segments)`.
///
/// For a single shape this is equivalent to [`adjacency`]. For multiple shapes,
/// the adjacency bits are OR'd and the gap segments are geometrically clipped
/// so they represent the boundary of the union polygon.
pub fn multi_shape_adjacency(shapes: &[u8]) -> (u8, Vec<Seg>) {
    match shapes.len() {
        0 => return (0, Vec::new()),
        1 => {
            let (bits, segs) = adjacency(shapes[0]);
            return (bits, segs.to_vec());
        }
        _ => {}
    }

    // Decompose multi-part shapes so that each component has a faithful
    // single polygon for clipping.
    let expanded: Vec<u8> = shapes
        .iter()
        .flat_map(|s| {
            let parts = shape_parts(*s);
            if parts.is_empty() {
                std::slice::from_ref(s)
            } else {
                parts
            }
        })
        .copied()
        .collect();
    let shapes = &expanded[..];

    let mut combined_bits = 0u8;
    for &s in shapes {
        combined_bits |= adjacency(s).0;
    }
    if combined_bits == 0xFF {
        return (0xFF, Vec::new());
    }

    let polygons: Vec<Vec<(f32, f32)>> = shapes.iter().map(|&s| build_unit_polygon(s)).collect();

    let mut combined_segs: Vec<Seg> = Vec::new();
    for (i, &s) in shapes.iter().enumerate() {
        let (_, gap_segs) = adjacency(s);
        if gap_segs.is_empty() {
            continue;
        }

        for &seg in gap_segs {
            let outside_normal = gap_outside_normal(seg, &polygons[i]);
            let mut intervals = vec![(0.0f32, 1.0f32)];
            for (j, poly_j) in polygons.iter().enumerate() {
                if i == j || poly_j.len() < 3 {
                    continue;
                }
                intervals = subtract_covered_intervals(seg, outside_normal, &intervals, poly_j);
                if intervals.is_empty() {
                    break;
                }
            }
            let (x1, y1, x2, y2) = seg;
            for (t0, t1) in intervals {
                if t1 - t0 < 1e-4 {
                    continue;
                }
                combined_segs.push((
                    x1 + t0 * (x2 - x1),
                    y1 + t0 * (y2 - y1),
                    x1 + t1 * (x2 - x1),
                    y1 + t1 * (y2 - y1),
                ));
            }
        }
    }

    // When two shapes share a collinear gap boundary (e.g. HALF1's diagonal
    // and QUAD1's diagonal), both shapes emit the overlapping portion because
    // `subtract_covered_intervals` cannot clip collinear polygon edges.
    // Deduplicate so each boundary segment appears exactly once.
    let seg_key = |s: &Seg| -> (u32, u32, u32, u32) {
        let (x1, y1, x2, y2) = *s;
        if (x1.to_bits(), y1.to_bits()) <= (x2.to_bits(), y2.to_bits()) {
            (x1.to_bits(), y1.to_bits(), x2.to_bits(), y2.to_bits())
        } else {
            (x2.to_bits(), y2.to_bits(), x1.to_bits(), y1.to_bits())
        }
    };
    combined_segs.sort_by_key(|a| seg_key(a));
    combined_segs.dedup_by(|a, b| seg_key(a) == seg_key(b));

    (combined_bits, combined_segs)
}

/// Determine which side of a gap segment is "outside" (empty) for the given polygon.
/// Returns a normal vector pointing toward the empty side.
fn gap_outside_normal(seg: Seg, polygon: &[(f32, f32)]) -> (f32, f32) {
    let (x1, y1, x2, y2) = seg;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let mx = (x1 + x2) * 0.5;
    let my = (y1 + y2) * 0.5;
    let eps = 0.002;
    // Left normal candidate
    let (nx, ny) = (-dy, dx);
    let len = (nx * nx + ny * ny).sqrt().max(1e-9);
    let (nx, ny) = (nx / len, ny / len);
    if !point_in_polygon(mx + eps * nx, my + eps * ny, polygon) {
        (nx, ny)
    } else {
        (-nx, -ny)
    }
}

fn point_in_polygon(x: f32, y: f32, polygon: &[(f32, f32)]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Clip gap segment intervals, removing parts where the "outside" of the
/// source shape is filled by `other_polygon`.
fn subtract_covered_intervals(
    seg: Seg,
    outside_normal: (f32, f32),
    intervals: &[(f32, f32)],
    other_polygon: &[(f32, f32)],
) -> Vec<(f32, f32)> {
    let (x1, y1, x2, y2) = seg;
    let dx = x2 - x1;
    let dy = y2 - y1;

    let n = other_polygon.len();
    let mut crossings: Vec<f32> = Vec::new();
    for i in 0..n {
        let (px1, py1) = other_polygon[i];
        let (px2, py2) = other_polygon[(i + 1) % n];
        if let Some(t) = seg_intersect_t(x1, y1, x2, y2, px1, py1, px2, py2)
            && t > 0.002
            && t < 0.998
        {
            crossings.push(t);
        }
    }
    crossings.sort_by(|a, b| a.partial_cmp(b).unwrap());
    crossings.dedup_by(|a, b| (*a - *b).abs() < 0.002);

    let eps = 0.002;
    let (nx, ny) = outside_normal;

    let mut result = Vec::new();
    for &(start, end) in intervals {
        let mut splits = vec![start];
        for &t in &crossings {
            if t > start + eps && t < end - eps {
                splits.push(t);
            }
        }
        splits.push(end);

        for k in 0..splits.len() - 1 {
            let s = splits[k];
            let e = splits[k + 1];
            let mid = (s + e) * 0.5;
            let test_x = x1 + mid * dx + eps * nx;
            let test_y = y1 + mid * dy + eps * ny;
            if !point_in_polygon(test_x, test_y, other_polygon) {
                result.push((s, e));
            }
        }
    }
    result
}

/// Parameter `t` along segment A where it intersects segment B.
/// Returns `None` if segments are parallel or don't intersect.
// Two segments as eight coordinates; a point type here would only add noise.
#[expect(clippy::too_many_arguments)]
fn seg_intersect_t(
    ax1: f32,
    ay1: f32,
    ax2: f32,
    ay2: f32,
    bx1: f32,
    by1: f32,
    bx2: f32,
    by2: f32,
) -> Option<f32> {
    let dx = ax2 - ax1;
    let dy = ay2 - ay1;
    let ex = bx2 - bx1;
    let ey = by2 - by1;
    let denom = dx * ey - dy * ex;
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = ((bx1 - ax1) * ey - (by1 - ay1) * ex) / denom;
    let u = ((bx1 - ax1) * dy - (by1 - ay1) * dx) / denom;
    if (-0.001..=1.001).contains(&t) && (-0.001..=1.001).contains(&u) {
        Some(t.clamp(0.0, 1.0))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Multi-shape difference adjacency (union of positive shapes minus union of
// negative shapes).  Used by contour tracing for negated ref layers.
// ---------------------------------------------------------------------------

/// Compute adjacency bits and gap segments for the geometric difference of
/// two sets of shapes within a single pixel cell:  union(positive) \ union(negative).
///
/// The result is computed by rasterizing both unions on a fine grid, taking
/// the set difference, and finding the closest known shape whose adjacency
/// (bits + gap segments) is guaranteed to form valid closed contours.
pub fn multi_shape_diff_adjacency(
    positive_shapes: &[u8],
    negative_shapes: &[u8],
) -> (u8, Vec<Seg>) {
    if negative_shapes.is_empty() {
        return multi_shape_adjacency(positive_shapes);
    }
    if positive_shapes.is_empty() {
        return (0, Vec::new());
    }

    let mut pos_raster = 0u128;
    for &s in positive_shapes {
        pos_raster |= SHAPE_RASTERS[s as usize];
    }
    let mut neg_raster = 0u128;
    for &s in negative_shapes {
        neg_raster |= SHAPE_RASTERS[s as usize];
    }

    let result_raster = pos_raster & (!neg_raster & FULL_RASTER);
    if result_raster == 0 {
        return (0, Vec::new());
    }

    let best_id = closest_raster_shape(result_raster);
    let (bits, segs) = adjacency(best_id);
    (bits, segs.to_vec())
}

fn closest_raster_shape(target: u128) -> u8 {
    if target == 0 {
        return PX_EMPTY;
    }
    if target == FULL_RASTER {
        return PX_ALMOSTFULL;
    }
    for i in 0u8..31 {
        if SHAPE_RASTERS[i as usize] == target {
            return i;
        }
    }
    for i in 97u8..128 {
        if SHAPE_RASTERS[i as usize] == target {
            return i;
        }
    }
    let mut best = PX_ALMOSTFULL;
    let mut best_dist = u32::MAX;
    for i in 1u8..31 {
        let dist = (target ^ SHAPE_RASTERS[i as usize]).count_ones();
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    for i in 97u8..128 {
        let dist = (target ^ SHAPE_RASTERS[i as usize]).count_ones();
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    best
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_shape_roundtrip() {
        for raw in 0u16..256 {
            let shape = PixelShape(raw as u8);
            let [c1, c2] = shape_to_chars(shape);
            if c1 == '?' {
                continue;
            }
            let decoded = chars_to_shape(c1, c2).unwrap();
            assert_eq!(
                shape, decoded,
                "roundtrip failed for raw={raw}, chars={c1}{c2}"
            );
        }
    }

    #[test]
    fn common_shapes() {
        assert_eq!(shape_to_chars(PixelShape::EMPTY), ['.', '.']);
        assert_eq!(
            shape_to_chars(PixelShape::new(PX_ALMOSTFULL, true)),
            ['@', '@']
        );
        assert_eq!(chars_to_shape('.', '.'), Some(PixelShape::EMPTY));
        assert_eq!(
            chars_to_shape('@', '@'),
            Some(PixelShape::new(PX_ALMOSTFULL, true))
        );
    }

    /// A hardblank is written `$$`, is occupied but not ink, and carries no
    /// geometry at all — every table answers for it as it does for an empty
    /// cell, without a case of its own.
    #[test]
    fn hardblank_is_a_blank_that_is_not_the_empty_cell() {
        let hb = PixelShape::new(PX_HARDBLANK, false);
        assert_eq!(shape_to_chars(hb), ['$', '$']);
        assert_eq!(chars_to_shape('$', '$'), Some(hb));

        assert!(hb.is_hardblank());
        assert!(hb.is_blank());
        assert!(!hb.is_empty(), "a hardblank occupies its cell");
        assert!(!hb.is_filled(), "a hardblank is not ink");
        assert_eq!(hb.ink_shape_id(), PX_EMPTY);

        assert_eq!(adjacency(PX_HARDBLANK).0, 0);
        assert!(adjacency(PX_HARDBLANK).1.is_empty());
        assert_eq!(SHAPE_RASTERS[PX_HARDBLANK as usize], 0);
        let cov = edge_coverage(PX_HARDBLANK);
        for side in [cov.top, cov.right, cov.bottom, cov.left] {
            assert!(side.is_empty(), "a hardblank covers no cell edge");
        }
        assert!(crate::detail::DetailRegion::from_shape(PX_HARDBLANK).is_empty());
    }

    /// Every transform has to land back on a shape that can be written: a
    /// hardblank has no complement id and no filled form to invert into.
    ///
    /// Gated like the transforms themselves — the headless binary never turns a
    /// shape over.
    #[cfg(feature = "editor")]
    #[test]
    fn hardblank_transforms_to_itself() {
        let hb = PixelShape::new(PX_HARDBLANK, false);
        for got in [
            hb.mirror_h(),
            hb.flip_v(),
            hb.rotate_cw(),
            hb.rotate_ccw(),
            hb.rotate_180(),
            hb.with_fill_toggled(),
            hb.opposite(),
            hb.opposite_bitmap(),
        ] {
            assert_eq!(got, hb, "a hardblank transforms to itself");
        }
    }

    /// Combining is by geometry, and a hardblank has none: it yields to
    /// anything drawn over it, and outlives only the empty cell.
    #[test]
    fn hardblank_combines_as_the_blank_it_is() {
        let hb = PixelShape::new(PX_HARDBLANK, false);
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        assert_eq!(shape_union(hb, full), full);
        assert_eq!(shape_union(full, hb), full);
        assert_eq!(shape_union(hb, PixelShape::EMPTY), hb);
        assert_eq!(shape_union(PixelShape::EMPTY, hb), hb);
        assert_eq!(shape_subtract(hb, full), hb);
        assert_eq!(shape_subtract(full, hb), full);
    }

    #[test]
    fn adjacency_table_correct() {
        let (bits, _) = adjacency(PX_EMPTY);
        assert_eq!(bits, 0);

        let (bits, _) = adjacency(PX_ALMOSTFULL);
        assert_eq!(bits, 0xFF);

        let (bits, segs) = adjacency(PX_HALF1);
        assert_eq!(bits, 0b00001111);
        assert_eq!(segs.len(), 1);

        let (bits, _) = adjacency(PX_HALF2);
        assert_eq!(bits, 0b11110000);
    }

    #[test]
    fn edge_coverage_slant1h() {
        // Slant1H: triangle (0,0)→(0.5,1)→(0,1) — covers left half of bottom
        let cov = edge_coverage(PX_SLANT1H);
        assert!(!cov.bottom.is_empty());
        assert!(
            (cov.bottom.start - 0.0).abs() < 0.01,
            "bottom.start={}",
            cov.bottom.start
        );
        assert!(
            (cov.bottom.end - 0.5).abs() < 0.01,
            "bottom.end={}",
            cov.bottom.end
        );
        // top: single point (0,0) — should be empty interval
        assert!(cov.top.is_empty(), "top should be empty: {:?}", cov.top);
    }

    #[test]
    fn edge_coverage_halfslant1h() {
        // HalfSlant1H (complement of Slant2H): covers left half of top only
        let cov = edge_coverage(PX_HALFSLANT1H);
        assert!(!cov.top.is_empty());
        assert!(
            (cov.top.start - 0.0).abs() < 0.01,
            "top.start={}",
            cov.top.start
        );
        assert!((cov.top.end - 0.5).abs() < 0.01, "top.end={}", cov.top.end);
    }

    #[test]
    fn edge_coverage_slant1h_above_halfslant1h() {
        // When Slant1H is above and HalfSlant1H is below:
        // overlap should be [0, 0.5] (left half only)
        let above_bottom = edge_coverage(PX_SLANT1H).bottom;
        let below_top = edge_coverage(PX_HALFSLANT1H).top;
        let overlap = above_bottom.intersect(below_top);
        assert!(!overlap.is_empty(), "should overlap");
        assert!((overlap.start - 0.0).abs() < 0.01);
        assert!(
            (overlap.end - 0.5).abs() < 0.01,
            "overlap.end={}",
            overlap.end
        );
    }

    #[test]
    fn union_empty_identity() {
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        let slant = PixelShape(PX_SLANT1H);
        assert_eq!(shape_union(PixelShape::EMPTY, slant), slant);
        assert_eq!(shape_union(slant, PixelShape::EMPTY), slant);
        assert_eq!(shape_union(PixelShape::EMPTY, full), full);
    }

    #[test]
    fn union_complement_gives_full() {
        // Two unfilled complements → unfilled almostfull
        let half1 = PixelShape(PX_HALF1);
        let half2 = PixelShape(PX_HALF2);
        assert_eq!(shape_union(half1, half2), PixelShape(PX_ALMOSTFULL));
        // One filled → result is filled
        let half1f = PixelShape::new(PX_HALF1, true);
        assert_eq!(
            shape_union(half1f, half2),
            PixelShape::new(PX_ALMOSTFULL, true),
        );
    }

    #[test]
    fn union_slant_with_complement() {
        // SLANT1H + HALFSLANT2H (its complement) → full
        assert_eq!(
            shape_union(PixelShape(PX_SLANT1H), PixelShape(PX_HALFSLANT2H)),
            PixelShape(PX_ALMOSTFULL),
        );
    }

    #[test]
    fn subtract_self_gives_empty() {
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        assert_eq!(shape_subtract(full, full), PixelShape::EMPTY);
        let half1 = PixelShape(PX_HALF1);
        assert_eq!(shape_subtract(half1, half1), PixelShape::EMPTY);
    }

    #[test]
    fn subtract_from_full() {
        let full = PixelShape::new(PX_ALMOSTFULL, true);
        let half1 = PixelShape(PX_HALF1);
        // Subtracting unfilled half1 from filled full → filled half2
        assert_eq!(shape_subtract(full, half1), PixelShape::new(PX_HALF2, true),);
    }

    #[test]
    fn edge_coverage_almostfull() {
        let cov = edge_coverage(PX_ALMOSTFULL);
        assert!((cov.top.start - 0.0).abs() < 0.01);
        assert!((cov.top.end - 1.0).abs() < 0.01);
        assert!((cov.bottom.start - 0.0).abs() < 0.01);
        assert!((cov.bottom.end - 1.0).abs() < 0.01);
        assert!((cov.left.start - 0.0).abs() < 0.01);
        assert!((cov.left.end - 1.0).abs() < 0.01);
        assert!((cov.right.start - 0.0).abs() < 0.01);
        assert!((cov.right.end - 1.0).abs() < 0.01);
    }

    #[test]
    fn multi_shape_single_same_as_adjacency() {
        for &s in &valid_shape_ids() {
            let (bits, segs) = adjacency(s);
            let (mbits, msegs) = multi_shape_adjacency(&[s]);
            assert_eq!(bits, mbits, "bits mismatch for shape {s}");
            assert_eq!(segs.len(), msegs.len(), "segs len mismatch for shape {s}");
        }
    }

    #[test]
    fn multi_shape_complements_fill_pixel() {
        // HALF1 + HALF2 = full pixel, no gap segs
        let (bits, segs) = multi_shape_adjacency(&[PX_HALF1, PX_HALF2]);
        assert_eq!(bits, 0xFF);
        assert!(segs.is_empty());
    }

    #[test]
    fn multi_shape_slant_union_gap_segments() {
        // SLANT1H (bottom-left triangle) + SLANT3H (upper-left triangle)
        // Union covers: a, h, g, f edges; gap goes via (0.25,0.5)
        let (bits, segs) = multi_shape_adjacency(&[PX_SLANT1H, PX_SLANT3H]);
        assert_eq!(bits, adjacency(PX_SLANT1H).0 | adjacency(PX_SLANT3H).0,);
        // Should have 2 gap segments meeting at the intersection point
        assert_eq!(
            segs.len(),
            2,
            "expected 2 clipped gap segments, got {}",
            segs.len()
        );
        // Both segments should share the intersection point (0.25, 0.5)
        let has_intersection = segs
            .iter()
            .any(|&(x1, y1, _, _)| (x1 - 0.25).abs() < 0.01 && (y1 - 0.5).abs() < 0.01)
            || segs
                .iter()
                .any(|&(_, _, x2, y2)| (x2 - 0.25).abs() < 0.01 && (y2 - 0.5).abs() < 0.01);
        assert!(
            has_intersection,
            "gap segs should meet at (0.25, 0.5): {segs:?}"
        );
    }

    #[test]
    fn cone_adjacency() {
        let (bits, segs) = adjacency(PX_CONE1);
        assert_eq!(bits, 0b00000011, "CONE1 bits");
        assert_eq!(segs.len(), 2, "CONE1 segs");

        let (bits, segs) = adjacency(PX_INVCONE1);
        assert_eq!(bits, 0b11111100, "INVCONE1 bits");
        assert_eq!(segs.len(), 2, "INVCONE1 segs");

        let poly = polygon_from_adjacency(bits, segs);
        assert!(
            poly.len() >= 5,
            "INVCONE1 polygon should have >= 5 vertices, got {}",
            poly.len()
        );
    }

    #[test]
    fn dot_polygon_is_the_edge_midpoint_diamond() {
        // The editor draws PX_DOT through the generic polygon path, so this
        // must match the outline the font builder emits (`detail.rs`).
        let (bits, segs) = adjacency(PX_DOT);
        let poly = polygon_from_adjacency(bits, segs);
        assert_eq!(poly.len(), 4, "DOT polygon vertices: {poly:?}");
        for v in [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)] {
            assert!(
                poly.iter().any(|&p| near_f(p, v)),
                "DOT polygon missing {v:?}: {poly:?}"
            );
        }
    }

    #[test]
    fn cone_complement_union_gives_full() {
        assert_eq!(
            shape_union(
                PixelShape::new(PX_CONE1, false),
                PixelShape::new(PX_INVCONE1, false),
            ),
            PixelShape::new(PX_ALMOSTFULL, false),
        );
    }

    #[test]
    fn invcone3_polygon_has_both_triangles() {
        let (bits, segs) = adjacency(PX_INVCONE3);
        let poly = polygon_from_adjacency(bits, segs);
        assert!(
            poly.len() >= 7,
            "INVCONE3 should have >= 7 vertices, got {}",
            poly.len()
        );
        let has_top_right = poly
            .iter()
            .any(|&(x, y)| (x - 1.0).abs() < 0.01 && y.abs() < 0.01);
        let has_bottom_right = poly
            .iter()
            .any(|&(x, y)| (x - 1.0).abs() < 0.01 && (y - 1.0).abs() < 0.01);
        assert!(has_top_right, "missing top-right corner (1,0)");
        assert!(has_bottom_right, "missing bottom-right corner (1,1)");
    }

    // -----------------------------------------------------------------------
    // Verification: recompute all precomputed tables from geometry and compare
    // -----------------------------------------------------------------------

    fn valid_shape_ids() -> Vec<u8> {
        let mut ids: Vec<u8> = Vec::new();
        for &(shape, _, _) in ADJACENCY_MAP {
            ids.push(shape);
            let complement = shape ^ PX_SUBPIXEL;
            if complement != shape && !ids.contains(&complement) {
                ids.push(complement);
            }
        }
        ids.sort();
        ids.dedup();
        ids
    }

    fn rasterize_polygon(polygon: &[(f32, f32)]) -> u128 {
        if polygon.len() < 3 {
            return 0;
        }
        let mut bits = 0u128;
        let n = polygon.len();
        for r in 0..RASTER_N {
            for c in 0..RASTER_N {
                let px = (c as f32 + 0.5) / RASTER_N as f32;
                let py = (r as f32 + 0.3) / RASTER_N as f32;
                let mut inside = false;
                let mut j = n - 1;
                for i in 0..n {
                    let (xi, yi) = polygon[i];
                    let (xj, yj) = polygon[j];
                    if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
                        inside = !inside;
                    }
                    j = i;
                }
                if inside {
                    bits |= 1u128 << (r * RASTER_N + c);
                }
            }
        }
        bits
    }

    /// Derive edge coverage from the adjacency half-edge bits, which are
    /// the tracer's ground truth. Every catalog shape covers each square
    /// edge in a single run whose endpoints are on the half lattice, so the
    /// two bits per edge determine the interval exactly. (The former
    /// polygon-based derivation inherited broken outlines for multi-part
    /// shapes like HQUAD, whose chained "single polygon" is not a faithful
    /// boundary.)
    fn compute_edge_coverage(shape_id: u8) -> ShapeEdgeCoverage {
        let bits = ADJACENCY_BITS[shape_id.min(128) as usize];
        let iv = |first: bool, second: bool| -> EdgeInterval {
            match (first, second) {
                (false, false) => EdgeInterval::EMPTY,
                (true, false) => EdgeInterval {
                    start: 0.0,
                    end: 0.5,
                },
                (false, true) => EdgeInterval {
                    start: 0.5,
                    end: 1.0,
                },
                (true, true) => EdgeInterval {
                    start: 0.0,
                    end: 1.0,
                },
            }
        };
        //    a   b
        //   +--+--+
        // h |     | c
        //   +     +
        // g |     | d
        //   +--+--+
        //    f   e
        ShapeEdgeCoverage {
            top: iv(bits & 0x80 != 0, bits & 0x40 != 0),
            right: iv(bits & 0x20 != 0, bits & 0x10 != 0),
            bottom: iv(bits & 0x04 != 0, bits & 0x08 != 0),
            left: iv(bits & 0x01 != 0, bits & 0x02 != 0),
        }
    }

    #[test]
    fn verify_adjacency_bits() {
        for &(shape, bits, _) in ADJACENCY_MAP {
            assert_eq!(
                ADJACENCY_BITS[shape as usize], bits,
                "ADJACENCY_BITS mismatch for base shape {shape}"
            );
            let compl = shape ^ PX_SUBPIXEL;
            assert_eq!(
                ADJACENCY_BITS[compl as usize],
                bits ^ 0xFF,
                "ADJACENCY_BITS mismatch for complement shape {compl}"
            );
        }
        assert_eq!(ADJACENCY_BITS[128], ADJACENCY_BITS[PX_ALMOSTFULL as usize]);
    }

    #[test]
    fn verify_adjacency_segs() {
        for &(shape, _, expected_segs) in ADJACENCY_MAP {
            let (_, segs) = adjacency(shape);
            assert_eq!(
                segs, expected_segs,
                "adjacency segs mismatch for base shape {shape}"
            );
            let compl = shape ^ PX_SUBPIXEL;
            let (_, csegs) = adjacency(compl);
            assert_eq!(
                csegs, expected_segs,
                "adjacency segs mismatch for complement shape {compl}"
            );
        }
    }

    #[test]
    fn verify_edge_coverage() {
        for i in 0u8..128 {
            let expected = compute_edge_coverage(i);
            let actual = &EDGE_COVERAGE_TABLE[i as usize];
            let close = |a: f32, b: f32| (a - b).abs() < 0.01;
            assert!(
                close(actual.top.start, expected.top.start)
                    && close(actual.top.end, expected.top.end)
                    && close(actual.bottom.start, expected.bottom.start)
                    && close(actual.bottom.end, expected.bottom.end)
                    && close(actual.left.start, expected.left.start)
                    && close(actual.left.end, expected.left.end)
                    && close(actual.right.start, expected.right.start)
                    && close(actual.right.end, expected.right.end),
                "EDGE_COVERAGE mismatch for shape {i}: \
                expected ({},{},{},{},{},{},{},{}) got ({},{},{},{},{},{},{},{})",
                expected.top.start,
                expected.top.end,
                expected.bottom.start,
                expected.bottom.end,
                expected.left.start,
                expected.left.end,
                expected.right.start,
                expected.right.end,
                actual.top.start,
                actual.top.end,
                actual.bottom.start,
                actual.bottom.end,
                actual.left.start,
                actual.left.end,
                actual.right.start,
                actual.right.end,
            );
        }
    }

    /// The raster a shape's geometry demands. Complements are the bitwise
    /// complement of their base rather than a rasterized outline, because
    /// [`polygon_from_adjacency`] can only chain a *connected* boundary: the
    /// two-corner complements (INVSLASH and friends) would lose a triangle.
    /// PX_VQUAD and the inverse dot are the two whose stored raster predates
    /// that rule: their parts meet only at points, and each holds the
    /// (unfaithful) outline the chainer produced — as PX_HQUAD does.
    fn expected_raster(s: u8) -> u128 {
        if s >= 97 && s != PX_VQUAD && s != PX_DOT ^ PX_SUBPIXEL {
            FULL_RASTER ^ rasterize_polygon(&build_unit_polygon(s ^ PX_SUBPIXEL))
        } else {
            rasterize_polygon(&build_unit_polygon(s))
        }
    }

    #[test]
    fn verify_rasters() {
        let valid = valid_shape_ids();
        for &s in &valid {
            assert_eq!(
                SHAPE_RASTERS[s as usize],
                expected_raster(s),
                "SHAPE_RASTERS mismatch for shape {s}"
            );
        }
    }

    #[test]
    fn verify_union_exhaustive() {
        let valid = valid_shape_ids();
        let mut computed_rasters = [0u128; 128];
        for &s in &valid {
            computed_rasters[s as usize] = expected_raster(s);
        }
        let mut raster_to_id = std::collections::HashMap::new();
        for &s in &valid {
            raster_to_id
                .entry(computed_rasters[s as usize])
                .or_insert(s);
        }
        raster_to_id.insert(0, PX_EMPTY);
        raster_to_id.insert(FULL_RASTER, PX_ALMOSTFULL);

        for &a in &valid {
            for &b in &valid {
                let ur = computed_rasters[a as usize] | computed_rasters[b as usize];
                let expected = raster_to_id.get(&ur).copied().unwrap_or(PX_DOT);
                let sa = PixelShape(a);
                let sb = PixelShape(b);
                if sa.is_empty() {
                    assert_eq!(shape_union(sa, sb), sb, "union({a},{b}) identity");
                } else if sb.is_empty() {
                    assert_eq!(shape_union(sa, sb), sa, "union({a},{b}) identity");
                } else {
                    assert_eq!(
                        shape_union(sa, sb).shape_id(),
                        expected,
                        "union({a},{b}) mismatch"
                    );
                }
            }
        }
    }

    #[test]
    fn verify_subtract_exhaustive() {
        let valid = valid_shape_ids();
        let mut computed_rasters = [0u128; 128];
        for &s in &valid {
            computed_rasters[s as usize] = expected_raster(s);
        }
        let mut raster_to_id = std::collections::HashMap::new();
        for &s in &valid {
            raster_to_id
                .entry(computed_rasters[s as usize])
                .or_insert(s);
        }
        raster_to_id.insert(0, PX_EMPTY);
        raster_to_id.insert(FULL_RASTER, PX_ALMOSTFULL);

        for &a in &valid {
            for &b in &valid {
                let sr =
                    computed_rasters[a as usize] & (!computed_rasters[b as usize] & FULL_RASTER);
                let expected = raster_to_id.get(&sr).copied().unwrap_or(PX_DOT);
                let sa = PixelShape(a);
                let sb = PixelShape(b);
                if sa.is_empty() || sb.is_empty() {
                    continue; // early-return paths tested separately
                }
                let result = shape_subtract(sa, sb);
                let result_id = if result.is_empty() {
                    PX_EMPTY
                } else {
                    result.shape_id()
                };
                assert_eq!(
                    result_id, expected,
                    "subtract({a},{b}) mismatch: got {result_id}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn transform_tables_consistent_with_adjacency() {
        // Verify transforms using adjacency bits (8 half-edges around the cell).
        // Mirror H swaps: a↔b, c↔h, d↔g, e↔f
        // Flip V: new = (f,e,d,c,b,a,h,g) from original (a,b,c,d,e,f,g,h)
        // Rotate CW: new = (g,h,a,b,c,d,e,f) (shift right by 2)
        fn adj(id: u8) -> u8 {
            ADJACENCY_BITS[id.min(128) as usize]
        }
        fn mirror_adj(bits: u8) -> u8 {
            let a = (bits >> 7) & 1;
            let b = (bits >> 6) & 1;
            let c = (bits >> 5) & 1;
            let d = (bits >> 4) & 1;
            let e = (bits >> 3) & 1;
            let f = (bits >> 2) & 1;
            let g = (bits >> 1) & 1;
            let h = bits & 1;
            (b << 7) | (a << 6) | (h << 5) | (g << 4) | (f << 3) | (e << 2) | (d << 1) | c
        }
        fn flip_adj(bits: u8) -> u8 {
            let a = (bits >> 7) & 1;
            let b = (bits >> 6) & 1;
            let c = (bits >> 5) & 1;
            let d = (bits >> 4) & 1;
            let e = (bits >> 3) & 1;
            let f = (bits >> 2) & 1;
            let g = (bits >> 1) & 1;
            let h = bits & 1;
            (f << 7) | (e << 6) | (d << 5) | (c << 4) | (b << 3) | (a << 2) | (h << 1) | g
        }
        fn rotate_cw_adj(bits: u8) -> u8 {
            let a = (bits >> 7) & 1;
            let b = (bits >> 6) & 1;
            let c = (bits >> 5) & 1;
            let d = (bits >> 4) & 1;
            let e = (bits >> 3) & 1;
            let f = (bits >> 2) & 1;
            let g = (bits >> 1) & 1;
            let h = bits & 1;
            (g << 7) | (h << 6) | (a << 5) | (b << 4) | (c << 3) | (d << 2) | (e << 1) | f
        }

        // Every catalog id except PX_CUSTOM (31) and its unused complement (96).
        let used_ids: Vec<u8> = (0..31).chain(97..128).collect();
        for &id in &used_ids {
            let bits = adj(id);
            let shape = PixelShape(id);

            let m_id = shape.mirror_h().shape_id();
            assert_eq!(
                adj(m_id),
                mirror_adj(bits),
                "mirror_h adjacency mismatch for id={id}: got id={m_id} adj={:#010b}, expected adj={:#010b}",
                adj(m_id),
                mirror_adj(bits)
            );

            let f_id = shape.flip_v().shape_id();
            assert_eq!(
                adj(f_id),
                flip_adj(bits),
                "flip_v adjacency mismatch for id={id}: got id={f_id} adj={:#010b}, expected adj={:#010b}",
                adj(f_id),
                flip_adj(bits)
            );

            let r_id = shape.rotate_cw().shape_id();
            assert_eq!(
                adj(r_id),
                rotate_cw_adj(bits),
                "rotate_cw adjacency mismatch for id={id}: got id={r_id} adj={:#010b}, expected adj={:#010b}",
                adj(r_id),
                rotate_cw_adj(bits)
            );
        }
    }

    #[test]
    fn transform_inverse_properties() {
        // Every catalog id except PX_CUSTOM (31) and its unused complement (96).
        let used_ids: Vec<u8> = (0..31).chain(97..128).collect();
        for &id in &used_ids {
            let shape = PixelShape(id | PX_FULL);

            // mirror_h is self-inverse
            assert_eq!(
                shape.mirror_h().mirror_h(),
                shape,
                "mirror_h not involutory for id={id}"
            );

            // flip_v is self-inverse
            assert_eq!(
                shape.flip_v().flip_v(),
                shape,
                "flip_v not involutory for id={id}"
            );

            // rotate_180 is self-inverse
            assert_eq!(
                shape.rotate_180().rotate_180(),
                shape,
                "rotate_180 not involutory for id={id}"
            );

            // rotate_cw and rotate_ccw are inverses
            assert_eq!(
                shape.rotate_cw().rotate_ccw(),
                shape,
                "cw/ccw not inverse for id={id}"
            );
            assert_eq!(
                shape.rotate_ccw().rotate_cw(),
                shape,
                "ccw/cw not inverse for id={id}"
            );

            // 4x rotate_cw = identity
            assert_eq!(
                shape.rotate_cw().rotate_cw().rotate_cw().rotate_cw(),
                shape,
                "4x cw not identity for id={id}"
            );

            // rotate_180 = rotate_cw twice
            assert_eq!(
                shape.rotate_cw().rotate_cw(),
                shape.rotate_180(),
                "2x cw != 180 for id={id}"
            );
        }
    }

    /// Two corners make the complement, and the dot on top of it makes the
    /// shape: both unions have to land on the catalog id rather than fall
    /// back to PX_DOT. (A *single* corner over the dot still has no catalog
    /// id, so it does not survive a union — build them the other way round.)
    #[test]
    fn dot_plus_two_corners_unions_to_the_new_shape() {
        let cases = [
            (PX_SLASH, (PX_CORNER1, PX_CORNER2), (PX_CORNER3, PX_CORNER4)),
            (
                PX_BACKSLASH,
                (PX_CORNER3, PX_CORNER4),
                (PX_CORNER1, PX_CORNER2),
            ),
            (
                PX_HOUSE1,
                (PX_CORNER3, PX_CORNER1),
                (PX_CORNER2, PX_CORNER4),
            ),
            (
                PX_HOUSE2,
                (PX_CORNER3, PX_CORNER2),
                (PX_CORNER1, PX_CORNER4),
            ),
            (
                PX_HOUSE3,
                (PX_CORNER2, PX_CORNER4),
                (PX_CORNER3, PX_CORNER1),
            ),
            (
                PX_HOUSE4,
                (PX_CORNER1, PX_CORNER4),
                (PX_CORNER3, PX_CORNER2),
            ),
        ];
        let lit = |s| PixelShape::new(s, true);
        for (id, (a, b), (c, d)) in cases {
            let pair = shape_union(lit(a), lit(b));
            let grown = shape_union(lit(PX_DOT), pair);
            assert_eq!(grown, lit(id), "DOT+{a}+{b}");
            // The two corners left over are the complement, and putting them
            // back fills the cell.
            let rest = shape_union(lit(c), lit(d));
            assert_eq!(rest.shape_id(), id ^ PX_SUBPIXEL, "complement of {id}");
            assert_eq!(shape_union(grown, rest), lit(PX_ALMOSTFULL));
            assert_eq!(shape_subtract(lit(PX_ALMOSTFULL), rest), grown);
        }
    }

    #[test]
    fn multi_shape_adjacency_hquad_dot() {
        let (bits, segs) = multi_shape_adjacency(&[PX_HQUAD, PX_DOT]);
        assert_eq!(bits, 0b00110011);
        // Gap segments + boundary edges must form closed contours.
        // Collect all edges (gap + boundary) and verify even degree.
        let mut all_segs = segs.clone();
        let boundary: [(u8, [f32; 4]); 8] = [
            (7, [0.0, 0.0, 0.5, 0.0]),
            (6, [0.5, 0.0, 1.0, 0.0]),
            (5, [1.0, 0.0, 1.0, 0.5]),
            (4, [1.0, 0.5, 1.0, 1.0]),
            (3, [1.0, 1.0, 0.5, 1.0]),
            (2, [0.5, 1.0, 0.0, 1.0]),
            (1, [0.0, 1.0, 0.0, 0.5]),
            (0, [0.0, 0.5, 0.0, 0.0]),
        ];
        for &(bit, seg) in &boundary {
            if bits & (1 << bit) != 0 {
                all_segs.push((seg[0], seg[1], seg[2], seg[3]));
            }
        }
        let mut degree: std::collections::HashMap<(i32, i32), u32> =
            std::collections::HashMap::new();
        let quantize = |v: f32| (v * 1200.0).round() as i32;
        for &(x1, y1, x2, y2) in &all_segs {
            *degree.entry((quantize(x1), quantize(y1))).or_default() += 1;
            *degree.entry((quantize(x2), quantize(y2))).or_default() += 1;
        }
        for (&k, &d) in &degree {
            assert!(d % 2 == 0, "odd degree {d} at ({}, {})", k.0, k.1);
        }
    }
}
