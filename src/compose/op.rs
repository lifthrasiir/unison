//! The IDC operators and how each one lays its parts out: the enclosing operators' `Walls`, the `Raster` a parent lays its parts on, `IdcOp`, and `Direction`.

/// Which sides of the parent's box an enclosing operator's outer part fills.
///
/// This is the whole of what tells the nine enclosing operators apart, and it
/// is read by everything: which boundary a clearance is measured against, which
/// side the cavity is flush with, and — through both of those — where the fixer
/// is allowed to put the inner part. Keeping it one table is why there is no
/// second place for `⿷` to mean something slightly different.
///
/// A side that is *not* a wall is **open**: the inner part is measured against
/// the parent's own edge there rather than against anything the outer part
/// draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walls {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Walls {
    /// The walls that face along one axis, low side first: left/right for the
    /// x axis, top/bottom for the y axis.
    pub fn along(self, horizontal: bool) -> (bool, bool) {
        if horizontal {
            (self.left, self.right)
        } else {
            (self.top, self.bottom)
        }
    }

    /// How many of an axis's two clearances touch the parent's own edge — 0
    /// when the axis is walled on both sides, 1 otherwise. Never 2: every
    /// enclosing operator walls at least one side of each axis.
    pub fn open_count(self, horizontal: bool) -> usize {
        let (lo, hi) = self.along(horizontal);
        usize::from(!lo) + usize::from(!hi)
    }
}

/// Where an IDC line's parent lays its parts out: the `scale` a `ref` offset
/// is counted in, and the declared `origin` its box's corner sits at. A split
/// fills the *box*, and a `ref` is placed against the *grid*, so the two meet
/// only when nothing moves the box off the grid's corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raster {
    pub scale: u8,
    pub origin: (i16, i16),
}

impl Raster {
    pub fn of(body: &crate::document::GlyphBody) -> Self {
        Raster {
            scale: body.scale,
            origin: body.declared_origin(),
        }
    }
}

/// The keyword in front of an [assumed](crate::compose#an-assumed-line) IDC line.
pub const ASSUME: &str = "assume";

/// The IDCs a source may write: the four one-dimensional splits and the nine
/// enclosures.
///
/// `⿻` (overlaid), `⿾` (mirrored) and `⿿` (rotated) are deliberately absent.
/// The first is not a layout — it says two drawings occupy the same box and
/// nothing about where — and the other two are transformations of one drawing
/// rather than a composition of two, which is a different mechanism entirely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdcOp {
    /// ⿰ U+2FF0 left to right.
    LeftRight,
    /// ⿱ U+2FF1 above to below.
    AboveBelow,
    /// ⿲ U+2FF2 left to middle and right.
    LeftMiddleRight,
    /// ⿳ U+2FF3 above to middle and below.
    AboveMiddleBelow,
    /// ⿴ U+2FF4 surround — walled on all four sides (囗).
    Surround,
    /// ⿵ U+2FF5 surround from above — open below (冂, 門).
    SurroundAbove,
    /// ⿶ U+2FF6 surround from below — open above (凵).
    SurroundBelow,
    /// ⿷ U+2FF7 surround from the left — open right (匚).
    SurroundLeft,
    /// ⿸ U+2FF8 surround from the upper left — open right and below (广, 尸).
    SurroundUpperLeft,
    /// ⿹ U+2FF9 surround from the upper right — open left and below (勹, 气).
    SurroundUpperRight,
    /// ⿺ U+2FFA surround from the lower left — open right and above (辶, 廴).
    SurroundLowerLeft,
    /// ⿼ U+2FFC surround from the right — open left.
    SurroundRight,
    /// ⿽ U+2FFD surround from the lower right — open left and above.
    SurroundLowerRight,
}

