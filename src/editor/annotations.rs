//! Inline annotations: display-only text spliced into a document line.
//!
//! An annotation is *not* part of the document buffer. It is drawn between two
//! document characters and is dimmed relative to the surrounding text, and the
//! caret steps over it in one go — the annotated character plus its annotation
//! behave as a single unit for hit-testing, caret placement and selection.
//!
//! For *soft wrapping* it is not a unit but ordinary text: the line breaks
//! wherever the rendered line overflows, annotation included, so a long
//! annotation wraps by itself instead of dragging the character it trails onto
//! the next line (`visual_lines::compute_wrap_segments`). The piece landing on
//! a later segment is an annotation at relative column 0 there.
//!
//! The producers are `map` and `assert shape`, which spell out the codepoints of
//! literally written text (`map 가 = ...` renders as `map 가 U+AC00 = ...`). Both
//! exist because a source may hold characters that cannot be seen: a variation
//! selector is invisible, so `map 0️ = ...` and `map 0 = ...` are the same line
//! on screen without one.
//!
//! New kinds plug into [`line_annotations`]; everything downstream — width
//! measurement, painting, hit-testing — is kind-agnostic and lives in
//! [`AnnotatedText`].

use crate::document_io::{TokenSpan, tokenize_with_spans};
use crate::pattern::has_top_level_pipe;

/// Display-only text inserted *after* document column `col` of a text line.
///
/// `col` is a character column, and on a whole document line it is always at
/// least 1: an annotation trails the character it describes, so the caret at
/// `col` sits past the annotation while the caret at `col - 1` sits before the
/// annotated character.
///
/// A soft wrap breaks the rendered line, annotations included, so a *wrapped
/// segment* may also carry `col == 0`: the tail of an annotation that began on
/// the previous visual line, drawn before every character of this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InlineAnnotation {
    pub col: usize,
    pub text: String,
}

/// Annotations for one document line, in ascending column order.
pub(crate) fn line_annotations(line: &str) -> Vec<InlineAnnotation> {
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();
    let spans = match tokenize_with_spans(trimmed) {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };

    let mut out = match spans[0].value.as_str() {
        "map" => map_annotations(&spans[1..], leading),
        "assert" => assert_annotations(&spans[1..], leading),
        _ => Vec::new(),
    };
    out.retain(|a| a.col >= 1);
    out.sort_by_key(|a| a.col);
    out
}

/// `map CHAR = GLYPH` / `map generate CHAR [= GLYPH]`: spell out the codepoint
/// of every literally written character. Tokens already in `U+XXXX` form
/// annotate nothing, and neither does a token that is not a mapping sequence
/// this format supports — see [`map_codepoint_annotation`].
fn map_annotations(rest: &[TokenSpan], leading: usize) -> Vec<InlineAnnotation> {
    // A `SLICE :` qualifier comes off first, exactly as the parser takes it
    // off, so a qualified mapping is annotated like any other.
    let rest = match rest {
        [slice, colon, tail @ ..] if colon.value == ":" && slice.value != ":" => tail,
        _ => rest,
    };
    // The arities are the parser's own, in the parser's order — a variation
    // sequence writes its base and its selector as two tokens, and *both* are
    // text worth spelling out (the selector especially, being invisible).
    let generate = rest.first().is_some_and(|s| s.value == "generate");
    let char_spans: &[TokenSpan] = match (generate, rest.len()) {
        (_, 3) if rest[1].value == "=" => &rest[0..1],
        (true, 2) => &rest[1..2],
        (true, 4) if rest[2].value == "=" => &rest[1..2],
        (true, 3) => &rest[1..3],
        (true, 5) if rest[3].value == "=" => &rest[1..3],
        (false, 4) if rest[2].value == "=" => &rest[0..2],
        _ => return Vec::new(),
    };

    let mut out = Vec::new();
    for (i, char_span) in char_spans.iter().enumerate() {
        // Two spans is the `BASE SELECTOR` form, so the second one stands in
        // the selector half; a lone span holds the whole sequence.
        let half = if char_spans.len() == 2 && i == 1 {
            CharHalf::Selector
        } else {
            CharHalf::Whole
        };
        let value = char_span.value.as_str();
        let quoted = char_span.raw_end - char_span.raw_start != value.chars().count();

        // A pipe list annotates each literal part in place; that is only
        // possible when the token is unquoted, since quoting shifts the inner
        // offsets.
        if !quoted && value.contains('|') {
            for (part, end_off) in split_top_level_pipes_with_ends(value) {
                if let Some(text) = map_codepoint_annotation(part, half) {
                    out.push(InlineAnnotation {
                        col: leading + char_span.raw_start + end_off,
                        text,
                    });
                }
            }
        } else if let Some(text) = map_codepoint_annotation(value, half) {
            out.push(InlineAnnotation {
                col: leading + char_span.raw_end,
                text,
            });
        }
    }
    out
}

