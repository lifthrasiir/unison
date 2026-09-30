//! Backtick-quoting tokenizer, comment/heading/continuation splitting, and the token
//! quoting the serializer writes back (`quote_token`, `slice_prefix`, `comment_suffix`).

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

/// The prose of a comment returned by [`split_comment`]: the text after `//`,
/// trimmed. Empty when the line ends right after the marker.
pub fn comment_text(comment: &str) -> &str {
    comment.strip_prefix("//").unwrap_or(comment).trim()
}

/// [`split_comment`] with the comment already reduced to an owned
/// [`comment_text`], and `None` for an empty one — the form document items
/// store.
pub(super) fn split_comment_owned(line: &str) -> (&str, Option<String>) {
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

/// `SLICE[|SLICE...] : ` in front of a directive body, or nothing for the base
/// slice.
#[cfg(any(feature = "editor", test))]
pub fn slice_prefix(slices: &[String]) -> String {
    if slices.is_empty() {
        return String::new();
    }
    format!("{} : ", quote_token(&slices.join("|")))
}

/// Quote a token for serialization. Wraps in backticks when the value is
/// empty, starts with a backtick or with `//` (which bare would start the
/// line's comment, [`split_comment`]), or contains whitespace; internal
/// backticks are doubled.
pub fn quote_token(s: &str) -> String {
    if !s.is_empty()
        && !s.starts_with('`')
        && !s.starts_with("//")
        && !s.contains(char::is_whitespace)
    {
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
pub(super) fn split_written_uvs_pair(token: &str) -> (String, Option<String>) {
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
pub(super) fn write_map_chars(char_repr: &str, selector: Option<&str>) -> String {
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
