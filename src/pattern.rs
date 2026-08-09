//! Name pattern parsing and expansion.
//!
//! A *name pattern* is a compact notation for a list of glyph names:
//!
//! - `(a|b|c)` — an alternation group spliced into the surrounding literal
//!   text (`foo-(a|b)` → `foo-a`, `foo-b`);
//! - `a*N` — inside a group, repeats one alternative N times;
//! - `(...**N)` — at the end of a group only, repeats each of its
//!   alternatives N times; a `**N` anywhere else is a syntax error;
//! - `$var` / `$0..9` / `$#a0..af` — name-part references and inline numeric
//!   ranges, substituted textually by [`substitute_name_parts`] *before*
//!   pattern parsing.  A reference is just one alternative among the others,
//!   so `(foo|$bar|baz*5|$#00..ff*3**2)` mixes all the forms freely; a `*N`
//!   on a reference repeats each of its values.
//!
//! Multiple groups in one pattern combine cyclically: the total length is the
//! LCM of the group sizes and group `k` contributes its `i % len(k)`-th
//! alternative to the `i`-th name.  This cyclic indexing is what lets remap
//! operands and `ref` targets expand in lock-step with a glyph-name pattern.
//!
//! A slice qualifier listing several slices (`map wide|narrow : ...`) is *not*
//! part of this.  The slices are an outer loop — the line is stated once per
//! slice, with that slice's [name parts](crate::document::SliceNameParts) in
//! force, and each statement then expands on its own.  Folding them in as one
//! more group would zip them against the codepoint list instead: two slices
//! against ten codepoints would produce ten names alternating between the two,
//! which is not what the line says.
//!
//! [`NamePattern`] is the parsed form.  It knows its [`len`](NamePattern::len)
//! without materializing anything and yields individual names via
//! [`get`](NamePattern::get), so consumers that combine several patterns
//! (the GSUB builder, `expand_glyph_block`) can defer or skip full expansion;
//! set operations such as a distinct-count could be added on it later without
//! touching call sites.  Iterating (or [`into_vec`](NamePattern::into_vec))
//! materializes names on demand.
//!
//! The same surface syntax is read in two contexts with different top-level
//! rules, matching how the `.unf` grammar evolved:
//!
//! - [`NamePattern::parse_element`] — a *single name element* (a `map` or
//!   `remap` operand, an `assume unused` argument, a ref target looked up at
//!   runtime).  A top-level `|` or `*` outside parentheses treats the whole
//!   string as one alternation group, so a `$var` substitution that produced
//!   `v1|v2**2` behaves like `(v1|v2**2)`.
//! - [`NamePattern::parse`] — a *glyph block name* (`glyph NAME ...`).  This
//!   additionally accepts a top-level `name1|name2` list whose branches are
//!   taken verbatim (trimmed, not recursively expanded), and a bare `foo*N`
//!   repeat.
//! - [`NamePattern::parse_segments`] — a `ref` target inside a pattern glyph
//!   block: groups only, no range or top-level list.
//!
//! The difference is visible on one string: `a*2|b` is `a`, `a`, `b` to
//! [`parse_element`](NamePattern::parse_element) and the two verbatim names
//! `a*2`, `b` to [`parse`](NamePattern::parse). Both readings are relied on, so
//! the tests below pin each one.
//!
//! A `name-parts` right-hand side is expanded the same way, one token at a
//! time, so a value may be written as a pattern
//! (`crate::document::resolve_name_part_values`).
//!
//! Expansion is capped at [`MAX_EXPANSION`] names — for a `name-parts` binding
//! too, which is an error when its own values go over it.
//!
//! (This is the single expansion engine. It was consolidated out of two separate
//! ones that had grown in `document.rs`, which still re-exports the API for the
//! import paths predating the split.)

use std::collections::HashMap;
use std::fmt;

pub const MAX_EXPANSION: usize = 1 << 16;

#[derive(Clone, Debug)]
pub enum NamePatternError {
    TooManyExpansions(usize),
    Syntax(String),
}

impl fmt::Display for NamePatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NamePatternError::TooManyExpansions(n) => {
                write!(f, "name pattern expands to {n} names (max {MAX_EXPANSION})")
            }
            NamePatternError::Syntax(msg) => write!(f, "name pattern syntax error: {msg}"),
        }
    }
}

