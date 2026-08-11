//! The Unicode character properties the status bar shows next to a character
//! name — and the `prop` directives a source may use to state them for
//! characters the UCD says nothing useful about.
//!
//! Two places in the UI name a code point — the Ctrl+K popup
//! ([`crate::editor::codepoint_popup`]) and the specimen's hover status
//! ([`crate::specimen`]) — and both append the same brace group, so a character
//! reads the same way whichever one produced it:
//!
//! ```text
//! LATIN SMALL LETTER A {gc=Ll eaw=Na}
//! COMBINING ACUTE ACCENT {gc=Mn ccc=230 eaw=A}
//! ```
//!
//! The three properties are the ones that decide what a glyph in this font has
//! to be: `gc` says whether it is a mark (and so should carry no advance),
//! `ccc` says how a mark stacks, and `eaw` is what the term face's advance must
//! agree with (see `faces.rs`). `ccc` is omitted when it is 0, which is nearly
//! every character; the other two always appear.
//!
//! The property values come from `icu_properties`, pinned to an exact version
//! in `Cargo.toml` so the UCD version behind them is a deliberate choice rather
//! than whatever a `cargo update` resolves to. [`tests::data_is_unicode_17`]
//! fails if that version ever stops being Unicode 17.0.
//!
//! # `prop`: what the UCD cannot say
//!
//! A Private Use character has a name and properties only because a font
//! decided so — the UCD reports no name at all and `{gc=Co eaw=A}` for the
//! whole area. [`CharProps`] is the source's answer: the `prop` directives of
//! every document, collected into one lookup that the two status lines and the
//! `sample.html` tooltips ask instead of asking `unicode_names2`/`icu` alone.
//!
//! ```text
//! prop block `Unison Symbols` = U+F0000..F00FF
//! prop U+F0000 = `UNISON LOGO` gc So eaw W
//! prop U+F0010..F001F = `UNISON BOX DRAWING-($#F0010..F001F)` gc So eaw W  // …-F0010, …
//! prop U+F0020|U+F0021 = `UNISON (ALPHA|BETA)`
//! ```
//!
//! Three rules shape the model:
//!
//! - **A character line is a `map` line with properties instead of a glyph.**
//!   The left side is the same character spelling `map` takes — one character,
//!   a `U+XXXX..YYYY` range or a `|` list — and the name on the right is a
//!   [`crate::pattern`] expanded against it in lock-step, exactly as a `map`'s
//!   glyph name is. That is what states several characters at once: `($#…)`
//!   over the same range names each of them, and a list gives one name each.
//!   Nothing else in the format has to grow a second way to spell a range. The
//!   expanded name is upper-cased (ASCII only), since a character name is upper
//!   case and `($#…)` expands to the lower-case hex a glyph name wants.
//! - **An unstated property is not a property.** A line states only what it
//!   overrides; `gc`, `ccc` and `eaw` are each independent, and a code point
//!   covered by several lines takes each field from the *last* line that states
//!   it. So a block-wide `prop U+F0000..F0FFF gc So eaw W` followed by one
//!   `prop U+F0000 = \`UNISON LOGO\`` names one character without restating its
//!   properties.
//! - **`prop block` is a label, not a rule.** It records which area of the
//!   Private Use planes a source has claimed and for what, so the claim is
//!   written down beside the characters. [`BlockMap`] is what reads it: a stated
//!   block is one more block of the code space, overriding whatever UCD block
//!   its area falls in. What it does not do is populate that area — which
//!   Private Use characters exist is what the per-character lines say, one at a
//!   time ([`CharProps::is_assigned`]).
//!
//! Nothing in the built font depends on any of this: `prop` describes
//! characters for the human reading the editor and the sample, and the TTF is
//! byte-identical with or without it. Validation of the values lives in
//! [`crate::issues`]; this module only records what was written.

use std::collections::BTreeMap;
#[cfg(feature = "editor")]
use std::sync::OnceLock;

