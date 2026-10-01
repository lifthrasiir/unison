//! Name-pattern expansion of document items, plus the on-demand and
//! decomposed-map glyph items synthesized on top of them.

use super::*;

mod alternatives;
mod compose;
mod decomposed;
mod items;
mod map_spec;
mod on_demand;

use alternatives::resolve_map_alternatives;
use compose::expand_compose_lines;
use decomposed::expand_decomposed_maps;
use items::expand_item;
use on_demand::inject_on_demand_glyph_items;

#[cfg(test)]
mod compose_expand_tests;
#[cfg(test)]
mod uvs_expand_tests;

#[cfg(feature = "editor")]
pub(crate) use alternatives::expand_map_pairs_per_alternative;
pub(crate) use alternatives::{MapAlternativeIndex, for_each_map_alternative_name};
#[cfg(test)]
pub(crate) use map_spec::parse_map_char;
pub(crate) use map_spec::{
    UvsExpandError, decomposed_map_pairs, expand_map_codepoints, expand_map_pairs,
    expand_uvs_map_triples, map_char_captures,
};

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
    pub aliases: std::sync::Arc<crate::alias::AliasMap>,
    /// Which items an `exists` governs and what each search found. The items
    /// above are already expanded against it and hold no `$N` at all; this is
    /// for the consumers that walk the *source* — validation, above all, which
    /// would otherwise read `han-($1)` as a glyph name nobody may write.
    pub exists: std::sync::Arc<crate::exists::ExistsScopes>,
    /// What resolving the searches reported, which `diagnostics` holds as
    /// well. Kept apart for [`NameLevel`], which is reused whole.
    #[cfg(feature = "editor")]
    exists_diagnostics: std::sync::Arc<[Diagnostic]>,
}

/// The searches with what resolving them reported, and the aliases (implicit
/// merges included): the two stages of an expansion that read names alone —
/// never a grid's cells, a ref's offset or a flag — and between them a third
/// of it. An edit to a drawing leaves both as they were, and one to a `ref` a
/// block's slots read leaves the searches. See [`crate::resolve::NameMemo`]
/// for when each is reused, and [`crate::document::NameMatch`] for what each
/// reads.
#[cfg(feature = "editor")]
#[derive(Clone)]
pub(crate) struct NameLevel {
    pub searches: Searches,
    pub aliases: std::sync::Arc<crate::alias::AliasMap>,
}

/// The searches, and what resolving them reported.
pub(crate) type Searches = (
    std::sync::Arc<crate::exists::ExistsScopes>,
    std::sync::Arc<[Diagnostic]>,
);

impl Expansion {
    pub fn items(&self) -> impl Iterator<Item = &DocumentItem> {
        self.items.iter().map(|e| &e.item)
    }

    /// The part of this a later expansion may reuse; see [`NameLevel`].
    #[cfg(feature = "editor")]
    pub(crate) fn name_level(&self) -> NameLevel {
        NameLevel {
            searches: (self.exists.clone(), self.exists_diagnostics.clone()),
            aliases: self.aliases.clone(),
        }
    }
}

