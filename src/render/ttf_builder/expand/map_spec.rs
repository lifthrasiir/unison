//! The character specs of `map` lines: ranges, pipe lists and patterns, and the codepoints and pairs they expand to.

use std::sync::{Arc, Mutex, OnceLock};

use super::*;
use crate::pattern::{capture_groups, substitute_captures};

/// A `map` line's character spec, parsed once for every pass that reads it.
///
/// The spec is a pure function of the text it is written as, and a range spec
/// is tens of thousands of code points wide: the han slice lines write three
/// distinct specs between them, and the expansion, each face's cmap and three
/// separate validation checks each used to parse every one of them again. The
/// memo in [`map_char_pattern`] is what makes that one parse per spec instead.
pub(super) struct MapCharSpec {
    pattern: NamePattern,
    /// `(index into the expansion, code point)` for every entry of the spec
    /// that names one — see [`WideMapRows::rows`]. Kept here rather than
    /// recomputed per caller because building it means expanding the whole
    /// pattern, which is the cost the memo exists to pay once.
    rows: Vec<(usize, u32)>,
    /// The groups the spec captures, for a target's `$-N`.
    captures: Vec<Vec<String>>,
    /// Per capture group, whether every value in it is a plain literal — which
    /// is what lets [`AltTarget::Capture`](super::alternatives::AltTarget::Capture) read a value straight out of the
    /// group instead of splicing the group into a string and parsing it back.
    plain: Vec<bool>,
    /// Per capture group, the values the expansion really produces — the
    /// group's entries at the indices that name a code point — and what lets
    /// [`MapAlternativeIndex`] answer a name without enumerating them.
    ///
    /// `None` for a group that does not span the whole expansion: a value there
    /// stands for several indices at once, so its presence no longer settles
    /// whether the name is produced. Also `None` throughout for a spec too
    /// narrow to be worth indexing, which is every spec the memo does not keep.
    pub(super) produced: Vec<Option<HashSet<String>>>,
}

/// The character spec of a `map`, when it is written as a *pattern*: the code
/// point spellings it stands for, and the groups it captures.
///
/// A parenthesized list is the only spelling on this side that captures, which
/// is what makes `map (ㅠ|ㅡ) = hangul-($-1)` different from the bare
/// `map ㅠ|ㅡ = ...` beside it: the parentheses mark the group, and only a
/// marked group can be named again. Inline ranges are expanded here (with no
/// bindings, since a `map`'s left-hand side has never taken a `$name-part`),
/// so `U+($#4e00..9fff)` is one line over a whole block.
///
/// `None` for every spelling that is not one — a lone `(` is a character to be
/// mapped like any other, and so is anything whose group does not parse.
fn map_char_pattern(char_repr: &str) -> Option<Arc<MapCharSpec>> {
    if !char_repr.contains('(') {
        return None;
    }
    // Wide specs only, and a handful of them: what is kept is a whole
    // expansion, and a narrow one costs less to parse than to look up.
    const MEMO_MIN_WIDTH: usize = 256;
    const MEMO_MAX_ENTRIES: usize = 32;
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<MapCharSpec>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(spec) = crate::parallel::lock_memo(cache).get(char_repr) {
        return Some(spec.clone());
    }

    let text = substitute_name_parts(char_repr, &NamePartsMap::default());
    let captures = capture_groups(&text);
    if captures.is_empty() {
        return None;
    }
    let pattern = NamePattern::parse_element(&text).ok()?;
    let rows: Vec<(usize, u32)> = (0..pattern.len())
        .filter_map(|i| parse_map_char(&pattern.get(i)).map(|cp| (i, cp)))
        .collect();
    let plain = captures
        .iter()
        .map(|g| !g.is_empty() && !g.iter().any(|v| v.contains(['(', ')', '|', '*'])))
        .collect();
    let wide = pattern.len() >= MEMO_MIN_WIDTH;
    let produced = captures
        .iter()
        .map(|group| {
            (wide && group.len() == pattern.len())
                .then(|| rows.iter().map(|&(i, _)| group[i].clone()).collect())
        })
        .collect();
    let spec = Arc::new(MapCharSpec {
        pattern,
        rows,
        captures,
        plain,
        produced,
    });
    if wide {
        let mut cache = crate::parallel::lock_memo(cache);
        if cache.len() < MEMO_MAX_ENTRIES {
            cache.insert(char_repr.to_string(), spec.clone());
        }
    }
    Some(spec)
}

