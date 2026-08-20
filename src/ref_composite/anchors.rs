//! Deriving a composite's offsets and anchors: matching a `ref`'s `-` anchors
//! against the `+` anchors already in the pool, the bearings a negative offset
//! stands for, and every way that derivation can fail.
//!
//! [`crate::ref_composite`] holds the two rules this rests on — anchor exposure
//! is opt-in, and a negative `ref` offset is a bearing.

use super::*;

/// A problem found while deriving a composite's offsets and anchors. Carried
/// as data rather than a formatted string so each consumer can attach the
/// glyph name and provenance it knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeriveIssue {
    /// Two surviving anchors with the same name would both be exposed. The
    /// composite exposes neither: a digraph must not pick one side's
    /// attachment point silently. Declare the anchor explicitly instead.
    DuplicateExposed { position: String },
    /// A `-` anchor found more than one size-matching `+` anchor to attach
    /// to. Nothing is attached or consumed.
    AmbiguousAttachment { position: String, ref_name: String },
    /// A `-` anchor found a same-name `+` of a different size (`(w, h)` in
    /// cells) and therefore attached to nothing. A near-miss like this almost
    /// always means the wrong `:narrow`/`:wide` variant — a `-` with no
    /// same-name `+` at all is ordinary forwarding and stays quiet.
    ///
    /// Once reported as a warning, on the reading that the composite still
    /// resolves around it. It does not: the mark sits at the pen instead of
    /// over its base, which is not a glyph anyone meant to ship, and a
    /// warning left the build shipping it anyway.
    SizeMismatchedAttachment {
        position: String,
        ref_name: String,
        minus: (u16, u16),
        plus: (u16, u16),
    },
}

impl DeriveIssue {
    pub(crate) fn message(&self, glyph: &str) -> String {
        match self {
            DeriveIssue::DuplicateExposed { position } => format!(
                "glyph '{glyph}' would expose anchor '{position}' from more than one \
                 source; it exposes neither — declare the anchor explicitly",
            ),
            DeriveIssue::AmbiguousAttachment { position, ref_name } => format!(
                "glyph '{glyph}': ref '{ref_name}' finds more than one '{}{}' anchor \
                 to attach its '{position}' to; nothing is attached",
                "+",
                position.strip_prefix('-').unwrap_or(position),
            ),
            DeriveIssue::SizeMismatchedAttachment {
                position,
                ref_name,
                minus,
                plus,
            } => {
                format!(
                    "glyph '{glyph}': ref '{ref_name}' has '{position}' ({}x{}) matching \
                     a '+{}' only by name ({}x{}); not attached — check the other variants",
                    minus.0,
                    minus.1,
                    position.strip_prefix('-').unwrap_or(position),
                    plus.0,
                    plus.1,
                )
            }
        }
    }
}

/// An anchor in the derivation pool: the point (already translated into the
/// composite's coordinates) and where it came from — `None` for the
/// composite's own declared anchors, `Some(i)` for one published by ref `i`.
type PoolAnchor = (GlyphPoint, Option<usize>);

