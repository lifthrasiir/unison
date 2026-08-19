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

/// One expanded `map` target: the glyph name a codepoint was pointed at, where
/// the line is, and whether it said `ifexists`.
struct MapTarget {
    name: String,
    origin: Option<ItemRef>,
    /// The `char_repr` the line was written with, for the message.
    char_repr: String,
    if_exists: bool,
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
    expand_inner(docs, name_parts, face, false)
}

/// [`expand_for`] for a caller that only wants the `map` lines out of it.
///
/// On-demand synthesis is the expensive half of an expansion — it is where a
/// font this size spends most of it — and it produces *glyphs* and the
/// diagnostics about the names that reached it. A secondary face of a
/// collection takes neither: its glyphs come from the shared union store, whose
/// own expansion is the full one, and the same names are reported from there.
/// So it is skipped, and the expansion stops at the maps.
///
/// The maps themselves are untouched by it, which is what makes this safe:
/// [`inject_on_demand_glyph_items`] only ever *appends* glyph items.
pub(crate) fn expand_maps_for(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    face: &crate::faces::Face,
) -> Expansion {
    expand_inner(docs, name_parts, face, true)
}

fn expand_inner(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    face: &crate::faces::Face,
    maps_only: bool,
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
                    // The block as written, with every name in it substituted:
                    // the expansion shares this body, so a grid, a box or a
                    // flag written once holds for every glyph the pattern
                    // declares, exactly as a non-pattern block's does for its
                    // one glyph.
                    //
                    // An IDC line expands with the block exactly as the refs
                    // do. What it *derives* does not: the split is solved per
                    // glyph, below, from the boxes each expansion's own parts
                    // declare, so one line can write a different layout for
                    // every glyph it stands for.
                    let mut subst_body = body.clone();
                    for gref in &mut subst_body.refs {
                        gref.name = substitute_name_parts(&gref.name, name_parts);
                    }
                    for item in subst_body
                        .compose
                        .iter_mut()
                        .flat_map(|c| c.items.iter_mut())
                    {
                        if let crate::document::ComposeItem::Part { name, .. } = item {
                            *name = substitute_name_parts(name, name_parts);
                        }
                    }
                    match expand_glyph_block(&subst_name, &subst_body) {
                        Ok(expanded) if expanded.is_empty() => {
                            // The name expanded to nothing at all — an empty
                            // alternation, or a reversed range, which expands
                            // to no names rather than failing to parse. The
                            // block declares no glyph, and every use of the
                            // name it looks like it declares would otherwise
                            // report as merely undefined.
                            diagnostics.push(Diagnostic::error(
                                origin,
                                format!(
                                    "glyph pattern '{}' expands to no names, so it declares \
                                     no glyphs",
                                    subst_name.display(),
                                ),
                            ));
                        }
                        Ok(expanded) => {
                            for item in expanded {
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
                    for item in body.compose.iter_mut().flat_map(|c| c.items.iter_mut()) {
                        if let crate::document::ComposeItem::Part { name, .. } = item {
                            *name = substitute_name_parts(name, name_parts);
                        }
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
                            if_exists,
                            ..
                        } => DocumentItem::Map {
                            slices: one,
                            comment: None,
                            char_repr: char_repr.clone(),
                            selector: selector.clone(),
                            glyph: substitute_name_parts(glyph, parts),
                            if_exists: *if_exists,
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
                for item in body.compose.iter_mut().flat_map(|c| c.items.iter_mut()) {
                    if let crate::document::ComposeItem::Part { name, .. } = item {
                        aliases.canonicalize(name);
                    }
                }
            }
        }
    }

    // After canonicalization, so a component named through an alias is sized by
    // the glyph it actually is, and before everything below, so nothing
    // downstream has to know an IDC line exists.
    let mut undecided_parts: HashSet<(Option<ItemRef>, String)> = HashSet::new();
    let clearances = crate::audit::AuditRules::collect(docs).ideal_clearance;
    expand_compose_lines(
        &mut all_items,
        &mut diagnostics,
        &mut undecided_parts,
        &clearances,
    );

    // Expanding a `map` is not free (the font has ranges thousands of
    // codepoints wide), and three later steps need the result, so it happens
    // exactly once here.
    //
    // The range form of `expand_map_pairs` filters out non-scalar values but
    // the single/pipe forms cannot, so an out-of-range `map U+FFFFFFFF = g`
    // used to reach the cmap builder unnoticed.
    let mut cp_to_glyph: HashMap<u32, String> = HashMap::new();
    let mut map_targets: Vec<MapTarget> = Vec::new();
    // Which names exist, for the `ifexists` lines below. On-demand glyphs are
    // not items yet — they are injected from `map_targets` further down — so
    // the test has to be `glyph_name_exists`, which knows how to recognize one.
    let defined_names: HashSet<String> = all_items
        .iter()
        .filter_map(|e| match &e.item {
            DocumentItem::Glyph {
                name: GlyphName(n), ..
            } => Some(n.clone()),
            _ => None,
        })
        .collect();
    for e in &all_items {
        let DocumentItem::Map {
            char_repr,
            glyph,
            if_exists,
            ..
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
            map_targets.push(MapTarget {
                name: target.clone(),
                origin: e.origin,
                char_repr: char_repr.clone(),
                if_exists: *if_exists,
            });
            // An `ifexists` line claims nothing where its target is absent, so
            // it must not hold the codepoint against a later line that does map
            // it — `cp_to_glyph` is first-wins, and this is what the two
            // overlapping ranges the flag exists for look like.
            if *if_exists && !glyph_name_exists(&target, &defined_names, &aliases) {
                continue;
            }
            cp_to_glyph.entry(cp).or_insert(target);
        }
    }

    expand_decomposed_maps(&mut all_items, &cp_to_glyph, &mut diagnostics);
    if !maps_only {
        inject_on_demand_glyph_items(
            &mut all_items,
            map_targets,
            name_parts,
            &aliases,
            &undecided_parts,
            &mut diagnostics,
        );
    }

    Expansion {
        items: all_items,
        diagnostics,
        aliases,
    }
}

/// Turn every IDC line into the `ref`s it stands for.
///
/// Here rather than at resolve time, for the reason `map generate` synthesizes
/// its refs here: what follows — the glyph cache, the cmap, validation, the
/// editor's composite view and its shadows — then sees one ordinary composite
/// and cannot disagree about it. Component boxes come from the `glyph` headers
/// already in `all_items` (see [`crate::compose`] for why the *declared* box
/// and not the resolved one), so the pass is one map build and one walk.
///
/// `undecided_parts` collects `(glyph item, component name)` for each component
/// that has not picked its variant yet. Such a component keeps a bare name that
/// need not exist, and the ref derived from it is deliberately left unresolved
/// — that is what holds the glyph out of the font until someone decides. But it
/// is a mechanism, not a fault, so [`inject_on_demand_glyph_items`] must not go
/// on to report it as an unresolved ref: the line already said its piece as a
/// [`Severity::Todo`], and an IDS-populated font would otherwise open with two
/// errors per glyph across 20k glyphs, failing every build over source that is
/// merely unfinished.
fn expand_compose_lines(
    all_items: &mut Vec<ExpandedItem>,
    diagnostics: &mut Vec<Diagnostic>,
    undecided_parts: &mut HashSet<(Option<ItemRef>, String)>,
    clearances: &crate::audit::IdealClearances,
) {
    let has_compose = |e: &ExpandedItem| matches!(&e.item, DocumentItem::Glyph { body, .. } if !body.compose.is_empty());
    if !all_items.iter().any(has_compose) {
        return;
    }

    // Declared, not raster: a header's `W H` before `scale` multiplied it.
    let declared = |body: &crate::document::GlyphBody| body.declared_extent();
    let mut boxes: HashMap<String, Option<(u16, u16)>> = HashMap::new();
    for e in all_items.iter() {
        if let DocumentItem::Glyph { name, body } = &e.item {
            // First definition wins, as everywhere else.
            boxes
                .entry(name.display())
                .or_insert_with(|| declared(body));
        }
    }
    let dims_of = |boxes: &HashMap<String, Option<(u16, u16)>>, name: &str| match boxes.get(name) {
        None => crate::compose::PartDims::Unknown,
        Some(None) => crate::compose::PartDims::Undeclared,
        Some(Some((w, h))) => crate::compose::PartDims::Size(*w, *h),
    };

    // The glyphs an `ifexists` line leaves standing for nothing: the line was
    // the whole of the shape, so what is left is not an empty glyph but no
    // glyph, and it is dropped outright. That is what lets the `map` that
    // reaches it say `ifexists` too, and what makes an unconditional one say
    // "not defined" rather than the puzzling "defined but not built".
    //
    // Read before anything is derived, and the names are taken out of `boxes`,
    // so a line naming a glyph that is about to go finds it missing rather than
    // sized. One round of that, not a fixpoint: a component of a component is a
    // glyph of its own and answers for itself.
    let mut drop_item = vec![false; all_items.len()];
    for (idx, e) in all_items.iter().enumerate() {
        let DocumentItem::Glyph { body, .. } = &e.item else {
            continue;
        };
        // An IDC glyph states its `W H` on the header and so carries a grid,
        // which is empty until someone draws on it: it is the box the split
        // fills, not a shape of its own, so a blank one is not content. A grid
        // with so much as a hardblank in it is, and keeps the glyph.
        let empty = body.refs.is_empty()
            && !body.keep
            && body.pixels.as_ref().is_none_or(|g| g.is_all_empty());
        drop_item[idx] = empty
            && body
                .compose
                .iter()
                .any(|c| crate::compose::stands_for_nothing(c, &|n| dims_of(&boxes, n)));
    }
    if drop_item.iter().any(|&d| d) {
        let mut surviving: HashSet<String> = HashSet::new();
        for (idx, e) in all_items.iter().enumerate() {
            if let (false, DocumentItem::Glyph { name, .. }) = (drop_item[idx], &e.item) {
                surviving.insert(name.display());
            }
        }
        boxes.retain(|name, _| surviving.contains(name));
    }
    let dims = |name: &str| dims_of(&boxes, name);

    let profiles = ink_profiles(all_items, clearances);
    let ink = |name: &str| profiles.get(name);

    for (idx, e) in all_items.iter_mut().enumerate() {
        let DocumentItem::Glyph { name, body } = &mut e.item else {
            continue;
        };
        if body.compose.is_empty() || drop_item[idx] {
            continue;
        }
        let glyph_name = name.display();
        let parent = declared(body);
        // A second IDC line would be a second answer to "what shape is this
        // glyph", and there is no rule for combining them: ⿰ inside ⿱ is a
        // component that is itself a composite, written as its own glyph.
        if body.compose.len() > 1 {
            diagnostics.push(
                Diagnostic::error(
                    e.origin,
                    format!(
                        "glyph '{glyph_name}' has {} IDC lines; a glyph is split once, and a \
                         part that is itself split is a glyph of its own",
                        body.compose.len(),
                    ),
                )
                .about(&glyph_name),
            );
        }
        let mut derived = Vec::new();
        for compose in &body.compose {
            // A line that stands for nothing says nothing either — not even
            // that its components have yet to pick a variant.
            if crate::compose::stands_for_nothing(compose, &dims) {
                continue;
            }
            for name in compose.part_names() {
                if crate::compose::is_undecided(name) {
                    undecided_parts.insert((e.origin, name.to_string()));
                }
            }
            let rule = clearances
                .for_glyph(&glyph_name)
                .map(|(written, min, max)| crate::compose::ClearanceRule {
                    written,
                    min,
                    max,
                    ink: &ink,
                });
            let (refs, issues) = crate::compose::expand_compose(
                &glyph_name,
                parent,
                body.scale,
                compose,
                &dims,
                rule.as_ref(),
            );
            for (severity, message) in issues {
                // Named down to the glyph, not just to the line: one pattern
                // block writes the split of thousands of glyphs, and what each
                // of them is made of — so what is wrong with it — is its own.
                // See [`crate::glyph_flags`].
                diagnostics.push(Diagnostic::new(severity, e.origin, message).about(&glyph_name));
            }
            derived.extend(refs);
        }
        // In front of the block's own refs, which are what is drawn *over* the
        // split, and in place of the line: an expanded body carries no IDC.
        derived.append(&mut body.refs);
        body.refs = derived;
        body.compose.clear();
    }

    if drop_item.iter().any(|&d| d) {
        let mut idx = 0;
        all_items.retain(|_| {
            idx += 1;
            !drop_item[idx - 1]
        });
    }
}

/// The [`InkProfile`](crate::compose::InkProfile) of every part a clearance-
/// checked IDC line names — and of nothing else, so a source stating no
/// `audit ideal-clearance` pays only a walk over the compose lines it has.
///
/// Only a part drawn *entirely* by its own pixels is measured: a part that is
/// itself a composite draws ink this pass has not resolved and cannot see, and
/// half its ink measured is worse than none. The clearance check treats a
/// missing profile as "not measurable" and stands down for the whole line.
fn ink_profiles(
    all_items: &[ExpandedItem],
    clearances: &crate::audit::IdealClearances,
) -> HashMap<String, crate::compose::InkProfile> {
    if clearances.is_empty() {
        return HashMap::new();
    }
    let mut wanted: HashSet<&str> = HashSet::new();
    for e in all_items {
        let DocumentItem::Glyph { name, body } = &e.item else {
            continue;
        };
        if body.compose.is_empty() || clearances.for_glyph(&name.display()).is_none() {
            continue;
        }
        wanted.extend(body.compose.iter().flat_map(|c| c.part_names()));
    }
    if wanted.is_empty() {
        return HashMap::new();
    }
    let mut profiles = HashMap::new();
    for e in all_items {
        let DocumentItem::Glyph { name, body } = &e.item else {
            continue;
        };
        let (Some(pixels), true) = (body.pixels.as_ref(), body.refs.is_empty()) else {
            continue;
        };
        if !body.compose.is_empty() {
            continue;
        }
        let name = name.display();
        if !wanted.contains(name.as_str()) || profiles.contains_key(&name) {
            continue; // first definition wins, as everywhere else
        }
        let extent = body.declared_extent().unwrap_or_else(|| {
            let s = body.scale.max(1) as u16;
            (pixels.width / s, pixels.height / s)
        });
        profiles.insert(
            name,
            crate::compose::InkProfile::of(pixels, body.scale, body.declared_origin(), extent),
        );
    }
    profiles
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
                    if_exists: false,
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
                    if_exists: false,
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
    map_targets: Vec<MapTarget>,
    name_parts: &NamePartsMap,
    aliases: &crate::alias::AliasMap,
    undecided_parts: &HashSet<(Option<ItemRef>, String)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut defined: HashSet<String> = HashSet::new();
    // Where each glyph's body is, not a copy of it: the only reader below is
    // the color/mono pair, which is a handful of glyphs, while a copy here is
    // every pixel grid in the font cloned for nothing.
    let mut glyph_bodies: HashMap<String, usize> = HashMap::new();
    // A glyph with neither a pixel grid nor a ref never enters the resolution
    // cache (see `glyph_cache::seed_cache`), so it is not built and every use
    // of it — cmap entry, composite component, GSUB coverage — is dropped. It
    // is as unusable as an undefined name, and reported the same way. The one
    // exception is a `keep` placeholder, which `seed_cache` does build as an
    // empty anchor-carrying entry (and `issues.rs` likewise exempts from its
    // "has no content" warning).
    let mut contentless: HashSet<String> = HashSet::new();

    for (idx, e) in all_items.iter().enumerate() {
        if let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = &e.item
        {
            defined.insert(n.clone());
            glyph_bodies.insert(n.clone(), idx);
            if body.pixels.is_none() && body.refs.is_empty() && !body.keep {
                contentless.insert(n.clone());
            }
        }
    }

    // Every mention of an undefined name, so one that turns out not to be an
    // on-demand glyph either is reported at each place the author wrote it —
    // not once for the whole font. Deduped per (site, name) so a pattern that
    // expands to the same missing name repeatedly reports once per line.

    let mut mentions: Vec<Mention> = Vec::new();
    let mut mention_seen: HashSet<(Option<ItemRef>, String)> = HashSet::new();
    // `if_exists` rides along rather than filtering here: an `ifexists` name may
    // still be one the font generates (`ref 3x5 ifexists`), and the synthesis
    // below is driven by this same list. Only the *reporting* loop honors it.
    let mut consider =
        |name: &str, origin: Option<ItemRef>, by: Option<&str>, kind: RefKind, if_exists: bool| {
            let unusable = !defined.contains(name) || contentless.contains(name);
            if unusable && mention_seen.insert((origin, name.to_string())) {
                mentions.push(Mention {
                    name: name.to_string(),
                    origin,
                    by: by.map(str::to_string),
                    kind,
                    if_exists,
                });
            }
        };

    for e in all_items.iter() {
        match &e.item {
            // The referring glyph is named because one `glyph a-($#…)` line is
            // one origin and thousands of glyphs, each substituting a different
            // target into the same `ref`: whether the target is there is the
            // one thing that varies between them, so this is the only fault a
            // pattern's expansions can disagree about. See `Mention::by`.
            DocumentItem::Glyph {
                name: GlyphName(by),
                body,
            } => {
                for r in &body.refs {
                    consider(&r.name, e.origin, Some(by), RefKind::Ref, r.if_exists);
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
                        consider(&name, e.origin, None, RefKind::Remap, false);
                    }
                }
            }
            _ => {}
        }
    }
    // Map targets were expanded once by the caller: `glyph` is still a pattern
    // on the item, and the cmap builder expands it per codepoint.
    for t in map_targets {
        consider(
            &t.name,
            t.origin,
            None,
            RefKind::Map(t.char_repr),
            t.if_exists,
        );
    }

    // Synthesis is per unique name; reporting is per mention, so the loops
    // are separate.
    let unique: Vec<(String, Option<ItemRef>)> = {
        let mut seen: HashSet<&str> = HashSet::new();
        mentions
            .iter()
            // Contentless names are in `mentions` to be reported, but they are
            // defined, so there is nothing to synthesize for them.
            .filter(|m| !defined.contains(&m.name) && seen.insert(m.name.as_str()))
            .map(|m| (m.name.clone(), m.origin))
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
                // Copied out, because the synthesis below appends to the very
                // list they live in. Two bodies per color/mono pair, of which a
                // font has a handful.
                let body_at = |name: &str| {
                    let &idx = glyph_bodies.get(canonical(aliases, name))?;
                    match &all_items[idx].item {
                        DocumentItem::Glyph { body, .. } => Some(body.clone()),
                        _ => None,
                    }
                };
                let mono_body = body_at(&mono);
                let color_body = body_at(&color);
                if let (Some(mono_body), Some(color_body)) = (&mono_body, &color_body) {
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
                            if_exists: r.if_exists,
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
                            if_exists: r.if_exists,
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
                                origin: mono_body.origin.or(color_body.origin),
                                extent: mono_body.extent.or(color_body.extent),
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

    for Mention {
        name,
        origin,
        by,
        kind,
        if_exists,
    } in mentions
    {
        // `ifexists` says the author already knows this name may or may not be
        // there and wants the absent case to build nothing: the glyph is left
        // unbuilt by `glyph_cache::resolve_pending` and the mapping dropped by
        // `collect.rs` exactly as an unresolved name always was, and that is
        // the intended outcome rather than something to report. Placed with
        // the other skips, after synthesis, so an `ifexists` name the font can
        // generate still resolves.
        if if_exists {
            continue;
        }
        // A ref an IDC line derived from a component that has not picked its
        // variant is unresolved on purpose (see `expand_compose_lines`), so it
        // is not reported — but only for that glyph and that name, which keeps
        // a hand-written `ref` beside the IDC line reportable. The skip is here
        // rather than in `consider` so that the name still reaches on-demand
        // synthesis above: an undecided component naming a shape the font can
        // generate resolves as it always did.
        if matches!(kind, RefKind::Ref) && undecided_parts.contains(&(origin, name.clone())) {
            continue;
        }
        // A name that is defined but contentless gets its own wording:
        // "undefined" would send the author looking for a definition that is
        // right there. `origin`/`advance`/`extent`/`point` do not make a glyph
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
        let mut d = Diagnostic::new(severity, origin, message);
        d.glyph = by;
        diagnostics.push(d);
    }
}

/// One mention of a name that is missing or contentless, as the reporting loop
/// below needs it.
struct Mention {
    name: String,
    origin: Option<ItemRef>,
    /// The glyph whose `ref` this is, for a `ref`; `None` for a `map` or
    /// `remap`, neither of which is written inside a glyph. This is what lets
    /// one expansion of a pattern be faulted without faulting its siblings —
    /// see [`crate::resolve::Diagnostic::glyph`].
    by: Option<String>,
    kind: RefKind,
    if_exists: bool,
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

/// The codepoints one half of a `map` names: a single character, a `U+X..Y`
/// range, or a top-level pipe list. Invalid and unparsable entries are dropped.
pub(crate) fn expand_map_codepoints(token: &str) -> Vec<u32> {
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

/// Whether `name` denotes a glyph the font can contain: one the sources define
/// — through an alias, like every other reference — or one the font generates
/// on demand.
///
/// This is the question `ifexists` asks, and it is one function so that the
/// build, the sample, the specimen and the validation pass cannot come to
/// different answers about which codepoints an `ifexists` line claims. It is
/// the *source-side* reading: a glyph that is defined but fails to resolve (an
/// unresolved ref of its own) counts as existing here, and is dropped later by
/// the resolution cache — which is what makes `ref … ifexists` work without
/// this having to resolve anything.
pub(crate) fn glyph_name_exists(
    name: &str,
    defined: &HashSet<String>,
    aliases: &crate::alias::AliasMap,
) -> bool {
    let canonical = |n: &str| aliases.resolved_target(n).unwrap_or(n).to_string();
    let name = canonical(name);
    defined.contains(&name)
        || crate::on_demand::detect_on_demand_glyph(&name, |n| defined.contains(&canonical(n)))
            .is_some()
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

/// The diagnostics an IDC line produces once its components have been looked
/// up — which is here rather than in `compose.rs`, because whether a component
/// name resolves is a question only the whole expansion can answer.
#[cfg(test)]
mod compose_expand_tests {
    use crate::document_io::parse_document_from_str;
    use crate::issues::Severity;

    fn expand(src: &str) -> super::Expansion {
        let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        super::expand_documents(&docs, &name_parts)
    }

    fn of(expansion: &super::Expansion, severity: Severity) -> Vec<&str> {
        expansion
            .diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .map(|d| d.message.as_str())
            .collect()
    }

    /// Every ref of `glyph`, with the offset the line derived for it.
    fn placed<'a>(expansion: &'a super::Expansion, glyph: &str) -> Vec<(&'a str, (i16, i16))> {
        expansion
            .items()
            .filter_map(|item| match item {
                crate::document::DocumentItem::Glyph { name, body } if name.display() == glyph => {
                    Some(
                        body.refs
                            .iter()
                            .map(|r| (r.name.as_str(), r.offset.unwrap_or_default())),
                    )
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn defines(expansion: &super::Expansion, glyph: &str) -> bool {
        expansion.items().any(|item| {
            matches!(item, crate::document::DocumentItem::Glyph { name, .. }
                if name.display() == glyph)
        })
    }

    fn refs_of<'a>(expansion: &'a super::Expansion, glyph: &str) -> Vec<&'a str> {
        expansion
            .items()
            .filter_map(|item| match item {
                crate::document::DocumentItem::Glyph { name, body } if name.display() == glyph => {
                    Some(body.refs.iter().map(|r| r.name.as_str()))
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// The state every Han glyph is populated in (`PLANS.han.md` item 10):
    /// both components bare, and neither naming anything yet. The line is a
    /// Todo per component and **nothing else** — in particular not an
    /// `unresolved ref` for each derived ref, which would turn a work queue of
    /// 20k items into 40k errors and fail every build and CI run over source
    /// that is merely unfinished (see [`Severity`]).
    ///
    /// The refs are still emitted and still unresolved: that is what keeps the
    /// glyph out of the font, and it is a mechanism rather than a complaint.
    #[test]
    fn an_undecided_component_naming_nothing_is_a_todo_and_not_an_unresolved_ref() {
        let expansion = expand("glyph han-6cb3 4 2\n\u{2FF0} han-6c35 han-53ef\n");
        assert!(
            of(&expansion, Severity::Error).is_empty(),
            "{:?}",
            of(&expansion, Severity::Error)
        );
        assert_eq!(of(&expansion, Severity::Todo).len(), 2);
        assert_eq!(
            refs_of(&expansion, "han-6cb3"),
            vec!["han-6c35", "han-53ef"]
        );
    }

    /// Only the *undecided* half stands down. A component that picked a
    /// variant made a claim, and a claim about a glyph that does not exist is
    /// an ordinary error — the author decided, and decided wrong.
    #[test]
    fn a_decided_component_naming_nothing_is_still_an_error() {
        let expansion = expand("glyph han-6cb3 4 2\n\u{2FF0} han-6c35:2x2 han-53ef\n");
        let errors = of(&expansion, Severity::Error);
        assert!(
            errors.iter().any(|m| m.contains("'han-6c35:2x2'")),
            "{errors:?}"
        );
        assert!(
            !errors.iter().any(|m| m.contains("'han-53ef'")),
            "{errors:?}"
        );
    }

    /// The suppression is scoped to the name the IDC line left undecided, not
    /// to the glyph: a hand-written `ref` on the same block that names nothing
    /// is still reported.
    #[test]
    fn a_ref_beside_an_undecided_idc_line_is_still_reported() {
        let expansion = expand("glyph han-6cb3 4 2\n\u{2FF0} han-6c35 han-53ef\nref gone 0 0\n");
        assert_eq!(
            of(&expansion, Severity::Error),
            vec!["unresolved ref 'gone'"]
        );
    }

    /// A part drawn narrower than its box, seen end to end: the boxes tile the
    /// parent perfectly and the ink still leaves a 2-cell canyon.
    const CANYON: &str = "\
glyph p-a:2x2 2 2
@@..
@@..
glyph p-b:2x2 2 2
..@@
..@@
glyph p-x 4 2
\u{2FF0} p-a:2x2 p-b:2x2
";

    #[test]
    fn a_clearance_outside_the_ideal_range_is_a_warning() {
        let expansion = expand(&format!("audit ideal-clearance p-* 0 1\n{CANYON}"));
        assert!(of(&expansion, Severity::Error).is_empty());
        let warnings = of(&expansion, Severity::Warning);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("leaves 2 between 'p-a:2x2' and 'p-b:2x2'"),
            "{warnings:?}",
        );
        assert!(warnings[1].contains("leaves 2 in total"), "{warnings:?}");
    }

    #[test]
    fn a_glyph_no_rule_reaches_is_not_measured() {
        // The same source, held only by a prefix that does not name it.
        let expansion = expand(&format!("audit ideal-clearance q-* 0 1\n{CANYON}"));
        assert!(of(&expansion, Severity::Warning).is_empty());
        // …and with no rule at all.
        let expansion = expand(CANYON);
        assert!(of(&expansion, Severity::Warning).is_empty());
    }

    /// The parts of two glyphs written by one line, as a Han source writes a
    /// family: the components expand in lock-step with the block's name, and
    /// the *layout* is solved per glyph — the same line puts the second part at
    /// 1 in one expansion and at 2 in the next, because that expansion's first
    /// part is that much wider.
    #[test]
    fn a_pattern_block_expands_its_idc_line_per_glyph() {
        let expansion = expand(
            "\
glyph p-a1:1x2 1 2
@.
@.
glyph p-a2:2x2 2 2
@@
@@
glyph p-b1:3x2 3 2
@@@
@@@
glyph p-b2:2x2 2 2
@@
@@
glyph p-x(1|2) 4 2
\u{2FF0} p-a(1|2):(1|2)x2 p-b(1|2):(3|2)x2
",
        );
        assert!(
            of(&expansion, Severity::Error).is_empty(),
            "{:?}",
            of(&expansion, Severity::Error)
        );
        assert_eq!(
            placed(&expansion, "p-x1"),
            vec![("p-a1:1x2", (0, 0)), ("p-b1:3x2", (1, 0))],
        );
        assert_eq!(
            placed(&expansion, "p-x2"),
            vec![("p-a2:2x2", (0, 0)), ("p-b2:2x2", (2, 0))],
        );
    }

    /// A finding about one expansion of a pattern line names the glyph it is
    /// about, so the specimen faults that cell rather than every glyph the line
    /// declares (see [`crate::glyph_flags`]).
    #[test]
    fn a_finding_about_one_expansion_names_that_glyph() {
        let expansion = expand(
            "\
glyph p-a:2x2 2 2
@@
@@
glyph p-x(1|2) 4 2
\u{2FF0} p-a:2x2 (p-a:2x2|p-gone:2x2)
",
        );
        let faulted: Vec<_> = expansion
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| (d.glyph.as_deref(), d.message.as_str()))
            .collect();
        // The missing component and the ref it derived, both about `p-x2`
        // alone — `p-x1` is written by the same line and is faultless.
        assert_eq!(faulted.len(), 2, "{faulted:?}");
        assert!(
            faulted.iter().all(|(g, _)| *g == Some("p-x2")),
            "{faulted:?}"
        );
    }

    /// `ifexists` on the line: the expansions whose parts are all there are
    /// built, and the ones missing one are not glyphs at all — no refs, no
    /// diagnostics, and nothing left behind for a `map` to reach.
    #[test]
    fn an_ifexists_idc_line_drops_the_expansions_that_name_nothing() {
        let src = "\
glyph p-a:2x2 2 2
@@
@@
glyph p-x(1|2) 4 2
\u{2FF0} p-a:2x2 (p-a:2x2|p-gone:2x2) ifexists
";
        let expansion = expand(src);
        assert!(
            expansion.diagnostics.is_empty(),
            "{:?}",
            expansion
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            placed(&expansion, "p-x1"),
            vec![("p-a:2x2", (0, 0)), ("p-a:2x2", (2, 0))],
        );
        assert!(!defines(&expansion, "p-x2"));

        // …and a `map` reaching the glyph that did not survive says so, in the
        // words of a name nothing defines rather than of a glyph that is there
        // but empty.
        let expansion = expand(&format!("{src}map U+4E00 = p-x2\n"));
        assert_eq!(
            of(&expansion, Severity::Error),
            vec!["map 'U+4E00' targets undefined glyph 'p-x2'"],
        );
    }

    /// A glyph that has more to it than the conditional line keeps its other
    /// content: only the split goes.
    #[test]
    fn an_ifexists_line_leaves_the_rest_of_the_block_alone() {
        let expansion = expand(
            "\
glyph p-a:2x2 2 2
@@
@@
glyph p-x 4 2
\u{2FF0} p-a:2x2 p-gone:2x2 ifexists
ref p-a:2x2 0 0
",
        );
        assert!(expansion.diagnostics.is_empty());
        assert_eq!(placed(&expansion, "p-x"), vec![("p-a:2x2", (0, 0))]);
    }

    /// `$$` is a cell the source keeps clear on purpose, so it holds a
    /// neighbour off exactly as ink does — that is what it is for.
    #[test]
    fn a_hardblank_is_a_frontier() {
        let expansion = expand(
            "audit ideal-clearance p-* 0 1
glyph p-a:2x2 2 2
@@..
@@..
glyph p-b:2x2 2 2
$$@@
$$@@
glyph p-x 4 2
\u{2FF0} p-a:2x2 p-b:2x2
",
        );
        // 0 at each edge and 1 down the middle, where the ink alone would have
        // read 2 and failed.
        assert!(of(&expansion, Severity::Warning).is_empty());
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
