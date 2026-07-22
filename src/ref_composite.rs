use std::collections::HashMap;

use crate::document::{
    Document, DocumentItem, GlyphBody, GlyphPoint, GlyphRef, NamePartsMap, PixelGrid,
    expand_name_pattern, substitute_name_parts,
};

#[cfg(feature = "editor")]
const PHI: f64 = 1.618033988749895;

#[cfg(feature = "editor")]
pub fn ref_color_sv(s: f32, v: f32, index: usize) -> egui::Color32 {
    let hue = ((index + 1) as f64 / PHI % 1.0 * 360.0) as f32;
    hsv_to_rgb(hue, s, v)
}

#[cfg(feature = "editor")]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    egui::Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub struct ResolvedGlyph {
    pub grid: PixelGrid,
    /// Logical coordinate represented by raster cell `(0, 0)`. Keeping this
    /// separate from the raster is essential for nested refs whose bounds
    /// extend left/up from the glyph origin.
    pub(crate) origin_row: i32,
    pub(crate) origin_col: i32,
    pub(crate) resolved_anchors: Vec<GlyphPoint>,
    /// The glyph body's own declared anchor/point lines (not forwarded
    /// from refs).  Used by look-ahead alternative selection.
    pub(crate) declared_anchors: Vec<GlyphPoint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnDemandGlyph {
    Rect(u8, u8),
    ColorMono { mono: String, color: String },
}

/// Parse an on-demand glyph name of the form `WxH` (e.g. `3x5`) into
/// `Rect(width, height)`.  Both W and H must start with a non-zero digit.
pub fn parse_on_demand_glyph(name: &str) -> Option<OnDemandGlyph> {
    let (w_str, h_str) = name.split_once('x')?;
    if w_str.is_empty()
        || h_str.is_empty()
        || w_str.starts_with('0')
        || h_str.starts_with('0')
    {
        return None;
    }
    let w: u8 = w_str.parse().ok()?;
    let h: u8 = h_str.parse().ok()?;
    Some(OnDemandGlyph::Rect(w, h))
}

/// Detect an on-demand color/mono glyph: name X is not defined, but both
/// X:mono and X:color exist.  X itself must not contain `:mono` or `:color`.
pub fn detect_color_mono_glyph(name: &str, has_glyph: impl Fn(&str) -> bool) -> Option<OnDemandGlyph> {
    if name.contains(":mono") || name.contains(":color") {
        return None;
    }
    let mono = format!("{name}:mono");
    let color = format!("{name}:color");
    if has_glyph(&mono) && has_glyph(&color) {
        Some(OnDemandGlyph::ColorMono { mono, color })
    } else {
        None
    }
}

fn make_on_demand_resolved(w: u8, h: u8) -> ResolvedGlyph {
    let mut grid = PixelGrid::new(w as u16, h as u16);
    for r in 0..h as u16 {
        for c in 0..w as u16 {
            grid.set(r, c, crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true));
        }
    }
    ResolvedGlyph {
        grid,
        origin_row: 0,
        origin_col: 0,
        resolved_anchors: Vec::new(),
        declared_anchors: Vec::new(),
    }
}

fn saturating_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn raster_dimension(min: i32, max: i32) -> u16 {
    max.saturating_sub(min).clamp(0, u16::MAX as i32) as u16
}

/// Expansion used by runtime ref lookup and dependency resolution.
pub(crate) fn expand_ref_names(name: &str) -> Option<Vec<String>> {
    expand_name_pattern(name).ok().map(|names| names.into_vec())
}

