//! The `meta` directive: font metadata, and the OpenType fields it feeds.
//!
//! # One key per line
//!
//! `meta [FACE :] KEY VALUE...` carries exactly one key. Keys are variadic — a
//! metric takes one number, `panose` takes ten, a flag takes none — so two keys
//! on one line could not be told apart without a separator.
//!
//! The optional scope is told from the key by the second token being a bare
//! `:`, exactly as a slice qualifier is (see [`crate::faces`]). A bare key
//! applies to every face; `* : KEY` is an explicit spelling of the same thing.
//! The design metrics — `height`, `ascent`, `descent` — may only be stated for
//! every face: they fix how the pixel grid maps onto the em, and every face
//! draws from one glyph set.
//!
//! # Everything is single-assignment
//!
//! Setting a key twice is an error even when the two values agree, and so is
//! setting the same slot through two spellings (`family` and `name 1` are one
//! slot). There is deliberately no precedence rule to appeal to: `meta` has no
//! override mechanism, because a silent override is exactly how a font ends up
//! shipping a value nobody meant to set. [`crate::issues`] reports the
//! conflicts; this module only decides what a line *means*.
//!
//! Scopes do not soften that. A bare key reaches every face, so stating a key
//! bare *and* for a face gives that face two values and is a conflict — the
//! same shape as a face including two slices that map one character. A value
//! that varies per face is stated once per face, never as a base plus an
//! exception.
//!
//! # Declared, derived, and computed
//!
//! Three different things, kept apart on purpose:
//!
//! - **Declared** — what a `meta` line states. Absent means absent, which is
//!   why every field is an `Option`: validation has to tell "not stated" from
//!   "stated as the default".
//! - **Derived** — name IDs 3, 4, 5 and 6, which convention builds out of
//!   family, subfamily and revision. Declaring them explicitly (`name 6 ...`)
//!   wins; otherwise [`FontMeta::name_records`] fills them in. They are emitted
//!   for en-US only: name ID 6 in particular is required to be English, and a
//!   localized PostScript name is not a thing.
//! - **Computed** — what only the built font knows (`ulUnicodeRange` from the
//!   cmap, `xAvgCharWidth` from the metrics). Those never come from `meta`.
//!
//! # Language slot
//!
//! Every string key takes an optional `@LANG` BCP 47 tag before its value, so
//! `meta family @ko-KR ...` is a localized name record. Tags are mapped to the
//! Windows language IDs that platform 3 name records are keyed by, through
//! [`WINDOWS_LANGUAGES`]; a tag with no mapping is an error rather than a
//! silently dropped record. Only platform 3 records are emitted, so `@LANG` is
//! the only way to localize at all — platform 0 records have no language slot.

use std::collections::BTreeMap;

use crate::document::{Document, DocumentItem};
use crate::resolve::ItemRef;

/// The Windows language ID a name record without `@LANG` is filed under.
pub const LANG_EN_US: u16 = 0x0409;

/// BCP 47 tag to Windows language ID, for platform 3 name records.
///
/// Not exhaustive — the registry has hundreds of entries and most would never
/// be used here. An unlisted tag is an error, so the failure mode of this list
/// being short is a clear message, not a wrong font.
pub const WINDOWS_LANGUAGES: &[(&str, u16)] = &[
    ("ar-SA", 0x0401),
    ("cs-CZ", 0x0405),
    ("da-DK", 0x0406),
    ("de-DE", 0x0407),
    ("el-GR", 0x0408),
    ("en-GB", 0x0809),
    ("en-US", 0x0409),
    ("es-ES", 0x0C0A),
    ("fi-FI", 0x040B),
    ("fr-FR", 0x040C),
    ("he-IL", 0x040D),
    ("hu-HU", 0x040E),
    ("it-IT", 0x0410),
    ("ja-JP", 0x0411),
    ("ko-KR", 0x0412),
    ("nb-NO", 0x0414),
    ("nl-NL", 0x0413),
    ("pl-PL", 0x0415),
    ("pt-BR", 0x0416),
    ("pt-PT", 0x0816),
    ("ru-RU", 0x0419),
    ("sv-SE", 0x041D),
    ("th-TH", 0x041E),
    ("tr-TR", 0x041F),
    ("uk-UA", 0x0422),
    ("vi-VN", 0x042A),
    ("zh-CN", 0x0804),
    ("zh-HK", 0x0C04),
    ("zh-TW", 0x0404),
];

