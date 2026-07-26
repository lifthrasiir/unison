//! Name pattern parsing and expansion.
//!
//! A *name pattern* is a compact notation for a list of glyph names:
//!
//! - `U+XXXX..YYYY` — a codepoint range, one `U+NNNN` name per codepoint;
//! - `(a|b|c)` — an alternation group spliced into the surrounding literal
//!   text (`foo-(a|b)` → `foo-a`, `foo-b`);
//! - `a*N` — inside a group, repeats one alternative N times;
//! - `(...**N)` — repeats each alternative of the whole group N times;
//! - `$var` / `$0..9` / `$#a0..af` — name-part references and inline numeric
//!   ranges, substituted textually by [`substitute_name_parts`] *before*
//!   pattern parsing.
//!
//! Multiple groups in one pattern combine cyclically: the total length is the
//! LCM of the group sizes and group `k` contributes its `i % len(k)`-th
//! alternative to the `i`-th name.  This cyclic indexing is what lets remap
//! operands and `ref` targets expand in lock-step with a glyph-name pattern.
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
    /// `U+XXXX..YYYY`: names are generated from the codepoint, never stored.
    Range { start: u32 },
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
        NamePattern { kind: Kind::Single(name), len: 1 }
    }

    /// Parses a single name element (`map`/`remap` operands, `assume unused`
    /// arguments, runtime ref lookups).  A top-level `|` or `*` without any
    /// parentheses wraps the whole string into one alternation group.
    pub fn parse_element(s: &str) -> Result<Self, NamePatternError> {
        if let Some(range) = parse_codepoint_range(s) {
            return range;
        }
        if s.chars().count() <= 1 || (!s.contains('(') && !s.contains('|') && !s.contains('*')) {
            return Ok(Self::single(s.to_string()));
        }
        if s.contains('(') {
            Self::from_segments(s)
        } else {
            Self::from_segments(&format!("({s})"))
        }
    }

    /// Parses a glyph block name: a codepoint range, a top-level verbatim
    /// `name1|name2` list, alternation groups, or a bare `foo*N` repeat.
    pub fn parse(s: &str) -> Result<Self, NamePatternError> {
        if let Some(range) = parse_codepoint_range(s) {
            return range;
        }

        if has_top_level_pipe(s) {
            let names: Vec<String> = split_top_level_pipes(s)
                .into_iter()
                .map(|p| p.trim().to_string())
                .collect();
            let len = names.len();
            if len > MAX_EXPANSION {
                return Err(NamePatternError::TooManyExpansions(len));
            }
            return Ok(NamePattern { kind: Kind::List(names), len });
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
            return Ok(NamePattern { kind: Kind::Segments(vec![Segment::Alts(alts)]), len });
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

        Ok(NamePattern { kind: Kind::Segments(segments), len })
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
            Kind::Range { start } => {
                let cp = start + (i % self.len) as u32;
                format!("U+{cp:04X}")
            }
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
        Iter { pattern: self, i: 0 }
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
        IntoIter { pattern: self, i: 0 }
    }
}

/// Combined expansion length of several patterns indexed in lock-step:
/// the LCM of the individual lengths.
pub fn combined_len<'a>(patterns: impl IntoIterator<Item = &'a NamePattern>) -> usize {
    patterns.into_iter().fold(1, |acc, p| lcm(acc, p.len()))
}

/// `U+XXXX..YYYY` / `u+XXXX..YYYY`; `None` when `s` is not range-shaped.
fn parse_codepoint_range(s: &str) -> Option<Result<NamePattern, NamePatternError>> {
    let hex_rest = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))?;
    let (start_hex, end_hex) = hex_rest.split_once("..")?;
    Some((|| {
        let start = u32::from_str_radix(start_hex, 16)
            .map_err(|_| NamePatternError::Syntax(format!("bad range start: {start_hex}")))?;
        let end = u32::from_str_radix(end_hex, 16)
            .map_err(|_| NamePatternError::Syntax(format!("bad range end: {end_hex}")))?;
        if end < start {
            return Err(NamePatternError::Syntax("range end < start".into()));
        }
        let count = u64::from(end) - u64::from(start) + 1;
        if count > MAX_EXPANSION as u64 {
            return Err(NamePatternError::TooManyExpansions(
                usize::try_from(count).unwrap_or(usize::MAX),
            ));
        }
        Ok(NamePattern { kind: Kind::Range { start }, len: count as usize })
    })())
}