use crate::document::{Document, DocumentItem};
use crate::render::ttf_builder::expand_map_pairs;

/// The `General_Category` short names, as `gc` accepts them.
pub const GENERAL_CATEGORIES: [&str; 30] = [
    "Lu", "Ll", "Lt", "Lm", "Lo", "Mn", "Mc", "Me", "Nd", "Nl", "No", "Pc", "Pd", "Ps", "Pe", "Pi",
    "Pf", "Po", "Sm", "Sc", "Sk", "So", "Zs", "Zl", "Zp", "Cc", "Cf", "Cs", "Co", "Cn",
];

/// The `East_Asian_Width` short names, as `eaw` accepts them.
pub const EAST_ASIAN_WIDTHS: [&str; 6] = ["N", "Na", "A", "W", "F", "H"];

/// The properties one `prop` line states, beside the name it may also state.
/// Every field is optional and independent: `None` means "leave whatever was
/// there".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharPropValues {
    pub gc: Option<String>,
    pub ccc: Option<u8>,
    pub eaw: Option<String>,
}

impl CharPropValues {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Apply `other`'s stated fields on top of these, leaving the rest alone —
    /// the rule that lets a range line carry the properties and a single-code
    /// point line carry only the name.
    fn overlay(&mut self, other: &Self) {
        if other.gc.is_some() {
            self.gc = other.gc.clone();
        }
        if other.ccc.is_some() {
            self.ccc = other.ccc;
        }
        if other.eaw.is_some() {
            self.eaw = other.eaw.clone();
        }
    }
}

/// Everything the `prop` lines together say about one code point.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatedChar {
    pub name: Option<String>,
    pub values: CharPropValues,
}

/// Every `prop` line of a document set, expanded per code point.
///
/// Expansion happens once, here, so that a lookup is a map probe rather than a
/// walk over every line: the status bar asks this on the frame a character is
/// hovered. A line whose character spelling expands to nothing (a backwards
/// range, or one past [`crate::pattern::MAX_EXPANSION`]) contributes nothing,
/// and [`crate::issues`] is what says so out loud.
#[derive(Clone, Debug, Default)]
pub struct CharProps {
    by_cp: BTreeMap<u32, StatedChar>,
}