fn windows_language(tag: &str) -> Option<u16> {
    WINDOWS_LANGUAGES
        .iter()
        .find(|(t, _)| t.eq_ignore_ascii_case(tag))
        .map(|&(_, id)| id)
}

fn language_tag(id: u16) -> &'static str {
    WINDOWS_LANGUAGES
        .iter()
        .find(|&&(_, i)| i == id)
        .map(|&(t, _)| t)
        .unwrap_or("?")
}

/// The string keys, and the name ID each one is a spelling of.
///
/// A key and its `name ID` form are the same slot, so declaring both is a
/// conflict — which is the reason this is a table rather than a match arm per
/// key.
pub const NAME_KEYS: &[(&str, u16)] = &[
    ("copyright", 0),
    ("family", 1),
    ("subfamily", 2),
    ("version-text", 5),
    ("trademark", 7),
    ("manufacturer", 8),
    ("designer", 9),
    ("description", 10),
    ("vendor-url", 11),
    ("designer-url", 12),
    ("license", 13),
    ("license-url", 14),
    ("family-text", 16),
    ("subfamily-text", 17),
    ("sample-text", 19),
];

/// One parsed `meta` line.
///
/// The variant *is* the key, so a key that parses is a key some consumer
/// handles. [`FontMeta::collect`] and [`crate::issues`] both go through
/// [`parse_meta_entry`] rather than walking tokens themselves, so what counts as
/// a valid line is decided in exactly one place.
#[derive(Clone, Debug, PartialEq)]
pub enum MetaEntry {
    Height(u16),
    Ascent(u16),
    Descent(u16),
    /// A value on the pixel grid, scaled to font units by the same
    /// `UNITS_PER_EM / height` the outlines are.
    Pixels(PixelKey, i16),
    /// `head.fontRevision`, and the default for name ID 5.
    Revision(f64),
    /// `OS/2.achVendID`, and part of the derived unique ID.
    VendorId(String),
    Weight(u16),
    Width(u16),
    FsType(u16),
    /// `subscript-at`/`superscript-at`: x size, y size, x offset, y offset.
    Script(ScriptKey, [i16; 4]),
    /// `underline-at POS THICKNESS` and `strikeout-at SIZE POS` set two fields
    /// each, so they are one entry rather than two.
    UnderlineAt {
        position: i16,
        thickness: i16,
    },
    StrikeoutAt {
        size: i16,
        position: i16,
    },
    Panose([u8; 10]),
    /// `caret-slope RISE RUN` — a ratio, so unlike everything else here it is
    /// not on the pixel grid and is not scaled.
    CaretSlope(i16, i16),
    /// `head.created`/`head.modified`, as seconds since the 1904 epoch.
    Created(i64),
    Flag(StyleFlag),
    /// Any name record, however it was spelled.
    Name {
        id: u16,
        lang: u16,
        text: String,
    },
}

/// The pixel-grid values, each `KEY N`, except `underline-at`/`strikeout-at`
/// which take two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PixelKey {
    LineGap,
    XHeight,
    CapHeight,
    CaretOffset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKey {
    Subscript,
    Superscript,
}

/// The valueless keys. Most are `OS/2.fsSelection` bits; `Bold` and `Italic`
/// are also `head.macStyle` bits, and `FixedPitch` is `post.isFixedPitch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StyleFlag {
    Italic,
    Underscore,
    Negative,
    Outlined,
    Shadow,
    Strikeout,
    Bold,
    Oblique,
    UseTypoMetrics,
    FixedPitch,
}

pub const STYLE_FLAGS: &[(&str, StyleFlag)] = &[
    ("italic", StyleFlag::Italic),
    ("underscore", StyleFlag::Underscore),
    ("negative", StyleFlag::Negative),
    ("outlined", StyleFlag::Outlined),
    ("shadow", StyleFlag::Shadow),
    ("strikeout", StyleFlag::Strikeout),
    ("bold", StyleFlag::Bold),
    ("oblique", StyleFlag::Oblique),
    ("use-typo-metrics", StyleFlag::UseTypoMetrics),
    ("fixed-pitch", StyleFlag::FixedPitch),
];

