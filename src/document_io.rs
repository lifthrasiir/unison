use std::fmt;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::document::*;
use crate::pixel::{chars_to_shape, shape_to_chars};

// ---------------------------------------------------------------------------
// Backtick-quoting tokenizer
// ---------------------------------------------------------------------------

/// Tokenize a line into tokens using backtick-quoting rules.
///
/// - Tokens are separated by whitespace.
/// - A token starting with `` ` `` is a quoted token: content runs until the
///   next `` ` ``. Inside the quotes, ` `` ` (two consecutive backticks)
///   represents a literal backtick character; a single `` ` `` ends the quote.
/// - After the closing `` ` ``, the next character must be whitespace or end
///   of input, otherwise an error is returned.
/// - Outside of quotes, backticks are ordinary characters.
pub fn tokenize_tokens(line: &str) -> std::result::Result<Vec<String>, String> {
    Ok(tokenize_with_spans(line)?.into_iter().map(|t| t.value).collect())
}

/// Quote a token for serialization. Wraps in backticks when the value is
/// empty, starts with a backtick, or contains whitespace; internal backticks
/// are doubled.
pub fn quote_token(s: &str) -> String {
    if !s.is_empty() && !s.starts_with('`') && !s.contains(char::is_whitespace) {
        s.to_string()
    } else {
        let escaped = s.replace('`', "``");
        format!("`{escaped}`")
    }
}

/// A token with its character-offset span in the original line (for editor
/// click/hover). `raw_start..raw_end` covers the full raw representation
/// including backtick delimiters.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct TokenSpan {
    pub value: String,
    pub raw_start: usize,
    pub raw_end: usize,
}

/// Like [`tokenize_tokens`] but also returns character-offset spans for each
/// token in the original line.
pub fn tokenize_with_spans(line: &str) -> std::result::Result<Vec<TokenSpan>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        let raw_start = i;
        if chars[i] == '`' {
            i += 1;
            let mut value = String::new();
            loop {
                if i >= chars.len() {
                    return Err("unclosed backtick quote".into());
                }
                if chars[i] == '`' {
                    if i + 1 < chars.len() && chars[i + 1] == '`' {
                        value.push('`');
                        i += 2;
                    } else {
                        i += 1;
                        if i < chars.len() && !chars[i].is_whitespace() {
                            return Err(format!(
                                "expected whitespace after closing backtick, got '{}'",
                                chars[i],
                            ));
                        }
                        break;
                    }
                } else {
                    value.push(chars[i]);
                    i += 1;
                }
            }
            tokens.push(TokenSpan { value, raw_start, raw_end: i });
        } else {
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            tokens.push(TokenSpan {
                value: chars[raw_start..i].iter().collect(),
                raw_start,
                raw_end: i,
            });
        }
    }

    Ok(tokens)
}

fn parse_visibility(s: &str) -> Option<LayerVisibility> {
    match s {
        "coloronly" => Some(LayerVisibility::ColorOnly),
        "monoonly" => Some(LayerVisibility::MonoOnly),
        _ => None,
    }
}

/// Parse the tokens after `ref` into a `GlyphRef`.
///
/// Accepted forms:
/// - `ref NAME`
/// - `ref NAME negated`
/// - `ref NAME COL ROW [negated]`
/// - Any of the above followed by `fill COLOR` and/or `coloronly`/`monoonly`
///   (`fill` and visibility are independent; either can appear without the other)
fn parse_ref_line(parts: &[String]) -> Option<GlyphRef> {
    if parts.is_empty() {
        return None;
    }
    let name = parts[0].clone();
    let mut idx = 1;
    let mut offset: Option<(i16, i16)> = None;
    let mut negated = false;
    let mut fill: Option<RefFill> = None;
    let mut visibility: Option<LayerVisibility> = None;

    // Try to parse COL ROW
    if idx + 1 < parts.len()
        && let Ok(col) = parts[idx].parse::<i16>()
        && let Ok(row) = parts[idx + 1].parse::<i16>()
    {
        offset = Some((col, row));
        idx += 2;
    }

    while idx < parts.len() {
        match parts[idx].as_str() {
            "negated" => negated = true,
            "fill" => {
                idx += 1;
                if idx >= parts.len() {
                    return None;
                }
                fill = Some(RefFill { color: parts[idx].clone() });
            }
            s => {
                if let Some(vis) = parse_visibility(s) {
                    visibility = Some(vis);
                } else {
                    return None;
                }
            }
        }
        idx += 1;
    }

    Some(GlyphRef { name, offset, negated, fill, visibility })
}

pub fn parse_document(path: &Path) -> Result<Document> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_document_from_str(&content, path.to_path_buf())
}

/// Parse a range token like `3` (single value) or `3..5` (inclusive range).
fn parse_range_token(s: &str) -> Option<(i16, i16)> {
    if let Some((start_s, end_s)) = s.split_once("..") {
        let start: i16 = start_s.parse().ok()?;
        let end: i16 = end_s.parse().ok()?;
        if end < start {
            return None;
        }
        Some((start, end))
    } else {
        let v: i16 = s.parse().ok()?;
        Some((v, v))
    }
}

/// Parse an anchor/point from its three token parts: position, col_range, row_range.
fn parse_anchor_point(position: &str, col_tok: &str, row_tok: &str) -> Option<GlyphPoint> {
    let (col, col_end) = parse_range_token(col_tok)?;
    let (row, row_end) = parse_range_token(row_tok)?;
    Some(GlyphPoint { position: position.to_string(), col, row, col_end, row_end })
}

/// Parsed dimensions of a `glyph NAME W H [OFF_ROW OFF_COL]` header, i.e. a
/// header that expects pixel rows to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphHeaderDims {
    pub width: u16,
    pub height: u16,
    pub scale: u8,
}

/// Glyph header flags/dimensions, as parsed by [`parse_glyph_flag_parts`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphHeaderFlags {
    pub sticky: bool,
    pub inline: bool,
    pub mark: bool,
    pub advance: Option<u16>,
    pub left: Option<i16>,
    pub top: Option<i16>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub scale: Option<u8>,
}

/// Parse the flag tokens of a `glyph NAME ...` header (everything after the
/// name, with any `= ALIAS` part already stripped).
///
/// This is the single implementation of the header flag grammar: keyword
/// flags (`sticky`, `inline`, `mark`), valued flags (`advance N`, `left N`,
/// `top N`) and the `W H` dimension pair may appear in any order. It is
/// shared by `derive_document` and [`glyph_header_dims`] so that the
/// document model and grid reconciliation can never disagree about whether
/// a header owns a pixel grid.
pub fn parse_glyph_flag_parts<S: AsRef<str>>(flag_parts: &[S]) -> GlyphHeaderFlags {
    let mut flags = GlyphHeaderFlags::default();
    let mut fp = 0;
    while fp < flag_parts.len() {
        match flag_parts[fp].as_ref() {
            "sticky" => flags.sticky = true,
            "inline" => flags.inline = true,
            "mark" => flags.mark = true,
            "advance" => {
                fp += 1;
                if fp < flag_parts.len() {
                    flags.advance = flag_parts[fp].as_ref().parse().ok();
                }
            }
            "left" => {
                fp += 1;
                if fp < flag_parts.len() {
                    flags.left = flag_parts[fp].as_ref().parse().ok();
                }
            }
            "top" => {
                fp += 1;
                if fp < flag_parts.len() {
                    flags.top = flag_parts[fp].as_ref().parse().ok();
                }
            }
            "scale" => {
                fp += 1;
                if fp < flag_parts.len() {
                    flags.scale = flag_parts[fp].as_ref().parse().ok();
                }
            }
            other => {
                if flags.width.is_none()
                    && let Ok(w) = other.parse::<u16>() {
                        flags.width = Some(w);
                        fp += 1;
                        if fp < flag_parts.len() {
                            flags.height = flag_parts[fp].as_ref().parse().ok();
                        }
                        fp += 1;
                        continue;
                    }
            }
        }
        fp += 1;
    }
    flags
}

