use crate::document::DocLine;

/// Byte offset in `s` corresponding to the given char index, clamped to
/// `s.len()` if `char_idx` is past the end.
pub(crate) fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Caret {
    pub line: usize,
    pub col: usize,
}

impl Caret {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }

    pub fn zero() -> Self {
        Self { line: 0, col: 0 }
    }
}

pub fn line_char_len(lines: &[DocLine], line: usize) -> usize {
    lines.get(line).map_or(0, |l| l.char_len())
}

pub fn move_left(lines: &[DocLine], c: Caret) -> Caret {
    if c.col > 0 {
        return Caret {
            line: c.line,
            col: c.col - 1,
        };
    }
    if c.line == 0 {
        return c;
    }
    let prev = c.line - 1;
    Caret {
        line: prev,
        col: line_char_len(lines, prev),
    }
}

pub fn move_right(lines: &[DocLine], c: Caret) -> Caret {
    let len = line_char_len(lines, c.line);
    if c.col < len {
        return Caret {
            line: c.line,
            col: c.col + 1,
        };
    }
    if c.line + 1 >= lines.len() {
        return c;
    }
    Caret {
        line: c.line + 1,
        col: 0,
    }
}

pub fn move_up(lines: &[DocLine], c: Caret) -> Caret {
    if c.line == 0 {
        return c;
    }
    let new_line = c.line - 1;
    let new_len = line_char_len(lines, new_line);
    Caret {
        line: new_line,
        col: c.col.min(new_len),
    }
}

pub fn move_down(lines: &[DocLine], c: Caret) -> Caret {
    if c.line + 1 >= lines.len() {
        return c;
    }
    let new_line = c.line + 1;
    let new_len = line_char_len(lines, new_line);
    Caret {
        line: new_line,
        col: c.col.min(new_len),
    }
}

pub fn home(lines: &[DocLine], c: Caret) -> Caret {
    let _ = lines;
    Caret {
        line: c.line,
        col: 0,
    }
}

pub fn end(lines: &[DocLine], c: Caret) -> Caret {
    Caret {
        line: c.line,
        col: line_char_len(lines, c.line),
    }
}

pub fn doc_home(_lines: &[DocLine]) -> Caret {
    Caret { line: 0, col: 0 }
}