/// One expanded `map` target: the glyph name a codepoint was pointed at, where
/// the line is.
struct MapTarget {
    name: String,
    origin: Option<ItemRef>,
    /// The `char_repr` the line was written with, for the message.
    char_repr: String,
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

/// The expansion every consumer shares: the union of every declared slice.
///
/// Not the primary face's. A face-filtered expansion cannot fault a line it
/// dropped, so validation used to be blind to whatever only a secondary face
/// includes; and the glyph store is the union's anyway. See
/// [`crate::faces::FaceSet::union`], and [`super::collect::face_items`] for
/// where a face's own view is taken out of this again.
pub(crate) fn expand_documents_for(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    faces: &crate::faces::FaceSet,
) -> Expansion {
    expand_for(docs, name_parts, &faces.union())
}

/// [`expand_documents_for`], given up between its stages once `cancel` is set.
///
/// For the editor's rebuild, which an edit arriving mid-expansion makes
/// obsolete and which the next rebuild waits behind; on a slow machine the
/// expansion is most of a second. `None` means cancelled and nothing else.
pub(crate) fn expand_documents_cancellable(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    faces: &crate::faces::FaceSet,
    cancel: &crate::cancel::CancelToken,
) -> Option<Expansion> {
    expand_inner(docs, name_parts, &faces.union(), Reuse::default(), cancel)
}

/// [`expand_documents_cancellable`], taking the searches and the aliases from
/// `reuse` where it has them rather than resolving them again. The caller
/// vouches for each: see [`crate::resolve::NameMemo`].
#[cfg(feature = "editor")]
pub(crate) fn expand_documents_reusing(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    faces: &crate::faces::FaceSet,
    reuse: Reuse,
    cancel: &crate::cancel::CancelToken,
) -> Option<Expansion> {
    expand_inner(docs, name_parts, &faces.union(), reuse, cancel)
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
    expand_inner(
        docs,
        name_parts,
        face,
        Reuse::default(),
        &crate::cancel::CancelToken::never(),
    )
    .expect("a `never` token cannot cancel")
}

/// What [`expand_inner`] takes as given rather than resolving: the searches,
/// or the searches and the aliases (the aliases are resolved against the
/// searches, so never without them).
#[derive(Default)]
pub(crate) struct Reuse {
    pub searches: Option<Searches>,
    pub aliases: Option<std::sync::Arc<crate::alias::AliasMap>>,
}

/// `cancel` is read between the stages below, and inside the one that runs on
/// every core (settling `map` alternatives); `None` means it was set. `reuse`
/// stands in for the first two stages where it can; see [`NameLevel`].
fn expand_inner(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    face: &crate::faces::Face,
    reuse: Reuse,
    cancel: &crate::cancel::CancelToken,
) -> Option<Expansion> {
    let mut all_items: Vec<ExpandedItem> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // Collected before anything is expanded: from here on every glyph name in
    // `all_items` is the canonical one, so nothing downstream — the glyph
    // cache, the cmap, the on-demand injector — ever sees an alias.
    // The searches are resolved first, because they decide which glyph blocks
    // declare what, and the merge candidates below rest on that. Nothing in the
    // other direction: a search reads the names as written, so it needs no
    // alias map of its own.
    let (exists, exists_diagnostics, aliases) = match reuse {
        Reuse {
            searches: Some((exists, diagnostics)),
            aliases: Some(aliases),
        } => (exists, diagnostics, aliases),
        Reuse { searches, .. } => {
            let (exists, exists_diagnostics) = match searches {
                Some(searches) => searches,
                None => {
                    let _perf = crate::startup::PerfStage::new("expand: exists");
                    let (exists, diagnostics) = crate::exists::resolve_scopes(docs, name_parts);
                    (exists.into(), diagnostics.into())
                }
            };
            if cancel.is_cancelled() {
                return None;
            }
            let _perf = crate::startup::PerfStage::new("expand: aliases");
            let aliases = crate::alias::AliasMap::collect_with_merges(docs, name_parts, &exists);
            (exists, exists_diagnostics, aliases.into())
        }
    };
    diagnostics.extend(exists_diagnostics.iter().cloned());
    let _perf = crate::startup::PerfStage::new("expand: items");
    if cancel.is_cancelled() {
        return None;
    }
    // Slice-scoped `name-parts`, so a qualified line substitutes with the
    // bindings of the slice it is being stated for. Empty (and free) unless the
    // source binds something per slice.
    let scoped = crate::document::SliceNameParts::with_base(docs, name_parts.clone());

    // Nothing an item expands to depends on another's, so the items run on
    // every core and are joined in source order, which is the order a serial
    // walk would have left them in. A glyph block under a search is cut into
    // runs of its matches, because a single han search is thousands of them
    // and would otherwise be one thread's work while the rest sat idle.
    const ROUNDS_PER_UNIT: usize = 256;
    let units: Vec<(ItemRef, std::ops::Range<usize>)> = docs
        .iter()
        .enumerate()
        .flat_map(|(d, doc)| {
            let exists = &exists;
            doc.items.iter().enumerate().flat_map(move |(i, item)| {
                let origin = ItemRef::new(d, i);
                let rounds = match (item, exists.scope(origin)) {
                    (DocumentItem::Glyph { .. }, Some(scope)) => scope.len(),
                    _ => 1,
                };
                (0..rounds.div_ceil(ROUNDS_PER_UNIT).max(1)).map(move |k| {
                    let start = k * ROUNDS_PER_UNIT;
                    (origin, start..(start + ROUNDS_PER_UNIT).min(rounds))
                })
            })
        })
        .collect();
    // Handed out a few dozen at a time: most items are a comment or a `map`
    // line, which is less work than taking one from the shared counter.
    let chunks: Vec<_> = units.chunks(64).collect();
    let expanded = crate::parallel::map_indexed(chunks.len(), cancel, |k| {
        let mut items = Vec::new();
        let mut diagnostics = Vec::new();
        for (origin, rounds) in chunks[k] {
            expand_item(
                &docs[origin.doc as usize].items[origin.item as usize],
                *origin,
                rounds.clone(),
                name_parts,
                &scoped,
                &exists,
                face,
                &mut items,
                &mut diagnostics,
            );
        }
        (items, diagnostics)
    });
    if cancel.is_cancelled() {
        return None;
    }
    for (items, diags) in expanded.into_iter().flatten() {
        all_items.extend(items);
        diagnostics.extend(diags);
    }
    if cancel.is_cancelled() {
        return None;
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
                for (name, raw_name) in body.compose.iter_mut().flat_map(|c| c.parts_mut()) {
                    // A component keeps the name it was written with beside
                    // the one it resolves to. The two say different things: the
                    // resolved name is the drawing, and the written one is
                    // which slot of the split the author picked — `阝:4x16-c =
                    // 阝:4x16-r` is a source saying that the right-hand drawing
                    // is what a `⿲`'s middle slot uses, and dropping the `-c`
                    // would leave `compose` ranking it as the wrong slot.
                    let written = name.clone();
                    aliases.canonicalize(name);
                    if raw_name.is_none() && *name != written {
                        *raw_name = Some(written);
                    }
                }
            }
        }
    }

    // An expansion merged into another of its block's declares no glyph of its
    // own: it is now a second name for the survivor, whose item is the one that
    // stays. Only an implicit merge is dropped this way — a `glyph` block whose
    // name a declared alias also claims is a source error, and dropping its
    // block would answer it by silently picking a winner. See `crate::merge`.
    all_items.retain(|e| match &e.item {
        DocumentItem::Glyph {
            name: GlyphName(n), ..
        } => !aliases.is_implicit(n),
        _ => true,
    });

    // After canonicalization, so a component named through an alias is sized by
    // the glyph it actually is, and before everything below, so nothing
    // downstream has to know an IDC line exists.
    drop(_perf);
    let _perf = crate::startup::PerfStage::new("expand: compose");
    let mut undecided_parts: HashSet<(Option<ItemRef>, String)> = HashSet::default();
    let audit = crate::audit::AuditRules::collect(docs);
    expand_compose_lines(
        &mut all_items,
        &mut diagnostics,
        &mut undecided_parts,
        &audit,
        &aliases,
        name_parts,
    );

    // Before anything reads a `map`'s target: every line here has exactly one
    // from now on, whatever it listed.
    if cancel.is_cancelled() {
        return None;
    }
    drop(_perf);
    let _perf = crate::startup::PerfStage::new("expand: map alternatives");
    resolve_map_alternatives(&mut all_items, &aliases, &mut diagnostics, cancel);
    drop(_perf);
    let _perf = crate::startup::PerfStage::new("expand: map pairs");
    if cancel.is_cancelled() {
        return None;
    }

    // Expanding a `map` is not free (the font has ranges thousands of
    // codepoints wide), and three later steps need the result, so it happens
    // exactly once here.
    //
    // The range form of `expand_map_pairs` filters out non-scalar values but
    // the single/pipe forms cannot, so an out-of-range `map U+FFFFFFFF = g`
    // used to reach the cmap builder unnoticed.
    let mut cp_to_glyph: HashMap<u32, String> = HashMap::default();
    let mut map_targets: Vec<MapTarget> = Vec::new();
    for e in &all_items {
        let DocumentItem::Map {
            char_repr, glyphs, ..
        } = &e.item
        else {
            continue;
        };
        // One target by now: `resolve_map_alternatives` above picked it.
        let glyph = glyphs.first().map(String::as_str).unwrap_or("");
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
            });
            cp_to_glyph.entry(cp).or_insert(target);
        }
    }

    drop(_perf);
    let _perf = crate::startup::PerfStage::new("expand: decomposed+on-demand");
    expand_decomposed_maps(&mut all_items, &cp_to_glyph, &mut diagnostics);
    if cancel.is_cancelled() {
        return None;
    }
    inject_on_demand_glyph_items(
        &mut all_items,
        map_targets,
        name_parts,
        &aliases,
        &undecided_parts,
        &mut diagnostics,
    );

    Some(Expansion {
        items: all_items,
        diagnostics,
        aliases,
        exists,
        #[cfg(feature = "editor")]
        exists_diagnostics,
    })
}

/// The one target an *expanded* `map` item carries.
///
/// Every item that reaches the collectors has been through
/// [`resolve_map_alternatives`], which leaves exactly one; this is that
/// invariant written down in one place rather than an indexing expression
/// repeated at each collector. An empty list cannot occur — the parser will not
/// produce one — and names no glyph if it somehow did.
pub(crate) fn resolved_map_target(glyphs: &[String]) -> &str {
    glyphs.first().map(String::as_str).unwrap_or("")
}