impl std::error::Error for NamePatternError {}

#[derive(Clone, Debug)]
enum Segment {
    Literal(String),
    Alts(Vec<String>),
}

#[derive(Clone, Debug)]
enum Kind {
    /// A name with no pattern syntax, kept verbatim.
    Single(String),
    /// Top-level `a|b|c` list (block names only): branches taken verbatim.
    List(Vec<String>),
    /// Literal text interleaved with alternation groups.
    Segments(Vec<Segment>),
}

/// A parsed name pattern; a plain name parses as a single-name pattern.
///
/// `len()` is always at least 1 and never requires materializing the names.
/// `get(i)` indexes cyclically (`i` is taken modulo the relevant group
/// sizes), which is the expansion rule every consumer shares.
#[derive(Clone, Debug)]
pub struct NamePattern {
    kind: Kind,
    len: usize,
}

impl NamePattern {
    fn single(name: String) -> Self {
        NamePattern {
            kind: Kind::Single(name),
            len: 1,
        }
    }

    /// Parses a single name element (`map`/`remap` operands, `assume unused`
    /// arguments, runtime ref lookups).  A top-level `|` or `*` without any
    /// parentheses wraps the whole string into one alternation group.
    pub fn parse_element(s: &str) -> Result<Self, NamePatternError> {
        if s.chars().count() <= 1 || (!s.contains('(') && !s.contains('|') && !s.contains('*')) {
            return Ok(Self::single(s.to_string()));
        }
        if s.contains('(') {
            Self::from_segments(s)
        } else {
            Self::from_segments(&format!("({s})"))
        }
    }

    /// Parses a glyph block name: a top-level verbatim `name1|name2` list,
    /// alternation groups, or a bare `foo*N` repeat.
    pub fn parse(s: &str) -> Result<Self, NamePatternError> {
        if has_top_level_pipe(s) {
            let names: Vec<String> = split_top_level_pipes(s)
                .into_iter()
                .map(|p| p.trim().to_string())
                .collect();
            let len = names.len();
            if len > MAX_EXPANSION {
                return Err(NamePatternError::TooManyExpansions(len));
            }
            return Ok(NamePattern {
                kind: Kind::List(names),
                len,
            });
        }

        Self::parse_segments(s)
    }

    /// Parses alternation groups and bare `foo*N` repeats only (`ref` targets
    /// inside a pattern glyph block).
    pub fn parse_segments(s: &str) -> Result<Self, NamePatternError> {
        // Bare `foo*N` (no parentheses) → treat as `(foo*N)`.
        if !s.contains('(') && has_bare_repeat(s) {
            let alts = parse_alt_content(s)?;
            let len = alts.len();
            return Ok(NamePattern {
                kind: Kind::Segments(vec![Segment::Alts(alts)]),
                len,
            });
        }
        Self::from_segments(s)
    }

    fn from_segments(s: &str) -> Result<Self, NamePatternError> {
        let mut segments = Vec::new();
        let bytes = s.as_bytes();
        let mut pos = 0;
        let mut lit_start = 0;
        let mut len = 1usize;

        while pos < bytes.len() {
            if bytes[pos] == b'(' {
                if pos > lit_start {
                    segments.push(Segment::Literal(s[lit_start..pos].to_string()));
                }
                let open = pos;
                let mut depth = 1;
                pos += 1;
                while pos < bytes.len() && depth > 0 {
                    if bytes[pos] == b'(' {
                        depth += 1;
                    }
                    if bytes[pos] == b')' {
                        depth -= 1;
                    }
                    pos += 1;
                }
                if depth != 0 {
                    return Err(NamePatternError::Syntax(format!("unmatched '(' in: {s}")));
                }
                let alts = parse_alt_content(&s[open + 1..pos - 1])?;
                len = checked_lcm(len, alts.len())?;
                segments.push(Segment::Alts(alts));
                lit_start = pos;
            } else {
                pos += 1;
            }
        }
        if lit_start < s.len() {
            segments.push(Segment::Literal(s[lit_start..].to_string()));
        }

        Ok(NamePattern {
            kind: Kind::Segments(segments),
            len,
        })
    }