/// Parse the whitespace-split tokens of a `glyph ...` header (with the glyph
/// name at index 0) to determine whether pixel rows follow, and if so their
/// dimensions.
///
/// Returns `None` for ref-only headers (`glyph NAME`) or simple aliases
/// (`glyph NAME = ALIAS`). Handles keyword flags like `sticky`, `advance N`,
/// `left N` appearing before or after `W H`.
pub fn glyph_header_dims<S: AsRef<str>>(parts: &[S]) -> Option<GlyphHeaderDims> {
    if parts.is_empty() {
        return None;
    }
    if parts.iter().any(|p| p.as_ref() == "=") {
        return None;
    }
    let flags = parse_glyph_flag_parts(&parts[1..]);
    let (width, height) = (flags.width?, flags.height?);
    let scale = flags.scale.unwrap_or(1);
    Some(GlyphHeaderDims {
        width: width.checked_mul(scale as u16)?,
        height: height.checked_mul(scale as u16)?,
        scale,
    })
}

/// Parse `.unf` source text into a `Document`.
///
/// This tokenizes the text into `DocLine`s (validating pixel rows strictly
/// along the way, via [`parse_pixel_rows`]) and then feeds them through
/// [`derive_document`], which is the single implementation of the
/// item-level `.unf` grammar (comments, font-meta, directives, glyphs, refs)
/// shared with the `DocLine`-based editor path.
pub fn parse_document_from_str(content: &str, path: std::path::PathBuf) -> Result<Document> {
    let lines = tokenize_strict(content)?;
    let (doc, _) = derive_document(&lines, path).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(doc)
}

/// Tokenize `.unf` source text into `DocLine`s, strictly validating any
/// pixel rows that follow a `glyph NAME W H [OFF_ROW OFF_COL]` header (see
/// [`parse_pixel_rows`]). All other lines (comments, font-meta, directives,
/// ref lines, alias/ref-only glyph headers) are passed through as-is; their
/// grammar is interpreted later by [`derive_document`].
fn tokenize_strict(content: &str) -> Result<Vec<DocLine>> {
    let mut lines = Vec::new();
    let mut iter = content.lines().enumerate().peekable();

    while let Some((line_no, line)) = iter.next() {
        let trimmed = line.trim();

        // Comments are free text — `derive_document` passes them through
        // verbatim. Tokenizing them anyway meant a backtick in prose aborted
        // the whole file.
        if trimmed.starts_with("//") {
            lines.push(DocLine::Text(line.to_string()));
            continue;
        }

        let tokens = tokenize_tokens(trimmed)
            .map_err(|e| anyhow::anyhow!("line {}: {}", line_no + 1, e))?;

        if tokens.first().is_some_and(|t| t == "glyph") {
            let parts = &tokens[1..];
            validate_glyph_header(parts, line_no)?;
            lines.push(DocLine::Text(line.to_string()));

            if let Some(dims) = glyph_header_dims(parts) {
                if is_pixel_row_next(&mut iter, dims.width) {
                    let grid = parse_pixel_rows(&mut iter, dims.width, dims.height, line_no)?;
                    lines.push(DocLine::Grid(grid));
                } else {
                    lines.push(DocLine::Grid(PixelGrid::new(dims.width, dims.height)));
                }
            }
        } else {
            lines.push(DocLine::Text(line.to_string()));
        }
    }

    Ok(lines)
}

fn validate_glyph_header<S: AsRef<str>>(parts: &[S], line_no: usize) -> Result<()> {
    if parts.is_empty() {
        bail!("line {}: empty glyph name", line_no + 1);
    }
    let rest = &parts[1..];

    // glyph NAME = ALIAS
    if let Some(eq_pos) = rest.iter().position(|p| p.as_ref() == "=") {
        // Validate tokens before '=' are valid flags
        validate_glyph_flags(&rest[..eq_pos], line_no)?;
        if eq_pos + 1 != rest.len() - 1 {
            if eq_pos + 1 >= rest.len() {
                bail!("line {}: missing alias target after '='", line_no + 1);
            }
            // Extra tokens after alias target
            let extra: Vec<&str> = rest[eq_pos + 2..].iter().map(|s| s.as_ref()).collect();
            bail!(
                "line {}: unexpected tokens after alias target: {}",
                line_no + 1,
                extra.join(" "),
            );
        }
        return Ok(());
    }

    validate_glyph_flags(rest, line_no)
}

fn validate_glyph_flags<S: AsRef<str>>(tokens: &[S], line_no: usize) -> Result<()> {
    let mut i = 0;
    let mut width_seen = false;
    while i < tokens.len() {
        match tokens[i].as_ref() {
            "sticky" | "inline" | "mark" => i += 1,
            "advance" | "scale" => {
                let kw = tokens[i].as_ref().to_string();
                i += 1;
                if i >= tokens.len() || tokens[i].as_ref().parse::<u16>().is_err() {
                    bail!(
                        "line {}: '{}' requires a numeric value",
                        line_no + 1,
                        kw,
                    );
                }
                i += 1;
            }
            "left" | "top" => {
                let kw = tokens[i].as_ref().to_string();
                i += 1;
                if i >= tokens.len() || tokens[i].as_ref().parse::<i16>().is_err() {
                    bail!(
                        "line {}: '{}' requires an i16 value",
                        line_no + 1,
                        kw,
                    );
                }
                i += 1;
            }
            other => {
                if !width_seen && other.parse::<u16>().is_ok() {
                    width_seen = true;
                    i += 1;
                    if i < tokens.len() && tokens[i].as_ref().parse::<u16>().is_ok() {
                        i += 1;
                    } else if i < tokens.len() && !matches!(
                        tokens[i].as_ref(),
                        "sticky" | "inline" | "mark" | "advance" | "left" | "top" | "scale",
                    ) {
                        bail!(
                            "line {}: expected height after width, got '{}'",
                            line_no + 1,
                            tokens[i].as_ref(),
                        );
                    }
                } else {
                    bail!(
                        "line {}: unrecognized glyph header token '{}'",
                        line_no + 1,
                        other,
                    );
                }
            }
        }
    }
    Ok(())
}

fn is_pixel_row_next(
    lines: &mut std::iter::Peekable<std::iter::Enumerate<std::str::Lines<'_>>>,
    width: u16,
) -> bool {
    let Some(&(_, line)) = lines.peek() else { return false };
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
    header_line: usize,
) -> Result<PixelGrid> {
    let mut grid = PixelGrid::new(width, height);

    for row in 0..height {
        let (line_no, line) = lines.next().ok_or_else(|| {
            anyhow::anyhow!(
                "line {}: expected {} pixel rows, got {}",
                header_line + 1,
                height,
                row,
            )
        })?;

        let chars: Vec<char> = line.chars().collect();
        let expected_len = width as usize * 2;
        if chars.len() != expected_len {
            bail!(
                "line {}: expected {} chars ({} pixel columns × 2), got {}",
                line_no + 1,
                expected_len,
                width,
                chars.len(),
            );
        }

        for col in 0..width as usize {
            let c1 = chars[col * 2];
            let c2 = chars[col * 2 + 1];
            let shape = chars_to_shape(c1, c2).ok_or_else(|| {
                anyhow::anyhow!(
                    "line {}: unknown pixel pair '{}{}' at column {}",
                    line_no + 1,
                    c1,
                    c2,
                    col,
                )
            })?;
            grid.set(row, col as u16, shape);
        }
    }

    Ok(grid)
}