impl MetaEntry {
    /// The slot this line assigns to, as duplicate detection sees it. Two
    /// spellings of one slot produce the same string.
    pub fn slot(&self) -> String {
        match self {
            Self::Height(_) => "height".to_string(),
            Self::Ascent(_) => "ascent".to_string(),
            Self::Descent(_) => "descent".to_string(),
            Self::Pixels(k, _) => match k {
                PixelKey::LineGap => "line-gap",
                PixelKey::XHeight => "x-height",
                PixelKey::CapHeight => "cap-height",
                PixelKey::CaretOffset => "caret-offset",
            }
            .to_string(),
            Self::Revision(_) => "revision".to_string(),
            Self::VendorId(_) => "vendor-id".to_string(),
            Self::Weight(_) => "weight".to_string(),
            Self::Width(_) => "width".to_string(),
            Self::FsType(_) => "fs-type".to_string(),
            Self::Script(ScriptKey::Subscript, _) => "subscript-at".to_string(),
            Self::Script(ScriptKey::Superscript, _) => "superscript-at".to_string(),
            Self::UnderlineAt { .. } => "underline-at".to_string(),
            Self::StrikeoutAt { .. } => "strikeout-at".to_string(),
            Self::Panose(_) => "panose".to_string(),
            Self::CaretSlope(..) => "caret-slope".to_string(),
            Self::Created(_) => "created".to_string(),
            Self::Flag(f) => STYLE_FLAGS
                .iter()
                .find(|(_, g)| g == f)
                .map(|(k, _)| (*k).to_string())
                .unwrap_or_default(),
            Self::Name { id, lang, .. } => format!("name {id} @{}", language_tag(*lang)),
        }
    }

    /// How the slot is worth naming in a diagnostic. A name slot says what it
    /// is rather than quoting a key, since the conflict may well be between two
    /// different spellings.
    pub fn describe_slot(&self) -> String {
        match self {
            Self::Name { id, lang, .. } => {
                format!("name ID {id} ({})", language_tag(*lang))
            }
            other => format!("`meta {}`", other.slot()),
        }
    }
}

/// Every key, sorted, for the unknown-key message.
fn known_keys() -> String {
    let mut known: Vec<&str> = vec![
        "height",
        "ascent",
        "descent",
        "line-gap",
        "x-height",
        "cap-height",
        "caret-offset",
        "caret-slope",
        "underline-at",
        "strikeout-at",
        "subscript-at",
        "superscript-at",
        "weight",
        "width",
        "fs-type",
        "panose",
        "created",
        "revision",
        "vendor-id",
        "name",
    ];
    known.extend(NAME_KEYS.iter().map(|(k, _)| *k));
    known.extend(STYLE_FLAGS.iter().map(|(k, _)| *k));
    known.sort_unstable();
    known.join(", ")
}

/// Days since 1970-01-01 for a proleptic Gregorian date, by Howard Hinnant's
/// `days_from_civil`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Days between the TrueType epoch (1904-01-01) and the Unix epoch.
const DAYS_1904_TO_1970: i64 = 24107;

/// `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`, to seconds since 1904-01-01.
///
/// Hand-parsed rather than pulled in with a date crate: it is one field in one
/// fixed format, and `head.created` is a `LONGDATETIME` counted from 1904, so
/// a general-purpose library would only be doing this same arithmetic.
fn parse_timestamp(s: &str) -> Result<i64, String> {
    let bad = || format!("`meta created` takes YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ, got `{s}`");
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, t.strip_suffix('Z').ok_or_else(bad)?),
        None => (s, "00:00:00"),
    };
    let num = |x: &str, width: usize| -> Result<i64, String> {
        if x.len() != width || !x.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        x.parse().map_err(|_| bad())
    };
    let dp: Vec<&str> = date.split('-').collect();
    let tp: Vec<&str> = time.split(':').collect();
    if dp.len() != 3 || tp.len() != 3 {
        return Err(bad());
    }
    let (y, mo, d) = (num(dp[0], 4)?, num(dp[1], 2)?, num(dp[2], 2)?);
    let (h, mi, sec) = (num(tp[0], 2)?, num(tp[1], 2)?, num(tp[2], 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return Err(bad());
    }
    let days = days_from_civil(y, mo, d) + DAYS_1904_TO_1970;
    Ok(days * 86400 + h * 3600 + mi * 60 + sec)
}

