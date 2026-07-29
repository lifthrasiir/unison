//! Inline annotations: display-only text spliced into a document line.
//!
//! An annotation is *not* part of the document buffer. It is drawn between two
//! document characters and is dimmed relative to the surrounding text, and the
//! caret steps over it in one go — the annotated character plus its annotation
//! behave as a single unit for hit-testing, caret placement and selection.
//!
//! Today the only producer is `map`, which spells out the codepoint of a
//! literally written character (`map 가 = ...` renders as `map 가 U+AC00 = ...`).
//! New kinds plug into [`line_annotations`]; everything downstream — width
//! measurement, painting, hit-testing — is kind-agnostic and lives in
//! [`AnnotatedText`].

use crate::document_io::{TokenSpan, tokenize_with_spans};
use crate::render::ttf_builder::parse_map_char;

/// Display-only text inserted *after* document column `col` of a text line.
///
/// `col` is a character column and is always >= 1: an annotation trails the
/// character it describes, so the caret at `col` sits past the annotation
/// while the caret at `col - 1` sits before the annotated character.
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
        _ => Vec::new(),
    };
    out.retain(|a| a.col >= 1);
    out.sort_by_key(|a| a.col);
    out
}

/// `map CHAR = GLYPH` / `map generate CHAR [= GLYPH]`: spell out the codepoint
/// of every literally written character. Tokens already in `U+XXXX` form
/// annotate nothing.
fn map_annotations(rest: &[TokenSpan], leading: usize) -> Vec<InlineAnnotation> {
    let generate = rest.first().is_some_and(|s| s.value == "generate");
    let char_span = match (generate, rest.len()) {
        (false, 3) if rest[1].value == "=" => &rest[0],
        (true, 2) => &rest[1],
        (true, 4) if rest[2].value == "=" => &rest[1],
        _ => return Vec::new(),
    };

    let value = char_span.value.as_str();
    let quoted = char_span.raw_end - char_span.raw_start != value.chars().count();

    // A pipe list annotates each literal part in place; that is only possible
    // when the token is unquoted, since quoting shifts the inner offsets.
    if !quoted && value.contains('|') {
        let mut out = Vec::new();
        for (part, end_off) in split_top_level_pipes_with_ends(value) {
            if let Some(text) = codepoint_annotation(part) {
                out.push(InlineAnnotation {
                    col: leading + char_span.raw_start + end_off,
                    text,
                });
            }
        }
        return out;
    }

    match codepoint_annotation(value) {
        Some(text) => vec![InlineAnnotation {
            col: leading + char_span.raw_end,
            text,
        }],
        None => Vec::new(),
    }
}

/// ` U+XXXX` for a single literal character, `None` for anything already
/// written as `U+XXXX` (or not a single character at all).
fn codepoint_annotation(s: &str) -> Option<String> {
    if s.starts_with("U+") || s.starts_with("u+") {
        return None;
    }
    let cp = parse_map_char(s)?;
    Some(format!(" U+{cp:04X}"))
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
        if col == 0 || self.text.is_empty() {
            return 0.0;
        }
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
                if run.is_annotation { c.gamma_multiply(ANNOTATION_OPACITY) } else { c }
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
        assert_eq!(ann("map generate 가 = hangul-ga"), vec![(14, " U+AC00".to_string())]);
        // The bare `map CHAR` form is gone; it is no longer a map at all.
        assert!(ann("map 가").is_empty());
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

    #[test]
    fn non_map_lines_have_no_annotations() {
        assert!(ann("glyph latin-a 8 16").is_empty());
        assert!(ann("assert shape 가 : hangul-ga").is_empty());
        assert!(ann("").is_empty());
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
