//! The serializer: `serialize_document`, grid-row encoding, `serialize_doclines`, and the
//! file-level helpers (`is_source_file`, `write_and_sync`).

#[cfg(any(feature = "editor", test))]
use std::io::Write;
use std::path::Path;

#[cfg(any(feature = "editor", test))]
use anyhow::Result;

#[cfg(any(feature = "editor", test))]
use crate::document::*;
#[cfg(any(feature = "editor", test))]
use crate::pixel::{chars_to_shape, shape_to_chars};

#[cfg(any(feature = "editor", test))]
use super::tokens::{comment_suffix, quote_token, slice_prefix, write_map_chars};

#[cfg(any(feature = "editor", test))]
pub fn serialize_document(doc: &Document, writer: &mut dyn Write) -> Result<()> {
    for item in &doc.items {
        match item {
            DocumentItem::BlankLine => writeln!(writer)?,
            DocumentItem::Comment(text) => writeln!(writer, "//{text}")?,
            DocumentItem::Heading { level, text } => {
                let hashes = "#".repeat(*level as usize);
                if text.is_empty() {
                    writeln!(writer, "{hashes}")?;
                } else {
                    writeln!(writer, "{hashes} {text}")?;
                }
            }
            DocumentItem::Meta(text) => writeln!(writer, "meta {text}")?,
            DocumentItem::Audit(text) => writeln!(writer, "audit {text}")?,
            DocumentItem::Directive(text) => writeln!(writer, "{text}")?,
            item @ DocumentItem::Exists { .. }
            | item @ DocumentItem::Face { .. }
            | item @ DocumentItem::Slice { .. }
            | item @ DocumentItem::NameParts { .. }
            | item @ DocumentItem::Remap { .. }
            | item @ DocumentItem::RemapGroup { .. }
            | item @ DocumentItem::Feature { .. }
            | item @ DocumentItem::FeatureAnchor { .. }
            | item @ DocumentItem::Color { .. }
            | item @ DocumentItem::PropBlock { .. }
            | item @ DocumentItem::PropChar { .. }
            | item @ DocumentItem::AssertShape { .. }
            | item @ DocumentItem::AssertSame { .. }
            | item @ DocumentItem::AssertDistinct { .. } => {
                if let Some(line) = item.serialize_line() {
                    writeln!(writer, "{line}")?;
                }
            }
            DocumentItem::Sample { .. } => {
                for line in item.sample_lines().into_iter().flatten() {
                    writeln!(writer, "{line}")?;
                }
            }
            DocumentItem::Glyph { name, body } => {
                serialize_glyph(writer, name, body)?;
            }
            DocumentItem::GlyphAlias {
                name,
                target,
                raw_name,
                raw_target,
                comment,
                ..
            } => {
                writeln!(
                    writer,
                    "glyph {} = {}{}",
                    quote_token(raw_name.as_deref().unwrap_or(&name.0)),
                    quote_token(raw_target.as_deref().unwrap_or(target)),
                    comment_suffix(comment),
                )?;
            }
            DocumentItem::Map {
                slices,
                char_repr,
                selector,
                glyphs,
                comment,
            } => {
                let targets: Vec<String> = glyphs.iter().map(|g| quote_token(g)).collect();
                writeln!(
                    writer,
                    "map {}{} = {}{}",
                    slice_prefix(slices),
                    write_map_chars(char_repr, selector.as_deref()),
                    targets.join(" "),
                    comment_suffix(comment),
                )?;
            }
            DocumentItem::MapDecomposed {
                slices,
                char_repr,
                selector,
                glyph,
                comment,
            } => {
                let target = match glyph {
                    Some(g) => format!(" = {}", quote_token(g)),
                    None => String::new(),
                };
                writeln!(
                    writer,
                    "map {}generate {}{}{}",
                    slice_prefix(slices),
                    write_map_chars(char_repr, selector.as_deref()),
                    target,
                    comment_suffix(comment),
                )?;
            }
        }
    }
    Ok(())
}

