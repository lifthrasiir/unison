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
    /// The glyph aliases the source declares. The items above are already
    /// canonicalized against it and carry no alias declarations at all; this is
    /// here for the consumers that do not go through the item list — GSUB,
    /// `assert shape`, validation — and for the editor, which has to recognize
    /// an alias name written in the text. See [`crate::alias`].
    pub aliases: crate::alias::AliasMap,
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
/// expanded, `map-decomposed` directives turned into synthesized composite
/// glyphs + `map` entries via NFD decomposition, and every glyph-name
/// reference canonicalized against the declared aliases.
pub(crate) fn expand_documents(docs: &[&Document], name_parts: &NamePartsMap) -> Expansion {
    expand_documents_for(docs, name_parts, &crate::faces::FaceSet::collect(docs))
}

pub(crate) fn expand_documents_for(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    faces: &crate::faces::FaceSet,
) -> Expansion {
    expand_for(docs, name_parts, faces.primary())
}

/// Expand for one face: items qualified with a slice the face does not include
/// are dropped here, so nothing downstream — cmap, GSUB, the glyph cache — ever
/// sees a mapping that belongs to a different typeface.
///
/// Glyphs are never filtered. Every face draws from the same glyph set; what a
/// slice changes is which character reaches which glyph.
pub(crate) fn expand_for(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    face: &crate::faces::Face,
) -> Expansion {
    let mut all_items: Vec<ExpandedItem> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // Collected before anything is expanded: from here on every glyph name in
    // `all_items` is the canonical one, so nothing downstream — the glyph
    // cache, the cmap, the on-demand injector — ever sees an alias.
    let aliases = crate::alias::AliasMap::collect(docs, name_parts);
    // Slice-scoped `name-parts`, so a qualified line substitutes with the
    // bindings of the slice it is being stated for. Empty (and free) unless the
    // source binds something per slice.
    let scoped = crate::document::SliceNameParts::with_base(docs, name_parts.clone());

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let origin = ItemRef::new(doc_idx, item_idx);
            // A qualifier lists the slices the line is stated for, one at a
            // time; the face keeps the ones it includes. An unqualified line is
            // the base slice, which every face includes.
            let mut slices: Vec<Option<&str>> = match item.slice_qualifier() {
                [] => vec![None],
                qual => qual.iter().map(|s| Some(s.as_str())).collect(),
            };
            slices.retain(|s| face.includes(*s));
            if slices.is_empty() {
                continue;
            }
            // An alias declares no glyph. It has already been folded into
            // `aliases`, and the references that named it are rewritten below.
            if matches!(item, DocumentItem::GlyphAlias { .. }) {
                continue;
            }
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
                            ..r.clone()
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
                                if let DocumentItem::Glyph {
                                    body: ref mut b, ..
                                } = item
                                {
                                    b.pixels = body.pixels.clone();
                                    b.points = body.points.clone();
                                    b.keep = body.keep;
                                    // `mark`, `inline` and `desync` are
                                    // invisible in the outline of one build, so
                                    // dropping one here builds a clean font
                                    // that behaves wrong: a mark that is not a
                                    // mark never reaches GPOS, and a lost
                                    // `desync` puts the grid back into the
                                    // vector face.
                                    b.inline = body.inline;
                                    b.mark = body.mark;
                                    b.desync = body.desync;
                                    b.advance = body.advance;
                                    b.left = body.left;
                                    b.top = body.top;
                                    b.scale = body.scale;
                                }
                                all_items.push(ExpandedItem {
                                    item,
                                    origin: Some(origin),
                                });
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
                        item: DocumentItem::Glyph {
                            name: GlyphName(name_str),
                            body,
                        },
                        origin: Some(origin),
                    });
                }
            } else {
                // Everything else is emitted once per slice it is stated for,
                // each with that slice's name parts. Downstream never sees a
                // multi-slice item: `slices` here is one slice or none.
                for slice in &slices {
                    let parts = scoped.for_slice(*slice);
                    let one: Vec<String> = slice.iter().map(|s| s.to_string()).collect();
                    let item = match item {
                        DocumentItem::Map {
                            char_repr,
                            selector,
                            glyph,
                            ..
                        } => DocumentItem::Map {
                            slices: one,
                            comment: None,
                            char_repr: char_repr.clone(),
                            selector: selector.clone(),
                            glyph: substitute_name_parts(glyph, parts),
                        },
                        DocumentItem::MapDecomposed {
                            char_repr,
                            selector,
                            glyph,
                            ..
                        } => DocumentItem::MapDecomposed {
                            slices: one,
                            comment: None,
                            char_repr: char_repr.clone(),
                            selector: selector.clone(),
                            glyph: glyph.as_ref().map(|g| substitute_name_parts(g, parts)),
                        },
                        DocumentItem::Feature { .. } | DocumentItem::FeatureAnchor { .. } => {
                            let mut item = item.clone();
                            match &mut item {
                                DocumentItem::Feature { slices, .. }
                                | DocumentItem::FeatureAnchor { slices, .. } => *slices = one,
                                _ => unreachable!(),
                            }
                            item
                        }
                        other => other.clone(),
                    };
                    all_items.push(ExpandedItem {
                        item,
                        origin: Some(origin),
                    });
                }
            }
        }
    }

    // Every `ref` now points at the glyph it actually names. A `map` target is
    // not rewritten in place: it is a pattern that `expand_map_pairs` unrolls
    // per codepoint, so its canonicalization happens where the concrete names
    // appear — `AliasMap::canonicalize_pairs`, at each of those call sites.
    if !aliases.is_empty() {
        for e in &mut all_items {
            if let DocumentItem::Glyph { body, .. } = &mut e.item {
                for gref in &mut body.refs {
                    aliases.canonicalize(&mut gref.name);
                }
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
        let DocumentItem::Map {
            char_repr, glyph, ..
        } = &e.item
        else {
            continue;
        };
        let mut pairs = expand_map_pairs(char_repr, glyph);
        aliases.canonicalize_pairs(&mut pairs);
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
    inject_on_demand_glyph_items(
        &mut all_items,
        map_targets,
        name_parts,
        &aliases,
        &mut diagnostics,
    );

    Expansion {
        items: all_items,
        diagnostics,
        aliases,
    }
}

/// A `map decomposed` directive waiting to be expanded, lifted out of
/// `all_items` so the expansion can push back into it.
struct PendingDecomposition {
    slices: Vec<String>,
    char_repr: String,
    /// Always invalid, and carried this far only so the rejection can name what
    /// was written. See [`DocumentItem::MapDecomposed`].
    selector: Option<String>,
    glyph: Option<String>,
    origin: Option<ItemRef>,
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
    let pending: Vec<PendingDecomposition> = all_items
        .iter()
        .filter_map(|e| match &e.item {
            DocumentItem::MapDecomposed {
                slices,
                char_repr,
                selector,
                glyph,
                ..
            } => Some(PendingDecomposition {
                slices: slices.clone(),
                char_repr: char_repr.clone(),
                selector: selector.clone(),
                glyph: glyph.clone(),
                origin: e.origin,
            }),
            _ => None,
        })
        .collect();

    for PendingDecomposition {
        slices,
        char_repr,
        selector,
        glyph,
        origin,
    } in pending
    {
        // A variation sequence is its own canonical decomposition, so there is
        // nothing here to synthesize from. Rejected before the pairs are even
        // expanded, so the message names the sequence rather than complaining
        // about the base character alone.
        if let Some(sel) = &selector {
            diagnostics.push(Diagnostic::error(
                origin,
                format!(
                    "map generate takes a single character, not the variation \
                     sequence '{char_repr} {sel}'; a variation sequence has no \
                     canonical decomposition — write `map {char_repr} {sel} = GLYPH`",
                ),
            ));
            continue;
        }

        // The synthesized composite's *name* is not canonicalized — it is a
        // glyph this expansion is about to define, not a reference to one.
        let pairs = decomposed_map_pairs(&char_repr, glyph.as_deref());
        if pairs.is_empty() {
            diagnostics.push(Diagnostic::error(
                origin,
                format!("map has no valid codepoints ('{char_repr}')"),
            ));
            continue;
        }

        for (cp, composite_name) in pairs {
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

            let refs: Vec<GlyphRef> = nfd
                .iter()
                .map(|c| GlyphRef {
                    raw_name: None,
                    comment: None,
                    name: cp_to_glyph[&(*c as u32)].clone(),
                    offset: None,
                    negated: false,
                    // A generated composite stands in for its decomposition, so
                    // forwarding the components' surviving anchors is the right
                    // default — a hand-written replacement decides per ref.
                    inherit: true,
                    fill: None,
                    visibility: None,
                })
                .collect();

            decomposed_items.push(ExpandedItem {
                item: DocumentItem::Glyph {
                    name: GlyphName(composite_name.clone()),
                    body: GlyphBody {
                        refs,
                        ..GlyphBody::new()
                    },
                },
                origin,
            });
            decomposed_items.push(ExpandedItem {
                item: DocumentItem::Map {
                    slices: slices.clone(),
                    comment: None,
                    char_repr: format!("U+{cp:04X}"),
                    selector: None,
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
    aliases: &crate::alias::AliasMap,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut defined: HashSet<String> = HashSet::new();
    let mut glyph_bodies: HashMap<String, GlyphBody> = HashMap::new();

    for e in all_items.iter() {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = &e.item
        {
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
    // is as unusable as an undefined name, and reported the same way. The one
    // exception is a `keep` placeholder, which `seed_cache` does build as an
    // empty anchor-carrying entry (and `issues.rs` likewise exempts from its
    // "has no content" warning).
    let contentless: HashSet<String> = glyph_bodies
        .iter()
        .filter(|(_, body)| body.pixels.is_none() && body.refs.is_empty() && !body.keep)
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
                    // Canonicalized for the same reason GSUB canonicalizes
                    // them: a remap naming an alias names its target, and
                    // reporting the alias as undefined would be reporting a
                    // rule the builder resolves perfectly well.
                    let mut names = expand_name_element(token, name_parts);
                    aliases.canonicalize_all(&mut names);
                    for name in names {
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

    // The alias items are gone from `all_items` by now, so `defined` holds
    // only the glyphs themselves. A half of a color/mono pair may well be
    // stated as an alias (`glyph X:color = Y:color`) — it is a second name for
    // a glyph, so the pair is complete — and both the existence check and the
    // body lookup have to see through it.
    fn canonical<'a>(aliases: &'a crate::alias::AliasMap, n: &'a str) -> &'a str {
        aliases.resolved_target(n).unwrap_or(n)
    }

    for (name, origin) in unique {
        use crate::on_demand::{OnDemandGlyph, detect_on_demand_glyph};
        match detect_on_demand_glyph(&name, |n| defined.contains(canonical(aliases, n))) {
            Some(OnDemandGlyph::Shape(spec)) => {
                let grid = crate::on_demand::make_on_demand_grid(&spec);
                all_items.push(ExpandedItem {
                    item: DocumentItem::Glyph {
                        name: GlyphName(name),
                        body: GlyphBody {
                            scale: spec.scale,
                            pixels: Some(grid),
                            inline: true,
                            ..GlyphBody::new()
                        },
                    },
                    origin,
                });
            }
            Some(OnDemandGlyph::ColorMono { mono, color }) => {
                let mono_body = glyph_bodies.get(canonical(aliases, &mono));
                let color_body = glyph_bodies.get(canonical(aliases, &color));
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
                            r.offset.map(|(row, col)| {
                                (row * combined_s / mono_s, col * combined_s / mono_s)
                            })
                        };
                        refs.push(GlyphRef {
                            raw_name: None,
                            comment: None,
                            name: r.name.clone(),
                            offset,
                            negated: r.negated,
                            inherit: r.inherit,
                            fill: r.fill.clone(),
                            visibility: Some(LayerVisibility::MonoOnly),
                        });
                    }
                    for r in &color_body.refs {
                        let offset = if color_s == combined_s {
                            r.offset
                        } else {
                            r.offset.map(|(row, col)| {
                                (row * combined_s / color_s, col * combined_s / color_s)
                            })
                        };
                        refs.push(GlyphRef {
                            raw_name: None,
                            comment: None,
                            name: r.name.clone(),
                            offset,
                            negated: r.negated,
                            inherit: r.inherit,
                            fill: r.fill.clone(),
                            visibility: Some(LayerVisibility::ColorOnly),
                        });
                    }
                    let mut points = Vec::new();
                    points.extend_from_slice(&mono_body.points);
                    points.extend_from_slice(&color_body.points);

                    // `desync` travels with the grid that was picked: it says
                    // what that grid is for, so it cannot be read off the other
                    // half.
                    let (pixels, desync) = match (&mono_body.pixels, &color_body.pixels) {
                        (Some(mg), Some(cg)) => {
                            let mg2 = if mono_s == combined_s {
                                mg.clone()
                            } else {
                                mg.rescale(mono_s as u8, combined_scale)
                            };
                            let cg2 = if color_s == combined_s {
                                cg.clone()
                            } else {
                                cg.rescale(color_s as u8, combined_scale)
                            };
                            if mg2.width >= cg2.width && mg2.height >= cg2.height {
                                (Some(mg2), mono_body.desync)
                            } else {
                                (Some(cg2), color_body.desync)
                            }
                        }
                        (None, Some(cg)) => (
                            Some(if color_s == combined_s {
                                cg.clone()
                            } else {
                                cg.rescale(color_s as u8, combined_scale)
                            }),
                            color_body.desync,
                        ),
                        (Some(mg), None) => (
                            Some(if mono_s == combined_s {
                                mg.clone()
                            } else {
                                mg.rescale(mono_s as u8, combined_scale)
                            }),
                            mono_body.desync,
                        ),
                        (None, None) => (None, false),
                    };

                    all_items.push(ExpandedItem {
                        item: DocumentItem::Glyph {
                            name: GlyphName(name),
                            body: GlyphBody {
                                refs,
                                points,
                                pixels,
                                desync,
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
        let (severity, message) = match (
            unresolved.contains(&name),
            contentless.contains(&name),
            kind,
        ) {
            (false, false, _) => continue,
            // A ref still carrying its `@` was written before any glyph the
            // `@` could stand for; saying so beats sending the author looking
            // for a glyph literally named `@…`.
            (true, _, RefKind::Ref) if name.starts_with('@') => (
                Severity::Error,
                format!(
                    "ref '{name}' has no glyph to expand `@` against: `@` stands for the \
                     last glyph declared without one, and this file declares none above it",
                ),
            ),
            (true, _, RefKind::Ref) => (Severity::Error, format!("unresolved ref '{name}'")),
            (true, _, RefKind::Map(char_repr)) => (
                Severity::Error,
                format!("map '{char_repr}' targets undefined glyph '{name}'"),
            ),
            (true, _, RefKind::Remap) => (
                Severity::Warning,
                format!("remap references undefined glyph '{name}'"),
            ),
            (false, true, RefKind::Ref) => (Severity::Error, format!("ref '{name}' {EMPTY}")),
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
    let bases = expand_map_codepoints(char_repr);
    let selectors = expand_map_codepoints(selector);
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
    if let Some(&cp) = bases.iter().find(|cp| crate::ucd::is_variation_selector(**cp)) {
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

/// The codepoints one half of a `map` names: a single character, a `U+X..Y`
/// range, or a top-level pipe list. Invalid and unparsable entries are dropped.
pub(crate) fn expand_map_codepoints(token: &str) -> Vec<u32> {
    if let Some(hex_rest) = token.strip_prefix("U+").or_else(|| token.strip_prefix("u+"))
        && let Some((start_hex, end_hex)) = hex_rest.split_once("..")
        && let (Ok(start), Ok(end)) = (
            u32::from_str_radix(start_hex, 16),
            u32::from_str_radix(end_hex, 16),
        )
    {
        if end < start || u64::from(end) - u64::from(start) + 1 > MAX_EXPANSION as u64 {
            return vec![];
        }
        return (start..=end).filter(|cp| char::from_u32(*cp).is_some()).collect();
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

#[cfg(test)]
mod uvs_expand_tests {
    use super::*;

    fn triples(base: &str, sel: &str, glyph: &str) -> Vec<(u32, u32, String)> {
        expand_uvs_map_triples(base, sel, glyph).expect("expected a valid expansion")
    }

    #[test]
    fn a_fixed_pair_expands_to_one_triple() {
        assert_eq!(
            triples("U+0030", "U+FE0F", "num-zero-emoji"),
            vec![(0x30, 0xFE0F, "num-zero-emoji".to_string())],
        );
    }

    /// The keycap shape: many bases, one selector. The glyph pattern runs in
    /// lock-step with the half that varies.
    ///
    /// The target arrives here already through `substitute_name_parts`, so an
    /// inline numeric range is written out — `($0..2)` is that step's input,
    /// not this one's.
    #[test]
    fn the_base_may_vary_against_a_fixed_selector() {
        assert_eq!(
            triples("U+0030..0032", "U+FE0F", "num-(0|1|2)-emoji"),
            vec![
                (0x30, 0xFE0F, "num-0-emoji".to_string()),
                (0x31, 0xFE0F, "num-1-emoji".to_string()),
                (0x32, 0xFE0F, "num-2-emoji".to_string()),
            ],
        );
    }

    /// The ideographic-variant shape: one base, many selectors. Both directions
    /// occur in real sources, which is why neither position is privileged.
    #[test]
    fn the_selector_may_vary_against_a_fixed_base() {
        assert_eq!(
            triples("U+4E00", "U+E0100..E0102", "han-4e00-ivs(1|2|3)"),
            vec![
                (0x4E00, 0xE0100, "han-4e00-ivs1".to_string()),
                (0x4E00, 0xE0101, "han-4e00-ivs2".to_string()),
                (0x4E00, 0xE0102, "han-4e00-ivs3".to_string()),
            ],
        );
    }

    #[test]
    fn a_pipe_list_varies_the_same_way_a_range_does() {
        assert_eq!(
            triples("#|*", "U+FE0F", "keycap-(hash|star)"),
            vec![
                (0x23, 0xFE0F, "keycap-hash".to_string()),
                (0x2A, 0xFE0F, "keycap-star".to_string()),
            ],
        );
    }

    #[test]
    fn both_halves_varying_is_rejected() {
        assert_eq!(
            expand_uvs_map_triples("U+0030..0032", "U+FE0E..FE0F", "x"),
            Err(UvsExpandError::BothVary),
        );
    }

    /// The halves are not interchangeable: a selector has to be one, and a base
    /// has to not be one. Both directions are checked so that a swapped line is
    /// caught rather than silently building a sequence nothing will ever match.
    #[test]
    fn each_half_must_be_the_kind_it_stands_in_for() {
        assert_eq!(
            expand_uvs_map_triples("U+0030", "U+0031", "x"),
            Err(UvsExpandError::NotASelector {
                cp: 0x31,
                selector_half: true,
            }),
        );
        assert_eq!(
            expand_uvs_map_triples("U+FE0F", "U+FE0F", "x"),
            Err(UvsExpandError::NotASelector {
                cp: 0xFE0F,
                selector_half: false,
            }),
        );
    }

    /// A range that varies must not drag a selector into the base half either.
    #[test]
    fn a_range_covering_selectors_is_rejected_in_the_base_half() {
        assert!(matches!(
            expand_uvs_map_triples("U+FDFF..FE0F", "U+FE0F", "x"),
            Err(UvsExpandError::NotASelector {
                selector_half: false,
                ..
            }),
        ));
    }

    #[test]
    fn an_unreadable_half_expands_to_nothing() {
        assert_eq!(
            expand_uvs_map_triples("nonsense", "U+FE0F", "x"),
            Err(UvsExpandError::Empty {
                selector_half: false
            }),
        );
        assert_eq!(
            expand_uvs_map_triples("U+0030", "nonsense", "x"),
            Err(UvsExpandError::Empty {
                selector_half: true
            }),
        );
    }

    /// The Mongolian selectors count, because the shaper's own range list has
    /// them — see [`crate::ucd::is_variation_selector`].
    #[test]
    fn mongolian_free_variation_selectors_count() {
        assert_eq!(
            triples("U+1820", "U+180B", "a-fvs1"),
            vec![(0x1820, 0x180B, "a-fvs1".to_string())],
        );
    }
}
