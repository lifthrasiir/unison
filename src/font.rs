use std::fmt;
use std::error::Error;
use std::io::{self, Read};
use std::collections::{BTreeMap, BTreeSet};
use serde_json;

pub const PX_SUBPIXEL: u8 = 0x1f;
pub const PX_FULL:     u8 = 0x20;

const M: u8 = PX_SUBPIXEL;

pub const PX_EMPTY:       u8 =  0;   // .
pub const PX_ALMOSTFULL:  u8 =  0^M; // @
pub const PX_HALF1:       u8 =  1;   // |\  b
pub const PX_HALF2:       u8 =  1^M; //  \| 9
pub const PX_HALF3:       u8 =  2;   // |/  P
pub const PX_HALF4:       u8 =  2^M; //  /| d
pub const PX_QUAD1:       u8 =  3;   // |>  |)
pub const PX_QUAD2:       u8 =  4;   //  v   u
pub const PX_QUAD3:       u8 =  5;   //  <|  (|
pub const PX_QUAD4:       u8 =  6;   //  ^   n
pub const PX_INVQUAD1:    u8 =  3^M; //  >|  )|
pub const PX_INVQUAD2:    u8 =  4^M; // |v| |u|
pub const PX_INVQUAD3:    u8 =  5^M; // |<  |(
pub const PX_INVQUAD4:    u8 =  6^M; // |^| |n|
pub const PX_SLANT1H:     u8 =  7;   // same to PX_HALF* but shifted and scaled horizontally by 1/2
pub const PX_SLANT2H:     u8 =  8;   //   ....               ....
pub const PX_SLANT3H:     u8 =  9;   //   : /| = PX_HALF4    .  | = PX_SLANT4H
pub const PX_SLANT4H:     u8 = 10;   //   :/_|               ../|
pub const PX_SLANT1V:     u8 = 11;   // same to PX_HALF* but shifted and scaled vertically by 1/2
pub const PX_SLANT2V:     u8 = 12;   //   ....               ....
pub const PX_SLANT3V:     u8 = 13;   //   : /| = PX_HALF4    .  : = PX_SLANT4V
pub const PX_SLANT4V:     u8 = 14;   //   :/_|               ._/|
pub const PX_HALFSLANT1H: u8 =  8^M; // same to PX_HALF* but shifted and scaled horizontally by 3/2
pub const PX_HALFSLANT2H: u8 =  7^M; //   ....               ....
pub const PX_HALFSLANT3H: u8 = 10^M; //   : /| = PX_HALF4    . || = PX_HALFSLANT4H
pub const PX_HALFSLANT4H: u8 =  9^M; //   :/_|               ./_|
pub const PX_HALFSLANT1V: u8 = 12^M; // same to PX_HALF* but shifted and scaled vertically by 3/2
pub const PX_HALFSLANT2V: u8 = 11^M; //   ....               ....
pub const PX_HALFSLANT3V: u8 = 14^M; //   : /| = PX_HALF4    ._/| = PX_HALFSLANT4V
pub const PX_HALFSLANT4V: u8 = 13^M; //   :/_|               |__|
pub const PX_DOT:         u8 = 15;   // *

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Font {
    pub height: usize,
    pub ascent: i32,
    pub descent: i32,
    pub glyphs: BTreeMap<String, Glyph>,
    pub cmap: BTreeMap<u32, String>,
    pub remaps: BTreeMap<String, Vec<Remap>>,
    pub features: BTreeMap<String, Vec<String>>,
    pub exclude_from_sample: BTreeSet<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Glyph {
    pub flags: u32,
    pub height: u32,
    pub width: u32,
    pub preferred_top: i32,
    pub preferred_left: i32,
    pub subglyphs: Vec<Subglyph>,
    pub points: BTreeMap<String, (i32, i32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subglyph {
    pub top: i32,
    pub left: i32,
    pub height: u32,
    pub width: u32,
    pub stride: Option<usize>,
    pub data: Option<SubglyphData>,
    pub negated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocatedSubglyph<'a> {
    pub top: i32,
    pub left: i32,
    pub height: u32,
    pub width: u32,
    pub stride: usize,
    pub data: &'a [u8],
    pub negated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubglyphData {
    Pixels(Vec<u8>),
    Named(String),
    Adjoin((String,))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remap {
    pub lookbehind: Vec<RemapItem>,
    pub pattern: Vec<RemapItem>,
    pub lookahead: Vec<RemapItem>,
    pub replacement: Vec<RemapItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RemapItem {
    Name(String), // glyph name, or set name prefixed by `%`
    List(Vec<String>), // glyph names
}

#[derive(Debug)]
pub struct NoSuchGlyphName {
    name: String,
}

impl fmt::Display for NoSuchGlyphName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "No such glyph name `{}`", self.name)
    }
}

impl Error for NoSuchGlyphName {
    fn description(&self) -> &str { "No such glyph name" }
}

impl From<NoSuchGlyphName> for io::Error {
    fn from(e: NoSuchGlyphName) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, e)
    }
}

impl Font {
    pub fn from_json<R: Read>(r: R) -> serde_json::Result<Font> {
        serde_json::from_reader(r)
    }

    pub fn get_glyph(&self, name: &str) -> Result<&Glyph, NoSuchGlyphName> {
        self.glyphs.get(name).ok_or_else(|| NoSuchGlyphName { name: name.to_owned() })
    }

    pub fn get_subglyphs<'a>(&'a self,
                             name: &str) -> Result<Vec<LocatedSubglyph<'a>>, NoSuchGlyphName> {
        fn collect<'a>(font: &'a Font, g: &'a Subglyph, roff: i32, coff: i32,
                       acc: &mut Vec<LocatedSubglyph<'a>>,
                       negated: usize) -> Result<(), NoSuchGlyphName> {
            match g.data {
                Some(SubglyphData::Pixels(ref px)) => {
                    acc.push(LocatedSubglyph {
                        top: g.top + roff,
                        left: g.left + coff,
                        height: g.height,
                        width: g.width,
                        stride: g.stride.expect("SubglyphData::Pixels with no stride"),
                        data: px,
                        negated: g.negated + negated,
                    });
                }

                Some(SubglyphData::Named(ref name)) => {
                    let gg = font.get_glyph(name)?;
                    let top = g.top + roff;
                    let left = g.left + coff;
                    let negated = g.negated + negated;
                    for g2 in &gg.subglyphs {
                        collect(font, g2, top, left, acc, negated)?;
                    }
                }

                Some(SubglyphData::Adjoin(_)) => panic!("unexpected SubglyphData::Adjoin"),
                None => panic!("unexpected None"),
            }

            Ok(())
        }

        let mut acc = Vec::new();
        let gg = self.get_glyph(name)?;
        for g in &gg.subglyphs {
            collect(self, g, gg.preferred_top, gg.preferred_left, &mut acc, 0)?;
        }
        Ok(acc)
    }
}


