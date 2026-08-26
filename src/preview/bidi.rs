//! Bidirectional reordering (UAX #9) for the preview field.
//!
//! # Why this is a *backend's* concern and not the shaper's
//!
//! A shaper takes a run that is already one direction and returns glyphs for
//! it; deciding *which* stretches of a paragraph are which direction, and what
//! order they end up in on screen, is the layer above. The preview has three
//! backends, and they sit at different heights in their platform's stack:
//! CoreText's [`CTLine`] does the whole of UAX #9 itself, DirectWrite's
//! `IDWriteTextAnalyzer` exposes `AnalyzeBidi` for it, and rustybuzz is a bare
//! shaper with nothing above it. Resolving the levels *here*, in the shared
//! path, and handing every backend single-direction runs would therefore
//! replace exactly the part of CoreText and DirectWrite the preview exists to
//! proof — the preview's whole point is to show what each platform actually
//! does with the built font. So this module is what the *rustybuzz* backend
//! reaches for, and each platform backend uses its own instead.
//!
//! [`CTLine`]: crate::preview::coretext
//!
//! # Where the Bidi_Class table comes from
//!
//! `unicode-bidi` carries its own copy of Bidi_Class under its default
//! `hardcoded-data` feature. `icu_properties` is already linked in for
//! [`crate::ucd`], and it implements `unicode_bidi::BidiDataSource` for its own
//! `BidiClass` map, so the feature is off and the table is borrowed from ICU4X
//! rather than duplicated. This is also why `icu_properties` carries the
//! `unicode_bidi` feature in `Cargo.toml`.
//!
//! # What this does not do
//!
//! Rule L3 (combining marks) and rule L4 (mirroring) are left to the shaper, as
//! UAX #9 intends. rustybuzz mirrors on its own — for a backward run it swaps
//! in the `Bidi_Mirroring_Glyph` code point when the face has a glyph for it,
//! and otherwise sets the `rtlm` mask so the font's own feature can do it. So a
//! run reaching the shaper with `Direction::RightToLeft` already gets L4.

use std::ops::Range;

use unicode_bidi::{Level, ParagraphBidiInfo};

/// The paragraph embedding level a line is laid out under.
///
/// `Auto` is UAX #9's P2/P3 — the first strong character decides — which is
/// what a browser does with `dir="auto"` and what an editor should default to.
/// The two explicit arms exist because proofing a font means being able to see
/// a string under a direction its own characters would not have picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
// The two explicit arms are what the preview's direction control will pick;
// nothing constructs them until it exists.
#[cfg_attr(not(test), expect(dead_code))]
pub enum ParagraphDirection {
    #[default]
    Auto,
    Ltr,
    Rtl,
}

impl ParagraphDirection {
    fn level(self) -> Option<Level> {
        match self {
            ParagraphDirection::Auto => None,
            ParagraphDirection::Ltr => Some(Level::ltr()),
            ParagraphDirection::Rtl => Some(Level::rtl()),
        }
    }
}

/// One maximal stretch of a line at a single embedding level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BidiRun {
    /// Byte range within the line.
    pub bytes: Range<usize>,
    /// Char index of the run's first character within the line, for rebasing
    /// per-run cluster indices back onto the whole line.
    pub char_start: usize,
    /// The resolved embedding level. Odd is right-to-left.
    pub level: u8,
}

impl BidiRun {
    pub fn is_rtl(&self) -> bool {
        self.level % 2 == 1
    }
}

/// Split one line into level runs, **in visual order** — that is, the order
/// they are to be painted left to right, which rule L2 has already applied.
///
/// The line is treated as a whole paragraph: the preview's text model is a
/// `Vec<DocLine>` and a hard line break there *is* a paragraph break, so there
/// is no case where one paragraph spans two of these calls. That also makes
/// rule L1 — trailing whitespace reset to the paragraph level — land where it
/// should with no extra work.
///
/// Returns an empty vector for empty input; otherwise the runs tile the whole
/// line, with no gaps, once put back in logical order.
pub fn split_bidi_runs(text: &str, direction: ParagraphDirection) -> Vec<BidiRun> {
    if text.is_empty() {
        return Vec::new();
    }

    let classes = icu_properties::CodePointMapData::<icu_properties::props::BidiClass>::new();
    let info = ParagraphBidiInfo::new_with_data_source(&classes, text, direction.level());
    let (levels, runs) = info.visual_runs(0..text.len());

    let byte_to_char = byte_to_char_starts(text);
    runs.into_iter()
        .map(|bytes| {
            // Every byte of a run carries the same level, so the first will do.
            let level = levels[bytes.start].number();
            BidiRun {
                char_start: byte_to_char[bytes.start],
                bytes,
                level,
            }
        })
        .collect()
}

