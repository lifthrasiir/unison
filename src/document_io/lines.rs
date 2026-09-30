//! The editor's line division of a source file: `SourceLine`, `walk_source_lines` and the
//! lenient `parse_doclines`.

use crate::document::*;
use crate::pixel::chars_to_shape;

use super::header::glyph_header_dims;
use super::tokens::tokenize_tokens;

/// One unit of a source file as the editor divides it: an ordinary line, or the
/// run of pixel rows a sized `glyph` header opens.
///
/// A whole grid is *one* line to the caret, so an editor line and a file line
/// are two different things. Anything that has to address the same place in an
/// open buffer and in the raw text of an unopened file — the search pane, which
/// counts a hit's ordinal in one and re-finds it in the other — has to divide
/// the text exactly as [`parse_doclines`] does, and sharing this walker is what
/// makes that agreement structural instead of two loops kept in step by hand.
#[cfg(any(feature = "editor", test))]
pub enum SourceLine<'a> {
    Text(&'a str),
    /// The rows actually present under a header, which a short grid leaves
    /// fewer of than `height`.
    Grid {
        width: u16,
        height: u16,
        rows: Vec<&'a str>,
    },
}

/// Walks `content` the way [`parse_doclines`] divides it, handing each unit to
/// `f` with the 1-based file line it starts at.
#[cfg(any(feature = "editor", test))]
pub fn walk_source_lines<'a>(content: &'a str, mut f: impl FnMut(usize, SourceLine<'a>)) {
    let mut iter = content.lines().enumerate().peekable();
    while let Some((idx, line)) = iter.next() {
        // Only a line that writes the word can be a header, quoted or not — the
        // tokenizer's one escape is a backtick — and this walk runs over every
        // line of every file a search touches, most of them pixel rows that
        // tokenizing would cost a pass each.
        let dims = line.contains("glyph").then(|| {
            tokenize_tokens(line.trim()).ok().and_then(|tokens| {
                tokens
                    .first()
                    .is_some_and(|t| t == "glyph")
                    .then(|| glyph_header_dims(&tokens[1..]))
                    .flatten()
            })
        });
        let dims = dims.flatten();
        f(idx + 1, SourceLine::Text(line));
        let Some(dims) = dims else { continue };
        let (width, height) = (dims.width, dims.height);
        let mut rows = Vec::new();
        // Zero width means no row to read at all — see [`is_pixel_row_next`];
        // the two parsers have to agree on where the glyph block ends.
        for _ in 0..if width == 0 { 0 } else { height } {
            let is_pixel = iter.peek().is_some_and(|(_, peek_line)| {
                let chars: Vec<char> = peek_line.chars().collect();
                chars.len() == width as usize * 2
                    && (0..width as usize)
                        .all(|col| chars_to_shape(chars[col * 2], chars[col * 2 + 1]).is_some())
            });
            if !is_pixel {
                break;
            }
            let Some((_, pixel_line)) = iter.next() else {
                break;
            };
            rows.push(pixel_line);
        }
        f(
            idx + 2,
            SourceLine::Grid {
                width,
                height,
                rows,
            },
        );
    }
}

/// Lenient counterpart of [`tokenize_strict`](super::strict::tokenize_strict), for text the editor is in the
/// middle of typing: a malformed header or pixel row becomes an ordinary
/// `Text` line instead of an error, and a short grid is padded rather than
/// rejected. The strict path stays the one behind [`parse_document_from_str`](super::strict::parse_document_from_str).
#[cfg(any(feature = "editor", test))]
pub fn parse_doclines(content: &str) -> Vec<DocLine> {
    let mut lines = Vec::new();
    walk_source_lines(content, |_, unit| match unit {
        SourceLine::Text(text) => lines.push(DocLine::text(text.to_string())),
        SourceLine::Grid {
            width,
            height,
            rows,
        } => {
            let mut grid = PixelGrid::new(width, height);
            for (row, pixel_line) in rows.iter().enumerate() {
                let chars: Vec<char> = pixel_line.chars().collect();
                for col in 0..width as usize {
                    let idx = col * 2;
                    if idx + 1 < chars.len()
                        && let Some(shape) = chars_to_shape(chars[idx], chars[idx + 1])
                    {
                        grid.set(row as u16, col as u16, shape);
                    }
                }
            }
            lines.push(DocLine::grid(grid));
        }
    });
    lines
}