/// Derive effective ref offsets and the anchors exposed by the resulting
/// composite without changing the source refs.  A target's `-name` anchors
/// consume matching `+name` anchors that are already available, then the
/// target's `+name` anchors are published for following refs.  A target's
/// several `-name` anchors are alternative ways in, so only the first with a
/// candidate is used; see the module docs for what the rest take with them.
///
/// What the composite *exposes* is opt-in: its own declared anchors, plus the
/// surviving anchors (published `+`, unconsumed `-`) of refs marked
/// `inherit`.  Attachment between sibling refs works regardless of the flag.
/// If two survivors of the exposed set share a name, the composite exposes
/// neither and a [`DeriveIssue::DuplicateExposed`] is reported; likewise a
/// `-` anchor with more than one size-matching `+` candidate attaches to
/// nothing and reports [`DeriveIssue::AmbiguousAttachment`].
///
/// Ref order does not matter: when a target carries `-name` and some other
/// still-unresolved ref could yet publish `+name`, resolution is deferred
/// until it does.  A minus anchor no remaining ref can satisfy does *not*
/// defer — deferral would let explicit-offset sibling refs commit first and
/// miss their consumption.  Refs that remain unresolved after the fixpoint
/// fall back to `(0, 0)`.
///
/// `lookup_alternatives` returns sorted alternative glyph names for a base
/// name (e.g. for "foo" it returns ["foo:bar", "foo:baz"]).  When the
/// primary ref's anchors don't size-match, alternatives are tried in order.
///
/// `lookup_declared_anchors` returns a ref target's own declared anchors
/// (not forwarded from its refs).  This enables look-ahead alternative
/// selection: if a later ref would consume `+X` via `-X` and the current
/// ref's declared anchors lack `+X` (it is only forwarded from sub-refs),
/// an alternative that declares `+X` directly is preferred.
///
/// Everything here is grid coordinates, in and out: an anchor is written where
/// the drawing is, so a written offset is converted on the way in by
/// `lookup_origin` and every offset that comes back out is converted once by
/// [`rebase_offsets_to_box`].
pub(crate) fn derive_ref_offsets_with(
    declared_anchors: &[GlyphPoint],
    refs: &[GlyphRef],
    parent_scale: u8,
    lookup_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    lookup_alternatives: impl FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
    lookup_declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    lookup_origin: impl FnMut(&str) -> (i16, i16),
) -> (Vec<GlyphRef>, Vec<PoolAnchor>, Vec<DeriveIssue>) {
    let outcome = derive_ref_offsets_detailed(
        declared_anchors,
        refs,
        parent_scale,
        lookup_anchors,
        lookup_alternatives,
        lookup_declared_anchors,
        lookup_origin,
    );
    (outcome.effective, outcome.exposed, outcome.issues)
}

/// What [`derive_ref_offsets_with`] worked out, plus *how* each ref got its
/// placement.
pub(crate) struct DeriveOutcome {
    pub effective: Vec<GlyphRef>,
    pub exposed: Vec<PoolAnchor>,
    pub issues: Vec<DeriveIssue>,
    /// Per ref: the placement came from an anchor match rather than from the
    /// line. A ref written with an offset is never one of these, and neither
    /// is an offset-less ref that found no anchor and fell back to (0, 0) —
    /// the distinction the two cannot express themselves, and the one a
    /// glyph resize needs: an anchored ref follows its target's anchors on
    /// its own, so its line must be left alone. See
    /// [`crate::editor::glyph_resize`].
    // Only the editor resizes glyphs, and the headless build has no other
    // reader; the flags are still computed there because they fall out of the
    // fixpoint that has to run anyway.
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    pub anchor_placed: Vec<bool>,
}

/// Convert what the derivation worked out from grid coordinates into box
/// coordinates.
///
/// An anchor is a point on the drawing, written where the drawing is — in grid
/// cells — so the whole derivation runs in grid coordinates. Everything
/// downstream, though, reads an offset as naming the child's box corner
/// ([`ref_effective_offset_scaled`], which will subtract that origin again), so
/// it has to be added back here or a ref lands twice-shifted on any target that
/// declares a box.
///
/// Every ref is converted, not only the ones the derivation placed: a written
/// offset was converted the other way on the way *in*, so this returns it
/// unchanged. An alternative may have been substituted, so the origin is looked
/// up by the name that survived.
pub(crate) fn rebase_offsets_to_box(
    effective: &mut [GlyphRef],
    parent_scale: u8,
    mut lookup_origin: impl FnMut(&str) -> (i16, i16),
) {
    let ps = parent_scale.max(1) as i32;
    for eff in effective.iter_mut() {
        let Some((c, r)) = eff.offset else { continue };
        let (child_c, child_r) = lookup_origin(&eff.name);
        eff.offset = Some((
            saturating_i16(c as i32 + child_c as i32 * ps),
            saturating_i16(r as i32 + child_r as i32 * ps),
        ));
    }
}

