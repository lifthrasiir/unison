//! The links on a document line, and the one the caret sits on.

use super::super::*;

/// Every link on one document line: the ones its directive states, plus the
/// glyph names a `// …` comment on it happens to mention.
///
/// The two are collected together so that a Ctrl/Cmd+click and its keyboard
/// form (Ctrl/Cmd+`]`) see one list, and so a comment word never has to be a
/// case of its own downstream — a link to a glyph is a link to a glyph
/// whichever half of the line it was written on.
pub(super) fn line_links(
    lines: &[DocLine],
    doc_line: usize,
    text: &str,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
) -> Vec<LinkSpan> {
    let mut links = doc_links::extract_line_links(
        text,
        crate::document::at_base_at_line(lines, doc_line).as_deref(),
    );
    doc_links::extract_comment_links(text, &|name| named_glyphs.contains_key(name), &mut links);
    links
}

/// The link the caret is sitting on, for the keyboard form of a Ctrl/Cmd+click.
///
/// Overlaps are resolved the way the pointer resolves them — the shortest span
/// wins — so a `$var` inside a pattern name is reached rather than the name
/// that encloses it.
pub(super) fn link_at_caret(
    lines: &[DocLine],
    caret: Caret,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
) -> Option<LinkSpan> {
    let DocLine::Text(text) = lines.get(caret.line)? else {
        return None;
    };
    line_links(lines, caret.line, text, named_glyphs)
        .into_iter()
        .filter(|l| caret.col >= l.col_start && caret.col <= l.col_end)
        .min_by_key(|l| l.col_end - l.col_start)
}
