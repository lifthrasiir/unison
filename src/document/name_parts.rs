//! `name-parts` as the data model reads them: the unqualified bindings, the
//! per-slice ones layered over them, and expanding one glyph block's names.

use std::collections::HashMap;

use super::{
    ComposeItem, Document, DocumentItem, GlyphBody, GlyphName, MAX_EXPANSION,
    find_invalid_inline_ranges, parse_glyph_name, split_top_level_pipes,
};
use crate::pattern::{NamePartsMap, NamePattern, substitute_name_parts};

/// The unqualified name parts: what every context that is not scoped to a
/// slice — a glyph name, a `ref` target, a `remap` operand — substitutes with.
pub fn collect_name_parts(docs: &[&Document]) -> NamePartsMap {
    let mut map = NamePartsMap::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::NameParts {
                slices,
                name,
                values,
                ..
            } = item
                && slices.is_empty()
            {
                let resolved = resolve_name_part_values(values, &map);
                map.insert(name.clone(), resolved);
            }
        }
    }
    map
}

/// Name parts as seen from each slice.
///
/// `name-parts wide : $half = ` `` ` / `name-parts narrow : $half = -half`
/// binds one name to a different value per slice, which is what lets a
/// slice-varying glyph name be written once:
///
/// ```text
/// map wide|narrow : ⁂ = triple-star($half)
/// ```
///
/// The slices of that qualifier are an *outer loop*: the line is stated once
/// per slice, each time with that slice's bindings in force, and only then does
/// the ordinary cyclic name expansion run. Folding the slices into the
/// expansion as one more alternation group would zip them against the codepoint
/// list instead, which is not what the line says.
///
/// A name is bound either unqualified or per slice, never both — an
/// unqualified binding a slice overrode would be a precedence rule, and
/// [`crate::faces`] has none. [`crate::issues`] reports the violation; here the
/// scoped binding simply wins for its own slice.
#[derive(Clone, Debug, Default)]
pub struct SliceNameParts {
    base: NamePartsMap,
    /// Per slice, the base map with that slice's own bindings applied. Only
    /// slices that bind something are in here.
    per_slice: HashMap<String, NamePartsMap>,
}

impl SliceNameParts {
    /// Built on top of an already-computed unqualified map, since every
    /// consumer of the expansion has one. Nothing is cloned when the source
    /// binds nothing per slice.
    pub fn with_base(docs: &[&Document], base: NamePartsMap) -> Self {
        let mut per_slice: HashMap<String, NamePartsMap> = HashMap::new();
        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::NameParts {
                    slices,
                    name,
                    values,
                    ..
                } = item
                {
                    for slice in slices {
                        let map = per_slice
                            .entry(slice.clone())
                            .or_insert_with(|| base.clone());
                        // Resolved against the *base* parts: a scoped binding
                        // is one value, not a place to build a list up from
                        // other scoped ones.
                        let resolved = resolve_name_part_values(values, &base);
                        map.insert(name.clone(), resolved);
                    }
                }
            }
        }
        Self { base, per_slice }
    }

    /// The bindings in force inside `slice`, falling back to the unqualified
    /// ones. `None` is the base slice.
    pub fn for_slice(&self, slice: Option<&str>) -> &NamePartsMap {
        match slice.and_then(|s| self.per_slice.get(s)) {
            Some(map) => map,
            None => &self.base,
        }
    }

    /// Whether any slice binds `name` (`$`-prefixed), for diagnostics that want
    /// to tell "undefined" from "defined, but not here".
    pub fn is_slice_scoped(&self, name: &str) -> bool {
        !self.base.contains_key(name) && self.per_slice.values().any(|m| m.contains_key(name))
    }
}

