//! The strict parse: `parse_document_from_str`, `tokenize_strict`, pixel-row validation
//! and `ParseError`.

use std::fmt;

use anyhow::Result;

use crate::document::*;
use crate::pixel::chars_to_shape;

use super::derive::derive_document;
use super::header::{glyph_header_dims, parse_glyph_flag_parts_impl};
use super::tokens::{continuation_text, split_heading, tokenize_tokens};

/// Parse `.unf` source text into a `Document`.
///
/// This tokenizes the text into `DocLine`s (validating pixel rows strictly
/// along the way, via [`parse_pixel_rows`]) and then feeds them through
/// [`derive_document`], which is the single implementation of the
/// item-level `.unf` grammar (comments, meta, directives, glyphs, refs)
/// shared with the `DocLine`-based editor path.
pub fn parse_document_from_str(content: &str, path: std::path::PathBuf) -> Result<Document> {
    let lines = tokenize_strict(content)?;
    let (doc, _) = derive_document(&lines, path).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(doc)
}

/// Tokenize `.unf` source text into `DocLine`s, strictly validating any
/// pixel rows that follow a `glyph NAME W H [OFF_ROW OFF_COL]` header (see
/// [`parse_pixel_rows`]). All other lines (comments, meta, directives,
/// ref lines, alias/ref-only glyph headers) are passed through as-is; their
/// grammar is interpreted later by [`derive_document`].
pub(super) fn tokenize_strict(content: &str) -> std::result::Result<Vec<DocLine>, ParseError> {
    let mut lines = Vec::new();
    let mut iter = content.lines().enumerate().peekable();

    while let Some((line_no, line)) = iter.next() {
        // The docline this line becomes is the one pushed next.
        let at = At {
            line: lines.len(),
            file_line: line_no + 1,
        };
        let trimmed = line.trim();

        // Comments and headings are free text — `derive_document` passes them
        // through verbatim, and tokenizing them would let a backtick in prose
        // abort the whole file.
        // A continuation carries prose and is passed through with the rest of
        // the free text: tokenizing one would let a backtick in a sample abort
        // the whole file.
        if trimmed.starts_with("//")
            || split_heading(trimmed).is_some()
            || continuation_text(trimmed).is_some()
        {
            lines.push(DocLine::text(line.to_string()));
            continue;
        }

        let tokens = tokenize_tokens(trimmed).map_err(|e| at.error(e))?;

        if tokens.first().is_some_and(|t| t == "glyph") {
            let parts = &tokens[1..];
            validate_glyph_header(parts, at)?;
            lines.push(DocLine::text(line.to_string()));

            if let Some(dims) = glyph_header_dims(parts) {
                if is_pixel_row_next(&mut iter, dims.width) {
                    let grid = parse_pixel_rows(&mut iter, dims.width, dims.height, at)?;
                    lines.push(DocLine::grid(grid));
                } else {
                    lines.push(DocLine::grid(PixelGrid::new(dims.width, dims.height)));
                }
            }
        } else {
            lines.push(DocLine::text(line.to_string()));
        }
    }

    Ok(lines)
}

/// A line of [`tokenize_strict`], as a [`ParseError`] on it is located.
#[derive(Clone, Copy)]
struct At {
    line: usize,
    file_line: usize,
}

impl At {
    fn error(self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            file_line: self.file_line,
            message: message.into(),
        }
    }
}

fn validate_glyph_header<S: AsRef<str>>(
    parts: &[S],
    at: At,
) -> std::result::Result<(), ParseError> {
    if parts.is_empty() {
        return Err(at.error("empty glyph name"));
    }
    let rest = &parts[1..];

    // glyph NAME = TARGET — an alias, which is a name and nothing else. The
    // flags used to be accepted here because the form built a real glyph; it
    // no longer does, so a flag on one is a mistake worth naming.
    if let Some(eq_pos) = rest.iter().position(|p| p.as_ref() == "=") {
        if eq_pos != 0 {
            let flags: Vec<&str> = rest[..eq_pos].iter().map(|s| s.as_ref()).collect();
            return Err(at.error(format!(
                "`glyph NAME = TARGET` is an alias for one glyph and takes no flags \
                 (found `{}`); write `glyph NAME {}` with a `ref TARGET` line instead",
                flags.join(" "),
                flags.join(" "),
            )));
        }
        if eq_pos + 1 != rest.len() - 1 {
            if eq_pos + 1 >= rest.len() {
                return Err(at.error("missing alias target after '='"));
            }
            // Extra tokens after alias target
            let extra: Vec<&str> = rest[eq_pos + 2..].iter().map(|s| s.as_ref()).collect();
            return Err(at.error(format!(
                "unexpected tokens after alias target: {}",
                extra.join(" "),
            )));
        }
        crate::alias::multi_alias_prefixes(parts[0].as_ref(), rest[eq_pos + 1].as_ref())
            .map_err(|message| at.error(message))?;
        return Ok(());
    }

    validate_glyph_flags(rest, at)
}