/// The groups a `map`'s character spec captures — [`map_char_pattern`]'s other
/// half, for the checks that read the line rather than expand it.
pub(crate) fn map_char_captures(char_repr: &str, selector: Option<&str>) -> Vec<Vec<String>> {
    map_char_pattern(char_repr)
        .into_iter()
        .chain(selector.and_then(map_char_pattern))
        .flat_map(|spec| spec.captures.clone())
        .collect()
}

/// The codepoints one half of a `map` names: a single character, a `U+X..Y`
/// range, or a top-level pipe list. Invalid and unparsable entries are dropped.
pub(crate) fn expand_map_codepoints(token: &str) -> Vec<u32> {
    if let Some(spec) = map_char_pattern(token) {
        // The memo already holds the expansion; rebuilding every name to read
        // its code point back out is what this used to cost.
        return spec.rows.iter().map(|&(_, cp)| cp).collect();
    }

    if let Some(hex_rest) = token
        .strip_prefix("U+")
        .or_else(|| token.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..")
        && let (Ok(start), Ok(end)) = (
            u32::from_str_radix(start_hex, 16),
            u32::from_str_radix(end_hex, 16),
        )
    {
        if end < start || u64::from(end) - u64::from(start) + 1 > MAX_EXPANSION as u64 {
            return vec![];
        }
        return (start..=end)
            .filter(|cp| char::from_u32(*cp).is_some())
            .collect();
    }

    if has_top_level_pipe(token) {
        let parts: Vec<&str> = split_top_level_pipes(token)
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= 2 {
            return parts.iter().filter_map(|s| parse_map_char(s)).collect();
        }
    }

    parse_map_char(token).into_iter().collect()
}

pub(crate) fn parse_map_char(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        let mut chars = s.chars();
        let c = chars.next()?;
        if chars.next().is_none() {
            Some(c as u32)
        } else {
            None
        }
    }
}

/// Codepoint/generated-glyph-name pairs for a `map generate CHAR [= GLYPH]`.
///
/// Without an explicit name each codepoint generates `uniXXXX`; with one, the
/// name is a pattern expanded in lock-step with `char_repr`, exactly as a plain
/// `map`'s target is.
pub fn decomposed_map_pairs(char_repr: &str, glyph: Option<&str>) -> Vec<(u32, String)> {
    expand_map_pairs(char_repr, glyph.unwrap_or(""))
        .into_iter()
        .map(|(cp, name)| {
            let name = if name.is_empty() {
                format!("uni{cp:04X}")
            } else {
                name
            };
            (cp, name)
        })
        .collect()
}

