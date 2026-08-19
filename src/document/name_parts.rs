//! `name-parts` as the data model reads them: the unqualified bindings, the
//! per-slice ones layered over them, and expanding one glyph block's names.

use std::collections::HashMap;

use super::{
    ComposeItem, Document, DocumentItem, GlyphBody, GlyphCompose, GlyphName, GlyphRef,
    MAX_EXPANSION, find_invalid_inline_ranges, parse_glyph_name, split_top_level_pipes,
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

/// Expand a ref-only glyph item (`glyph NAME` + `ref`/IDC lines, no pixel data)
/// whose name, ref targets and/or IDC components carry alternation/range
/// patterns, directly from its in-memory `GlyphName`/`GlyphRef`s (no
/// serialize/reparse round-trip through `.unf` text).
///
/// Mirrors the historical behavior exactly: pixel data is not meaningful
/// for a batch of expanded ref-composites, so expanded items always come
/// out as `pixels: None` — this function is only ever called on an
/// already-pattern-named item, and `.unf` content never combines a
/// pattern name with pixel data on the same glyph (patterns are only
/// used for ref/composite batches).
///
/// An IDC line's components expand exactly as a `ref` target does, in lock-step
/// with the block's name. What the line *stands for* is not expanded here at
/// all: the split is solved per glyph from the boxes that glyph's own parts
/// declare, which is [`crate::compose`]'s business and happens downstream.
pub fn expand_glyph_block(
    name: &GlyphName,
    refs: &[GlyphRef],
    compose: &[GlyphCompose],
    scale: u8,
) -> Result<Vec<DocumentItem>, String> {
    let name_pattern = NamePattern::parse(&name.display()).map_err(|e| e.to_string())?;

    // Each ref is reduced to a template once, with the two fields the expansion
    // replaces already empty. `..r.clone()` per expanded name instead copied the
    // pattern string into every one of them only for `name` to overwrite it,
    // which over a block covering a whole CJK range is one wasted allocation per
    // glyph declared.
    let mut parsed_refs: Vec<(NamePattern, GlyphRef)> = Vec::new();
    for r in refs {
        let pattern = NamePattern::parse_segments(&r.name).map_err(|e| e.to_string())?;
        let template = GlyphRef {
            name: String::new(),
            comment: None,
            ..r.clone()
        };
        parsed_refs.push((pattern, template));
    }

    // The same, per IDC line: every component is a pattern of its own, and the
    // gaps between them are the line's whatever it expands to.
    let mut parsed_compose: Vec<(Vec<NamePattern>, &GlyphCompose)> = Vec::new();
    for c in compose {
        let mut patterns = Vec::new();
        for item in &c.items {
            if let ComposeItem::Part { name, .. } = item {
                patterns.push(NamePattern::parse_segments(name).map_err(|e| e.to_string())?);
            }
        }
        parsed_compose.push((patterns, c));
    }

    // The glyph-name pattern determines how many glyphs are declared. Each
    // ref pattern is consumed cyclically in lock-step with those names.
    let n = name_pattern.len();

    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let expanded_name = parse_glyph_name(&name_pattern.get(i));

        let expanded_refs: Vec<GlyphRef> = parsed_refs
            .iter()
            .map(|(pattern, template)| GlyphRef {
                name: pattern.get(i),
                ..template.clone()
            })
            .collect();

        let expanded_compose: Vec<GlyphCompose> = parsed_compose
            .iter()
            .map(|(patterns, c)| {
                let mut patterns = patterns.iter();
                GlyphCompose {
                    op: c.op,
                    items: c
                        .items
                        .iter()
                        .map(|item| match item {
                            ComposeItem::Gap(gap) => ComposeItem::Gap(*gap),
                            ComposeItem::Part { raw_name, .. } => ComposeItem::Part {
                                name: patterns.next().expect("one pattern per part").get(i),
                                raw_name: raw_name.clone(),
                            },
                        })
                        .collect(),
                    if_exists: c.if_exists,
                    comment: None,
                }
            })
            .collect();

        if expanded_refs.is_empty() && expanded_compose.is_empty() {
            continue;
        }

        items.push(DocumentItem::Glyph {
            name: expanded_name,
            body: GlyphBody {
                refs: expanded_refs,
                compose: expanded_compose,
                scale,
                ..GlyphBody::new()
            },
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