/// Which half of a `map`'s character side a written token stands in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharHalf {
    /// The whole sequence: a base, optionally followed by its selector.
    Whole,
    /// The selector of the two-token `BASE SELECTOR` form.
    Selector,
}

/// [`codepoint_annotation`], but only for a token that is a mapping sequence
/// this format actually supports — `split_written_uvs_pair`'s shape.
///
/// Anything else on the character side is *not* text the font maps: a pattern
/// (`(a|b)c`), a longer paste that [`crate::issues`] rejects by name, a
/// selector where a base belongs. Spelling those out claims they are a
/// sequence, and on a slice-qualified line — where the eye already has a
/// qualifier, a pattern and a glyph name to sort out — the spurious `U+0028
/// U+0061 U+007C …` buried the line. So the annotation stops at what a `map`
/// can mean, and an unsupported form is left to the diagnostics report.
fn map_codepoint_annotation(s: &str, half: CharHalf) -> Option<String> {
    let mut chars = s.chars();
    let supported = match (chars.next(), chars.next(), chars.next()) {
        (Some(c), None, _) => {
            crate::ucd::is_variation_selector(c as u32) == (half == CharHalf::Selector)
        }
        (Some(base), Some(sel), None) => {
            half == CharHalf::Whole
                && !crate::ucd::is_variation_selector(base as u32)
                && crate::ucd::is_variation_selector(sel as u32)
        }
        _ => false,
    };
    supported.then(|| codepoint_annotation(s)).flatten()
}

/// `assert shape TEXT …`: spell out every codepoint of the shaped text.
///
/// The only other place a source states text, and the one where it can be long.
/// Nothing else on the line distinguishes a text-presentation sequence from an
/// emoji one, or a bare base from a base plus a selector — and an assertion
/// that shapes different text than its author thought fails for a reason that
/// is invisible in the file. `assert same`/`assert distinct` name glyphs, not
/// text, so they annotate nothing.
fn assert_annotations(rest: &[TokenSpan], leading: usize) -> Vec<InlineAnnotation> {
    let [kind, text, ..] = rest else {
        return Vec::new();
    };
    if kind.value != "shape" {
        return Vec::new();
    }
    match codepoint_annotation(text.value.as_str()) {
        Some(annotation) => vec![InlineAnnotation {
            col: leading + text.raw_end,
            text: annotation,
        }],
        None => Vec::new(),
    }
}

/// ` U+XXXX …` for literally written text, `None` for anything already written
/// as `U+XXXX`.
///
/// More than one character is spelled out in full rather than skipped, because
/// that is exactly the case worth reading: a variation sequence's second half
/// is invisible, and without this a `map 0️ = …` and a `map 0 = …` look the
/// same in the editor. *Which* multi-character tokens reach here is the
/// caller's business — a `map` filters to what it can mean, while an `assert
/// shape` states arbitrary text. A token holding a top-level pipe is left to the caller,
/// which annotates each alternative in place — the pipes are syntax, and
/// running them together would spell `U+007C` as if it were text.
fn codepoint_annotation(s: &str) -> Option<String> {
    if s.is_empty() || s.starts_with("U+") || s.starts_with("u+") || has_top_level_pipe(s) {
        return None;
    }
    let mut out = String::new();
    for c in s.chars() {
        out.push_str(&format!(" U+{:04X}", c as u32));
    }
    Some(out)
}

/// Splits on pipes outside parentheses, yielding each part with the character
/// offset just past it.
fn split_top_level_pipes_with_ends(s: &str) -> Vec<(&str, usize)> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start_byte = 0usize;
    for (char_idx, (byte_idx, c)) in s.char_indices().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 => {
                parts.push((&s[start_byte..byte_idx], char_idx));
                start_byte = byte_idx + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push((&s[start_byte..], s.chars().count()));
    parts
}

