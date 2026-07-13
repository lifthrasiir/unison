use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use crate::pixel::PixelShape;

#[derive(Clone, Debug, PartialEq)]
pub struct PixelGrid {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<PixelShape>,
}

impl PixelGrid {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![PixelShape::EMPTY; width as usize * height as usize],
        }
    }

    pub fn get(&self, row: u16, col: u16) -> PixelShape {
        if row < self.height && col < self.width {
            self.pixels[row as usize * self.width as usize + col as usize]
        } else {
            PixelShape::EMPTY
        }
    }

    pub fn set(&mut self, row: u16, col: u16, shape: PixelShape) {
        if row < self.height && col < self.width {
            self.pixels[row as usize * self.width as usize + col as usize] = shape;
        }
    }

    pub fn resize(&mut self, new_width: u16, new_height: u16) {
        if new_width == self.width && new_height == self.height {
            return;
        }
        let mut new_pixels =
            vec![PixelShape::EMPTY; new_width as usize * new_height as usize];
        let copy_w = self.width.min(new_width) as usize;
        let copy_h = self.height.min(new_height) as usize;
        for r in 0..copy_h {
            for c in 0..copy_w {
                new_pixels[r * new_width as usize + c] =
                    self.pixels[r * self.width as usize + c];
            }
        }
        self.width = new_width;
        self.height = new_height;
        self.pixels = new_pixels;
    }

    pub fn is_all_empty(&self) -> bool {
        self.pixels.iter().all(|s| s.is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct GlyphRef {
    pub name: String,
    /// `(col, row)` offset. `None` = auto-resolve from points (adjoin), defaulting to (0, 0).
    pub offset: Option<(i16, i16)>,
    pub negated: bool,
}

impl GlyphRef {
    pub fn row(&self) -> i16 {
        self.offset.map_or(0, |(_, r)| r)
    }

    pub fn col(&self) -> i16 {
        self.offset.map_or(0, |(c, _)| c)
    }
}

#[derive(Clone, Debug)]
pub struct GlyphPoint {
    pub position: String,
    pub col: i16,
    pub row: i16,
    /// Inclusive end of the column range. Equal to `col` for single-cell anchors.
    pub col_end: i16,
    /// Inclusive end of the row range. Equal to `row` for single-cell anchors.
    pub row_end: i16,
}

impl GlyphPoint {
    pub fn width(&self) -> u16 {
        (self.col_end - self.col + 1) as u16
    }

    pub fn height(&self) -> u16 {
        (self.row_end - self.row + 1) as u16
    }

    pub fn is_single_cell(&self) -> bool {
        self.col == self.col_end && self.row == self.row_end
    }

    pub fn size_matches(&self, other: &GlyphPoint) -> bool {
        self.width() == other.width() && self.height() == other.height()
    }
}

#[derive(Clone, Debug)]
pub struct GlyphBody {
    pub pixels: Option<PixelGrid>,
    pub refs: Vec<GlyphRef>,
    pub points: Vec<GlyphPoint>,
    pub sticky: bool,
    pub inline: bool,
    pub advance: Option<u16>,
    pub left: Option<i16>,
}

impl GlyphBody {
    pub fn new() -> Self {
        Self {
            pixels: None,
            refs: Vec::new(),
            points: Vec::new(),
            sticky: false,
            inline: false,
            advance: None,
            left: None,
        }
    }

    /// True if this body is a simple alias (`glyph NAME = ALIAS`): no pixel
    /// data, exactly one ref, with no positional offset.
    pub fn is_simple_alias(&self) -> bool {
        self.pixels.is_none()
            && self.refs.len() == 1
            && self.refs[0].offset.is_none()
            && !self.refs[0].negated
            && self.points.is_empty()
            && !self.sticky
            && !self.inline
            && self.advance.is_none()
            && self.left.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphName(pub String);

impl GlyphName {
    pub fn display(&self) -> String {
        self.0.clone()
    }
}

#[derive(Clone, Debug)]
pub enum DocumentItem {
    Comment(String),
    BlankLine,
    Directive(String),
    FontMeta(String),
    Glyph {
        name: GlyphName,
        body: GlyphBody,
    },
    /// `map CHAR = GLYPH` — cmap mapping from a Unicode character to a glyph name.
    Map {
        char_repr: String,
        glyph: String,
    },
    /// `name-parts $NAME = token1 token2 $ref3 ...`
    NameParts {
        name: String,
        values: Vec<String>,
    },
    /// `remap FEATURE : [LOOKBEHIND... :] SOURCE -> TARGET [: LOOKAHEAD...]`
    Remap {
        feature: String,
        lookbehind: Vec<String>,
        source: String,
        target: String,
        lookahead: Vec<String>,
    },
    /// `feature NAME for SCRIPT... : REMAP_GROUP`
    Feature {
        name: String,
        scripts: Vec<String>,
        remap_group: String,
    },
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct Document {
    pub items: Vec<DocumentItem>,
    pub item_line_starts: Vec<usize>,
    /// Maps each DocLine index to its 0-based file line number.
    pub docline_file_lines: Vec<usize>,
    pub path: PathBuf,
    pub dirty: bool,
    pub edit_gen: u64,
}

impl Document {
    pub fn new(path: PathBuf) -> Self {
        Self {
            items: Vec::new(),
            item_line_starts: Vec::new(),
            docline_file_lines: Vec::new(),
            path,
            dirty: false,
            edit_gen: 0,
        }
    }
}

pub fn compute_docline_file_lines(lines: &[DocLine]) -> Vec<usize> {
    let mut result = Vec::with_capacity(lines.len());
    let mut file_line = 0usize;
    for line in lines {
        result.push(file_line);
        match line {
            DocLine::Text(_) => file_line += 1,
            DocLine::Grid(grid) => {
                if !grid.is_all_empty() {
                    file_line += grid.height as usize;
                }
            }
        }
    }
    result
}

impl DocumentItem {
    /// Parse a structured directive from pre-tokenized tokens.
    /// The first token is the keyword ("name-parts", "remap", or "feature").
    pub fn parse_directive(tokens: &[String]) -> DocumentItem {
        if tokens.is_empty() {
            return DocumentItem::Directive(String::new());
        }
        match tokens[0].as_str() {
            "name-parts" => {
                let rest = &tokens[1..];
                if rest.len() >= 3 && rest[1] == "=" {
                    return DocumentItem::NameParts {
                        name: rest[0].clone(),
                        values: rest[2..].to_vec(),
                    };
                }
            }
            "remap" => {
                if let Some(item) = Self::parse_remap(&tokens[1..]) {
                    return item;
                }
            }
            "feature" => {
                let rest = &tokens[1..];
                // feature NAME for SCRIPT... : REMAP_GROUP
                if let Some(for_pos) = rest.iter().position(|t| t == "for") {
                    if let Some(colon_pos) = rest.iter().position(|t| t == ":") {
                        if for_pos == 1 && colon_pos > 2 && colon_pos + 1 < rest.len() {
                            return DocumentItem::Feature {
                                name: rest[0].clone(),
                                scripts: rest[2..colon_pos].to_vec(),
                                remap_group: rest[colon_pos + 1].clone(),
                            };
                        }
                    }
                }
            }
            _ => {}
        }
        let quoted: Vec<String> = tokens.iter().map(|t| crate::document_io::quote_token(t)).collect();
        DocumentItem::Directive(quoted.join(" "))
    }

    fn parse_remap(tokens: &[String]) -> Option<DocumentItem> {
        let arrow_pos = tokens.iter().position(|t| t == "->")?;

        let colon_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| t.as_str() == ":")
            .map(|(i, _)| i)
            .collect();

        let first = tokens.first()?;
        let (feature, first_colon_after_feature) = if first.ends_with(':') && first.len() > 1 {
            (first.trim_end_matches(':').to_string(), 1)
        } else {
            let fc = colon_positions.iter().copied().find(|&p| p < arrow_pos)?;
            (first.clone(), fc + 1)
        };

        let last_colon_before_arrow = colon_positions
            .iter()
            .copied().rfind(|&p| p >= first_colon_after_feature && p < arrow_pos);

        let (lookbehind, source_start) = if let Some(lc) = last_colon_before_arrow {
            let lb: Vec<String> = tokens[first_colon_after_feature..lc].to_vec();
            (lb, lc + 1)
        } else {
            (Vec::new(), first_colon_after_feature)
        };

        let source = tokens[source_start..arrow_pos].join(" ");

        let after_arrow = arrow_pos + 1;
        let lookahead_colon = colon_positions.iter().copied().find(|&p| p > arrow_pos);

        let (target, lookahead) = if let Some(lc) = lookahead_colon {
            let target = tokens[after_arrow..lc].join(" ");
            let la: Vec<String> = tokens[lc + 1..].to_vec();
            (target, la)
        } else {
            (tokens[after_arrow..].join(" "), Vec::new())
        };

        Some(DocumentItem::Remap {
            feature,
            lookbehind,
            source,
            target,
            lookahead,
        })
    }

    pub fn serialize_line(&self) -> Option<String> {
        use crate::document_io::quote_token;
        match self {
            DocumentItem::NameParts { name, values } => {
                let qvals: Vec<String> = values.iter().map(|v| quote_token(v)).collect();
                Some(format!("name-parts {} = {}", quote_token(name), qvals.join(" ")))
            }
            DocumentItem::Remap {
                feature,
                lookbehind,
                source,
                target,
                lookahead,
            } => {
                let mut parts = vec![format!("remap {} :", quote_token(feature))];
                if !lookbehind.is_empty() {
                    let lb: Vec<String> = lookbehind.iter().map(|s| quote_token(s)).collect();
                    parts.push(format!("{} :", lb.join(" ")));
                }
                parts.push(format!("{} -> {}", quote_token(source), quote_token(target)));
                if !lookahead.is_empty() {
                    let la: Vec<String> = lookahead.iter().map(|s| quote_token(s)).collect();
                    parts.push(format!(": {}", la.join(" ")));
                }
                Some(parts.join(" "))
            }
            DocumentItem::Feature { name, scripts, remap_group } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                Some(format!(
                    "feature {} for {} : {}",
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(remap_group),
                ))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Name pattern expansion: `foo-(a|b|c)` → ["foo-a", "foo-b", "foo-c"]
// ---------------------------------------------------------------------------

pub const MAX_EXPANSION: usize = 1 << 16;

#[derive(Clone, Debug)]
pub struct ExpandedNames {
    names: Vec<String>,
}

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

impl ExpandedNames {
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(|s| s.as_str())
    }

    pub fn into_vec(self) -> Vec<String> {
        self.names
    }
}

impl<'a> IntoIterator for &'a ExpandedNames {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;
    fn into_iter(self) -> Self::IntoIter {
        self.names.iter()
    }
}

impl IntoIterator for ExpandedNames {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;
    fn into_iter(self) -> Self::IntoIter {
        self.names.into_iter()
    }
}

/// Returns true if the name string looks like a multi-glyph pattern
/// (contains alternation `|`, grouping `(`, or range `..`).
/// Single-character names are never patterns even if the character is `(` or `|`.
pub fn has_bare_repeat(s: &str) -> bool {
    if let Some((_, count_str)) = s.rsplit_once('*') {
        !count_str.is_empty() && count_str.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    }
}

pub fn is_name_pattern(s: &str) -> bool {
    s.chars().count() > 1
        && (s.contains('(') || s.contains('|') || s.contains("..") || has_bare_repeat(s))
}

pub fn expand_name_pattern(s: &str) -> Result<ExpandedNames, NamePatternError> {
    if let Some(hex_rest) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..") {
            let start = u32::from_str_radix(start_hex, 16).map_err(|_| {
                NamePatternError::Syntax(format!("bad range start: {start_hex}"))
            })?;
            let end = u32::from_str_radix(end_hex, 16).map_err(|_| {
                NamePatternError::Syntax(format!("bad range end: {end_hex}"))
            })?;
            if end < start {
                return Err(NamePatternError::Syntax("range end < start".into()));
            }
            let count = u64::from(end) - u64::from(start) + 1;
            if count > MAX_EXPANSION as u64 {
                return Err(NamePatternError::TooManyExpansions(
                    usize::try_from(count).unwrap_or(usize::MAX),
                ));
            }
            let names = (start..=end).map(|cp| format!("U+{cp:04X}")).collect();
            return Ok(ExpandedNames { names });
        }

    if s.chars().count() <= 1 || (!s.contains('(') && !s.contains('|') && !s.contains('*')) {
        return Ok(ExpandedNames {
            names: vec![s.to_string()],
        });
    }

    let normalized = if !s.contains('(') {
        format!("({s})")
    } else {
        s.to_string()
    };

    let normalized_replaced = normalized.replace('(', ")");
    let raw_parts: Vec<&str> = normalized_replaced.split(')').collect();

    if raw_parts.len().is_multiple_of(2) {
        return Err(NamePatternError::Syntax("unmatched parentheses".into()));
    }

    enum Part {
        Fixed(String),
        Alternation(Vec<String>),
    }

    let mut parts: Vec<Part> = Vec::new();
    for (i, part) in raw_parts.iter().enumerate() {
        if i % 2 == 0 {
            parts.push(Part::Fixed(part.to_string()));
        } else {
            let mut alts = Vec::new();
            for alt in part.split('|') {
                if let Some((name, rep_str)) = alt.rsplit_once('*') {
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
                    alts.push(alt.to_string());
                }
            }
            if alts.is_empty() {
                return Err(NamePatternError::Syntax("empty alternation group".into()));
            }
            parts.push(Part::Alternation(alts));
        }
    }

    let mut count: usize = 1;
    for part in &parts {
        if let Part::Alternation(alts) = part {
            count = lcm(count, alts.len());
            if count > MAX_EXPANSION {
                return Err(NamePatternError::TooManyExpansions(count));
            }
        }
    }

    let mut names = Vec::with_capacity(count);
    for k in 0..count {
        let mut name = String::new();
        for part in &parts {
            match part {
                Part::Fixed(s) => name.push_str(s),
                Part::Alternation(alts) => {
                    name.push_str(&alts[k % alts.len()]);
                }
            }
        }
        names.push(name);
    }

    Ok(ExpandedNames { names })
}

// ---------------------------------------------------------------------------
// Name-parts collection and $var substitution
// ---------------------------------------------------------------------------

pub type NamePartsMap = HashMap<String, Vec<String>>;

pub fn collect_name_parts(docs: &[&Document]) -> NamePartsMap {
    let mut map = NamePartsMap::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::NameParts { name, values } = item {
                let mut resolved = Vec::new();
                for token in values {
                    if token.starts_with('$') {
                        if let Some(referenced) = map.get(token.as_str()) {
                            if referenced.len() > MAX_EXPANSION.saturating_sub(resolved.len()) {
                                resolved.push(token.clone());
                            } else {
                                resolved.extend(referenced.iter().cloned());
                            }
                        } else {
                            resolved.push(token.clone());
                        }
                    } else {
                        for part in token.split('|') {
                            if part.is_empty() || part == "``" {
                                resolved.push(String::new());
                            } else if let Some((name_part, rep_str)) = part.rsplit_once('*') {
                                if let Ok(rep) = rep_str.parse::<usize>() {
                                    if rep > MAX_EXPANSION.saturating_sub(resolved.len()) {
                                        resolved.push(part.to_string());
                                    } else {
                                        for _ in 0..rep {
                                            resolved.push(name_part.to_string());
                                        }
                                    }
                                } else {
                                    resolved.push(part.to_string());
                                }
                            } else {
                                resolved.push(part.to_string());
                            }
                        }
                    }
                }
                map.insert(name.clone(), resolved);
            }
        }
    }
    map
}

/// Replace `$var` tokens inside `(...)` groups with `val1|val2|...` from name-parts.
/// E.g. `hangul-init-($init)-l-f` with `$init = [g, gg, n]`
/// becomes `hangul-init-(g|gg|n)-l-f`.
pub fn substitute_name_parts(s: &str, parts: &NamePartsMap) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            let var: String = chars[start..i].iter().collect();
            if let Some(values) = parts.get(&var) {
                result.push_str(&values.join("|"));
            } else {
                result.push_str(&var);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Glyph-block pattern expansion: the richer engine used when *building* a
// font from a whole `glyph NAME\nref ...\nref ...` item (as opposed to
// `expand_name_pattern` above, which only expands a single name string).
//
// This additionally supports `U+XXXX..YYYY` codepoint ranges and top-level
// `name1|name2|...` lists (no enclosing parens) on the glyph name, and lets
// each `ref` line's target name carry its own `(a|b|c)` alternation that is
// expanded in lock-step with the name pattern.
//
// Multiple alternation groups use the same LCM/cyclic-repeat semantics as
// `expand_name_pattern`; this richer engine additionally handles ranges,
// top-level lists, and ref patterns.
enum Segment {
    Literal(String),
    Alts(Vec<String>),
}

fn parse_alt_content(content: &str) -> Result<Vec<String>, String> {
    let mut alts = Vec::new();
    for part in content.split('|') {
        if let Some((name, count_str)) = part.rsplit_once('*') {
            let n: usize = count_str
                .parse()
                .map_err(|_| format!("invalid repeat count: {count_str}"))?;
            if n > MAX_EXPANSION.saturating_sub(alts.len()) {
                return Err("alternation too large".into());
            }
            for _ in 0..n {
                alts.push(name.to_string());
            }
        } else {
            alts.push(part.to_string());
        }
    }
    if alts.is_empty() {
        return Err("alternation must contain at least one value".into());
    }
    Ok(alts)
}

/// Parse `(a|b|c)` groups in a string. Returns (expansion_count, segments).
/// Group counts combine by least common multiple (cyclic repeat).
fn parse_line_segments(s: &str) -> Result<(usize, Vec<Segment>), String> {
    let mut segments = Vec::new();
    let bytes = s.as_bytes();
    let mut pos = 0;
    let mut lit_start = 0;
    let mut group_counts: Vec<usize> = Vec::new();

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
                return Err(format!("unmatched '(' in: {s}"));
            }
            let content = &s[open + 1..pos - 1];
            let alts = parse_alt_content(content)?;
            let n = alts.len();
            if n > 1 {
                group_counts.push(n);
            }
            segments.push(Segment::Alts(alts));
            lit_start = pos;
        } else {
            pos += 1;
        }
    }
    if lit_start < s.len() {
        segments.push(Segment::Literal(s[lit_start..].to_string()));
    }

    // Bare `foo*N` (no parentheses) → treat as `(foo*N)`.
    if group_counts.is_empty() && segments.len() == 1
        && let Segment::Literal(ref lit) = segments[0]
            && has_bare_repeat(lit) {
                let alts = parse_alt_content(lit)?;
                let n = alts.len();
                segments = vec![Segment::Alts(alts)];
                if n > 1 {
                    group_counts.push(n);
                }
            }

    let mut count = 1usize;
    for group_count in group_counts {
        count = (count / gcd(count, group_count))
            .checked_mul(group_count)
            .ok_or_else(|| "expansion too large".to_string())?;
        if count > MAX_EXPANSION {
            return Err(format!("expansion too large: {count}"));
        }
    }
    Ok((count, segments))
}

fn expand_segments_at(segments: &[Segment], i: usize) -> String {
    let mut out = String::new();
    for seg in segments {
        match seg {
            Segment::Literal(s) => out.push_str(s),
            Segment::Alts(alts) => {
                if alts.len() == 1 {
                    out.push_str(&alts[0]);
                } else {
                    out.push_str(&alts[i % alts.len()]);
                }
            }
        }
    }
    out
}

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

pub fn parse_glyph_name(s: &str) -> GlyphName {
    GlyphName(s.trim().to_string())
}

/// Parse the glyph name pattern. Returns (parsed segments/names, count).
/// Handles U+XXXX..YYYY ranges, top-level pipes, and (a|b|c) groups.
fn parse_name_pattern(s: &str) -> Result<(NamePattern, usize), String> {
    // U+XXXX..YYYY codepoint range
    if let Some(hex_rest) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..") {
            let start = u32::from_str_radix(start_hex, 16)
                .map_err(|_| format!("bad range start: {start_hex}"))?;
            let end = u32::from_str_radix(end_hex, 16)
                .map_err(|_| format!("bad range end: {end_hex}"))?;
            if end < start {
                return Err("range end < start".into());
            }
            let count = u64::from(end) - u64::from(start) + 1;
            if count > MAX_EXPANSION as u64 {
                return Err(format!("codepoint range too large: {count}"));
            }
            let count = count as usize;
            return Ok((NamePattern::Range(start), count));
        }

    // Top-level pipe: name1|name2|...
    if has_top_level_pipe(s) {
        let names: Vec<String> = s.split('|').map(|p| p.trim().to_string()).collect();
        let count = names.len();
        if count > MAX_EXPANSION {
            return Err(format!("pipe list too large: {count}"));
        }
        return Ok((NamePattern::List(names), count));
    }

    // (a|b|c) alternation
    let (count, segments) = parse_line_segments(s)?;
    Ok((NamePattern::Segments(segments), count))
}

