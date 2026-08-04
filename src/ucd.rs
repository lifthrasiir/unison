//! The Unicode character properties the status bar shows next to a character
//! name.
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

use icu_properties::CodePointMapData;
use icu_properties::PropertyNamesShort;
use icu_properties::props::{CanonicalCombiningClass, EastAsianWidth, GeneralCategory};

/// The brace group for `ch`, e.g. `{gc=Lo eaw=W}`.
///
/// Every code point has all three properties — an unassigned one included, as
/// `{gc=Cn eaw=N}` — so this never returns an empty string.
pub(crate) fn property_summary(ch: char) -> String {
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

    if ccc == 0 {
        format!("{{gc={gc_name} eaw={eaw_name}}}")
    } else {
        format!("{{gc={gc_name} ccc={ccc} eaw={eaw_name}}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ccc_is_shown_only_when_nonzero() {
        assert_eq!(property_summary('a'), "{gc=Ll eaw=Na}");
        // U+0301 COMBINING ACUTE ACCENT: the one property that is usually 0.
        assert_eq!(property_summary('\u{0301}'), "{gc=Mn ccc=230 eaw=A}");
    }

    #[test]
    fn east_asian_width_covers_the_widths_the_term_face_cares_about() {
        assert_eq!(property_summary('한'), "{gc=Lo eaw=W}");
        assert_eq!(property_summary('ﾊ'), "{gc=Lo eaw=H}");
        assert_eq!(property_summary('Ａ'), "{gc=Lu eaw=F}");
        // U+2160 ROMAN NUMERAL ONE — Ambiguous, the class PLAN.md's A1 is about.
        assert_eq!(property_summary('\u{2160}'), "{gc=Nl eaw=A}");
        assert_eq!(property_summary('\u{2190}'), "{gc=Sm eaw=A}");
    }

    #[test]
    fn unassigned_code_points_still_report_something() {
        // U+0378 is a permanent hole in the Greek block.
        assert_eq!(property_summary('\u{0378}'), "{gc=Cn eaw=N}");
    }

    /// Guards the exact `icu_properties` pin in `Cargo.toml`: the values above
    /// are only as good as the UCD version baked into it, and a bump that
    /// silently changes the Unicode version should be a visible decision.
    #[test]
    fn data_is_unicode_17() {
        // U+323B0, the first of CJK Unified Ideographs Extension J — assigned
        // in Unicode 17.0 and unassigned in 16.0.
        assert_eq!(property_summary('\u{323B0}'), "{gc=Lo eaw=W}");
        // U+1E5D0, Tolong Siki — assigned in 16.0, so a *downgrade* past 16
        // fails here rather than passing the check above by accident.
        assert_eq!(property_summary('\u{1E5D0}'), "{gc=Lo eaw=N}");
    }
}