/// Decode one line of text as a pixel row of `width` columns, or `None` if it
/// is not one — the inverse of [`encode_grid_row`], and the same test
/// [`is_pixel_row_next`](super::strict::is_pixel_row_next) applies while parsing a file. Both the promotion a
/// comment toggle does (`editor::comment`) and the parser have to agree on
/// what counts as a row, or a block would stop being a grid the moment it was
/// commented and uncommented again.
#[cfg(any(feature = "editor", test))]
#[cfg_attr(all(not(feature = "editor"), test), expect(dead_code))]
pub fn decode_grid_row(line: &str, width: u16) -> Option<Vec<crate::pixel::PixelShape>> {
    // Zero width encodes to the empty string, which every blank line would
    // match; see [`is_pixel_row_next`] for why that row is never read.
    if width == 0 {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.len() != width as usize * 2 {
        return None;
    }
    (0..width as usize)
        .map(|col| chars_to_shape(chars[col * 2], chars[col * 2 + 1]))
        .collect()
}

/// Encode a single pixel row of `grid` as a string of 2-char pixel codes.
#[cfg(any(feature = "editor", test))]
pub fn encode_grid_row(grid: &PixelGrid, row: u16) -> String {
    let mut s = String::with_capacity(grid.width as usize * 2);
    for col in 0..grid.width {
        let [c1, c2] = shape_to_chars(grid.get(row, col));
        s.push(c1);
        s.push(c2);
    }
    s
}

#[cfg(any(feature = "editor", test))]
fn format_glyph_flags(body: &GlyphBody) -> String {
    let mut flags = String::new();
    if body.keep {
        flags.push_str(" keep");
    }
    if body.inline {
        flags.push_str(" inline");
    }
    if body.mark {
        flags.push_str(" mark");
    }
    if body.desync {
        flags.push_str(" desync");
    }
    if body.vectoronly {
        flags.push_str(" vectoronly");
    }
    if let Some(adv) = body.advance {
        flags.push_str(&format!(" advance {adv}"));
    }
    if let Some((c, r)) = body.origin {
        flags.push_str(&format!(" origin {c} {r}"));
    }
    if let Some((w, h)) = body.extent {
        flags.push_str(&format!(" extent {w} {h}"));
    }
    if body.scale > 1 {
        flags.push_str(&format!(" scale {}", body.scale));
    }
    for (x_axis, margin) in [(true, body.margin.x), (false, body.margin.y)] {
        if let Some(margin) = margin {
            flags.push(' ');
            flags.push_str(&crate::document::Margin::flag(x_axis, margin));
        }
    }
    flags
}

#[cfg(any(feature = "editor", test))]
fn serialize_glyph(writer: &mut dyn Write, name: &GlyphName, body: &GlyphBody) -> Result<()> {
    let flags = format_glyph_flags(body);
    let qname = quote_token(body.raw_name.as_deref().unwrap_or(&name.0));

    let hcomment = comment_suffix(&body.comment);

    if let Some(grid) = &body.pixels {
        let s = body.scale as u16;
        writeln!(
            writer,
            "glyph {qname} {} {}{flags}{hcomment}",
            grid.width / s,
            grid.height / s
        )?;
        if !grid.is_all_empty() {
            for row in 0..grid.height {
                writeln!(writer, "{}", encode_grid_row(grid, row))?;
            }
        }
    } else {
        writeln!(writer, "glyph {qname}{flags}{hcomment}")?;
    }
    // Before the refs: an IDC line *is* the glyph's shape, and the refs a
    // block also carries are what is added on top of it.
    for c in &body.compose {
        writeln!(writer, "{}", c.format_line())?;
    }
    for r in &body.refs {
        writeln!(writer, "{}", r.format_line(None))?;
    }
    for p in &body.points {
        writeln!(writer, "{}", p.format_line())?;
    }
    Ok(())
}

#[cfg(any(feature = "editor", test))]
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

/// Whether a directory entry is one of a font project's source documents.
///
/// `.unf`, and not a dot-file. The second half is not cosmetic: `write_and_sync`
/// below stages every save as `.~name.unf` — a name that ends in `.unf` like
/// any other — so a directory read that catches a save in flight would
/// otherwise parse the staging file as a second copy of the document being
/// saved. Editors that leave their own dot-files behind are excluded with it.
///
/// The single answer for the question, shared by the directory loader, the
/// sidebar's list and the file watcher, so they cannot disagree about what the
/// project contains.
pub fn is_source_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "unf")
        && path
            .file_name()
            .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
}

// Write via temp file + rename to work around macOS SMB server silently
// ignoring file truncation (https://github.com/rust-lang/rust/issues/159054).
#[cfg(feature = "editor")]
pub fn write_and_sync(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp_path = dir.join(format!(
        ".~{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut f = std::fs::File::create(&tmp_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    f.write_all(data).map_err(|e| anyhow::anyhow!("{e}"))?;
    f.sync_all().map_err(|e| anyhow::anyhow!("{e}"))?;
    drop(f);
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(anyhow::anyhow!("{e}"));
    }
    Ok(())
}