pub fn serialize_document(doc: &Document, writer: &mut dyn Write) -> Result<()> {
    for item in &doc.items {
        match item {
            DocumentItem::BlankLine => writeln!(writer)?,
            DocumentItem::Comment(text) => writeln!(writer, "//{text}")?,
            DocumentItem::FontMeta(text) => writeln!(writer, "font-meta {text}")?,
            DocumentItem::Directive(text) => writeln!(writer, "{text}")?,
            item @ DocumentItem::NameParts { .. }
            | item @ DocumentItem::Remap { .. }
            | item @ DocumentItem::Feature { .. }
            | item @ DocumentItem::FeatureAnchor { .. }
            | item @ DocumentItem::Color { .. }
            | item @ DocumentItem::AssertShape { .. }
            | item @ DocumentItem::AssertSame { .. }
            | item @ DocumentItem::AssertDistinct { .. } => {
                if let Some(line) = item.serialize_line() {
                    writeln!(writer, "{line}")?;
                }
            }
            DocumentItem::Glyph { name, body } => {
                serialize_glyph(writer, name, body)?;
            }
            DocumentItem::Map { char_repr, glyph } => {
                writeln!(writer, "map {} = {}", quote_token(char_repr), quote_token(glyph))?;
            }
            DocumentItem::MapDecomposed { char_repr } => {
                writeln!(writer, "map {}", quote_token(char_repr))?;
            }
        }
    }
    Ok(())
}

/// Encode a single pixel row of `grid` as a string of 2-char pixel codes.
pub fn encode_grid_row(grid: &PixelGrid, row: u16) -> String {
    let mut s = String::with_capacity(grid.width as usize * 2);
    for col in 0..grid.width {
        let [c1, c2] = shape_to_chars(grid.get(row, col));
        s.push(c1);
        s.push(c2);
    }
    s
}

fn format_glyph_flags(body: &GlyphBody) -> String {
    let mut flags = String::new();
    if body.sticky {
        flags.push_str(" sticky");
    }
    if body.inline {
        flags.push_str(" inline");
    }
    if body.mark {
        flags.push_str(" mark");
    }
    if let Some(adv) = body.advance {
        flags.push_str(&format!(" advance {adv}"));
    }
    if let Some(left) = body.left {
        flags.push_str(&format!(" left {left}"));
    }
    if let Some(top) = body.top {
        flags.push_str(&format!(" top {top}"));
    }
    if body.scale > 1 {
        flags.push_str(&format!(" scale {}", body.scale));
    }
    flags
}

fn serialize_glyph(writer: &mut dyn Write, name: &GlyphName, body: &GlyphBody) -> Result<()> {
    let flags = format_glyph_flags(body);
    let qname = quote_token(&name.display());

    // Simple alias: glyph NAME [flags] = ALIAS
    if body.is_simple_alias() {
        writeln!(writer, "glyph {qname}{flags} = {}", quote_token(&body.refs[0].name))?;
        return Ok(());
    }

    if let Some(grid) = &body.pixels {
        let s = body.scale as u16;
        writeln!(writer, "glyph {qname} {} {}{flags}", grid.width / s, grid.height / s)?;
        if !grid.is_all_empty() {
            for row in 0..grid.height {
                writeln!(writer, "{}", encode_grid_row(grid, row))?;
            }
        }
    } else {
        writeln!(writer, "glyph {qname}{flags}")?;
    }
    for r in &body.refs {
        writeln!(writer, "{}", r.format_line(None))?;
    }
    for p in &body.points {
        let col_s = if p.col == p.col_end {
            format!("{}", p.col)
        } else {
            format!("{}..{}", p.col, p.col_end)
        };
        let row_s = if p.row == p.row_end {
            format!("{}", p.row)
        } else {
            format!("{}..{}", p.row, p.row_end)
        };
        writeln!(writer, "anchor {} {} {}", quote_token(&p.position), col_s, row_s)?;
    }
    Ok(())
}

