//! The tokens of a `ref`, `anchor`, IDC and `glyph` header line: `parse_ref_line`, the
//! glyph flag walker, and the in-place header rewrites the editor uses.

use crate::document::*;

#[cfg(any(feature = "editor", test))]
use super::tokens::{split_comment, tokenize_with_spans};

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
/// - Any of the above followed by `inherit`, `goto`, `fill COLOR` and/or
///   `coloronly`/`monoonly`, in any order (each is independent of the others)
///
/// `base` is the `@` base in force — the last glyph name declared without one.
pub(super) fn parse_ref_line(
    parts: &[String],
    comment: Option<String>,
    base: Option<&str>,
) -> Option<GlyphRef> {
    if parts.is_empty() {
        return None;
    }
    let name = crate::document::expand_at_name(&parts[0], base);
    let raw_name = crate::document::written_form(&parts[0], &name);
    let mut idx = 1;
    let mut offset: Option<(i16, i16)> = None;
    let mut negated = false;
    let mut inherit = false;
    let mut goto = false;
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
            "inherit" => inherit = true,
            "goto" => goto = true,
            "fill" => {
                idx += 1;
                if idx >= parts.len() {
                    return None;
                }
                fill = Some(RefFill {
                    color: parts[idx].clone(),
                });
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

    Some(GlyphRef {
        name,
        raw_name,
        offset,
        negated,
        inherit,
        goto,
        fill,
        visibility,
        comment,
    })
}

/// Parse an IDC line — the tokens after the operator — into a [`GlyphCompose`].
///
/// A token that reads as a number is a [`ComposeItem::Gap`], anything else is a
/// component name; arity, sizes and the layout are [`crate::compose`]'s
/// business, so a line that tokenizes at all parses here. That includes what a
/// number *means*: a gap on a split and an offset on an enclosure, which is a
/// question about the operator and not about the token. `base` is the `@` base
/// in force, as for a `ref`.
///
/// A token with a `|` outside any parentheses is a
/// [`ComposeItem::Nested`], read piece by piece the same way; `(a|b)` is still
/// the one pattern component it always was. A nested split with an empty piece
/// (`a||b`, `|a`) is the one thing here that does not read, and the line is
/// then unread like a `ref` that does not parse.
pub(super) fn parse_compose_line(
    op: crate::compose::IdcOp,
    assumed: bool,
    parts: &[String],
    comment: Option<String>,
    base: Option<&str>,
) -> Option<GlyphCompose> {
    let item = |token: &str| match token.parse::<i16>() {
        Ok(gap) => ComposeItem::Gap(gap),
        Err(_) => {
            let name = crate::document::expand_at_name(token, base);
            let raw_name = crate::document::written_form(token, &name);
            ComposeItem::Part { name, raw_name }
        }
    };
    let items = parts
        .iter()
        .map(|token| {
            if token.chars().count() > 1 && crate::pattern::has_top_level_pipe(token) {
                let pieces = crate::pattern::split_top_level_pipes(token);
                if pieces.iter().any(|p| p.is_empty()) {
                    return None;
                }
                Some(ComposeItem::Nested(pieces.into_iter().map(item).collect()))
            } else {
                Some(item(token))
            }
        })
        .collect::<Option<Vec<_>>>()?;
    Some(GlyphCompose {
        op,
        items,
        assumed,
        comment,
    })
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

/// Parse an anchor/point from its three token parts: position, col_range, row_range,
/// on a grid at `scale`.
pub(super) fn parse_anchor_point(
    position: &str,
    col_tok: &str,
    row_tok: &str,
    scale: u8,
    comment: Option<String>,
) -> Option<GlyphPoint> {
    let (col, col_end) = parse_range_token(col_tok)?;
    let (row, row_end) = parse_range_token(row_tok)?;
    Some(GlyphPoint {
        position: position.to_string(),
        col,
        row,
        col_end,
        row_end,
        scale: scale.max(1).into(),
        comment,
    })
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
    pub keep: bool,
    pub inline: bool,
    pub mark: bool,
    pub desync: bool,
    pub vectoronly: bool,
    pub advance: Option<u16>,
    pub origin: Option<(i16, i16)>,
    pub extent: Option<(u16, u16)>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub scale: Option<u8>,
    pub margin: crate::document::Margin,
    /// Index of the width token *within the flag parts* (i.e. one less than
    /// its index in the whole header), and of the height token beside it.
    /// [`replace_glyph_header_dims`] rewrites exactly those two tokens rather
    /// than reformatting the line, which is what keeps a resize from
    /// reordering flags or dropping a comment.
    pub width_at: Option<usize>,
    pub height_at: Option<usize>,
}

/// Parse the flag tokens of a `glyph NAME ...` header (everything after the
/// name, with any `= ALIAS` part already stripped).
///
/// This is the single implementation of the header flag grammar: keyword
/// flags (`keep`, `inline`, `mark`, `desync`, `vectoronly`), valued flags
/// (`advance N`,
/// `origin C R`, `extent W H`) and the `W H` dimension pair may appear in any
/// order. It is
/// shared by `derive_document` and [`glyph_header_dims`] so that the
/// document model and grid reconciliation can never disagree about whether
/// a header owns a pixel grid.
pub fn parse_glyph_flag_parts<S: AsRef<str>>(flag_parts: &[S]) -> GlyphHeaderFlags {
    parse_glyph_flag_parts_impl(flag_parts, &mut |_| {})
}

/// Every keyword a `glyph` header may carry, valued flags included. The
/// parser matches on it and the editor completes from it, so a flag added in
/// one place cannot go missing in the other.
pub const GLYPH_FLAG_KEYWORDS: [&str; 11] = [
    "keep",
    "inline",
    "mark",
    "desync",
    "vectoronly",
    "advance",
    "origin",
    "extent",
    "scale",
    "margin-x",
    "margin-y",
];

/// The one walker behind both the lenient parse and the strict validation.
/// `err` receives a message for each malformed token; the lenient caller
/// ignores them, the strict caller reports the first one.
/// The most cells a glyph grid may have along either side, scale applied. Far
/// past any real glyph (a few hundred), and far short of an allocation that
/// would stop the editor.
pub const MAX_GRID_SIDE: u32 = 4096;

pub(super) fn parse_glyph_flag_parts_impl<S: AsRef<str>>(
    flag_parts: &[S],
    err: &mut impl FnMut(String),
) -> GlyphHeaderFlags {
    let mut flags = GlyphHeaderFlags::default();
    let mut fp = 0;
    while fp < flag_parts.len() {
        match flag_parts[fp].as_ref() {
            "keep" => flags.keep = true,
            "inline" => flags.inline = true,
            "mark" => flags.mark = true,
            "desync" => flags.desync = true,
            "vectoronly" => flags.vectoronly = true,
            "advance" => {
                fp += 1;
                flags.advance = flag_parts.get(fp).and_then(|t| t.as_ref().parse().ok());
                if flags.advance.is_none() {
                    err("'advance' requires a numeric value".to_string());
                }
            }
            "scale" => {
                fp += 1;
                // Zero is not a scale: it would multiply the header's `W H`
                // away and leave the pixel rows below with no grid to belong
                // to. Rejected here so the error names the flag rather than
                // every row after it.
                flags.scale = flag_parts
                    .get(fp)
                    .and_then(|t| t.as_ref().parse().ok())
                    .filter(|&n| n > 0);
                if flags.scale.is_none() {
                    err("'scale' requires a numeric value of at least 1".to_string());
                }
            }
            // The only two-valued flags. Both components are required: a lone
            // number would be indistinguishable from the bare `W H` pair.
            "origin" => {
                let c = flag_parts.get(fp + 1).and_then(|t| t.as_ref().parse().ok());
                let r = flag_parts.get(fp + 2).and_then(|t| t.as_ref().parse().ok());
                flags.origin = c.zip(r);
                if flags.origin.is_none() {
                    err("'origin' requires two i16 values (column, row)".to_string());
                }
                fp += 2;
            }
            "extent" => {
                let w = flag_parts.get(fp + 1).and_then(|t| t.as_ref().parse().ok());
                let h = flag_parts.get(fp + 2).and_then(|t| t.as_ref().parse().ok());
                flags.extent = w.zip(h);
                if flags.extent.is_none() {
                    err("'extent' requires two u16 values (width, height)".to_string());
                }
                fp += 2;
            }
            keyword @ ("margin-x" | "margin-y") => {
                fp += 1;
                let side = if keyword == "margin-x" {
                    &mut flags.margin.x
                } else {
                    &mut flags.margin.y
                };
                if side.is_some() {
                    err(format!("'{keyword}' is stated twice"));
                }
                *side = flag_parts.get(fp).and_then(|t| parse_margin(t.as_ref()));
                if side.is_none() {
                    err(format!(
                        "'{keyword}' requires one value, or two written `N|M`"
                    ));
                }
            }
            other => {
                if flags.width.is_none()
                    && let Ok(w) = other.parse::<u16>()
                {
                    flags.width = Some(w);
                    flags.width_at = Some(fp);
                    fp += 1;
                    if fp < flag_parts.len() {
                        let next = flag_parts[fp].as_ref();
                        if let Ok(h) = next.parse::<u16>() {
                            flags.height = Some(h);
                            flags.height_at = Some(fp);
                        } else if GLYPH_FLAG_KEYWORDS.contains(&next) {
                            // A flag keyword right after a lone width:
                            // no height given, keyword handled next round.
                            continue;
                        } else {
                            err(format!("expected height after width, got '{next}'"));
                        }
                    }
                    fp += 1;
                    continue;
                }
                err(format!("unrecognized glyph header token '{other}'"));
            }
        }
        fp += 1;
    }
    // The grid a header asks for is allocated as soon as it is read — in the
    // editor, as soon as the caret leaves a header being typed a digit at a
    // time — so a size past any real glyph is an error here, and the lenient
    // parse reads a header that owns no grid rather than billions of cells.
    if let (Some(w), Some(h)) = (flags.width, flags.height) {
        let scale = u32::from(flags.scale.unwrap_or(1));
        let (cells_w, cells_h) = (u32::from(w) * scale, u32::from(h) * scale);
        if cells_w.max(cells_h) > MAX_GRID_SIDE {
            err(format!(
                "a {cells_w}×{cells_h} cell grid is past the limit of {MAX_GRID_SIDE} cells a side"
            ));
            flags.width = None;
            flags.height = None;
        }
    }
    // One slot, one spelling. The lenient parse keeps both values — it has no
    // way to report anything and something has to be shown — and the strict one
    // rejects the line, so nothing downstream has to pick a winner.
    if flags.advance.is_some() && flags.extent.is_some() {
        err("'advance' and 'extent' both state the declared box's width".to_string());
    }
    // `desync` keeps the grid out of the vector build; `vectoronly` puts the
    // vector drawing into the bitmap one. A glyph asking for both is asking
    // which of two drawings does not exist, so neither is assumed.
    if flags.desync && flags.vectoronly {
        err("'desync' and 'vectoronly' ask for opposite things".to_string());
    }
    flags
}

/// The value of a `margin-x` or `margin-y` flag: `N` for both sides, `N|M` for
/// each. The two sides are one token so that the flag always takes one, which
/// is what keeps it apart from the header's `W H` wherever it stands.
fn parse_margin(token: &str) -> Option<(u16, u16)> {
    match token.split_once('|') {
        None => token.parse().ok().map(|n| (n, n)),
        Some((lo, hi)) => Some((lo.parse().ok()?, hi.parse().ok()?)),
    }
}

/// Parse the whitespace-split tokens of a `glyph ...` header (with the glyph
/// name at index 0) to determine whether pixel rows follow, and if so their
/// dimensions.
///
/// Returns `None` for ref-only headers (`glyph NAME`) or simple aliases
/// (`glyph NAME = ALIAS`). Handles keyword flags like `keep`, `advance N`,
/// `origin C R` appearing before or after `W H`.
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

/// Rewrite the `W H` pair of a `glyph …` header line in place, leaving every
/// other character — the name's quoting, the flag order, the spacing and the
/// trailing comment — exactly as written.
///
/// The dimensions are *logical* pixels, as the file states them: a `scale N`
/// header divides by the scale on the way in and this writes the same
/// undivided numbers back. Returns `None` for a header that owns no grid
/// (a ref-only glyph or an alias), which has no pair to rewrite.
#[cfg(any(feature = "editor", test))]
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn replace_glyph_header_dims(line: &str, width: u16, height: u16) -> Option<String> {
    let spans = tokenize_with_spans(line).ok()?;
    if spans.first().map(|s| s.value.as_str()) != Some("glyph") {
        return None;
    }
    if spans.iter().any(|s| s.value == "=") {
        return None;
    }
    let flag_values: Vec<&str> = spans.iter().skip(2).map(|s| s.value.as_str()).collect();
    let flags = parse_glyph_flag_parts(&flag_values);
    // Both halves of the pair, in header-token indices.
    let wi = 2 + flags.width_at?;
    let hi = 2 + flags.height_at?;

    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 4);
    let mut cut = 0usize;
    for (idx, value) in [(wi, width), (hi, height)] {
        let span = spans.get(idx)?;
        out.extend(&chars[cut..span.raw_start]);
        out.push_str(&value.to_string());
        cut = span.raw_end;
    }
    out.extend(&chars[cut..]);
    Some(out)
}

/// Rewrite the declared-box flags of a `glyph …` header line, leaving every
/// other character — the name's quoting, the other flags, the spacing and the
/// trailing comment — exactly as written.
///
/// Each argument is the value the header should end up stating: `Some` writes
/// it (in place if the flag is already there, appended otherwise) and `None`
/// removes the flag. `advance` and `extent` state the same slot, so writing one
/// while leaving the other is rejected by the parser — pass `None` for the one
/// being replaced, as the box editor does.
///
/// Values are *logical* pixels, like every number a header states. Returns
/// `None` for a line that is not a glyph header, or for an alias, which has no
/// flags to carry.
#[cfg(any(feature = "editor", test))]
pub fn replace_glyph_box_flags(
    line: &str,
    origin: Option<(i16, i16)>,
    advance: Option<u16>,
    extent: Option<(u16, u16)>,
) -> Option<String> {
    let spans = tokenize_with_spans(line).ok()?;
    if spans.first().map(|s| s.value.as_str()) != Some("glyph") {
        return None;
    }
    if spans.iter().any(|s| s.value == "=") {
        return None;
    }

    let wanted: [(&str, Option<String>); 3] = [
        ("origin", origin.map(|(c, r)| format!("origin {c} {r}"))),
        ("advance", advance.map(|a| format!("advance {a}"))),
        ("extent", extent.map(|(w, h)| format!("extent {w} {h}"))),
    ];

    // Where each flag stands today: the keyword's span and the span of its last
    // value, so a replacement covers the whole flag and a removal takes the
    // space before it too.
    let chars: Vec<char> = line.chars().collect();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut appended = String::new();
    for (keyword, value) in wanted {
        let values = if keyword == "advance" { 1 } else { 2 };
        // Past the keyword and the name: a glyph may be *called* `origin`.
        let at = spans
            .iter()
            .skip(2)
            .position(|s| s.value == keyword)
            .map(|i| i + 2);
        match (at, value) {
            (Some(i), Some(text)) => {
                let start = spans[i].raw_start;
                let end = spans
                    .get(i + values)
                    .map_or(spans[i].raw_end, |s| s.raw_end);
                edits.push((start, end, text));
            }
            (Some(i), None) => {
                let start = spans[i].raw_start;
                let end = spans
                    .get(i + values)
                    .map_or(spans[i].raw_end, |s| s.raw_end);
                // The separating space goes with the flag; without it a removal
                // in the middle of a header leaves a double space behind.
                let start = chars[..start]
                    .iter()
                    .rposition(|c| !c.is_whitespace())
                    .map_or(start, |p| p + 1);
                edits.push((start, end, String::new()));
            }
            (None, Some(text)) => {
                appended.push(' ');
                appended.push_str(&text);
            }
            (None, None) => {}
        }
    }

    // Appended flags go at the end of the code, i.e. before the comment.
    if !appended.is_empty() {
        let code_len = split_comment(line).0.chars().count();
        let end = chars[..code_len]
            .iter()
            .rposition(|c| !c.is_whitespace())
            .map_or(code_len, |p| p + 1);
        edits.push((end, end, appended));
    }

    edits.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(line.len() + 16);
    let mut cut = 0usize;
    for (start, end, text) in edits {
        if start < cut {
            return None; // overlapping flags: not something to rewrite blind
        }
        out.extend(&chars[cut..start]);
        out.push_str(&text);
        cut = end;
    }
    out.extend(&chars[cut..]);
    Some(out)
}
