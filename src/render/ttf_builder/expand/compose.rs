//! Expansion of IDC composition lines (`compose`), and the ink profiles they are checked against.

use super::*;

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
pub(super) fn expand_compose_lines(
    all_items: &mut [ExpandedItem],
    diagnostics: &mut Vec<Diagnostic>,
    undecided_parts: &mut HashSet<(Option<ItemRef>, String)>,
    audit: &crate::audit::AuditRules,
    aliases: &crate::alias::AliasMap,
    name_parts: &NamePartsMap,
) {
    let clearances = &audit.ideal_clearance;
    let has_compose = |e: &ExpandedItem| matches!(&e.item, DocumentItem::Glyph { body, .. } if !body.compose.is_empty());
    if !all_items.iter().any(has_compose) {
        return;
    }

    // Declared, not raster: a header's `W H` before `scale` multiplied it.
    let declared = |body: &crate::document::GlyphBody| body.declared_extent();
    let mut boxes: HashMap<String, crate::compose::PartDims> = HashMap::default();
    for e in all_items.iter() {
        if let DocumentItem::Glyph { name, body } = &e.item {
            // First definition wins, as everywhere else.
            boxes
                .entry(name.display())
                .or_insert_with(|| crate::compose::PartDims::of(body));
        }
    }
    let dims = |name: &str| {
        boxes
            .get(name)
            .copied()
            .unwrap_or(crate::compose::PartDims::Unknown)
    };

    // What every base name is drawn at, for the one question `expand_compose`
    // cannot answer from a name alone: whether an undecided component *could*
    // ever fill its slot. Built once over the names already collected above,
    // and only where the source has an undecided component to ask about — an
    // IDS-populated source has tens of thousands of names and nearly all of
    // them are decided.
    let mut families: HashMap<&str, Vec<crate::compose::FamilyVariant>> = HashMap::default();
    if all_items.iter().any(|e| match &e.item {
        DocumentItem::Glyph { body, .. } => body
            .compose
            .iter()
            .flat_map(|c| c.part_names())
            .any(crate::compose::is_undecided),
        _ => false,
    }) {
        for (name, part) in &boxes {
            if let Some((base, _)) = name.split_once(':')
                && let crate::compose::PartDims::Size(w, h, margin) = *part
            {
                families.entry(base).or_default().push(((w, h), margin));
            }
        }
    }
    let family = |name: &str| families.get(name).cloned().unwrap_or_default();

    let profiles = ink_profiles(all_items, clearances, aliases, name_parts);
    let ink = |name: &str| profiles.get(name);

    // Each line is solved from what the maps above say and nothing another
    // line derives, so the lines are solved on every core and written back in
    // item order, which keeps the diagnostics in the order a serial walk left
    // them in.
    let composed: Vec<usize> = (0..all_items.len())
        .filter(|&i| has_compose(&all_items[i]))
        .collect();
    let solved = {
        let all_items = &*all_items;
        crate::parallel::map_indexed(composed.len(), &crate::cancel::CancelToken::never(), |k| {
            let e = &all_items[composed[k]];
            let DocumentItem::Glyph { name, body } = &e.item else {
                unreachable!("`composed` holds glyph blocks only");
            };
            let glyph_name = name.display();
            let parent = declared(body);
            let mut diagnostics = Vec::new();
            let mut undecided = Vec::new();
            // A second IDC line would be a second answer to "what shape is
            // this glyph", and there is no rule for combining them: ⿰
            // inside ⿱ is a component that is itself a composite, written
            // as its own glyph.
            if body.compose.len() > 1 {
                diagnostics.push(
                    Diagnostic::error(
                        e.origin,
                        format!(
                            "glyph '{glyph_name}' has {} IDC lines; a glyph is split once, \
                                 and a part that is itself split is a glyph of its own",
                            body.compose.len(),
                        ),
                    )
                    .about(&glyph_name),
                );
            }
            let mut derived = Vec::new();
            for compose in &body.compose {
                for name in compose.part_names() {
                    if crate::compose::is_undecided(name) {
                        undecided.push((e.origin, name.to_string()));
                    }
                }
                let contact = audit.max_contact_run.for_glyph(&glyph_name);
                let rule = clearances.for_glyph(&glyph_name).map(|(written, band)| {
                    crate::compose::ClearanceRule {
                        written,
                        band,
                        ink: &ink,
                        max_contact_run: contact.map(|(_, max)| max),
                        contact_written: contact.map_or("", |(w, _)| w),
                    }
                });
                let (refs, issues) = crate::compose::expand_compose(
                    &glyph_name,
                    parent,
                    crate::compose::Raster::of(body),
                    compose,
                    &dims,
                    Some(&family),
                    rule.as_ref(),
                );
                for (severity, message) in issues {
                    // Named down to the glyph, not just to the line: one
                    // pattern block writes the split of thousands of
                    // glyphs, and what each of them is made of — so what
                    // is wrong with it — is its own. See
                    // [`crate::glyph_flags`].
                    diagnostics
                        .push(Diagnostic::new(severity, e.origin, message).about(&glyph_name));
                }
                derived.extend(refs);
            }
            (derived, diagnostics, undecided)
        })
    };
    for (i, slot) in composed.into_iter().zip(solved) {
        let (mut derived, diags, undecided) = slot.expect("a `never` token cannot cancel");
        let DocumentItem::Glyph { body, .. } = &mut all_items[i].item else {
            unreachable!("`composed` holds glyph blocks only");
        };
        diagnostics.extend(diags);
        undecided_parts.extend(undecided);
        // In front of the block's own refs, which are what is drawn *over* the
        // split, and in place of the line: an expanded body carries no IDC.
        derived.append(&mut body.refs);
        body.refs = derived;
        body.compose.clear();
    }
}