/// Splits `(...)` group content on `|`, applying `a*N` per-alternative
/// repeats and a trailing `**N` whole-group multiplier.
fn parse_alt_content(content: &str) -> Result<Vec<String>, NamePatternError> {
    let (content, group_mult) = extract_group_mult(content)?;

    let mut alts = Vec::new();
    for part in content.split('|') {
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

/// Recognizes a `**N` multiplier at the end of a group's last alternative and
/// splits it off.  Returns `(content_without_multiplier, multiplier)`.
fn extract_group_mult(content: &str) -> Result<(&str, usize), NamePatternError> {
    let last_pipe = content.rfind('|').map_or(0, |p| p + 1);
    let last_alt = &content[last_pipe..];
    if let Some(pos) = last_alt.rfind("**") {
        let after = &last_alt[pos + 2..];
        if !after.is_empty() && after.bytes().all(|b| b.is_ascii_digit()) {
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
/// sticky/mark roots all need, and it is what the GSUB builder applies to
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
/// `($#a0..af)` → `(a0|a1|...|af)`.
pub fn substitute_name_parts(s: &str, parts: &NamePartsMap) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            if let Some((end_pos, expanded)) = try_expand_inline_range(&chars, i) {
                if expanded.is_empty() {
                    let orig: String = chars[i..end_pos].iter().collect();
                    result.push_str(&orig);
                } else {
                    result.push_str(&expanded);
                }
                i = end_pos;
                continue;
            }
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            let var: String = chars[start..i].iter().collect();
            // Check for a `**N` group-multiplier suffix.
            let suffix_start = i;
            let mut group_mult = String::new();
            if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
                let star = i;
                i += 2;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i > star + 2 {
                    group_mult = chars[star..i].iter().collect();
                } else {
                    i = suffix_start;
                }
            }
            if let Some(values) = parts.get(&var) {
                result.push_str(&values.join("|"));
                result.push_str(&group_mult);
            } else {
                result.push_str(&var);
                result.push_str(&group_mult);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Try to parse an inline numeric range at `chars[start]` (which must be `$`).
///
/// Syntax: `$DIGITS..DIGITS` (decimal) or `$#HEX..HEX` (hexadecimal, lowercase).
/// Returns `Some((end_pos, expanded))` where `expanded` is `v1|v2|...|vn`.
/// The minimum output width is determined by the number of digits in the start
/// number, so `$00..09` produces `00|01|...|09`.
/// Returns `None` if the text doesn't match the range syntax at all.
/// Returns an empty expansion string if end < start (caller should leave as-is
/// or flag an error).
fn try_expand_inline_range(chars: &[char], start: usize) -> Option<(usize, String)> {
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
        return Some((i, String::new()));
    }
    let count = end_val - start_val + 1;
    if count > MAX_EXPANSION as u64 {
        return Some((i, String::new()));
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
    Some((i, parts.join("|")))
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
        if chars[i] == '$' {
            if let Some((end_pos, expanded)) = try_expand_inline_range(&chars, i) {
                if expanded.is_empty() {
                    let orig: String = chars[i..end_pos].iter().collect();
                    errors.push(orig);
                }
                i = end_pos;
                continue;
            }
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

    #[test]
    fn element_expands_codepoint_ranges() {
        assert_eq!(element("U+2800..2802"), vec!["U+2800", "U+2801", "U+2802"]);
        assert_eq!(element("u+00fe..0100"), vec!["U+00FE", "U+00FF", "U+0100"]);
    }

    #[test]
    fn element_rejects_invalid_or_oversized_ranges() {
        assert!(matches!(
            NamePattern::parse_element("U+2802..2800"),
            Err(NamePatternError::Syntax(_)),
        ));
        assert!(matches!(
            NamePattern::parse_element("U+00000000..FFFFFFFF"),
            Err(NamePatternError::TooManyExpansions(_)),
        ));
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
            vec!["out-a-1", "out-b-2", "out-a-3", "out-b-1", "out-a-2", "out-b-3"],
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
        let p = NamePattern::parse("U+0000..FFFF").unwrap();
        assert_eq!(p.len(), 0x10000);
        assert_eq!(p.get(0x41), "U+0041");
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
        assert_eq!(
            substitute_name_parts("($#a..f)", &parts),
            "(a|b|c|d|e|f)",
        );
    }

    #[test]
    fn inline_range_hex_zero_padded() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($#0a..0c)", &parts),
            "(0a|0b|0c)",
        );
    }

    #[test]
    fn inline_range_reversed_leaves_as_is() {
        let parts = NamePartsMap::new();
        assert_eq!(
            substitute_name_parts("($3..2)", &parts),
            "($3..2)",
        );
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
        assert_eq!(
            find_invalid_inline_ranges("($3..2)"),
            vec!["$3..2"],
        );
        assert!(find_invalid_inline_ranges("($0..9)").is_empty());
    }

    #[test]
    fn substitute_name_parts_with_group_mult_suffix() {
        let mut parts = NamePartsMap::new();
        parts.insert(
            "$foo".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        assert_eq!(
            substitute_name_parts("($foo**3)", &parts),
            "(a|b|c**3)",
        );
        // Without suffix, normal expansion.
        assert_eq!(
            substitute_name_parts("($foo)", &parts),
            "(a|b|c)",
        );
        // Unknown var keeps suffix verbatim.
        assert_eq!(
            substitute_name_parts("($bar**2)", &NamePartsMap::new()),
            "($bar**2)",
        );
    }
}