/// Convert old `= ..` range format to standard `glyph`/`ref` format.
/// `glyph NAME = ..\n\tbody1 ..\n\tbody2` becomes `glyph NAME\nref body1 0 0\nref body2 0 0`.
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn parse_doclines(content: &str) -> Vec<DocLine> {
    let mut lines = Vec::new();
    let mut iter = content.lines().peekable();

    while let Some(line) = iter.next() {
        let trimmed = line.trim();

        let is_glyph = tokenize_tokens(trimmed)
            .ok()
            .and_then(|tokens| {
                if tokens.first().is_some_and(|t| t == "glyph") {
                    glyph_header_dims(&tokens[1..])
                } else {
                    None
                }
            });

        if let Some(dims) = is_glyph {
            lines.push(DocLine::Text(line.to_string()));
            let width = dims.width;
            let height = dims.height;
            let mut grid = PixelGrid::new(width, height);
            for row in 0..height {
                let is_pixel = iter.peek().is_some_and(|peek_line| {
                    let chars: Vec<char> = peek_line.chars().collect();
                    chars.len() == width as usize * 2
                        && (0..width as usize).all(|col| {
                            chars_to_shape(chars[col * 2], chars[col * 2 + 1]).is_some()
                        })
                });
                if !is_pixel {
                    break;
                }
                if let Some(pixel_line) = iter.next() {
                    let chars: Vec<char> = pixel_line.chars().collect();
                    for col in 0..width as usize {
                        let idx = col * 2;
                        if idx + 1 < chars.len()
                            && let Some(shape) = chars_to_shape(chars[idx], chars[idx + 1]) {
                                grid.set(row, col as u16, shape);
                            }
                    }
                }
            }
            lines.push(DocLine::Grid(grid));
        } else {
            lines.push(DocLine::Text(line.to_string()));
        }
    }

    lines
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn serialize_doclines(lines: &[DocLine], writer: &mut dyn Write) -> Result<()> {
    for line in lines {
        match line {
            DocLine::Text(s) => writeln!(writer, "{s}")?,
            DocLine::Grid(g) => {
                if !g.is_all_empty() {
                    for row in 0..g.height {
                        writeln!(writer, "{}", encode_grid_row(g, row))?;
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Derive Document from Vec<DocLine> (replaces reparse)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DeriveError(pub String);

impl fmt::Display for DeriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "derive error: {}", self.0)
    }
}

impl std::error::Error for DeriveError {}

pub fn derive_document(
    lines: &[DocLine],
    path: std::path::PathBuf,
) -> std::result::Result<(Document, Vec<usize>), DeriveError> {
    let mut doc = Document::new(path);
    let mut item_line_starts: Vec<usize> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        match &lines[i] {
            DocLine::Grid(_) => {
                // Orphan grid — skip (reconciliation should prevent this)
                i += 1;
            }
            DocLine::Text(s) => {
                let trimmed = s.trim();

                if trimmed.is_empty() {
                    item_line_starts.push(i);
                    doc.items.push(DocumentItem::BlankLine);
                    i += 1;
                    continue;
                }

                if let Some(comment) = trimmed.strip_prefix("//") {
                    item_line_starts.push(i);
                    doc.items.push(DocumentItem::Comment(comment.to_string()));
                    i += 1;
                    continue;
                }

                let tokens = tokenize_tokens(trimmed)
                    .map_err(DeriveError)?;
                if tokens.is_empty() {
                    item_line_starts.push(i);
                    doc.items.push(DocumentItem::BlankLine);
                    i += 1;
                    continue;
                }

                match tokens[0].as_str() {
                    "font-meta" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> = tokens[1..].iter().map(|t| quote_token(t)).collect();
                        doc.items.push(DocumentItem::FontMeta(rest.join(" ")));
                        i += 1;
                    }
                    "exclude-from-sample" | "assume" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> = tokens[1..].iter().map(|t| quote_token(t)).collect();
                        doc.items.push(DocumentItem::Directive(
                            format!("{} {}", tokens[0], rest.join(" ")),
                        ));
                        i += 1;
                    }
                    "map" => {
                        if tokens.len() == 4 && tokens[2] == "=" {
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::Map {
                                char_repr: tokens[1].clone(),
                                glyph: tokens[3].clone(),
                            });
                            i += 1;
                        } else if tokens.len() == 2 {
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::MapDecomposed {
                                char_repr: tokens[1].clone(),
                            });
                            i += 1;
                        } else {
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                            i += 1;
                        }
                    }
                    "glyph" => {
                        let header_idx = i;
                        i += 1;

                        let parts = &tokens[1..];
                        if parts.is_empty() {
                            return Err(DeriveError("empty glyph name".into()));
                        }

                        let name = parse_glyph_name(&parts[0]);

                        let rest_parts = &parts[1..];
                        let (alias, flag_parts) = if let Some(eq_pos) = rest_parts.iter().position(|p| p == "=") {
                            let alias = if eq_pos + 1 < rest_parts.len() {
                                Some(rest_parts[eq_pos + 1].clone())
                            } else {
                                None
                            };
                            (alias, &rest_parts[..eq_pos])
                        } else {
                            (None, rest_parts)
                        };

                        let mut body = GlyphBody::new();
                        let flags = parse_glyph_flag_parts(flag_parts);
                        body.sticky = flags.sticky;
                        body.inline = flags.inline;
                        body.mark = flags.mark;
                        body.advance = flags.advance;
                        body.left = flags.left;
                        body.top = flags.top;
                        body.scale = flags.scale.unwrap_or(1);
                        let scale = body.scale as u16;
                        let (width, height) = (
                            flags.width.and_then(|w| w.checked_mul(scale)),
                            flags.height.and_then(|h| h.checked_mul(scale)),
                        );

                        if let Some(alias_name) = alias {
                            body.refs.push(GlyphRef {
                                name: alias_name,
                                offset: None,
                                negated: false,
                                fill: None,
                                visibility: None,
                            });
                            item_line_starts.push(header_idx);
                            doc.items.push(DocumentItem::Glyph { name, body });
                            continue;
                        }

                        if let (Some(w), Some(h)) = (width, height) {
                            if let Some(DocLine::Grid(g)) = lines.get(i)
                                && g.width == w && g.height == h {
                                    body.pixels = Some(g.clone());
                                    i += 1;
                                } else {
                                    body.pixels = Some(PixelGrid::new(w, h));
                                }
                        }

                        // Collect ref and point lines
                        while let Some(DocLine::Text(t)) = lines.get(i) {
                            let rt = t.trim();
                            let sub_tokens = match tokenize_tokens(rt) {
                                Ok(t) => t,
                                Err(_) => break,
                            };
                            if sub_tokens.first().is_some_and(|t| t == "ref") {
                                let parsed_ref = parse_ref_line(&sub_tokens[1..]);
                                let Some(parsed_ref) = parsed_ref else {
                                    break;
                                };
                                body.refs.push(parsed_ref);
                                i += 1;
                                continue;
                            } else if sub_tokens.first().is_some_and(|t| t == "point" || t == "anchor") {
                                let point_parts = &sub_tokens[1..];
                                if point_parts.len() == 3
                                    && let Some(pt) = parse_anchor_point(&point_parts[0], &point_parts[1], &point_parts[2]) {
                                        body.points.push(pt);
                                        i += 1;
                                        continue;
                                    }
                                break;
                            } else {
                                break;
                            }
                        }

                        item_line_starts.push(header_idx);
                        doc.items.push(DocumentItem::Glyph { name, body });
                    }
                    "name-parts" | "remap" | "feature" | "assert" => {
                        item_line_starts.push(i);
                        doc.items.push(DocumentItem::parse_directive(&tokens));
                        i += 1;
                    }
                    "color" => {
                        item_line_starts.push(i);
                        if tokens.len() >= 4 && tokens[2] == "=" {
                            let visibility = match tokens.get(4).map(|s| s.as_str()) {
                                Some("coloronly") => Some(LayerVisibility::ColorOnly),
                                Some("monoonly") => Some(LayerVisibility::MonoOnly),
                                _ => None,
                            };
                            doc.items.push(DocumentItem::Color {
                                name: tokens[1].clone(),
                                value: tokens[3].clone(),
                                visibility,
                            });
                        } else {
                            doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                        }
                        i += 1;
                    }
                    _ => {
                        item_line_starts.push(i);
                        doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                        i += 1;
                    }
                }
            }
        }
    }

    doc.item_line_starts = item_line_starts.clone();
    doc.docline_file_lines = crate::document::compute_docline_file_lines(lines);
    Ok((doc, item_line_starts))
}

