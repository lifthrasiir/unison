//! Which numbers on a `.unf` text line the grammar reads as *signed*.
//!
//! [`line_fields`](super::line_fields) knows where names live; this knows the
//! one thing about numbers that the editor's Alt + wheel / Alt + Up/Down
//! gesture needs, which is whether stepping one below zero writes something
//! the parser reads as meant. Everything not listed here is treated as
//! unsigned, and the gesture stops at zero there as it always has: a `-` in
//! front of a size is not a value but a different token (`glyph foo -1 2`
//! does not parse, and `-2x3` is another on-demand name than `2x3`).
//!
//! A position is signed only where a negative number is a meaningful value,
//! not merely where the parser happens to read an `i16`:
//!
//! | Where | Why negative |
//! | --- | --- |
//! | `ref NAME X Y` | a bearing, preserved as one |
//! | `anchor ±NAME X[..X2] Y[..Y2]` | a site outside the grid; each end of a range |
//! | `glyph ... origin C R` | the box starts left of / above the grid |
//! | a split's (⿰⿱⿲⿳) gaps, nested pieces included | overlapping parts |
//! | the pixel-grid `meta` keys | below the baseline, a leftward caret |
//! | `audit ideal-clearance` bands | a clearance is negative when parts overlap |
//! | `assert shape ... offset X Y` | a mark placed left of / above its base |
//!
//! An enclosure's `P Q` reads as `i16` too but is deliberately absent: it is
//! the inner part's offset *inside* the walls, and a negative one puts it
//! outside the box it is being measured against.

use std::ops::Range;

use crate::document_io::{TokenSpan, tokenize_with_spans};

/// The `meta` keys whose every value is a signed pixel amount or ratio; see
/// [`crate::meta::parse_meta_entry`].
const SIGNED_META_KEYS: [&str; 9] = [
    "line-gap",
    "x-height",
    "cap-height",
    "caret-offset",
    "underline-at",
    "strikeout-at",
    "caret-slope",
    "subscript-at",
    "superscript-at",
];

