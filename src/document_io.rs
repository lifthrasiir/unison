//! Parser and serializer for the `.unf` font source format.
//!
//! The user-facing reference for the format is `doc/reference.md`; this doc
//! records only what the *parser* and *serializer* commit to, and where each directive's meaning is implemented.
//!
//! Parsing is incremental at the line level: [`crate::document::DocLine`] is
//! what the editor edits, and a pixel-only edit does not reparse the file. The
//! editor canonicalizes every file through [`serialize_document`] when it opens
//! it, so anything the model drops on the way in is something the user loses —
//! comments included (below).
//!
//! # Names
//!
//! A glyph name is letters, digits, `-`, `.`, `_` and `:`. Every character the
//! pattern syntax uses (`(`, `)`, `|`, `$`, `*`, `#`) is excluded, so a pattern
//! that failed to expand cannot reach the font as a name that merely looks odd.
//! The rule is checked against *expanded* names by [`crate::issues`]; see
//! [`crate::document::is_valid_glyph_name`]. Face and slice ids are narrower
//! still — see [`crate::faces`].
//!
//! A `glyph` header and a `ref` target may start with `@`, the last glyph name
//! declared *without* one ([`crate::document::expand_at_name`]). The
//! substitution is textual and happens before anything else reads the name, so
//! the rest of the pipeline sees an ordinary name; the written form is kept
//! beside it (`GlyphBody::raw_name`, `GlyphAlias::raw_name`/`raw_target`,
//! `GlyphRef::raw_name`) so [`serialize_document`] puts back what was written.
//!
//! `$-N` back-references and `exists` captures are substituted by the item's
//! own expansion, not here; see [`crate::pattern`] and [`crate::exists`].
//!
//! # Tokens and comments
//!
//! Whitespace-separated, with backtick quoting: `` `foo bar` ``, two backticks
//! for a literal one inside a quote, so a lone literal backtick is four.
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
//! # Headings
//!
//! `# TEXT`, `## TEXT`, `### TEXT` ([`DocumentItem::Heading`]): the `#` run must
//! be a token of its own, and the text after it is prose taken to the end of the
//! line the way a comment is. A heading is a second kind of comment — no build
//! stage reads one — spelled differently because the editor does read it
//! ([`crate::editor::folding`]). A fourth level is an error from
//! [`crate::issues`] rather than more of the same: the three levels plus the
//! glyph block are the four the editor nests.
//!
//! # Continuation lines
//!
//! `|| TEXT` continues the command above it (only `sample` takes one). It is a
//! line's keyword and not a mid-line escape: only whitespace may precede the
//! marker, everything after it is taken raw — no tokens, no quoting, no comment
//! — and a `||` with nothing above it to continue is an error. The whitespace
//! *every* continuation of one command shares is removed on the way in
//! ([`dedent_continuations`]), so a text whose every line is indented is read,
//! and written back, dedented; [`tokenize_strict`] never sees one, and
//! `serialize.rs`'s `sample_lines` relies on a dedented line starting at column
//! 0 for the round trip.
//!
//! # Directives
//!
//! One keyword per line, and the `SLICE :` qualifier a `map`, `feature` or
//! `name-parts` may carry (or `meta`'s `FACE :` scope) is told from the body by
//! the *second* token being a bare `:`, which no name or value can be — so
//! `map : = colon` still maps U+003A. `assert shape` takes `for SLICE...`
//! instead, since it already uses `:` as a separator, and the two mean opposite
//! things (a line stated once per slice, versus a face including all of them).
//! `editor/line_fields.rs`'s `split_qualifier` takes the qualifier off the same
//! way.
//!
//! Where each directive's meaning lives:
//!
//! - `meta` — [`crate::meta`]; `audit` — [`crate::audit`]; `face`/`slice` —
//!   [`crate::faces`]; `sample` — [`crate::samples`]; `prop` — [`crate::ucd`];
//!   `color` — `render/ttf_builder/color.rs`.
//! - `exists` — [`crate::exists`]; `name-parts` — [`crate::pattern`] and
//!   [`crate::document::SliceNameParts`] (each token of the right-hand side is
//!   itself a pattern, `resolve_name_part_values`).
//! - `map` — `render/ttf_builder/expand.rs` (`resolve_map_alternatives` for the
//!   ordered targets and the empty last one, `expand_uvs_map_triples` for a
//!   variation sequence, and `generate`); `Map::selector` in
//!   [`crate::document`] for the two spellings of a selector and why length
//!   stops at two.
//! - `remap`/`remap group` — `render/ttf_builder/gsub.rs` and
//!   `document/remap.rs`; `feature … : anchor … [align]` —
//!   [`crate::document::AnchorAlign`] and `render/ttf_builder/gpos.rs`.
//! - `assert` — [`crate::render::assert`]; `assume unused` — `issues/unused.rs`.
//!
//! `meta` and `audit` are single-assignment, one key per line, because their
//! keys are variadic; both say why in their own docs.
//!
//! # Glyph blocks
//!
//! `glyph NAME [W H] [flags...]`, with flags `keep`, `inline`, `mark`, `desync`,
//! `vectoronly`, `origin C R`, `advance W`, `extent W H` and `scale N`. `scale`
//! is the per-glyph sub-pixel resolution: the grid is stored N× finer, and this
//! module multiplies the declared dimensions by it but not the other flags.
//! `origin`/`advance`/`extent` state the declared box and meet in
//! [`crate::document::GlyphBody::declared_origin`] and
//! [`declared_extent`](crate::document::GlyphBody::declared_extent), which is
//! all anything downstream reads; `advance` beside `extent` is a parse error
//! (`parse_glyph_flag_parts_impl`). `desync`/`vectoronly` are the build's —
//! [`crate::render::ttf_builder`] — and `keep` on a pattern block is
//! [`crate::merge`]'s opt-out.
//!
//! With `W H`, exactly `H` pixel rows follow, two characters per pixel
//! ([`crate::pixel`]'s catalog; `$$` is [`crate::pixel::PX_HARDBLANK`]). A
//! first row that does not parse reads as "no rows"; a later one of the wrong
//! length or with an unknown pair is an error at that line.
//!
//! The lines under a header: `ref` ([`crate::ref_composite`]; `goto` is the one
//! flag no build stage reads, see `GlyphRef::goto`), `anchor`
//! ([`crate::ref_composite`] and `gpos.rs`), and the IDC lines
//! ([`crate::compose`] — each token a gap if it reads as a number, else a
//! component name, except on an enclosure where the two numbers are offsets;
//! `assume` in front of the operator is part of the line, not a directive).
//! `glyph NAME = TARGET` is an alias ([`crate::alias`]), takes no flags and no
//! body; `glyph NAME* = PREFIX*` is a multi-alias, read as the alias an
//! `exists PREFIX(.*)` would scope. NAME accepts the patterns of [`crate::pattern`], and a block expands
//! in lock-step with its `ref` and IDC patterns.
//!
//! A glyph needs a pixel grid, at least one `ref` or an IDC line to exist at
//! all. `origin`/`advance`/`extent`/`anchor` do not make one buildable, and a
//! contentless glyph never enters the resolution cache — so it is absent from
//! cmap, composites and GSUB coverage, and referring to it is an error; `keep`
//! is the one way to write a bodiless glyph (an empty outline carrying its
//! anchors). For a deliberately blank glyph, use `ref sp` or a grid with no
//! rows.