impl CharProps {
    pub fn collect(docs: &[&Document]) -> Self {
        // A name goes through the two steps a `map` target does, in the same
        // order: name parts and inline `($#…)` ranges are substituted first,
        // and what comes out is expanded in lock-step with the characters.
        let name_parts = crate::document::collect_name_parts(docs);
        let mut by_cp: BTreeMap<u32, StatedChar> = BTreeMap::new();
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::PropChar {
                    char_repr,
                    name,
                    values,
                    ..
                } = item
                else {
                    continue;
                };
                let name_pattern = name
                    .as_deref()
                    .map(|n| crate::pattern::substitute_name_parts(n, &name_parts))
                    .unwrap_or_default();
                for (cp, expanded_name) in expand_map_pairs(char_repr, &name_pattern) {
                    let entry = by_cp.entry(cp).or_default();
                    if name.is_some() && !expanded_name.is_empty() {
                        // A character name is upper case, and `($#…)` expands
                        // to lower-case hex — the case a *glyph* name wants.
                        // Upper-casing the whole expanded name, rather than
                        // the substituted digits alone, is what makes the two
                        // halves of `UNISON BOX-e000` agree; it is ASCII-only,
                        // so a name in another script passes through as
                        // written.
                        entry.name = Some(expanded_name.to_ascii_uppercase());
                    }
                    entry.values.overlay(values);
                }
            }
        }
        Self { by_cp }
    }

    /// Everything the `prop` lines state about `cp` — every field of every line
    /// that covers it, folded in source order.
    pub fn stated(&self, cp: u32) -> Option<&StatedChar> {
        self.by_cp.get(&cp)
    }

    /// The character name to show for `cp`: what a `prop` line states, else the
    /// Unicode name, else `None` for a code point neither names (an unassigned
    /// one, or a Private Use character no `prop` line covers).
    pub fn name(&self, cp: u32) -> Option<String> {
        if let Some(name) = self.stated(cp).and_then(|s| s.name.clone()) {
            return Some(name);
        }
        char::from_u32(cp)
            .and_then(unicode_names2::name)
            .map(|n| n.to_string())
    }

    /// Whether *this source* gives `cp` a character: its general category, the
    /// `prop` overrides applied, is anything other than `Cn`.
    ///
    /// Private Use is where the source's word replaces the UCD's rather than
    /// merely overriding it. The UCD assigns all 137,468 Private Use code
    /// points `Co` and describes none, which says they are available, not that
    /// any of them is a character; so there, a `prop` line is the only thing
    /// that can assign one, and a code point still `Cn` afterwards is one the
    /// source has no character for — inside a `prop block` claim as much as
    /// outside one, since a claim names an area without populating it.
    #[cfg(feature = "editor")]
    pub fn is_assigned(&self, cp: u32) -> bool {
        let stated = self.stated(cp);
        if let Some(gc) = stated.and_then(|s| s.values.gc.as_deref()) {
            return gc != "Cn";
        }
        if is_private_use(cp) {
            // A `prop` line with no `gc` of its own still states the character;
            // its category then comes from the UCD, i.e. `Co`.
            return stated.is_some();
        }
        is_assigned(cp)
    }

    /// The brace group for `ch`, e.g. `{gc=Lo eaw=W}`, with any `prop`
    /// overrides applied. See [`property_summary`] for the shape of it.
    #[cfg(feature = "editor")]
    pub fn property_summary(&self, ch: char) -> String {
        let Some(stated) = self.stated(ch as u32).map(|s| &s.values) else {
            return property_summary(ch);
        };
        if stated.is_empty() {
            return property_summary(ch);
        }
        let base = base_properties(ch);
        format_properties(
            stated.gc.as_deref().unwrap_or(base.0),
            stated.ccc.unwrap_or(base.1),
            stated.eaw.as_deref().unwrap_or(base.2),
        )
    }
}

/// Parse a `prop block` code point range: `U+XXXX` or `U+XXXX..YYYY`. Returns
/// the inclusive `(start, end)`; `None` for anything else, a backwards range
/// included.
///
/// A *character* line takes the full `map` character spelling instead (see
/// [`expand_map_pairs`]) — a block is one contiguous area by definition, so it
/// takes the narrower form that says exactly that.
pub fn parse_block_range(s: &str) -> Option<(u32, u32)> {
    let hex = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))?;
    let (start, end) = match hex.split_once("..") {
        Some((a, b)) => (
            u32::from_str_radix(a, 16).ok()?,
            u32::from_str_radix(b, 16).ok()?,
        ),
        None => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            (v, v)
        }
    };
    (end >= start).then_some((start, end))
}

/// Serialize an inclusive code point range the way [`parse_block_range`] reads
/// it back. Only a `prop block` written back out needs it, which is the editor.
#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
pub fn format_block_range(start: u32, end: u32) -> String {
    if start == end {
        format!("U+{start:04X}")
    } else {
        format!("U+{start:04X}..{end:04X}")
    }
}

/// `Blocks.txt` of the pinned UCD version, bundled rather than read from
/// `data/` at run time: the editor groups the specimen by block, and a panel
/// that silently loses its headings when the working directory is elsewhere is
/// worse than 11 KB in the binary. Keep the version in step with the
/// `icu_properties` pin ([`tests::data_is_unicode_17`]).
#[cfg(feature = "editor")]
const BLOCKS_TXT: &str = include_str!("../data/Blocks-17.0.0.txt");

/// One block of the code space: an inclusive range with a name.
#[cfg(feature = "editor")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInfo<'a> {
    pub start: u32,
    pub end: u32,
    pub name: &'a str,
}

