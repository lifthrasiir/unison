//! Parser and serializer for the `.unf` font source format — and the reference
//! for the format itself.
//!
//! Parsing is incremental at the line level: [`crate::document::DocLine`] is
//! what the editor edits, and a pixel-only edit does not reparse the file. The
//! editor canonicalizes every file through [`serialize_document`] when it opens
//! it, so anything the model drops on the way in is something the user loses —
//! comments included (below).
//!
//! # Names
//!
//! A glyph name is letters, digits, `-`, `.`, `_` and `:` — the last for a
//! variant suffix (`a-lower:compressed`). Every character the pattern syntax
//! uses (`(`, `)`, `|`, `$`, `*`, `#`) is excluded, so a pattern that failed to
//! expand cannot reach the font as a name that merely looks odd. The rule is
//! checked against *expanded* names by [`crate::issues`]; see
//! [`crate::document::is_valid_glyph_name`].
//!
//! Face and slice ids are narrower still — no `:`, and a face id additionally
//! becomes a file name, so it may not start with `.`; see [`crate::faces`].
//!
//! There is no `U+XXXX` glyph-name form. A range of hex-named glyphs is
//! `uni($#XXXX..YYYY)`, which is what `($#…)` was added for; `U+XXXX` remains a
//! *character* spelling on the left of a `map`, which is a different context.
//!
//! # Tokens
//!
//! Whitespace-separated, with backtick quoting for tokens containing spaces:
//! `` `foo bar` ``. A literal backtick is four backticks — two to escape, two
//! to quote.
//!
//! `//` starts a comment on every line *except* pixel rows, where `//` is a
//! legal pixel pair; see [`split_comment`] for the exact rule and why a pixel
//! row must never reach it. Comments are dropped by
//! [`tokenize_tokens`]/[`tokenize_with_spans`], so grammar, links, completion
//! and rename never see comment prose, and every item carries its own comment
//! (a `comment` field on the structured [`crate::document::DocumentItem`]
//! variants, on `GlyphBody`/`GlyphRef`/`GlyphPoint`, inline in the raw text of
//! `Meta`/`Directive`) so serializing does not lose it. Appending to a line
//! goes through `append_to_line`, which keeps the insertion in front of the
//! comment.
//!
//! # Directives
//!
//! - `meta KEY [@LANG] VALUE...` — font metadata, **one key per line**. Keys are
//!   variadic (a metric takes one number, `panose` takes ten, a flag takes
//!   none), which is why they do not share a line: with no separator, two keys
//!   on one line could not be told apart. Declaring the same slot twice is an
//!   error even when the two values agree, and `family` and `name 1` are one
//!   slot — see [`crate::meta`] for the key set, the `@LANG` language slot and
//!   the name IDs derived from what is declared, and [`crate::issues`] for the
//!   checks.
//!
//!   `meta FACE : KEY VALUE...` scopes a key to one face, and `meta * : ...`
//!   spells out the default of every face. The design metrics are every-face
//!   only. A bare key and a face-scoped one for the same slot conflict, since
//!   the bare one already reaches that face.
//! - `face FACE [: SLICE...]` — one typeface in the output. `slice SLICE
//!   [= SLICE...]` declares a slice, the `= ...` form being shorthand for
//!   including those too, transitively. See [`crate::faces`] for the model, and
//!   for the one rule that shapes how a split font is written: a character
//!   whose mapping differs between faces must not be in the base slice at all,
//!   because there is no override — every conflict is an error.
//! - `map CHAR = GLYPH` — cmap mapping.
//! - `map generate CHAR [= GLYPH]` — cmap mapping to a glyph synthesized from
//!   the character's Unicode canonical decomposition, named `uniXXXX` unless
//!   `GLYPH` names it. `GLYPH` is a pattern expanded in lock-step with `CHAR`,
//!   exactly as a plain `map`'s target is. The `generate` keyword is mandatory:
//!   the older bare `map CHAR` was too easily misread as the plain form. The
//!   synthesized refs carry `inherit` implicitly, since the composite stands in
//!   for its decomposition (see [`crate::ref_composite`] on anchor exposure) —
//!   so hand-rewriting one as a plain `glyph` + `map` means deciding per ref
//!   whether to keep `inherit`.
//!
//!   `map`, `feature`, `name-parts` and `assert shape` may be scoped to a
//!   slice. The first three take a `SLICE :` qualifier in front of what they
//!   already said (`map wide : ° = degree-wide`); `assert shape` takes
//!   `for SLICE...` before its first `:`, since it already uses `:` as a
//!   separator. Unqualified means the base slice, which every face includes —
//!   so every file written before faces existed keeps its meaning exactly.
//!
//!   The qualifier is told from the body by the *second* token being a bare
//!   `:`, which no name or value can be. That is what keeps `map : = colon` — a
//!   perfectly good mapping of U+003A — from reading as a qualifier, while
//!   `map wide : : = colon` still qualifies one.
//!
//!   A qualifier may list slices — `map wide|narrow : ⁂ = triple-star($half)` —
//!   which states the line once *per* slice. `for SLICE...` on an assertion
//!   means the opposite (a face including *all* of them), because the two
//!   answer different questions.
//! - `name-parts [SLICE[|SLICE...] :] $NAME = token1 token2 ...` — see
//!   [`crate::pattern`]. Each token is itself a name pattern, so
//!   `$foo = bar($1..3)` binds what `$foo = bar1 bar2 bar3` binds
//!   (`resolve_name_part_values` in [`crate::document`]).
//!   A slice-scoped binding takes exactly one value and
//!   applies only to lines stated for that slice, which is how a name that
//!   differs between slices by a suffix is written once instead of once per
//!   slice; see [`crate::document::SliceNameParts`].
//! - `color NAME = #RRGGBB[AA] [coloronly|monoonly]` — named palette entry.
//! - `remap FEATURE : [LOOKBEHIND... :] SOURCE... -> TARGET... [: LOOKAHEAD...]`
//!   — GSUB substitution. Source and target are *lists* of glyph names in all
//!   cases, and an empty target means removal. The list lengths pick the lookup
//!   type: 1→1 single, 1→N (including 1→0) multiple, N→1 ligature. N→M and N→0
//!   have no OpenType lookup type and are an error [`crate::issues`] reports,
//!   rather than something the builder emits close-but-wrong. Rules of one
//!   group are subtables of one lookup, so their order is match priority; see
//!   `render/ttf_builder/gsub.rs`.
//! - `remap group NAME [reversed] [after GROUP]...` — declares a remap group,
//!   carrying what belongs to the lookup rather than to a rule. Optional: an
//!   undeclared group is unreversed and unconstrained, ordered where its first
//!   rule appears. It is told from a rule by the absence of a colon, so a group
//!   named `group` still writes its rules as `remap group : a -> b`.
//! - `feature NAME for TARGET... : REMAP_GROUP` — OpenType feature. A target is
//!   a script tag (`latn`, `DFLT`) or a script narrowed to one language system,
//!   `script/LANG` (`latn/ROM`); see `render/ttf_builder/gsub.rs` for why the
//!   two are written explicitly and how scope fallback works.
//! - `feature NAME for TARGET... : anchor ANCHOR_NAME` — the anchor-driven
//!   (mark attachment) variant.
//! - `assert shape TEXT [@lang] [+feat|-feat...] [for SLICE...] : GLYPH [advance N] [offset X Y] : GLYPH ...`
//!   — shaping assertion; `@lang` is a BCP 47 tag, see [`crate::render::assert`].
//!   `for SLICE...` restricts it to faces including all of them; a combination
//!   no face satisfies is an error, not an assertion that quietly never runs.
//! - `assert same NAME...` / `assert distinct NAME...` — resolved-glyph
//!   equality assertions.
//! - `exclude-from-sample NAME`
//! - `assume unused NAME...` — suppresses the unused-glyph warning (patterns
//!   accepted).
//!
//! # Glyph blocks
//!
//! `glyph NAME [W H] [flags...]`, with flags `sticky`, `inline`, `mark`,
//! `advance N`, `left N`, `top N` and `scale N` (the per-glyph sub-pixel detail
//! resolution: the grid is N× finer, and `document_io` multiplies the declared
//! dimensions by it but not the other flags).
//!
//! - With `W H`, pixel rows follow immediately, two characters per pixel (`@@`
//!   filled, `..` empty, plus the sub-pixel shape codes in [`crate::pixel`]).
//! - `ref OTHER [COL ROW] [negated] [inherit] [coloronly|monoonly] [fill COLOR]`
//!   — a composite reference. Omitting the offset auto-resolves it from
//!   `anchor`s; `fill` takes a `#RRGGBB[AA]` literal or a `color` name.
//! - `anchor POS COL ROW` — an anchor for auto-ref alignment; supports `+`/`-`
//!   prefixes and cell ranges.
//! - `glyph NAME = TARGET` — an alias: a second *name* for `TARGET`, sharing
//!   its glyph id rather than declaring a glyph of its own. It takes no flags
//!   and has no body; a glyph that needs either — including one that must
//!   forward its target's anchors — is written in block form with a
//!   `ref TARGET [inherit]` line. See [`crate::alias`].
//! - `glyph NAME [flags...]` with no dimensions — a ref-only composite,
//!   followed by `ref`/`anchor` lines.
//! - NAME accepts the patterns of [`crate::pattern`]; a block expands in
//!   lock-step with its `ref` patterns.
//!
//! A glyph needs a pixel grid or at least one `ref` to exist at all.
//! `advance`/`left`/`top`/`anchor` do not make one buildable, and a contentless
//! glyph never enters the resolution cache — so it is absent from cmap, from
//! composites and from GSUB coverage, and referring to it from a `map`, `ref`
//! or `remap` is an error (leaving it unused is only the usual warning).
//! Pattern glyphs are stricter still and need `ref` lines, since a pixel grid
//! cannot be shared across expansions. For a deliberately blank glyph, use
//! `ref sp`.