    /// The number of names this pattern denotes.  Always at least 1.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The `i`-th name.  Indexes cyclically: each alternation group picks its
    /// `i % group_len`-th alternative, so any `i` is valid and `get(i)` equals
    /// `get(i % len())`.
    pub fn get(&self, i: usize) -> String {
        match &self.kind {
            Kind::Single(name) => name.clone(),
            Kind::List(names) => names[i % names.len()].clone(),
            Kind::Segments(segments) => {
                let mut out = String::new();
                for seg in segments {
                    match seg {
                        Segment::Literal(s) => out.push_str(s),
                        Segment::Alts(alts) => out.push_str(&alts[i % alts.len()]),
                    }
                }
                out
            }
        }
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            pattern: self,
            i: 0,
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        self.iter().collect()
    }
}

pub struct Iter<'a> {
    pattern: &'a NamePattern,
    i: usize,
}

impl Iterator for Iter<'_> {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        if self.i >= self.pattern.len {
            return None;
        }
        let name = self.pattern.get(self.i);
        self.i += 1;
        Some(name)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.pattern.len - self.i;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Iter<'_> {}

impl<'a> IntoIterator for &'a NamePattern {
    type Item = String;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

pub struct IntoIter {
    pattern: NamePattern,
    i: usize,
}

impl Iterator for IntoIter {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        if self.i >= self.pattern.len {
            return None;
        }
        let name = self.pattern.get(self.i);
        self.i += 1;
        Some(name)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.pattern.len - self.i;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for IntoIter {}

impl IntoIterator for NamePattern {
    type Item = String;
    type IntoIter = IntoIter;
    fn into_iter(self) -> IntoIter {
        IntoIter {
            pattern: self,
            i: 0,
        }
    }
}

/// Combined expansion length of several patterns indexed in lock-step:
/// the LCM of the individual lengths.
pub fn combined_len<'a>(patterns: impl IntoIterator<Item = &'a NamePattern>) -> usize {
    patterns.into_iter().fold(1, |acc, p| lcm(acc, p.len()))
}

/// Splits `(...)` group content on `|`, applying `a*N` per-alternative
/// repeats and a trailing `**N` whole-group multiplier.
fn parse_alt_content(content: &str) -> Result<Vec<String>, NamePatternError> {
    let (content, group_mult) = extract_group_mult(content)?;

    let mut alts = Vec::new();
    for part in content.split('|') {
        // `**N` is the whole-group multiplier and only exists at the very end
        // of the group, where `extract_group_mult` has already taken it off.
        // Anywhere else it is a typo for the per-alternative `*N`.
        if part.contains("**") {
            return Err(NamePatternError::Syntax(format!(
                "'**' multiplier is only allowed at the end of a group (use '*N' to repeat one \
                 alternative): {part}"
            )));
        }
        if let Some((name, rep_str)) = part.rsplit_once('*') {
            let rep: usize = rep_str.parse().map_err(|_| {
                NamePatternError::Syntax(format!("invalid repeat count: {rep_str}"))
            })?;
            let expanded_count = alts
                .len()
                .checked_add(rep)
                .ok_or(NamePatternError::TooManyExpansions(usize::MAX))?;
            if expanded_count > MAX_EXPANSION {
                return Err(NamePatternError::TooManyExpansions(expanded_count));
            }
            for _ in 0..rep {
                alts.push(name.to_string());
            }
        } else {
            alts.push(part.to_string());
        }
    }
    if alts.is_empty() {
        return Err(NamePatternError::Syntax("empty alternation group".into()));
    }

    if group_mult > 1 {
        let base = alts;
        let total = base
            .len()
            .checked_mul(group_mult)
            .ok_or(NamePatternError::TooManyExpansions(usize::MAX))?;
        if total > MAX_EXPANSION {
            return Err(NamePatternError::TooManyExpansions(total));
        }
        alts = Vec::with_capacity(total);
        for name in &base {
            for _ in 0..group_mult {
                alts.push(name.clone());
            }
        }
    }

    Ok(alts)
}

/// Returns true if `s` is a non-empty run of ASCII digits (a repeat count).
fn is_count(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Recognizes a `**N` multiplier at the end of a group's last alternative and
/// splits it off.  Returns `(content_without_multiplier, multiplier)`.
fn extract_group_mult(content: &str) -> Result<(&str, usize), NamePatternError> {
    let last_pipe = content.rfind('|').map_or(0, |p| p + 1);
    let last_alt = &content[last_pipe..];
    if let Some(pos) = last_alt.rfind("**") {
        let after = &last_alt[pos + 2..];
        if is_count(after) {
            let k: usize = after.parse().map_err(|_| {
                NamePatternError::Syntax(format!("invalid group multiplier: {after}"))
            })?;
            return Ok((&content[..last_pipe + pos], k));
        }
    }
    Ok((content, 1))
}

/// Returns true if `s` ends in a numeric `*N` repeat (`foo*3`).
pub fn has_bare_repeat(s: &str) -> bool {
    if let Some((_, count_str)) = s.rsplit_once('*') {
        !count_str.is_empty() && count_str.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    }
}

/// Returns true if the name string looks like a multi-glyph pattern
/// (contains alternation `|`, grouping `(`, or range `..`).
/// Single-character names are never patterns even if the character is `(` or `|`.
pub fn is_name_pattern(s: &str) -> bool {
    s.chars().count() > 1
        && (s.contains('(') || s.contains('|') || s.contains("..") || has_bare_repeat(s))
}

/// Returns true if `s` contains a `|` outside any parentheses.
pub fn has_top_level_pipe(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Splits on `|` at paren depth 0 only.
pub fn split_top_level_pipes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Shared integer helpers; callers with narrower types cast through `usize`.
pub(crate) fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

pub(crate) fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b) * b
}

fn checked_lcm(a: usize, b: usize) -> Result<usize, NamePatternError> {
    let l = (a / gcd(a, b))
        .checked_mul(b)
        .ok_or(NamePatternError::TooManyExpansions(usize::MAX))?;
    if l > MAX_EXPANSION {
        return Err(NamePatternError::TooManyExpansions(l));
    }
    Ok(l)
}

// ---------------------------------------------------------------------------
// Name-part ($var) substitution
// ---------------------------------------------------------------------------

pub type NamePartsMap = HashMap<String, Vec<String>>;

/// Every glyph name a single written name can denote: name-part references
/// substituted, then the resulting pattern expanded.
///
/// This is the one operation `remap` operands, `assume unused` arguments and
/// `keep`/`mark` roots all need, and it is what the GSUB builder applies to
/// remap operands — validation has to expand them exactly the same way or it
/// checks names the font never looks up. A name whose pattern does not expand
/// is returned as-is; the malformed pattern is reported elsewhere.
pub fn expand_name_element(s: &str, parts: &NamePartsMap) -> Vec<String> {
    parse_name_element(s, parts).into_vec()
}

/// The deferred form of [`expand_name_element`]: substitutes name parts and
/// parses, falling back to a single-name pattern on a malformed one, so the
/// caller can combine lengths and index lazily instead of materializing.
pub fn parse_name_element(s: &str, parts: &NamePartsMap) -> NamePattern {
    let substituted = substitute_name_parts(s, parts);
    match NamePattern::parse_element(&substituted) {
        Ok(pattern) => pattern,
        Err(_) => NamePattern::single(substituted),
    }
}

/// Replace `$var` tokens inside `(...)` groups with `val1|val2|...` from name-parts.
/// E.g. `hangul-init-($init)-l-f` with `$init = [g, gg, n]`
/// becomes `hangul-init-(g|gg|n)-l-f`.
///
/// Also expands inline numeric ranges: `($0..9)` → `(0|1|...|9)`,
/// `($#a0..af)` → `(a0|a1|...|af)`. Values are zero-padded to the width of the
/// *start* token, so `($00..12)` yields `00`…`12` while `($0..11)` yields
/// `0`…`11`.
///
/// A reference may carry a `*N` repeat, which distributes over every
/// substituted value — `($foo*2|bar)` becomes `(a*2|b*2|c*2|bar)` — so a
/// reference can be freely mixed with literal alternatives.  `**N` right at
/// the end of the group keeps its historical whole-group meaning and is left
/// for [`extract_group_mult`]; elsewhere it is treated like `*N`.
pub fn substitute_name_parts(s: &str, parts: &NamePartsMap) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // The reference itself: an inline range, or a `$name` looked up in
        // `parts` (`None` when undefined — then everything stays verbatim).
        let start = i;
        let (values, after_ref) = match try_expand_inline_range(&chars, i) {
            Some((end_pos, values)) if values.is_empty() => {
                // Malformed range; reported by `find_invalid_inline_ranges`.
                result.extend(&chars[start..end_pos]);
                i = end_pos;
                continue;
            }
            Some((end_pos, values)) => (Some(values), end_pos),
            None => {
                let mut j = start + 1;
                while j < chars.len()
                    && (chars[j].is_alphanumeric() || chars[j] == '-' || chars[j] == '_')
                {
                    j += 1;
                }
                let var: String = chars[start..j].iter().collect();
                (parts.get(&var).cloned(), j)
            }
        };