/// The UCD blocks, parsed once. Sorted and non-overlapping by construction of
/// the file, so a lookup is a binary search.
#[cfg(feature = "editor")]
fn ucd_blocks() -> &'static [(u32, u32, &'static str)] {
    static BLOCKS: OnceLock<Vec<(u32, u32, &'static str)>> = OnceLock::new();
    BLOCKS.get_or_init(|| {
        let mut out = Vec::new();
        for line in BLOCKS_TXT.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((range, name)) = line.split_once(';') else {
                continue;
            };
            if let Some((start, end)) = parse_block_range(&format!("U+{}", range.trim())) {
                out.push((start, end, name.trim()));
            }
        }
        out.sort_by_key(|b| b.0);
        out
    })
}

/// Which block every code point belongs to: the UCD's blocks with the source's
/// own `prop block` claims laid over them.
///
/// A stated block wins over the UCD block it sits inside — that is the whole
/// point of stating one, since the UCD calls the entire area "Private Use Area"
/// and says nothing about what a font put there. Two stated blocks that overlap
/// are resolved last-wins, in source order, like every other `prop` field.
#[cfg(feature = "editor")]
#[derive(Clone, Debug, Default)]
pub struct BlockMap {
    /// `prop block` claims in source order; searched back to front.
    stated: Vec<(u32, u32, String)>,
}

#[cfg(feature = "editor")]
impl BlockMap {
    pub fn collect(docs: &[&Document]) -> Self {
        let mut stated = Vec::new();
        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::PropBlock {
                    name, start, end, ..
                } = item
                {
                    stated.push((*start, *end, name.clone()));
                }
            }
        }
        Self { stated }
    }

    /// The block `cp` belongs to, or `None` for a code point no block covers
    /// (the UCD leaves gaps between blocks).
    pub fn block_of(&self, cp: u32) -> Option<BlockInfo<'_>> {
        if let Some((start, end, name)) = self
            .stated
            .iter()
            .rev()
            .find(|(start, end, _)| (*start..=*end).contains(&cp))
        {
            return Some(BlockInfo {
                start: *start,
                end: *end,
                name,
            });
        }
        let blocks = ucd_blocks();
        let idx = blocks
            .partition_point(|(start, _, _)| *start <= cp)
            .checked_sub(1)?;
        let (start, end, name) = blocks[idx];
        (cp <= end).then_some(BlockInfo { start, end, name })
    }
}

#[cfg(feature = "editor")]
fn base_properties(ch: char) -> (&'static str, u8, &'static str) {
    use icu_properties::CodePointMapData;
    use icu_properties::PropertyNamesShort;
    use icu_properties::props::{CanonicalCombiningClass, EastAsianWidth, GeneralCategory};

    let gc = CodePointMapData::<GeneralCategory>::new().get(ch);
    let gc_name = PropertyNamesShort::<GeneralCategory>::new()
        .get(gc)
        .unwrap_or("?");
    let eaw = CodePointMapData::<EastAsianWidth>::new().get(ch);
    let eaw_name = PropertyNamesShort::<EastAsianWidth>::new()
        .get(eaw)
        .unwrap_or("?");
    let ccc = CodePointMapData::<CanonicalCombiningClass>::new()
        .get(ch)
        .to_icu4c_value();
    (gc_name, ccc, eaw_name)
}

#[cfg(feature = "editor")]
fn format_properties(gc: &str, ccc: u8, eaw: &str) -> String {
    if ccc == 0 {
        format!("{{gc={gc} eaw={eaw}}}")
    } else {
        format!("{{gc={gc} ccc={ccc} eaw={eaw}}}")
    }
}

/// The brace group for `ch`, e.g. `{gc=Lo eaw=W}`, as the UCD alone reports it.
///
/// Every code point has all three properties — an unassigned one included, as
/// `{gc=Cn eaw=N}` — so this never returns an empty string. Callers that have a
/// document set should ask [`CharProps::property_summary`] instead, so a `prop`
/// line is not silently ignored.
#[cfg(feature = "editor")]
pub(crate) fn property_summary(ch: char) -> String {
    let (gc, ccc, eaw) = base_properties(ch);
    format_properties(gc, ccc, eaw)
}