use std::fmt;
#[cfg(any(feature = "editor", test))]
use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::document::*;
use crate::pixel::chars_to_shape;
#[cfg(any(feature = "editor", test))]
use crate::pixel::shape_to_chars;

// ---------------------------------------------------------------------------
// Backtick-quoting tokenizer
// ---------------------------------------------------------------------------

/// Tokenize a line into tokens using backtick-quoting rules, dropping any
/// trailing `// …` comment (see [`split_comment`]).
///
/// - Tokens are separated by whitespace.
/// - A token starting with `` ` `` is a quoted token: content runs until the
///   next `` ` ``. Inside the quotes, ` `` ` (two consecutive backticks)
///   represents a literal backtick character; a single `` ` `` ends the quote.
/// - After the closing `` ` ``, the next character must be whitespace or end
///   of input, otherwise an error is returned.
/// - Outside of quotes, backticks are ordinary characters.
pub fn tokenize_tokens(line: &str) -> std::result::Result<Vec<String>, String> {
    Ok(tokenize_with_spans(line)?
        .into_iter()
        .map(|t| t.value)
        .collect())
}

/// Split a line into its command text and its trailing `// …` comment
/// (the returned comment keeps its `//` marker; use [`comment_text`] for the
/// prose alone).
///
/// The comment is a *single* token: it starts at an unquoted token beginning
/// with `//` and runs to the end of the line, and quoting does not apply
/// inside it. Conversely a quoted `` `//` `` is an ordinary token, so
/// ``foo `//` bar // quux`` is four tokens.
///
/// Pixel rows must never be passed through here — `//` is a legal pixel pair.
pub fn split_comment(line: &str) -> (&str, Option<&str>) {
    let mut chars = line.char_indices().peekable();
    let mut at_token_start = true;
    while let Some(&(idx, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            at_token_start = true;
            continue;
        }
        if at_token_start && line[idx..].starts_with("//") {
            return (&line[..idx], Some(&line[idx..]));
        }
        // Not a comment: skip the whole token. A quoted token is skipped by
        // its quoting rules so that a `` `//` `` inside it is not a marker;
        // a malformed quote is left to the tokenizer to report.
        if c == '`' {
            chars.next();
            loop {
                match chars.next() {
                    None => return (line, None),
                    Some((_, '`')) => {
                        if matches!(chars.peek(), Some(&(_, '`'))) {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    Some(_) => {}
                }
            }
        } else {
            while chars.peek().is_some_and(|&(_, c)| !c.is_whitespace()) {
                chars.next();
            }
        }
        // Only whitespace opens a new token, so `` `a`//b `` stays malformed
        // rather than becoming a valid line plus a comment.
        at_token_start = false;
    }
    (line, None)
}

/// The prose of a comment returned by [`split_comment`]: the text after `//`,
/// trimmed. Empty when the line ends right after the marker.
pub fn comment_text(comment: &str) -> &str {
    comment.strip_prefix("//").unwrap_or(comment).trim()
}

/// [`split_comment`] with the comment already reduced to an owned
/// [`comment_text`], and `None` for an empty one — the form document items
/// store.
fn split_comment_owned(line: &str) -> (&str, Option<String>) {
    let (body, comment) = split_comment(line);
    let comment = comment
        .map(comment_text)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    (body, comment)
}

/// Append `extra` to a directive line, keeping any trailing `// …` comment
/// last — a comment is only a comment at the end of its line, so text appended
/// after one would be swallowed by it.
#[cfg(any(feature = "editor", test))]
pub fn append_to_line(line: &str, extra: &str) -> String {
    let (body, comment) = split_comment(line);
    match comment {
        Some(c) => format!("{} {extra} {c}", body.trim_end()),
        None => format!("{} {extra}", body.trim_end()),
    }
}

/// ` // comment`, or the empty string. The serialized form of a comment on a
/// directive line.
#[cfg(any(feature = "editor", test))]
pub fn comment_suffix(comment: &Option<String>) -> String {
    match comment {
        Some(c) => format!(" // {c}"),
        None => String::new(),
    }
}

/// Quote a token for serialization. Wraps in backticks when the value is
/// empty, starts with a backtick, or contains whitespace; internal backticks
/// are doubled.
/// `SLICE[|SLICE...] : ` in front of a directive body, or nothing for the base
/// slice.
#[cfg(any(feature = "editor", test))]
pub fn slice_prefix(slices: &[String]) -> String {
    if slices.is_empty() {
        return String::new();
    }
    format!("{} : ", quote_token(&slices.join("|")))
}

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
#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
pub struct TokenSpan {
    pub value: String,
    pub raw_start: usize,
    pub raw_end: usize,
}

/// Like [`tokenize_tokens`] but also returns character-offset spans for each
/// token in the original line. The trailing comment is not a token here
/// either, so span-consuming callers (links, completion, annotations) never
/// mistake comment prose for a name.
pub fn tokenize_with_spans(line: &str) -> std::result::Result<Vec<TokenSpan>, String> {
    let (line, _) = split_comment(line);
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
            tokens.push(TokenSpan {
                value,
                raw_start,
                raw_end: i,
            });
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
/// - Any of the above followed by `inherit`, `fill COLOR` and/or
///   `coloronly`/`monoonly`, in any order (each is independent of the others)
fn parse_ref_line(parts: &[String], comment: Option<String>) -> Option<GlyphRef> {
    if parts.is_empty() {
        return None;
    }
    let name = parts[0].clone();
    let mut idx = 1;
    let mut offset: Option<(i16, i16)> = None;
    let mut negated = false;
    let mut inherit = false;
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
        offset,
        negated,
        inherit,
        fill,
        visibility,
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

/// Parse an anchor/point from its three token parts: position, col_range, row_range.
fn parse_anchor_point(
    position: &str,
    col_tok: &str,
    row_tok: &str,
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
    parse_glyph_flag_parts_impl(flag_parts, &mut |_| {})
}

const GLYPH_FLAG_KEYWORDS: [&str; 7] = [
    "sticky", "inline", "mark", "advance", "left", "top", "scale",
];

/// The one walker behind both the lenient parse and the strict validation.
/// `err` receives a message for each malformed token; the lenient caller
/// ignores them, the strict caller reports the first one.
fn parse_glyph_flag_parts_impl<S: AsRef<str>>(
    flag_parts: &[S],
    err: &mut impl FnMut(String),
) -> GlyphHeaderFlags {
    let mut flags = GlyphHeaderFlags::default();
    let mut fp = 0;
    while fp < flag_parts.len() {
        match flag_parts[fp].as_ref() {
            "sticky" => flags.sticky = true,
            "inline" => flags.inline = true,
            "mark" => flags.mark = true,
            "advance" => {
                fp += 1;
                flags.advance = flag_parts.get(fp).and_then(|t| t.as_ref().parse().ok());
                if flags.advance.is_none() {
                    err("'advance' requires a numeric value".to_string());
                }
            }
            "scale" => {
                fp += 1;
                flags.scale = flag_parts.get(fp).and_then(|t| t.as_ref().parse().ok());
                if flags.scale.is_none() {
                    err("'scale' requires a numeric value".to_string());
                }
            }
            kw @ ("left" | "top") => {
                fp += 1;
                let value = flag_parts.get(fp).and_then(|t| t.as_ref().parse().ok());
                if value.is_none() {
                    err(format!("'{kw}' requires an i16 value"));
                }
                match kw {
                    "left" => flags.left = value,
                    _ => flags.top = value,
                }
            }
            other => {
                if flags.width.is_none()
                    && let Ok(w) = other.parse::<u16>()
                {
                    flags.width = Some(w);
                    fp += 1;
                    if fp < flag_parts.len() {
                        let next = flag_parts[fp].as_ref();
                        if let Ok(h) = next.parse::<u16>() {
                            flags.height = Some(h);
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

        let tokens =
            tokenize_tokens(trimmed).map_err(|e| anyhow::anyhow!("line {}: {}", line_no + 1, e))?;

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

    // glyph NAME = TARGET — an alias, which is a name and nothing else. The
    // flags used to be accepted here because the form built a real glyph; it
    // no longer does, so a flag on one is a mistake worth naming.
    if let Some(eq_pos) = rest.iter().position(|p| p.as_ref() == "=") {
        if eq_pos != 0 {
            let flags: Vec<&str> = rest[..eq_pos].iter().map(|s| s.as_ref()).collect();
            bail!(
                "line {}: `glyph NAME = TARGET` is an alias for one glyph and takes no flags \
                 (found `{}`); write `glyph NAME {}` with a `ref TARGET` line instead",
                line_no + 1,
                flags.join(" "),
                flags.join(" "),
            );
        }
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

/// Strict form of [`parse_glyph_flag_parts`]: same grammar, same walker,
/// but the first malformed token becomes an error.
fn validate_glyph_flags<S: AsRef<str>>(tokens: &[S], line_no: usize) -> Result<()> {
    let mut first_err: Option<String> = None;
    parse_glyph_flag_parts_impl(tokens, &mut |msg| {
        if first_err.is_none() {
            first_err = Some(msg);
        }
    });
    match first_err {
        Some(msg) => bail!("line {}: {}", line_no + 1, msg),
        None => Ok(()),
    }
}

fn is_pixel_row_next(
    lines: &mut std::iter::Peekable<std::iter::Enumerate<std::str::Lines<'_>>>,
    width: u16,
) -> bool {
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

#[cfg(any(feature = "editor", test))]
pub fn serialize_document(doc: &Document, writer: &mut dyn Write) -> Result<()> {
    for item in &doc.items {
        match item {
            DocumentItem::BlankLine => writeln!(writer)?,
            DocumentItem::Comment(text) => writeln!(writer, "//{text}")?,
            DocumentItem::Meta(text) => writeln!(writer, "meta {text}")?,
            DocumentItem::Directive(text) => writeln!(writer, "{text}")?,
            item @ DocumentItem::Face { .. }
            | item @ DocumentItem::Slice { .. }
            | item @ DocumentItem::NameParts { .. }
            | item @ DocumentItem::Remap { .. }
            | item @ DocumentItem::RemapGroup { .. }
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
            DocumentItem::GlyphAlias {
                name,
                target,
                comment,
            } => {
                writeln!(
                    writer,
                    "glyph {} = {}{}",
                    quote_token(&name.display()),
                    quote_token(target),
                    comment_suffix(comment),
                )?;
            }
            DocumentItem::Map {
                slices,
                char_repr,
                glyph,
                comment,
            } => {
                writeln!(
                    writer,
                    "map {}{} = {}{}",
                    slice_prefix(slices),
                    quote_token(char_repr),
                    quote_token(glyph),
                    comment_suffix(comment),
                )?;
            }
            DocumentItem::MapDecomposed {
                slices,
                char_repr,
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
                    quote_token(char_repr),
                    target,
                    comment_suffix(comment),
                )?;
            }
        }
    }
    Ok(())
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

#[cfg(any(feature = "editor", test))]
fn serialize_glyph(writer: &mut dyn Write, name: &GlyphName, body: &GlyphBody) -> Result<()> {
    let flags = format_glyph_flags(body);
    let qname = quote_token(&name.display());

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
    for r in &body.refs {
        writeln!(writer, "{}", r.format_line(None))?;
    }
    for p in &body.points {
        writeln!(writer, "{}", p.format_line())?;
    }
    Ok(())
}

/// Convert old `= ..` range format to standard `glyph`/`ref` format.
/// `glyph NAME = ..\n\tbody1 ..\n\tbody2` becomes `glyph NAME\nref body1 0 0\nref body2 0 0`.
#[cfg(any(feature = "editor", test))]
pub fn parse_doclines(content: &str) -> Vec<DocLine> {
    let mut lines = Vec::new();
    let mut iter = content.lines().peekable();

    while let Some(line) = iter.next() {
        let trimmed = line.trim();

        let is_glyph = tokenize_tokens(trimmed).ok().and_then(|tokens| {
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
                        && (0..width as usize)
                            .all(|col| chars_to_shape(chars[col * 2], chars[col * 2 + 1]).is_some())
                });
                if !is_pixel {
                    break;
                }
                if let Some(pixel_line) = iter.next() {
                    let chars: Vec<char> = pixel_line.chars().collect();
                    for col in 0..width as usize {
                        let idx = col * 2;
                        if idx + 1 < chars.len()
                            && let Some(shape) = chars_to_shape(chars[idx], chars[idx + 1])
                        {
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

                // Every directive line may end in a `// …` comment; it is one
                // token, it never reaches the grammar below, and it is kept on
                // the item so serializing the document does not drop it.
                let (body_text, comment) = split_comment_owned(trimmed);
                let comment_raw = comment
                    .as_deref()
                    .map(|c| format!(" // {c}"))
                    .unwrap_or_default();
                let tokens = tokenize_tokens(body_text).map_err(DeriveError)?;
                if tokens.is_empty() {
                    item_line_starts.push(i);
                    // A comment-only line never reaches here: it was taken by
                    // the `//` branch above.
                    doc.items.push(DocumentItem::BlankLine);
                    i += 1;
                    continue;
                }

                match tokens[0].as_str() {
                    "meta" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> =
                            tokens[1..].iter().map(|t| quote_token(t)).collect();
                        let text = rest.join(" ");
                        doc.items.push(DocumentItem::Meta(format!(
                            "{}{comment_raw}",
                            text.trim_end(),
                        )));
                        i += 1;
                    }
                    "exclude-from-sample" | "assume" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> =
                            tokens[1..].iter().map(|t| quote_token(t)).collect();
                        let text = format!("{} {}", tokens[0], rest.join(" "));
                        doc.items.push(DocumentItem::Directive(format!(
                            "{}{comment_raw}",
                            text.trim_end()
                        )));
                        i += 1;
                    }
                    "map" => {
                        // An optional `SLICE :` qualifier comes off first, so
                        // the arities below are the same ones the unqualified
                        // form has always had. See
                        // `DocumentItem::split_slice_qualifier` for why the
                        // qualifier cannot be confused with `map : = colon`.
                        let (slices, tokens) = DocumentItem::split_slice_qualifier(&tokens[1..]);
                        // `map generate CHAR [= GLYPH]` is checked first, but only
                        // in the arities the plain form cannot take: `map generate
                        // = g` stays an ordinary (if nonsensical) `map`.
                        let generate = tokens.len() >= 2 && tokens[0] == "generate";
                        if tokens.len() == 3 && tokens[1] == "=" {
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::Map {
                                slices,
                                char_repr: tokens[0].clone(),
                                glyph: tokens[2].clone(),
                                comment,
                            });
                            i += 1;
                        } else if generate
                            && (tokens.len() == 2 || (tokens.len() == 4 && tokens[2] == "="))
                        {
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::MapDecomposed {
                                slices,
                                char_repr: tokens[1].clone(),
                                glyph: tokens.get(3).cloned(),
                                comment,
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

                        // `glyph NAME = TARGET` is an alias: a name for a
                        // glyph, with no body of its own. Flags before the `=`
                        // are rejected by `validate_glyph_header`; the lenient
                        // `DocLine` path drops them the same way.
                        if let Some(eq_pos) = rest_parts.iter().position(|p| p == "=")
                            && let Some(target) = rest_parts.get(eq_pos + 1)
                        {
                            item_line_starts.push(header_idx);
                            doc.items.push(DocumentItem::GlyphAlias {
                                name,
                                target: target.clone(),
                                comment,
                            });
                            continue;
                        }

                        let mut body = GlyphBody::new();
                        body.comment = comment;
                        let flags = parse_glyph_flag_parts(rest_parts);
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

                        if let (Some(w), Some(h)) = (width, height) {
                            if let Some(DocLine::Grid(g)) = lines.get(i)
                                && g.width == w
                                && g.height == h
                            {
                                body.pixels = Some(g.clone());
                                i += 1;
                            } else {
                                body.pixels = Some(PixelGrid::new(w, h));
                            }
                        }

                        // Collect ref and anchor lines
                        while let Some(DocLine::Text(t)) = lines.get(i) {
                            let (sub_text, sub_comment) = split_comment_owned(t.trim());
                            let sub_tokens = match tokenize_tokens(sub_text) {
                                Ok(t) => t,
                                Err(_) => break,
                            };
                            if sub_tokens.first().is_some_and(|t| t == "ref") {
                                let parsed_ref = parse_ref_line(&sub_tokens[1..], sub_comment);
                                let Some(parsed_ref) = parsed_ref else {
                                    break;
                                };
                                body.refs.push(parsed_ref);
                                i += 1;
                                continue;
                            } else if sub_tokens.first().is_some_and(|t| t == "anchor") {
                                let point_parts = &sub_tokens[1..];
                                if point_parts.len() == 3
                                    && let Some(pt) = parse_anchor_point(
                                        &point_parts[0],
                                        &point_parts[1],
                                        &point_parts[2],
                                        sub_comment,
                                    )
                                {
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
                    "name-parts" | "remap" | "feature" | "assert" | "face" | "slice" => {
                        item_line_starts.push(i);
                        doc.items
                            .push(DocumentItem::parse_directive(&tokens, comment));
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
                                comment,
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
#[cfg(any(feature = "editor", test))]
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

#[cfg(test)]
#[path = "document_io_tests.rs"]
mod tests;