        let (repeat, after_suffix) = parse_repeat_suffix(&chars, after_ref);
        let Some(values) = values else {
            result.extend(&chars[start..after_suffix]);
            i = after_suffix;
            continue;
        };
        i = after_suffix;

        match repeat {
            // `**N` closing the group multiplies the whole group, not just
            // these values; leave it in place for the pattern parser.
            Some((2, n)) if after_suffix >= chars.len() || chars[after_suffix] == ')' => {
                result.push_str(&values.join("|"));
                result.push_str(&format!("**{n}"));
            }
            Some((_, n)) => {
                result.push_str(&values.join(&format!("*{n}|")));
                result.push_str(&format!("*{n}"));
            }
            None => result.push_str(&values.join("|")),
        }
    }
    result
}

/// Reads a `*N` / `**N` repeat suffix at `pos`.  Returns the star count and
/// the repeat, plus the position just past the suffix (`pos` if there is none).
fn parse_repeat_suffix(chars: &[char], pos: usize) -> (Option<(usize, usize)>, usize) {
    if pos >= chars.len() || chars[pos] != '*' {
        return (None, pos);
    }
    let stars = if pos + 1 < chars.len() && chars[pos + 1] == '*' {
        2
    } else {
        1
    };
    let digits_start = pos + stars;
    let mut end = digits_start;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    let digits: String = chars[digits_start..end].iter().collect();
    match digits.parse::<usize>() {
        Ok(n) => (Some((stars, n)), end),
        Err(_) => (None, pos),
    }
}