/// Char index of the character each byte offset belongs to, plus one past the
/// end so a run ending at `text.len()` can be looked up too.
fn byte_to_char_starts(text: &str) -> Vec<usize> {
    let mut map = Vec::with_capacity(text.len() + 1);
    for (char_idx, ch) in text.chars().enumerate() {
        for _ in 0..ch.len_utf8() {
            map.push(char_idx);
        }
    }
    map.push(text.chars().count());
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(text, level)` per run, in the visual order the runs came back in.
    fn runs(text: &str, direction: ParagraphDirection) -> Vec<(&str, u8)> {
        split_bidi_runs(text, direction)
            .into_iter()
            .map(|r| (&text[r.bytes], r.level))
            .collect()
    }

    #[test]
    fn empty_text_has_no_runs() {
        assert!(split_bidi_runs("", ParagraphDirection::Auto).is_empty());
    }

    #[test]
    fn plain_latin_is_one_left_to_right_run() {
        assert_eq!(runs("abc", ParagraphDirection::Auto), vec![("abc", 0)]);
    }

    #[test]
    fn plain_hebrew_is_one_right_to_left_run() {
        assert_eq!(runs("שלום", ParagraphDirection::Auto), vec![("שלום", 1)]);
    }

    /// P2/P3: the first strong character sets the paragraph level, so the same
    /// characters lay out differently depending on which comes first.
    #[test]
    fn the_first_strong_character_picks_the_paragraph_level() {
        assert_eq!(
            runs("a שלום", ParagraphDirection::Auto),
            vec![("a ", 0), ("שלום", 1)],
        );
        // Hebrew first: the paragraph is RTL, so the Latin run is the embedded
        // one and sits to the *left* of the Hebrew that precedes it logically.
        assert_eq!(
            runs("שלום a", ParagraphDirection::Auto),
            vec![("a", 2), ("שלום ", 1)],
        );
    }

    /// An explicit direction overrides P2/P3, which is the whole reason the
    /// preview offers one.
    #[test]
    fn an_explicit_direction_overrides_the_first_strong_character() {
        // The paragraph is RTL, so the neutral space between the two joins
        // the Hebrew (N2), and the Latin island is painted leftmost.
        assert_eq!(
            runs("a שלום", ParagraphDirection::Rtl),
            vec![(" שלום", 1), ("a", 2)],
        );
        // ...and mirror-image: forced LTR gives the space to the Latin side.
        assert_eq!(
            runs("שלום a", ParagraphDirection::Ltr),
            vec![("שלום", 1), (" a", 0)],
        );
    }

    /// W2/W7 and rule I1: digits after Hebrew resolve to EN and take level 2,
    /// an *even* level nested inside an odd one — the case that makes "one
    /// level of each" the wrong mental model.
    #[test]
    fn digits_inside_hebrew_take_an_even_level_of_their_own() {
        // Visual order, so the *last* logical run is leftmost.
        assert_eq!(
            runs("שלום 42 שלום", ParagraphDirection::Auto),
            vec![(" שלום", 1), ("42", 2), ("שלום ", 1)],
        );
    }

    #[test]
    fn runs_tile_the_whole_line() {
        let text = "abc שלום 42 def";
        let mut spans: Vec<Range<usize>> = split_bidi_runs(text, ParagraphDirection::Auto)
            .into_iter()
            .map(|r| r.bytes)
            .collect();
        spans.sort_by_key(|r| r.start);
        assert_eq!(spans.first().map(|r| r.start), Some(0));
        assert_eq!(spans.last().map(|r| r.end), Some(text.len()));
        for pair in spans.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
    }

    #[test]
    fn char_start_is_the_runs_first_character_in_the_whole_line() {
        let text = "שלום a";
        let runs = split_bidi_runs(text, ParagraphDirection::Auto);
        // Visual order puts the Latin run first; it is char 5 logically.
        assert_eq!(runs[0].char_start, 5);
        assert_eq!(runs[1].char_start, 0);
    }

    /// Isolates (U+2066..U+2069) are the part of UAX #9 that postdates the
    /// embeddings most implementations were written against; they must resolve,
    /// not be treated as ordinary neutrals.
    #[test]
    fn isolates_keep_their_content_from_leaking_into_the_paragraph() {
        // FSI ... PDI around Hebrew inside an LTR paragraph: the Hebrew is one
        // RTL island and the trailing Latin stays at the paragraph level.
        let text = "a \u{2068}שלום\u{2069} b";
        let levels: Vec<u8> = split_bidi_runs(text, ParagraphDirection::Auto)
            .into_iter()
            .map(|r| r.level)
            .collect();
        assert!(levels.contains(&1), "the isolated Hebrew must be RTL");
        assert_eq!(levels.first(), Some(&0));
        assert_eq!(levels.last(), Some(&0));
    }
}
