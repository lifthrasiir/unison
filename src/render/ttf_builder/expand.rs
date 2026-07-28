//! Name-pattern expansion of document items, plus the on-demand and
//! decomposed-map glyph items synthesized on top of them.

use super::*;

/// One item of the expanded item list, tagged with the source item it came
/// from. Synthesized items (on-demand glyphs, `map <decomposable>` composites)
/// carry the origin of whatever asked for them.
pub(crate) struct ExpandedItem {
    pub item: DocumentItem,
    pub origin: Option<ItemRef>,
}

/// Result of expanding a document set, including everything the expansion
/// could not make sense of. Before this carried diagnostics the expansion
/// simply `continue`d past bad input, so problems that are only detectable
/// here — an unmapped decomposition component, an unresolvable on-demand
/// glyph name — never reached the user.
pub(crate) struct Expansion {
    pub items: Vec<ExpandedItem>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Expansion {
    pub fn items(&self) -> impl Iterator<Item = &DocumentItem> {
        self.items.iter().map(|e| &e.item)
    }
}

/// How a glyph name was referenced, so a name that resolves to nothing can be
/// reported in the terms the author wrote it.
#[derive(Clone, PartialEq, Eq)]
enum RefKind {
    Ref,
    /// Carries the `char_repr` the map was written with, which is what the
    /// author recognizes the line by.
    Map(String),
    Remap,
}

/// Collect all items from `docs` with name-part patterns substituted and
/// expanded, and `map-decomposed` directives turned into synthesized
/// composite glyphs + `map` entries via NFD decomposition.
pub(crate) fn collect_expanded_items(docs: &[&Document], name_parts: &NamePartsMap) -> Vec<DocumentItem> {
    expand_documents(docs, name_parts)
        .items
        .into_iter()
        .map(|e| e.item)
        .collect()
}

pub(crate) fn expand_documents(docs: &[&Document], name_parts: &NamePartsMap) -> Expansion {
    let mut all_items: Vec<ExpandedItem> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let origin = ItemRef::new(doc_idx, item_idx);
            if let DocumentItem::Glyph { name, body } = item {
                let name_str = substitute_name_parts(&name.display(), name_parts);
                if is_name_pattern(&name_str) {
                    let subst_name = GlyphName(name_str);
                    let subst_refs: Vec<GlyphRef> = body
                        .refs
                        .iter()
                        .map(|r| GlyphRef {
                            comment: None,
                            name: substitute_name_parts(&r.name, name_parts),
                            offset: r.offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: r.visibility,
                        })
                        .collect();
                    match expand_glyph_block(&subst_name, &subst_refs, body.scale) {
                        Ok(expanded) if expanded.is_empty() => {
                            // `expand_glyph_block` only emits a glyph per
                            // expanded name when that name has refs, so a
                            // pattern glyph carrying just a pixel grid used to
                            // vanish from the font without a word.
                            diagnostics.push(Diagnostic::error(
                                origin,
                                format!(
                                    "glyph pattern '{}' defines no glyphs; a pattern glyph \
                                     needs `ref` lines, a pixel grid alone cannot be shared",
                                    subst_name.display(),
                                ),
                            ));
                        }
                        Ok(expanded) => {
                            for mut item in expanded {
                                if let DocumentItem::Glyph { body: ref mut b, .. } = item {
                                    b.pixels = body.pixels.clone();
                                    b.points = body.points.clone();
                                    b.sticky = body.sticky;
                                    b.advance = body.advance;
                                    b.left = body.left;
                                    b.top = body.top;
                                    b.scale = body.scale;
                                }
                                all_items.push(ExpandedItem { item, origin: Some(origin) });
                            }
                        }
                        Err(e) => diagnostics.push(Diagnostic::error(origin, e)),
                    }
                } else {
                    let mut body = body.clone();
                    for gref in &mut body.refs {
                        gref.name = substitute_name_parts(&gref.name, name_parts);
                    }
                    all_items.push(ExpandedItem {
                        item: DocumentItem::Glyph { name: GlyphName(name_str), body },
                        origin: Some(origin),
                    });
                }
            } else if let DocumentItem::Map { char_repr, glyph, .. } = item {
                all_items.push(ExpandedItem {
                    item: DocumentItem::Map {
                        comment: None,
                        char_repr: char_repr.clone(),
                        glyph: substitute_name_parts(glyph, name_parts),
                    },
                    origin: Some(origin),
                });
            } else {
                all_items.push(ExpandedItem { item: item.clone(), origin: Some(origin) });
            }
        }
    }

    // Expanding a `map` is not free (the font has ranges thousands of
    // codepoints wide), and three later steps need the result, so it happens
    // exactly once here.
    //
    // The range form of `expand_map_pairs` filters out non-scalar values but
    // the single/pipe forms cannot, so an out-of-range `map U+FFFFFFFF = g`
    // used to reach the cmap builder unnoticed.
    let mut cp_to_glyph: HashMap<u32, String> = HashMap::new();
    let mut map_targets: Vec<(String, Option<ItemRef>, String)> = Vec::new();
    for e in &all_items {
        let DocumentItem::Map { char_repr, glyph, .. } = &e.item else {
            continue;
        };
        let pairs = expand_map_pairs(char_repr, glyph);
        if pairs.is_empty() {
            diagnostics.push(Diagnostic::error(
                e.origin,
                format!("map has no valid codepoints ('{char_repr}')"),
            ));
            continue;
        }
        let mut reported = false;
        for (cp, target) in pairs {
            if !reported && char::from_u32(cp).is_none() {
                diagnostics.push(Diagnostic::error(
                    e.origin,
                    format!("map 'U+{cp:04X}' is not a valid Unicode scalar value"),
                ));
                reported = true;
            }
            map_targets.push((target.clone(), e.origin, char_repr.clone()));
            cp_to_glyph.entry(cp).or_insert(target);
        }
    }

    expand_decomposed_maps(&mut all_items, &cp_to_glyph, &mut diagnostics);
    inject_on_demand_glyph_items(&mut all_items, map_targets, name_parts, &mut diagnostics);

    Expansion { items: all_items, diagnostics }
}