/// Try to parse an inline numeric range at `chars[start]` (which must be `$`).
///
/// Syntax: `$DIGITS..DIGITS` (decimal) or `$#HEX..HEX` (hexadecimal, lowercase).
/// Returns `Some((end_pos, values))`.  The minimum output width is determined
/// by the number of digits in the start number, so `$00..09` produces
/// `00`, `01`, …, `09`.
/// Returns `None` if the text doesn't match the range syntax at all.
/// Returns an empty value list if end < start (caller should leave as-is
/// or flag an error).
fn try_expand_inline_range(chars: &[char], start: usize) -> Option<(usize, Vec<String>)> {
    let mut i = start + 1; // skip '$'

    let is_hex = i < chars.len() && chars[i] == '#';
    if is_hex {
        i += 1;
    }

    let digit_pred: fn(char) -> bool = if is_hex {
        |c: char| c.is_ascii_hexdigit()
    } else {
        |c: char| c.is_ascii_digit()
    };

    let num_start = i;
    while i < chars.len() && digit_pred(chars[i]) {
        i += 1;
    }
    if i == num_start {
        return None;
    }
    let start_str: String = chars[num_start..i].iter().collect();

    if i + 1 >= chars.len() || chars[i] != '.' || chars[i + 1] != '.' {
        return None;
    }
    i += 2;

    let end_start = i;
    while i < chars.len() && digit_pred(chars[i]) {
        i += 1;
    }
    if i == end_start {
        return None;
    }
    let end_str: String = chars[end_start..i].iter().collect();

    let min_width = start_str.len();
    let radix = if is_hex { 16 } else { 10 };

    let start_val = u64::from_str_radix(&start_str, radix).ok()?;
    let end_val = u64::from_str_radix(&end_str, radix).ok()?;
    if end_val < start_val {
        return Some((i, Vec::new()));
    }
    let count = end_val - start_val + 1;
    if count > MAX_EXPANSION as u64 {
        return Some((i, Vec::new()));
    }

    let parts: Vec<String> = if is_hex {
        (start_val..=end_val)
            .map(|v| format!("{v:0>width$x}", width = min_width))
            .collect()
    } else {
        (start_val..=end_val)
            .map(|v| format!("{v:0>width$}", width = min_width))
            .collect()
    };
    Some((i, parts))
}