use std::fmt;
#[cfg(any(feature = "editor", test))]
use std::io::Write;
use std::path::Path;

use anyhow::Result;

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

/// A heading line split into its level (how many `#` were written) and the
/// text after them, or `None` for a line that is not a heading at all.
///
/// `line` must already be trimmed. A heading is a leading run of `#` that is a
/// *token* of its own: the run has to be followed by whitespace or end of line,
/// so `#name` — and a `$#…` pattern, which never starts a line — is untouched.
/// The run is not capped at three here; `####` parses as level 4 so that
/// [`crate::issues`] can name it, rather than being read as something else.
pub fn split_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if hashes == 0 {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some((hashes.min(u8::MAX as usize) as u8, rest.trim()))
}

/// The prose of a comment returned by [`split_comment`]: the text after `//`,
/// trimmed. Empty when the line ends right after the marker.
/// The marker a continuation line starts with. See
/// [`dedent_continuations`] and `# Continuation lines` in this module's docs.
pub const CONTINUATION: &str = "||";

/// The text a continuation line carries, or `None` if `line` is not one.
///
/// Only leading whitespace may come in front of the marker — a `||` is a line's
/// *keyword*, not something that may turn up in the middle of one the way `//`
/// can. Everything after the marker is taken raw: a continuation carries prose,
/// so it has neither tokens nor a `// …` comment of its own, and a backtick in
/// it is a backtick.
pub fn continuation_text(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(CONTINUATION)
}

