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
//!   written down beside the characters; nothing reads it yet.
//!
//! Nothing in the built font depends on any of this: `prop` describes
//! characters for the human reading the editor and the sample, and the TTF is
//! byte-identical with or without it. Validation of the values lives in
//! [`crate::issues`]; this module only records what was written.

use std::collections::BTreeMap;

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
/// it back.
pub fn format_block_range(start: u32, end: u32) -> String {
    if start == end {
        format!("U+{start:04X}")
    } else {
        format!("U+{start:04X}..{end:04X}")
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

    #[test]
    fn block_ranges_parse_both_spellings_and_reject_a_backwards_one() {
        assert_eq!(parse_block_range("U+E000"), Some((0xE000, 0xE000)));
        assert_eq!(parse_block_range("u+e000..e0ff"), Some((0xE000, 0xE0FF)));
        assert_eq!(parse_block_range("U+E0FF..E000"), None);
        assert_eq!(parse_block_range("E000"), None);
    }
}