/// Whether `name` is a legal glyph name.
///
/// Letters, digits, `-`, `.`, `_`, and `:` — the last because a glyph name may
/// carry a variant suffix (`a-lower:compressed`). Deliberately narrower than
/// "any token": every character the pattern syntax uses (`(`, `)`, `|`, `$`,
/// `*`, `#`) is excluded, so a pattern that failed to expand cannot reach the
/// font as a name that merely looks odd. Face ids are narrower still, since
/// they become file names — see `crate::faces::is_valid_face_id`.
///
/// Checked against *expanded* names, so how one was written does not matter.
pub fn is_valid_glyph_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':'))
}

/// Check a string for invalid inline numeric ranges (`$end..start` where
/// end < start, or ranges exceeding `MAX_EXPANSION`). Returns descriptions
/// of each invalid range found.
pub fn find_invalid_inline_ranges(s: &str) -> Vec<String> {
    if !s.contains('$') {
        return Vec::new();
    }
    let mut errors = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$'
            && let Some((end_pos, values)) = try_expand_inline_range(&chars, i)
        {
            if values.is_empty() {
                let orig: String = chars[i..end_pos].iter().collect();
                errors.push(orig);
            }
            i = end_pos;
            continue;
        }
        i += 1;
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(s: &str) -> Vec<String> {
        NamePattern::parse_element(s).unwrap().into_vec()
    }

    fn block(s: &str) -> Vec<String> {
        NamePattern::parse(s).unwrap().into_vec()
    }

    /// The `U+XXXX..YYYY` form is gone. `uni($#XXXX..YYYY)` says the same thing
    /// and produces names inside the name charset — but `$#` is substituted
    /// textually *before* this layer, so what reaches it is the group.
    /// The charset every expanded name has to fit. `_` is in it because the
    /// jamo names use it (`hangul-init-_c-l-f`), `:` because of variant
    /// suffixes, and the pattern metacharacters are out so that a pattern which
    /// failed to expand cannot pass for a name.
    #[test]
    fn glyph_name_charset_admits_what_the_font_uses() {
        for good in [
            "a",
            "a-lower:compressed",
            "hangul-init-_c-l-f",
            "uni0041",
            ".notdef",
            "num.1",
        ] {
            assert!(is_valid_glyph_name(good), "{good} should be valid");
        }
        for bad in [
            "",
            "U+0041",
            "a b",
            "pat-($digit)",
            "a|b",
            "x*2",
            "$var",
            "한글",
            "a/b",
        ] {
            assert!(!is_valid_glyph_name(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn element_expands_hex_ranges() {
        let parts = NamePartsMap::new();
        let expanded = substitute_name_parts("uni($#2800..2802)", &parts);
        assert_eq!(element(&expanded), vec!["uni2800", "uni2801", "uni2802"]);
        assert_eq!(
            element("U+2800..2802"),
            vec!["U+2800..2802"],
            "`U+…` is now one verbatim name, not a range",
        );
    }

    /// A backwards or oversized `$#` range is left verbatim by substitution and
    /// caught by `find_invalid_inline_ranges`, which names the range rather than
    /// the whole pattern. This layer only sees whatever substitution produced.
    #[test]
    fn an_unsubstituted_inline_range_stays_verbatim() {
        let parts = NamePartsMap::new();
        for bad in ["uni($#2802..2800)", "uni($#00000000..FFFFFFFF)"] {
            assert_eq!(
                substitute_name_parts(bad, &parts),
                bad,
                "left for the checker"
            );
        }
        assert!(
            !crate::document::find_invalid_inline_ranges("uni($#2802..2800)").is_empty(),
            "the backwards range must be reported",
        );
    }

    #[test]
    fn element_rejects_oversized_repeat_before_materializing_it() {
        let pattern = format!("(name*{})", MAX_EXPANSION + 1);
        assert!(matches!(
            NamePattern::parse_element(&pattern),
            Err(NamePatternError::TooManyExpansions(_)),
        ));
    }

    #[test]
    fn element_group_mult() {
        assert_eq!(element("(a|b**3)"), vec!["a", "a", "a", "b", "b", "b"]);
        assert_eq!(
            element("(a*2|b**3)"),
            vec!["a", "a", "a", "a", "a", "a", "b", "b", "b"],
        );
    }

    #[test]
    fn element_top_level_pipe_acts_as_one_group() {
        // A `$var` substitution can leave `|` and `**N` at the top level;
        // the whole string then behaves as one parenthesized group.
        assert_eq!(element("a*2|b"), vec!["a", "a", "b"]);
        assert_eq!(element("v1|v2**2"), vec!["v1", "v1", "v2", "v2"]);
    }

    #[test]
    fn element_single_char_is_never_a_pattern() {
        assert_eq!(element("("), vec!["("]);
        assert_eq!(element("|"), vec!["|"]);
    }

    #[test]
    fn element_plain_name_is_single() {
        let p = NamePattern::parse_element("foo-bar").unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p.get(0), "foo-bar");
    }

    #[test]
    fn block_top_level_pipe_is_verbatim_list() {
        // Unlike element context, a block name list keeps branches verbatim.
        assert_eq!(block("a*2|b"), vec!["a*2", "b"]);
        assert_eq!(block("a | b"), vec!["a", "b"]);
    }

    #[test]
    fn block_bare_repeat() {
        assert_eq!(block("foo*3"), vec!["foo", "foo", "foo"]);
    }

    #[test]
    fn block_groups_combine_by_lcm() {
        assert_eq!(
            block("out-(a|b)-(1|2|3)"),
            vec![
                "out-a-1", "out-b-2", "out-a-3", "out-b-1", "out-a-2", "out-b-3"
            ],
        );
    }

    #[test]
    fn get_indexes_cyclically() {
        let p = NamePattern::parse("(a|b)").unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p.get(5), "b");
        assert_eq!(p.get(5), p.get(5 % p.len()));
    }

    #[test]
    fn range_len_is_available_without_materializing() {
        let parts = NamePartsMap::new();
        let p = NamePattern::parse(&substitute_name_parts("uni($#0000..FFFF)", &parts)).unwrap();
        assert_eq!(p.len(), 0x10000);
        assert_eq!(p.get(0x41), "uni0041");
    }

    #[test]
    fn combined_len_is_lcm_of_pattern_lens() {
        let a = NamePattern::parse_element("(x|y)").unwrap();
        let b = NamePattern::parse_element("(1|2|3)").unwrap();
        let c = NamePattern::parse_element("plain").unwrap();
        assert_eq!(combined_len([&a, &b, &c]), 6);
        assert_eq!(combined_len([] as [&NamePattern; 0]), 1);
    }

    #[test]
    fn parse_name_element_falls_back_to_the_substituted_literal() {
        let parts = NamePartsMap::new();
        // `foo*bar` is not a valid repeat, so the element grammar rejects it;
        // the fallback keeps the (substituted) name as-is.
        assert_eq!(expand_name_element("foo*bar", &parts), vec!["foo*bar"]);
    }

    #[test]
    fn inline_range_decimal() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($0..9)", &parts),
            "(0|1|2|3|4|5|6|7|8|9)",
        );
    }

    #[test]
    fn inline_range_decimal_zero_padded() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($00..12)", &parts),
            "(00|01|02|03|04|05|06|07|08|09|10|11|12)",
        );
    }

    #[test]
    fn inline_range_decimal_mixed_width() {
        let parts = NamePartsMap::new();
        let result = substitute_name_parts("($0..11)", &parts);
        assert!(result.starts_with("(0|1|2|"));
        assert!(result.contains("|9|10|11)"));
    }

    #[test]
    fn inline_range_hex() {
        let parts = NamePartsMap::new();
        assert_eq!(substitute_name_parts("($#a..f)", &parts), "(a|b|c|d|e|f)",);
    }

    #[test]
    fn inline_range_hex_zero_padded() {
        let parts = NamePartsMap::new();
        assert_eq!(substitute_name_parts("($#0a..0c)", &parts), "(0a|0b|0c)",);
    }

    #[test]
    fn inline_range_reversed_leaves_as_is() {
        let parts = NamePartsMap::new();
        assert_eq!(substitute_name_parts("($3..2)", &parts), "($3..2)",);
    }

    #[test]
    fn inline_range_in_glyph_name() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("sup-($0..9)", &parts),
            "sup-(0|1|2|3|4|5|6|7|8|9)",
        );
    }

    #[test]
    fn inline_range_find_invalid() {
        assert_eq!(find_invalid_inline_ranges("($3..2)"), vec!["$3..2"],);
        assert!(find_invalid_inline_ranges("($0..9)").is_empty());
    }

    fn abc_parts() -> NamePartsMap {
        let mut parts = NamePartsMap::new();
        parts.insert(
            "$foo".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        parts
    }

    #[test]
    fn name_part_mixed_with_literal_alternatives() {
        let parts = abc_parts();
        assert_eq!(substitute_name_parts("($foo|bar)", &parts), "(a|b|c|bar)");
        assert_eq!(
            element(&substitute_name_parts("($foo|bar)", &parts)),
            vec!["a", "b", "c", "bar"],
        );
        assert_eq!(
            element(&substitute_name_parts("(x|$foo|y)", &parts)),
            vec!["x", "a", "b", "c", "y"],
        );
    }

    #[test]
    fn name_part_repeat_distributes_over_every_value() {
        let parts = abc_parts();
        // `*N` right after a name-part repeats *each* value, not just the last.
        assert_eq!(
            element(&substitute_name_parts("($foo*2)", &parts)),
            vec!["a", "a", "b", "b", "c", "c"]
        );
        // Same when other alternatives follow, where the repeat can no longer
        // be read as a whole-group multiplier.
        assert_eq!(
            element(&substitute_name_parts("($foo*2|bar)", &parts)),
            vec!["a", "a", "b", "b", "c", "c", "bar"],
        );
        assert_eq!(
            element(&substitute_name_parts("($foo**2|bar)", &parts)),
            vec!["a", "a", "b", "b", "c", "c", "bar"],
        );
    }

    #[test]
    fn inline_range_repeat_distributes_over_every_value() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($#0a..0c*2)", &parts),
            "(0a*2|0b*2|0c*2)",
        );
        assert_eq!(
            element(&substitute_name_parts("($0..2*2|z)", &parts)),
            vec!["0", "0", "1", "1", "2", "2", "z"]
        );
    }

    #[test]
    fn mid_group_double_star_is_a_syntax_error() {
        // `**N` is the whole-group multiplier and only means anything at the
        // end of the group; per-alternative repeats must be written `*N`.
        assert!(matches!(
            NamePattern::parse_element("(a|b**3|c)"),
            Err(NamePatternError::Syntax(_)),
        ));
        assert_eq!(element("(a|b*3|c)"), vec!["a", "b", "b", "b", "c"]);
    }

    #[test]
    fn arbitrary_mixture_of_alternative_forms() {
        let mut parts = NamePartsMap::new();
        parts.insert("$bar".to_string(), vec!["b1".to_string(), "b2".to_string()]);
        let s = substitute_name_parts("(foo|$bar|baz*5|$#00..02*3**2)", &parts);
        let mut expected = vec!["foo", "foo", "b1", "b1", "b2", "b2"];
        expected.extend(std::iter::repeat_n("baz", 10));
        expected.extend(std::iter::repeat_n("00", 6));
        expected.extend(std::iter::repeat_n("01", 6));
        expected.extend(std::iter::repeat_n("02", 6));
        assert_eq!(element(&s), expected);
    }

    #[test]
    fn substitute_name_parts_with_group_mult_suffix() {
        let mut parts = NamePartsMap::new();
        parts.insert(
            "$foo".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        assert_eq!(substitute_name_parts("($foo**3)", &parts), "(a|b|c**3)",);
        // Without suffix, normal expansion.
        assert_eq!(substitute_name_parts("($foo)", &parts), "(a|b|c)",);
        // Unknown var keeps suffix verbatim.
        assert_eq!(
            substitute_name_parts("($bar**2)", &NamePartsMap::new()),
            "($bar**2)",
        );
    }
}