/// Strip the whitespace every continuation of one command shares.
///
/// A continuation is written `|| text`, so the space after the marker is
/// punctuation rather than content — but only the part of it that *every* line
/// has, which is what lets a text indent one of its own lines relative to the
/// rest. A whitespace-only line is not a line of the text as far as the shared
/// prefix goes (it would otherwise cap the prefix at nothing whenever the text
/// has a paragraph break) and becomes empty.
///
/// Nothing here can put a common indent *back*: a text whose every line is
/// indented is written, and read, dedented. That is the whole of what the rule
/// costs, and it is what makes the model round-trip — see `serialize_sample`.
pub fn dedent_continuations(raw: &[String]) -> Vec<String> {
    let mut prefix: Option<&str> = None;
    for line in raw {
        if line.trim().is_empty() {
            continue;
        }
        let ws = &line[..line.len() - line.trim_start().len()];
        prefix = Some(match prefix {
            None => ws,
            Some(prev) => {
                let shared = prev
                    .char_indices()
                    .zip(ws.chars())
                    .take_while(|((_, a), b)| a == b)
                    .map(|((i, a), _)| i + a.len_utf8())
                    .last()
                    .unwrap_or(0);
                &prev[..shared]
            }
        });
    }
    let prefix = prefix.unwrap_or("");
    raw.iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                line[prefix.len()..].to_string()
            }
        })
        .collect()
}

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
// Not editor-gated: `GlyphCompose::format_line` is what `uniform fix` writes an
// IDC line back with, and that is a headless command.
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

/// Split a single written `map` token into a base and a variation selector.
///
/// A variation sequence written literally — what pasting `0️` from a character
/// picker gives you — is *one* token holding two characters, while the `U+XXXX
/// U+YYYY` spelling is two. Only the exact shape "two characters, the second a
/// selector and the first not" splits; everything else stays whole, so a pipe
/// list keeps its last alternative and a longer paste (`0️⃣`) survives intact
/// for [`crate::issues`] to reject by name instead of being truncated here.
fn split_written_uvs_pair(token: &str) -> (String, Option<String>) {
    let mut chars = token.chars();
    if let (Some(base), Some(sel), None) = (chars.next(), chars.next(), chars.next())
        && !crate::ucd::is_variation_selector(base as u32)
        && crate::ucd::is_variation_selector(sel as u32)
    {
        return (base.to_string(), Some(sel.to_string()));
    }
    (token.to_string(), None)
}