pub(crate) fn expand_map_pairs(char_repr: &str, glyph: &str) -> Vec<(u32, String)> {
    // Written as a pattern, which is the one spelling that binds `$-N` for the
    // target beside it.
    if let Some(spec) = map_char_pattern(char_repr) {
        let glyph = substitute_captures(glyph, &spec.captures);
        let count = spec.pattern.len();
        let names = expand_glyph_pattern(&glyph, count);
        return (0..count)
            .filter_map(|i| {
                parse_map_char(&spec.pattern.get(i)).map(|cp| (cp, names[i % names.len()].clone()))
            })
            .collect();
    }

    // Range: U+XXXX..YYYY or u+XXXX..YYYY
    if let Some(hex_rest) = char_repr
        .strip_prefix("U+")
        .or_else(|| char_repr.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..")
        && let (Ok(start), Ok(end)) = (
            u32::from_str_radix(start_hex, 16),
            u32::from_str_radix(end_hex, 16),
        )
    {
        if end < start {
            return vec![];
        }
        let count64 = u64::from(end) - u64::from(start) + 1;
        if count64 > MAX_EXPANSION as u64 {
            return vec![];
        }
        let count = count64 as usize;
        let glyph_names = expand_glyph_pattern(glyph, count);
        return (0..count)
            .zip(glyph_names.iter().cycle())
            .filter_map(|(i, name)| {
                let cp = start + i as u32;
                char::from_u32(cp).map(|_| (cp, name.clone()))
            })
            .collect();
    }

    // Multi-char with pipe (depth-aware)
    // Filter empty parts so a bare "|" (the pipe character) falls through to single-char.
    if has_top_level_pipe(char_repr) {
        let chars: Vec<&str> = split_top_level_pipes(char_repr)
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if chars.len() >= 2 {
            let glyph_names = if has_top_level_pipe(glyph) {
                let glyphs = split_top_level_pipes(glyph);
                if glyphs.len() == chars.len() {
                    glyphs.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                } else {
                    expand_glyph_pattern(glyph, chars.len())
                }
            } else {
                expand_glyph_pattern(glyph, chars.len())
            };
            return chars
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    parse_map_char(c).map(|cp| (cp, glyph_names[i % glyph_names.len()].clone()))
                })
                .collect();
        }
    }

    // Single char — still expand the glyph pattern
    if let Some(cp) = parse_map_char(char_repr) {
        let names = expand_glyph_pattern(glyph, 1);
        vec![(
            cp,
            names
                .into_iter()
                .next()
                .unwrap_or_else(|| glyph.to_string()),
        )]
    } else {
        vec![]
    }
}

pub(crate) fn expand_glyph_pattern(pattern: &str, count: usize) -> Vec<String> {
    match NamePattern::parse_element(pattern) {
        Ok(expanded) => (0..count).map(|i| expanded.get(i)).collect(),
        Err(_) => vec![pattern.to_string(); count],
    }
}

/// Why a `map BASE SELECTOR = GLYPH` line expands to nothing.
///
/// Separate from [`Diagnostic`] so the expansion stays a pure function that
/// [`crate::issues`] and the builder can each phrase in their own terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UvsExpandError {
    /// Both halves list more than one codepoint. Which base goes with which
    /// selector would have to be either a zip or a cross product, and neither
    /// is more obviously right than the other, so the line has to say.
    BothVary,
    /// A half expanded to no valid codepoint at all.
    Empty { selector_half: bool },
    /// The second half is not a variation selector, or the first half is one.
    NotASelector { cp: u32, selector_half: bool },
}

/// Expand `map BASE SELECTOR = GLYPH` into `(base, selector, glyph)` triples.
///
/// Either half may be a range or a pipe list, but not both: one position
/// varies and the other is held fixed, which covers the two shapes that occur —
/// a fixed selector over many bases (keycaps) and a fixed base over many
/// selectors (ideographic variants) — without having to define a cross product
/// nobody asked for. The glyph pattern is expanded in lock-step with whichever
/// half varies, exactly as a plain `map`'s target is.
///
/// Deliberately not written in terms of [`expand_map_pairs`]: that one pairs
/// glyph names with codepoints *positionally, including the invalid ones*, so
/// the alignment of a malformed pipe list is part of its observable behaviour.
/// This one answers a different question and drops invalid codepoints outright.
pub(crate) fn expand_uvs_map_triples(
    char_repr: &str,
    selector: &str,
    glyph: &str,
) -> Result<Vec<(u32, u32, String)>, UvsExpandError> {
    // A single code point is parsed as written, scalar value or not.
    let mut bases = expand_map_codepoints(char_repr);
    bases.retain(|&cp| char::from_u32(cp).is_some());
    let selectors = expand_map_codepoints(selector);
    // The groups of both halves, numbered in written order — the base's first,
    // since that is how the line reads.
    let captures = map_char_captures(char_repr, Some(selector));
    let glyph = &substitute_captures(glyph, &captures);
    if bases.is_empty() {
        return Err(UvsExpandError::Empty {
            selector_half: false,
        });
    }
    if selectors.is_empty() {
        return Err(UvsExpandError::Empty {
            selector_half: true,
        });
    }
    if bases.len() > 1 && selectors.len() > 1 {
        return Err(UvsExpandError::BothVary);
    }
    if let Some(&cp) = bases
        .iter()
        .find(|cp| crate::ucd::is_variation_selector(**cp))
    {
        return Err(UvsExpandError::NotASelector {
            cp,
            selector_half: false,
        });
    }
    if let Some(&cp) = selectors
        .iter()
        .find(|cp| !crate::ucd::is_variation_selector(**cp))
    {
        return Err(UvsExpandError::NotASelector {
            cp,
            selector_half: true,
        });
    }

    let count = bases.len().max(selectors.len());
    let names = expand_glyph_pattern(glyph, count);
    Ok((0..count)
        .map(|i| {
            (
                bases[i % bases.len()],
                selectors[i % selectors.len()],
                names[i % names.len()].clone(),
            )
        })
        .collect())
}