/// Parse the text of a `meta` item — everything after the keyword, comment
/// included, exactly as [`DocumentItem::Meta`] stores it.
///
/// `Err` carries a message ready to report; the caller supplies the position. A
/// key that takes N values rejects N±1, because a `meta` value is invisible in
/// the built font: a line quietly half-read is a mistake that ships.
pub fn parse_meta_entry(text: &str) -> Result<(Option<String>, MetaEntry), String> {
    let tokens = crate::document_io::tokenize_tokens(text)
        .map_err(|e| format!("malformed `meta` line: {e}"))?;

    // The scope comes off the front exactly as a slice qualifier does. `*` is
    // an explicit spelling of "every face"; a bare key means the same thing.
    let (scope, tokens) = match crate::document::DocumentItem::split_qualifier_token(&tokens) {
        (Some(face), rest) if face == "*" => (None, rest.to_vec()),
        (Some(face), rest) => (Some(face), rest.to_vec()),
        (None, rest) => (None, rest.to_vec()),
    };

    let Some((key, rest)) = tokens.split_first() else {
        return Err("`meta` needs a key".to_string());
    };

    // The design metrics fix how the pixel grid maps onto the em, and every
    // face shares one glyph set — so they cannot differ per face.
    if scope.is_some() && matches!(key.as_str(), "height" | "ascent" | "descent") {
        return Err(format!(
            "`meta {key}` applies to every face and cannot be face-scoped: it fixes \
             how the pixel grid maps onto the em, which the whole glyph set shares",
        ));
    }

    let one_number = |rest: &[String]| -> Result<u16, String> {
        match rest {
            [v] => v
                .parse::<u16>()
                .map_err(|_| format!("`meta {key}` takes a number, got `{v}`")),
            _ => Err(format!(
                "`meta {key}` takes exactly 1 value, got {}",
                rest.len(),
            )),
        }
    };

    // Signed, because a position below the baseline is the normal case for
    // `underline-at` and a negative `caret-offset` is legal.
    let nums = |rest: &[String], want: usize| -> Result<Vec<i16>, String> {
        if rest.len() != want {
            return Err(format!(
                "`meta {key}` takes exactly {want} value(s), got {}",
                rest.len(),
            ));
        }
        rest.iter()
            .map(|v| {
                v.parse::<i16>()
                    .map_err(|_| format!("`meta {key}` takes numbers, got `{v}`"))
            })
            .collect()
    };
    let bounded = |rest: &[String], lo: u16, hi: u16| -> Result<u16, String> {
        let v = one_number(rest)?;
        if !(lo..=hi).contains(&v) {
            return Err(format!("`meta {key}` must be {lo}..={hi}, got {v}"));
        }
        Ok(v)
    };

    if let Some(&(_, flag)) = STYLE_FLAGS.iter().find(|(k, _)| *k == key) {
        if !rest.is_empty() {
            return Err(format!("`meta {key}` is a flag and takes no value"));
        }
        return Ok((scope, MetaEntry::Flag(flag)));
    }

    match key.as_str() {
        "height" => return Ok((scope, MetaEntry::Height(one_number(rest)?))),
        "ascent" => return Ok((scope, MetaEntry::Ascent(one_number(rest)?))),
        "descent" => return Ok((scope, MetaEntry::Descent(one_number(rest)?))),
        "line-gap" => {
            return Ok((
                scope,
                MetaEntry::Pixels(PixelKey::LineGap, nums(rest, 1)?[0]),
            ));
        }
        "x-height" => {
            return Ok((
                scope,
                MetaEntry::Pixels(PixelKey::XHeight, nums(rest, 1)?[0]),
            ));
        }
        "cap-height" => {
            return Ok((
                scope,
                MetaEntry::Pixels(PixelKey::CapHeight, nums(rest, 1)?[0]),
            ));
        }
        "caret-offset" => {
            return Ok((
                scope,
                MetaEntry::Pixels(PixelKey::CaretOffset, nums(rest, 1)?[0]),
            ));
        }
        "underline-at" => {
            let v = nums(rest, 2)?;
            return Ok((
                scope,
                MetaEntry::UnderlineAt {
                    position: v[0],
                    thickness: v[1],
                },
            ));
        }
        "strikeout-at" => {
            let v = nums(rest, 2)?;
            return Ok((
                scope,
                MetaEntry::StrikeoutAt {
                    size: v[0],
                    position: v[1],
                },
            ));
        }
        "caret-slope" => {
            let v = nums(rest, 2)?;
            if v[0] == 0 && v[1] == 0 {
                return Err("`meta caret-slope` cannot be 0 0".to_string());
            }
            return Ok((scope, MetaEntry::CaretSlope(v[0], v[1])));
        }
        "subscript-at" | "superscript-at" => {
            let v = nums(rest, 4)?;
            let which = if key == "subscript-at" {
                ScriptKey::Subscript
            } else {
                ScriptKey::Superscript
            };
            return Ok((scope, MetaEntry::Script(which, [v[0], v[1], v[2], v[3]])));
        }
        // usWeightClass and usWidthClass have defined ranges; a value outside
        // them is ignored by consumers rather than clamped, so it is an error.
        "weight" => return Ok((scope, MetaEntry::Weight(bounded(rest, 1, 1000)?))),
        "width" => return Ok((scope, MetaEntry::Width(bounded(rest, 1, 9)?))),
        "fs-type" => return Ok((scope, MetaEntry::FsType(one_number(rest)?))),
        "panose" => {
            if rest.len() != 10 {
                return Err(format!(
                    "`meta panose` takes exactly 10 values, got {}",
                    rest.len(),
                ));
            }
            let mut out = [0u8; 10];
            for (slot, v) in out.iter_mut().zip(rest) {
                *slot = v
                    .parse::<u8>()
                    .map_err(|_| format!("`meta panose` takes 10 numbers 0..=255, got `{v}`"))?;
            }
            return Ok((scope, MetaEntry::Panose(out)));
        }
        "created" => {
            let [v] = rest else {
                return Err(format!(
                    "`meta created` takes exactly 1 value, got {}",
                    rest.len(),
                ));
            };
            return parse_timestamp(v).map(|t| (scope, MetaEntry::Created(t)));
        }
        "revision" => {
            let [v] = rest else {
                return Err(format!(
                    "`meta revision` takes exactly 1 value, got {}",
                    rest.len(),
                ));
            };
            let n: f64 = v
                .parse()
                .map_err(|_| format!("`meta revision` takes a number, got `{v}`"))?;
            if !n.is_finite() || n <= 0.0 {
                return Err(format!("`meta revision` must be positive, got `{v}`"));
            }
            return Ok((scope, MetaEntry::Revision(n)));
        }
        "vendor-id" => {
            let [v] = rest else {
                return Err(format!(
                    "`meta vendor-id` takes exactly 1 value, got {}",
                    rest.len(),
                ));
            };
            // achVendID is a 4-byte tag; a longer or non-ASCII value would be
            // truncated into something that identifies nobody.
            if v.is_empty() || v.len() > 4 || !v.bytes().all(|b| (0x20..0x7F).contains(&b)) {
                return Err(format!(
                    "`meta vendor-id` takes 1 to 4 printable ASCII characters, got `{v}`",
                ));
            }
            return Ok((scope, MetaEntry::VendorId(v.clone())));
        }
        _ => {}
    }

    // From here on, everything is a name record: `KEY [@LANG] TEXT` or
    // `name ID [@LANG] TEXT`.
    let (id, rest) = if key == "name" {
        let Some((id_tok, rest)) = rest.split_first() else {
            return Err("`meta name` takes a name ID and a value".to_string());
        };
        let id: u16 = id_tok
            .parse()
            .map_err(|_| format!("`meta name` takes a numeric name ID, got `{id_tok}`"))?;
        (id, rest)
    } else if let Some(&(_, id)) = NAME_KEYS.iter().find(|(k, _)| *k == key) {
        (id, rest)
    } else {
        return Err(format!(
            "unknown `meta` key `{key}` (known keys: {})",
            known_keys()
        ));
    };

    let (lang, rest) = match rest.split_first() {
        Some((tok, tail)) if tok.starts_with('@') => {
            let tag = &tok[1..];
            let Some(lang) = windows_language(tag) else {
                return Err(format!(
                    "`meta {key}` has an unmapped language tag `@{tag}`; \
                     platform 3 name records are keyed by Windows language ID, \
                     and only these tags are mapped: {}",
                    WINDOWS_LANGUAGES
                        .iter()
                        .map(|(t, _)| *t)
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            };
            (lang, tail)
        }
        _ => (LANG_EN_US, rest),
    };

    match rest {
        [text] => Ok((
            scope,
            MetaEntry::Name {
                id,
                lang,
                text: text.clone(),
            },
        )),
        [] => Err(format!("`meta {key}` takes a value")),
        _ => Err(format!(
            "`meta {key}` takes exactly 1 value, got {} — quote a value \
             containing spaces with backticks",
            rest.len(),
        )),
    }
}

/// The design metrics, which are global to the font rather than per-face: they
/// fix how a pixel grid maps onto the em, so the whole glyph set shares them.
///
/// Split out of [`FontMeta`] because the editor needs exactly these, every
/// frame, and copying them is free.
#[derive(Clone, Copy, Debug, Default)]
pub struct FontMetrics {
    pub height: Option<u16>,
    pub ascent: Option<u16>,
    pub descent: Option<u16>,
}

impl FontMetrics {
    pub const DEFAULT_HEIGHT: u16 = 16;
    pub const DEFAULT_ASCENT: u16 = 14;
    pub const DEFAULT_DESCENT: u16 = 2;

    pub fn height(&self) -> u16 {
        self.height.unwrap_or(Self::DEFAULT_HEIGHT)
    }

    pub fn ascent(&self) -> u16 {
        self.ascent.unwrap_or(Self::DEFAULT_ASCENT)
    }

    pub fn descent(&self) -> u16 {
        self.descent.unwrap_or(Self::DEFAULT_DESCENT)
    }
}

/// The `meta` values a document set declares.
///
/// Malformed and duplicate lines are *not* reported here: this runs on every
/// editor frame, while reporting belongs to [`crate::issues`]. Both sides share
/// [`parse_meta_entry`], so they cannot disagree about what a line means; a line
/// that fails to parse is skipped, leaving the default in place.
#[derive(Clone, Debug, Default)]
pub struct FontMeta {
    pub metrics: FontMetrics,
    pub revision: Option<f64>,
    pub vendor_id: Option<String>,
    /// Pixel-grid values, unscaled. The builder scales them, because only it
    /// knows the scale the outlines were built at.
    pub pixels: BTreeMap<PixelKey, i16>,
    pub underline: Option<(i16, i16)>,
    pub strikeout: Option<(i16, i16)>,
    pub subscript: Option<[i16; 4]>,
    pub superscript: Option<[i16; 4]>,
    pub weight: Option<u16>,
    pub width: Option<u16>,
    pub fs_type: Option<u16>,
    pub panose: Option<[u8; 10]>,
    pub caret_slope: Option<(i16, i16)>,
    pub created: Option<i64>,
    pub flags: std::collections::BTreeSet<StyleFlag>,
    /// Declared name records, keyed by `(name ID, Windows language ID)`.
    pub names: BTreeMap<(u16, u16), String>,
    /// The last `meta` line that set anything, for error reporting.
    pub origin: Option<ItemRef>,
}

/// Used when nothing declares a family, so that a font built from a fixture
/// still has a coherent name table. A real font is expected to say its name.
pub const DEFAULT_FAMILY: &str = "Untitled";
pub const DEFAULT_SUBFAMILY: &str = "Regular";
pub const DEFAULT_VENDOR_ID: &str = "NONE";

impl FontMeta {
    pub fn height(&self) -> u16 {
        self.metrics.height()
    }

    pub fn ascent(&self) -> u16 {
        self.metrics.ascent()
    }

    pub fn descent(&self) -> u16 {
        self.metrics.descent()
    }

    pub fn revision(&self) -> f64 {
        self.revision.unwrap_or(1.0)
    }

    pub fn vendor_id(&self) -> &str {
        self.vendor_id.as_deref().unwrap_or(DEFAULT_VENDOR_ID)
    }

    pub fn has(&self, flag: StyleFlag) -> bool {
        self.flags.contains(&flag)
    }

    /// A declared pixel value, in pixels.
    pub fn pixels(&self, key: PixelKey) -> Option<i16> {
        self.pixels.get(&key).copied()
    }

    /// `OS/2.fsSelection`.
    ///
    /// REGULAR is set only when nothing else claims a style: the format
    /// requires bit 6 to exclude bits 0, 1, 5 and 9, and a font asserting both
    /// bold and regular is a familiar and very visible bug.
    pub fn fs_selection(&self) -> u16 {
        let mut bits = 0u16;
        for (flag, bit) in [
            (StyleFlag::Italic, 0),
            (StyleFlag::Underscore, 1),
            (StyleFlag::Negative, 2),
            (StyleFlag::Outlined, 3),
            (StyleFlag::Strikeout, 4),
            (StyleFlag::Bold, 5),
            (StyleFlag::UseTypoMetrics, 7),
            (StyleFlag::Oblique, 9),
        ] {
            if self.has(flag) {
                bits |= 1 << bit;
            }
        }
        // `shadow` has no fsSelection bit; it lives in head.macStyle only.
        //
        // REGULAR excludes exactly ITALIC, BOLD and OBLIQUE. Not UNDERSCORE or
        // STRIKEOUT: those decorate a font that is still regular.
        const EXCLUSIVE: u16 = (1 << 0) | (1 << 5) | (1 << 9);
        if bits & EXCLUSIVE == 0 {
            bits |= 1 << 6; // REGULAR
        }
        bits
    }

    /// `head.macStyle`. Only the bits `meta` can state; the rest describe a
    /// width or slant this font has no way to declare yet.
    pub fn mac_style(&self) -> u16 {
        let mut bits = 0u16;
        if self.has(StyleFlag::Bold) {
            bits |= 1 << 0;
        }
        if self.has(StyleFlag::Italic) {
            bits |= 1 << 1;
        }
        if self.has(StyleFlag::Underscore) {
            bits |= 1 << 2;
        }
        if self.has(StyleFlag::Outlined) {
            bits |= 1 << 3;
        }
        if self.has(StyleFlag::Shadow) {
            bits |= 1 << 4;
        }
        bits
    }

    /// A declared name record in the given language, falling back to en-US —
    /// a localized font still has one family name everything else derives from.
    pub fn name(&self, id: u16, lang: u16) -> Option<&str> {
        self.names
            .get(&(id, lang))
            .or_else(|| self.names.get(&(id, LANG_EN_US)))
            .map(|s| s.as_str())
    }

    pub fn family(&self) -> &str {
        self.name(1, LANG_EN_US).unwrap_or(DEFAULT_FAMILY)
    }

    pub fn subfamily(&self) -> &str {
        self.name(2, LANG_EN_US).unwrap_or(DEFAULT_SUBFAMILY)
    }

    /// name ID 5. `Version X.YYY` is what every consumer expects to be able to
    /// parse out of it, so it is formatted from `revision` rather than being a
    /// second, independently drifting spelling of the same number.
    pub fn version_text(&self) -> String {
        self.name(5, LANG_EN_US)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Version {:.3}", self.revision()))
    }

    /// name ID 4. "Family Subfamily", with the subfamily dropped when it is the
    /// unremarkable `Regular`.
    pub fn full_name(&self) -> String {
        match self.name(4, LANG_EN_US) {
            Some(declared) => declared.to_string(),
            None if self.subfamily() == DEFAULT_SUBFAMILY => self.family().to_string(),
            None => format!("{} {}", self.family(), self.subfamily()),
        }
    }

    /// name ID 6. The charset is the restricted one PostScript allows, so the
    /// derived form is filtered rather than assumed clean; the result only has
    /// to be *valid*, not a pretty transliteration of an exotic family name.
    pub fn postscript_name(&self) -> String {
        if let Some(declared) = self.name(6, LANG_EN_US) {
            return declared.to_string();
        }
        let raw = if self.subfamily() == DEFAULT_SUBFAMILY {
            self.family().to_string()
        } else {
            format!("{}-{}", self.family(), self.subfamily())
        };
        let filtered: String = raw
            .chars()
            .filter(|c| {
                c.is_ascii_graphic()
                    && !matches!(c, '[' | ']' | '(' | ')' | '{' | '}' | '<' | '>' | '/' | '%')
            })
            .take(63)
            .collect();
        if filtered.is_empty() {
            DEFAULT_FAMILY.to_string()
        } else {
            filtered
        }
    }

    /// name ID 3, in the conventional `version;vendor;psname` shape.
    pub fn unique_id(&self) -> String {
        self.name(3, LANG_EN_US)
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "{};{};{}",
                    self.version_text(),
                    self.vendor_id(),
                    self.postscript_name(),
                )
            })
    }

    /// Every name record the font should carry: what was declared, plus the
    /// derived IDs 3, 4, 5 and 6 for en-US where they were not.
    ///
    /// Sorted by `(language, name ID)`, which is what the format requires:
    /// records go in platform, encoding, language, name ID order, and the first
    /// two are constant here because only platform 3 / encoding 1 is emitted.
    /// `write-fonts` validates this and refuses to build otherwise. It also
    /// makes the table byte-stable across builds.
    pub fn name_records(&self) -> Vec<(u16, u16, String)> {
        let mut out: BTreeMap<(u16, u16), String> = self.names.clone();
        out.entry((1, LANG_EN_US))
            .or_insert_with(|| self.family().to_string());
        out.entry((2, LANG_EN_US))
            .or_insert_with(|| self.subfamily().to_string());
        out.entry((3, LANG_EN_US))
            .or_insert_with(|| self.unique_id());
        out.entry((4, LANG_EN_US))
            .or_insert_with(|| self.full_name());
        out.entry((5, LANG_EN_US))
            .or_insert_with(|| self.version_text());
        out.entry((6, LANG_EN_US))
            .or_insert_with(|| self.postscript_name());
        let mut records: Vec<(u16, u16, String)> = out
            .into_iter()
            .map(|((id, lang), text)| (id, lang, text))
            .collect();
        records.sort_by_key(|&(id, lang, _)| (lang, id));
        records
    }

    /// Collect for the implicit face — the one a source declaring no `face`
    /// describes. Face-scoped entries never match it.
    pub fn collect(docs: &[&Document]) -> Self {
        Self::for_face(docs, None)
    }

    /// Collect for one face: bare entries plus the ones scoped to it.
    ///
    /// `face` is the id, or `None` for the implicit face. A bare entry applies
    /// to every face, which is why stating a key bare *and* for a face gives
    /// that face two values — reported as a conflict by [`crate::issues`],
    /// since there is no override rule here any more than there is in
    /// [`crate::faces`].
    pub fn for_face(docs: &[&Document], face: Option<&str>) -> Self {
        let mut meta = Self::default();
        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let DocumentItem::Meta(s) = item else {
                    continue;
                };
                let Ok((scope, entry)) = parse_meta_entry(s) else {
                    continue;
                };
                if let Some(scope) = &scope
                    && Some(scope.as_str()) != face
                {
                    continue;
                }
                meta.origin = Some(ItemRef::new(doc_idx, item_idx));
                match entry {
                    MetaEntry::Height(v) => meta.metrics.height = Some(v),
                    MetaEntry::Ascent(v) => meta.metrics.ascent = Some(v),
                    MetaEntry::Descent(v) => meta.metrics.descent = Some(v),
                    MetaEntry::Pixels(k, v) => {
                        meta.pixels.insert(k, v);
                    }
                    MetaEntry::UnderlineAt {
                        position,
                        thickness,
                    } => {
                        meta.underline = Some((position, thickness));
                    }
                    MetaEntry::StrikeoutAt { size, position } => {
                        meta.strikeout = Some((size, position));
                    }
                    MetaEntry::Script(ScriptKey::Subscript, v) => meta.subscript = Some(v),
                    MetaEntry::Script(ScriptKey::Superscript, v) => meta.superscript = Some(v),
                    MetaEntry::Weight(v) => meta.weight = Some(v),
                    MetaEntry::Width(v) => meta.width = Some(v),
                    MetaEntry::FsType(v) => meta.fs_type = Some(v),
                    MetaEntry::Panose(v) => meta.panose = Some(v),
                    MetaEntry::CaretSlope(rise, run) => meta.caret_slope = Some((rise, run)),
                    MetaEntry::Created(v) => meta.created = Some(v),
                    MetaEntry::Flag(f) => {
                        meta.flags.insert(f);
                    }
                    MetaEntry::Revision(v) => meta.revision = Some(v),
                    MetaEntry::VendorId(v) => meta.vendor_id = Some(v),
                    MetaEntry::Name { id, lang, text } => {
                        meta.names.insert((id, lang), text);
                    }
                }
            }
        }
        meta
    }
}

#[cfg(test)]
#[path = "meta_tests.rs"]
mod tests;