/// Write the character half of a `map` back out in the form it was written in.
///
/// The two spellings of one variation sequence are different text and each has
/// to round-trip, so something has to tell `U+0030 U+FE0F` (two tokens) from
/// `0️` (one). Concatenating is safe only when *both* halves are literal: with a
/// `U+XXXX` base and a literal selector it would glue them into one
/// seven-character token, which re-parses as a single unreadable character
/// rather than as the pair that was written. Two tokens are always safe, since
/// only a two-character token is ever split.
#[cfg(any(feature = "editor", test))]
fn write_map_chars(char_repr: &str, selector: Option<&str>) -> String {
    let is_hex = |s: &str| s.starts_with("U+") || s.starts_with("u+");
    match selector {
        Some(sel) if is_hex(sel) || is_hex(char_repr) => {
            format!("{} {}", quote_token(char_repr), quote_token(sel))
        }
        Some(sel) => quote_token(&format!("{char_repr}{sel}")),
        None => quote_token(char_repr),
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
/// - Any of the above followed by `inherit`, `goto`, `fill COLOR` and/or
///   `coloronly`/`monoonly`, in any order (each is independent of the others)
///
/// `base` is the `@` base in force — the last glyph name declared without one.
fn parse_ref_line(
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
fn parse_compose_line(
    op: crate::compose::IdcOp,
    assumed: bool,
    parts: &[String],
    comment: Option<String>,
    base: Option<&str>,
) -> GlyphCompose {
    let items = parts
        .iter()
        .map(|token| match token.parse::<i16>() {
            Ok(gap) => ComposeItem::Gap(gap),
            Err(_) => {
                let name = crate::document::expand_at_name(token, base);
                let raw_name = crate::document::written_form(token, &name);
                ComposeItem::Part { name, raw_name }
            }
        })
        .collect();
    GlyphCompose {
        op,
        items,
        assumed,
        comment,
    }
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
pub const GLYPH_FLAG_KEYWORDS: [&str; 9] = [
    "keep",
    "inline",
    "mark",
    "desync",
    "vectoronly",
    "advance",
    "origin",
    "extent",
    "scale",
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

/// Parse the whitespace-split tokens of a `glyph ...` header (with the glyph
/// name at index 0) to determine whether pixel rows follow, and if so their
/// dimensions.
///
/// Returns `None` for ref-only headers (`glyph NAME`) or simple aliases
/// (`glyph NAME = ALIAS`). Handles keyword flags like `keep`, `advance N`,
/// `origin C R` appearing before or after `W H`.
/// `LABEL [SUBLABEL] [: MODE...]` — the tokens of a `sample` line after the
/// keyword, or `None` if they do not read as one.
///
/// The mode tail is told from the labels by a bare `:` token, the same rule a
/// slice qualifier is told by; a label that is literally `:` is written
/// `` `:` `` like any other token holding punctuation. Nothing validates `MODE`
/// here — the words are kept as written, [`crate::samples::SampleMode`] reads
/// them and [`crate::issues`] is where one that names nothing is reported.
fn parse_sample_header<S: AsRef<str>>(
    parts: &[S],
) -> Option<(String, Option<String>, Vec<String>)> {
    let colon = parts.iter().position(|p| p.as_ref() == ":");
    let (labels, mode) = match colon {
        Some(at) => (
            &parts[..at],
            parts[at + 1..]
                .iter()
                .map(|p| p.as_ref().to_string())
                .collect(),
        ),
        None => (parts, Vec::new()),
    };
    match labels {
        [label] => Some((label.as_ref().to_string(), None, mode)),
        [label, sublabel] => Some((
            label.as_ref().to_string(),
            Some(sublabel.as_ref().to_string()),
            mode,
        )),
        _ => None,
    }
}

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
        let at = spans
            .iter()
            .position(|s| s.value == keyword)
            .filter(|&i| i >= 2);
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
fn tokenize_strict(content: &str) -> std::result::Result<Vec<DocLine>, ParseError> {
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

/// Strict form of [`parse_glyph_flag_parts`]: same grammar, same walker,
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

fn is_pixel_row_next(
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
/// [`is_pixel_row_next`] applies while parsing a file. Both the promotion a
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

/// Lenient counterpart of [`tokenize_strict`], for text the editor is in the
/// middle of typing: a malformed header or pixel row becomes an ordinary
/// `Text` line instead of an error, and a short grid is padded rather than
/// rejected. The strict path stays the one behind [`parse_document_from_str`].
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
    /// [`parse_doclines`] gives the same line, which is what the editor opens.
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

/// The keywords that begin a top-level item, as [`derive_document`] dispatches
/// on them.
///
/// A line starting with one of these ends whatever block came before it; a line
/// starting with anything else — `ref`, `anchor`, an IDC operator, a pixel row —
/// belongs to the glyph block above. `assume` is the one keyword that is both:
/// in front of an IDC operator it is that line's
/// ([`IdcOp::of_line`](crate::compose::IdcOp::of_line)), so a caller holding
/// more than the first token asks that first. The editor's text-only passes (search,
/// navigation) need that boundary without parsing the file, so it is stated
/// here, beside the dispatch it has to agree with, rather than re-listed there.
pub fn starts_item(token: &str) -> bool {
    matches!(
        token,
        "meta"
            | "audit"
            | "assume"
            | "map"
            | "glyph"
            | "name-parts"
            | "remap"
            | "feature"
            | "assert"
            | "face"
            | "slice"
            | "prop"
            | "exists"
            | "color"
            | "sample"
    )
}

pub fn derive_document(
    lines: &[DocLine],
    path: std::path::PathBuf,
) -> std::result::Result<(Document, Vec<usize>), DeriveError> {
    let mut doc = Document::new(path);
    let mut item_line_starts: Vec<usize> = Vec::new();
    let mut i = 0;
    // The `@` base: the last glyph name declared without one. Scoped to the
    // file, and carried across the lines between two glyph blocks, so a helper
    // glyph keeps expanding against its base however far below it is written.
    let mut at_base: Option<String> = None;

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

                // A continuation reaching here is one nothing claimed: every
                // command that takes them swallows its own below. Kept as an
                // opaque directive so the line survives a round trip and
                // `issues` can name it — see `Directive::OrphanContinuation`.
                if continuation_text(trimmed).is_some() {
                    item_line_starts.push(i);
                    doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                    i += 1;
                    continue;
                }

                // A heading is prose after its `#` run, so it is taken whole
                // like a comment rather than tokenized: a backtick in a section
                // title is a backtick, not an unterminated quote.
                if let Some((level, text)) = split_heading(trimmed) {
                    item_line_starts.push(i);
                    doc.items.push(DocumentItem::Heading {
                        level,
                        text: text.to_string(),
                    });
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
                // A line the tokenizer cannot read at all — in practice an
                // unterminated `` ` `` quote, usually one the author is halfway
                // through typing — is kept as one opaque text item rather than
                // failing the whole derive. The editor builds its visual lines
                // from this item list against the same buffer, so a derive that
                // bails leaves the *previous* structure in place and every grid
                // after the bad line is drawn on the wrong line. `issues.rs`
                // reports it as an unrecognized directive; the strict CLI path
                // (`tokenize_strict`) still rejects the file outright.
                let Ok(tokens) = tokenize_tokens(body_text) else {
                    item_line_starts.push(i);
                    doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                    i += 1;
                    continue;
                };
                if tokens.is_empty() {
                    item_line_starts.push(i);
                    // A comment-only line never reaches here: it was taken by
                    // the `//` branch above.
                    doc.items.push(DocumentItem::BlankLine);
                    i += 1;
                    continue;
                }

                match tokens[0].as_str() {
                    "meta" | "audit" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> =
                            tokens[1..].iter().map(|t| quote_token(t)).collect();
                        let text = rest.join(" ");
                        let text = format!("{}{comment_raw}", text.trim_end());
                        doc.items.push(if tokens[0] == "meta" {
                            DocumentItem::Meta(text)
                        } else {
                            DocumentItem::Audit(text)
                        });
                        i += 1;
                    }
                    "assume" => {
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
                        // Everything past the `=` is a target, and there may be
                        // several: `map A = first second` tries `first` and
                        // falls back to `second`. See `DocumentItem::Map`.
                        if tokens.len() >= 3 && tokens[1] == "=" {
                            let (char_repr, selector) = split_written_uvs_pair(&tokens[0]);
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::Map {
                                slices,
                                char_repr,
                                selector,
                                glyphs: tokens[2..].to_vec(),
                                comment,
                            });
                            i += 1;
                        } else if generate
                            && (tokens.len() == 2 || (tokens.len() == 4 && tokens[2] == "="))
                        {
                            let (char_repr, selector) = split_written_uvs_pair(&tokens[1]);
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::MapDecomposed {
                                slices,
                                char_repr,
                                selector,
                                glyph: tokens.get(3).cloned(),
                                comment,
                            });
                            i += 1;
                        } else if generate
                            && (tokens.len() == 3 || (tokens.len() == 5 && tokens[3] == "="))
                        {
                            // `map generate BASE SELECTOR [= GLYPH]` parses so
                            // that it can be *rejected* by name. `generate`
                            // wins this arity over the plain pair form below,
                            // which is what keeps `map generate Á = a-acute`
                            // decomposed rather than read as a sequence.
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::MapDecomposed {
                                slices,
                                char_repr: tokens[1].clone(),
                                selector: Some(tokens[2].clone()),
                                glyph: tokens.get(4).cloned(),
                                comment,
                            });
                            i += 1;
                        } else if tokens.len() >= 4 && tokens[2] == "=" && !generate {
                            // `!generate`, so the one arity the two forms share
                            // (`map generate B = g`) stays decomposed above
                            // rather than becoming a variation sequence here.
                            item_line_starts.push(i);
                            doc.items.push(DocumentItem::Map {
                                slices,
                                char_repr: tokens[0].clone(),
                                selector: Some(tokens[1].clone()),
                                glyphs: tokens[3..].to_vec(),
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
                        // A bare `glyph`, which is what a header being typed
                        // from scratch looks like for a keystroke or two. Same
                        // reasoning as the unreadable line above: one opaque
                        // item, not a failed derive.
                        if parts.is_empty() {
                            item_line_starts.push(header_idx);
                            doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                            continue;
                        }

                        // The header's own `@` expands against the base that
                        // was already in force, and only a header written
                        // *without* one becomes the next base — which is what
                        // makes `glyph @-bar` / `ref @-baz` name `foo-baz`
                        // rather than `foo-bar-baz`.
                        let rest_parts = &parts[1..];
                        let alias_target = rest_parts
                            .iter()
                            .position(|p| p == "=")
                            .and_then(|eq_pos| rest_parts.get(eq_pos + 1));
                        // `glyph NAME* = PREFIX*` is read as the alias its
                        // `exists` form scopes, `glyph NAME($1) = ($0)`, and
                        // every name-reading step below sees that. One the
                        // strict parse rejects stays an ordinary alias here,
                        // whose `*` names no glyph.
                        let multi = alias_target.and_then(|target| {
                            crate::alias::multi_alias_prefixes(&parts[0], target)
                                .ok()
                                .flatten()
                        });

                        let written = match multi {
                            Some((name_prefix, _)) => format!("{name_prefix}($1)"),
                            None => parts[0].clone(),
                        };
                        let expanded =
                            crate::document::expand_at_name(&written, at_base.as_deref());
                        let raw_name = match multi {
                            Some(_) => Some(parts[0].clone()),
                            None => crate::document::written_form(&written, &expanded),
                        };
                        if let Some(base) = crate::document::at_base_from_glyph_name(&written) {
                            at_base = Some(base);
                        }
                        let name = parse_glyph_name(&expanded);

                        // `glyph NAME = TARGET` is an alias: a name for a
                        // glyph, with no body of its own. Flags before the `=`
                        // are rejected by `validate_glyph_header`; the lenient
                        // `DocLine` path drops them the same way.
                        if let Some(target) = alias_target {
                            item_line_starts.push(header_idx);
                            let (expanded_target, raw_target, search_prefix) = match multi {
                                Some((_, prefix)) => (
                                    "($0)".to_string(),
                                    Some(target.clone()),
                                    Some(prefix.to_string()),
                                ),
                                None => {
                                    let expanded_target =
                                        crate::document::expand_at_name(target, at_base.as_deref());
                                    let raw_target =
                                        crate::document::written_form(target, &expanded_target);
                                    (expanded_target, raw_target, None)
                                }
                            };
                            doc.items.push(DocumentItem::GlyphAlias {
                                name,
                                target: expanded_target,
                                raw_name,
                                raw_target,
                                search_prefix,
                                comment,
                            });
                            continue;
                        }

                        let mut body = GlyphBody::new();
                        body.comment = comment;
                        body.raw_name = raw_name;
                        let flags = parse_glyph_flag_parts(rest_parts);
                        body.keep = flags.keep;
                        body.inline = flags.inline;
                        body.mark = flags.mark;
                        body.desync = flags.desync;
                        body.vectoronly = flags.vectoronly;
                        body.advance = flags.advance;
                        body.origin = flags.origin;
                        body.extent = flags.extent;
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
                                body.pixels = Some(PixelGrid::clone(g));
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
                                let parsed_ref = parse_ref_line(
                                    &sub_tokens[1..],
                                    sub_comment,
                                    at_base.as_deref(),
                                );
                                let Some(parsed_ref) = parsed_ref else {
                                    break;
                                };
                                body.refs.push(parsed_ref);
                                i += 1;
                                continue;
                            } else if let Some((op, assumed)) = crate::compose::IdcOp::of_line(
                                sub_tokens.iter().map(String::as_str),
                            ) {
                                body.compose.push(parse_compose_line(
                                    op,
                                    assumed,
                                    &sub_tokens[1 + usize::from(assumed)..],
                                    sub_comment,
                                    at_base.as_deref(),
                                ));
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
                    "name-parts" | "remap" | "feature" | "assert" | "face" | "slice" | "prop" => {
                        item_line_starts.push(i);
                        doc.items
                            .push(DocumentItem::parse_directive(&tokens, comment));
                        i += 1;
                    }
                    "exists" => {
                        item_line_starts.push(i);
                        // Exactly one token: the pattern is a regex, and a
                        // second token would either be a second pattern (there
                        // is no conjunction) or a flag (there are none). Both
                        // are better said by `issues` than guessed at here.
                        if tokens.len() == 2 {
                            doc.items.push(DocumentItem::Exists {
                                pattern: tokens[1].clone(),
                                comment,
                            });
                        } else {
                            doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                        }
                        i += 1;
                    }
                    "sample" => {
                        item_line_starts.push(i);
                        // A header the grammar cannot read keeps its own line
                        // as an opaque directive and claims nothing: its
                        // continuations fall through to the orphan branch on
                        // the next pass, so both halves of the mistake are
                        // reported instead of one being folded into the other.
                        let Some((label, sublabel, mode)) = parse_sample_header(&tokens[1..])
                        else {
                            doc.items.push(DocumentItem::Directive(trimmed.to_string()));
                            i += 1;
                            continue;
                        };
                        i += 1;
                        // The continuations are the command's, not the
                        // document's: they are read here so that no later pass
                        // has to know a `sample` spans more than one line.
                        let mut raw: Vec<String> = Vec::new();
                        while let Some(DocLine::Text(t)) = lines.get(i) {
                            let Some(rest) = continuation_text(t) else {
                                break;
                            };
                            raw.push(rest.to_string());
                            i += 1;
                        }
                        doc.items.push(DocumentItem::Sample {
                            label,
                            sublabel,
                            mode,
                            text: dedent_continuations(&raw),
                            comment,
                        });
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
    #[cfg(feature = "editor")]
    {
        doc.line_ids = lines.iter().map(DocLine::line_id).collect();
    }
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

#[cfg(test)]
#[path = "document_io_tests/mod.rs"]
mod tests;