impl IdcOp {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '\u{2FF0}' => Some(Self::LeftRight),
            '\u{2FF1}' => Some(Self::AboveBelow),
            '\u{2FF2}' => Some(Self::LeftMiddleRight),
            '\u{2FF3}' => Some(Self::AboveMiddleBelow),
            '\u{2FF4}' => Some(Self::Surround),
            '\u{2FF5}' => Some(Self::SurroundAbove),
            '\u{2FF6}' => Some(Self::SurroundBelow),
            '\u{2FF7}' => Some(Self::SurroundLeft),
            '\u{2FF8}' => Some(Self::SurroundUpperLeft),
            '\u{2FF9}' => Some(Self::SurroundUpperRight),
            '\u{2FFA}' => Some(Self::SurroundLowerLeft),
            '\u{2FFC}' => Some(Self::SurroundRight),
            '\u{2FFD}' => Some(Self::SurroundLowerRight),
            _ => None,
        }
    }

    /// The operator a whole token spells, or `None` for a token that is not one
    /// IDC character.
    pub fn from_token(token: &str) -> Option<Self> {
        let mut chars = token.chars();
        let op = Self::from_char(chars.next()?)?;
        chars.next().is_none().then_some(op)
    }

    /// The operator a line's leading tokens start an IDC line with, and whether
    /// the line is [`assume`d](crate::compose#an-assumed-line) — `None` for every other
    /// line. The one place that knows `assume` may stand in front of the
    /// operator, so that nothing reading a line by its first token has to.
    pub fn of_line<'a>(mut tokens: impl Iterator<Item = &'a str>) -> Option<(Self, bool)> {
        match tokens.next()? {
            ASSUME => Some((Self::from_token(tokens.next()?)?, true)),
            first => Some((Self::from_token(first)?, false)),
        }
    }

    pub fn as_char(self) -> char {
        match self {
            Self::LeftRight => '\u{2FF0}',
            Self::AboveBelow => '\u{2FF1}',
            Self::LeftMiddleRight => '\u{2FF2}',
            Self::AboveMiddleBelow => '\u{2FF3}',
            Self::Surround => '\u{2FF4}',
            Self::SurroundAbove => '\u{2FF5}',
            Self::SurroundBelow => '\u{2FF6}',
            Self::SurroundLeft => '\u{2FF7}',
            Self::SurroundUpperLeft => '\u{2FF8}',
            Self::SurroundUpperRight => '\u{2FF9}',
            Self::SurroundLowerLeft => '\u{2FFA}',
            Self::SurroundRight => '\u{2FFC}',
            Self::SurroundLowerRight => '\u{2FFD}',
        }
    }

    /// Which sides the outer part fills, or `None` for a one-dimensional
    /// operator. This is the test for "is this an enclosure" everywhere:
    /// asking for the walls and asking whether there are any are the same
    /// question, and two spellings of it would be two chances to drift.
    pub fn walls(self) -> Option<Walls> {
        let w = |left, right, top, bottom| {
            Some(Walls {
                left,
                right,
                top,
                bottom,
            })
        };
        match self {
            Self::LeftRight | Self::AboveBelow | Self::LeftMiddleRight | Self::AboveMiddleBelow => {
                None
            }
            Self::Surround => w(true, true, true, true),
            Self::SurroundAbove => w(true, true, true, false),
            Self::SurroundBelow => w(true, true, false, true),
            Self::SurroundLeft => w(true, false, true, true),
            Self::SurroundRight => w(false, true, true, true),
            Self::SurroundUpperLeft => w(true, false, true, false),
            Self::SurroundUpperRight => w(false, true, true, false),
            Self::SurroundLowerLeft => w(true, false, false, true),
            Self::SurroundLowerRight => w(false, true, false, true),
        }
    }

    /// Whether the operator encloses rather than splits along an axis.
    pub fn enclosing(self) -> bool {
        self.walls().is_some()
    }

    /// Whether the split runs along the x axis.
    ///
    /// Only a one-dimensional operator has one axis; an enclosure lays out on
    /// both, and every consumer of this has to ask [`Self::walls`] first.
    /// `false` here is the answer for an operator that has no axis at all, not
    /// a claim that it is vertical.
    pub fn horizontal(self) -> bool {
        matches!(self, Self::LeftRight | Self::LeftMiddleRight)
    }

    /// How many components the operator takes.
    pub fn arity(self) -> usize {
        match self {
            Self::LeftMiddleRight | Self::AboveMiddleBelow => 3,
            _ => 2,
        }
    }

    /// Which position a component sits in, for the name check and the
    /// tie-break. Slots past the arity have no direction, and so does every
    /// slot of an enclosure: `l`/`r`/`u`/`d` describe an *end* of an axis,
    /// which is not what an outer and an inner part are to each other. What
    /// says which slot an enclosure's name was drawn for is the cavity in it
    /// — see [`VariantSpec::inner`](super::variant::VariantSpec::inner) and [`enclosure_rank`](super::variant::enclosure_rank).
    ///
    /// A three-part split's middle slot has no direction either: there is no
    /// such thing as a drawing made for the middle, only one made for a side
    /// and used there. See the module docs.
    pub fn slot_direction(self, slot: usize) -> Option<Direction> {
        if self.enclosing() {
            return None;
        }
        let arity = self.arity();
        if slot >= arity {
            return None;
        }
        let last = slot + 1 == arity;
        Some(match (self.horizontal(), slot == 0, last) {
            (true, true, _) => Direction::Left,
            (true, _, true) => Direction::Right,
            (false, true, _) => Direction::Up,
            (false, _, true) => Direction::Down,
            _ => return None,
        })
    }
}

/// The position a variant name claims: `l`, `r`, `u`, `d`.
///
/// An end of an axis, never its middle — see [`IdcOp::slot_direction`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub(super) fn from_token(token: &str) -> Option<Self> {
        match token {
            "l" => Some(Self::Left),
            "r" => Some(Self::Right),
            "u" => Some(Self::Up),
            "d" => Some(Self::Down),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "l",
            Self::Right => "r",
            Self::Up => "u",
            Self::Down => "d",
        }
    }
}