// ---------------------------------------------------------------------------
// Geometry / painting
// ---------------------------------------------------------------------------

/// One contiguous stretch of the rendered line.
pub(crate) struct Run {
    pub text: String,
    pub is_annotation: bool,
    /// Byte offset of this run within the full display string.
    pub display_start: usize,
}

/// A text line paired with the annotations rendered inside it. All public
/// column arguments and results are *document* columns of `text`; annotations
/// only affect the pixel geometry.
#[derive(Clone, Copy)]
pub(crate) struct AnnotatedText<'a> {
    text: &'a str,
    annotations: &'a [InlineAnnotation],
}

impl<'a> AnnotatedText<'a> {
    pub(crate) fn new(text: &'a str, annotations: &'a [InlineAnnotation]) -> Self {
        Self { text, annotations }
    }

    /// The underlying document text, without annotations.
    pub(crate) fn text(&self) -> &'a str {
        self.text
    }

    /// The line as drawn, annotations spliced in.
    pub(crate) fn display_string(&self) -> String {
        self.runs().into_iter().map(|r| r.text).collect()
    }

    /// Display text up to document column `col`, including the annotations
    /// that trail characters before `col`.
    pub(crate) fn display_prefix(&self, col: usize) -> String {
        let mut out = String::new();
        let mut ai = 0usize;
        // A leading fragment precedes every character here, so it is part of
        // even the empty prefix: column 0 sits after it.
        while self.annotations.get(ai).is_some_and(|a| a.col == 0) {
            out.push_str(&self.annotations[ai].text);
            ai += 1;
        }
        for (i, c) in self.text.chars().enumerate() {
            if i >= col {
                break;
            }
            out.push(c);
            while let Some(a) = self.annotations.get(ai) {
                if a.col <= i + 1 {
                    out.push_str(&a.text);
                    ai += 1;
                } else {
                    break;
                }
            }
        }
        out
    }

    pub(crate) fn runs(&self) -> Vec<Run> {
        if self.annotations.is_empty() {
            return vec![Run {
                text: self.text.to_string(),
                is_annotation: false,
                display_start: 0,
            }];
        }
        let mut runs: Vec<Run> = Vec::new();
        let mut display_len = 0usize;
        let mut pending = String::new();
        let mut ai = 0usize;
        let push = |runs: &mut Vec<Run>, display_len: &mut usize, text: String, is_ann: bool| {
            if text.is_empty() {
                return;
            }
            let start = *display_len;
            *display_len += text.len();
            runs.push(Run {
                text,
                is_annotation: is_ann,
                display_start: start,
            });
        };
        while self.annotations.get(ai).is_some_and(|a| a.col == 0) {
            push(
                &mut runs,
                &mut display_len,
                self.annotations[ai].text.clone(),
                true,
            );
            ai += 1;
        }
        for (i, c) in self.text.chars().enumerate() {
            pending.push(c);
            while let Some(a) = self.annotations.get(ai) {
                if a.col <= i + 1 {
                    push(
                        &mut runs,
                        &mut display_len,
                        std::mem::take(&mut pending),
                        false,
                    );
                    push(&mut runs, &mut display_len, a.text.clone(), true);
                    ai += 1;
                } else {
                    break;
                }
            }
        }
        push(&mut runs, &mut display_len, pending, false);
        runs
    }

    /// Pixel x of document column `col`, relative to the line's left edge.
    pub(crate) fn x_pos(&self, ui: &egui::Ui, font_id: &egui::FontId, col: usize) -> f32 {
        // Not short-circuited at `col == 0`: a segment starting mid-annotation
        // draws that tail before its first column.
        text_width(ui, font_id, &self.display_prefix(col))
    }

    /// Inverse of [`x_pos`]: the document column whose left edge is nearest to
    /// (but not past) `x`. Positions inside an annotation resolve to the
    /// column after it, so the caret never lands in the middle.
    pub(crate) fn x_to_col(&self, ui: &egui::Ui, font_id: &egui::FontId, x: f32) -> usize {
        if self.text.is_empty() || x <= 0.0 {
            return 0;
        }
        let char_count = self.text.chars().count();
        for col in 0..=char_count {
            if self.x_pos(ui, font_id, col) > x {
                return col.saturating_sub(1);
            }
        }
        char_count
    }

    /// Draws the line at `pos` (LEFT_TOP), annotations dimmed against `color`.
    ///
    /// `comment` is `(document column where the line's `// …` comment starts,
    /// the color to draw it in)`; everything from that column on is painted in
    /// the comment color instead of `color`.
    pub(crate) fn paint(
        &self,
        painter: &egui::Painter,
        ui: &egui::Ui,
        font_id: &egui::FontId,
        pos: egui::Pos2,
        color: egui::Color32,
        comment: Option<(usize, egui::Color32)>,
    ) {
        // Byte offset in the *display* string at which the comment starts.
        let split = comment.map(|(col, c)| (self.display_prefix(col).len(), c));

        if self.annotations.is_empty() && split.is_none() {
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                self.text,
                font_id.clone(),
                color,
            );
            return;
        }
        let display = self.display_string();
        let dim = color.gamma_multiply(ANNOTATION_OPACITY);
        for run in self.runs() {
            let base = if run.is_annotation { dim } else { color };
            let run_end = run.display_start + run.text.len();
            let commented = |c: egui::Color32| {
                if run.is_annotation {
                    c.gamma_multiply(ANNOTATION_OPACITY)
                } else {
                    c
                }
            };
            // A run straddling the comment boundary is drawn in two pieces so
            // the split lands exactly on the `//`.
            let pieces: Vec<(usize, &str, egui::Color32)> = match split {
                Some((at, ccolor)) if at > run.display_start && at < run_end => {
                    let cut = at - run.display_start;
                    vec![
                        (run.display_start, &run.text[..cut], base),
                        (at, &run.text[cut..], commented(ccolor)),
                    ]
                }
                Some((at, ccolor)) if at <= run.display_start => {
                    vec![(run.display_start, run.text.as_str(), commented(ccolor))]
                }
                _ => vec![(run.display_start, run.text.as_str(), base)],
            };
            for (start, text, c) in pieces {
                let x = text_width(ui, font_id, &display[..start]);
                painter.text(
                    egui::pos2(pos.x + x, pos.y),
                    egui::Align2::LEFT_TOP,
                    text,
                    font_id.clone(),
                    c,
                );
            }
        }
    }
}