/// Turn `map <decomposable codepoint>` into a synthesized composite glyph plus
/// a plain `map` to it, reporting the characters that cannot be synthesized.
fn expand_decomposed_maps(
    all_items: &mut Vec<ExpandedItem>,
    cp_to_glyph: &HashMap<u32, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use unicode_normalization::UnicodeNormalization;

    let mut decomposed_items: Vec<ExpandedItem> = Vec::new();
    let pending: Vec<(String, Option<ItemRef>)> = all_items
        .iter()
        .filter_map(|e| match &e.item {
            DocumentItem::MapDecomposed { char_repr, .. } => Some((char_repr.clone(), e.origin)),
            _ => None,
        })
        .collect();

    for (char_repr, origin) in pending {
        let pairs = expand_map_pairs(&char_repr, "");
        if pairs.is_empty() {
            diagnostics.push(Diagnostic::error(
                origin,
                format!("map has no valid codepoints ('{char_repr}')"),
            ));
            continue;
        }

        for (cp, _) in pairs {
            let Some(ch) = char::from_u32(cp) else {
                diagnostics.push(Diagnostic::error(
                    origin,
                    format!("map 'U+{cp:04X}' is not a valid character"),
                ));
                continue;
            };

            let nfd: Vec<char> = ch.nfd().collect();
            if nfd.len() == 1 && nfd[0] == ch {
                diagnostics.push(Diagnostic::error(
                    origin,
                    format!(
                        "map '{ch}' (U+{cp:04X}) has no canonical decomposition; \
                         use `map {ch} = GLYPH` instead",
                    ),
                ));
                continue;
            }

            let missing: Vec<String> = nfd
                .iter()
                .filter(|c| !cp_to_glyph.contains_key(&(**c as u32)))
                .map(|c| format!("U+{:04X}", *c as u32))
                .collect();
            if !missing.is_empty() {
                diagnostics.push(Diagnostic::error(
                    origin,
                    format!(
                        "map '{}' (U+{:04X}) decomposes to unmapped codepoint{} {}",
                        ch,
                        cp,
                        if missing.len() == 1 { "" } else { "s" },
                        missing.join(", "),
                    ),
                ));
                continue;
            }

            let composite_name = format!("uni{cp:04X}");
            let refs: Vec<GlyphRef> = nfd
                .iter()
                .map(|c| GlyphRef {
                    comment: None,
                    name: cp_to_glyph[&(*c as u32)].clone(),
                    offset: None,
                    negated: false,
                    fill: None,
                    visibility: None,
                })
                .collect();

            decomposed_items.push(ExpandedItem {
                item: DocumentItem::Glyph {
                    name: GlyphName(composite_name.clone()),
                    body: GlyphBody { refs, ..GlyphBody::new() },
                },
                origin,
            });
            decomposed_items.push(ExpandedItem {
                item: DocumentItem::Map {
                    comment: None,
                    char_repr: format!("U+{cp:04X}"),
                    glyph: composite_name,
                },
                origin,
            });
        }
    }

    all_items.retain(|e| !matches!(e.item, DocumentItem::MapDecomposed { .. }));
    all_items.extend(decomposed_items);
}