/// Strict form of [`parse_glyph_flag_parts`](super::header::parse_glyph_flag_parts): same grammar, same walker,
/// but the first malformed token becomes an error.
fn validate_glyph_flags<S: AsRef<str>>(
    tokens: &[S],
    at: At,
) -> std::result::Result<(), ParseError> {
    let mut first_err: Option<String> = None;
    parse_glyph_flag_parts_impl(tokens, &mut |msg| {
        if first_err.is_none() {
            first_err = Some(msg);
        }
    });
    match first_err {
        Some(msg) => Err(at.error(msg)),
        None => Ok(()),
    }
}

pub(super) fn is_pixel_row_next(
    lines: &mut std::iter::Peekable<std::iter::Enumerate<std::str::Lines<'_>>>,
    width: u16,
) -> bool {
    // A zero-width glyph has no row to read: every row would encode to the
    // empty string, so accepting one here would swallow the blank lines that
    // follow the header (and then fail on the first non-blank one).
    if width == 0 {
        return false;
    }
    let Some(&(_, line)) = lines.peek() else {
        return false;
    };
    let chars: Vec<char> = line.chars().collect();
    let expected_len = width as usize * 2;
    if chars.len() != expected_len {
        return false;
    }
    for col in 0..width as usize {
        if chars_to_shape(chars[col * 2], chars[col * 2 + 1]).is_none() {
            return false;
        }
    }
    true
}

fn parse_pixel_rows(
    lines: &mut std::iter::Peekable<std::iter::Enumerate<std::str::Lines<'_>>>,
    width: u16,
    height: u16,
    header: At,
) -> std::result::Result<PixelGrid, ParseError> {
    let mut grid = PixelGrid::new(width, height);

    for row in 0..height {
        // Rows that run out are the header's promise broken.
        let (line_no, line) = lines
            .next()
            .ok_or_else(|| header.error(format!("expected {height} pixel rows, got {row}")))?;
        // Every row is part of the one grid docline after the header.
        let at = At {
            line: header.line + 1,
            file_line: line_no + 1,
        };

        let chars: Vec<char> = line.chars().collect();
        let expected_len = width as usize * 2;
        if chars.len() != expected_len {
            return Err(at.error(format!(
                "expected {} chars ({} pixel columns × 2), got {}",
                expected_len,
                width,
                chars.len(),
            )));
        }

        for col in 0..width as usize {
            let c1 = chars[col * 2];
            let c2 = chars[col * 2 + 1];
            let shape = chars_to_shape(c1, c2).ok_or_else(|| {
                at.error(format!("unknown pixel pair '{c1}{c2}' at column {col}"))
            })?;
            grid.set(row, col as u16, shape);
        }
    }

    Ok(grid)
}

/// Where the strict parse ([`parse_document_from_str`]) stopped, and why.
///
/// The location is kept out of the message so that a report can put it where
/// it puts every other location: a file that fails to parse is an `error:` in
/// the same list as the rest, and one whose line lived only in its text could
/// be sent nowhere but the top of the file. `Display` still spells it out, for
/// the callers that have nowhere else to show it.
#[derive(Debug)]
pub struct ParseError {
    /// The DocLine index (0-based) of the line the parse stopped on — the index
    /// [`parse_doclines`](super::lines::parse_doclines) gives the same line, which is what the editor opens.
    pub line: usize,
    /// The 1-based file line of the same line.
    pub file_line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.file_line, self.message)
    }
}

impl std::error::Error for ParseError {}