/// Whether the UCD gives `cp` a character at all — `gc` other than `Cn`.
///
/// A surrogate code point is `Cs` rather than `Cn`, but it is not a `char` and
/// nothing can draw it, so it counts as unassigned here. This is what keeps the
/// specimen's "show undeclared characters" from filling a block's permanent
/// holes and its unused tail with cells.
#[cfg(feature = "editor")]
pub(crate) fn is_assigned(cp: u32) -> bool {
    use icu_properties::CodePointMapData;
    use icu_properties::props::GeneralCategory;

    char::from_u32(cp).is_some_and(|ch| {
        CodePointMapData::<GeneralCategory>::new().get(ch) != GeneralCategory::Unassigned
    })
}

/// Whether `cp` is a Private Use code point (`gc=Co`).
///
/// The UCD assigns every one of the 137,468 of them, and says nothing else about
/// any: which of them exist is a thing only a font and its `prop` lines know.
/// That is why the specimen fills a Private Use block from what the source
/// states rather than from [`is_assigned`], which would call the whole plane
/// present.
#[cfg(feature = "editor")]
pub(crate) fn is_private_use(cp: u32) -> bool {
    use icu_properties::CodePointMapData;
    use icu_properties::props::GeneralCategory;

    char::from_u32(cp).is_some_and(|ch| {
        CodePointMapData::<GeneralCategory>::new().get(ch) == GeneralCategory::PrivateUse
    })
}