// Write via temp file + rename to work around macOS SMB server silently
// ignoring file truncation (https://github.com/rust-lang/rust/issues/159054).
pub fn write_and_sync(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp_path = dir.join(format!(
        ".~{}",
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    let mut f = std::fs::File::create(&tmp_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    f.write_all(data)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    f.sync_all()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    drop(f);
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(anyhow::anyhow!("{e}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `glyph_header_dims` and `derive_document` must agree on which headers
    /// own a pixel grid — a disagreement leaves reconciliation and the
    /// document model permanently fighting over the grid DocLine.
    #[test]
    fn header_dims_match_derive_for_valued_flags() {
        // Valued flags may precede W H; their argument is not a dimension.
        let dims = glyph_header_dims(&["foo", "advance", "0", "4", "3"]);
        assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 3, scale: 1 }));

        let dims = glyph_header_dims(&["foo", "left", "2", "3"]);
        assert_eq!(dims, None, "width 3 without height is not a grid header");

        let dims = glyph_header_dims(&["foo", "4", "3", "advance", "0"]);
        assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 3, scale: 1 }));

        let dims = glyph_header_dims(&["foo", "sticky", "4", "3"]);
        assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 3, scale: 1 }));

        // Cross-check against derive_document on the same headers.
        for (header, expected) in [
            ("glyph foo advance 0 4 3", Some((4u16, 3u16))),
            ("glyph foo left 2 3", None),
            ("glyph foo 4 3 advance 0", Some((4, 3))),
        ] {
            let lines = vec![DocLine::Text(header.to_string())];
            let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
            let Some(DocumentItem::Glyph { body, .. }) = doc.items.first() else {
                panic!("expected glyph item for {header:?}");
            };
            let derived = body.pixels.as_ref().map(|g| (g.width, g.height));
            assert_eq!(derived, expected, "derive mismatch for {header:?}");
            let tokens = tokenize_tokens(header).unwrap();
            let dims = glyph_header_dims(&tokens[1..])
                .map(|d| (d.width, d.height));
            assert_eq!(dims, expected, "glyph_header_dims mismatch for {header:?}");
        }
    }

    #[test]
    fn roundtrip_simple() {
        let input = "\
// test comment
font-meta height 16 ascent 14 descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph U+0041
ref test-glyph 2 0

exclude-from-sample U+AD00
";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        // comment, font-meta, blank, glyph, blank, glyph, blank, directive = 8
        assert_eq!(doc.items.len(), 8);

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let doc2 = parse_document_from_str(&output_str, "test2.unf".into()).unwrap();
        assert_eq!(doc2.items.len(), doc.items.len());
    }

    #[test]
    fn anchor_range_roundtrip() {
        let input = "\
glyph foo 2 2
@@@@
@@@@
anchor +join 1..3 0..2
anchor -bar 5 7
";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert_eq!(body.points.len(), 2);
            let p0 = &body.points[0];
            assert_eq!(p0.position, "+join");
            assert_eq!((p0.col, p0.col_end), (1, 3));
            assert_eq!((p0.row, p0.row_end), (0, 2));
            assert_eq!(p0.width(), 3);
            assert_eq!(p0.height(), 3);
            let p1 = &body.points[1];
            assert_eq!(p1.position, "-bar");
            assert_eq!((p1.col, p1.col_end), (5, 5));
            assert_eq!((p1.row, p1.row_end), (7, 7));
            assert!(p1.is_single_cell());
        } else {
            panic!("expected glyph");
        }

        // Roundtrip through serialize_document
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("anchor +join 1..3 0..2"));
        assert!(output_str.contains("anchor -bar 5 7"));

        // Re-parse the serialized output
        let doc2 = parse_document_from_str(&output_str, "test2.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc2.items[0] {
            assert_eq!(body.points.len(), 2);
            assert_eq!(body.points[0].width(), 3);
            assert!(body.points[1].is_single_cell());
        } else {
            panic!("expected glyph on re-parse");
        }
    }

    #[test]
    fn legacy_point_parsed_as_single_cell_anchor() {
        let input = "glyph foo 1 1\n@@\npoint +bar 3 5\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert_eq!(body.points.len(), 1);
            let p = &body.points[0];
            assert_eq!(p.position, "+bar");
            assert_eq!((p.col, p.col_end), (3, 3));
            assert_eq!((p.row, p.row_end), (5, 5));
            assert!(p.is_single_cell());
        } else {
            panic!("expected glyph");
        }
    }

    #[test]
    fn parse_glyph_with_all_shapes() {
        let input = "glyph shapes 4 1\n..@@1\\1>\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            let grid = body.pixels.as_ref().expect("expected glyph with pixels");
            assert!(grid.get(0, 0).is_empty());
            assert_eq!(grid.get(0, 1).shape_id(), crate::pixel::PX_ALMOSTFULL);
            assert!(grid.get(0, 1).is_filled());
            assert_eq!(grid.get(0, 2).shape_id(), crate::pixel::PX_HALF1);
            assert!(grid.get(0, 2).is_filled());
            assert_eq!(grid.get(0, 3).shape_id(), crate::pixel::PX_QUAD1);
            assert!(grid.get(0, 3).is_filled());
        } else {
            panic!("expected glyph");
        }
    }

    #[test]
    fn parse_glyph_without_pixel_rows() {
        let input = "glyph empty 4 3\nref other 0 0\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            let grid = body.pixels.as_ref().expect("should produce empty grid");
            assert_eq!(grid.width, 4);
            assert_eq!(grid.height, 3);
            assert!(grid.is_all_empty());
            assert_eq!(body.refs.len(), 1);
            assert_eq!(body.refs[0].name, "other");
        } else {
            panic!("expected glyph");
        }

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, "glyph empty 4 3\nref other 0 0\n");
    }

    #[test]
    fn roundtrip_alias() {
        let input = "glyph U+0041 = test-glyph\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::Glyph { name, body } = &doc.items[0] {
            assert_eq!(name.display(), "U+0041");
            assert!(body.pixels.is_none());
            assert_eq!(body.refs.len(), 1);
            assert_eq!(body.refs[0].name, "test-glyph");
            assert_eq!(body.refs[0].row(), 0);
            assert_eq!(body.refs[0].col(), 0);
        } else {
            panic!("expected glyph");
        }

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, "glyph U+0041 = test-glyph\n");
    }

    #[test]
    fn explicit_zero_ref_roundtrips_as_explicit() {
        let input = "glyph composite\nref target 0 0\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected glyph");
        };
        assert_eq!(body.refs[0].offset, Some((0, 0)));
        assert!(!body.is_simple_alias());

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output, input);

        let reparsed = parse_document_from_str(&output, "test2.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &reparsed.items[0] else {
            panic!("expected glyph");
        };
        assert_eq!(body.refs[0].offset, Some((0, 0)));
    }

    #[test]
    fn derive_accepts_only_complete_ref_forms() {
        let input = "\
glyph composite
ref auto
ref auto-negated negated
ref explicit 0 0
ref explicit-negated 1 -2 negated
";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected glyph");
        };
        assert_eq!(body.refs.len(), 4);
        assert_eq!(body.refs[0].offset, None);
        assert!(!body.refs[0].negated);
        assert_eq!(body.refs[1].offset, None);
        assert!(body.refs[1].negated);
        assert_eq!(body.refs[2].offset, Some((0, 0)));
        assert!(!body.refs[2].negated);
        assert_eq!(body.refs[3].offset, Some((1, -2)));
        assert!(body.refs[3].negated);
    }

    #[test]
    fn malformed_ref_is_not_reinterpreted_as_auto_ref() {
        for malformed in [
            "ref target 1",
            "ref target garbage",
            "ref target 32768 0",
            "ref target 1 2 extra",
            "ref target negated extra",
        ] {
            let input = format!("glyph composite\n{malformed}\n");
            let doc = parse_document_from_str(&input, "test.unf".into()).unwrap();
            assert_eq!(doc.items.len(), 2, "input: {malformed}");
            let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
                panic!("expected glyph for input: {malformed}");
            };
            assert!(body.refs.is_empty(), "input: {malformed}");
            assert!(
                matches!(&doc.items[1], DocumentItem::Directive(line) if line == malformed),
                "input: {malformed}",
            );
        }
    }


    // -----------------------------------------------------------------------
    // DocLine round-trip tests
    // -----------------------------------------------------------------------

    fn docline_roundtrip(input: &str) {
        let lines = parse_doclines(input);
        let mut output = Vec::new();
        serialize_doclines(&lines, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(input, output_str, "serialize_doclines did not round-trip");
    }

    #[test]
    fn docline_roundtrip_simple() {
        let input = "\
// test comment
font-meta height 16 ascent 14 descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph U+0041
ref test-glyph 2 0

exclude-from-sample U+AD00
";
        docline_roundtrip(input);

        let lines = parse_doclines(input);
        // comment, font-meta, blank, glyph-header, Grid, blank, glyph-header, ref, blank, directive
        assert_eq!(lines.len(), 10);
        assert!(matches!(lines[0], DocLine::Text(ref s) if s.starts_with("//")));
        assert!(matches!(lines[3], DocLine::Text(ref s) if s.starts_with("glyph test-glyph")));
        assert!(matches!(lines[4], DocLine::Grid(_)));
        assert!(matches!(lines[6], DocLine::Text(ref s) if s.starts_with("glyph U+0041")));
        assert!(matches!(lines[7], DocLine::Text(ref s) if s.starts_with("ref ")));
    }

    #[test]
    fn docline_roundtrip_alias() {
        docline_roundtrip("glyph U+0041 = test-glyph\n");
        let lines = parse_doclines("glyph U+0041 = test-glyph\n");
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0], DocLine::Text(_)));
    }


    #[test]
    fn docline_roundtrip_ref_only_glyph() {
        let input = "\
glyph composite
ref part-a 0 0
ref part-b 4 2
";
        docline_roundtrip(input);
        let lines = parse_doclines(input);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| matches!(l, DocLine::Text(_))));
    }

    #[test]
    fn docline_roundtrip_glyph_with_pixels_and_refs() {
        let input = "\
glyph mixed 2 2
..@@
@@..
ref other 1 1
";
        docline_roundtrip(input);
        let lines = parse_doclines(input);
        assert_eq!(lines.len(), 3);
        assert!(matches!(lines[0], DocLine::Text(_)));
        assert!(matches!(lines[1], DocLine::Grid(_)));
        assert!(matches!(lines[2], DocLine::Text(ref s) if s.starts_with("ref ")));
    }

    // -----------------------------------------------------------------------
    // derive_document equivalence tests
    // -----------------------------------------------------------------------

    fn assert_derive_equivalent(input: &str) {
        let old_doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let lines = parse_doclines(input);
        let (new_doc, starts) = derive_document(&lines, "test.unf".into()).unwrap();

        assert_eq!(
            old_doc.items.len(),
            new_doc.items.len(),
            "item count mismatch"
        );
        assert_eq!(starts.len(), new_doc.items.len());

        for (idx, (old_item, new_item)) in
            old_doc.items.iter().zip(new_doc.items.iter()).enumerate()
        {
            match (old_item, new_item) {
                (DocumentItem::BlankLine, DocumentItem::BlankLine) => {}
                (DocumentItem::Comment(a), DocumentItem::Comment(b)) => {
                    assert_eq!(a, b, "comment mismatch at item {idx}");
                }
                (DocumentItem::FontMeta(a), DocumentItem::FontMeta(b)) => {
                    assert_eq!(a, b, "font-meta mismatch at item {idx}");
                }
                (DocumentItem::Directive(a), DocumentItem::Directive(b)) => {
                    assert_eq!(a, b, "directive mismatch at item {idx}");
                }
                (
                    DocumentItem::NameParts { name: n1, values: v1 },
                    DocumentItem::NameParts { name: n2, values: v2 },
                ) => {
                    assert_eq!(n1, n2, "name-parts name mismatch at item {idx}");
                    assert_eq!(v1, v2, "name-parts values mismatch at item {idx}");
                }
                (
                    DocumentItem::Remap { feature: f1, lookbehind: lb1, source: s1, target: t1, lookahead: la1 },
                    DocumentItem::Remap { feature: f2, lookbehind: lb2, source: s2, target: t2, lookahead: la2 },
                ) => {
                    assert_eq!(f1, f2, "remap feature mismatch at item {idx}");
                    assert_eq!(lb1, lb2, "remap lookbehind mismatch at item {idx}");
                    assert_eq!(s1, s2, "remap source mismatch at item {idx}");
                    assert_eq!(t1, t2, "remap target mismatch at item {idx}");
                    assert_eq!(la1, la2, "remap lookahead mismatch at item {idx}");
                }
                (
                    DocumentItem::Feature { name: n1, scripts: s1, remap_group: r1 },
                    DocumentItem::Feature { name: n2, scripts: s2, remap_group: r2 },
                ) => {
                    assert_eq!(n1, n2, "feature name mismatch at item {idx}");
                    assert_eq!(s1, s2, "feature scripts mismatch at item {idx}");
                    assert_eq!(r1, r2, "feature remap_group mismatch at item {idx}");
                }
                (
                    DocumentItem::Glyph {
                        name: n1,
                        body: b1,
                    },
                    DocumentItem::Glyph {
                        name: n2,
                        body: b2,
                    },
                ) => {
                    assert_eq!(
                        n1.display(),
                        n2.display(),
                        "name mismatch at item {idx}"
                    );
                    assert_eq!(
                        b1.pixels, b2.pixels,
                        "pixels mismatch at item {idx}"
                    );
                    assert_eq!(
                        b1.refs.len(),
                        b2.refs.len(),
                        "ref count mismatch at item {idx}"
                    );
                    for (ri, (r1, r2)) in b1.refs.iter().zip(b2.refs.iter()).enumerate() {
                        assert_eq!(r1.name, r2.name, "ref name mismatch at item {idx} ref {ri}");
                        assert_eq!(r1.offset, r2.offset, "ref offset mismatch at item {idx} ref {ri}");
                        assert_eq!(r1.negated, r2.negated, "ref negation mismatch at item {idx} ref {ri}");
                    }
                }
                _ => panic!(
                    "item kind mismatch at item {idx}: {:?} vs {:?}",
                    std::mem::discriminant(old_item),
                    std::mem::discriminant(new_item),
                ),
            }
        }
    }

    #[test]
    fn derive_equivalent_simple() {
        assert_derive_equivalent(
            "\
// test comment
font-meta height 16 ascent 14 descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph U+0041
ref test-glyph 2 0

exclude-from-sample U+AD00
",
        );
    }

    #[test]
    fn derive_equivalent_alias() {
        assert_derive_equivalent("glyph U+0041 = test-glyph\n");
    }


    #[test]
    fn derive_equivalent_mixed_refs() {
        assert_derive_equivalent(
            "\
glyph mixed 2 2
..@@
@@..
ref other 1 1
",
        );
    }

    #[test]
    fn derive_item_line_starts() {
        let input = "\
// comment
glyph foo 2 1
..@@
ref bar 0 0
";
        let lines = parse_doclines(input);
        let (doc, starts) = derive_document(&lines, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 2);
        assert_eq!(starts, vec![0, 1]); // comment at line 0, glyph header at line 1
    }

    // -----------------------------------------------------------------------
    // Intermediate editing states (derive_document tolerance)
    // -----------------------------------------------------------------------

    #[test]
    fn derive_empty_body_glyph() {
        let input = "glyph foo\n";
        let lines = parse_doclines(input);
        let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert!(body.pixels.is_none());
            assert!(body.refs.is_empty());
        } else {
            panic!("expected glyph");
        }
    }

    #[test]
    fn derive_glyph_header_split_from_alias() {
        let input = "glyph foo\n= bar\n";
        let lines = parse_doclines(input);
        let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 2);
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert!(body.pixels.is_none());
            assert!(body.refs.is_empty());
        } else {
            panic!("expected glyph at item 0");
        }
        assert!(matches!(doc.items[1], DocumentItem::Directive(_)));
    }

    #[test]
    fn derive_glyph_with_dims_no_grid_docline() {
        // Simulates editing state: header with dims but Grid DocLine removed
        let lines = vec![DocLine::Text("glyph foo 8 16".to_string())];
        let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            let grid = body.pixels.as_ref().expect("should have empty grid");
            assert_eq!(grid.width, 8);
            assert_eq!(grid.height, 16);
            assert!(grid.is_all_empty());
            assert!(body.refs.is_empty());
        } else {
            panic!("expected glyph");
        }
    }

    #[test]
    fn docline_roundtrip_all_directive_types() {
        let input = "\
font-meta height 16 ascent 12 descent 4

// a comment
name-parts $base = stem wide

glyph stem 2 2
@@@@
..@@

glyph wide 3 1
@@..@@

glyph alias = stem

glyph comp
ref stem
ref wide 1 0
point -join 0 0
point +join 2 0

glyph batch
ref stem-(a|b)

glyph sticky-empty sticky advance 0

map A = stem
map B = wide
remap set1 : stem -> wide
remap liga1 : stem wide -> batch
remap liga2 : stem wide -> batch stem
feature liga for latn : set1
exclude-from-sample stem
";
        let lines = parse_doclines(input);
        let mut output = Vec::new();
        serialize_doclines(&lines, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(input, output_str, "DocLine round-trip failed");

        let old_doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let (new_doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
        assert_eq!(
            old_doc.items.len(),
            new_doc.items.len(),
            "item count mismatch"
        );
    }

    #[test]
    fn strict_parse_rejects_partial_glyph_header() {
        let input = "glyph x 2 nope\n..@@\n";
        assert!(parse_document_from_str(input, "test.unf".into()).is_err());
    }

    #[test]
    fn strict_parse_accepts_valid_glyph_headers() {
        for input in [
            "glyph foo\n",
            "glyph foo 2 1\n..@@\n",
            "glyph foo sticky\n",
            "glyph foo 2 1 sticky\n..@@\n",
            "glyph foo advance 5\n",
            "glyph foo left -1\n",
            "glyph foo 2 1 sticky advance 5 left -1\n..@@\n",
            "glyph foo = bar\n",
            "glyph foo sticky = bar\n",
        ] {
            assert!(
                parse_document_from_str(input, "test.unf".into()).is_ok(),
                "should accept: {input:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Backtick-quoting tokenizer tests
    // -----------------------------------------------------------------------

    #[test]
    fn comment_lines_are_not_tokenized() {
        // A comment is free text; backticks in it are not quoting syntax.
        // Tokenizing comments made one stray backtick abort the whole file,
        // and the CLI build then silently proceeded without that file.
        let input = "// see `foo`/`bar`\nglyph a 2 1\n@@..\n";
        let doc = parse_document_from_str(input, "test.unf".into())
            .expect("comment with backticks must parse");
        assert!(matches!(&doc.items[0], DocumentItem::Comment(_)));
    }

    #[test]
    fn tokenize_simple_whitespace() {
        assert_eq!(
            tokenize_tokens("hello world").unwrap(),
            vec!["hello", "world"],
        );
    }

    #[test]
    fn tokenize_empty_string() {
        assert!(tokenize_tokens("").unwrap().is_empty());
        assert!(tokenize_tokens("   ").unwrap().is_empty());
    }

    #[test]
    fn tokenize_unquoted_backtick() {
        // a`b = 3 chars, single unquoted token
        assert_eq!(tokenize_tokens("a`b").unwrap(), vec!["a`b"]);
    }

    #[test]
    fn tokenize_quoted_empty() {
        // `` = empty string
        assert_eq!(tokenize_tokens("``").unwrap(), vec![""]);
    }

    #[test]
    fn tokenize_quoted_backtick() {
        // ```` = one backtick character
        assert_eq!(tokenize_tokens("````").unwrap(), vec!["`"]);
    }

    #[test]
    fn tokenize_quoted_with_spaces() {
        // `a b` = "a b" (3 chars)
        assert_eq!(tokenize_tokens("`a b`").unwrap(), vec!["a b"]);
    }

    #[test]
    fn tokenize_quoted_error_no_space() {
        // `ab`c = error
        assert!(tokenize_tokens("`ab`c").is_err());
    }

    #[test]
    fn tokenize_unclosed_quote() {
        assert!(tokenize_tokens("`abc").is_err());
    }

    #[test]
    fn tokenize_mixed() {
        assert_eq!(
            tokenize_tokens("glyph `foo bar` 8 16").unwrap(),
            vec!["glyph", "foo bar", "8", "16"],
        );
    }

    #[test]
    fn tokenize_multiple_quoted() {
        assert_eq!(
            tokenize_tokens("`` `a` ````").unwrap(),
            vec!["", "a", "`"],
        );
    }

    #[test]
    fn tokenize_quoted_with_escaped_backtick() {
        // `a``b` = "a`b"
        assert_eq!(tokenize_tokens("`a``b`").unwrap(), vec!["a`b"]);
    }

    #[test]
    fn quote_token_simple() {
        assert_eq!(quote_token("hello"), "hello");
    }

    #[test]
    fn quote_token_empty() {
        assert_eq!(quote_token(""), "``");
    }

    #[test]
    fn quote_token_with_space() {
        assert_eq!(quote_token("a b"), "`a b`");
    }

    #[test]
    fn quote_token_backtick() {
        assert_eq!(quote_token("`"), "````");
    }

    #[test]
    fn quote_token_starts_with_backtick() {
        assert_eq!(quote_token("`foo"), "```foo`");
    }

    #[test]
    fn quote_roundtrip() {
        for val in ["", "hello", "a b", "`", "a`b", "`foo", "``", "a b c"] {
            let quoted = quote_token(val);
            let parsed = tokenize_tokens(&quoted).unwrap();
            assert_eq!(parsed, vec![val], "roundtrip failed for {val:?}");
        }
    }

    #[test]
    fn parse_map_with_quoted_backtick() {
        // map ```` = bquot  →  map backtick-char to "bquot"
        let input = "map ```` = bquot\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Map { char_repr, glyph } = &doc.items[0] {
            assert_eq!(char_repr, "`");
            assert_eq!(glyph, "bquot");
        } else {
            panic!("expected Map");
        }

        // Roundtrip
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn parse_name_parts_with_empty_token() {
        // name-parts $init0 = `` $init
        let input = "name-parts $init0 = `` $init\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::NameParts { name, values } = &doc.items[0] {
            assert_eq!(name, "$init0");
            assert_eq!(values, &vec!["".to_string(), "$init".to_string()]);
        } else {
            panic!("expected NameParts");
        }

        // Roundtrip
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn parse_glyph_with_quoted_name() {
        let input = "`glyph` `foo bar` 2 1\n..@@\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { name, body } = &doc.items[0] {
            assert_eq!(name.display(), "foo bar");
            assert!(body.pixels.is_some());
        } else {
            panic!("expected Glyph");
        }
    }

    #[test]
    fn tokenize_with_spans_basic() {
        let spans = tokenize_with_spans("glyph `foo` 8").unwrap();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].value, "glyph");
        assert_eq!(spans[0].raw_start, 0);
        assert_eq!(spans[0].raw_end, 5);
        assert_eq!(spans[1].value, "foo");
        assert_eq!(spans[1].raw_start, 6);
        assert_eq!(spans[1].raw_end, 11); // includes backticks
        assert_eq!(spans[2].value, "8");
        assert_eq!(spans[2].raw_start, 12);
        assert_eq!(spans[2].raw_end, 13);
    }

    #[test]
    fn roundtrip_color_directive() {
        let input = "color red = #ff0000\ncolor blue = #0000ffcc coloronly\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 2);
        if let DocumentItem::Color { name, value, visibility } = &doc.items[0] {
            assert_eq!(name, "red");
            assert_eq!(value, "#ff0000");
            assert!(visibility.is_none());
        } else {
            panic!("expected Color");
        }
        if let DocumentItem::Color { name, value, visibility } = &doc.items[1] {
            assert_eq!(name, "blue");
            assert_eq!(value, "#0000ffcc");
            assert_eq!(*visibility, Some(LayerVisibility::ColorOnly));
        } else {
            panic!("expected Color");
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn roundtrip_ref_fill() {
        let input = "\
glyph combo
ref part-a fill #ff0000
ref part-b 2 3 fill fg coloronly
ref part-c fill blue monoonly
";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert_eq!(body.refs.len(), 3);
            let r0 = &body.refs[0];
            assert_eq!(r0.name, "part-a");
            assert_eq!(r0.offset, None);
            let f0 = r0.fill.as_ref().unwrap();
            assert_eq!(f0.color, "#ff0000");
            assert!(r0.visibility.is_none());

            let r1 = &body.refs[1];
            assert_eq!(r1.name, "part-b");
            assert_eq!(r1.offset, Some((2, 3)));
            let f1 = r1.fill.as_ref().unwrap();
            assert_eq!(f1.color, "fg");
            assert_eq!(r1.visibility, Some(LayerVisibility::ColorOnly));

            let r2 = &body.refs[2];
            assert_eq!(r2.fill.as_ref().unwrap().color, "blue");
            assert_eq!(r2.visibility, Some(LayerVisibility::MonoOnly));
        } else {
            panic!("expected Glyph");
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn ref_visibility_without_fill() {
        let input = "\
glyph combo
ref part-a coloronly
ref part-b monoonly
ref part-c fill #ff0000 monoonly
";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            assert_eq!(body.refs.len(), 3);

            let r0 = &body.refs[0];
            assert!(r0.fill.is_none());
            assert_eq!(r0.visibility, Some(LayerVisibility::ColorOnly));

            let r1 = &body.refs[1];
            assert!(r1.fill.is_none());
            assert_eq!(r1.visibility, Some(LayerVisibility::MonoOnly));

            let r2 = &body.refs[2];
            assert_eq!(r2.fill.as_ref().unwrap().color, "#ff0000");
            assert_eq!(r2.visibility, Some(LayerVisibility::MonoOnly));
        } else {
            panic!("expected Glyph");
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn ref_fill_negated_combined() {
        let input = "glyph foo\nref bar 1 2 negated fill #00ff00\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
            let r = &body.refs[0];
            assert_eq!(r.name, "bar");
            assert_eq!(r.offset, Some((1, 2)));
            assert!(r.negated);
            assert_eq!(r.fill.as_ref().unwrap().color, "#00ff00");
        } else {
            panic!("expected Glyph");
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn parse_assert_shape_basic() {
        let input = "assert shape `AB` : a-upper : b-upper\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::AssertShape { text, features, expected, comment } = &doc.items[0] {
            assert_eq!(text, "AB");
            assert!(features.is_empty());
            assert_eq!(expected.len(), 2);
            assert_eq!(expected[0].name, "a-upper");
            assert_eq!(expected[1].name, "b-upper");
            assert!(expected[0].advance.is_none());
            assert!(comment.is_none());
        } else {
            panic!("expected AssertShape");
        }
    }

    #[test]
    fn parse_assert_shape_with_features_and_props() {
        let input = "assert shape `fi` +liga -frac : fi-lig advance 512 : x offset 10 20\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::AssertShape { text, features, expected, .. } = &doc.items[0] {
            assert_eq!(text, "fi");
            assert_eq!(features.len(), 2);
            assert_eq!(features[0].tag, "liga");
            assert!(features[0].enable);
            assert_eq!(features[1].tag, "frac");
            assert!(!features[1].enable);
            assert_eq!(expected.len(), 2);
            assert_eq!(expected[0].name, "fi-lig");
            assert_eq!(expected[0].advance, Some(512));
            assert_eq!(expected[1].name, "x");
            assert_eq!(expected[1].offset, Some((10, 20)));
        } else {
            panic!("expected AssertShape");
        }
    }

    #[test]
    fn roundtrip_assert_shape() {
        let input = "assert shape `AB` +liga : a-upper advance 512 : b-upper offset 10 20\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, "assert shape AB +liga : a-upper advance 512 : b-upper offset 10 20\n");
    }

    #[test]
    fn roundtrip_assert_shape_quoted_text() {
        let input = "assert shape `hello world` : hw\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn parse_assert_same() {
        let input = "assert same foo bar\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::AssertSame { names, comment } = &doc.items[0] {
            assert_eq!(names, &["foo", "bar"]);
            assert!(comment.is_none());
        } else {
            panic!("expected AssertSame, got {:?}", doc.items[0]);
        }
    }

    #[test]
    fn parse_assert_distinct() {
        let input = "assert distinct a b c\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert_eq!(doc.items.len(), 1);
        if let DocumentItem::AssertDistinct { names, .. } = &doc.items[0] {
            assert_eq!(names, &["a", "b", "c"]);
        } else {
            panic!("expected AssertDistinct, got {:?}", doc.items[0]);
        }
    }

    #[test]
    fn roundtrip_assert_same() {
        let input = "assert same foo bar baz\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), input);
    }

    #[test]
    fn roundtrip_assert_distinct() {
        let input = "assert distinct foo bar\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), input);
    }

    #[test]
    fn assert_same_too_few_names_falls_back() {
        let input = "assert same foo\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        assert!(matches!(&doc.items[0], DocumentItem::Directive(_)));
    }

    #[test]
    fn roundtrip_assert_same_quoted() {
        let input = "assert same `foo bar` `baz quux`\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::AssertSame { names, .. } = &doc.items[0] {
            assert_eq!(names, &["foo bar", "baz quux"]);
        } else {
            panic!("expected AssertSame, got {:?}", doc.items[0]);
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), input);
    }

    #[test]
    fn parse_assert_same_with_comment() {
        let input = "assert same foo bar // they should match\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::AssertSame { names, comment } = &doc.items[0] {
            assert_eq!(names, &["foo", "bar"]);
            assert_eq!(comment.as_deref(), Some("they should match"));
        } else {
            panic!("expected AssertSame, got {:?}", doc.items[0]);
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), input);
    }

    #[test]
    fn roundtrip_assert_shape_with_comment() {
        let input = "assert shape AB : a-upper : b-upper // check shaping\n";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        if let DocumentItem::AssertShape { comment, .. } = &doc.items[0] {
            assert_eq!(comment.as_deref(), Some("check shaping"));
        } else {
            panic!("expected AssertShape, got {:?}", doc.items[0]);
        }
        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), input);
    }

    #[test]
    fn scale_roundtrip() {
        let input = "\
glyph flag 10 5 scale 2
....................@@@@@@@@@@..........@@@@@@@@@@
....................@@@@@@@@@@..........@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
";
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else { panic!() };
        assert_eq!(body.scale, 2);
        let grid = body.pixels.as_ref().unwrap();
        assert_eq!((grid.width, grid.height), (20, 10));

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, input);
    }

    #[test]
    fn scale_header_dims() {
        let dims = glyph_header_dims(&["foo", "10", "5", "scale", "2"]);
        assert_eq!(dims, Some(GlyphHeaderDims { width: 20, height: 10, scale: 2 }));

        let dims = glyph_header_dims(&["foo", "scale", "3", "4", "2"]);
        assert_eq!(dims, Some(GlyphHeaderDims { width: 12, height: 6, scale: 3 }));

        let dims = glyph_header_dims(&["foo", "4", "2"]);
        assert_eq!(dims, Some(GlyphHeaderDims { width: 4, height: 2, scale: 1 }));
    }
}