/// The [`InkProfile`](crate::compose::InkProfile) of every part a clearance-
/// checked IDC line names — and of nothing else, so a source stating no
/// `audit ideal-clearance` pays only a walk over the compose lines it has.
///
/// A part drawn by its own pixels is read straight off them. A part that is a
/// **composite** draws no pixels of its own, so it is flattened first, by the
/// same resolution the font is built from ([`crate::ref_composite::resolve_glyph_bodies`]) over the
/// parts alone — a radical written as `ref` to a shared drawing (`han-6b63` is
/// `ref han-6b62`) is a component like any other, and measuring it by a second,
/// simpler flattener would measure it differently from what the font draws. The
/// subgraph is walked rather than the whole font: what is resolved here is the
/// parts of clearance-checked lines and, transitively, what those refer to.
///
/// A part that is *itself* split by an IDC line is measured the same way: the
/// walk derives its line before flattening it
/// ([`resolve_reachable`](crate::ref_composite::resolve_reachable)), which is
/// what an IDS-populated source is full of — `⿱艹林` names 林, which is
/// `⿰木木`. What no profile can be had for — a name nothing declares, a part
/// with no ink, a nested line whose own components are undecided — the
/// clearance check reads as "not measurable" and stands down for the whole
/// line, since half a line's ink measured is worse than none.
fn ink_profiles(
    all_items: &[ExpandedItem],
    clearances: &crate::audit::IdealClearances,
    aliases: &crate::alias::AliasMap,
    name_parts: &NamePartsMap,
) -> HashMap<String, crate::compose::InkProfile> {
    if clearances.is_empty() {
        return HashMap::default();
    }
    let mut wanted: HashSet<&str> = HashSet::default();
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
        return HashMap::default();
    }
    // First definition wins here as everywhere else, which is what the
    // `or_insert_with` says; the lookups below all go through it.
    let mut bodies: HashMap<&str, &GlyphBody> = HashMap::default();
    for e in all_items {
        if let DocumentItem::Glyph { name, body } = &e.item {
            bodies.entry(name.0.as_str()).or_insert(body);
        }
    }
    // A nested split is measured as the glyph it stands for, so that glyph is
    // made up here ([`crate::compose::nested_body`]) under the key the line
    // around it looks its ink up by, and flattened with the composites below.
    // See `crate::compose` (`# Nested splits`).
    let nested: Vec<(String, GlyphBody)> = {
        let dims = |name: &str| match bodies.get(name) {
            None => crate::compose::PartDims::Unknown,
            Some(body) => crate::compose::PartDims::of(body),
        };
        let mut nested: Vec<(String, GlyphBody)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::default();
        for e in all_items {
            let DocumentItem::Glyph { name, body } = &e.item else {
                continue;
            };
            if clearances.for_glyph(&name.display()).is_none() {
                continue;
            }
            for c in body.compose.iter().filter(|c| !c.op.enclosing()) {
                for item in &c.items {
                    let crate::document::ComposeItem::Nested(members) = item else {
                        continue;
                    };
                    let key = crate::compose::nested_key(c.op, members);
                    if bodies.contains_key(key.as_str()) || !seen.insert(key.clone()) {
                        continue;
                    }
                    if let Some(body) = crate::compose::nested_body(c.op, members, &dims) {
                        nested.push((key, body));
                    }
                }
            }
        }
        nested
    };
    for (key, body) in &nested {
        bodies.insert(key.as_str(), body);
        wanted.insert(key.as_str());
    }
    let profile_of = |body: &GlyphBody, pixels: &PixelGrid, raster: (i32, i32), scale: u8| {
        let extent = body.declared_extent().unwrap_or_else(|| {
            let s = scale.max(1) as u16;
            (pixels.width / s, pixels.height / s)
        });
        crate::compose::InkProfile::of(pixels, scale, raster, body.declared_origin(), extent)
    };

    // A profile is a pure function of one grid, and a source checking the
    // clearance of every han IDC line asks for thousands of them, so they are
    // taken on every core; the flattening between the two batches is not.
    // A part's name, its body, and the grid it is measured on with that grid's
    // raster origin and scale.
    type Measured<'a> = (&'a str, &'a GlyphBody, &'a PixelGrid, (i32, i32), u8);
    let profile_all = |grids: &[Measured]| {
        crate::parallel::map_indexed(grids.len(), &crate::cancel::CancelToken::never(), |i| {
            let (name, body, pixels, raster, scale) = grids[i];
            (name.to_string(), profile_of(body, pixels, raster, scale))
        })
        .into_iter()
        .flatten()
    };

    let mut direct = Vec::new();
    let mut composites: Vec<&str> = Vec::new();
    for &name in &wanted {
        let Some(&body) = bodies.get(name) else {
            continue;
        };
        // A part split by a line of its own draws no pixels either: the walk
        // derives that line and flattens what it stands for. See the note above.
        if !body.compose.is_empty() {
            composites.push(name);
            continue;
        }
        match (body.refs.is_empty(), body.pixels.as_ref()) {
            (true, Some(pixels)) => direct.push((name, body, pixels, (0, 0), body.scale)),
            (false, _) => composites.push(name),
            (true, None) => {}
        }
    }
    let mut profiles: HashMap<String, crate::compose::InkProfile> = profile_all(&direct).collect();
    if composites.is_empty() {
        return profiles;
    }

    let resolved = crate::ref_composite::resolve_reachable(
        composites.iter().copied(),
        &|name| bodies.get(name).copied(),
        aliases,
        name_parts,
        &crate::document::collect_anchor_aligns(all_items.iter().map(|e| &e.item)),
    );
    let flattened: Vec<_> = composites
        .into_iter()
        .filter_map(|name| {
            let (Some(&body), Some(flat)) = (bodies.get(name), resolved.get(name)) else {
                return None;
            };
            Some((
                name,
                body,
                &flat.grid,
                (flat.origin_col, flat.origin_row),
                flat.scale,
            ))
        })
        .collect();
    profiles.extend(profile_all(&flattened));
    profiles
}