/// How strongly annotations are dimmed against the text they trail.
pub(crate) const ANNOTATION_OPACITY: f32 = 0.5;

fn text_width(ui: &egui::Ui, font_id: &egui::FontId, s: &str) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    ui.fonts(|f| {
        f.layout_no_wrap(s.to_string(), font_id.clone(), egui::Color32::WHITE)
            .rect
            .width()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(line: &str) -> Vec<(usize, String)> {
        line_annotations(line)
            .into_iter()
            .map(|a| (a.col, a.text))
            .collect()
    }

    #[test]
    fn map_literal_char_is_annotated() {
        assert_eq!(ann("map 가 = hangul-ga"), vec![(5, " U+AC00".to_string())]);
        assert_eq!(ann("map A = latin-a"), vec![(5, " U+0041".to_string())]);
    }

    #[test]
    fn map_decomposed_form_is_annotated() {
        assert_eq!(ann("map generate 가"), vec![(14, " U+AC00".to_string())]);
        assert_eq!(
            ann("map generate 가 = hangul-ga"),
            vec![(14, " U+AC00".to_string())]
        );
        // The bare `map CHAR` form is gone; it is no longer a map at all.
        assert!(ann("map 가").is_empty());
    }

    /// A variation sequence written literally is one token holding two
    /// characters, the second of which is invisible. Spelling both out is the
    /// only way to read the line — and the reason the serializer is allowed to
    /// keep the pasted form.
    #[test]
    fn a_literal_variation_sequence_spells_out_both_codepoints() {
        assert_eq!(
            ann("map 0\u{FE0F} = num-zero-emoji"),
            vec![(6, " U+0030 U+FE0F".to_string())],
        );
    }

    /// Only what a `map` can actually name is annotated: one character, or one
    /// character and a selector. A longer paste is not a mapping sequence at
    /// all — `issues` rejects it by name — and spelling it out would suggest it
    /// is one.
    #[test]
    fn an_unsupported_sequence_is_not_annotated() {
        assert!(ann("map 0\u{FE0F}\u{20E3} = keycap-zero").is_empty());
        // A pattern is a name, not text; running its syntax through the
        // speller is exactly the noise that made a slice line unreadable.
        assert!(ann("map (a|b)c = latin-(a|b)-c").is_empty());
        assert!(ann("map narrow : (a|b)c = latin-(a|b)-c").is_empty());
        // A bare selector cannot be a base, and a base cannot stand where the
        // selector goes.
        assert!(ann("map \u{FE0F} = num-zero-emoji").is_empty());
        assert_eq!(
            ann("map 0 1 = num-zero-emoji"),
            vec![(5, " U+0030".to_string())],
        );
    }

    /// The `U+X U+Y` spelling says it already; nothing to add.
    #[test]
    fn an_explicit_variation_sequence_is_not_annotated() {
        assert!(ann("map U+0030 U+FE0F = num-zero-emoji").is_empty());
        assert!(ann("map generate U+0030 U+FE0F").is_empty());
    }

    /// `map BASE SELECTOR = GLYPH` is the same sequence written as two tokens,
    /// so each half is spelled out where it stands. The selector half is the
    /// one that most needs it — written literally it is invisible.
    #[test]
    fn a_two_token_variation_sequence_annotates_both_halves() {
        assert_eq!(
            ann("map 0 U+FE0F = num-zero-emoji"),
            vec![(5, " U+0030".to_string())],
        );
        assert_eq!(
            ann("map U+0030 \u{FE0F} = num-zero-emoji"),
            vec![(12, " U+FE0F".to_string())],
        );
        assert_eq!(
            ann("map 0 \u{FE0F} = num-zero-emoji"),
            vec![(5, " U+0030".to_string()), (7, " U+FE0F".to_string())],
        );
        // A range on the base half is written `U+…`, so only the selector is.
        assert_eq!(
            ann("map U+0030..0039 \u{FE0F} = num-(zero|one)-emoji"),
            vec![(18, " U+FE0F".to_string())],
        );
    }

    /// `map generate BASE SELECTOR [= GLYPH]` parses as its own arity, and is
    /// annotated like the plain pair form.
    #[test]
    fn a_generated_two_token_sequence_annotates_both_halves() {
        assert_eq!(
            ann("map generate 0 \u{FE0F}"),
            vec![(14, " U+0030".to_string()), (16, " U+FE0F".to_string())],
        );
        assert_eq!(
            ann("map generate 0 \u{FE0F} = num-zero-emoji"),
            vec![(14, " U+0030".to_string()), (16, " U+FE0F".to_string())],
        );
    }

    /// A slice qualifier comes off before the arity is read, pair form included.
    #[test]
    fn a_slice_qualified_pair_annotates_both_halves() {
        assert_eq!(
            ann("map narrow : 0 \u{FE0F} = num-zero-emoji"),
            vec![(14, " U+0030".to_string()), (16, " U+FE0F".to_string())],
        );
    }

    /// A pipe list is annotated part by part, and must not be run together into
    /// one spelled-out sequence — the pipes are syntax, not characters.
    #[test]
    fn a_quoted_pipe_list_is_not_spelled_out_as_one_sequence() {
        assert!(
            ann("map `a|b` = ab")
                .iter()
                .all(|(_, text)| !text.contains("U+007C")),
        );
    }

    /// `assert shape` is the other place a source states text, and the only one
    /// where the text can be long. Nothing else on the line can be read to tell
    /// a text-presentation sequence from an emoji one.
    #[test]
    fn assert_shape_text_is_annotated() {
        assert_eq!(
            ann("assert shape ⚫\u{FE0E} : black-circle-6-inside"),
            vec![(15, " U+26AB U+FE0E".to_string())],
        );
        assert_eq!(
            ann("assert shape `0\u{FE0F}` : keycap-zero"),
            vec![(17, " U+0030 U+FE0F".to_string())],
        );
    }

    #[test]
    fn assert_same_and_distinct_are_not_annotated() {
        assert!(ann("assert same a b").is_empty());
        assert!(ann("assert distinct a b").is_empty());
    }

    #[test]
    fn indented_line_annotation_is_offset() {
        assert_eq!(ann("  map A = latin-a"), vec![(7, " U+0041".to_string())]);
    }

    #[test]
    fn explicit_codepoint_is_not_annotated() {
        assert!(ann("map U+AC00 = hangul-ga").is_empty());
        assert!(ann("map u+ac00 = hangul-ga").is_empty());
        assert!(ann("map U+0041..005A = latin-(a|b)").is_empty());
    }

    #[test]
    fn quoted_char_is_annotated_after_the_quotes() {
        // `` ` `` is written quoted; the annotation follows the closing quote.
        assert_eq!(ann("map ```` = grave"), vec![(8, " U+0060".to_string())]);
    }

    /// A list of one-character alternatives is real, everyday `map` syntax, so
    /// the per-alternative filter has to accept each part on its own — the
    /// pipes are syntax and never make the token unsupported.
    #[test]
    fn a_long_alternative_list_annotates_every_part() {
        assert_eq!(
            ann("map a|b|c|d = latin-(a|b|c|d)"),
            vec![
                (5, " U+0061".to_string()),
                (7, " U+0062".to_string()),
                (9, " U+0063".to_string()),
                (11, " U+0064".to_string()),
            ],
        );
        // Alternatives that are themselves variation sequences count too.
        assert_eq!(
            ann("map 0\u{FE0F}|1\u{FE0F} = num-emoji"),
            vec![
                (6, " U+0030 U+FE0F".to_string()),
                (9, " U+0031 U+FE0F".to_string())
            ],
        );
        // A slice qualifier only shifts the columns.
        assert_eq!(
            ann("map narrow : a|b = latin-(a|b)"),
            vec![(14, " U+0061".to_string()), (16, " U+0062".to_string())],
        );
    }

    #[test]
    fn pipe_list_annotates_each_literal_part() {
        assert_eq!(
            ann("map A|B = latin-(a|b)"),
            vec![(5, " U+0041".to_string()), (7, " U+0042".to_string())]
        );
        // Mixed forms: only the literal parts get one.
        assert_eq!(
            ann("map U+0041|B = latin-(a|b)"),
            vec![(12, " U+0042".to_string())]
        );
    }

    /// A slice-qualified mapping is the same mapping, so it is annotated the
    /// same way — the qualifier only shifts the column.
    #[test]
    fn slice_qualified_map_is_annotated() {
        assert_eq!(
            ann("map narrow : A = latin-a"),
            vec![(14, " U+0041".to_string())]
        );
        assert_eq!(
            ann("map narrow : generate 가"),
            vec![(23, " U+AC00".to_string())]
        );
        // The colon being mapped is not a qualifier.
        assert_eq!(ann("map : = colon"), vec![(5, " U+003A".to_string())]);
    }

    #[test]
    fn non_map_lines_have_no_annotations() {
        assert!(ann("glyph latin-a 8 16").is_empty());
        assert!(ann("").is_empty());
        // `assert shape` used to be listed here. It states text, and text is
        // exactly what an annotation is for; see `assert_shape_text_is_annotated`.
    }

    #[test]
    fn malformed_map_is_left_alone() {
        assert!(ann("map A B C D").is_empty());
        assert!(ann("map").is_empty());
    }

    #[test]
    fn display_string_splices_annotations_in() {
        let a = line_annotations("map 가 = hangul-ga");
        let at = AnnotatedText::new("map 가 = hangul-ga", &a);
        assert_eq!(at.display_string(), "map 가 U+AC00 = hangul-ga");
        let runs = at.runs();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "map 가");
        assert!(runs[1].is_annotation);
        assert_eq!(runs[1].text, " U+AC00");
        assert_eq!(runs[2].text, " = hangul-ga");
        assert_eq!(runs[2].display_start, runs[1].display_start + 7);
    }

    #[test]
    fn display_prefix_treats_char_and_annotation_as_one_unit() {
        let a = line_annotations("map 가 = hangul-ga");
        let at = AnnotatedText::new("map 가 = hangul-ga", &a);
        // Before the character: no annotation yet.
        assert_eq!(at.display_prefix(4), "map ");
        // Past the character: the annotation comes along.
        assert_eq!(at.display_prefix(5), "map 가 U+AC00");
        assert_eq!(at.display_prefix(6), "map 가 U+AC00 ");
    }
}