/// Scan `all_items` for on-demand glyph names referenced in refs, maps,
/// and remaps. For each one not already defined as a glyph, append a
/// synthetic `DocumentItem::Glyph` (filled rectangle for WxH, or a
/// color/mono composite when X:mono and X:color both exist).
fn inject_on_demand_glyph_items(
    all_items: &mut Vec<ExpandedItem>,
    map_targets: Vec<(String, Option<ItemRef>, String)>,
    name_parts: &NamePartsMap,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut defined: HashSet<String> = HashSet::new();
    let mut glyph_bodies: HashMap<String, GlyphBody> = HashMap::new();

    for e in all_items.iter() {
        if let DocumentItem::Glyph { name: GlyphName(n), body } = &e.item {
            defined.insert(n.clone());
            glyph_bodies.insert(n.clone(), body.clone());
        }
    }

    // Every mention of an undefined name, so one that turns out not to be an
    // on-demand glyph either is reported at each place the author wrote it —
    // not once for the whole font. Deduped per (site, name) so a pattern that
    // expands to the same missing name repeatedly reports once per line.
    // A glyph with neither a pixel grid nor a ref never enters the resolution
    // cache (see `glyph_cache::seed_cache`), so it is not built and every use
    // of it — cmap entry, composite component, GSUB coverage — is dropped. It
    // is as unusable as an undefined name, and reported the same way.
    let contentless: HashSet<String> = glyph_bodies
        .iter()
        .filter(|(_, body)| body.pixels.is_none() && body.refs.is_empty())
        .map(|(name, _)| name.clone())
        .collect();

    let mut mentions: Vec<(String, Option<ItemRef>, RefKind)> = Vec::new();
    let mut mention_seen: HashSet<(Option<ItemRef>, String)> = HashSet::new();
    let mut consider = |name: &str, origin: Option<ItemRef>, kind: RefKind| {
        let unusable = !defined.contains(name) || contentless.contains(name);
        if unusable && mention_seen.insert((origin, name.to_string())) {
            mentions.push((name.to_string(), origin, kind));
        }
    };

    for e in all_items.iter() {
        match &e.item {
            DocumentItem::Glyph { body, .. } => {
                for r in &body.refs {
                    consider(&r.name, e.origin, RefKind::Ref);
                }
            }
            DocumentItem::Remap { .. } => {
                for token in e.item.remap_operands() {
                    // Remap operands keep their name-part patterns until the
                    // GSUB builder expands them, so checking the written name
                    // says nothing. Expanding with the same helper the builder
                    // uses is what keeps the two from drifting apart: rules
                    // whose glyphs have no id are dropped there without a word.
                    for name in expand_name_element(token, name_parts) {
                        consider(&name, e.origin, RefKind::Remap);
                    }
                }
            }
            _ => {}
        }
    }
    // Map targets were expanded once by the caller: `glyph` is still a pattern
    // on the item, and the cmap builder expands it per codepoint.
    for (target, origin, char_repr) in map_targets {
        consider(&target, origin, RefKind::Map(char_repr));
    }

    // Synthesis is per unique name; reporting is per mention, so the loops
    // are separate.
    let unique: Vec<(String, Option<ItemRef>)> = {
        let mut seen: HashSet<&str> = HashSet::new();
        mentions
            .iter()
            // Contentless names are in `mentions` to be reported, but they are
            // defined, so there is nothing to synthesize for them.
            .filter(|(n, _, _)| !defined.contains(n) && seen.insert(n.as_str()))
            .map(|(n, o, _)| (n.clone(), *o))
            .collect()
    };
    let mut unresolved: HashSet<String> = HashSet::new();

    for (name, origin) in unique {
        use crate::ref_composite::{OnDemandGlyph, detect_on_demand_glyph};
        match detect_on_demand_glyph(&name, |n| defined.contains(n)) {
            Some(OnDemandGlyph::Rect(rect)) => {
                let grid = crate::ref_composite::make_on_demand_grid(&rect);
                all_items.push(ExpandedItem {
                    item: DocumentItem::Glyph {
                        name: GlyphName(name),
                        body: GlyphBody {
                            scale: rect.scale,
                            pixels: Some(grid),
                            inline: true,
                            ..GlyphBody::new()
                        },
                    },
                    origin,
                });
            }
            Some(OnDemandGlyph::ColorMono { mono, color }) => {
                let mono_body = glyph_bodies.get(&mono);
                let color_body = glyph_bodies.get(&color);
                if let (Some(mono_body), Some(color_body)) = (mono_body, color_body) {
                    let mono_s = mono_body.scale.max(1);
                    let color_s = color_body.scale.max(1);
                    let combined_scale =
                        crate::pattern::lcm(mono_s as usize, color_s as usize) as u8;
                    let mono_s = mono_s as i16;
                    let color_s = color_s as i16;
                    let combined_s = combined_scale as i16;

                    let mut refs = Vec::new();
                    for r in &mono_body.refs {
                        let offset = if mono_s == combined_s {
                            r.offset
                        } else {
                            r.offset.map(|(row, col)| (row * combined_s / mono_s, col * combined_s / mono_s))
                        };
                        refs.push(GlyphRef {
                            comment: None,
                            name: r.name.clone(),
                            offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: Some(LayerVisibility::MonoOnly),
                        });
                    }
                    for r in &color_body.refs {
                        let offset = if color_s == combined_s {
                            r.offset
                        } else {
                            r.offset.map(|(row, col)| (row * combined_s / color_s, col * combined_s / color_s))
                        };
                        refs.push(GlyphRef {
                            comment: None,
                            name: r.name.clone(),
                            offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: Some(LayerVisibility::ColorOnly),
                        });
                    }
                    let mut points = Vec::new();
                    points.extend_from_slice(&mono_body.points);
                    points.extend_from_slice(&color_body.points);

                    let pixels = match (&mono_body.pixels, &color_body.pixels) {
                        (Some(mg), Some(cg)) => {
                            let mg2 = if mono_s == combined_s { mg.clone() } else { mg.rescale(mono_s as u8, combined_scale) };
                            let cg2 = if color_s == combined_s { cg.clone() } else { cg.rescale(color_s as u8, combined_scale) };
                            Some(if mg2.width >= cg2.width && mg2.height >= cg2.height { mg2 } else { cg2 })
                        }
                        (None, Some(cg)) => {
                            Some(if color_s == combined_s { cg.clone() } else { cg.rescale(color_s as u8, combined_scale) })
                        }
                        (Some(mg), None) => {
                            Some(if mono_s == combined_s { mg.clone() } else { mg.rescale(mono_s as u8, combined_scale) })
                        }
                        (None, None) => None,
                    };

                    all_items.push(ExpandedItem {
                        item: DocumentItem::Glyph {
                            name: GlyphName(name),
                            body: GlyphBody {
                                refs,
                                points,
                                pixels,
                                scale: combined_scale,
                                advance: mono_body.advance.or(color_body.advance),
                                left: mono_body.left.or(color_body.left),
                                top: mono_body.top.or(color_body.top),
                                ..GlyphBody::new()
                            },
                        },
                        origin,
                    });
                } else {
                    // `X:mono`/`X:color` was recognized but one half is not a
                    // real glyph, so nothing is emitted and every reference to
                    // `X` silently resolves to nothing.
                    let absent = if mono_body.is_none() { &mono } else { &color };
                    diagnostics.push(Diagnostic::error(
                        origin,
                        format!(
                            "color/mono glyph '{name}' cannot be synthesized: \
                             '{absent}' is not defined",
                        ),
                    ));
                }
            }
            None => {
                unresolved.insert(name);
            }
        }
    }

    for (name, origin, kind) in mentions {
        // A name that is defined but contentless gets its own wording:
        // "undefined" would send the author looking for a definition that is
        // right there. `advance`/`left`/`top`/`point` do not make a glyph
        // buildable, so the fix is always to add a pixel grid or a `ref`.
        const EMPTY: &str = "has neither a pixel grid nor a ref, so it is not built";
        let (severity, message) = match (unresolved.contains(&name), contentless.contains(&name), kind) {
            (false, false, _) => continue,
            (true, _, RefKind::Ref) => (Severity::Error, format!("unresolved ref '{name}'")),
            (true, _, RefKind::Map(char_repr)) => (
                Severity::Error,
                format!("map '{char_repr}' targets undefined glyph '{name}'"),
            ),
            (true, _, RefKind::Remap) => (
                Severity::Warning,
                format!("remap references undefined glyph '{name}'"),
            ),
            (false, true, RefKind::Ref) => (
                Severity::Error,
                format!("ref '{name}' {EMPTY}"),
            ),
            (false, true, RefKind::Map(char_repr)) => (
                Severity::Error,
                format!("map '{char_repr}' targets glyph '{name}', which {EMPTY}"),
            ),
            (false, true, RefKind::Remap) => (
                Severity::Error,
                format!("remap references glyph '{name}', which {EMPTY}"),
            ),
        };
        diagnostics.push(Diagnostic::new(severity, origin, message));
    }
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

pub(crate) fn expand_map_pairs(char_repr: &str, glyph: &str) -> Vec<(u32, String)> {
    // Range: U+XXXX..YYYY or u+XXXX..YYYY
    if let Some(hex_rest) = char_repr.strip_prefix("U+").or_else(|| char_repr.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..")
            && let (Ok(start), Ok(end)) = (
                u32::from_str_radix(start_hex, 16),
                u32::from_str_radix(end_hex, 16),
            ) {
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
        vec![(cp, names.into_iter().next().unwrap_or_else(|| glyph.to_string()))]
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