/// One `map` line's character spec, expanded once for the whole line rather
/// than once per alternative target.
///
/// Every alternative of a line ranges over the same characters, so the spec —
/// a pattern in its own right, and one that can be tens of thousands of code
/// points wide — is parsed and expanded once here and each alternative is read
/// off it by index. Expanding it per alternative is what made the seven han
/// slice lines, three ranges wide and nine alternatives deep, cost their width
/// nine times over on every rebuild.
pub(super) struct WideMapRows {
    /// A range line's own rows; a pattern line reads the spec's, which the
    /// memo already holds. See [`WideMapRows::rows`].
    range_rows: Vec<(usize, u32)>,
    /// The parsed spec, where the line writes one — a range line captures
    /// nothing and needs none.
    pub(super) spec: Option<Arc<MapCharSpec>>,
}

impl WideMapRows {
    /// `(index into the expansion, code point)`. The index is what a target is
    /// read at and is *not* the row number: a spec entry naming no code point
    /// drops the row and keeps the index, exactly as [`expand_map_pairs`] does.
    pub(super) fn rows(&self) -> &[(usize, u32)] {
        self.spec.as_ref().map_or(&self.range_rows, |s| &s.rows)
    }

    pub(super) fn captures(&self) -> &[Vec<String>] {
        self.spec.as_ref().map_or(&[], |s| &s.captures)
    }

    /// Whether capture group `idx` holds plain literals only.
    pub(super) fn plain(&self, idx: usize) -> bool {
        self.spec
            .as_ref()
            .and_then(|s| s.plain.get(idx))
            .copied()
            .unwrap_or(false)
    }
}

/// The rows of a `map` line wide enough to be worth expanding once for the
/// whole line: the pattern and the `U+X..Y` range forms.
///
/// The pipe and single-character forms name a handful of characters each, and
/// their target pairing has rules of its own ([`expand_map_pairs`] matches a
/// piped target against a piped spec element by element), so they keep the
/// straightforward path rather than restating those rules here.
pub(super) fn wide_map_rows(char_repr: &str) -> Option<WideMapRows> {
    if let Some(spec) = map_char_pattern(char_repr) {
        return Some(WideMapRows {
            range_rows: Vec::new(),
            spec: Some(spec),
        });
    }

    let hex_rest = char_repr
        .strip_prefix("U+")
        .or_else(|| char_repr.strip_prefix("u+"))?;
    let (start_hex, end_hex) = hex_rest.split_once("..")?;
    let (Ok(start), Ok(end)) = (
        u32::from_str_radix(start_hex, 16),
        u32::from_str_radix(end_hex, 16),
    ) else {
        return None;
    };
    // Reversed and over-wide ranges expand to nothing, like every other
    // spelling `expand_map_pairs` cannot make sense of; the line then keeps its
    // shape below.
    let count = match u64::from(end).checked_sub(u64::from(start)) {
        Some(span) if span < MAX_EXPANSION as u64 => (span + 1) as usize,
        _ => 0,
    };
    Some(WideMapRows {
        range_rows: (0..count)
            .map(|i| (i, start + i as u32))
            .filter(|(_, cp)| char::from_u32(*cp).is_some())
            .collect(),
        spec: None,
    })
}