enum NamePattern {
    Range(u32),
    List(Vec<String>),
    Segments(Vec<Segment>),
}

fn expand_name_at(pattern: &NamePattern, count: usize, i: usize) -> GlyphName {
    match pattern {
        NamePattern::Range(start) => {
            let cp = start + (i % count) as u32;
            GlyphName(format!("U+{cp:04X}"))
        }
        NamePattern::List(names) => GlyphName(names[i % names.len()].clone()),
        NamePattern::Segments(segs) => parse_glyph_name(&expand_segments_at(segs, i)),
    }
}

/// Expand a ref-only glyph item (`glyph NAME` + `ref ...` lines, no pixel
/// data) whose name and/or ref targets carry alternation/range patterns,
/// directly from its in-memory `GlyphName`/`GlyphRef`s (no serialize/reparse
/// round-trip through `.unf` text).
///
/// Mirrors the historical behavior exactly: pixel data is not meaningful
/// for a batch of expanded ref-composites, so expanded items always come
/// out as `pixels: None` — this function is only ever called on an
/// already-pattern-named item, and `.unf` content never combines a
/// pattern name with pixel data on the same glyph (patterns are only
/// used for ref/composite batches).
pub fn expand_glyph_block(name: &GlyphName, refs: &[GlyphRef]) -> Result<Vec<DocumentItem>, String> {
    let name_str = name.display();
    let (name_pattern, name_count) = parse_name_pattern(&name_str)?;

    let mut parsed_refs: Vec<(Vec<Segment>, Option<(i16, i16)>, bool)> = Vec::new();
    for r in refs {
        let (_, segs) = parse_line_segments(&r.name)?;
        parsed_refs.push((segs, r.offset, r.negated));
    }

    // The glyph-name pattern determines how many glyphs are declared. Each
    // ref pattern is consumed cyclically in lock-step with those names.
    let n = name_count;
    if n > MAX_EXPANSION {
        return Err(format!("expansion too large: {n}"));
    }

    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let expanded_name = expand_name_at(&name_pattern, name_count, i);

        let expanded_refs: Vec<GlyphRef> = parsed_refs
            .iter()
            .map(|(segs, offset, negated)| GlyphRef {
                name: expand_segments_at(segs, i),
                offset: *offset,
                negated: *negated,
            })
            .collect();

        if expanded_refs.is_empty() {
            continue;
        }

        items.push(DocumentItem::Glyph {
            name: expanded_name,
            body: GlyphBody { refs: expanded_refs, ..GlyphBody::new() },
        });
    }

    Ok(items)
}