/// Expand a glyph item whose name, `ref` targets and/or IDC components carry
/// alternation/range patterns, directly from its in-memory
/// `GlyphName`/`GlyphBody` (no serialize/reparse round-trip through `.unf`
/// text). `body`'s names are expected to have had their `$name-parts`
/// substituted already; only the pattern expansion happens here.
///
/// The block's name decides *how many* glyphs are declared — one per expanded
/// name, whatever the block holds. Everything else the block states is the
/// same for all of them: a `ref` or IDC component is a pattern consumed in
/// lock-step with the name, and every other field — the pixel grid, the box,
/// the flags — is shared verbatim, because there is only one of it written.
/// So a pattern glyph stating just a box is the pattern form of `glyph blank
/// W H`, and a block that states nothing at all expands to glyphs with no
/// content, which the ordinary contentless-glyph diagnostics report per
/// expanded name (there is no rule of its own about patterns here).
///
/// What an IDC line *stands for* is not expanded here at all: the split is
/// solved per glyph from the boxes that glyph's own parts declare, which is
/// [`crate::compose`]'s business and happens downstream.
pub fn expand_glyph_block(name: &GlyphName, body: &GlyphBody) -> Result<Vec<DocumentItem>, String> {
    let name_pattern = NamePattern::parse(&name.display()).map_err(|e| e.to_string())?;

    // Each ref is reduced to a pattern once, so that expanding a block covering
    // a whole CJK range does not re-parse the same pattern string per glyph.
    let mut ref_patterns: Vec<NamePattern> = Vec::new();
    for r in &body.refs {
        ref_patterns.push(NamePattern::parse_segments(&r.name).map_err(|e| e.to_string())?);
    }

    // The same, per IDC line: every component is a pattern of its own, and the
    // gaps between them are the line's whatever it expands to.
    let mut compose_patterns: Vec<Vec<NamePattern>> = Vec::new();
    for c in &body.compose {
        let mut patterns = Vec::new();
        for item in &c.items {
            if let ComposeItem::Part { name, .. } = item {
                patterns.push(NamePattern::parse_segments(name).map_err(|e| e.to_string())?);
            }
        }
        compose_patterns.push(patterns);
    }

    // The glyph-name pattern determines how many glyphs are declared. Each
    // ref pattern is consumed cyclically in lock-step with those names.
    let n = name_pattern.len();

    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let expanded_name = parse_glyph_name(&name_pattern.get(i));

        // The one body, with the two fields the expansion rewrites replaced.
        // `raw_name` and `comment` are the header line's own — a name that has
        // been expanded is no longer the one that was written, and nothing
        // downstream serializes an expansion back out.
        let mut expanded_body = body.clone();
        expanded_body.raw_name = None;
        expanded_body.comment = None;
        for (r, pattern) in expanded_body.refs.iter_mut().zip(&ref_patterns) {
            r.name = pattern.get(i);
            r.comment = None;
        }
        for (c, patterns) in expanded_body.compose.iter_mut().zip(&compose_patterns) {
            let mut patterns = patterns.iter();
            for item in &mut c.items {
                if let ComposeItem::Part { name, .. } = item {
                    *name = patterns.next().expect("one pattern per part").get(i);
                }
            }
            c.comment = None;
        }

        items.push(DocumentItem::Glyph {
            name: expanded_name,
            body: expanded_body,
        });
    }

    Ok(items)
}

// ---------------------------------------------------------------------------
// DocLine — the new ground truth for the editor

/// Decode one `name-parts` right-hand side against the parts defined so far.
///
/// Every value token is a name pattern in its own right, expanded exactly as a
/// glyph name would be: `$ref`s and inline ranges are substituted (including
/// inside a group), then alternation groups and `*N` repeats expand. So
/// `name-parts $foo = bar($1..3)` binds the same three values as
/// `name-parts $foo = bar1 bar2 bar3`, and the tokens of a line concatenate in
/// order. `` `` `` (and the empty token) stands for the empty string.
///
/// A whole binding is capped at [`MAX_EXPANSION`] values like any other
/// expansion — over the cap, or on a malformed pattern, the tokens are kept
/// verbatim and [`crate::issues`] reports the error against the line.
pub(crate) fn resolve_name_part_values(values: &[String], defined: &NamePartsMap) -> Vec<String> {
    try_resolve_name_part_values(values, defined).unwrap_or_else(|_| values.to_vec())
}

/// [`resolve_name_part_values`], reporting why a binding does not expand.
pub(crate) fn try_resolve_name_part_values(
    values: &[String],
    defined: &NamePartsMap,
) -> Result<Vec<String>, String> {
    let mut resolved: Vec<String> = Vec::new();
    let push = |resolved: &mut Vec<String>, names: Vec<String>| {
        let total = resolved.len() + names.len();
        if total > MAX_EXPANSION {
            return Err(format!(
                "`name-parts` expands to {total} values or more (max {MAX_EXPANSION})"
            ));
        }
        resolved.extend(names);
        Ok(())
    };
    for token in values {
        // A bare `$ref` is spliced as it stands: its values are already
        // expanded, and round-tripping them through the pattern parser would
        // only give the characters in them a second meaning.
        if let Some(referenced) = defined.get(token.as_str()) {
            push(&mut resolved, referenced.clone())?;
            continue;
        }
        for part in split_top_level_pipes(token) {
            // A lone `` `` `` is already the empty token by the time the
            // tokenizer is done; it survives literally only when glued to more
            // text (`` ``|a ``).
            if part.is_empty() || part == "``" {
                push(&mut resolved, vec![String::new()])?;
                continue;
            }
            // An oversized or reversed range expands to nothing rather than
            // failing to parse, so it is checked before the pattern is.
            if let Some(bad) = find_invalid_inline_ranges(part).into_iter().next() {
                return Err(format!(
                    "invalid inline range '{bad}' (end < start or too large)"
                ));
            }
            let substituted = substitute_name_parts(part, defined);
            let pattern = NamePattern::parse_element(&substituted).map_err(|e| e.to_string())?;
            push(&mut resolved, pattern.into_vec())?;
        }
    }
    Ok(resolved)
}