/// [`derive_ref_offsets_with`] with the anchor-placement flags kept.
pub(crate) fn derive_ref_offsets_detailed(
    declared_anchors: &[GlyphPoint],
    refs: &[GlyphRef],
    parent_scale: u8,
    mut lookup_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut lookup_alternatives: impl FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
    mut lookup_declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut lookup_origin: impl FnMut(&str) -> (i16, i16),
) -> DeriveOutcome {
    let mut issues: Vec<DeriveIssue> = Vec::new();
    let mut survived_minus: Vec<PoolAnchor> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .map(|p| (p.clone(), None))
        .collect();
    let mut available_plus: Vec<PoolAnchor> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .map(|p| (p.clone(), None))
        .collect();

    let target_anchors_list: Vec<Option<Vec<GlyphPoint>>> =
        refs.iter().map(|gref| lookup_anchors(&gref.name)).collect();

    let target_declared_anchors_list: Vec<Option<Vec<GlyphPoint>>> = refs
        .iter()
        .map(|gref| lookup_declared_anchors(&gref.name))
        .collect();

    let alternatives_list: Vec<Vec<(String, Vec<GlyphPoint>)>> = refs
        .iter()
        .map(|gref| {
            if gref.offset.is_some() {
                Vec::new()
            } else {
                lookup_alternatives(&gref.name)
            }
        })
        .collect();

    // A written offset names the target's box corner; an anchor is a point on
    // the target's *grid*. The two only meet once the box term is taken out, so
    // it comes out here, once, and the rest of the derivation is grid-only.
    // A ref written with an offset never takes an alternative, so the origin
    // can be looked up by the name on the line.
    let ps = parent_scale.max(1) as i32;
    let grid_offsets: Vec<Option<(i16, i16)>> = refs
        .iter()
        .map(|gref| {
            gref.offset.map(|(c, r)| {
                let (child_c, child_r) = lookup_origin(&gref.name);
                (
                    saturating_i16(c as i32 - child_c as i32 * ps),
                    saturating_i16(r as i32 - child_r as i32 * ps),
                )
            })
        })
        .collect();

    let n = refs.len();
    let mut effective_refs: Vec<Option<GlyphRef>> = vec![None; n];
    let mut anchor_placed = vec![false; n];

    loop {
        let mut progress = false;
        for (i, gref) in refs.iter().enumerate() {
            if effective_refs[i].is_some() {
                continue;
            }

            let Some(ref target_anchors) = target_anchors_list[i] else {
                effective_refs[i] = Some(gref.clone());
                progress = true;
                continue;
            };

            if let Some(offset) = grid_offsets[i] {
                commit_ref(
                    gref,
                    i,
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            match try_match_minus_plus(target_anchors, &available_plus) {
                MatchOutcome::Unique(offset) => {
                    anchor_placed[i] = true;
                    commit_ref(
                        gref,
                        i,
                        offset,
                        target_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    progress = true;
                    continue;
                }
                MatchOutcome::Ambiguous => {
                    // The attachment is ill-defined; commit unattached and be
                    // loud (commit_ref reports it) rather than silently
                    // swapping in an alternative or picking one candidate.
                    commit_ref(
                        gref,
                        i,
                        (0, 0),
                        target_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    progress = true;
                    continue;
                }
                MatchOutcome::NoMatch => {}
            }

            // Try alternatives when primary doesn't size-match.
            let mut alt_matched = false;
            for (alt_name, alt_anchors) in &alternatives_list[i] {
                if let MatchOutcome::Unique(offset) =
                    try_match_minus_plus(alt_anchors, &available_plus)
                {
                    let alt_gref = GlyphRef {
                        raw_name: None,
                        comment: None,
                        name: alt_name.clone(),
                        offset: None,
                        negated: gref.negated,
                        inherit: gref.inherit,
                        fill: gref.fill.clone(),
                        visibility: gref.visibility,
                    };
                    anchor_placed[i] = true;
                    commit_ref(
                        &alt_gref,
                        i,
                        offset,
                        alt_anchors,
                        &mut available_plus,
                        &mut survived_minus,
                        &mut issues,
                        &mut effective_refs[i],
                    );
                    alt_matched = true;
                    progress = true;
                    break;
                }
            }
            if alt_matched {
                continue;
            }

            // Defer only while some other still-unresolved ref could publish a
            // `+` this candidate's `-` might match. A minus nothing remaining
            // can satisfy (a base's own `-center`, say) must not wait: waiting
            // would let explicit-offset sibling refs commit first and miss the
            // consumption of this ref's `+` anchors.
            let wanted: Vec<&str> = target_anchors
                .iter()
                .chain(alternatives_list[i].iter().flat_map(|(_, a)| a.iter()))
                .filter_map(|p| p.position.strip_prefix('-'))
                .collect();
            let could_match_later = !wanted.is_empty()
                && (0..n).any(|j| {
                    j != i
                        && effective_refs[j].is_none()
                        && target_anchors_list[j]
                            .iter()
                            .flatten()
                            .chain(alternatives_list[j].iter().flat_map(|(_, a)| a.iter()))
                            .filter_map(|p| p.position.strip_prefix('+'))
                            .any(|base| wanted.contains(&base))
                });
            if could_match_later {
                continue;
            }

            // Look-ahead substitution: if this ref publishes +anchor
            // that a subsequent unresolved ref would consume via -anchor,
            // prefer an alternative that provides +anchor directly.
            let alt_found = try_lookahead_alt(
                i,
                n,
                target_anchors,
                target_declared_anchors_list[i].as_deref(),
                &available_plus,
                &alternatives_list[i],
                &effective_refs,
                &target_anchors_list,
            );
            if let Some((alt_name, alt_anchors)) = alt_found {
                let alt_gref = GlyphRef {
                    raw_name: None,
                    comment: None,
                    name: alt_name,
                    offset: None,
                    negated: gref.negated,
                    inherit: gref.inherit,
                    fill: gref.fill.clone(),
                    visibility: gref.visibility,
                };
                commit_ref(
                    &alt_gref,
                    i,
                    (0, 0),
                    alt_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
            } else {
                commit_ref(
                    gref,
                    i,
                    (0, 0),
                    target_anchors,
                    &mut available_plus,
                    &mut survived_minus,
                    &mut issues,
                    &mut effective_refs[i],
                );
            }
            progress = true;
        }
        if !progress {
            break;
        }
    }

    for (i, gref) in refs.iter().enumerate() {
        if effective_refs[i].is_some() {
            continue;
        }
        let target_anchors = target_anchors_list[i].as_deref().unwrap_or(&[]);
        let (resolved_name, offset, used_anchors) = if let Some(offset) = grid_offsets[i] {
            (gref.name.clone(), offset, target_anchors)
        } else {
            match try_match_minus_plus(target_anchors, &available_plus) {
                MatchOutcome::Unique(offset) => {
                    anchor_placed[i] = true;
                    (gref.name.clone(), offset, target_anchors)
                }
                // Ambiguity is reported by commit_ref below; commit unattached.
                MatchOutcome::Ambiguous => (gref.name.clone(), (0, 0), target_anchors),
                MatchOutcome::NoMatch => {
                    let mut found = None;
                    for (alt_name, alt_anchors) in &alternatives_list[i] {
                        if let MatchOutcome::Unique(offset) =
                            try_match_minus_plus(alt_anchors, &available_plus)
                        {
                            anchor_placed[i] = true;
                            found = Some((alt_name.clone(), offset, alt_anchors.as_slice()));
                            break;
                        }
                    }
                    if found.is_none()
                        && let Some((alt_name, alt_anchors)) = try_lookahead_alt(
                            i,
                            n,
                            target_anchors,
                            target_declared_anchors_list[i].as_deref(),
                            &available_plus,
                            &alternatives_list[i],
                            &effective_refs,
                            &target_anchors_list,
                        )
                    {
                        found = Some((alt_name.clone(), (0, 0), alt_anchors));
                    }
                    found.unwrap_or_else(|| (gref.name.clone(), (0, 0), target_anchors))
                }
            }
        };
        let resolved_gref = GlyphRef {
            raw_name: None,
            comment: None,
            name: resolved_name,
            offset: gref.offset,
            negated: gref.negated,
            inherit: gref.inherit,
            fill: gref.fill.clone(),
            visibility: gref.visibility,
        };
        commit_ref(
            &resolved_gref,
            i,
            offset,
            used_anchors,
            &mut available_plus,
            &mut survived_minus,
            &mut issues,
            &mut effective_refs[i],
        );
    }

    let effective_refs: Vec<GlyphRef> = effective_refs.into_iter().map(Option::unwrap).collect();

    // Exposure is opt-in: declared anchors always pass, a ref's survivors only
    // through its `inherit` flag. Survivors sharing a name are then dropped
    // together — the composite must not pick one of them silently. Each
    // exposed anchor keeps its source (`None` = declared, `Some(i)` = ref
    // `i`), which is how the palette colors an inherited anchor like the
    // subglyph it came from.
    let mut exposed: Vec<PoolAnchor> = survived_minus
        .into_iter()
        .chain(available_plus)
        .filter(|(_, source)| source.is_none_or(|i| refs[i].inherit))
        .collect();
    let mut duplicated: Vec<String> = Vec::new();
    for (i, (p, _)) in exposed.iter().enumerate() {
        if exposed[..i].iter().any(|(o, _)| o.position == p.position)
            && !duplicated.contains(&p.position)
        {
            duplicated.push(p.position.clone());
        }
    }
    for position in duplicated {
        exposed.retain(|(p, _)| p.position != position);
        issues.push(DeriveIssue::DuplicateExposed { position });
    }

    DeriveOutcome {
        effective: effective_refs,
        exposed,
        issues,
        anchor_placed,
    }
}

/// Look-ahead alternative selection: when an unresolved sibling would consume
/// `-anchor` and an alternative of the ref at index `i` declares the matching
/// `+anchor` that the ref itself does *not* declare, prefer that alternative.
///
/// This handles cases like `i-lower` (whose `+above` lives on
/// `i-lower:dotless`) followed by `dia-above` (which needs `-above`):
/// `i-lower:dotless` should be used because it is the correct visual form
/// when the above-anchor is consumed (the dot would conflict).  But when
/// followed by `dia-below`, no substitution occurs because `i-lower` has
/// `+below` as its own declared anchor.
///
/// The question is asked of the *alternative*, never of what the primary
/// exposes: `inherit` decides what a glyph offers, not which form of it gets
/// picked. `ttf_builder/gpos.rs`'s base-alternative selection reads
/// `declared_anchors` plus the alternative index for the same reason, and the
/// two must agree — otherwise a `map generate` composite and its typed
/// decomposition render differently, which is what made every generated
/// `ï í ì ī ǐ î ĭ ĩ` keep its dot once anchor inheritance became explicit.
///
/// A second, size-driven trigger substitutes a *publisher* whose `+X`
/// name-matches an unresolved sibling's `-X` but size-mismatches it, when an
/// alternative's `+X` fits — the consumer side cannot adapt there, since its
/// own alternatives were already tried and rejected. Both triggers scan every
/// unresolved sibling, not just the following ones: a deferred consumer
/// *earlier* in the ref list is still waiting on this publisher.
#[expect(clippy::too_many_arguments)]
fn try_lookahead_alt<'a>(
    i: usize,
    n: usize,
    target_anchors: &[GlyphPoint],
    target_declared_anchors: Option<&[GlyphPoint]>,
    available_plus: &[PoolAnchor],
    alternatives: &'a [(String, Vec<GlyphPoint>)],
    effective_refs: &[Option<GlyphRef>],
    target_anchors_list: &[Option<Vec<GlyphPoint>>],
) -> Option<(String, &'a [GlyphPoint])> {
    let unresolved = |j: usize| j != i && effective_refs[j].is_none();

    if let Some(declared) = target_declared_anchors {
        for (alt_name, alt_anchors) in alternatives {
            for plus in alt_anchors.iter().filter(|p| p.position.starts_with('+')) {
                // The primary declares it, so the primary *is* the right form:
                // `i-lower` keeps its dot under `dia-below`.
                if declared.iter().any(|o| o.position == plus.position) {
                    continue;
                }
                // Already published — by the composite itself or by a sibling
                // that committed earlier — so there is nothing to switch for.
                if available_plus
                    .iter()
                    .any(|(a, _)| a.position == plus.position)
                {
                    continue;
                }
                let Some(base) = plus.position.strip_prefix('+') else {
                    continue;
                };
                let minus_name = format!("-{base}");
                let needed = (0..n).any(|j| {
                    unresolved(j)
                        && target_anchors_list[j]
                            .as_deref()
                            .unwrap_or(&[])
                            .iter()
                            .any(|p| p.position == minus_name)
                });
                if needed {
                    return Some((alt_name.clone(), alt_anchors.as_slice()));
                }
            }
        }
    }

    // Size-aware publisher substitution: a `+X` this ref would publish
    // (declared or forwarded) name-matches an unresolved sibling's `-X` but
    // size-mismatches it, and an alternative's `+X` fits that consumer. The
    // consumer cannot adapt — its own alternatives were already tried — so
    // the publisher must: `enclosing-circle:alt` exists exactly for the
    // descenders whose `-center` is one cell, not two.
    for (alt_name, alt_anchors) in alternatives {
        for plus in target_anchors
            .iter()
            .filter(|p| p.position.starts_with('+'))
        {
            let Some(base) = plus.position.strip_prefix('+') else {
                continue;
            };
            let minus_name = format!("-{base}");
            for j in (0..n).filter(|&j| unresolved(j)) {
                for minus in target_anchors_list[j]
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|p| p.position == minus_name)
                {
                    // The primary (or something already published) serves
                    // this consumer: nothing to fix.
                    if plus.size_matches(minus)
                        || available_plus
                            .iter()
                            .any(|(p, _)| p.position == plus.position && p.size_matches(minus))
                    {
                        continue;
                    }
                    if alt_anchors
                        .iter()
                        .any(|a| a.position == plus.position && a.size_matches(minus))
                    {
                        return Some((alt_name.clone(), alt_anchors.as_slice()));
                    }
                }
            }
        }
    }
    None
}

/// What matching a target's `-` anchors against the pool produced.
enum MatchOutcome {
    NoMatch,
    Unique((i16, i16)),
    /// The first `-` anchor with any candidate had more than one — the
    /// attachment is ill-defined, and no other anchor is tried instead.
    Ambiguous,
}

fn try_match_minus_plus(
    target_anchors: &[GlyphPoint],
    available_plus: &[PoolAnchor],
) -> MatchOutcome {
    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        let mut candidates = available_plus
            .iter()
            .filter(|(p, _)| p.position.strip_prefix('+') == Some(base) && p.size_matches(minus));
        let Some((plus, _)) = candidates.next() else {
            continue;
        };
        if candidates.next().is_some() {
            return MatchOutcome::Ambiguous;
        }
        return MatchOutcome::Unique((
            saturating_i16(plus.col as i32 - minus.col as i32),
            saturating_i16(plus.row as i32 - minus.row as i32),
        ));
    }
    MatchOutcome::NoMatch
}

fn translate_point(p: &GlyphPoint, off_col: i16, off_row: i16) -> GlyphPoint {
    GlyphPoint {
        comment: None,
        position: p.position.clone(),
        col: saturating_i16(p.col as i32 + off_col as i32),
        row: saturating_i16(p.row as i32 + off_row as i32),
        col_end: saturating_i16(p.col_end as i32 + off_col as i32),
        row_end: saturating_i16(p.row_end as i32 + off_row as i32),
    }
}

#[expect(clippy::too_many_arguments)]
fn commit_ref(
    gref: &GlyphRef,
    ref_idx: usize,
    offset: (i16, i16),
    target_anchors: &[GlyphPoint],
    available_plus: &mut Vec<PoolAnchor>,
    survived_minus: &mut Vec<PoolAnchor>,
    issues: &mut Vec<DeriveIssue>,
    out: &mut Option<GlyphRef>,
) {
    let effective = GlyphRef {
        raw_name: None,
        comment: None,
        name: gref.name.clone(),
        offset: Some(offset),
        negated: gref.negated,
        inherit: gref.inherit,
        fill: gref.fill.clone(),
        visibility: gref.visibility,
    };
    let off_col = effective.col();
    let off_row = effective.row();

    // Consume before publishing. In particular, a component carrying
    // both `-join` and `+join` must publish its outgoing anchor rather
    // than immediately deleting it again. Consumption needs a *unique*
    // size-matching `+`: more than one means the attachment is ill-defined,
    // so nothing is consumed (and nothing survives) — loudly.
    //
    // Several `-` anchors are *alternatives*, not several attachments: one
    // mark that can adjoin to more than one anchor system (`gr-psili` joins
    // either a Greek `+gr-above` or a plain `+above`). The first one with a
    // candidate decides — the very order `try_match_minus_plus` derived the
    // offset from — and the rest retire without a trace: no survivor, and no
    // near-miss report against a `+` this ref was never going to use. That
    // silence is load-bearing now that a near-miss drops the glyph.
    let minus_anchors: Vec<&GlyphPoint> = target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .collect();
    let mut joined: Option<&str> = None;
    for minus in &minus_anchors {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        let matching: Vec<usize> = available_plus
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| {
                p.position.strip_prefix('+') == Some(base) && p.size_matches(minus)
            })
            .map(|(i, _)| i)
            .collect();
        if matching.is_empty() {
            continue;
        }
        joined = Some(base);
        if matching.len() == 1 {
            available_plus.remove(matching[0]);
        } else {
            issues.push(DeriveIssue::AmbiguousAttachment {
                position: minus.position.clone(),
                ref_name: gref.name.clone(),
            });
        }
        break;
    }
    if joined.is_none() {
        for minus in &minus_anchors {
            let Some(base) = minus.position.strip_prefix('-') else {
                continue;
            };
            // A same-name `+` of the wrong size is a near-miss worth
            // flagging; no same-name `+` at all is plain forwarding.
            if let Some((near, _)) = available_plus
                .iter()
                .find(|(p, _)| p.position.strip_prefix('+') == Some(base))
            {
                issues.push(DeriveIssue::SizeMismatchedAttachment {
                    position: minus.position.clone(),
                    ref_name: gref.name.clone(),
                    minus: (minus.width(), minus.height()),
                    plus: (near.width(), near.height()),
                });
            }
            survived_minus.push((translate_point(minus, off_col, off_row), Some(ref_idx)));
        }
    }
    for plus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
    {
        // A `+` whose `-` partner retired goes with it: having joined one
        // system, the mark offers the next mark only that same system. A `+`
        // with no `-` partner at all is a base's hosting point, untouched —
        // `s-upper` keeps offering `+above` after `+below` was taken.
        if let Some(joined) = joined
            && let Some(base) = plus.position.strip_prefix('+')
            && base != joined
            && minus_anchors
                .iter()
                .any(|m| m.position.strip_prefix('-') == Some(base))
        {
            continue;
        }
        available_plus.push((translate_point(plus, off_col, off_row), Some(ref_idx)));
    }

    *out = Some(effective);
}