pub fn doc_end(lines: &[DocLine]) -> Caret {
    let last = lines.len().saturating_sub(1);
    Caret {
        line: last,
        col: line_char_len(lines, last),
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Forward scan of `egui`'s word-boundary rule (`text_cursor_state::
/// next_word_boundary_char_index`), so Ctrl+Left/Right here move exactly like
/// they do in a plain `TextEdit` — notably `-` separates words and `_` does not.
/// One character is always consumed; after that the scan runs while the
/// word/non-word class keeps matching the *second* character's. The one
/// deliberate difference is `is_word_char` above: `egui`'s is ASCII-only, ours
/// is not, so a Hangul or Cyrillic run counts as a word.
///
/// Both scans stop at the end of the line; the callers handle line crossings.
fn next_word_boundary(chars: &[char], mut index: usize) -> usize {
    if index >= chars.len() {
        return index;
    }
    index += 1;
    if index < chars.len() {
        let class = is_word_char(chars[index]);
        index += 1;
        while index < chars.len() && is_word_char(chars[index]) == class {
            index += 1;
        }
    }
    index
}

/// Mirror image of [`next_word_boundary`], equivalent to running it over the
/// reversed line as `egui` does.
fn prev_word_boundary(chars: &[char], index: usize) -> usize {
    let mut index = index.min(chars.len());
    if index == 0 {
        return 0;
    }
    index -= 1;
    if index > 0 {
        let class = is_word_char(chars[index - 1]);
        index -= 1;
        while index > 0 && is_word_char(chars[index - 1]) == class {
            index -= 1;
        }
    }
    index
}

pub fn move_word_left(lines: &[DocLine], c: Caret) -> Caret {
    if c.col == 0 {
        if c.line == 0 {
            return c;
        }
        let prev = c.line - 1;
        return Caret {
            line: prev,
            col: line_char_len(lines, prev),
        };
    }
    let Some(DocLine::Text(s)) = lines.get(c.line) else {
        return Caret {
            line: c.line,
            col: 0,
        };
    };
    let chars: Vec<char> = s.chars().collect();
    Caret {
        line: c.line,
        col: prev_word_boundary(&chars, c.col),
    }
}

pub fn move_word_right(lines: &[DocLine], c: Caret) -> Caret {
    let len = line_char_len(lines, c.line);
    if c.col >= len {
        if c.line + 1 >= lines.len() {
            return c;
        }
        return Caret {
            line: c.line + 1,
            col: 0,
        };
    }
    let Some(DocLine::Text(s)) = lines.get(c.line) else {
        return Caret {
            line: c.line + 1,
            col: 0,
        };
    };
    let chars: Vec<char> = s.chars().collect();
    Caret {
        line: c.line,
        col: next_word_boundary(&chars, c.col),
    }
}

pub fn word_bounds_at(lines: &[DocLine], c: Caret) -> (Caret, Caret) {
    let Some(DocLine::Text(s)) = lines.get(c.line) else {
        return (
            Caret {
                line: c.line,
                col: 0,
            },
            Caret {
                line: c.line,
                col: 0,
            },
        );
    };
    let chars: Vec<char> = s.chars().collect();
    if c.col >= chars.len() {
        let len = chars.len();
        return (
            Caret {
                line: c.line,
                col: len,
            },
            Caret {
                line: c.line,
                col: len,
            },
        );
    }
    let at = chars[c.col];
    let word = is_word_char(at);
    let mut lo = c.col;
    let mut hi = c.col;
    if word {
        while lo > 0 && is_word_char(chars[lo - 1]) {
            lo -= 1;
        }
        while hi < chars.len() && is_word_char(chars[hi]) {
            hi += 1;
        }
    } else {
        while lo > 0 && !is_word_char(chars[lo - 1]) && !chars[lo - 1].is_whitespace() {
            lo -= 1;
        }
        while hi < chars.len() && !is_word_char(chars[hi]) && !chars[hi].is_whitespace() {
            hi += 1;
        }
        if lo == hi {
            while lo > 0 && chars[lo - 1].is_whitespace() {
                lo -= 1;
            }
            while hi < chars.len() && chars[hi].is_whitespace() {
                hi += 1;
            }
        }
    }
    (
        Caret {
            line: c.line,
            col: lo,
        },
        Caret {
            line: c.line,
            col: hi,
        },
    )
}

pub fn extract_text(lines: &[DocLine], lo: Caret, hi: Caret) -> String {
    if lo == hi {
        return String::new();
    }
    if lo.line == hi.line {
        if let Some(DocLine::Text(s)) = lines.get(lo.line) {
            let b0 = char_to_byte(s, lo.col);
            let b1 = char_to_byte(s, hi.col);
            return s[b0..b1].to_string();
        }
        return String::new();
    }
    let mut result = String::new();
    for line_idx in lo.line..=hi.line {
        match lines.get(line_idx) {
            Some(DocLine::Text(s)) => {
                let start = if line_idx == lo.line {
                    char_to_byte(s, lo.col)
                } else {
                    0
                };
                let end = if line_idx == hi.line {
                    char_to_byte(s, hi.col)
                } else {
                    s.len()
                };
                result.push_str(&s[start..end]);
            }
            Some(DocLine::Grid(g)) => {
                for row in 0..g.height {
                    if row > 0 {
                        result.push('\n');
                    }
                    result.push_str(&crate::document_io::encode_grid_row(g, row));
                }
            }
            None => {}
        }
        if line_idx < hi.line {
            result.push('\n');
        }
    }
    result
}

pub fn clamp(lines: &[DocLine], c: Caret) -> Caret {
    if lines.is_empty() {
        return Caret::zero();
    }
    let line = c.line.min(lines.len() - 1);
    let col = c.col.min(line_char_len(lines, line));
    Caret { line, col }
}

pub fn selection_range(cursor: Caret, anchor: Option<Caret>) -> Option<(Caret, Caret)> {
    anchor.map(|a| {
        let lo = a.min(cursor);
        let hi = a.max(cursor);
        (lo, hi)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PixelGrid;

    fn text(s: &str) -> DocLine {
        DocLine::Text(s.to_string())
    }
    fn grid(w: u16, h: u16) -> DocLine {
        DocLine::Grid(PixelGrid::new(w, h))
    }

    #[test]
    fn move_right_within_text() {
        let lines = vec![text("abc"), text("def")];
        assert_eq!(move_right(&lines, Caret::new(0, 0)), Caret::new(0, 1));
        assert_eq!(move_right(&lines, Caret::new(0, 2)), Caret::new(0, 3));
    }

    #[test]
    fn move_right_across_text_lines() {
        let lines = vec![text("ab"), text("cd")];
        assert_eq!(move_right(&lines, Caret::new(0, 2)), Caret::new(1, 0));
    }

    #[test]
    fn move_right_onto_grid() {
        let lines = vec![text("ab"), grid(4, 4), text("cd")];
        assert_eq!(move_right(&lines, Caret::new(0, 2)), Caret::new(1, 0));
    }

    #[test]
    fn move_right_off_grid() {
        let lines = vec![text("ab"), grid(4, 4), text("cd")];
        // Grid has char_len 0, so col 0 == end. Right moves to next line.
        assert_eq!(move_right(&lines, Caret::new(1, 0)), Caret::new(2, 0));
    }

    #[test]
    fn move_right_at_end_of_doc() {
        let lines = vec![text("ab")];
        assert_eq!(move_right(&lines, Caret::new(0, 2)), Caret::new(0, 2));
    }

    #[test]
    fn move_left_within_text() {
        let lines = vec![text("abc")];
        assert_eq!(move_left(&lines, Caret::new(0, 2)), Caret::new(0, 1));
    }

    #[test]
    fn move_left_across_text_lines() {
        let lines = vec![text("ab"), text("cd")];
        assert_eq!(move_left(&lines, Caret::new(1, 0)), Caret::new(0, 2));
    }

    #[test]
    fn move_left_onto_grid() {
        let lines = vec![text("ab"), grid(4, 4), text("cd")];
        assert_eq!(move_left(&lines, Caret::new(2, 0)), Caret::new(1, 0));
    }

    #[test]
    fn move_left_off_grid() {
        let lines = vec![text("ab"), grid(4, 4), text("cd")];
        assert_eq!(move_left(&lines, Caret::new(1, 0)), Caret::new(0, 2));
    }

    #[test]
    fn move_left_at_start() {
        let lines = vec![text("ab")];
        assert_eq!(move_left(&lines, Caret::new(0, 0)), Caret::new(0, 0));
    }

    #[test]
    fn move_up_clamps_col() {
        let lines = vec![text("ab"), text("cdef")];
        assert_eq!(move_up(&lines, Caret::new(1, 3)), Caret::new(0, 2));
    }

    #[test]
    fn move_up_to_grid_clamps_col_to_zero() {
        let lines = vec![grid(4, 4), text("abcde")];
        assert_eq!(move_up(&lines, Caret::new(1, 3)), Caret::new(0, 0));
    }

    #[test]
    fn move_down_clamps_col() {
        let lines = vec![text("abcde"), text("fg")];
        assert_eq!(move_down(&lines, Caret::new(0, 4)), Caret::new(1, 2));
    }

    #[test]
    fn home_and_end() {
        let lines = vec![text("abc"), grid(4, 4)];
        assert_eq!(home(&lines, Caret::new(0, 2)), Caret::new(0, 0));
        assert_eq!(end(&lines, Caret::new(0, 1)), Caret::new(0, 3));
        // Grid: home and end both go to col 0
        assert_eq!(home(&lines, Caret::new(1, 0)), Caret::new(1, 0));
        assert_eq!(end(&lines, Caret::new(1, 0)), Caret::new(1, 0));
    }

    #[test]
    fn doc_home_and_doc_end() {
        let lines = vec![text("abc"), text("def"), text("ghi")];
        assert_eq!(doc_home(&lines), Caret::new(0, 0));
        assert_eq!(doc_end(&lines), Caret::new(2, 3));
    }

    #[test]
    fn doc_end_with_grid() {
        let lines = vec![text("abc"), grid(4, 4)];
        assert_eq!(doc_end(&lines), Caret::new(1, 0));
    }

    #[test]
    fn doc_home_end_single_line() {
        let lines = vec![text("hello")];
        assert_eq!(doc_home(&lines), Caret::new(0, 0));
        assert_eq!(doc_end(&lines), Caret::new(0, 5));
    }

    #[test]
    fn selection_range_ordering() {
        let a = Caret::new(2, 5);
        let b = Caret::new(0, 3);
        assert_eq!(selection_range(a, Some(b)), Some((b, a)));
        assert_eq!(selection_range(b, Some(a)), Some((b, a)));
        assert_eq!(selection_range(a, None), None);
    }

    #[test]
    fn clamp_within_bounds() {
        let lines = vec![text("ab"), grid(4, 4), text("c")];
        assert_eq!(clamp(&lines, Caret::new(0, 5)), Caret::new(0, 2));
        assert_eq!(clamp(&lines, Caret::new(1, 3)), Caret::new(1, 0));
        assert_eq!(clamp(&lines, Caret::new(99, 0)), Caret::new(2, 0));
    }

    #[test]
    fn clamp_empty_doc() {
        let lines: Vec<DocLine> = vec![];
        assert_eq!(clamp(&lines, Caret::new(5, 3)), Caret::zero());
    }

    #[test]
    fn move_word_left_basic() {
        let lines = vec![text("hello world")];
        assert_eq!(move_word_left(&lines, Caret::new(0, 11)), Caret::new(0, 6));
        assert_eq!(move_word_left(&lines, Caret::new(0, 6)), Caret::new(0, 0));
        assert_eq!(move_word_left(&lines, Caret::new(0, 0)), Caret::new(0, 0));
    }

    #[test]
    fn move_word_left_across_lines() {
        let lines = vec![text("ab"), text("cd")];
        assert_eq!(move_word_left(&lines, Caret::new(1, 0)), Caret::new(0, 2));
    }

    #[test]
    fn move_word_right_basic() {
        let lines = vec![text("hello world")];
        // Like a `TextEdit`, the caret lands at the *end* of the word it crossed.
        assert_eq!(move_word_right(&lines, Caret::new(0, 0)), Caret::new(0, 5));
        assert_eq!(move_word_right(&lines, Caret::new(0, 5)), Caret::new(0, 11));
    }

    #[test]
    fn move_word_right_across_lines() {
        let lines = vec![text("ab"), text("cd")];
        assert_eq!(move_word_right(&lines, Caret::new(0, 2)), Caret::new(1, 0));
    }

    #[test]
    fn move_word_right_on_grid() {
        let lines = vec![text("ab"), grid(2, 2), text("cd")];
        assert_eq!(move_word_right(&lines, Caret::new(1, 0)), Caret::new(2, 0));
    }

    #[test]
    fn move_word_crosses_a_punctuation_run_in_one_step() {
        // "remap foo : # = xxx": a `TextEdit` treats the whole " : # = " run as
        // one non-word stretch rather than stopping at each token.
        let lines = vec![text("remap foo : # = xxx")];
        assert_eq!(move_word_right(&lines, Caret::new(0, 6)), Caret::new(0, 9));
        assert_eq!(move_word_right(&lines, Caret::new(0, 9)), Caret::new(0, 16));
        assert_eq!(
            move_word_right(&lines, Caret::new(0, 16)),
            Caret::new(0, 19)
        );
        assert_eq!(move_word_left(&lines, Caret::new(0, 19)), Caret::new(0, 16));
        assert_eq!(move_word_left(&lines, Caret::new(0, 16)), Caret::new(0, 9));
        assert_eq!(move_word_left(&lines, Caret::new(0, 9)), Caret::new(0, 6));
    }

    #[test]
    fn move_word_treats_hyphen_as_separator() {
        // Same rule as an egui `TextEdit`: only alphanumerics and `_` are word
        // characters, so `-` splits a name but `_` does not.
        let lines = vec![text("uni-form_x ab")];
        assert_eq!(move_word_right(&lines, Caret::new(0, 0)), Caret::new(0, 3));
        assert_eq!(move_word_right(&lines, Caret::new(0, 3)), Caret::new(0, 10));
        assert_eq!(
            move_word_right(&lines, Caret::new(0, 10)),
            Caret::new(0, 13)
        );
        assert_eq!(move_word_left(&lines, Caret::new(0, 13)), Caret::new(0, 11));
        assert_eq!(move_word_left(&lines, Caret::new(0, 11)), Caret::new(0, 4));
        assert_eq!(move_word_left(&lines, Caret::new(0, 4)), Caret::new(0, 0));
    }

    #[test]
    fn move_word_keeps_non_ascii_letters_as_word_chars() {
        // egui's own predicate is ASCII-only; ours is not, so a Hangul run is a
        // word and does not merge with the punctuation around it.
        let lines = vec![text("map 가나 : x")];
        assert_eq!(move_word_right(&lines, Caret::new(0, 0)), Caret::new(0, 3));
        assert_eq!(move_word_right(&lines, Caret::new(0, 3)), Caret::new(0, 6));
        assert_eq!(move_word_left(&lines, Caret::new(0, 6)), Caret::new(0, 4));
    }

    #[test]
    fn move_word_right_inside_whitespace_stops_at_token() {
        let lines = vec![text("ab   cd")];
        assert_eq!(move_word_right(&lines, Caret::new(0, 3)), Caret::new(0, 5));
    }

    #[test]
    fn word_bounds_basic() {
        let lines = vec![text("hello world")];
        assert_eq!(
            word_bounds_at(&lines, Caret::new(0, 2)),
            (Caret::new(0, 0), Caret::new(0, 5))
        );
        assert_eq!(
            word_bounds_at(&lines, Caret::new(0, 7)),
            (Caret::new(0, 6), Caret::new(0, 11))
        );
    }

    #[test]
    fn word_bounds_on_space() {
        let lines = vec![text("hello world")];
        let (lo, hi) = word_bounds_at(&lines, Caret::new(0, 5));
        assert_eq!(lo, Caret::new(0, 5));
        assert_eq!(hi, Caret::new(0, 6));
    }

    #[test]
    fn word_bounds_on_grid() {
        let lines = vec![grid(2, 2)];
        let (lo, hi) = word_bounds_at(&lines, Caret::new(0, 0));
        assert_eq!(lo, Caret::new(0, 0));
        assert_eq!(hi, Caret::new(0, 0));
    }

    #[test]
    fn extract_text_single_line() {
        let lines = vec![text("hello world")];
        assert_eq!(
            extract_text(&lines, Caret::new(0, 0), Caret::new(0, 5)),
            "hello",
        );
    }

    #[test]
    fn extract_text_multi_line() {
        let lines = vec![text("abc"), text("def"), text("ghi")];
        assert_eq!(
            extract_text(&lines, Caret::new(0, 1), Caret::new(2, 2)),
            "bc\ndef\ngh",
        );
    }

    #[test]
    fn extract_text_across_grid() {
        let lines = vec![text("abc"), grid(2, 2), text("def")];
        assert_eq!(
            extract_text(&lines, Caret::new(0, 1), Caret::new(2, 2)),
            "bc\n....\n....\nde",
        );
    }

    #[test]
    fn extract_text_empty_range() {
        let lines = vec![text("abc")];
        assert_eq!(extract_text(&lines, Caret::new(0, 1), Caret::new(0, 1)), "");
    }
}