// ---------------------------------------------------------------------------
// DocLine — the new ground truth for the editor
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum DocLine {
    Text(String),
    Grid(PixelGrid),
}

#[cfg(any(feature = "editor", test))]
impl DocLine {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            DocLine::Text(s) => Some(s),
            DocLine::Grid(_) => None,
        }
    }

    pub fn as_text_mut(&mut self) -> Option<&mut String> {
        match self {
            DocLine::Text(s) => Some(s),
            DocLine::Grid(_) => None,
        }
    }

    #[cfg(test)]
    pub fn as_grid(&self) -> Option<&PixelGrid> {
        match self {
            DocLine::Grid(g) => Some(g),
            DocLine::Text(_) => None,
        }
    }

    pub fn char_len(&self) -> usize {
        match self {
            DocLine::Text(s) => s.chars().count(),
            DocLine::Grid(_) => 0,
        }
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b) * b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_ref(name: &str) -> GlyphRef {
        GlyphRef {
            name: name.to_string(),
            offset: None,
            negated: false,
        }
    }

    #[test]
    fn collect_name_parts_decodes_empty_alternative() {
        let mut doc = Document::new("test.unf".into());
        doc.items.push(DocumentItem::NameParts {
            name: "$part".to_string(),
            values: vec!["``|a".to_string()],
        });

        let parts = collect_name_parts(&[&doc]);
        assert_eq!(
            parts.get("$part"),
            Some(&vec![String::new(), "a".to_string()]),
        );
    }

    #[test]
    fn collect_name_parts_preserves_repeat_that_exceeds_cumulative_limit() {
        let mut doc = Document::new("test.unf".into());
        let oversized = format!("b*{}", MAX_EXPANSION);
        doc.items.push(DocumentItem::NameParts {
            name: "$part".to_string(),
            values: vec!["a".to_string(), oversized.clone()],
        });

        let parts = collect_name_parts(&[&doc]);
        assert_eq!(
            parts.get("$part"),
            Some(&vec!["a".to_string(), oversized]),
        );
    }

    #[test]
    fn expand_name_pattern_expands_codepoint_ranges() {
        assert_eq!(
            expand_name_pattern("U+2800..2802").unwrap().into_vec(),
            vec![
                "U+2800".to_string(),
                "U+2801".to_string(),
                "U+2802".to_string(),
            ],
        );
        assert_eq!(
            expand_name_pattern("u+00fe..0100").unwrap().into_vec(),
            vec![
                "U+00FE".to_string(),
                "U+00FF".to_string(),
                "U+0100".to_string(),
            ],
        );
    }

    #[test]
    fn expand_name_pattern_rejects_invalid_or_oversized_ranges() {
        assert!(matches!(
            expand_name_pattern("U+2802..2800"),
            Err(NamePatternError::Syntax(_)),
        ));
        assert!(matches!(
            expand_name_pattern("U+00000000..FFFFFFFF"),
            Err(NamePatternError::TooManyExpansions(_)),
        ));
    }

    #[test]
    fn expand_name_pattern_rejects_oversized_repeat_before_materializing_it() {
        let pattern = format!("(name*{})", MAX_EXPANSION + 1);
        assert!(matches!(
            expand_name_pattern(&pattern),
            Err(NamePatternError::TooManyExpansions(_)),
        ));
    }

    #[test]
    fn expand_glyph_block_rejects_zero_repeat_without_panicking() {
        let result = expand_glyph_block(
            &GlyphName("glyph*0".to_string()),
            &[pattern_ref("base")],
        );

        assert!(result.is_err());
    }

    #[test]
    fn expand_glyph_block_rejects_overflowing_codepoint_range() {
        let result = expand_glyph_block(
            &GlyphName("U+00000000..FFFFFFFF".to_string()),
            &[pattern_ref("base")],
        );

        assert!(result.is_err());
    }

    #[test]
    fn expand_glyph_block_expands_lowercase_codepoint_range() {
        let items = expand_glyph_block(
            &GlyphName("u+2800..2801".to_string()),
            &[pattern_ref("base")],
        )
        .unwrap();
        let names: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, .. } => name.display(),
                _ => unreachable!(),
            })
            .collect();

        assert_eq!(names, vec!["U+2800".to_string(), "U+2801".to_string()]);
    }

    #[test]
    fn glyph_name_count_drives_ref_pattern_expansion() {
        let items = expand_glyph_block(
            &GlyphName("out-(a|b)".to_string()),
            &[pattern_ref("dep-(1|2|3|4)")],
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        let expanded: Vec<(String, String)> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, body } => {
                    (name.display(), body.refs[0].name.clone())
                }
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            expanded,
            vec![
                ("out-a".to_string(), "dep-1".to_string()),
                ("out-b".to_string(), "dep-2".to_string()),
            ],
        );
    }

    #[test]
    fn glyph_block_uses_lcm_for_independent_alternation_groups() {
        let items = expand_glyph_block(
            &GlyphName("out-(a|b)-(1|2|3)".to_string()),
            &[pattern_ref("base")],
        )
        .unwrap();

        let names: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                DocumentItem::Glyph { name, .. } => name.display(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "out-a-1".to_string(),
                "out-b-2".to_string(),
                "out-a-3".to_string(),
                "out-b-1".to_string(),
                "out-a-2".to_string(),
                "out-b-3".to_string(),
            ],
        );
    }
}