/// Whether `s` is written as a (possibly negative) decimal integer.
fn is_signed_integer(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The char columns of every signed number on `line`, sign included, in line
/// order. Only what is written as a number now is listed: a position that is
/// signed but holds something else has no number to step.
pub(crate) fn signed_numbers(line: &str) -> Vec<Range<usize>> {
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();
    let Ok(spans) = tokenize_with_spans(trimmed) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // A quoted token's raw span does not line up with its value, and nobody
    // quotes a number; such a token is left alone.
    let mut push = |span: &TokenSpan, from: usize, text: &str| {
        let unquoted = span.raw_end - span.raw_start == span.value.chars().count();
        if unquoted && is_signed_integer(text) {
            let start = leading + span.raw_start + from;
            out.push(start..start + text.chars().count());
        }
    };
    let Some((keyword, rest)) = spans.split_first() else {
        return out;
    };

    if let Some((op, assumed)) =
        crate::compose::IdcOp::of_line(spans.iter().map(|s| s.value.as_str()))
    {
        if op.walls().is_some() {
            return out;
        }
        let items = if assumed { &rest[1..] } else { rest };
        for span in items {
            if span.value.chars().count() > 1 && crate::pattern::has_top_level_pipe(&span.value) {
                let mut from = 0;
                for piece in crate::pattern::split_top_level_pipes(&span.value) {
                    push(span, from, piece);
                    from += piece.chars().count() + 1;
                }
            } else {
                push(span, 0, &span.value);
            }
        }
        return out;
    }

    let mut push_whole = |span: Option<&TokenSpan>| {
        if let Some(span) = span {
            push(span, 0, &span.value);
        }
    };
    match keyword.value.as_str() {
        "ref" => rest
            .iter()
            .skip(1)
            .take(2)
            .for_each(|s| push_whole(Some(s))),
        "anchor" => {
            for span in rest.iter().skip(1).take(2) {
                match span.value.split_once("..") {
                    Some((lo, hi)) => {
                        push(span, 0, lo);
                        push(span, lo.chars().count() + 2, hi);
                    }
                    None => push(span, 0, &span.value),
                }
            }
        }
        "glyph" => {
            if let Some(at) = rest.iter().skip(1).position(|s| s.value == "origin") {
                rest.iter()
                    .skip(at + 2)
                    .take(2)
                    .for_each(|s| push_whole(Some(s)));
            }
        }
        "meta" => {
            let rest = match rest {
                [_, colon, rest @ ..] if colon.value == ":" => rest,
                _ => rest,
            };
            if let Some((key, values)) = rest.split_first()
                && SIGNED_META_KEYS.contains(&key.value.as_str())
            {
                values.iter().for_each(|s| push_whole(Some(s)));
            }
        }
        "audit" => {
            if let [key, _prefix, values @ ..] = rest
                && key.value == "ideal-clearance"
            {
                values.iter().for_each(|s| push_whole(Some(s)));
            }
        }
        "assert" if rest.first().is_some_and(|s| s.value == "shape") => {
            // `offset` is only a keyword in the glyph segments, after the
            // first bare `:`; before it, it would be the text being shaped.
            let Some(colon) = rest.iter().position(|s| s.value == ":") else {
                return out;
            };
            let segs = &rest[colon + 1..];
            for (i, span) in segs.iter().enumerate() {
                if span.value == "offset" {
                    segs.iter()
                        .skip(i + 1)
                        .take(2)
                        .for_each(|s| push_whole(Some(s)));
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers `signed_numbers` finds, as the text they cover.
    fn found(line: &str) -> Vec<String> {
        signed_numbers(line)
            .into_iter()
            .map(|r| line.chars().skip(r.start).take(r.len()).collect())
            .collect()
    }

    #[test]
    fn a_ref_offset_is_signed_and_its_name_is_not() {
        assert_eq!(found("ref sp-2x3 1 -2 negated"), ["1", "-2"]);
        assert_eq!(found("  ref sp 0 4 // 5"), ["0", "4"]);
        assert_eq!(found("ref sp fill #000000"), Vec::<String>::new());
    }

    #[test]
    fn each_end_of_an_anchor_range_is_signed() {
        assert_eq!(found("anchor +above 1..3 -2"), ["1", "3", "-2"]);
        assert_eq!(found("anchor -x -3..-1 0"), ["-3", "-1", "0"]);
        // The columns of the second end skip the `..`.
        assert_eq!(signed_numbers("anchor -x -3..-1 0")[1], 14..16);
    }

    #[test]
    fn only_a_glyph_headers_origin_is_signed() {
        assert_eq!(found("glyph foo 8 16 origin -1 2 advance 7"), ["-1", "2"]);
        assert_eq!(found("glyph foo 8 16"), Vec::<String>::new());
        // A glyph named `origin` is a name, not the flag.
        assert_eq!(found("glyph origin 3 4"), Vec::<String>::new());
    }

    #[test]
    fn a_splits_gaps_are_signed_and_an_enclosures_offsets_are_not() {
        assert_eq!(found("⿰ a:4x16 -1 b:12x16 1"), ["-1", "1"]);
        assert_eq!(found("assume ⿳ a 2|b:11x3|-1 1 c"), ["2", "-1", "1"]);
        assert_eq!(signed_numbers("⿱ 1|b|2")[1], 6..7);
        // `(a|1)` is one pattern, not a nested split.
        assert_eq!(found("⿰ (a|1) b"), Vec::<String>::new());
        assert_eq!(found("⿴ a b 3 2"), Vec::<String>::new());
    }

    #[test]
    fn meta_audit_and_assert_numbers_are_signed_by_key() {
        assert_eq!(found("meta underline-at -2 1"), ["-2", "1"]);
        assert_eq!(found("meta term : caret-offset 0"), ["0"]);
        assert_eq!(found("meta height 16"), Vec::<String>::new());
        assert_eq!(
            found("meta panose 2 0 0 0 0 0 0 0 0 0"),
            Vec::<String>::new()
        );
        assert_eq!(
            found("audit ideal-clearance han- 0 2 -1 3"),
            ["0", "2", "-1", "3"]
        );
        assert_eq!(found("audit max-contact-run han- 3"), Vec::<String>::new());
        assert_eq!(
            found("assert shape offset : a advance 8 : b offset -4 0"),
            ["-4", "0"]
        );
    }
}