/// Whether `cp` is a variation selector — the second half of a Unicode
/// variation sequence, and the only thing the second half of a `map` pair may
/// be.
///
/// Deliberately not `#[cfg(feature = "editor")]` and deliberately not asked of
/// the UCD tables: this decides how a source line parses, so the headless build
/// needs it and it must not move when the pinned UCD version does.
///
/// The Mongolian selectors are in the set because the *shaper's* definition has
/// them. HarfBuzz reads a variation sequence out of exactly these ranges in its
/// normalizer, before GSUB runs, so a pair written outside them would never
/// reach the cmap format 14 lookup it was stated for — the set that matters
/// here is the one the consumer uses, not the one the UCD publishes. `U+180F`
/// joined it in Unicode 14; a shaper old enough to stop at `U+180D` only loses
/// a pair that nothing else would have matched either.
pub fn is_variation_selector(cp: u32) -> bool {
    matches!(
        cp,
        0x180B..=0x180D | 0x180F | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(src: &str) -> CharProps {
        let doc = crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap();
        CharProps::collect(&[&doc])
    }

    #[cfg(feature = "editor")]
    #[test]
    fn ccc_is_shown_only_when_nonzero() {
        assert_eq!(property_summary('a'), "{gc=Ll eaw=Na}");
        // U+0301 COMBINING ACUTE ACCENT: the one property that is usually 0.
        assert_eq!(property_summary('\u{0301}'), "{gc=Mn ccc=230 eaw=A}");
    }

    #[cfg(feature = "editor")]
    #[test]
    fn east_asian_width_covers_the_widths_the_term_face_cares_about() {
        assert_eq!(property_summary('한'), "{gc=Lo eaw=W}");
        assert_eq!(property_summary('ﾊ'), "{gc=Lo eaw=H}");
        assert_eq!(property_summary('Ａ'), "{gc=Lu eaw=F}");
        // U+2160 ROMAN NUMERAL ONE — East_Asian_Width Ambiguous.
        assert_eq!(property_summary('\u{2160}'), "{gc=Nl eaw=A}");
        assert_eq!(property_summary('\u{2190}'), "{gc=Sm eaw=A}");
    }

    #[cfg(feature = "editor")]
    #[test]
    fn unassigned_code_points_still_report_something() {
        // U+0378 is a permanent hole in the Greek block.
        assert_eq!(property_summary('\u{0378}'), "{gc=Cn eaw=N}");
    }

    /// Guards the exact `icu_properties` pin in `Cargo.toml`: the values above
    /// are only as good as the UCD version baked into it, and a bump that
    /// silently changes the Unicode version should be a visible decision.
    #[cfg(feature = "editor")]
    #[test]
    fn data_is_unicode_17() {
        // U+323B0, the first of CJK Unified Ideographs Extension J — assigned
        // in Unicode 17.0 and unassigned in 16.0.
        assert_eq!(property_summary('\u{323B0}'), "{gc=Lo eaw=W}");
        // U+1E5D0, Tolong Siki — assigned in 16.0, so a *downgrade* past 16
        // fails here rather than passing the check above by accident.
        assert_eq!(property_summary('\u{1E5D0}'), "{gc=Lo eaw=N}");
    }

    #[test]
    fn a_prop_line_names_a_private_use_character() {
        let p = props("prop U+E000 = `UNISON LOGO` gc So eaw W\n");
        assert_eq!(p.name(0xE000).as_deref(), Some("UNISON LOGO"));
        // Untouched neighbours keep whatever the UCD says, which for a Private
        // Use code point is no name at all.
        assert_eq!(p.name(0xE001), None);
        assert_eq!(p.name(0x41).as_deref(), Some("LATIN CAPITAL LETTER A"));
    }

    /// The point of expanding the name: one line, one name per character.
    /// `($#…)` expands to lower-case hex, and the name comes out upper case
    /// anyway — a character name is upper case whichever half produced it.
    #[test]
    fn the_name_expands_in_lock_step_with_the_characters() {
        let p = props("prop U+E000..E002 = `UNISON BOX-($#E000..E002)` gc So\n");
        assert_eq!(p.name(0xE000).as_deref(), Some("UNISON BOX-E000"));
        assert_eq!(p.name(0xE002).as_deref(), Some("UNISON BOX-E002"));
        assert_eq!(p.name(0xE003), None);
        // Every character of the range gets the properties, expansion or not.
        assert_eq!(p.stated(0xE001).unwrap().values.gc.as_deref(), Some("So"));

        // A list on both sides pairs up one-to-one.
        let p = props("prop U+E010|U+E011 = `UNISON (ALPHA|BETA)`\n");
        assert_eq!(p.name(0xE010).as_deref(), Some("UNISON ALPHA"));
        assert_eq!(p.name(0xE011).as_deref(), Some("UNISON BETA"));

        // One name over a range is that name for each — the `map` rule, where
        // a shorter target list cycles.
        let p = props("prop U+E020..E021 = `UNISON MARKER`\n");
        assert_eq!(p.name(0xE021).as_deref(), Some("UNISON MARKER"));

        // Upper-casing is ASCII-only, so a name in another script is left as
        // it was written.
        let p = props("prop U+E030 = `unison 로고`\n");
        assert_eq!(p.name(0xE030).as_deref(), Some("UNISON 로고"));
    }

    #[test]
    fn a_later_line_overrides_only_the_fields_it_states() {
        let p = props(concat!(
            "prop U+E000..E0FF gc So eaw W\n",
            "prop U+E000 = `UNISON LOGO`\n",
            "prop U+E000 eaw N\n",
        ));
        let s = p.stated(0xE000).unwrap();
        assert_eq!(s.name.as_deref(), Some("UNISON LOGO"));
        assert_eq!(s.values.gc.as_deref(), Some("So"));
        assert_eq!(s.values.eaw.as_deref(), Some("N"));
        assert_eq!(s.values.ccc, None);
        // The rest of the range kept the properties and gained no name.
        let s = p.stated(0xE0FF).unwrap();
        assert_eq!(s.name, None);
        assert_eq!(s.values.eaw.as_deref(), Some("W"));
    }

    #[cfg(feature = "editor")]
    #[test]
    fn stated_properties_replace_the_ucd_ones_field_by_field() {
        let p = props("prop U+E000 = `UNISON LOGO` gc So eaw W\n");
        // Without `prop`, a Private Use code point reports `{gc=Co eaw=A}`.
        assert_eq!(property_summary('\u{E000}'), "{gc=Co eaw=A}");
        assert_eq!(p.property_summary('\u{E000}'), "{gc=So eaw=W}");
        // A line that states nothing about a character leaves the UCD alone.
        assert_eq!(p.property_summary('a'), "{gc=Ll eaw=Na}");
        // `ccc` alone still reports the real `gc`/`eaw` beside it.
        let p = props("prop U+E000 ccc 230\n");
        assert_eq!(p.property_summary('\u{E000}'), "{gc=Co ccc=230 eaw=A}");
    }

    #[cfg(feature = "editor")]
    fn blocks(src: &str) -> BlockMap {
        let doc = crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap();
        BlockMap::collect(&[&doc])
    }

    #[cfg(feature = "editor")]
    #[test]
    fn the_bundled_blocks_file_covers_the_code_space_it_should() {
        let m = BlockMap::default();
        let b = m.block_of(0x41).unwrap();
        assert_eq!((b.start, b.end, b.name), (0x0000, 0x007F, "Basic Latin"));
        let b = m.block_of(0xAC00).unwrap();
        assert_eq!(b.name, "Hangul Syllables");
        // The last code point of the last block, and the block boundaries
        // around a gap the UCD leaves unassigned to any block.
        assert_eq!(
            m.block_of(0x10FFFF).unwrap().name,
            "Supplementary Private Use Area-B"
        );
        assert_eq!(m.block_of(0x2FE0), None);
        assert_eq!(m.block_of(0x2FDF).unwrap().name, "Kangxi Radicals");
    }

    /// The point of `prop block`: the UCD calls all of U+F0000..FFFFD "Private
    /// Use Area", so only the source can say what a font put in there.
    #[cfg(feature = "editor")]
    #[test]
    fn a_stated_block_overrides_the_ucd_one_for_its_range_only() {
        let m = blocks("prop block `Unison Symbols` = U+F0000..F00FF\n");
        let b = m.block_of(0xF0010).unwrap();
        assert_eq!(
            (b.start, b.end, b.name),
            (0xF0000, 0xF00FF, "Unison Symbols")
        );
        // One past the claim is the plain UCD block again.
        assert_eq!(
            m.block_of(0xF0100).unwrap().name,
            "Supplementary Private Use Area-A"
        );
        // And a claim changes nothing outside the Private Use planes.
        assert_eq!(m.block_of(0x41).unwrap().name, "Basic Latin");

        // Two claims over one code point: the later line wins.
        let m = blocks(concat!(
            "prop block `First` = U+E000..E0FF\n",
            "prop block `Second` = U+E080..E0FF\n",
        ));
        assert_eq!(m.block_of(0xE000).unwrap().name, "First");
        assert_eq!(m.block_of(0xE080).unwrap().name, "Second");
    }

    #[cfg(feature = "editor")]
    #[test]
    fn only_assigned_code_points_count_as_assigned() {
        assert!(is_assigned(0x41));
        // U+0378: a permanent hole inside Basic Greek.
        assert!(!is_assigned(0x378));
        // A surrogate is not a `char`, and a noncharacter is `Cn`.
        assert!(!is_assigned(0xD800));
        assert!(!is_assigned(0xFFFE));
        // Private Use is `Co` — assigned, whether a `prop` line names it or not.
        assert!(is_assigned(0xE000));
    }

    #[test]
    fn block_ranges_parse_both_spellings_and_reject_a_backwards_one() {
        assert_eq!(parse_block_range("U+E000"), Some((0xE000, 0xE000)));
        assert_eq!(parse_block_range("u+e000..e0ff"), Some((0xE000, 0xE0FF)));
        assert_eq!(parse_block_range("U+E0FF..E000"), None);
        assert_eq!(parse_block_range("E000"), None);
    }
}