/// Derive effective ref offsets and the anchors exposed by the resulting
/// composite without changing the source refs.  A target's `-name` anchors
/// consume matching `+name` anchors that are already available, then the
/// target's `+name` anchors are published for following refs.  Unconsumed
/// minus anchors remain exposed so aliases/composites forward anchors from
/// their targets.
///
/// Ref order does not matter: when a target carries `-name` but no matching
/// `+name` is available yet, resolution is deferred until a later ref
/// publishes it.  Refs that remain unresolved after the fixpoint fall back
/// to `(0, 0)`.
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
pub(crate) fn derive_ref_offsets_with(
    declared_anchors: &[GlyphPoint],
    refs: &[GlyphRef],
    mut lookup_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
    mut lookup_alternatives: impl FnMut(&str) -> Vec<(String, Vec<GlyphPoint>)>,
    mut lookup_declared_anchors: impl FnMut(&str) -> Option<Vec<GlyphPoint>>,
) -> (Vec<GlyphRef>, Vec<GlyphPoint>) {
    let mut exposed_minus: Vec<GlyphPoint> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .cloned()
        .collect();
    let mut available_plus: Vec<GlyphPoint> = declared_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .cloned()
        .collect();

    let target_anchors_list: Vec<Option<Vec<GlyphPoint>>> = refs
        .iter()
        .map(|gref| lookup_anchors(&gref.name))
        .collect();

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

    let n = refs.len();
    let mut effective_refs: Vec<Option<GlyphRef>> = vec![None; n];

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

            if let Some(offset) = gref.offset {
                commit_ref(
                    gref,
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut exposed_minus,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            if let Some(offset) = try_match_minus_plus(target_anchors, &available_plus) {
                commit_ref(
                    gref,
                    offset,
                    target_anchors,
                    &mut available_plus,
                    &mut exposed_minus,
                    &mut effective_refs[i],
                );
                progress = true;
                continue;
            }

            // Try alternatives when primary doesn't size-match.
            let mut alt_matched = false;
            for (alt_name, alt_anchors) in &alternatives_list[i] {
                if let Some(offset) = try_match_minus_plus(alt_anchors, &available_plus) {
                    let alt_gref = GlyphRef {
                        name: alt_name.clone(),
                        offset: None,
                        negated: gref.negated,
                        fill: gref.fill.clone(),
                        visibility: gref.visibility,
                    };
                    commit_ref(
                        &alt_gref,
                        offset,
                        alt_anchors,
                        &mut available_plus,
                        &mut exposed_minus,
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

            // Defer if any candidate has minus anchors (might match later).
            let has_minus = target_anchors
                .iter()
                .any(|p| p.position.starts_with('-'))
                || alternatives_list[i]
                    .iter()
                    .any(|(_, a)| a.iter().any(|p| p.position.starts_with('-')));
            if has_minus {
                continue;
            }

            // Look-ahead substitution: if this ref publishes +anchor
            // that a subsequent unresolved ref would consume via -anchor,
            // prefer an alternative that provides +anchor directly.
            let alt_found = try_lookahead_alt(
                i, n, target_anchors,
                target_declared_anchors_list[i].as_deref(),
                &available_plus,
                &alternatives_list[i], &effective_refs, &target_anchors_list,
            );
            if let Some((alt_name, alt_anchors)) = alt_found {
                let alt_gref = GlyphRef {
                    name: alt_name,
                    offset: None,
                    negated: gref.negated,
                    fill: gref.fill.clone(),
                    visibility: gref.visibility,
                };
                commit_ref(
                    &alt_gref,
                    (0, 0),
                    alt_anchors,
                    &mut available_plus,
                    &mut exposed_minus,
                    &mut effective_refs[i],
                );
            } else {
                commit_ref(
                    gref,
                    (0, 0),
                    target_anchors,
                    &mut available_plus,
                    &mut exposed_minus,
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
        let (resolved_name, offset, used_anchors) = if let Some(offset) = gref.offset {
            (gref.name.clone(), offset, target_anchors)
        } else {
            match try_match_minus_plus(target_anchors, &available_plus) {
                Some(offset) => (gref.name.clone(), offset, target_anchors),
                None => {
                    let mut found = None;
                    for (alt_name, alt_anchors) in &alternatives_list[i] {
                        if let Some(offset) = try_match_minus_plus(alt_anchors, &available_plus) {
                            found = Some((alt_name.clone(), offset, alt_anchors.as_slice()));
                            break;
                        }
                    }
                    if found.is_none()
                        && let Some((alt_name, alt_anchors)) = try_lookahead_alt(
                            i, n, target_anchors,
                            target_declared_anchors_list[i].as_deref(),
                            &available_plus,
                            &alternatives_list[i], &effective_refs, &target_anchors_list,
                        ) {
                            found = Some((alt_name.clone(), (0, 0), alt_anchors));
                        }
                    found.unwrap_or_else(|| (gref.name.clone(), (0, 0), target_anchors))
                }
            }
        };
        let resolved_gref = GlyphRef {
            name: resolved_name,
            offset: gref.offset,
            negated: gref.negated,
            fill: gref.fill.clone(),
            visibility: gref.visibility,
        };
        commit_ref(
            &resolved_gref,
            offset,
            used_anchors,
            &mut available_plus,
            &mut exposed_minus,
            &mut effective_refs[i],
        );
    }

    let effective_refs = effective_refs.into_iter().map(Option::unwrap).collect();
    exposed_minus.extend(available_plus);
    (effective_refs, exposed_minus)
}

/// Look-ahead alternative selection: when a ref at index `i` publishes
/// `+anchor` that a subsequent unresolved ref would consume via `-anchor`,
/// AND the ref's own declared points do NOT include that `+anchor` (it is
/// only forwarded from the ref's own sub-refs), prefer an alternative that
/// provides `+anchor` directly.
///
/// This handles cases like `i-lower` (which forwards `+above` from
/// `ref i-lower:dotless`) followed by `dia-above` (which needs `-above`):
/// `i-lower:dotless` should be used because it is the correct visual form
/// when the above-anchor is consumed (the dot would conflict).  But when
/// followed by `dia-below`, no substitution occurs because `i-lower` has
/// `+below` as its own declared anchor.
fn try_lookahead_alt<'a>(
    i: usize,
    n: usize,
    target_anchors: &[GlyphPoint],
    target_declared_anchors: Option<&[GlyphPoint]>,
    available_plus: &[GlyphPoint],
    alternatives: &'a [(String, Vec<GlyphPoint>)],
    effective_refs: &[Option<GlyphRef>],
    target_anchors_list: &[Option<Vec<GlyphPoint>>],
) -> Option<(String, &'a [GlyphPoint])> {
    let declared = target_declared_anchors?;
    let plus_anchors: Vec<&GlyphPoint> = target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
        .filter(|p| !available_plus.iter().any(|a| a.position == p.position))
        .filter(|p| !declared.iter().any(|o| o.position == p.position))
        .collect();
    if plus_anchors.is_empty() {
        return None;
    }
    for (alt_name, alt_anchors) in alternatives {
        for plus in &plus_anchors {
            let base = plus.position.strip_prefix('+')?;
            let minus_name = format!("-{base}");
            let needed_by_later = (i + 1..n).any(|j| {
                effective_refs[j].is_none()
                    && target_anchors_list[j]
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .any(|p| p.position == minus_name)
            });
            if needed_by_later
                && alt_anchors.iter().any(|a| a.position == plus.position)
            {
                return Some((alt_name.clone(), alt_anchors.as_slice()));
            }
        }
    }
    None
}

fn try_match_minus_plus(
    target_anchors: &[GlyphPoint],
    available_plus: &[GlyphPoint],
) -> Option<(i16, i16)> {
    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let base = minus.position.strip_prefix('-')?;
        if let Some(plus) = available_plus
            .iter()
            .find(|p| p.position.strip_prefix('+') == Some(base) && p.size_matches(minus))
        {
            return Some((
                saturating_i16(plus.col as i32 - minus.col as i32),
                saturating_i16(plus.row as i32 - minus.row as i32),
            ));
        }
    }
    None
}

fn commit_ref(
    gref: &GlyphRef,
    offset: (i16, i16),
    target_anchors: &[GlyphPoint],
    available_plus: &mut Vec<GlyphPoint>,
    exposed_minus: &mut Vec<GlyphPoint>,
    out: &mut Option<GlyphRef>,
) {
    let effective = GlyphRef {
        name: gref.name.clone(),
        offset: Some(offset),
        negated: gref.negated,
        fill: gref.fill.clone(),
        visibility: gref.visibility,
    };
    let off_col = effective.col();
    let off_row = effective.row();

    // Consume before publishing. In particular, a component carrying
    // both `-join` and `+join` must publish its outgoing anchor rather
    // than immediately deleting it again.
    let consumed_names: Vec<String> = target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
        .filter_map(|minus| {
            let base = minus.position.strip_prefix('-')?;
            if available_plus.iter().any(|p| {
                p.position.strip_prefix('+') == Some(base) && p.size_matches(minus)
            }) {
                Some(base.to_string())
            } else {
                None
            }
        })
        .collect();
    available_plus.retain(|p| {
        !p.position
            .strip_prefix('+')
            .is_some_and(|base| consumed_names.iter().any(|n| n == base))
    });

    for minus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('-'))
    {
        let Some(base) = minus.position.strip_prefix('-') else {
            continue;
        };
        if !consumed_names.iter().any(|n| n == base) {
            exposed_minus.push(GlyphPoint {
                position: minus.position.clone(),
                col: saturating_i16(minus.col as i32 + off_col as i32),
                row: saturating_i16(minus.row as i32 + off_row as i32),
                col_end: saturating_i16(minus.col_end as i32 + off_col as i32),
                row_end: saturating_i16(minus.row_end as i32 + off_row as i32),
            });
        }
    }
    for plus in target_anchors
        .iter()
        .filter(|p| p.position.starts_with('+'))
    {
        let Some(base) = plus.position.strip_prefix('+') else {
            continue;
        };
        if !available_plus
            .iter()
            .any(|p| p.position.strip_prefix('+') == Some(base))
        {
            available_plus.push(GlyphPoint {
                position: plus.position.clone(),
                col: saturating_i16(plus.col as i32 + off_col as i32),
                row: saturating_i16(plus.row as i32 + off_row as i32),
                col_end: saturating_i16(plus.col_end as i32 + off_col as i32),
                row_end: saturating_i16(plus.row_end as i32 + off_row as i32),
            });
        }
    }

    *out = Some(effective);
}

/// Detect any on-demand glyph for `name`: first tries WxH rect, then
/// color/mono composite (checking whether X:mono and X:color exist).
pub fn detect_on_demand_glyph(name: &str, has_glyph: impl Fn(&str) -> bool) -> Option<OnDemandGlyph> {
    parse_on_demand_glyph(name).or_else(|| detect_color_mono_glyph(name, has_glyph))
}

/// Scan documents for on-demand glyph names referenced in refs, maps,
/// and remaps, and insert synthetic glyphs into the cache for any that
/// aren't already defined.
fn inject_on_demand_glyphs(
    docs: &[&Document],
    cache: &mut HashMap<String, ResolvedGlyph>,
) {
    let mut needed: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut consider = |name: &str| {
        if !cache.contains_key(name) && parse_on_demand_glyph(name).is_some() {
            needed.insert(name.to_string());
        }
    };

    for doc in docs {
        for item in &doc.items {
            match item {
                DocumentItem::Glyph { body, .. } => {
                    for r in &body.refs {
                        consider(&r.name);
                    }
                }
                DocumentItem::Map { glyph, .. } => consider(glyph),
                DocumentItem::Remap {
                    lookbehind,
                    source,
                    target,
                    lookahead,
                    ..
                } => {
                    for token in source {
                        consider(token);
                    }
                    for token in target {
                        consider(token);
                    }
                    for lb in lookbehind {
                        consider(lb);
                    }
                    for la in lookahead {
                        consider(la);
                    }
                }
                _ => {}
            }
        }
    }

    for name in &needed {
        if let Some(OnDemandGlyph::Rect(w, h)) = parse_on_demand_glyph(name) {
            cache.entry(name.clone()).or_insert_with(|| make_on_demand_resolved(w, h));
        }
    }
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn resolve_named_glyphs_with_parts(
    docs: &[&Document],
    name_parts: &NamePartsMap,
) -> (HashMap<String, ResolvedGlyph>, AlternativesIndex) {
    let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();

    struct Pending {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
    }

    let mut pending: Vec<Pending> = Vec::new();

    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Glyph { name, body } = item {
                let raw_key = name.display();
                let key = substitute_name_parts(&raw_key, name_parts);
                let expanded_keys = expand_ref_names(&key);
                let expanded_count = expanded_keys.as_ref().map_or(1, |e| e.len());
                if expanded_count <= 1 {
                    if !cache.contains_key(&key) && !pending.iter().any(|p| p.name == key) {
                        if body.refs.is_empty() {
                            cache.insert(
                                key,
                                ResolvedGlyph {
                                    grid: body
                                        .pixels
                                        .clone()
                                        .unwrap_or_else(|| PixelGrid::new(0, 0)),
                                    origin_row: 0,
                                    origin_col: 0,
                                    resolved_anchors: body.points.clone(),
                                    declared_anchors: body.points.clone(),
                                },
                            );
                        } else {
                            let subs_refs: Vec<GlyphRef> = body
                                .refs
                                .iter()
                                .map(|r| GlyphRef {
                                    name: substitute_name_parts(&r.name, name_parts),
                                    offset: r.offset,
                                    negated: r.negated,
                                    fill: r.fill.clone(),
                                    visibility: r.visibility,
                                })
                                .collect();

                            pending.push(Pending {
                                name: key,
                                pixels: body.pixels.clone(),
                                refs: subs_refs,
                                points: body.points.clone(),
                            });
                        }
                    }
                } else {
                    let expanded_keys = expanded_keys.unwrap();
                    let ref_expansions: Vec<Option<_>> = body
                        .refs
                        .iter()
                        .map(|r| {
                            let subst = substitute_name_parts(&r.name, name_parts);
                            expand_ref_names(&subst)
                        })
                        .collect();
                    for (k, expanded_name) in expanded_keys.into_iter().enumerate() {
                        if cache.contains_key(&expanded_name)
                            || pending.iter().any(|p| p.name == expanded_name)
                        {
                            continue;
                        }
                        let expanded_refs: Vec<GlyphRef> = body
                            .refs
                            .iter()
                            .enumerate()
                            .map(|(ri, r)| {
                                let rname = ref_expansions[ri]
                                    .as_ref()
                                    .and_then(|e| {
                                        if e.len() > 1 {
                                            e.get(k % e.len()).cloned()
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or_else(|| substitute_name_parts(&r.name, name_parts));
                                GlyphRef {
                                    name: rname,
                                    offset: r.offset,
                                    negated: r.negated,
                                    fill: r.fill.clone(),
                                    visibility: r.visibility,
                                }
                            })
                            .collect();
                        if expanded_refs.is_empty() {
                            cache.insert(
                                expanded_name,
                                ResolvedGlyph {
                                    grid: body
                                        .pixels
                                        .clone()
                                        .unwrap_or_else(|| PixelGrid::new(0, 0)),
                                    origin_row: 0,
                                    origin_col: 0,
                                    resolved_anchors: body.points.clone(),
                                    declared_anchors: body.points.clone(),
                                },
                            );
                        } else {
                            pending.push(Pending {
                                name: expanded_name,
                                pixels: body.pixels.clone(),
                                refs: expanded_refs,
                                points: body.points.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    inject_on_demand_glyphs(docs, &mut cache);

    // Collect all referenced glyph names that are not yet defined.
    {
        let mut all_referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
        for doc in docs {
            for item in &doc.items {
                match item {
                    DocumentItem::Glyph { body, .. } => {
                        for r in &body.refs {
                            let subst = substitute_name_parts(&r.name, name_parts);
                            all_referenced.insert(subst);
                        }
                    }
                    DocumentItem::Map { glyph, .. } => {
                        all_referenced.insert(substitute_name_parts(glyph, name_parts));
                    }
                    DocumentItem::Remap { lookbehind, source, target, lookahead, .. } => {
                        for token in source.iter().chain(target).chain(lookbehind).chain(lookahead) {
                            all_referenced.insert(substitute_name_parts(token, name_parts));
                        }
                    }
                    _ => {}
                }
            }
        }

        let all_defined: std::collections::HashSet<String> = cache.keys().cloned()
            .chain(pending.iter().map(|p| p.name.clone()))
            .collect();

        // Build a lookup of glyph bodies from documents for color/mono source glyphs.
        let mut glyph_bodies: HashMap<String, &GlyphBody> = HashMap::new();
        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Glyph { name, body } = item {
                    let key = substitute_name_parts(&name.display(), name_parts);
                    glyph_bodies.entry(key).or_insert(body);
                }
            }
        }

        for name in &all_referenced {
            if all_defined.contains(name) {
                continue;
            }
            if let Some(OnDemandGlyph::ColorMono { mono, color }) =
                detect_color_mono_glyph(name, |n| all_defined.contains(n) || glyph_bodies.contains_key(n))
            {
                let mono_body = glyph_bodies.get(&mono);
                let color_body = glyph_bodies.get(&color);
                if let (Some(mono_body), Some(color_body)) = (mono_body, color_body) {
                    let mut refs = Vec::new();
                    for r in &mono_body.refs {
                        refs.push(GlyphRef {
                            name: substitute_name_parts(&r.name, name_parts),
                            offset: r.offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: Some(crate::document::LayerVisibility::MonoOnly),
                        });
                    }
                    for r in &color_body.refs {
                        refs.push(GlyphRef {
                            name: substitute_name_parts(&r.name, name_parts),
                            offset: r.offset,
                            negated: r.negated,
                            fill: r.fill.clone(),
                            visibility: Some(crate::document::LayerVisibility::ColorOnly),
                        });
                    }
                    let mut points = Vec::new();
                    points.extend_from_slice(&mono_body.points);
                    points.extend_from_slice(&color_body.points);
                    pending.push(Pending {
                        name: name.clone(),
                        pixels: None,
                        refs,
                        points,
                    });
                }
            }
        }
    }

    let mut progress = true;
    while progress {
        progress = false;
        let alt_index = AlternativesIndex::build(&cache);
        pending.retain(|pg| {
            if !pg
                .refs
                .iter()
                .all(|r| resolve_ref_name_with_parts(&r.name, &cache, name_parts).is_some())
            {
                return true;
            }
            let (effective_refs, anchors) =
                derive_ref_offsets_with(
                    &pg.points,
                    &pg.refs,
                    |name| {
                        resolve_ref_name_with_parts(name, &cache, name_parts)
                            .map(|resolved| resolved.resolved_anchors.clone())
                    },
                    |name| alt_index.get(name).to_vec(),
                    |name| {
                        resolve_ref_name_with_parts(name, &cache, name_parts)
                            .map(|resolved| resolved.declared_anchors.clone())
                    },
                );
            let (min_r, min_c, _, _) =
                composite_bounds(pg.pixels.as_ref(), &effective_refs, &cache, name_parts);
            let grid = composite_to_grid(&pg.pixels, &effective_refs, &cache, name_parts);
            cache.insert(
                pg.name.clone(),
                ResolvedGlyph {
                    grid,
                    origin_row: min_r,
                    origin_col: min_c,
                    resolved_anchors: anchors,
                    declared_anchors: pg.points.clone(),
                },
            );
            progress = true;
            false
        });
    }

    let alt_index = AlternativesIndex::build(&cache);
    (cache, alt_index)
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct GlyphComposite {
    pub width: u16,
    pub height: u16,
    pub own_offset_row: i16,
    pub own_offset_col: i16,
    pub layers: Vec<CompositeLayer>,
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
impl GlyphComposite {
    pub fn any_layer_filled_at(&self, composite_row: i16, composite_col: i16) -> bool {
        let mut filled = false;
        for layer in &self.layers {
            let lr = composite_row - layer.offset_row;
            let lc = composite_col - layer.offset_col;
            if lr >= 0
                && lr < layer.grid.height as i16
                && lc >= 0
                && lc < layer.grid.width as i16
                && layer.grid.get(lr as u16, lc as u16).is_filled()
            {
                filled = !layer.negated;
            }
        }
        filled
    }
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct CompositeLayer {
    pub ref_idx: usize,
    /// The resolved name (may differ from the source ref if an alternative was chosen).
    pub resolved_name: String,
    pub grid: PixelGrid,
    pub offset_row: i16,
    pub offset_col: i16,
    /// The ref placement in the owning glyph's logical coordinate space.
    /// This differs from `offset_*` when the resolved target has a negative
    /// logical origin.
    pub logical_offset_row: i16,
    pub logical_offset_col: i16,
    pub negated: bool,
    #[cfg(feature = "editor")]
    pub fill_color: Option<egui::Color32>,
}

pub fn resolve_ref_name<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
) -> Option<&'a ResolvedGlyph> {
    resolve_ref_name_with_parts(name, named_glyphs, &NamePartsMap::new())
}

pub fn resolve_ref_name_with_parts<'a>(
    name: &str,
    named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> Option<&'a ResolvedGlyph> {
    if let Some(resolved) = named_glyphs.get(name) {
        return Some(resolved);
    }
    let subst = substitute_name_parts(name, name_parts);
    if let Some(resolved) = named_glyphs.get(&subst) {
        return Some(resolved);
    }
    if let Some(expanded) = expand_ref_names(&subst)
        && let Some(first) = expanded.first()
    {
        return named_glyphs.get(first);
    }
    None
}

/// Check that a ref name resolves to valid glyphs. For pattern refs, ALL
/// expansions must exist; returns false if any expansion is missing.
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn is_ref_valid(
    name: &str,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> bool {
    if named_glyphs.contains_key(name) {
        return true;
    }
    if parse_on_demand_glyph(name).is_some() {
        return true;
    }
    let subst = substitute_name_parts(name, name_parts);
    if named_glyphs.contains_key(&subst) {
        return true;
    }
    if parse_on_demand_glyph(&subst).is_some() {
        return true;
    }
    if let Some(expanded) = expand_ref_names(&subst) {
        return expanded
            .into_iter()
            .all(|n| named_glyphs.contains_key(&n) || parse_on_demand_glyph(&n).is_some());
    }
    false
}

/// Pre-built index mapping each base name to its sorted alternatives.
/// For glyph "foo:bar", entries are added under base "foo".
/// For "foo:bar:baz", entries are added under "foo" and "foo:bar".
#[derive(Clone, Debug, Default)]
pub struct AlternativesIndex {
    map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>>,
}

impl AlternativesIndex {
    pub fn build(named_glyphs: &HashMap<String, ResolvedGlyph>) -> Self {
        let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
        for (name, resolved) in named_glyphs {
            let mut prefix = name.as_str();
            while let Some(colon_pos) = prefix.rfind(':') {
                prefix = &prefix[..colon_pos];
                map.entry(prefix.to_string())
                    .or_default()
                    .push((name.clone(), resolved.resolved_anchors.clone()));
            }
        }
        for alts in map.values_mut() {
            alts.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        Self { map }
    }

    pub fn get(&self, base_name: &str) -> &[(String, Vec<GlyphPoint>)] {
        self.map.get(base_name).map_or(&[], |v| v.as_slice())
    }
}

/// The effective (row, col) offset of a resolved ref within its owning glyph.
pub(crate) fn ref_effective_offset(gref: &GlyphRef, resolved: &ResolvedGlyph) -> (i32, i32) {
    (
        gref.row() as i32 + resolved.origin_row,
        gref.col() as i32 + resolved.origin_col,
    )
}

/// Bounding box (min_row, min_col, max_row, max_col) of a composite made of
/// `own_pixels` (if any) plus `refs`, each resolved against `named_glyphs`
/// via [`resolve_ref_name`] (which falls back to pattern expansion when a
/// ref name isn't a direct cache key).
pub(crate) fn composite_bounds(
    own_pixels: Option<&PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> (i32, i32, i32, i32) {
    let mut min_r: i32 = 0;
    let mut min_c: i32 = 0;
    let mut max_r: i32 = 0;
    let mut max_c: i32 = 0;

    if let Some(grid) = own_pixels {
        max_r = grid.height as i32;
        max_c = grid.width as i32;
    }

    for gref in refs {
        let resolved = if name_parts.is_empty() {
            resolve_ref_name(&gref.name, named_glyphs)
        } else {
            resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts)
        };
        if let Some(resolved) = resolved {
            let (eff_row, eff_col) = ref_effective_offset(gref, resolved);
            if resolved.grid.width != 0 && resolved.grid.height != 0 {
                min_r = min_r.min(eff_row);
                min_c = min_c.min(eff_col);
                max_r = max_r.max(eff_row + resolved.grid.height as i32);
                max_c = max_c.max(eff_col + resolved.grid.width as i32);
            }
        }
    }

    (min_r, min_c, max_r, max_c)
}

#[cfg(feature = "editor")]
fn resolve_fill_display_color(
    fill: &crate::document::RefFill,
    aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<egui::Color32> {
    if fill.color == "fg" {
        return None;
    }
    if fill.color.starts_with('#') {
        let rgba = crate::render::ttf_builder::parse_hex_color(&fill.color)?;
        return Some(egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a));
    }
    let (rgba, _) = aliases.get(&fill.color)?;
    Some(egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a))
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn compute_composite(
    body: &GlyphBody,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    alt_index: &AlternativesIndex,
    color_aliases: &crate::render::ttf_builder::ColorAliasMap,
) -> Option<GlyphComposite> {
    if body.refs.is_empty() {
        return None;
    }

    let (effective_refs, _) = derive_ref_offsets_with(
        &body.points,
        &body.refs,
        |name| {
            resolve_ref_name_with_parts(name, named_glyphs, name_parts)
                .map(|resolved| resolved.resolved_anchors.clone())
        },
        |name| alt_index.get(name).to_vec(),
        |name| {
            resolve_ref_name_with_parts(name, named_glyphs, name_parts)
                .map(|resolved| resolved.declared_anchors.clone())
        },
    );

    let (min_r, min_c, max_r, max_c) = composite_bounds(
        body.pixels.as_ref(),
        &effective_refs,
        named_glyphs,
        name_parts,
    );

    let width = raster_dimension(min_c, max_c).max(1);
    let height = raster_dimension(min_r, max_r).max(1);

    let mut layers = Vec::new();
    for (idx, gref) in effective_refs.iter().enumerate() {
        if let Some(resolved) = resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts) {
            let (raster_row, raster_col) = ref_effective_offset(gref, resolved);
            let orig_ref = &body.refs[idx.min(body.refs.len() - 1)];
            #[cfg(feature = "editor")]
            let fill_color = orig_ref.fill.as_ref().and_then(|f| resolve_fill_display_color(f, color_aliases));
            #[cfg(not(feature = "editor"))]
            {
                let _ = color_aliases;
                let _ = &orig_ref.fill;
            }
            layers.push(CompositeLayer {
                ref_idx: idx,
                resolved_name: gref.name.clone(),
                grid: resolved.grid.clone(),
                offset_row: saturating_i16(raster_row - min_r),
                offset_col: saturating_i16(raster_col - min_c),
                logical_offset_row: gref.row(),
                logical_offset_col: gref.col(),
                negated: gref.negated,
                #[cfg(feature = "editor")]
                fill_color,
            });
        }
    }

    Some(GlyphComposite {
        width,
        height,
        own_offset_row: saturating_i16(-min_r),
        own_offset_col: saturating_i16(-min_c),
        layers,
    })
}

fn composite_to_grid(
    own_pixels: &Option<PixelGrid>,
    refs: &[GlyphRef],
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> PixelGrid {
    let (min_r, min_c, max_r, max_c) =
        composite_bounds(own_pixels.as_ref(), refs, named_glyphs, name_parts);

    let width = raster_dimension(min_c, max_c);
    let height = raster_dimension(min_r, max_r);
    let mut result = PixelGrid::new(width, height);

    // The owning glyph is the initial canvas. Refs are then applied in
    // document order, so a negated ref can actually cut the owning pixels.
    if let Some(grid) = own_pixels {
        result.blit(grid, -min_r, -min_c, false);
    }

    for gref in refs {
        if let Some(resolved) = resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts) {
            let (eff_row, eff_col) = ref_effective_offset(gref, resolved);
            result.blit(&resolved.grid, eff_row - min_r, eff_col - min_c, gref.negated);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::PixelShape;

    fn filled_grid(w: u16, h: u16) -> PixelGrid {
        let mut g = PixelGrid::new(w, h);
        for r in 0..h {
            for c in 0..w {
                g.set(r, c, PixelShape::new(0, true));
            }
        }
        g
    }

    /// `compute_composite` resolves ref names via `resolve_ref_name`, which
    /// falls back to `expand_name_pattern` when a direct cache lookup misses
    /// (e.g. a ref pointing at a pattern name like "digit(0|1)" whose
    /// expansions, not the raw pattern string, are the cache keys).
    /// `composite_to_grid` used to do a bare `cache.get(&gref.name)` with no
    /// such fallback, so the same ref would render live via
    /// `compute_composite` but silently drop out of the flattened grid
    /// produced by `composite_to_grid`.
    #[test]
    fn composite_to_grid_resolves_pattern_refs_like_compute_composite() {
        let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();
        cache.insert(
            "digit0".to_string(),
            ResolvedGlyph {
                grid: filled_grid(2, 2),
                origin_row: 0,
                origin_col: 0,
                resolved_anchors: Vec::new(),
                declared_anchors: Vec::new(),
            },
        );

        let refs = vec![GlyphRef {
            name: "digit(0|1)".to_string(),
            offset: None,
            negated: false,
            fill: None,
            visibility: None,
        }];

        // compute_composite resolves the pattern ref via resolve_ref_name's
        // fallback and includes the layer.
        let body = GlyphBody {
            refs: refs.clone(),
            ..GlyphBody::new()
        };
        let empty_parts = NamePartsMap::new();
        let composite = compute_composite(&body, &cache, &empty_parts, &AlternativesIndex::default(), &Default::default()).expect("has refs");
        assert_eq!(
            composite.layers.len(),
            1,
            "compute_composite should include the pattern-resolved layer"
        );

        // composite_to_grid must resolve the same ref the same way, and thus
        // produce a non-empty grid with the layer's pixels present.
        let grid = composite_to_grid(&None, &refs, &cache, &empty_parts);
        assert_eq!(
            grid.get(0, 0),
            PixelShape::new(0, true),
            "composite_to_grid should include the pattern-resolved layer's pixels"
        );
    }

    #[test]
    fn adjoin_resolves_offset_from_points() {
        use crate::document_io;

        let input = "\
glyph target 10 10
....................
....................
....................
....................
....................
....................
....................
....................
....................
....................
point -blah 5 5

glyph container 12 12
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
point +blah 3 3
ref target
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();

        let docs = vec![&doc];
        let name_parts = NamePartsMap::new();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

        let container = resolved
            .get("container")
            .expect("container should be resolved");
        // target placed at offset (col=3-5, row=3-5) = (-2, -2).
        // Container own pixels 12×12 at (0,0), target 10×10 at (-2,-2).
        // Bounding box: min=-2, max=12 → total 14×14.
        assert_eq!(
            container.grid.width, 14,
            "width should be 14 (12 + 2 for negative offset)"
        );
        assert_eq!(
            container.grid.height, 14,
            "height should be 14 (12 + 2 for negative offset)"
        );
    }

    #[test]
    fn auto_offsets_are_rederived_without_mutating_source_refs() {
        use crate::document_io;

        let input = "\
glyph target 1 1
@@
point -join 0 0

glyph container 1 1
..
point +join 3 0
ref target
";
        let mut doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let name_parts = NamePartsMap::new();

        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        assert_eq!(resolved["container"].grid.width, 4);
        let container_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
                _ => None,
        })
        .unwrap();
        assert_eq!(container_body.refs[0].offset, None);
        let composite = compute_composite(container_body, &resolved, &name_parts, &_alt_idx, &Default::default()).unwrap();
        assert_eq!(
            (
                composite.layers[0].offset_row - composite.own_offset_row,
                composite.layers[0].offset_col - composite.own_offset_col,
            ),
            (0, 3)
        );

        let target_body = doc
            .items
            .iter_mut()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "target" => Some(body),
                _ => None,
            })
            .unwrap();
        target_body.points[0].col = 2;
        target_body.points[0].col_end = 2;

        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);
        assert_eq!(resolved["container"].grid.width, 2);
        let container_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
                _ => None,
        })
        .unwrap();
        assert_eq!(container_body.refs[0].offset, None);
        let composite = compute_composite(container_body, &resolved, &name_parts, &_alt_idx, &Default::default()).unwrap();
        assert_eq!(
            (
                composite.layers[0].offset_row - composite.own_offset_row,
                composite.layers[0].offset_col - composite.own_offset_col,
            ),
            (0, 1)
        );
    }

    #[test]
    fn anchors_are_forwarded_transitively_and_publish_after_consume() {
        use crate::document_io;

        let input = "\
glyph link 1 1
@@
point -join 0 0
point +join 2 0

glyph wrapped
ref link

glyph chain 1 1
..
point +join 0 0
ref wrapped
ref wrapped
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
        assert_eq!(resolved["chain"].grid.width, 3);
        assert!(resolved["chain"].grid.get(0, 0).is_filled());
        assert!(resolved["chain"].grid.get(0, 2).is_filled());
    }

    #[test]
    fn substituted_and_pattern_refs_resolve_in_all_container_shapes() {
        use crate::document_io;

        let input = "\
name-parts $base = stem

glyph stem 1 1
@@

glyph stem-a 1 1
@@

glyph stem-b 1 1
@@

glyph via-parts
ref $base

glyph via-pattern
ref stem-(a|b)

glyph pair-(a|b)
ref $base

glyph U+2800..2801
ref $base

glyph pipe-a|pipe-b
ref $base
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs = [&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

        for name in [
            "via-parts",
            "via-pattern",
            "pair-a",
            "pair-b",
            "U+2800",
            "U+2801",
            "pipe-a",
            "pipe-b",
        ] {
            assert!(
                resolved
                    .get(name)
                    .is_some_and(|g| g.grid.get(0, 0).is_filled()),
                "{name} did not resolve"
            );
        }
        assert!(is_ref_valid("$base", &resolved, &name_parts));
        assert!(is_ref_valid("stem-(a|b)", &resolved, &name_parts));
    }

    #[test]
    fn adjoin_resolves_minus_before_plus_ref_order() {
        use crate::document_io;

        let input = "\
glyph inner 8 8
................
................
................
................
................
................
................
................
point +center 4 4

glyph outer 12 12
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
point -center 6 6

glyph combo-plus-first
ref inner
ref outer

glyph combo-minus-first
ref outer
ref inner
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

        let pf = resolved.get("combo-plus-first").unwrap();
        let mf = resolved.get("combo-minus-first").unwrap();
        assert_eq!(
            (pf.grid.width, pf.grid.height),
            (mf.grid.width, mf.grid.height),
            "ref order should not affect resolved dimensions"
        );
    }

    #[test]
    fn anchor_range_parsing_and_size_match() {
        use crate::document_io;

        let input = "\
glyph target-wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 1..2 0..1

glyph target-narrow 2 2
@@@@
@@@@
anchor -join 0 0

glyph container 6 2
............
............
anchor +join 3..4 0..1
ref target-wide
ref target-narrow
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

        let container = resolved.get("container").unwrap();
        // target-wide has 2x2 anchor matching +join (2x2): offset = 3-1 = 2, placed at col 2.
        // target-narrow has 1x1 anchor, doesn't match +join (2x2), falls back to (0,0).
        // container own 6px + target-wide 4px at col 2 → max(6, 2+4) = 6.
        // target-narrow 2px at col 0 → still within bounds.
        assert_eq!(container.grid.width, 6);
    }

    #[test]
    fn alternative_glyph_selected_on_size_mismatch() {
        use crate::document_io;

        let input = "\
glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 0..1 0

glyph container 6 2
............
............
anchor +join 3..4 0
ref stem
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let name_parts = NamePartsMap::new();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        // stem has 1x1 anchor, +join is 2x1. stem:wide has 2x1 anchor → matches.
        let container = resolved.get("container").unwrap();
        // stem:wide is 4 wide, placed at col 3-0=3? No: +join col=3..4, -join col=0..1
        // offset = plus.col - minus.col = 3 - 0 = 3
        // container pixels: 6 wide, stem:wide at col 3 → extends to col 7.
        // total width = max(6, 3+4) = 7
        assert_eq!(container.grid.width, 7);

        // Verify via compute_composite that resolved_name is the alternative.
        let container_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
                _ => None,
            })
            .unwrap();
        let composite = compute_composite(container_body, &resolved, &name_parts, &_alt_idx, &Default::default()).unwrap();
        assert_eq!(composite.layers[0].resolved_name, "stem:wide");
    }

    #[test]
    fn alternative_glyph_alphabetical_priority() {
        use crate::document_io;

        let input = "\
glyph base 1 1
@@
anchor -a 0 0

glyph base:zzz 2 2
@@@@
@@@@
anchor -a 0..1 0..1

glyph base:aaa 2 2
@@@@
@@@@
anchor -a 0..1 0..1

glyph host 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +a 1..2 1..2
ref base
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let name_parts = NamePartsMap::new();
        let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        let host_body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "host" => Some(body),
                _ => None,
            })
            .unwrap();
        let composite = compute_composite(host_body, &resolved, &name_parts, &_alt_idx, &Default::default()).unwrap();
        // base:aaa comes before base:zzz alphabetically.
        assert_eq!(composite.layers[0].resolved_name, "base:aaa");
    }

    #[test]
    fn pattern_ref_selects_alternative_by_anchor_size() {
        use crate::document_io;

        let input = "\
name-parts $ab = a b

glyph enclosing 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph a-inner 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 2

glyph b-inner 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 2

glyph b-inner:compressed 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 1..2

glyph ($ab)-combo
ref enclosing
ref ($ab)-inner
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs = [&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

        let b_refs = vec![
            GlyphRef { name: "enclosing".to_string(), offset: None, negated: false, fill: None, visibility: None },
            GlyphRef { name: "b-inner".to_string(), offset: None, negated: false, fill: None, visibility: None },
        ];
        let (effective, _) = derive_ref_offsets_with(
            &[],
            &b_refs,
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
        );
        assert_eq!(
            effective[1].name, "b-inner:compressed",
            "b-inner:compressed should be selected because its -center (1x2) matches +center (1x2)"
        );
    }

    #[test]
    fn overlapping_subpixel_contours_are_correct() {
        use crate::document_io;
        use crate::pixel::PX_SUBPIXEL;
        use crate::render::contour::track_contour_multi;

        // HALF1 (1\, bottom-left triangle) + HALF2 (\1, top-right triangle) = full
        let input = "\
glyph base 1 1
1\\

glyph overlay 1 1
\\1

glyph combined
ref base
ref overlay
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
        let base = &resolved["base"].grid;
        let overlay = &resolved["overlay"].grid;
        let contours = track_contour_multi(
            &[(base, 0, 0), (overlay, 0, 0)],
            PX_SUBPIXEL,
        );
        assert_eq!(contours.len(), 1, "complement halves should form one full-pixel contour");
        let path = &contours[0];
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(1.0, 0.0)));
        assert!(path.contains(&(1.0, 1.0)));
        assert!(path.contains(&(0.0, 1.0)));
    }

    #[test]
    fn own_grid_plus_ref_contours_are_unioned() {
        use crate::document_io;
        use crate::pixel::PX_SUBPIXEL;
        use crate::render::contour::track_contour_multi;

        let input = "\
glyph part 1 1
\\1

glyph host 1 1
1\\
ref part
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
        let host_grid = &resolved["host"].grid;
        let part_grid = &resolved["part"].grid;

        // The own grid has HALF1 at (0,0), the ref has HALF2 at (0,0).
        // track_contour_multi should trace the union as a full pixel.
        let contours = track_contour_multi(
            &[(host_grid, 0, 0), (part_grid, 0, 0)],
            PX_SUBPIXEL,
        );
        assert_eq!(contours.len(), 1);
        let path = &contours[0];
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(1.0, 1.0)));
    }

    #[test]
    fn lookahead_selects_alternative_when_later_ref_consumes_forwarded_anchor() {
        use crate::document_io;

        let input = "\
glyph base:alt 2 2
@@@@
@@@@
anchor +above 1 0
anchor +below 1 1

glyph base 2 4
@@@@
@@@@
....
....
ref base:alt 0 2
anchor +below 1 3

glyph mark-above 2 1 mark
@@@@
anchor -above 1 0

glyph mark-below 2 1 mark
@@@@
anchor -below 1 0

glyph combo-above
ref base
ref mark-above

glyph combo-below
ref base
ref mark-below
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

        let mut decl_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
        for item in &doc.items {
            if let DocumentItem::Glyph { name, body } = item {
                decl_anchors.entry(name.display()).or_insert_with(|| body.points.clone());
            }
        }

        // combo-above: base + mark-above → should substitute base:alt
        // because base's own points lack +above (forwarded from ref base:alt)
        let above_body = doc.items.iter().find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "combo-above" => Some(body),
            _ => None,
        }).unwrap();
        let (effective, _) = derive_ref_offsets_with(
            &above_body.points,
            &above_body.refs,
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| decl_anchors.get(name).cloned(),
        );
        assert_eq!(
            effective[0].name, "base:alt",
            "should select base:alt for mark-above (base's own points lack +above)"
        );

        // combo-below: base + mark-below → should NOT substitute
        // because base's own points include +below
        let below_body = doc.items.iter().find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "combo-below" => Some(body),
            _ => None,
        }).unwrap();
        let (effective, _) = derive_ref_offsets_with(
            &below_body.points,
            &below_body.refs,
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| decl_anchors.get(name).cloned(),
        );
        assert_eq!(
            effective[0].name, "base",
            "should keep base for mark-below (base's own points have +below)"
        );
    }

    #[test]
    fn parse_on_demand_glyph_valid() {
        assert_eq!(parse_on_demand_glyph("3x5"), Some(OnDemandGlyph::Rect(3, 5)));
        assert_eq!(parse_on_demand_glyph("12x34"), Some(OnDemandGlyph::Rect(12, 34)));
        assert_eq!(parse_on_demand_glyph("1x1"), Some(OnDemandGlyph::Rect(1, 1)));
    }

    #[test]
    fn parse_on_demand_glyph_rejects_invalid() {
        assert_eq!(parse_on_demand_glyph("0x5"), None);
        assert_eq!(parse_on_demand_glyph("3x0"), None);
        assert_eq!(parse_on_demand_glyph("03x5"), None);
        assert_eq!(parse_on_demand_glyph("3x05"), None);
        assert_eq!(parse_on_demand_glyph("abc"), None);
        assert_eq!(parse_on_demand_glyph("3"), None);
        assert_eq!(parse_on_demand_glyph("x5"), None);
        assert_eq!(parse_on_demand_glyph("3x"), None);
    }

    fn make_doc(text: &str) -> Document {
        use crate::document_io::{parse_doclines, derive_document};
        let lines = parse_doclines(text);
        let (doc, _) = derive_document(&lines, std::path::PathBuf::new()).unwrap();
        doc
    }

    #[test]
    fn on_demand_glyph_injected_for_ref() {
        let doc = make_doc("glyph test 3 5\n......\n......\n......\n......\n......\n  ref 2x3\n");
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        assert!(cache.contains_key("2x3"));
        let resolved = &cache["2x3"];
        assert_eq!(resolved.grid.width, 2);
        assert_eq!(resolved.grid.height, 3);
        for r in 0..3 {
            for c in 0..2 {
                assert!(resolved.grid.get(r, c).is_filled());
            }
        }
    }

    #[test]
    fn on_demand_glyph_composite_resolves() {
        let doc = make_doc("glyph composite\n  ref 3x2\n");
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        assert!(cache.contains_key("3x2"), "on-demand glyph 3x2 missing from cache");
        assert!(cache.contains_key("composite"), "composite glyph missing from cache");
        let comp = &cache["composite"];
        assert_eq!(comp.grid.width, 3);
        assert_eq!(comp.grid.height, 2);
        for r in 0..2 {
            for c in 0..3 {
                assert!(comp.grid.get(r, c).is_filled(),
                    "composite pixel ({r},{c}) should be filled");
            }
        }
    }

    #[test]
    fn on_demand_glyph_resolves_in_multi_ref_composite() {
        let doc = make_doc(concat!(
            "glyph base 2 2\n@@@@\n@@@@\n",
            "glyph comp\n",
            "  ref base\n",
            "  ref 3x2 2 0\n",
        ));
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        assert!(cache.contains_key("3x2"), "on-demand 3x2 should be in cache");
        assert!(cache.contains_key("comp"), "comp should resolve");
        let comp = &cache["comp"];
        assert!(comp.grid.width >= 5, "composite width should span base(2) + 3x2 at col 2");
    }

    #[test]
    fn on_demand_glyph_not_injected_when_defined() {
        let doc = make_doc("glyph 2x3 2 3\n....\n....\n....\n");
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        let resolved = &cache["2x3"];
        for r in 0..3 {
            for c in 0..2 {
                assert!(!resolved.grid.get(r, c).is_filled());
            }
        }
    }

    #[test]
    fn color_mono_on_demand_glyph_created() {
        let doc = make_doc(concat!(
            "glyph part-a 2 2\n@@@@\n@@@@\n",
            "glyph part-b 2 2\n@@@@\n@@@@\n",
            "glyph test:mono\n  ref part-a\n",
            "glyph test:color\n  ref part-b\n",
            "glyph container\n  ref test\n",
        ));
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        assert!(cache.contains_key("test"), "color/mono on-demand glyph 'test' should be synthesized");
        let resolved = &cache["test"];
        assert_eq!(resolved.grid.width, 2);
        assert_eq!(resolved.grid.height, 2);
    }

    #[test]
    fn color_mono_on_demand_not_created_when_name_contains_mono_or_color() {
        assert_eq!(detect_color_mono_glyph("foo:mono", |_| true), None);
        assert_eq!(detect_color_mono_glyph("foo:color", |_| true), None);
        assert_eq!(detect_color_mono_glyph("foo:mono:bar", |_| true), None);
    }

    #[test]
    fn color_mono_on_demand_not_created_when_defined() {
        let doc = make_doc(concat!(
            "glyph part 2 2\n@@@@\n@@@@\n",
            "glyph test:mono\n  ref part\n",
            "glyph test:color\n  ref part\n",
            "glyph test\n  ref part\n",
            "glyph container\n  ref test\n",
        ));
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        assert!(cache.contains_key("test"));
    }

    #[test]
    fn color_mono_on_demand_not_created_when_only_mono_exists() {
        let doc = make_doc(concat!(
            "glyph part 2 2\n@@@@\n@@@@\n",
            "glyph test:mono\n  ref part\n",
            "glyph container\n  ref test\n",
        ));
        let name_parts = NamePartsMap::new();
        let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

        assert!(!cache.contains_key("test"), "should not synthesize when only :mono exists");
    }
}
