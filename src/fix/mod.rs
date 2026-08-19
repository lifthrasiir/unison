//! `uniform fix`: the commands that rewrite the font *source*.
//!
//! Everything else in this program reads the source and produces something
//! else — a font file, a report, a rendering. A `fix` command reads it and
//! writes it back. That is a different kind of act and it gets its own home, so
//! that the rules every one of them shares are stated once:
//!
//! - **A fix is a plan first.** A command computes what it would rewrite
//!   ([`clearance::ClearanceFix`] and its kin) without touching anything, and a
//!   *frontend* applies the plan. There are two of them — the `fix` subcommand,
//!   which rewrites the files, and the editor's Font menu, which rewrites the
//!   open documents so the change is undoable and saved deliberately — and
//!   neither may know anything the other does not.
//! - **A fix rewrites whole lines, in place.** Never a re-serialization of the
//!   document: that would reformat every line a human wrote for reasons of
//!   their own, and bury the actual change in the diff. So a plan carries the
//!   line's new text and the frontends carry it to a line number, which is what
//!   [`nth_compose_line`] and [`find_glyph_item`] are for.
//! - **A fix only touches what is already reported.** A command that "improves"
//!   a line nothing complained about is a command nobody can review, at 20k
//!   glyphs least of all. Each command states its own version of this rule; for
//!   clearance it is that the line must warn *and* the rewrite must improve on
//!   it — lower the score, or, for a line that stands for a whole family of
//!   glyphs, leave fewer of them warning.
//!
//! The commands so far:
//!
//! | Flag | What it rewrites |
//! | --- | --- |
//! | `--optimize-clearance` | IDC lines whose clearances fall outside `audit ideal-clearance` ([`clearance`]) |

pub mod clearance;

use crate::document::{Document, DocumentItem};

/// The item index of the glyph named `name`, preferring `hint`.
///
/// A plan is computed against a snapshot of the documents and applied to what
/// they are now, which may have been rederived in between — and that renumbers
/// items. The name is what identifies the glyph; the index is only a hint. Same
/// rule, and same reason, as `app::resize`'s `defining_item`.
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn find_glyph_item(doc: &Document, name: &str, hint: usize) -> Option<usize> {
    let is_target = |idx: usize| {
        matches!(
            doc.items.get(idx),
            Some(DocumentItem::Glyph { name: n, .. }) if n.0 == name
        )
    };
    if is_target(hint) {
        return Some(hint);
    }
    (0..doc.items.len()).find(|&idx| is_target(idx))
}

/// The line of the `n`-th IDC line inside a glyph block, searching `range` and
/// asking `text` for each line's content.
///
/// The caller decides which line space that is: the editor indexes its
/// [`DocLine`](crate::document::DocLine)s, the `fix` subcommand indexes the
/// file's own lines, and the two do not agree because a whole pixel grid is one
/// `DocLine`. What they do agree on is the search: a glyph block's lines are
/// contiguous and an IDC character can start no other kind of line, so the
/// n-th line in the block that starts with one is the n-th IDC line — whatever
/// order the block happens to write its `ref`s, `anchor`s and IDC lines in,
/// which the parser accepts in any.
pub fn nth_compose_line<'t>(
    text: &dyn Fn(usize) -> Option<&'t str>,
    range: std::ops::Range<usize>,
    n: usize,
) -> Option<usize> {
    let mut seen = 0usize;
    for line in range {
        let Some(content) = text(line) else { continue };
        let Some(first) = content.split_whitespace().next() else {
            continue;
        };
        if crate::compose::IdcOp::from_token(first).is_none() {
            continue;
        }
        if seen == n {
            return Some(line);
        }
        seen += 1;
    }
    None
}

/// The 0-based line of a glyph block's `n`-th IDC line in the file's own text,
/// `lines` being that text split into lines.
///
/// The block is bounded by the next item's line rather than by guessing where
/// a block ends: the parser numbers every item, so the answer is exact and it
/// costs nothing.
pub fn compose_file_line(
    doc: &Document,
    item_idx: usize,
    compose_idx: usize,
    lines: &[&str],
) -> Option<usize> {
    // 1-based header line, which is the 0-based line *after* the header.
    let start = doc.item_lines(item_idx).1;
    let end = match item_idx + 1 < doc.items.len() {
        true => doc.item_lines(item_idx + 1).1.saturating_sub(1),
        false => lines.len(),
    };
    nth_compose_line(
        &|i| lines.get(i).copied(),
        start..end.min(lines.len()),
        compose_idx,
    )
}

/// `text` with each `(line, replacement)` put in place of that 0-based line.
///
/// Only the named lines change: everything else — spacing, comments, the file's
/// final newline or the lack of one — comes back byte for byte, since a fix is
/// a line edit and not a re-serialization. A `\r\n` line keeps its `\r`.
pub fn rewrite_lines(text: &str, edits: &[(usize, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match edits.iter().find(|(l, _)| *l == i) {
            Some((_, replacement)) => {
                out.push_str(replacement);
                if line.ends_with('\r') {
                    out.push('\r');
                }
            }
            None => out.push_str(line),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewrite_touches_only_the_named_lines() {
        let text = "a\nb\nc\n";
        assert_eq!(
            rewrite_lines(text, &[(1, "B".to_string())]),
            "a\nB\nc\n",
            "the trailing newline is the file's, not a line's",
        );
        // No final newline, and a CRLF line keeps its carriage return.
        assert_eq!(rewrite_lines("a\nb", &[(1, "B".into())]), "a\nB");
        assert_eq!(
            rewrite_lines("a\r\nb\r\n", &[(0, "A".into())]),
            "A\r\nb\r\n"
        );
        assert_eq!(rewrite_lines(text, &[(9, "x".into())]), text);
    }

    #[test]
    fn the_nth_compose_line_skips_everything_else() {
        let lines = [
            "glyph x 8 4",
            "ref a",
            "\u{2FF0} a:4x4 b:4x4",
            "anchor top 0 0",
            "\u{2FF1} c:8x2 d:8x2",
        ];
        let text = |i: usize| lines.get(i).copied();
        assert_eq!(nth_compose_line(&text, 1..5, 0), Some(2));
        assert_eq!(nth_compose_line(&text, 1..5, 1), Some(4));
        assert_eq!(nth_compose_line(&text, 1..5, 2), None);
        assert_eq!(nth_compose_line(&text, 3..5, 0), Some(4));
    }
}
